//! Register pre-scans over the v2 tree: the per-frame variable table and the
//! implicit-capture set of an arrow function.
//!
//! Both are [`Visitor`]s that stop at function boundaries — a nested closure
//! or arrow function is its own frame, so only what it *captures* from this
//! frame is recorded here.

use std::collections::HashMap;

use rphp_ast::v2::visit::{walk_expr, walk_stmt, Visitor};
use rphp_ast::v2::{ArrowFn, Expr, Stmt};
use rphp_bytecode::Reg;
use rphp_intern::IdentId;

use crate::func::ClosureBody;

/// Give `id` a register if it has none yet.
pub(crate) fn ensure_var(id: IdentId, vars: &mut HashMap<IdentId, Reg>, next: &mut Reg) {
    vars.entry(id).or_insert_with(|| {
        let r = *next;
        *next += 1;
        r
    });
}

/// Assign a permanent register to every variable a body names, in first-use
/// order. Nested function bodies are separate frames and are not entered;
/// a closure's `use` list and an arrow function's implicit captures are
/// variables of *this* frame, so they are recorded.
pub(crate) fn collect_body(
    body: ClosureBody<'_>,
    vars: &mut HashMap<IdentId, Reg>,
    next: &mut Reg,
) {
    let mut c = VarCollector { vars, next };
    match body {
        ClosureBody::Stmts(stmts) => c.visit_block(stmts),
        ClosureBody::ReturnExpr(e) => c.visit_expr(e),
    }
}

struct VarCollector<'a> {
    vars: &'a mut HashMap<IdentId, Reg>,
    next: &'a mut Reg,
}

impl Visitor for VarCollector<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match s {
            // Separate scopes: do not pull their variables (or their methods'
            // variables) into this frame.
            Stmt::Func(_) | Stmt::ClassLike(_) => {}
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Var(id, _) => ensure_var(*id, self.vars, self.next),
            // A closure captures its `use` variables from this scope by value,
            // so they need registers here; its body and params are pre-scanned
            // when the closure itself is compiled.
            Expr::Closure(c) => {
                for u in &c.uses {
                    ensure_var(u.name, self.vars, self.next);
                }
            }
            // An arrow function captures its free variables implicitly.
            Expr::ArrowFn(f) => {
                for id in arrow_free_vars(f) {
                    ensure_var(id, self.vars, self.next);
                }
            }
            _ => walk_expr(self, e),
        }
    }
}

/// The variables an arrow function reads from its enclosing scope, in
/// first-use order without duplicates and with its own parameters removed —
/// the by-value auto-capture list. A nested closure contributes its `use`
/// list; a nested arrow function contributes its own (already filtered) free
/// variables, so captures propagate outwards through arrow-function chains.
pub(crate) fn arrow_free_vars(f: &ArrowFn) -> Vec<IdentId> {
    let mut out = Vec::new();
    let mut fv = FreeVars { out: &mut out };
    fv.visit_expr(&f.body);
    out.retain(|v| !f.params.iter().any(|p| p.name == *v));
    out
}

struct FreeVars<'a> {
    out: &'a mut Vec<IdentId>,
}

impl FreeVars<'_> {
    fn push_unique(&mut self, id: IdentId) {
        if !self.out.contains(&id) {
            self.out.push(id);
        }
    }
}

impl Visitor for FreeVars<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Var(id, _) => self.push_unique(*id),
            // A plain-variable assignment target is a local write, not a
            // read; only the value side captures. Index/property targets read
            // their base and are walked normally.
            Expr::Assign { target, value, .. } => {
                if !matches!(**target, Expr::Var(..)) {
                    self.visit_expr(target);
                }
                self.visit_expr(value);
            }
            Expr::Closure(c) => {
                for u in &c.uses {
                    self.push_unique(u.name);
                }
            }
            Expr::ArrowFn(inner) => {
                for id in arrow_free_vars(inner) {
                    self.push_unique(id);
                }
            }
            _ => walk_expr(self, e),
        }
    }
}
