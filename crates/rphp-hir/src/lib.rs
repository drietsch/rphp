//! Name resolution and desugaring over `rphp_ast::v2` (roadmap F4, ADR-026).
//!
//! The HIR is a newtype over the same tree the parser produces, restricted
//! to a *canonical subset*: after [`lower`] there is no `elseif`, no `??=`,
//! no `?:`, no `?->` chain, no destructuring pattern, no pipe, no multi-arg
//! `isset`, no `get => expr` / `set => expr` hook shorthand, and every name in
//! a class/function/constant position carries its [`Resolved`] entry. The
//! three HIR-only nodes `Let`/`Temp`/`Seq` are the only additions. One
//! printer ([`print()`]), one visitor and one compiler input.
//!
//! The pass runs in three stages over the tree, in place:
//!
//! 1. [`resolve`] — per-statement namespace and per-kind import tables, class
//!    names to static FQNs, functions/constants to the runtime two-step keys,
//!    `::class` folding, `self`/`static`/`parent` scope checks, magic-constant
//!    substitution ([`magic`]), reserved-name and import-conflict diagnostics,
//!    duplicate unconditional declarations.
//! 2. [`validate`] — `goto`/label rules, `break`/`continue` levels, `const`
//!    in a function body, promotion placement.
//! 3. [`desugar`] — the rewrites listed in [`desugar`]'s docs, with the shared
//!    *stabilize* rule so every side effect of a write target runs once and in
//!    php's order.
//!
//! What stays a node (the compiler lowers it natively): `for`, `do`/`while`,
//! `switch`, `match`, full `?:`, `??`, `op=`, `++`/`--`, `isset`/`empty`
//! (single operand), `clone`, `print`/`exit`, `include`/`eval`, `yield`,
//! `global`/`static`, `unset`, `Interp`, casts, `@`, `(void)`, first-class
//! callables and — deliberately — `ArrowFn` (see [`free_vars`]: PHP's
//! implicit binding never warns about undefined outer variables, which a
//! `Closure` with `use` would, and the AST has no slot for that flag).
//!
//! Every message that mirrors a PHP compile error uses PHP 8.5's exact text;
//! the tests under `tests/` record the `php -r` one-liners that established
//! them.
//!
//! # Contract with the compiler
//!
//! * `Expr::Let{temp, init, body}` evaluates `init`, binds it to `temp` for
//!   `body`, and yields `body`; `Expr::Temp(t)` reads the binding (it may
//!   also be the base of a write target, e.g. `Temp(t)[0]`); `Expr::Seq`
//!   evaluates in order and yields the last value. Temporaries are numbered
//!   from `t0` per function body (main, function, method, closure, arrow
//!   function, hook).
//! * `Stmt::Foreach{value: Temp(t)}` *binds* `t` to each element (by
//!   reference iff `by_ref`); the destructuring statement prepended to the
//!   body reads it.
//! * A `Let` whose body is `Ternary(Identical(Temp t, null), d, Coalesce(chain, d))`
//!   is a short-circuited `??`; the `Coalesce` inside keeps the quiet fetch.
//! * `Stmt::Use` statements are inert after resolution and `Stmt::Namespace`
//!   is a transparent statement list; both are kept so `--emit=hir` stays
//!   readable and hoisting classification can see namespace bodies.
//! * `Name::resolved` is filled on every class/function/constant reference;
//!   `use` items and namespace names are declarations and stay `None`.
//! * The only `MagicConst` nodes left are `__CLASS__` inside traits and
//!   anonymous classes, and `__METHOD__`/`__FUNCTION__` where they would
//!   embed an anonymous class name (see [`magic`]).
//! * `ArrowFn` stays a node; [`free_vars::arrow_free_vars`] is php's
//!   implicit capture list.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use rphp_ast::v2::{pretty, ClassKind, ClassLike, FuncDecl, Member, Program, Stmt};
use rphp_diagnostics::Diagnostic;
use rphp_intern::Interner;
use rphp_span::Span;

pub mod canonical;
pub mod desugar;
pub mod free_vars;
pub mod magic;
pub mod resolve;
pub mod scope;
pub mod validate;

pub use rphp_ast::v2::Resolved;

/// The lowered program: an AST v2 tree restricted to the canonical subset
/// (see the crate docs and [`canonical::check`]).
#[derive(Clone, PartialEq, Debug)]
pub struct Hir(pub Program);

impl Hir {
    /// The underlying tree.
    pub fn program(&self) -> &Program {
        &self.0
    }

    /// Unwrap the tree.
    pub fn into_program(self) -> Program {
        self.0
    }

    /// Check the canonical-subset invariant; the returned list is empty for
    /// a well-formed HIR. [`lower`] asserts this in debug builds whenever it
    /// produced no error diagnostic.
    pub fn validate_canonical(&self) -> Vec<canonical::Violation> {
        canonical::check(&self.0)
    }

    /// The declarations PHP hoists ahead of `{main}`, see [`Hoisted`].
    pub fn hoisted(&self) -> Hoisted<'_> {
        hoisted(&self.0)
    }
}

/// How `lower` names the unit; see the field docs.
pub struct LowerOptions<'a> {
    /// The absolute script path: the value of `__FILE__` (unless
    /// [`eval_name`](Self::eval_name) overrides it) and, through its parent
    /// directory, of `__DIR__`. `None` yields `""` for both.
    pub file: Option<PathBuf>,
    /// `__FILE__` for code that is not a file: `"<file>(<line>) : eval()'d
    /// code"` for `eval()` (php's spelling), `"Command line code"` for `-r`.
    /// `__DIR__` still comes from [`file`](Self::file) / [`dir`](Self::dir),
    /// as php does for eval'd code.
    pub eval_name: Option<String>,
    /// Explicit `__DIR__` (php uses the working directory for `-r` code).
    /// `None` derives it from `file`.
    pub dir: Option<PathBuf>,
    /// Maps a span to its 1-based line, for `__LINE__` and the
    /// `{closure:...:<line>}` names.
    pub line_of: &'a dyn Fn(Span) -> u32,
    /// Assert the canonical-subset invariant after lowering (debug builds
    /// only; on by default). Turn it off for trees the parser rejected —
    /// their shapes (a standalone `list()`, a nullsafe write, ...) are not
    /// this pass's to fix.
    pub check_canonical: bool,
}

impl<'a> LowerOptions<'a> {
    /// Options with no file (`__FILE__`/`__DIR__` are `""`).
    pub fn new(line_of: &'a dyn Fn(Span) -> u32) -> Self {
        Self {
            file: None,
            eval_name: None,
            dir: None,
            line_of,
            check_canonical: true,
        }
    }

    /// Set the script path.
    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Set the eval name.
    pub fn with_eval_name(mut self, name: impl Into<String>) -> Self {
        self.eval_name = Some(name.into());
        self
    }

    /// Set an explicit `__DIR__`.
    pub fn with_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// The `__FILE__` value.
    pub(crate) fn file_value(&self) -> Vec<u8> {
        if let Some(n) = &self.eval_name {
            return n.as_bytes().to_vec();
        }
        self.file.as_deref().map(path_bytes).unwrap_or_default()
    }

    /// The `__DIR__` value: php's `dirname()` of the script path.
    pub(crate) fn dir_value(&self) -> Vec<u8> {
        if let Some(d) = &self.dir {
            return path_bytes(d);
        }
        match self.file.as_deref() {
            None => Vec::new(),
            Some(f) => match f.parent() {
                Some(p) if !p.as_os_str().is_empty() => path_bytes(p),
                // `dirname("/a.php")` is `/`, `dirname("a.php")` is `.`.
                _ => {
                    if f.is_absolute() {
                        b"/".to_vec()
                    } else {
                        b".".to_vec()
                    }
                }
            },
        }
    }
}

fn path_bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_encoded_bytes().to_vec()
}

/// Resolve, validate and desugar `program` in place and wrap it as [`Hir`].
///
/// Diagnostics are errors unless noted (the "non-compound `use` has no
/// effect" notice is a warning, as in php). A program with parse errors may
/// be lowered — recovery placeholders are left alone — but the canonical
/// check is only asserted when this pass reported no error, and callers
/// lowering parser-rejected trees should clear
/// [`LowerOptions::check_canonical`].
pub fn lower(
    mut program: Program,
    interner: &mut Interner,
    opts: &LowerOptions<'_>,
) -> (Hir, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    resolve::run(&mut program, interner, opts, &mut diags);
    validate::run(&program, interner, &mut diags);
    desugar::run(&mut program, interner, &mut diags);
    let hir = Hir(program);
    #[cfg(debug_assertions)]
    {
        if opts.check_canonical && !diags.iter().any(Diagnostic::is_error) {
            // Recovery placeholders come from the parser, not from this pass:
            // a tree with parse errors is lowered on a best-effort basis.
            let violations: Vec<_> = hir
                .validate_canonical()
                .into_iter()
                .filter(|v| v.what != canonical::PARSE_ERROR_PLACEHOLDER)
                .collect();
            debug_assert!(
                violations.is_empty(),
                "rphp-hir produced non-canonical output: {violations:#?}"
            );
        }
    }
    (hir, diags)
}

/// Print the HIR through the shared S-expression printer (`--emit=hir`).
pub fn print(hir: &Hir, interner: &Interner) -> String {
    pretty::print(&hir.0, interner)
}

/// Print the HIR with printer options (`--emit=hir --spans`).
pub fn print_with(hir: &Hir, interner: &Interner, opts: pretty::Options) -> String {
    pretty::print_with(&hir.0, interner, opts)
}

/// How PHP treats a class-like declared at the top level of a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClassHoist {
    /// No parent, no interfaces, no traits, not an enum: bound at compile
    /// time in php, i.e. declared before `{main}` runs.
    Early,
    /// Only a parent: php early-binds it iff the parent is already declared
    /// when the file is compiled (no autoload); otherwise it is declared when
    /// its statement executes. `Interp::load_unit` may try the same at load
    /// time; treating it as [`InOrder`](Self::InOrder) is the conservative
    /// choice.
    TryEarly,
    /// Interfaces, traits (`use`), enums (they implicitly implement
    /// `UnitEnum`), interfaces that extend others: declared in execution
    /// order.
    InOrder,
}

/// Classify a class-like that appears in a top-level position (the file's
/// statement list, a plain `{ }` block there, or a namespace body — what
/// `zend_compile_top_stmt` reaches; `declare { }`, `if`, loops and function
/// bodies are not top level).
pub fn class_hoist(c: &ClassLike) -> ClassHoist {
    let has_traits = c.members.iter().any(|m| matches!(m, Member::TraitUse(_)));
    if c.kind == ClassKind::Enum || !c.implements.is_empty() || has_traits {
        return ClassHoist::InOrder;
    }
    if c.extends.is_empty() {
        return ClassHoist::Early;
    }
    match c.kind {
        ClassKind::Class => ClassHoist::TryEarly,
        // An interface's `extends` list is its interface list.
        _ => ClassHoist::InOrder,
    }
}

/// The declarations of a program that php hoists ahead of `{main}`, in
/// source order. Everything not listed here is declared when its statement
/// executes (`DeclareFunction`/`DeclareClass`), including every declaration
/// nested in a function body, a control-flow statement or a `declare { }`.
#[derive(Clone, Debug, Default)]
pub struct Hoisted<'a> {
    /// Top-level functions: always hoisted.
    pub funcs: Vec<&'a FuncDecl>,
    /// Top-level class-likes classified [`ClassHoist::Early`].
    pub classes: Vec<&'a ClassLike>,
    /// Top-level classes classified [`ClassHoist::TryEarly`] (only a parent).
    pub classes_try_early: Vec<&'a ClassLike>,
}

/// Collect the hoisted declarations of `program`, see [`Hoisted`].
pub fn hoisted(program: &Program) -> Hoisted<'_> {
    let mut out = Hoisted::default();
    collect_top_level(&program.items, &mut out);
    out
}

fn collect_top_level<'a>(stmts: &'a [Stmt], out: &mut Hoisted<'a>) {
    for s in stmts {
        match s {
            Stmt::Func(f) => out.funcs.push(f),
            Stmt::ClassLike(c) => match class_hoist(c) {
                ClassHoist::Early => out.classes.push(c),
                ClassHoist::TryEarly => out.classes_try_early.push(c),
                ClassHoist::InOrder => {}
            },
            Stmt::Block { body, .. } | Stmt::Namespace { body, .. } => collect_top_level(body, out),
            _ => {}
        }
    }
}
