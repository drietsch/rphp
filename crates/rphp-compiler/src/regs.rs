//! Register pre-scans over the v2 tree: the per-frame variable table, the
//! frame-shape facts the lowering needs up front (`$this` use, symbol-table
//! need) and the implicit-capture set of an arrow function.
//!
//! All of them are [`Visitor`]s that stop at function boundaries — a nested
//! closure or arrow function is its own frame, so only what it *captures*
//! from this frame is recorded here.

use std::collections::HashMap;

use rphp_ast::v2::visit::{walk_expr, walk_stmt, Visitor};
use rphp_ast::v2::{ArrowFn, Callee, Expr, Stmt};
use rphp_bytecode::Reg;
use rphp_intern::{IdentId, Interner};

use crate::func::ClosureBody;

/// Give `id` a register if it has none yet.
pub(crate) fn ensure_var(id: IdentId, vars: &mut HashMap<IdentId, Reg>, next: &mut Reg) {
    vars.entry(id).or_insert_with(|| {
        let r = *next;
        *next += 1;
        r
    });
}

/// What the pre-scan learned about a body besides its variables.
#[derive(Default)]
pub(crate) struct BodyFacts {
    /// The body reads `$this`.
    pub(crate) uses_this: bool,
    /// The body needs a named symbol table (`$$x`, `compact`, `extract`,
    /// `get_defined_vars`, `eval`, `include`/`require`).
    pub(crate) needs_symtab: bool,
}

/// Assign a permanent register to every variable a body names, in first-use
/// order, and collect the [`BodyFacts`]. Nested function bodies are separate
/// frames and are not entered; a closure's `use` list and an arrow function's
/// implicit captures are variables of *this* frame, so they are recorded.
pub(crate) fn collect_body(
    body: ClosureBody<'_>,
    interner: &Interner,
    vars: &mut HashMap<IdentId, Reg>,
    next: &mut Reg,
) -> BodyFacts {
    let mut c = VarCollector {
        vars,
        next,
        interner,
        facts: BodyFacts::default(),
    };
    match body {
        ClosureBody::Stmts(stmts) => c.visit_block(stmts),
        ClosureBody::ReturnExpr(e) => c.visit_expr(e),
    }
    c.facts
}

struct VarCollector<'a> {
    vars: &'a mut HashMap<IdentId, Reg>,
    next: &'a mut Reg,
    interner: &'a Interner,
    facts: BodyFacts,
}

/// The natives whose presence in a body forces a symbol table (they read or
/// write the caller's locals by name).
const SYMTAB_NATIVES: &[&[u8]] = &[b"compact", b"extract", b"get_defined_vars"];

impl Visitor for VarCollector<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match s {
            // Separate scopes: do not pull their variables (or their methods'
            // variables) into this frame.
            Stmt::Func(_) | Stmt::ClassLike(_) => {}
            // Names that are not `Expr::Var`s: `static $x`, `catch (E $e)`.
            Stmt::StaticVar { vars, .. } => {
                for v in vars {
                    ensure_var(v.name, self.vars, self.next);
                }
                walk_stmt(self, s);
            }
            Stmt::Try { catches, .. } => {
                for c in catches {
                    if let Some(v) = c.var {
                        ensure_var(v, self.vars, self.next);
                    }
                }
                walk_stmt(self, s);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Var(id, _) => {
                if self.interner.resolve(*id) == b"this" {
                    self.facts.uses_this = true;
                }
                ensure_var(*id, self.vars, self.next);
            }
            Expr::VarVar { name, .. } => {
                self.facts.needs_symtab = true;
                self.visit_expr(name);
            }
            Expr::Include { path, .. } => {
                self.facts.needs_symtab = true;
                self.visit_expr(path);
            }
            Expr::Eval { code, .. } => {
                self.facts.needs_symtab = true;
                self.visit_expr(code);
            }
            Expr::Call {
                callee: Callee::Name(name),
                args,
                ..
            } => {
                // The global candidate of the call (the fallback of a
                // two-step lookup inside a namespace, the only candidate
                // otherwise); an import alias of another function is not it.
                let text = match name.resolved {
                    Some(rphp_ast::v2::Resolved::Func { global_key, .. }) => {
                        self.interner.resolve(global_key)
                    }
                    _ => self.interner.resolve(name.text),
                };
                if SYMTAB_NATIVES.iter().any(|n| text.eq_ignore_ascii_case(n)) {
                    self.facts.needs_symtab = true;
                }
                for a in args {
                    self.visit_expr(&a.value);
                }
            }
            // A closure captures its `use` variables from this scope, so they
            // need registers here; its body and params are pre-scanned when
            // the closure itself is compiled. A non-static closure also
            // captures `$this`.
            Expr::Closure(c) => {
                for u in &c.uses {
                    ensure_var(u.name, self.vars, self.next);
                }
                if !c.static_ && body_uses_this(&c.body, self.interner) {
                    self.facts.uses_this = true;
                }
            }
            // An arrow function captures its free variables implicitly.
            Expr::ArrowFn(f) => {
                for id in arrow_free_vars(f, self.interner) {
                    ensure_var(id, self.vars, self.next);
                }
                if !f.static_ && expr_uses_this(&f.body, self.interner) {
                    self.facts.uses_this = true;
                }
            }
            _ => walk_expr(self, e),
        }
    }
}

/// Whether a statement list (a closure body) reads `$this`, looking through
/// nested non-static closures.
pub(crate) fn body_uses_this(body: &[Stmt], interner: &Interner) -> bool {
    let mut v = ThisFinder {
        interner,
        found: false,
    };
    v.visit_block(body);
    v.found
}

/// Whether an expression reads `$this`.
pub(crate) fn expr_uses_this(e: &Expr, interner: &Interner) -> bool {
    let mut v = ThisFinder {
        interner,
        found: false,
    };
    v.visit_expr(e);
    v.found
}

struct ThisFinder<'a> {
    interner: &'a Interner,
    found: bool,
}

impl Visitor for ThisFinder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Func(_) | Stmt::ClassLike(_) => {}
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Var(id, _) => {
                if self.interner.resolve(*id) == b"this" {
                    self.found = true;
                }
            }
            Expr::Closure(c) => {
                if !c.static_ {
                    walk_expr(self, e);
                }
            }
            Expr::ArrowFn(f) => {
                if !f.static_ {
                    walk_expr(self, e);
                }
            }
            _ => walk_expr(self, e),
        }
    }
}

/// The variables an arrow function reads from its enclosing scope, in
/// first-use order without duplicates and with its own parameters (and
/// `$this`, which is bound separately) removed — the by-value auto-capture
/// list. A nested closure contributes its `use` list; a nested arrow
/// function contributes its own (already filtered) free variables, so
/// captures propagate outwards through arrow-function chains.
pub(crate) fn arrow_free_vars(f: &ArrowFn, interner: &Interner) -> Vec<IdentId> {
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
    fn push_unique(&mut self, id: IdentId) {
        if self.interner.resolve(id) == b"this" {
            return;
        }
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
                for id in arrow_free_vars(inner, self.interner) {
                    self.push_unique(id);
                }
            }
            _ => walk_expr(self, e),
        }
    }
}
