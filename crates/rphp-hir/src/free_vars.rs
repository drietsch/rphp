//! The implicit by-value capture list of an arrow function, exactly as
//! php's `find_implicit_binds` computes it (verified with
//! `ReflectionFunction::getStaticVariables()` on php 8.5.0, see
//! `tests/free_vars.rs`):
//!
//! * every `$name` in the body, in first-occurrence order, including
//!   assignment targets (`fn() => $x = 1` captures `$x`), destructuring
//!   targets, `++`/`--` operands and the variable of `$$name` (but not the
//!   variables `compact('x')` or `${'x'}` name);
//! * `$this` and the auto-globals are never captured;
//! * a nested closure contributes only its `use (...)` list — its body is
//!   not scanned;
//! * a nested arrow function's body *is* scanned and its parameters are
//!   **not** subtracted (php only removes the outer function's own
//!   parameters), so `fn() => (fn($v) => $v)()` captures `$v` if the
//!   enclosing scope has one;
//! * finally the arrow function's own parameters are removed.
//!
//! The HIR keeps `ArrowFn` as a node (the binding is *implicit*: php never
//! warns when a captured variable is undefined in the enclosing scope,
//! whereas `use ($x)` does, and the tree has no slot for that flag), so this
//! is a pure query the compiler uses when it lowers the node.

use rphp_ast::v2::visit::{walk_expr, Visitor};
use rphp_ast::v2::{ArrowFn, ClassLike, Expr, Stmt};
use rphp_intern::{IdentId, Interner};

use crate::scope::is_auto_global;

/// The variables `f` captures from its enclosing scope, in first-use order,
/// without duplicates, `$this`, auto-globals or `f`'s own parameters.
pub fn arrow_free_vars(f: &ArrowFn, interner: &Interner) -> Vec<IdentId> {
    let mut out = Vec::new();
    let mut fv = FreeVars {
        out: &mut out,
        interner,
    };
    fv.visit_expr(&f.body);
    out.retain(|v| !f.params.iter().any(|p| p.name == *v));
    out
}

struct FreeVars<'a> {
    out: &'a mut Vec<IdentId>,
    interner: &'a Interner,
}

impl FreeVars<'_> {
    fn push(&mut self, id: IdentId) {
        let name = self.interner.resolve(id);
        if name == b"this" || is_auto_global(name) {
            return;
        }
        if !self.out.contains(&id) {
            self.out.push(id);
        }
    }
}

impl Visitor for FreeVars<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        // Statements only occur inside nested closures, which are not scanned.
        let _ = s;
    }

    fn visit_class_like(&mut self, _c: &ClassLike) {
        // An anonymous class body is a declaration: php does not scan it
        // (only the `new` arguments).
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Var(id, _) => self.push(*id),
            Expr::Closure(c) => {
                for u in &c.uses {
                    self.push(u.name);
                }
            }
            Expr::ArrowFn(inner) => self.visit_expr(&inner.body),
            _ => walk_expr(self, e),
        }
    }
}
