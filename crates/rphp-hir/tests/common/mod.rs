//! Shared helpers: parse a snippet with the mago adapter, lower it, and
//! query the result.
#![allow(dead_code)]

use std::path::PathBuf;

use rphp_ast::v2::visit::{walk_expr, Visitor};
use rphp_ast::v2::{pretty, Callee, ClassRef, Expr, Name, Program, Resolved, Type, TypeKind};
use rphp_diagnostics::Diagnostic;
use rphp_hir::{Hir, LowerOptions};
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_source::SourceMap;
use rphp_span::FileId;

/// The outcome of lowering a snippet.
pub struct Lowered {
    pub hir: Hir,
    pub diags: Vec<Diagnostic>,
    pub interner: Interner,
    pub sources: SourceMap,
}

impl Lowered {
    pub fn print(&self) -> String {
        rphp_hir::print(&self.hir, &self.interner)
    }

    pub fn program(&self) -> &Program {
        self.hir.program()
    }

    pub fn errors(&self) -> Vec<String> {
        self.diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| format!("{} {}", d.code, d.message))
            .collect()
    }

    pub fn messages(&self) -> Vec<String> {
        self.diags.iter().map(|d| d.message.clone()).collect()
    }

    pub fn codes(&self) -> Vec<&'static str> {
        self.diags.iter().map(|d| d.code).collect()
    }

    pub fn text(&self, id: rphp_intern::IdentId) -> String {
        self.interner.resolve_lossy(id).into_owned()
    }

    /// `(fqn, key)` of a resolved class name.
    pub fn class(&self, n: &Name) -> (String, String) {
        match n.resolved {
            Some(Resolved::Class { fqn, key }) => (self.text(fqn), self.text(key)),
            other => panic!("not a class resolution: {other:?}"),
        }
    }

    /// `(ns_key, global_key)` of a resolved function name.
    pub fn func(&self, n: &Name) -> (Option<String>, String) {
        match n.resolved {
            Some(Resolved::Func { ns_key, global_key }) => {
                (ns_key.map(|k| self.text(k)), self.text(global_key))
            }
            other => panic!("not a function resolution: {other:?}"),
        }
    }

    /// `(ns_key, global_key)` of a resolved constant name.
    pub fn konst(&self, n: &Name) -> (Option<String>, String) {
        match n.resolved {
            Some(Resolved::Const { ns_key, global_key }) => {
                (ns_key.map(|k| self.text(k)), self.text(global_key))
            }
            other => panic!("not a constant resolution: {other:?}"),
        }
    }

    /// Every class-position name in the tree, in source order.
    pub fn class_names(&self) -> Vec<(String, String)> {
        let mut c = Collect::default();
        c.visit_program(self.program());
        c.classes.iter().map(|n| self.class(n)).collect()
    }

    /// Every function callee name, in source order.
    pub fn func_names(&self) -> Vec<(Option<String>, String)> {
        let mut c = Collect::default();
        c.visit_program(self.program());
        c.funcs.iter().map(|n| self.func(n)).collect()
    }

    /// Every constant fetch, in source order.
    pub fn const_names(&self) -> Vec<(Option<String>, String)> {
        let mut c = Collect::default();
        c.visit_program(self.program());
        c.consts.iter().map(|n| self.konst(n)).collect()
    }

    /// Every string literal in `echo` statements, in order (magic constants
    /// and `::class` fold to strings).
    pub fn echoed(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect_echo(&self.program().items, &self.interner, &mut out);
        out
    }
}

fn collect_echo(stmts: &[rphp_ast::v2::Stmt], it: &Interner, out: &mut Vec<String>) {
    use rphp_ast::v2::Stmt;
    for s in stmts {
        match s {
            Stmt::Echo { args, .. } => {
                for a in args {
                    out.push(match a {
                        Expr::Str(id, _) => it.resolve_lossy(*id).into_owned(),
                        Expr::Int(i, _) => i.to_string(),
                        Expr::MagicConst { kind, .. } => format!("<{}>", kind.as_str()),
                        other => format!("<{}>", pretty::print_expr(other, it, Default::default())),
                    });
                }
            }
            Stmt::Namespace { body, .. } | Stmt::Block { body, .. } => collect_echo(body, it, out),
            Stmt::Func(f) => collect_echo(&f.body, it, out),
            Stmt::ClassLike(c) => {
                for m in &c.members {
                    if let rphp_ast::v2::Member::Method(md) = m {
                        if let Some(b) = &md.body {
                            collect_echo(b, it, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Collect {
    classes: Vec<Name>,
    funcs: Vec<Name>,
    consts: Vec<Name>,
}

impl Visitor for Collect {
    fn visit_stmt(&mut self, s: &rphp_ast::v2::Stmt) {
        use rphp_ast::v2::Stmt;
        match s {
            Stmt::Use { .. } => {}
            Stmt::Namespace { body, .. } => self.visit_block(body),
            _ => rphp_ast::v2::visit::walk_stmt(self, s),
        }
    }
    fn visit_name(&mut self, n: &Name) {
        self.classes.push(n.clone());
    }
    fn visit_type(&mut self, t: &Type) {
        if let TypeKind::Named(n) = &t.kind {
            self.classes.push(n.clone());
        } else {
            rphp_ast::v2::visit::walk_type(self, t);
        }
    }
    fn visit_class_ref(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Named(n) => self.classes.push(n.clone()),
            ClassRef::Expr(e) => self.visit_expr(e),
            _ => {}
        }
    }
    fn visit_callee(&mut self, c: &Callee) {
        match c {
            Callee::Name(n) => self.funcs.push(n.clone()),
            Callee::Expr(e) => self.visit_expr(e),
        }
    }
    fn visit_expr(&mut self, e: &Expr) {
        if let Expr::Const(n) = e {
            self.consts.push(n.clone());
            return;
        }
        walk_expr(self, e);
    }
}

/// Parse and lower `src` as the file `/tmp/t.php` (the path php would print).
pub fn lower(src: &str) -> Lowered {
    lower_with(src, Some(PathBuf::from("/tmp/t.php")), None)
}

/// Parse and lower with explicit naming options.
pub fn lower_with(src: &str, file: Option<PathBuf>, eval_name: Option<&str>) -> Lowered {
    let mut sources = SourceMap::new();
    let fid = sources.add("t.php", src.as_bytes());
    let mut interner = Interner::new();
    let parsed = parse_v2(src.as_bytes(), ParseOptions::new(fid), &mut interner);
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diagnostics for {src:?}: {:?}",
        parsed.diagnostics
    );
    let line_of = |s: rphp_span::Span| sources.get(FileId(s.file.0)).line_col(s.lo).0;
    let mut opts = LowerOptions::new(&line_of);
    opts.file = file;
    opts.eval_name = eval_name.map(str::to_owned);
    let (hir, diags) = rphp_hir::lower(parsed.program, &mut interner, &opts);
    drop(opts);
    Lowered {
        hir,
        diags,
        interner,
        sources,
    }
}

/// Lower a snippet that the *adapter* may already reject (the HIR pass still
/// runs, its own diagnostics are returned separately from the parser's).
pub fn lower_lenient(src: &str) -> (Lowered, Vec<Diagnostic>) {
    let mut sources = SourceMap::new();
    let fid = sources.add("t.php", src.as_bytes());
    let mut interner = Interner::new();
    let parsed = parse_v2(src.as_bytes(), ParseOptions::new(fid), &mut interner);
    let line_of = |s: rphp_span::Span| sources.get(FileId(s.file.0)).line_col(s.lo).0;
    let mut opts = LowerOptions::new(&line_of);
    opts.file = Some(PathBuf::from("/tmp/t.php"));
    let (hir, diags) = rphp_hir::lower(parsed.program, &mut interner, &opts);
    drop(opts);
    (
        Lowered {
            hir,
            diags,
            interner,
            sources,
        },
        parsed.diagnostics,
    )
}
