//! Type declarations: `Hint` → [`Type`], plus the compile-time type rules
//! PHP enforces (`void`/`never` placement, redundant union members, ...).

use mago_span::HasSpan;
use mago_syntax::cst::Hint;
use rphp_ast::v2::{Builtin, ClassKind, Type, TypeKind};

use super::ctx::Ctx;
use super::names;

/// Convert a written type hint.
pub(crate) fn hint(ctx: &mut Ctx<'_, '_>, h: &Hint<'_>) -> Type {
    let span = ctx.sp(h.span());
    match h {
        Hint::Identifier(id) => Type::named(names::name(ctx, id)),
        Hint::Parenthesized(p) => {
            let mut inner = hint(ctx, p.hint);
            inner.span = span;
            inner
        }
        Hint::Nullable(n) => Type {
            kind: TypeKind::Nullable(Box::new(hint(ctx, n.hint))),
            span,
        },
        Hint::Union(_) => {
            let mut members = Vec::new();
            flatten_union(ctx, h, &mut members);
            Type {
                kind: TypeKind::Union(members),
                span,
            }
        }
        Hint::Intersection(_) => {
            let mut members = Vec::new();
            flatten_intersection(ctx, h, &mut members);
            Type {
                kind: TypeKind::Intersection(members),
                span,
            }
        }
        Hint::Null(_) => Type::builtin(Builtin::Null, span),
        Hint::True(_) => Type::builtin(Builtin::True, span),
        Hint::False(_) => Type::builtin(Builtin::False, span),
        Hint::Array(_) => Type::builtin(Builtin::Array, span),
        Hint::Callable(_) => Type::builtin(Builtin::Callable, span),
        Hint::Static(_) => Type::builtin(Builtin::StaticTy, span),
        Hint::Self_(_) => Type::builtin(Builtin::SelfTy, span),
        Hint::Parent(_) => Type::builtin(Builtin::ParentTy, span),
        Hint::Void(_) => Type::builtin(Builtin::Void, span),
        Hint::Never(_) => Type::builtin(Builtin::Never, span),
        Hint::Float(_) => Type::builtin(Builtin::Float, span),
        Hint::Bool(_) => Type::builtin(Builtin::Bool, span),
        Hint::Integer(_) => Type::builtin(Builtin::Int, span),
        Hint::String(_) => Type::builtin(Builtin::String, span),
        Hint::Object(_) => Type::builtin(Builtin::Object, span),
        Hint::Mixed(_) => Type::builtin(Builtin::Mixed, span),
        Hint::Iterable(_) => Type::builtin(Builtin::Iterable, span),
    }
}

fn flatten_union(ctx: &mut Ctx<'_, '_>, h: &Hint<'_>, out: &mut Vec<Type>) {
    match h {
        Hint::Union(u) => {
            flatten_union(ctx, u.left, out);
            flatten_union(ctx, u.right, out);
        }
        other => out.push(hint(ctx, other)),
    }
}

fn flatten_intersection(ctx: &mut Ctx<'_, '_>, h: &Hint<'_>, out: &mut Vec<Type>) {
    match h {
        Hint::Intersection(i) => {
            flatten_intersection(ctx, i.left, out);
            flatten_intersection(ctx, i.right, out);
        }
        other => out.push(hint(ctx, other)),
    }
}

/// Where a type was written; decides which PHP rules apply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TypePos {
    /// A parameter type.
    Param,
    /// A return type.
    Return,
    /// A property type (`owner` is `C::$p`).
    Prop,
    /// A class-constant type (`owner` is `C::X`).
    Const,
}

/// The spelling of a type for messages.
pub(crate) fn display(ctx: &Ctx<'_, '_>, t: &Type) -> String {
    match &t.kind {
        TypeKind::Named(n) => {
            let text = ctx.interner.resolve_lossy(n.text).into_owned();
            match n.kind {
                rphp_ast::v2::NameKind::FullyQualified => format!("\\{text}"),
                rphp_ast::v2::NameKind::Relative => format!("namespace\\{text}"),
                _ => text,
            }
        }
        TypeKind::Builtin(b) => b.as_str().to_string(),
        TypeKind::Nullable(inner) => format!("?{}", display(ctx, inner)),
        TypeKind::Union(ms) => ms
            .iter()
            .map(|m| match m.kind {
                TypeKind::Intersection(_) => format!("({})", display(ctx, m)),
                _ => display(ctx, m),
            })
            .collect::<Vec<_>>()
            .join("|"),
        TypeKind::Intersection(ms) => ms
            .iter()
            .map(|m| display(ctx, m))
            .collect::<Vec<_>>()
            .join("&"),
    }
}

/// A case-folded identity for duplicate detection.
fn key(ctx: &Ctx<'_, '_>, t: &Type) -> String {
    display(ctx, t).to_ascii_lowercase()
}

/// Apply PHP's compile-time type rules. `owner` names the declaration for
/// messages (`C::$p`, `C::X`).
pub(crate) fn check(ctx: &mut Ctx<'_, '_>, t: &Type, pos: TypePos, owner: &str) {
    let full = display(ctx, t);
    check_inner(ctx, t, pos, owner, true, &full);
}

fn check_inner(ctx: &mut Ctx<'_, '_>, t: &Type, pos: TypePos, owner: &str, standalone: bool, full: &str) {
    let span = t.span;
    match &t.kind {
        TypeKind::Builtin(b) => match b {
            Builtin::Void => match pos {
                TypePos::Return if standalone => {}
                TypePos::Return => ctx.reject("Void can only be used as a standalone type", span),
                TypePos::Param => ctx.reject("void cannot be used as a parameter type", span),
                TypePos::Prop => ctx.reject(format!("Property {owner} cannot have type void"), span),
                TypePos::Const => {
                    ctx.reject(format!("Class constant {owner} cannot have type void"), span)
                }
            },
            Builtin::Never => match pos {
                TypePos::Return if standalone => {}
                TypePos::Return => ctx.reject("never can only be used as a standalone type", span),
                TypePos::Param => ctx.reject("never cannot be used as a parameter type", span),
                TypePos::Prop => ctx.reject(format!("Property {owner} cannot have type never"), span),
                TypePos::Const => {
                    ctx.reject(format!("Class constant {owner} cannot have type never"), span)
                }
            },
            Builtin::Mixed if !standalone => {
                ctx.reject("Type mixed can only be used as a standalone type", span)
            }
            Builtin::Callable => match pos {
                TypePos::Prop => {
                    ctx.reject(format!("Property {owner} cannot have type {full}"), span)
                }
                TypePos::Const => {
                    ctx.reject(format!("Class constant {owner} cannot have type {full}"), span)
                }
                _ => {}
            },
            Builtin::StaticTy if matches!(pos, TypePos::Param | TypePos::Prop) => {
                ctx.reject("syntax error, unexpected token \"static\"", span)
            }
            Builtin::SelfTy | Builtin::StaticTy | Builtin::ParentTy => check_scope(ctx, *b, span),
            _ => {}
        },
        TypeKind::Named(_) => {}
        TypeKind::Nullable(inner) => {
            match &inner.kind {
                TypeKind::Intersection(_) => ctx.reject("syntax error, unexpected token \"&\"", span),
                TypeKind::Union(_) => ctx.reject("syntax error, unexpected token \"|\"", span),
                TypeKind::Builtin(Builtin::Mixed) => ctx.reject(
                    "Type mixed cannot be marked as nullable since mixed already includes null",
                    span,
                ),
                TypeKind::Builtin(Builtin::Null) => {
                    ctx.reject("null cannot be marked as nullable", span)
                }
                _ => {}
            }
            check_inner(ctx, inner, pos, owner, false, full);
        }
        TypeKind::Union(members) => {
            let mut seen: Vec<String> = Vec::new();
            let mut has_bool = false;
            let mut has_true = false;
            let mut has_false = false;
            let mut has_iterable = false;
            let mut has_object = false;
            let mut class_names: Vec<String> = Vec::new();
            for m in members {
                check_inner(ctx, m, pos, owner, false, full);
                let k = key(ctx, m);
                if seen.contains(&k) {
                    ctx.reject(
                        format!("Duplicate type {} is redundant", display(ctx, m)),
                        m.span,
                    );
                    continue;
                }
                seen.push(k.clone());
                match &m.kind {
                    TypeKind::Builtin(Builtin::Bool) => has_bool = true,
                    TypeKind::Builtin(Builtin::True) => has_true = true,
                    TypeKind::Builtin(Builtin::False) => has_false = true,
                    TypeKind::Builtin(Builtin::Iterable) => has_iterable = true,
                    TypeKind::Builtin(Builtin::Object) => has_object = true,
                    TypeKind::Named(_) => class_names.push(k),
                    _ => {}
                }
            }
            if has_true && has_false {
                ctx.reject(
                    "Type contains both true and false, bool must be used instead",
                    span,
                );
            } else if has_bool && (has_true || has_false) {
                let which = if has_true { "true" } else { "false" };
                ctx.reject(format!("Duplicate type {which} is redundant"), span);
            }
            if has_iterable {
                if seen.iter().any(|k| k == "array") {
                    ctx.reject("Duplicate type array is redundant", span);
                }
                if class_names.iter().any(|k| k == "traversable" || k == "\\traversable") {
                    ctx.reject("Duplicate type Traversable is redundant", span);
                }
            }
            // DNF: an intersection is redundant with a class member it contains,
            // with another intersection of the same classes, or with a subset.
            let inter: Vec<(String, Vec<String>)> = members
                .iter()
                .filter_map(|m| match &m.kind {
                    TypeKind::Intersection(ms) => Some((
                        display(ctx, m),
                        ms.iter().map(|x| key(ctx, x)).collect::<Vec<_>>(),
                    )),
                    _ => None,
                })
                .collect();
            for (i, (shown, ks)) in inter.iter().enumerate() {
                if let Some(c) = ks.iter().find(|k| class_names.contains(k)) {
                    let cname = members
                        .iter()
                        .find(|m| matches!(&m.kind, TypeKind::Named(_)) && key(ctx, m) == *c)
                        .map(|m| display(ctx, m))
                        .unwrap_or_default();
                    ctx.reject(
                        format!("Type {shown} is redundant as it is more restrictive than type {cname}"),
                        span,
                    );
                    break;
                }
                for (other_shown, oks) in inter.iter().take(i) {
                    let subset = oks.iter().all(|k| ks.contains(k));
                    if subset && oks.len() == ks.len() {
                        ctx.reject(format!("Type {shown} is redundant with type {other_shown}"), span);
                    } else if subset {
                        ctx.reject(
                            format!("Type {shown} is redundant as it is more restrictive than type {other_shown}"),
                            span,
                        );
                    } else if ks.iter().all(|k| oks.contains(k)) {
                        ctx.reject(
                            format!("Type {other_shown} is redundant as it is more restrictive than type {shown}"),
                            span,
                        );
                    }
                }
            }
            if has_object && !class_names.is_empty() {
                ctx.reject(
                    format!(
                        "Type {} contains both object and a class type, which is redundant",
                        display(ctx, t)
                    ),
                    span,
                );
            }
        }
        TypeKind::Intersection(members) => {
            let mut seen: Vec<String> = Vec::new();
            for m in members {
                match &m.kind {
                    TypeKind::Named(_) => {}
                    TypeKind::Builtin(b @ (Builtin::SelfTy | Builtin::StaticTy | Builtin::ParentTy)) => {
                        check_scope(ctx, *b, m.span);
                        // Zend allows self/parent here but static is rejected
                        // at the parser level; nothing else to check.
                    }
                    _ => ctx.reject(
                        format!(
                            "Type {} cannot be part of an intersection type",
                            display(ctx, m)
                        ),
                        m.span,
                    ),
                }
                let k = key(ctx, m);
                if seen.contains(&k) {
                    ctx.reject(
                        format!("Duplicate type {} is redundant", display(ctx, m)),
                        m.span,
                    );
                }
                seen.push(k);
            }
        }
    }
}

/// `self`/`static`/`parent` need a class scope when the scope is known.
pub(crate) fn check_scope(ctx: &mut Ctx<'_, '_>, b: Builtin, span: rphp_span::Span) {
    if !ctx.scope_known() {
        return;
    }
    match ctx.effective_class() {
        None => ctx.reject(
            format!("Cannot use \"{}\" when no class scope is active", b.as_str()),
            span,
        ),
        Some(c) if b == Builtin::ParentTy && !c.has_parent && c.kind == ClassKind::Class => {
            ctx.reject(
                "Cannot use \"parent\" when current class scope has no parent",
                span,
            );
        }
        Some(_) => {}
    }
}

/// `true` if the return type is exactly `void` / `never` (standalone).
pub(crate) fn is_builtin(t: &Type, b: Builtin) -> bool {
    matches!(&t.kind, TypeKind::Builtin(x) if *x == b)
}
