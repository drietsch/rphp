//! The conformance rules: everything mago 1.49 accepts that PHP 8.5.0
//! rejects at parse/compile time, reported as `RPHP_E0011` with PHP's own
//! message (`RPHP_E0020..E0025` for the write-context rules, which come from
//! `rphp_ast::v2::lvalue`).
//!
//! Every rule here is backed by a `php -l` verdict; the snippets live in
//! `tests/negative/*.php` and the corpus job's `--negative` mode checks that
//! stock PHP keeps rejecting them.

use mago_syntax::cst::{Access, Expression, Identifier};
use rphp_ast::v2::lvalue::{self, LvalueCtx, LvalueError};
use rphp_ast::v2::{Arg, CallableTarget, Callee, ClassRef, Expr, NewTarget};
use rphp_span::Span;

use super::ctx::Ctx;

/// PHP's generic parse-error wording for an unexpected token.
pub(crate) fn unexpected_token(tok: &str) -> String {
    format!("syntax error, unexpected token \"{tok}\"")
}

/// Report a write-context rejection with its own code.
fn lvalue_error(ctx: &mut Ctx<'_, '_>, err: LvalueError) {
    ctx.error(err.code(), err.message(), err.span);
}

/// Validate a write target (see [`lvalue::validate`]).
pub(crate) fn validate_target(ctx: &mut Ctx<'_, '_>, target: &Expr, lctx: LvalueCtx) {
    let sv = ctx.sv;
    if let Err(e) = lvalue::validate(target, lctx, &sv) {
        lvalue_error(ctx, e);
    }
}

/// Validate an expression in rvalue position (`[]` read, standalone `list()`).
pub(crate) fn validate_rvalue(ctx: &mut Ctx<'_, '_>, e: &Expr) {
    if let Err(err) = lvalue::validate_rvalue(e) {
        lvalue_error(ctx, err);
    }
}

/// Reject a parenthesized expression used as a whole write target
/// (`($a) = 1`, `($a)++`, `foreach (... as ($a))`, `unset(($a))`).
pub(crate) fn check_paren_target(ctx: &mut Ctx<'_, '_>, target: &Expression<'_>, tok: &str) {
    if let Expression::Parenthesized(p) = target {
        let span = ctx.sp(mago_span::HasSpan::span(p));
        ctx.reject(unexpected_token(tok), span);
    }
}

/// Named/positional/unpacking ordering rules of an argument list.
pub(crate) fn check_args_order(ctx: &mut Ctx<'_, '_>, args: &[Arg]) {
    let mut seen_named = false;
    let mut seen_spread = false;
    for a in args {
        if a.name.is_some() {
            seen_named = true;
        } else if a.spread {
            if seen_named {
                ctx.reject("Cannot use argument unpacking after named arguments", a.span);
            }
            seen_spread = true;
        } else if seen_named {
            ctx.reject("Cannot use positional argument after named argument", a.span);
        } else if seen_spread {
            ctx.reject("Cannot use positional argument after argument unpacking", a.span);
        }
    }
}

/// Where a constant expression is required.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ConstCtx {
    /// A parameter default value.
    ParamDefault,
    /// A property default value.
    PropDefault,
    /// A class constant value.
    ClassConst,
    /// A top-level `const` value.
    GlobalConst,
    /// An enum case value.
    EnumCase,
    /// An attribute argument.
    AttrArg,
}

const INVALID_CONST: &str = "Constant expression contains invalid operations";

/// PHP's compile-time constant-expression rules.
pub(crate) fn check_const_expr(ctx: &mut Ctx<'_, '_>, e: &Expr, cctx: ConstCtx) {
    if let Some((msg, span)) = first_const_violation(e, cctx) {
        ctx.reject(msg, span);
    }
}

/// The first violation in evaluation order, if any.
fn first_const_violation(e: &Expr, cctx: ConstCtx) -> Option<(String, Span)> {
    if !is_const_shape(e) {
        return Some((INVALID_CONST.to_string(), e.span()));
    }
    let mut found = None;
    walk_const(e, cctx, &mut found);
    found
}

/// `Expr::is_constant_shape` plus the PHP 8.2 enum-case property fetch
/// (`E::A->value`, also nullsafe), which the tree's own approximation
/// leaves out.
fn is_const_shape(e: &Expr) -> bool {
    match e {
        // `E::A->value`, `(new A)->{expr}`, `x?->y` (PHP 8.2+ / 8.5).
        Expr::Prop { obj, name, .. } => {
            let name_ok = match name {
                rphp_ast::v2::MemberName::Ident(..) => true,
                rphp_ast::v2::MemberName::Expr(e) => is_const_shape(e),
            };
            name_ok && is_const_shape(obj)
        }
        Expr::ClassConst { class, name, .. } => {
            let class_ok = match class {
                ClassRef::Expr(inner) => is_const_shape(inner),
                _ => true,
            };
            let name_ok = match name {
                rphp_ast::v2::ConstSel::Expr(inner) => is_const_shape(inner),
                _ => true,
            };
            class_ok && name_ok
        }
        Expr::Array { items, syntax, .. } => {
            !syntax.is_list()
                && items.iter().all(|it| {
                    !it.by_ref
                        && it.key.as_ref().is_none_or(is_const_shape)
                        && it.value.as_ref().is_some_and(is_const_shape)
                })
        }
        Expr::Index {
            base,
            index: Some(index),
            ..
        } => is_const_shape(base) && is_const_shape(index),
        Expr::Unary { op, expr, .. } => {
            !op.is_inc_dec()
                && !matches!(op, rphp_ast::v2::UnOp::Silence | rphp_ast::v2::UnOp::Void)
                && is_const_shape(expr)
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            *op != rphp_ast::v2::BinOp::Pipe && is_const_shape(lhs) && is_const_shape(rhs)
        }
        Expr::Ternary {
            cond, then, else_, ..
        } => {
            is_const_shape(cond)
                && then.as_deref().is_none_or(is_const_shape)
                && is_const_shape(else_)
        }
        Expr::New { args, .. } => args.iter().all(|a| is_const_shape(&a.value)),
        Expr::Closure(_) | Expr::ArrowFn(_) => true,
        Expr::Callable { target, .. } => match target {
            CallableTarget::Func(_) => true,
            CallableTarget::Static { name, .. } => match name {
                rphp_ast::v2::MemberName::Ident(..) => true,
                rphp_ast::v2::MemberName::Expr(e) => is_const_shape(e),
            },
            CallableTarget::Method { .. } => false,
        },
        other => other.is_constant_shape(),
    }
}

fn walk_const(e: &Expr, cctx: ConstCtx, found: &mut Option<(String, Span)>) {
    if found.is_some() {
        return;
    }
    fn hit(found: &mut Option<(String, Span)>, msg: &str, span: Span) {
        if found.is_none() {
            *found = Some((msg.to_string(), span));
        }
    }
    match e {
        Expr::ArrowFn(f) => hit(found, INVALID_CONST, f.span),
        Expr::Callable { target, span } => match target {
            CallableTarget::Func(Callee::Expr(_)) => hit(
                found,
                "Cannot use dynamic function name in constant expression",
                *span,
            ),
            CallableTarget::Static {
                class: ClassRef::Static(_),
                ..
            } => hit(found, "\"static\" is not allowed in compile-time constants", *span),
            CallableTarget::Static {
                class: ClassRef::Expr(_),
                ..
            } => hit(found, "Cannot use dynamic class name in constant expression", *span),
            _ => {}
        },
        Expr::Closure(c) => {
            if !c.static_ {
                hit(found, "Closures in constant expressions must be static", c.span);
            }
        }
        Expr::ClassConst { class, name, span } => {
            match (class, name) {
                (ClassRef::Static(_), rphp_ast::v2::ConstSel::Class(_)) => hit(
                    found,
                    "static::class cannot be used for compile-time class name resolution",
                    *span,
                ),
                (ClassRef::Static(_), _) => {
                    hit(found, "\"static::\" is not allowed in compile-time constants", *span)
                }
                (ClassRef::Expr(_), rphp_ast::v2::ConstSel::Class(_)) => hit(
                    found,
                    "(expression)::class cannot be used in constant expressions",
                    *span,
                ),
                (ClassRef::Expr(_), _) => hit(
                    found,
                    "Dynamic class names are not allowed in compile-time class constant references",
                    *span,
                ),
                _ => {}
            }
            if let ClassRef::Expr(inner) = class {
                walk_const(inner, cctx, found);
            }
            if let rphp_ast::v2::ConstSel::Expr(inner) = name {
                walk_const(inner, cctx, found);
            }
        }
        Expr::New { class, args, span } => {
            if matches!(
                cctx,
                ConstCtx::PropDefault | ConstCtx::ClassConst | ConstCtx::EnumCase
            ) {
                hit(found, "New expressions are not supported in this context", *span);
                return;
            }
            match class {
                NewTarget::Anon(_) => {
                    hit(found, "Cannot use anonymous class in constant expression", *span)
                }
                NewTarget::Ref(ClassRef::Static(_)) => {
                    hit(found, "\"static\" is not allowed in compile-time constants", *span)
                }
                NewTarget::Ref(ClassRef::Expr(_)) => {
                    hit(found, "Cannot use dynamic class name in constant expression", *span)
                }
                _ => {}
            }
            for a in args {
                if a.spread {
                    hit(found, "Argument unpacking in constant expressions is not supported",
                        a.span,
                    );
                }
                walk_const(&a.value, cctx, found);
            }
        }
        Expr::Array { items, .. } => {
            for it in items {
                if let Some(k) = &it.key {
                    walk_const(k, cctx, found);
                }
                if let Some(v) = &it.value {
                    walk_const(v, cctx, found);
                }
            }
        }
        Expr::Index { base, index, .. } => {
            walk_const(base, cctx, found);
            if let Some(i) = index {
                walk_const(i, cctx, found);
            }
        }
        Expr::Unary { expr, .. } => walk_const(expr, cctx, found),
        Expr::Binary { lhs, rhs, .. } => {
            walk_const(lhs, cctx, found);
            walk_const(rhs, cctx, found);
        }
        Expr::Ternary {
            cond, then, else_, ..
        } => {
            walk_const(cond, cctx, found);
            if let Some(t) = then {
                walk_const(t, cctx, found);
            }
            walk_const(else_, cctx, found);
        }
        _ => {}
    }
}

/// Zend's `new_variable`: a variable-rooted chain of `[..]`, `->`, `?->` and
/// `::$` fetches, as allowed after `new` and `instanceof`.
pub(crate) fn is_new_variable(e: &Expression<'_>) -> bool {
    match e {
        Expression::Variable(_) => true,
        Expression::ArrayAccess(a) => is_new_variable(a.array),
        Expression::Access(Access::Property(p)) => is_new_variable(p.object),
        Expression::Access(Access::NullSafeProperty(p)) => is_new_variable(p.object),
        Expression::Access(Access::StaticProperty(s)) => {
            is_class_name(s.class) || is_new_variable(s.class)
        }
        _ => false,
    }
}

/// A static class name (`Foo`, `self`, `static`, `parent`).
pub(crate) fn is_class_name(e: &Expression<'_>) -> bool {
    matches!(
        e,
        Expression::Identifier(_) | Expression::Static(_) | Expression::Self_(_) | Expression::Parent(_)
    )
}

/// Zend's `class_name_reference`: what may follow `new` and `instanceof`.
pub(crate) fn is_class_name_reference(e: &Expression<'_>) -> bool {
    is_class_name(e)
        || matches!(e, Expression::Parenthesized(_) | Expression::Error(_))
        || is_new_variable(e)
}

/// The token text PHP would report for an unexpected class expression.
pub(crate) fn describe_unexpected(e: &Expression<'_>) -> String {
    match e {
        Expression::Literal(mago_syntax::cst::Literal::String(_)) => {
            "syntax error, unexpected string".to_string()
        }
        Expression::Literal(mago_syntax::cst::Literal::Integer(_)) => {
            "syntax error, unexpected integer".to_string()
        }
        Expression::Array(_) => unexpected_token("["),
        Expression::Call(_) => unexpected_token("("),
        Expression::Access(Access::ClassConstant(_)) => {
            "syntax error, unexpected identifier, expecting variable or \"$\"".to_string()
        }
        Expression::Access(Access::Property(_)) => unexpected_token("->"),
        Expression::ArrayAccess(_) => unexpected_token("["),
        _ => "syntax error, unexpected expression".to_string(),
    }
}

/// `true` if the identifier is the unqualified `clone` (the PHP 8.5
/// `clone()` function form).
pub(crate) fn is_clone_callee(e: &Expression<'_>) -> bool {
    matches!(e, Expression::Identifier(Identifier::Local(l)) if super::ctx::eq_ci(l.value, "clone"))
}
