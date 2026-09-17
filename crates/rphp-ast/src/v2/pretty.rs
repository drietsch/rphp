//! Deterministic S-expression printer, used by `--emit=ast` and by snapshot
//! tests.
//!
//! Format: one node per line, children indented two spaces deeper than their
//! parent, closing parentheses on the last child's line:
//!
//! ```text
//! (program file=0
//!   (echo
//!     (binary add
//!       (int 1)
//!       (int 2))))
//! ```
//!
//! A node head is `(kind attr=value ... flag ...)`; attributes are printed in
//! a fixed order and omitted when they hold their default. With
//! [`Options::spans`] every head ends in `@lo..hi`. Identifiers print bare when
//! they match `[A-Za-z0-9_\\]+`, otherwise as quoted strings. Quoted strings
//! escape `"`, `\`, `\n`, `\r`, `\t`, `\0`; every other non-printable byte
//! prints as `\xHH`, so the output is ASCII and byte-lossless. Floats print in
//! Rust `{:?}` form, integers in decimal.

use std::fmt::Write as _;

use rphp_intern::{IdentId, Interner};
use rphp_span::Span;

use super::attr::AttrGroup;
use super::decl::{
    Adaptation, ClassLike, FuncDecl, Hook, HookBody, Member, MethodDecl, Modifiers, Param,
};
use super::expr::{Arg, ArrayItem, CallableTarget, Callee, ConstSel, Expr, InterpPart, NewTarget};
use super::name::{ClassRef, MemberName, Name, NameKind, Resolved};
use super::stmt::{Program, Stmt};
use super::types::{Type, TypeKind};

/// Printer options.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Options {
    /// Append `@lo..hi` to every node head (`--emit=ast --spans`).
    pub spans: bool,
}

/// Print a whole program without spans.
pub fn print(program: &Program, interner: &Interner) -> String {
    print_with(program, interner, Options::default())
}

/// Print a whole program with the given options.
pub fn print_with(program: &Program, interner: &Interner, opts: Options) -> String {
    let mut p = Printer::new(interner, opts);
    p.program(program);
    p.out
}

/// Print a single statement (for tests and diagnostics).
pub fn print_stmt(stmt: &Stmt, interner: &Interner, opts: Options) -> String {
    let mut p = Printer::new(interner, opts);
    p.stmt(stmt);
    p.out
}

/// Print a single expression (for tests and diagnostics).
pub fn print_expr(expr: &Expr, interner: &Interner, opts: Options) -> String {
    let mut p = Printer::new(interner, opts);
    p.expr(expr);
    p.out
}

/// Quote and escape raw bytes as described in the module docs.
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() + 2);
    s.push('"');
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0 => s.push_str("\\0"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                let _ = write!(s, "\\x{b:02x}");
            }
        }
    }
    s.push('"');
    s
}

fn is_bare(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'\\')
}

/// Incremental node-head builder: `kind`, then `k=v` pairs and bare flags.
struct Head(String);

impl Head {
    fn new(kind: &str) -> Self {
        Head(kind.to_owned())
    }

    fn kv(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        let _ = write!(self.0, " {key}={value}");
        self
    }

    fn opt(self, key: &str, value: Option<impl std::fmt::Display>) -> Self {
        match value {
            Some(v) => self.kv(key, v),
            None => self,
        }
    }

    fn flag(mut self, name: &str, on: bool) -> Self {
        if on {
            self.0.push(' ');
            self.0.push_str(name);
        }
        self
    }
}

struct Printer<'a> {
    out: String,
    depth: usize,
    spans: bool,
    it: &'a Interner,
}

impl<'a> Printer<'a> {
    fn new(it: &'a Interner, opts: Options) -> Self {
        Self {
            out: String::new(),
            depth: 0,
            spans: opts.spans,
            it,
        }
    }

    // ----- low-level emission --------------------------------------------

    fn open(&mut self, head: &str, span: Option<Span>) {
        if !self.out.is_empty() {
            self.out.push('\n');
            for _ in 0..self.depth {
                self.out.push_str("  ");
            }
        }
        self.out.push('(');
        self.out.push_str(head);
        if self.spans {
            if let Some(s) = span {
                let _ = write!(self.out, " @{}..{}", s.lo, s.hi);
            }
        }
        self.depth += 1;
    }

    fn close(&mut self) {
        self.depth -= 1;
        self.out.push(')');
    }

    fn leaf(&mut self, head: &str, span: Option<Span>) {
        self.open(head, span);
        self.close();
    }

    fn node(&mut self, head: &str, span: Option<Span>, f: impl FnOnce(&mut Self)) {
        self.open(head, span);
        f(self);
        self.close();
    }

    fn ident(&self, id: IdentId) -> String {
        let b = self.it.resolve(id);
        if is_bare(b) {
            String::from_utf8_lossy(b).into_owned()
        } else {
            escape_bytes(b)
        }
    }

    fn var(&self, id: IdentId) -> String {
        format!("${}", self.ident(id))
    }

    fn quoted(&self, id: IdentId) -> String {
        escape_bytes(self.it.resolve(id))
    }

    fn doc(&self, doc: Option<IdentId>) -> Option<String> {
        doc.map(|d| self.quoted(d))
    }

    fn mods(m: &Modifiers) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = m.vis {
            parts.push(v.as_str().to_owned());
        }
        if let Some(v) = m.set_vis {
            parts.push(format!("{}(set)", v.as_str()));
        }
        if m.static_ {
            parts.push("static".into());
        }
        if m.readonly {
            parts.push("readonly".into());
        }
        if m.final_ {
            parts.push("final".into());
        }
        if m.abstract_ {
            parts.push("abstract".into());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(","))
        }
    }

    /// `text kind=k [resolved=...]` — the attribute tail shared by every node
    /// that embeds a [`Name`].
    fn name_attrs(&self, n: &Name) -> String {
        let kind = match n.kind {
            NameKind::Unqualified => "u",
            NameKind::Qualified => "q",
            NameKind::FullyQualified => "fq",
            NameKind::Relative => "rel",
        };
        let mut s = format!("{} kind={kind}", self.ident(n.text));
        match n.resolved {
            None => {}
            Some(Resolved::Class { fqn, key }) => {
                let _ = write!(
                    s,
                    " resolved=class fqn={} key={}",
                    self.ident(fqn),
                    self.ident(key)
                );
            }
            Some(Resolved::Func { ns_key, global_key }) => {
                s.push_str(" resolved=func");
                if let Some(k) = ns_key {
                    let _ = write!(s, " ns={}", self.ident(k));
                }
                let _ = write!(s, " global={}", self.ident(global_key));
            }
            Some(Resolved::Const { ns_key, global_key }) => {
                s.push_str(" resolved=const");
                if let Some(k) = ns_key {
                    let _ = write!(s, " ns={}", self.ident(k));
                }
                let _ = write!(s, " global={}", self.ident(global_key));
            }
        }
        s
    }

    fn name(&mut self, n: &Name) {
        let head = format!("name {}", self.name_attrs(n));
        self.leaf(&head, Some(n.span));
    }

    fn names(&mut self, wrapper: &str, names: &[Name]) {
        if names.is_empty() {
            return;
        }
        self.node(wrapper, None, |p| {
            for n in names {
                p.name(n);
            }
        });
    }

    // ----- program / statements ------------------------------------------

    fn program(&mut self, prog: &Program) {
        let head = Head::new("program")
            .kv("file", prog.file.0)
            .flag("strict", prog.strict_types)
            .opt("halt", prog.halt_offset);
        self.node(&head.0, None, |p| p.stmts(&prog.items));
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn block(&mut self, wrapper: &str, stmts: &[Stmt]) {
        self.node(wrapper, None, |p| p.stmts(stmts));
    }

    fn exprs(&mut self, wrapper: &str, exprs: &[Expr]) {
        self.node(wrapper, None, |p| {
            for e in exprs {
                p.expr(e);
            }
        });
    }

    fn stmt(&mut self, s: &Stmt) {
        let sp = Some(s.span());
        match s {
            Stmt::InlineHtml { text, .. } => {
                let head = format!("inline-html {}", self.quoted(*text));
                self.leaf(&head, sp);
            }
            Stmt::Expr { expr, .. } => self.node("expr-stmt", sp, |p| p.expr(expr)),
            Stmt::Echo { args, .. } => self.node("echo", sp, |p| {
                for a in args {
                    p.expr(a);
                }
            }),
            Stmt::Block { body, .. } => self.node("block", sp, |p| p.stmts(body)),
            Stmt::If {
                cond,
                then,
                elseifs,
                else_,
                ..
            } => self.node("if", sp, |p| {
                p.expr(cond);
                p.block("then", then);
                for e in elseifs {
                    p.node("elseif", Some(e.span), |p| {
                        p.expr(&e.cond);
                        p.block("body", &e.body);
                    });
                }
                if let Some(e) = else_ {
                    p.block("else", e);
                }
            }),
            Stmt::While { cond, body, .. } => self.node("while", sp, |p| {
                p.expr(cond);
                p.block("body", body);
            }),
            Stmt::DoWhile { body, cond, .. } => self.node("do-while", sp, |p| {
                p.block("body", body);
                p.expr(cond);
            }),
            Stmt::For {
                init,
                cond,
                step,
                body,
                ..
            } => self.node("for", sp, |p| {
                p.exprs("init", init);
                p.exprs("cond", cond);
                p.exprs("step", step);
                p.block("body", body);
            }),
            Stmt::Foreach {
                subject,
                key,
                value,
                by_ref,
                body,
                ..
            } => {
                let head = Head::new("foreach").flag("by-ref", *by_ref);
                self.node(&head.0, sp, |p| {
                    p.expr(subject);
                    if let Some(k) = key {
                        p.node("key", None, |p| p.expr(k));
                    }
                    p.node("value", None, |p| p.expr(value));
                    p.block("body", body);
                });
            }
            Stmt::Switch { subject, cases, .. } => self.node("switch", sp, |p| {
                p.expr(subject);
                for c in cases {
                    match &c.cond {
                        Some(cond) => p.node("case", Some(c.span), |p| {
                            p.expr(cond);
                            p.stmts(&c.body);
                        }),
                        None => p.node("default", Some(c.span), |p| p.stmts(&c.body)),
                    }
                }
            }),
            Stmt::Break { levels, .. } => self.leaf(&format!("break {levels}"), sp),
            Stmt::Continue { levels, .. } => self.leaf(&format!("continue {levels}"), sp),
            Stmt::Return { value, .. } => self.node("return", sp, |p| {
                if let Some(v) = value {
                    p.expr(v);
                }
            }),
            Stmt::Try {
                body,
                catches,
                finally,
                ..
            } => self.node("try", sp, |p| {
                p.block("body", body);
                for c in catches {
                    let head = Head::new("catch").opt("var", c.var.map(|v| p.var(v)));
                    p.node(&head.0, Some(c.span), |p| {
                        for t in &c.types {
                            p.name(t);
                        }
                        p.block("body", &c.body);
                    });
                }
                if let Some(f) = finally {
                    p.block("finally", f);
                }
            }),
            Stmt::Goto { label, .. } => self.leaf(&format!("goto {}", self.ident(*label)), sp),
            Stmt::Label { name, .. } => self.leaf(&format!("label {}", self.ident(*name)), sp),
            Stmt::Global { vars, .. } => self.node("global", sp, |p| {
                for v in vars {
                    p.expr(v);
                }
            }),
            Stmt::StaticVar { vars, .. } => self.node("static-var", sp, |p| {
                for v in vars {
                    let head = format!("item {}", p.var(v.name));
                    p.node(&head, Some(v.span), |p| {
                        if let Some(i) = &v.init {
                            p.expr(i);
                        }
                    });
                }
            }),
            Stmt::Unset { targets, .. } => self.node("unset", sp, |p| {
                for t in targets {
                    p.expr(t);
                }
            }),
            Stmt::Declare {
                directives, body, ..
            } => self.node("declare", sp, |p| {
                for d in directives {
                    let head = format!("directive {}", p.ident(d.name));
                    p.node(&head, Some(d.span), |p| p.expr(&d.value));
                }
                if let Some(b) = body {
                    p.block("body", b);
                }
            }),
            Stmt::Namespace {
                name, body, braced, ..
            } => {
                let head = Head::new("namespace").flag("braced", *braced);
                self.node(&head.0, sp, |p| {
                    if let Some(n) = name {
                        p.name(n);
                    }
                    p.stmts(body);
                });
            }
            Stmt::Use {
                kind,
                prefix,
                items,
                ..
            } => {
                let head = Head::new("use").kv("kind", kind.as_str());
                self.node(&head.0, sp, |p| {
                    if let Some(pre) = prefix {
                        p.node("prefix", None, |p| p.name(pre));
                    }
                    for it in items {
                        let head = Head::new("item")
                            .opt("alias", it.alias.map(|a| p.ident(a)))
                            .opt("kind", it.kind.map(|k| k.as_str()));
                        p.node(&head.0, Some(it.span), |p| p.name(&it.name));
                    }
                });
            }
            Stmt::ConstDecl { attrs, items, .. } => self.node("const-decl", sp, |p| {
                p.attrs(attrs);
                for it in items {
                    let head = format!("item {}", p.ident(it.name));
                    p.node(&head, Some(it.span), |p| p.expr(&it.value));
                }
            }),
            Stmt::Func(f) => self.func_decl(f),
            Stmt::ClassLike(c) => self.class_like(c),
            Stmt::HaltCompiler { .. } => self.leaf("halt-compiler", sp),
            Stmt::Nop { .. } => self.leaf("nop", sp),
        }
    }

    // ----- declarations ----------------------------------------------------

    fn attrs(&mut self, groups: &[AttrGroup]) {
        for g in groups {
            self.node("attr-group", Some(g.span), |p| {
                for a in &g.attrs {
                    let head = format!("attr {}", p.name_attrs(&a.name));
                    p.node(&head, Some(a.span), |p| p.args(&a.args));
                }
            });
        }
    }

    fn params(&mut self, params: &[Param]) {
        self.node("params", None, |p| {
            for prm in params {
                p.param(prm);
            }
        });
    }

    fn param(&mut self, prm: &Param) {
        let head = Head::new(&format!("param {}", self.var(prm.name)))
            .flag("by-ref", prm.by_ref)
            .flag("variadic", prm.variadic)
            .opt(
                "promote",
                prm.promote
                    .as_ref()
                    .map(|m| Self::mods(m).unwrap_or_default()),
            );
        self.node(&head.0, Some(prm.span), |p| {
            p.attrs(&prm.attrs);
            if let Some(t) = &prm.ty {
                p.ty(t);
            }
            if let Some(d) = &prm.default {
                p.node("default", None, |p| p.expr(d));
            }
            for h in &prm.hooks {
                p.hook(h);
            }
        });
    }

    fn ret(&mut self, ret: Option<&Type>) {
        if let Some(t) = ret {
            self.node("ret", None, |p| p.ty(t));
        }
    }

    fn ty(&mut self, t: &Type) {
        let sp = Some(t.span);
        match &t.kind {
            TypeKind::Named(n) => {
                let head = format!("type-name {}", self.name_attrs(n));
                self.leaf(&head, sp);
            }
            TypeKind::Builtin(b) => self.leaf(&format!("type {}", b.as_str()), sp),
            TypeKind::Nullable(inner) => self.node("nullable", sp, |p| p.ty(inner)),
            TypeKind::Union(ts) => self.node("union", sp, |p| {
                for t in ts {
                    p.ty(t);
                }
            }),
            TypeKind::Intersection(ts) => self.node("intersection", sp, |p| {
                for t in ts {
                    p.ty(t);
                }
            }),
        }
    }

    fn hook(&mut self, h: &Hook) {
        let head = Head::new(&format!("hook {}", h.kind.as_str()))
            .flag("final", h.final_)
            .flag("by-ref", h.by_ref);
        self.node(&head.0, Some(h.span), |p| {
            p.attrs(&h.attrs);
            if let Some(params) = &h.params {
                p.params(params);
            }
            match &h.body {
                HookBody::Expr(e) => p.node("expr", None, |p| p.expr(e)),
                HookBody::Block(b) => p.block("body", b),
                HookBody::Abstract => p.leaf("abstract", None),
            }
        });
    }

    fn func_decl(&mut self, f: &FuncDecl) {
        let head = Head::new(&format!("func {}", self.ident(f.name)))
            .flag("by-ref", f.by_ref)
            .opt("doc", self.doc(f.doc));
        self.node(&head.0, Some(f.span), |p| {
            p.attrs(&f.attrs);
            p.params(&f.params);
            p.ret(f.ret.as_ref());
            p.block("body", &f.body);
        });
    }

    fn method(&mut self, m: &MethodDecl) {
        let head = Head::new(&format!("method {}", self.ident(m.name)))
            .opt("mods", Self::mods(&m.modifiers))
            .flag("by-ref", m.by_ref)
            .opt("doc", self.doc(m.doc));
        self.node(&head.0, Some(m.span), |p| {
            p.attrs(&m.attrs);
            p.params(&m.params);
            p.ret(m.ret.as_ref());
            match &m.body {
                Some(b) => p.block("body", b),
                None => p.leaf("abstract", None),
            }
        });
    }

    fn class_like(&mut self, c: &ClassLike) {
        let head = Head::new("class-like")
            .kv("kind", c.kind.as_str())
            .opt("name", c.name.map(|n| self.ident(n)))
            .opt("mods", Self::mods(&c.modifiers))
            .opt("doc", self.doc(c.doc));
        self.node(&head.0, Some(c.span), |p| {
            p.attrs(&c.attrs);
            p.names("extends", &c.extends);
            p.names("implements", &c.implements);
            if let Some(b) = &c.backing {
                p.node("backing", None, |p| p.ty(b));
            }
            for m in &c.members {
                p.member(m);
            }
        });
    }

    fn member(&mut self, m: &Member) {
        match m {
            Member::Const(c) => {
                let head = Head::new("const")
                    .opt("mods", Self::mods(&c.modifiers))
                    .opt("doc", self.doc(c.doc));
                self.node(&head.0, Some(c.span), |p| {
                    p.attrs(&c.attrs);
                    if let Some(t) = &c.ty {
                        p.ty(t);
                    }
                    for it in &c.items {
                        let head = format!("item {}", p.ident(it.name));
                        p.node(&head, Some(it.span), |p| p.expr(&it.value));
                    }
                });
            }
            Member::Prop(pr) => {
                let head = Head::new("prop")
                    .opt("mods", Self::mods(&pr.modifiers))
                    .opt("doc", self.doc(pr.doc));
                self.node(&head.0, Some(pr.span), |p| {
                    p.attrs(&pr.attrs);
                    if let Some(t) = &pr.ty {
                        p.ty(t);
                    }
                    for it in &pr.items {
                        let head = format!("item {}", p.var(it.name));
                        p.node(&head, Some(it.span), |p| {
                            if let Some(d) = &it.default {
                                p.expr(d);
                            }
                        });
                    }
                    for h in &pr.hooks {
                        p.hook(h);
                    }
                });
            }
            Member::Method(m) => self.method(m),
            Member::EnumCase(c) => {
                let head =
                    Head::new(&format!("case {}", self.ident(c.name))).opt("doc", self.doc(c.doc));
                self.node(&head.0, Some(c.span), |p| {
                    p.attrs(&c.attrs);
                    if let Some(v) = &c.value {
                        p.expr(v);
                    }
                });
            }
            Member::TraitUse(t) => self.node("trait-use", Some(t.span), |p| {
                for n in &t.traits {
                    p.name(n);
                }
                for a in &t.adaptations {
                    match a {
                        Adaptation::Precedence {
                            trait_,
                            method,
                            insteadof,
                            span,
                        } => {
                            let head = format!("precedence method={}", p.ident(*method));
                            p.node(&head, Some(*span), |p| {
                                p.name(trait_);
                                p.names("insteadof", insteadof);
                            });
                        }
                        Adaptation::Alias {
                            trait_,
                            method,
                            alias,
                            vis,
                            span,
                        } => {
                            let head = Head::new("alias")
                                .kv("method", p.ident(*method))
                                .opt("alias", alias.map(|a| p.ident(a)))
                                .opt("vis", vis.map(|v| v.as_str()));
                            p.node(&head.0, Some(*span), |p| {
                                if let Some(t) = trait_ {
                                    p.name(t);
                                }
                            });
                        }
                    }
                }
            }),
        }
    }

    // ----- expressions -----------------------------------------------------

    fn args(&mut self, args: &[Arg]) {
        for a in args {
            let head = Head::new("arg")
                .opt("name", a.name.map(|n| self.ident(n)))
                .flag("spread", a.spread);
            self.node(&head.0, Some(a.span), |p| p.expr(&a.value));
        }
    }

    fn class_ref(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Named(n) => self.name(n),
            ClassRef::SelfKw(s) => self.leaf("self", Some(*s)),
            ClassRef::Static(s) => self.leaf("static", Some(*s)),
            ClassRef::Parent(s) => self.leaf("parent", Some(*s)),
            ClassRef::Expr(e) => self.node("class-expr", None, |p| p.expr(e)),
        }
    }

    fn callee(&mut self, c: &Callee) {
        match c {
            Callee::Name(n) => self.name(n),
            Callee::Expr(e) => self.expr(e),
        }
    }

    /// The `name=x` attribute for a literal member name; `None` for a computed
    /// one (printed as a `(name-expr ...)` child by [`Self::member_name_child`]).
    fn member_attr(&self, m: &MemberName) -> Option<String> {
        m.as_ident().map(|id| self.ident(id))
    }

    fn member_name_child(&mut self, m: &MemberName) {
        if let MemberName::Expr(e) = m {
            self.node("name-expr", None, |p| p.expr(e));
        }
    }

    fn interp(&mut self, head: &str, parts: &[InterpPart], span: Span) {
        self.node(head, Some(span), |p| {
            for part in parts {
                match part {
                    InterpPart::Lit(id, s) => {
                        let head = format!("lit {}", p.quoted(*id));
                        p.leaf(&head, Some(*s));
                    }
                    InterpPart::Expr(e) => p.expr(e),
                }
            }
        });
    }

    fn array_item(&mut self, it: &ArrayItem) {
        let head = Head::new("item")
            .flag("by-ref", it.by_ref)
            .flag("spread", it.spread);
        self.node(&head.0, Some(it.span), |p| {
            if let Some(k) = &it.key {
                p.node("key", None, |p| p.expr(k));
            }
            match &it.value {
                Some(v) => p.expr(v),
                None => p.leaf("skip", None),
            }
        });
    }

    fn expr(&mut self, e: &Expr) {
        let sp = Some(e.span());
        match e {
            Expr::Null(_) => self.leaf("null", sp),
            Expr::Bool(b, _) => self.leaf(&format!("bool {b}"), sp),
            Expr::Int(i, _) => self.leaf(&format!("int {i}"), sp),
            Expr::Float(f, _) => self.leaf(&format!("float {f:?}"), sp),
            Expr::Str(s, _) => self.leaf(&format!("str {}", self.quoted(*s)), sp),
            Expr::Interp { parts, span } => self.interp("interp", parts, *span),
            Expr::ShellExec { parts, span } => self.interp("shell-exec", parts, *span),
            Expr::Var(v, _) => self.leaf(&format!("var {}", self.var(*v)), sp),
            Expr::VarVar { name, .. } => self.node("var-var", sp, |p| p.expr(name)),
            Expr::Array { items, syntax, .. } => {
                let head = Head::new("array").kv("syntax", syntax.as_str());
                self.node(&head.0, sp, |p| {
                    for it in items {
                        p.array_item(it);
                    }
                });
            }
            Expr::Index { base, index, .. } => {
                let head = Head::new("index").flag("append", index.is_none());
                self.node(&head.0, sp, |p| {
                    p.expr(base);
                    if let Some(i) = index {
                        p.expr(i);
                    }
                });
            }
            Expr::Prop {
                obj,
                name,
                nullsafe,
                ..
            } => {
                let head = Head::new("prop")
                    .opt("name", self.member_attr(name))
                    .flag("nullsafe", *nullsafe);
                self.node(&head.0, sp, |p| {
                    p.expr(obj);
                    p.member_name_child(name);
                });
            }
            Expr::StaticProp { class, name, .. } => {
                let head = Head::new("static-prop").opt("name", self.member_attr(name));
                self.node(&head.0, sp, |p| {
                    p.class_ref(class);
                    p.member_name_child(name);
                });
            }
            Expr::ClassConst { class, name, .. } => {
                let sel = match name {
                    ConstSel::Ident(id, _) => Some(self.ident(*id)),
                    ConstSel::Class(_) => Some("class".to_owned()),
                    ConstSel::Expr(_) => None,
                };
                let head = Head::new("class-const").opt("const", sel);
                self.node(&head.0, sp, |p| {
                    p.class_ref(class);
                    if let ConstSel::Expr(e) = name {
                        p.node("const-expr", None, |p| p.expr(e));
                    }
                });
            }
            Expr::Const(n) => {
                let head = format!("const {}", self.name_attrs(n));
                self.leaf(&head, sp);
            }
            Expr::MagicConst { kind, .. } => self.leaf(&format!("magic {}", kind.as_str()), sp),
            Expr::Call { callee, args, .. } => self.node("call", sp, |p| {
                p.callee(callee);
                p.args(args);
            }),
            Expr::MethodCall {
                obj,
                name,
                args,
                nullsafe,
                ..
            } => {
                let head = Head::new("method-call")
                    .opt("name", self.member_attr(name))
                    .flag("nullsafe", *nullsafe);
                self.node(&head.0, sp, |p| {
                    p.expr(obj);
                    p.member_name_child(name);
                    p.args(args);
                });
            }
            Expr::StaticCall {
                class, name, args, ..
            } => {
                let head = Head::new("static-call").opt("name", self.member_attr(name));
                self.node(&head.0, sp, |p| {
                    p.class_ref(class);
                    p.member_name_child(name);
                    p.args(args);
                });
            }
            Expr::Callable { target, .. } => match target {
                CallableTarget::Func(c) => self.node("callable", sp, |p| p.callee(c)),
                CallableTarget::Method { obj, name } => {
                    let head = Head::new("callable-method").opt("name", self.member_attr(name));
                    self.node(&head.0, sp, |p| {
                        p.expr(obj);
                        p.member_name_child(name);
                    });
                }
                CallableTarget::Static { class, name } => {
                    let head = Head::new("callable-static").opt("name", self.member_attr(name));
                    self.node(&head.0, sp, |p| {
                        p.class_ref(class);
                        p.member_name_child(name);
                    });
                }
            },
            Expr::New { class, args, .. } => self.node("new", sp, |p| {
                match class {
                    NewTarget::Ref(c) => p.class_ref(c),
                    NewTarget::Anon(c) => p.class_like(c),
                }
                p.args(args);
            }),
            Expr::Clone { expr, with, .. } => self.node("clone", sp, |p| {
                p.expr(expr);
                if let Some(w) = with {
                    p.node("with", None, |p| p.expr(w));
                }
            }),
            Expr::Unary { op, expr, .. } => {
                self.node(&format!("unary {}", op.name()), sp, |p| p.expr(expr));
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                self.node(&format!("binary {}", op.name()), sp, |p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            Expr::Assign {
                target,
                value,
                op,
                by_ref,
                ..
            } => {
                let head = Head::new("assign")
                    .opt("op", op.map(|o| o.name()))
                    .flag("by-ref", *by_ref);
                self.node(&head.0, sp, |p| {
                    p.expr(target);
                    p.expr(value);
                });
            }
            Expr::Ternary {
                cond, then, else_, ..
            } => {
                let head = Head::new("ternary").flag("short", then.is_none());
                self.node(&head.0, sp, |p| {
                    p.expr(cond);
                    if let Some(t) = then {
                        p.expr(t);
                    }
                    p.expr(else_);
                });
            }
            Expr::Isset { vars, .. } => self.node("isset", sp, |p| {
                for v in vars {
                    p.expr(v);
                }
            }),
            Expr::Empty { expr, .. } => self.node("empty", sp, |p| p.expr(expr)),
            Expr::Include { kind, path, .. } => {
                let head = Head::new("include").kv("kind", kind.as_str());
                self.node(&head.0, sp, |p| p.expr(path));
            }
            Expr::Eval { code, .. } => self.node("eval", sp, |p| p.expr(code)),
            Expr::Exit { arg, .. } => self.node("exit", sp, |p| {
                if let Some(a) = arg {
                    p.expr(a);
                }
            }),
            Expr::Print { expr, .. } => self.node("print", sp, |p| p.expr(expr)),
            Expr::Closure(c) => {
                let head = Head::new("closure")
                    .flag("static", c.static_)
                    .flag("by-ref", c.by_ref)
                    .opt("doc", self.doc(c.doc));
                self.node(&head.0, sp, |p| {
                    p.attrs(&c.attrs);
                    p.params(&c.params);
                    p.node("uses", None, |p| {
                        for u in &c.uses {
                            let head = Head::new(&format!("use {}", p.var(u.name)))
                                .flag("by-ref", u.by_ref);
                            p.leaf(&head.0, Some(u.span));
                        }
                    });
                    p.ret(c.ret.as_ref());
                    p.block("body", &c.body);
                });
            }
            Expr::ArrowFn(f) => {
                let head = Head::new("arrow-fn")
                    .flag("static", f.static_)
                    .flag("by-ref", f.by_ref)
                    .opt("doc", self.doc(f.doc));
                self.node(&head.0, sp, |p| {
                    p.attrs(&f.attrs);
                    p.params(&f.params);
                    p.ret(f.ret.as_ref());
                    p.node("body", None, |p| p.expr(&f.body));
                });
            }
            Expr::Match { subject, arms, .. } => self.node("match", sp, |p| {
                p.expr(subject);
                for arm in arms {
                    let head = Head::new("arm").flag("default", arm.conds.is_none());
                    p.node(&head.0, Some(arm.span), |p| {
                        if let Some(conds) = &arm.conds {
                            p.exprs("conds", conds);
                        }
                        p.expr(&arm.body);
                    });
                }
            }),
            Expr::Throw { expr, .. } => self.node("throw", sp, |p| p.expr(expr)),
            Expr::Yield { key, value, .. } => self.node("yield", sp, |p| {
                if let Some(k) = key {
                    p.node("key", None, |p| p.expr(k));
                }
                if let Some(v) = value {
                    p.node("value", None, |p| p.expr(v));
                }
            }),
            Expr::YieldFrom { expr, .. } => self.node("yield-from", sp, |p| p.expr(expr)),
            Expr::InstanceOf { expr, class, .. } => self.node("instanceof", sp, |p| {
                p.expr(expr);
                p.class_ref(class);
            }),
            Expr::Let {
                temp, init, body, ..
            } => {
                self.node(&format!("let t{}", temp.0), sp, |p| {
                    p.expr(init);
                    p.expr(body);
                });
            }
            Expr::Temp(t, _) => self.leaf(&format!("temp t{}", t.0), sp),
            Expr::Seq(exprs, _) => self.node("seq", sp, |p| {
                for e in exprs {
                    p.expr(e);
                }
            }),
            Expr::Error(_) => self.leaf("error", sp),
        }
    }
}
