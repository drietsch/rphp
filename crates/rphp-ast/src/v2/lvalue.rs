//! Write-context validation of lvalue-shaped expressions.
//!
//! PHP has no separate lvalue grammar: assignment targets, `foreach` targets,
//! `unset`/`global` operands and by-reference arguments are ordinary
//! expressions that Zend rejects at *compile time* when their shape cannot be
//! written through. This module mirrors those rejections — the messages are
//! PHP's own, verified against php 8.5.0 — so that the parser adapter, the HIR
//! pass and the compiler never hand-roll them.
//!
//! Terminology: the *chain* of a target is the sequence of `[..]` and `->`
//! fetches down to its *root* (`$v`, `$$v`, `C::$p`, a call, or an HIR
//! temporary). `C::$p` is always a root: its class expression is an rvalue,
//! except that a nullsafe fetch inside it still poisons the chain, exactly as
//! in Zend's `zend_ast_is_short_circuited`.
//!
//! Only the outermost expression is validated. Sub-expressions in rvalue
//! positions (indices, class expressions, call arguments) are validated when
//! they are compiled as rvalues; [`validate_rvalue`] covers the two
//! rvalue-position rejections (`[]` for reading, standalone `list()`).

use rphp_intern::{IdentId, Interner};
use rphp_span::Span;

use super::expr::{ArraySyntax, Expr};
use super::name::ClassRef;

/// `RPHP_E0020`: the expression's shape cannot be written through, referenced,
/// or used in this position.
pub const E_NOT_WRITABLE: &str = "RPHP_E0020";
/// `RPHP_E0021`: a nullsafe fetch inside a write or reference chain.
pub const E_NULLSAFE: &str = "RPHP_E0021";
/// `RPHP_E0022`: `$this` in a position that would rebind, unset or import it.
pub const E_THIS: &str = "RPHP_E0022";
/// `RPHP_E0023`: `$GLOBALS` modified or referenced as a whole, or appended to.
pub const E_GLOBALS: &str = "RPHP_E0023";
/// `RPHP_E0024`: `[]` (append) in a context that reads or unsets.
pub const E_APPEND: &str = "RPHP_E0024";
/// `RPHP_E0025`: an ill-formed destructuring pattern.
pub const E_DESTRUCTURING: &str = "RPHP_E0025";

/// The interned names the validator must recognise. Built once from the
/// interner the tree was parsed with; a name that was never interned cannot
/// occur in the tree, so `None` is a valid state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SpecialVars {
    /// The id of `this` (the variable `$this`), if interned.
    pub this: Option<IdentId>,
    /// The id of `GLOBALS` (the variable `$GLOBALS`), if interned.
    pub globals: Option<IdentId>,
}

impl SpecialVars {
    /// Intern both names (idempotent) and return their ids.
    pub fn intern(interner: &mut Interner) -> Self {
        Self {
            this: Some(interner.intern(b"this")),
            globals: Some(interner.intern(b"GLOBALS")),
        }
    }

    /// Look both names up without interning them.
    pub fn lookup(interner: &Interner) -> Self {
        Self {
            this: interner.get(b"this"),
            globals: interner.get(b"GLOBALS"),
        }
    }

    fn is_this(&self, id: IdentId) -> bool {
        self.this == Some(id)
    }

    fn is_globals(&self, id: IdentId) -> bool {
        self.globals == Some(id)
    }
}

/// The write context a target appears in. Each context has its own set of
/// PHP compile-time rules; see [`validate`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LvalueCtx {
    /// The target of `=`. Destructuring patterns allowed.
    Assign,
    /// The target of `op=` (except `??=`) and of `++`/`--`. PHP defers the
    /// `$this` check to runtime here; no patterns.
    CompoundAssign,
    /// The target of `??=`: like [`Assign`](Self::Assign) but the target is
    /// also *read*, so `[]` is rejected anywhere in the chain; no patterns.
    CoalesceAssign,
    /// The target of `=&`, and the value of a by-reference `foreach`
    /// (`as &$v`). No patterns.
    AssignRef,
    /// The value of a by-value `foreach`. Same rules as [`Assign`](Self::Assign).
    Foreach,
    /// The key of a `foreach`. Like [`Assign`](Self::Assign) but no patterns.
    ForeachKey,
    /// An operand of `unset()`.
    Unset,
    /// An operand of `global`: a simple variable only.
    Global,
    /// An argument passed to a by-reference parameter. PHP reports every
    /// violation here at *runtime* ("could not be passed by reference"), so
    /// this context is advisory: the engine may use it to emit the runtime
    /// error eagerly.
    ByRefArg,
    /// One element of a destructuring pattern (used internally when a pattern
    /// is validated; exposed for callers that validate elements one by one).
    List,
}

/// Why a target was rejected. [`LvalueErrorKind::message`] is PHP's wording
/// where PHP has one (a few shapes are parse errors in PHP and get a message
/// of ours).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LvalueErrorKind {
    /// A temporary (literal, `new`, operator result, ...) as the chain root.
    Temporary,
    /// A function call or first-class callable as the whole target.
    FunctionResult,
    /// A method or static call as the whole target.
    MethodResult,
    /// A destructuring element that cannot be written (PHP uses one message
    /// for every element-level rejection, including nullsafe chains).
    ListElementNotWritable,
    /// `global` with anything but `$v` / `$$v`.
    NotSimpleVariable,
    /// A destructuring pattern where the grammar forbids one.
    ListNotAllowed,
    /// A `=&` source or by-reference argument that cannot be referenced.
    NotReferenceable,
    /// `?->` inside a write chain.
    NullsafeWrite,
    /// `?->` inside a chain being referenced.
    NullsafeRef,
    /// `$this` assigned, destructured into, or used as a foreach target.
    ThisReassign,
    /// `unset($this)`.
    ThisUnset,
    /// `global $this`.
    ThisGlobal,
    /// `$GLOBALS` written as a whole.
    GlobalsWrite,
    /// `$GLOBALS[]`.
    GlobalsAppend,
    /// `&$GLOBALS`.
    GlobalsRef,
    /// `[]` where the chain is read (`??=`, rvalues, `isset`).
    AppendRead,
    /// `[]` inside an `unset()` operand.
    AppendUnset,
    /// A pattern with no elements or only skipped slots.
    EmptyList,
    /// Keyed and unkeyed elements in one pattern level.
    MixedKeys,
    /// A skipped slot in a keyed pattern.
    SkippedInKeyed,
    /// `list()` nested in `[]` or vice versa.
    MixedSyntax,
    /// `array(...)` as a pattern.
    LongArrayTarget,
    /// `...` in a pattern.
    SpreadInList,
    /// `&` applied to a nested pattern.
    RefOnNestedList,
    /// A pattern as a `foreach` key.
    ListAsKey,
    /// `list()` in rvalue position.
    ListAsRvalue,
}

impl LvalueErrorKind {
    /// The stable diagnostic code (`RPHP_E0020`..`RPHP_E0025`).
    pub fn code(self) -> &'static str {
        use LvalueErrorKind as K;
        match self {
            K::Temporary
            | K::FunctionResult
            | K::MethodResult
            | K::ListElementNotWritable
            | K::NotSimpleVariable
            | K::ListNotAllowed
            | K::NotReferenceable => E_NOT_WRITABLE,
            K::NullsafeWrite | K::NullsafeRef => E_NULLSAFE,
            K::ThisReassign | K::ThisUnset | K::ThisGlobal => E_THIS,
            K::GlobalsWrite | K::GlobalsAppend | K::GlobalsRef => E_GLOBALS,
            K::AppendRead | K::AppendUnset => E_APPEND,
            K::EmptyList
            | K::MixedKeys
            | K::SkippedInKeyed
            | K::MixedSyntax
            | K::LongArrayTarget
            | K::SpreadInList
            | K::RefOnNestedList
            | K::ListAsKey
            | K::ListAsRvalue => E_DESTRUCTURING,
        }
    }

    /// The human-readable message (PHP's own wording where it exists).
    pub fn message(self) -> &'static str {
        use LvalueErrorKind as K;
        match self {
            K::Temporary => "Cannot use temporary expression in write context",
            K::FunctionResult => "Can't use function return value in write context",
            K::MethodResult => "Can't use method return value in write context",
            K::ListElementNotWritable => "Assignments can only happen to writable values",
            K::NotSimpleVariable => "Only simple variables can be declared global",
            K::ListNotAllowed => "Cannot use a destructuring pattern here",
            K::NotReferenceable => "Cannot assign reference to non referenceable value",
            K::NullsafeWrite => "Can't use nullsafe operator in write context",
            K::NullsafeRef => "Cannot take reference of a nullsafe chain",
            K::ThisReassign => "Cannot re-assign $this",
            K::ThisUnset => "Cannot unset $this",
            K::ThisGlobal => "Cannot use $this as global variable",
            K::GlobalsWrite => {
                "$GLOBALS can only be modified using the $GLOBALS[$name] = $value syntax"
            }
            K::GlobalsAppend => "Cannot append to $GLOBALS",
            K::GlobalsRef => "Cannot acquire reference to $GLOBALS",
            K::AppendRead => "Cannot use [] for reading",
            K::AppendUnset => "Cannot use [] for unsetting",
            K::EmptyList => "Cannot use empty list",
            K::MixedKeys => "Cannot mix keyed and unkeyed array entries in assignments",
            K::SkippedInKeyed => "Cannot use empty array entries in keyed array assignment",
            K::MixedSyntax => "Cannot mix [] and list()",
            K::LongArrayTarget => "Cannot assign to array(), use [] instead",
            K::SpreadInList => "Spread operator is not supported in assignments",
            K::RefOnNestedList => "Cannot take a reference to a nested destructuring pattern",
            K::ListAsKey => "Cannot use list as key element",
            K::ListAsRvalue => "Cannot use list() as standalone expression",
        }
    }
}

/// A rejected target: the reason and the offending span (the whole target,
/// or the offending element / chain root when that is more precise).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LvalueError {
    /// Why.
    pub kind: LvalueErrorKind,
    /// Where.
    pub span: Span,
}

impl LvalueError {
    /// The diagnostic code, see [`LvalueErrorKind::code`].
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// The message, see [`LvalueErrorKind::message`].
    pub fn message(&self) -> &'static str {
        self.kind.message()
    }
}

type Res = Result<(), LvalueError>;

fn err(kind: LvalueErrorKind, span: Span) -> Res {
    Err(LvalueError { kind, span })
}

/// Validate `target` as a write target in `ctx`.
///
/// Accepted shapes (context permitting): `$v`, `$$v`/`${e}`, `a[i]`, `a[]`,
/// `o->p`, `C::$p`, chains of those rooted in a variable, a static property,
/// a call (`f()->p = 1`) or an HIR temporary, and — for `Assign`/`Foreach` —
/// destructuring patterns, possibly nested, keyed, by-reference or with
/// skipped slots. Rejections mirror PHP 8.5; see [`LvalueErrorKind`].
pub fn validate(target: &Expr, ctx: LvalueCtx, sv: &SpecialVars) -> Res {
    use LvalueCtx as C;
    use LvalueErrorKind as K;

    let span = target.span();
    match ctx {
        C::Global => return validate_global(target, sv),
        C::List => return validate_list_element(target, sv),
        _ => {}
    }

    if matches!(target, Expr::Array { .. }) {
        return match ctx {
            C::Assign | C::Foreach => validate_pattern(target, None, sv),
            C::ForeachKey => err(K::ListAsKey, span),
            C::ByRefArg => err(K::NotReferenceable, span),
            C::CompoundAssign | C::CoalesceAssign | C::AssignRef | C::Unset => {
                err(K::ListNotAllowed, span)
            }
            C::Global | C::List => unreachable!("handled above"),
        };
    }

    if let Some(name) = simple_var_name(target) {
        if sv.is_this(name) {
            match ctx {
                C::Unset => return err(K::ThisUnset, span),
                // PHP compiles these and fails (or not) at runtime.
                C::CompoundAssign | C::ByRefArg => {}
                _ => return err(K::ThisReassign, span),
            }
        }
        if sv.is_globals(name) && ctx != C::ByRefArg {
            return err(K::GlobalsWrite, span);
        }
    }

    let by_ref = ctx == C::ByRefArg;
    match target {
        Expr::Var(..)
        | Expr::VarVar { .. }
        | Expr::Index { .. }
        | Expr::Prop { .. }
        | Expr::StaticProp { .. }
        | Expr::Temp(..)
        | Expr::Error(_) => {}
        // Calls may be passed to by-ref parameters (runtime notice at most).
        Expr::Call { .. } | Expr::MethodCall { .. } | Expr::StaticCall { .. } if by_ref => {
            return if chain_has_nullsafe(target) {
                err(K::NullsafeRef, span)
            } else {
                Ok(())
            };
        }
        Expr::Callable { .. } if by_ref => return err(K::NotReferenceable, span),
        Expr::Call { .. } | Expr::Callable { .. } => return err(K::FunctionResult, span),
        Expr::MethodCall { .. } | Expr::StaticCall { .. } => return err(K::MethodResult, span),
        _ => {
            return err(
                if by_ref {
                    K::NotReferenceable
                } else {
                    K::Temporary
                },
                span,
            )
        }
    }

    if chain_has_nullsafe(target) {
        return err(
            if by_ref {
                K::NullsafeRef
            } else {
                K::NullsafeWrite
            },
            span,
        );
    }
    if let Some(s) = find_globals_append(target, sv) {
        return err(K::GlobalsAppend, s);
    }
    match ctx {
        C::CoalesceAssign => {
            if let Some(s) = find_append(target) {
                return err(K::AppendRead, s);
            }
        }
        C::Unset => {
            if let Some(s) = find_append(target) {
                return err(K::AppendUnset, s);
            }
        }
        _ => {}
    }
    let root = chain_root(target);
    if !root_is_writable(root) {
        return err(
            if by_ref {
                K::NotReferenceable
            } else {
                K::Temporary
            },
            root.span(),
        );
    }
    Ok(())
}

/// Validate the right-hand side of `=&` (and the `use (&$x)`-like positions
/// that need a referenceable expression): `$v`, `$$v`, `a[i]`, `a[]`, `o->p`,
/// `C::$p`, chains of those, or a call (PHP accepts calls with a runtime
/// notice when they do not return by reference). Rejects `$GLOBALS`, nullsafe
/// chains, temporaries and everything else.
pub fn validate_ref_source(source: &Expr, sv: &SpecialVars) -> Res {
    use LvalueErrorKind as K;

    let span = source.span();
    if simple_var_name(source).is_some_and(|n| sv.is_globals(n)) {
        return err(K::GlobalsRef, span);
    }
    if chain_has_nullsafe(source) {
        return err(K::NullsafeRef, span);
    }
    match source {
        Expr::Var(..)
        | Expr::VarVar { .. }
        | Expr::Index { .. }
        | Expr::Prop { .. }
        | Expr::StaticProp { .. }
        | Expr::Temp(..)
        | Expr::Error(_) => {}
        Expr::Call { .. } | Expr::MethodCall { .. } | Expr::StaticCall { .. } => return Ok(()),
        _ => return err(K::NotReferenceable, span),
    }
    if let Some(s) = find_globals_append(source, sv) {
        return err(K::GlobalsAppend, s);
    }
    let root = chain_root(source);
    if !root_is_writable(root) {
        return err(K::Temporary, root.span());
    }
    Ok(())
}

/// Validate an expression in rvalue position for the two shape errors PHP
/// reports there: `[]` anywhere in its fetch chain ("Cannot use [] for
/// reading", also for `isset`/`empty` operands) and a standalone `list()`.
/// Nested sub-expressions are not inspected.
pub fn validate_rvalue(expr: &Expr) -> Res {
    use LvalueErrorKind as K;

    if let Expr::Array {
        syntax: ArraySyntax::List,
        span,
        ..
    } = expr
    {
        return err(K::ListAsRvalue, *span);
    }
    if let Some(s) = find_append(expr) {
        return err(K::AppendRead, s);
    }
    Ok(())
}

fn validate_global(target: &Expr, sv: &SpecialVars) -> Res {
    use LvalueErrorKind as K;
    if simple_var_name(target).is_some_and(|n| sv.is_this(n)) {
        return err(K::ThisGlobal, target.span());
    }
    match target {
        Expr::Var(..) | Expr::VarVar { .. } | Expr::Error(_) => Ok(()),
        _ => err(K::NotSimpleVariable, target.span()),
    }
}

fn validate_pattern(pat: &Expr, parent: Option<ArraySyntax>, sv: &SpecialVars) -> Res {
    use LvalueErrorKind as K;
    let Expr::Array {
        items,
        syntax,
        span,
    } = pat
    else {
        unreachable!("validate_pattern is only called on Expr::Array");
    };
    if *syntax == ArraySyntax::Long {
        return err(K::LongArrayTarget, *span);
    }
    if let Some(p) = parent {
        if p != *syntax {
            return err(K::MixedSyntax, *span);
        }
    }
    if items.iter().all(|it| it.value.is_none()) {
        return err(K::EmptyList, *span);
    }
    let keyed = items.iter().any(|it| it.key.is_some());
    for it in items {
        let Some(value) = &it.value else {
            if keyed {
                return err(K::SkippedInKeyed, it.span);
            }
            continue;
        };
        if keyed && it.key.is_none() {
            return err(K::MixedKeys, it.span);
        }
        if it.spread {
            return err(K::SpreadInList, it.span);
        }
        if matches!(value, Expr::Array { .. }) {
            if it.by_ref {
                return err(K::RefOnNestedList, it.span);
            }
            validate_pattern(value, Some(*syntax), sv)?;
        } else {
            validate_list_element(value, sv)?;
        }
    }
    Ok(())
}

fn validate_list_element(elem: &Expr, sv: &SpecialVars) -> Res {
    use LvalueErrorKind as K;

    let span = elem.span();
    if let Some(name) = simple_var_name(elem) {
        if sv.is_this(name) {
            return err(K::ThisReassign, span);
        }
        if sv.is_globals(name) {
            return err(K::GlobalsWrite, span);
        }
    }
    match elem {
        Expr::Array { .. } => return validate_pattern(elem, None, sv),
        Expr::Call { .. } | Expr::Callable { .. } => return err(K::FunctionResult, span),
        Expr::MethodCall { .. } | Expr::StaticCall { .. } => return err(K::MethodResult, span),
        Expr::Var(..)
        | Expr::VarVar { .. }
        | Expr::Index { .. }
        | Expr::Prop { .. }
        | Expr::StaticProp { .. }
        | Expr::Temp(..)
        | Expr::Error(_) => {}
        _ => return err(K::ListElementNotWritable, span),
    }
    if chain_has_nullsafe(elem) {
        return err(K::ListElementNotWritable, span);
    }
    if let Some(s) = find_globals_append(elem, sv) {
        return err(K::GlobalsAppend, s);
    }
    let root = chain_root(elem);
    if !root_is_writable(root) {
        return err(K::ListElementNotWritable, root.span());
    }
    Ok(())
}

/// The name of a *simple variable*: `$x`, or `${'x'}` (a variable-variable
/// whose name is a string literal, which Zend parses as the same node as
/// `$x`). `$this` and `$GLOBALS` are recognised through this helper so that
/// `${'this'}` is treated like `$this`, as in PHP.
fn simple_var_name(e: &Expr) -> Option<IdentId> {
    match e {
        Expr::Var(name, _) => Some(*name),
        Expr::VarVar { name, .. } => match &**name {
            Expr::Str(id, _) => Some(*id),
            _ => None,
        },
        _ => None,
    }
}

/// The root of a write chain: follow `[..]` bases and `->` objects. `C::$p`
/// and calls are roots themselves.
fn chain_root(e: &Expr) -> &Expr {
    let mut cur = e;
    loop {
        cur = match cur {
            Expr::Index { base, .. } => base,
            Expr::Prop { obj, .. } => obj,
            _ => return cur,
        };
    }
}

fn root_is_writable(root: &Expr) -> bool {
    matches!(
        root,
        Expr::Var(..)
            | Expr::VarVar { .. }
            | Expr::StaticProp { .. }
            | Expr::Temp(..)
            | Expr::Call { .. }
            | Expr::MethodCall { .. }
            | Expr::StaticCall { .. }
            | Expr::Error(_)
    )
}

/// Zend's `zend_ast_is_short_circuited`: a `?->` anywhere along the chain,
/// including inside the class expression of `::`.
fn chain_has_nullsafe(e: &Expr) -> bool {
    match e {
        Expr::Prop { nullsafe: true, .. } | Expr::MethodCall { nullsafe: true, .. } => true,
        Expr::Prop { obj, .. } | Expr::MethodCall { obj, .. } => chain_has_nullsafe(obj),
        Expr::Index { base, .. } => chain_has_nullsafe(base),
        Expr::StaticProp {
            class: ClassRef::Expr(c),
            ..
        }
        | Expr::StaticCall {
            class: ClassRef::Expr(c),
            ..
        } => chain_has_nullsafe(c),
        _ => false,
    }
}

/// The innermost `[]` along the chain, if any.
fn find_append(e: &Expr) -> Option<Span> {
    match e {
        Expr::Index { base, index, span } => {
            find_append(base).or(if index.is_none() { Some(*span) } else { None })
        }
        Expr::Prop { obj, .. } => find_append(obj),
        _ => None,
    }
}

/// `$GLOBALS[]` along the chain, if any.
fn find_globals_append(e: &Expr, sv: &SpecialVars) -> Option<Span> {
    match e {
        Expr::Index { base, index, span } => {
            if index.is_none() && simple_var_name(base).is_some_and(|n| sv.is_globals(n)) {
                return Some(*span);
            }
            find_globals_append(base, sv)
        }
        Expr::Prop { obj, .. } => find_globals_append(obj, sv),
        _ => None,
    }
}
