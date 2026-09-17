//! Compile-time validation that needs whole-body context, with php 8.5's
//! messages:
//!
//! * `goto`/labels are per function body (closures, methods and nested
//!   functions have their own). An undefined target is `'goto' to undefined
//!   label 'x'` (`RPHP_E0202`), a duplicate label `Label 'x' already defined`
//!   (`RPHP_E0202`). A jump may leave loops and switches but never enter one
//!   the goto is not inside (`'goto' into loop or switch statement is
//!   disallowed`, `RPHP_E0203`), and it may not cross a `finally` boundary
//!   in either direction (`jump out of a finally block is disallowed`,
//!   `jump into a finally block is disallowed`).
//! * `break`/`continue` levels against the enclosing loop/switch count
//!   (`'break' not in the 'loop' or 'switch' context`, `Cannot 'break' 2
//!   levels`; `RPHP_E0203`). The adapter checks this too; the pass repeats it
//!   so a tree built by other means is validated.
//! * A `const` declaration inside a function body is a parse error in php
//!   (`syntax error, unexpected token "const"`, reported as `RPHP_E0011`).
//! * Constructor promotion outside `__construct` (`Cannot declare promoted
//!   property outside a constructor`, `RPHP_E0011`; the adapter's rule,
//!   repeated for the same reason).
//!
//! Static variable initializers accept any expression (PHP 8.3).

use rphp_ast::v2::visit::{walk_expr, walk_stmt, Visitor};
use rphp_ast::v2::{ClassLike, Expr, FuncDecl, Hook, HookBody, Member, Param, Program, Stmt};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;

/// Run the validation pass.
pub(crate) fn run(program: &Program, interner: &Interner, diags: &mut Vec<Diagnostic>) {
    let mut v = Validator {
        it: interner,
        diags,
        body: Body::default(),
        fn_kind: FnKind::Main,
    };
    v.visit_block(&program.items);
    v.finish_body();
}

/// Where a label or goto sits: the enclosing loop/switch ids (outermost
/// first) and the enclosing `finally` block ids.
#[derive(Clone, Debug, Default)]
struct Position {
    loops: Vec<u32>,
    finallies: Vec<u32>,
}

#[derive(Debug, Default)]
struct Body {
    labels: Vec<(IdentId, Position, Span)>,
    gotos: Vec<(IdentId, Position, Span)>,
    pos: Position,
    next_id: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FnKind {
    Main,
    Function,
    Constructor,
    Method,
    Closure,
}

struct Validator<'a> {
    it: &'a Interner,
    diags: &'a mut Vec<Diagnostic>,
    body: Body,
    fn_kind: FnKind,
}

impl Validator<'_> {
    fn error(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::error(code, msg).with_primary(span, ""));
    }

    fn name(&self, id: IdentId) -> String {
        self.it.resolve_lossy(id).into_owned()
    }

    /// Enter a new function body; returns the saved state.
    fn enter_body(&mut self, kind: FnKind) -> (Body, FnKind) {
        let saved = (std::mem::take(&mut self.body), self.fn_kind);
        self.fn_kind = kind;
        saved
    }

    fn leave_body(&mut self, saved: (Body, FnKind)) {
        self.finish_body();
        self.body = saved.0;
        self.fn_kind = saved.1;
    }

    /// Resolve the gotos of the finished body.
    fn finish_body(&mut self) {
        let body = std::mem::take(&mut self.body);
        for (name, pos, span) in &body.gotos {
            let Some((_, dest, _)) = body.labels.iter().find(|(n, _, _)| n == name) else {
                let n = self.name(*name);
                self.error(
                    codes::UNDEFINED_LABEL,
                    format!("'goto' to undefined label '{n}'"),
                    *span,
                );
                continue;
            };
            // The label's loops must be a prefix of the goto's loops.
            let into_loop = dest.loops.len() > pos.loops.len()
                || dest.loops.iter().zip(&pos.loops).any(|(a, b)| a != b);
            if into_loop {
                self.error(
                    codes::INVALID_JUMP,
                    "'goto' into loop or switch statement is disallowed",
                    *span,
                );
                continue;
            }
            let out_of_finally = pos.finallies.iter().any(|f| !dest.finallies.contains(f));
            if out_of_finally {
                self.error(
                    codes::INVALID_JUMP,
                    "jump out of a finally block is disallowed",
                    *span,
                );
                continue;
            }
            let into_finally = dest.finallies.iter().any(|f| !pos.finallies.contains(f));
            if into_finally {
                self.error(
                    codes::INVALID_JUMP,
                    "jump into a finally block is disallowed",
                    *span,
                );
            }
        }
    }

    fn in_loop(&mut self, f: impl FnOnce(&mut Self)) {
        let id = self.body.next_id;
        self.body.next_id += 1;
        self.body.pos.loops.push(id);
        f(self);
        self.body.pos.loops.pop();
    }

    fn in_finally(&mut self, f: impl FnOnce(&mut Self)) {
        let id = self.body.next_id;
        self.body.next_id += 1;
        self.body.pos.finallies.push(id);
        f(self);
        self.body.pos.finallies.pop();
    }

    fn check_levels(&mut self, kw: &str, levels: u32, span: Span) {
        let depth = self.body.pos.loops.len() as u32;
        if depth == 0 {
            self.error(
                codes::INVALID_JUMP,
                format!("'{kw}' not in the 'loop' or 'switch' context"),
                span,
            );
        } else if levels > depth {
            self.error(
                codes::INVALID_JUMP,
                format!("Cannot '{kw}' {levels} levels"),
                span,
            );
        }
    }

    fn check_params(&mut self, params: &[Param]) {
        for p in params {
            if let Some(_m) = &p.promote {
                match self.fn_kind {
                    FnKind::Constructor => {
                        if p.variadic {
                            self.error(
                                codes::SUPERSET_REJECTED,
                                "Cannot declare variadic promoted property",
                                p.span,
                            );
                        }
                    }
                    _ => self.error(
                        codes::SUPERSET_REJECTED,
                        "Cannot declare promoted property outside a constructor",
                        p.span,
                    ),
                }
            }
        }
    }

    fn function_body(&mut self, kind: FnKind, params: &[Param], body: &[Stmt]) {
        let saved = self.enter_body(kind);
        self.check_params(params);
        for p in params {
            if let Some(d) = &p.default {
                self.visit_expr(d);
            }
            for h in &p.hooks {
                self.visit_hook(h);
            }
        }
        self.visit_block(body);
        self.leave_body(saved);
    }
}

impl Visitor for Validator<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::While { cond, body, .. } => {
                self.visit_expr(cond);
                self.in_loop(|v| v.visit_block(body));
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.in_loop(|v| v.visit_block(body));
                self.visit_expr(cond);
            }
            Stmt::For {
                init,
                cond,
                step,
                body,
                ..
            } => {
                for e in init.iter().chain(cond).chain(step) {
                    self.visit_expr(e);
                }
                self.in_loop(|v| v.visit_block(body));
            }
            Stmt::Foreach {
                subject,
                key,
                value,
                body,
                ..
            } => {
                self.visit_expr(subject);
                if let Some(k) = key {
                    self.visit_expr(k);
                }
                self.visit_expr(value);
                self.in_loop(|v| v.visit_block(body));
            }
            Stmt::Switch { subject, cases, .. } => {
                self.visit_expr(subject);
                self.in_loop(|v| {
                    for c in cases {
                        if let Some(e) = &c.cond {
                            v.visit_expr(e);
                        }
                        v.visit_block(&c.body);
                    }
                });
            }
            Stmt::Try {
                body,
                catches,
                finally,
                ..
            } => {
                self.visit_block(body);
                for c in catches {
                    self.visit_block(&c.body);
                }
                if let Some(f) = finally {
                    self.in_finally(|v| v.visit_block(f));
                }
            }
            Stmt::Break { levels, span } => self.check_levels("break", *levels, *span),
            Stmt::Continue { levels, span } => self.check_levels("continue", *levels, *span),
            Stmt::Goto { label, span } => {
                let pos = self.body.pos.clone();
                self.body.gotos.push((*label, pos, *span));
            }
            Stmt::Label { name, span } => {
                if self.body.labels.iter().any(|(n, _, _)| n == name) {
                    let n = self.name(*name);
                    self.error(
                        codes::UNDEFINED_LABEL,
                        format!("Label '{n}' already defined"),
                        *span,
                    );
                } else {
                    let pos = self.body.pos.clone();
                    self.body.labels.push((*name, pos, *span));
                }
            }
            Stmt::ConstDecl { span, .. } => {
                if self.fn_kind != FnKind::Main {
                    self.error(
                        codes::SUPERSET_REJECTED,
                        "syntax error, unexpected token \"const\"",
                        *span,
                    );
                }
                walk_stmt(self, s);
            }
            Stmt::Func(f) => self.visit_func_decl(f),
            Stmt::ClassLike(c) => self.visit_class_like(c),
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Closure(c) => {
                for g in &c.attrs {
                    self.visit_attr_group(g);
                }
                self.function_body(FnKind::Closure, &c.params, &c.body);
            }
            Expr::ArrowFn(f) => {
                for g in &f.attrs {
                    self.visit_attr_group(g);
                }
                let saved = self.enter_body(FnKind::Closure);
                self.check_params(&f.params);
                for p in &f.params {
                    if let Some(d) = &p.default {
                        self.visit_expr(d);
                    }
                }
                self.visit_expr(&f.body);
                self.leave_body(saved);
            }
            _ => walk_expr(self, e),
        }
    }

    fn visit_func_decl(&mut self, f: &FuncDecl) {
        for g in &f.attrs {
            self.visit_attr_group(g);
        }
        self.function_body(FnKind::Function, &f.params, &f.body);
    }

    fn visit_class_like(&mut self, c: &ClassLike) {
        for g in &c.attrs {
            self.visit_attr_group(g);
        }
        for m in &c.members {
            match m {
                Member::Method(md) => {
                    for g in &md.attrs {
                        self.visit_attr_group(g);
                    }
                    let is_ctor = self
                        .it
                        .resolve(md.name)
                        .eq_ignore_ascii_case(b"__construct");
                    let kind = if is_ctor {
                        FnKind::Constructor
                    } else {
                        FnKind::Method
                    };
                    match &md.body {
                        Some(b) => self.function_body(kind, &md.params, b),
                        None => {
                            // Abstract: promotion is rejected by the adapter
                            // (`... in an abstract constructor`); still walk
                            // defaults for nested closures.
                            let saved = self.enter_body(kind);
                            for p in &md.params {
                                if let Some(d) = &p.default {
                                    self.visit_expr(d);
                                }
                            }
                            self.leave_body(saved);
                        }
                    }
                }
                Member::Prop(p) => {
                    for it in &p.items {
                        if let Some(d) = &it.default {
                            self.visit_expr(d);
                        }
                    }
                    for h in &p.hooks {
                        self.visit_hook(h);
                    }
                }
                Member::Const(cm) => {
                    for it in &cm.items {
                        self.visit_expr(&it.value);
                    }
                }
                Member::EnumCase(ec) => {
                    if let Some(v) = &ec.value {
                        self.visit_expr(v);
                    }
                }
                Member::TraitUse(_) => {}
            }
        }
    }

    fn visit_hook(&mut self, h: &Hook) {
        let saved = self.enter_body(FnKind::Method);
        if let Some(ps) = &h.params {
            for p in ps {
                if let Some(d) = &p.default {
                    self.visit_expr(d);
                }
            }
        }
        match &h.body {
            HookBody::Expr(e) => self.visit_expr(e),
            HookBody::Block(b) => self.visit_block(b),
            HookBody::Abstract => {}
        }
        self.leave_body(saved);
    }
}
