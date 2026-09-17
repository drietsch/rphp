//! Names, class references, member names and constant selectors.

use mago_span::HasSpan;
use mago_syntax::cst::{
    ClassLikeConstantSelector, ClassLikeMemberSelector, Expression, Identifier, LocalIdentifier,
    Variable,
};
use rphp_ast::v2::{ClassRef, ConstSel, Expr, MemberName, Name, NameKind};
use rphp_intern::IdentId;

use super::ctx::{eq_ci, Ctx};
use super::expr;

/// Split a written identifier into its qualification and text.
pub(crate) fn name_parts<'a>(id: &Identifier<'a>) -> (NameKind, &'a [u8]) {
    match id {
        Identifier::Local(l) => (NameKind::Unqualified, l.value),
        Identifier::Qualified(q) => {
            let v = q.value;
            if v.len() > 10 && eq_ci(&v[..10], "namespace\\") {
                (NameKind::Relative, &v[10..])
            } else {
                (NameKind::Qualified, v)
            }
        }
        Identifier::FullyQualified(f) => {
            (NameKind::FullyQualified, f.value.strip_prefix(b"\\").unwrap_or(f.value))
        }
    }
}

/// A written name.
pub(crate) fn name(ctx: &mut Ctx<'_, '_>, id: &Identifier<'_>) -> Name {
    let (kind, text) = name_parts(id);
    let text = ctx.intern(text);
    Name::new(text, kind, ctx.sp(id.span()))
}

/// Intern a local identifier.
pub(crate) fn local(ctx: &mut Ctx<'_, '_>, id: &LocalIdentifier<'_>) -> IdentId {
    ctx.intern(id.value)
}

/// Intern a local identifier and record its span for `TOKEN_PARSE`.
pub(crate) fn local_recorded(ctx: &mut Ctx<'_, '_>, id: &LocalIdentifier<'_>) -> IdentId {
    ctx.ident_span(id.span);
    ctx.intern(id.value)
}

/// The class part of `X::...`, `new X`, `instanceof X`.
pub(crate) fn class_ref(ctx: &mut Ctx<'_, '_>, class: &Expression<'_>) -> ClassRef {
    match class {
        Expression::Identifier(id) => {
            if let Identifier::Local(l) = id {
                if eq_ci(l.value, "self") {
                    return ClassRef::SelfKw(ctx.sp(l.span));
                }
                if eq_ci(l.value, "static") {
                    return ClassRef::Static(ctx.sp(l.span));
                }
                if eq_ci(l.value, "parent") {
                    return ClassRef::Parent(ctx.sp(l.span));
                }
            }
            ClassRef::Named(name(ctx, id))
        }
        Expression::Static(k) => ClassRef::Static(ctx.sp(k.span)),
        Expression::Self_(k) => ClassRef::SelfKw(ctx.sp(k.span)),
        Expression::Parent(k) => ClassRef::Parent(ctx.sp(k.span)),
        Expression::Parenthesized(p) => ClassRef::Expr(Box::new(expr::expr(ctx, p.expression))),
        other => ClassRef::Expr(Box::new(expr::expr(ctx, other))),
    }
}

/// A member selector after `->`, `?->` or `::` (methods and properties).
pub(crate) fn member(ctx: &mut Ctx<'_, '_>, sel: &ClassLikeMemberSelector<'_>) -> MemberName {
    match sel {
        ClassLikeMemberSelector::Identifier(id) => {
            let name = local_recorded(ctx, id);
            MemberName::Ident(name, ctx.sp(id.span))
        }
        ClassLikeMemberSelector::Variable(v) => MemberName::Expr(Box::new(expr::variable(ctx, v))),
        ClassLikeMemberSelector::Expression(e) => {
            MemberName::Expr(Box::new(expr::expr(ctx, e.expression)))
        }
        ClassLikeMemberSelector::Missing(s) => MemberName::Expr(Box::new(Expr::Error(ctx.sp(*s)))),
    }
}

/// The property part of `X::$p` / `X::$$p`.
pub(crate) fn static_prop(ctx: &mut Ctx<'_, '_>, v: &Variable<'_>) -> MemberName {
    match v {
        Variable::Direct(d) => {
            let name = ctx.intern_var(d.name);
            MemberName::Ident(name, ctx.sp(d.span))
        }
        Variable::Indirect(i) => MemberName::Expr(Box::new(expr::expr(ctx, i.expression))),
        Variable::Nested(n) => MemberName::Expr(Box::new(expr::variable(ctx, n.variable))),
    }
}

/// The selector of `X::NAME` / `X::class` / `X::{expr}`.
pub(crate) fn const_sel(ctx: &mut Ctx<'_, '_>, sel: &ClassLikeConstantSelector<'_>) -> ConstSel {
    match sel {
        ClassLikeConstantSelector::Identifier(id) => {
            ctx.ident_span(id.span);
            if eq_ci(id.value, "class") {
                ConstSel::Class(ctx.sp(id.span))
            } else {
                let name = ctx.intern(id.value);
                ConstSel::Ident(name, ctx.sp(id.span))
            }
        }
        ClassLikeConstantSelector::Expression(e) => {
            ConstSel::Expr(Box::new(expr::expr(ctx, e.expression)))
        }
        ClassLikeConstantSelector::Missing(s) => ConstSel::Expr(Box::new(Expr::Error(ctx.sp(*s)))),
    }
}

/// PHP's reserved keywords: never legal as a function, class, constant,
/// label or alias name. `readonly` is a keyword too but PHP allows
/// `function readonly()` for backward compatibility.
const RESERVED: &[&str] = &[
    "abstract", "and", "array", "as", "break", "callable", "case", "catch", "class", "clone",
    "const", "continue", "declare", "default", "die", "do", "echo", "else", "elseif", "empty",
    "enddeclare", "endfor", "endforeach", "endif", "endswitch", "endwhile", "eval", "exit",
    "extends", "final", "finally", "fn", "for", "foreach", "function", "global", "goto", "if",
    "implements", "include", "include_once", "instanceof", "insteadof", "interface", "isset",
    "list", "match", "namespace", "new", "or", "print", "private", "protected", "public",
    "require", "require_once", "return", "static", "switch", "throw", "trait", "try", "unset",
    "use", "var", "while", "xor", "yield", "__halt_compiler", "__class__", "__dir__",
    "__file__", "__function__", "__line__", "__method__", "__namespace__", "__trait__",
];

/// `true` if `text` is a reserved keyword (case-insensitive).
pub(crate) fn is_reserved(text: &[u8]) -> bool {
    RESERVED.iter().any(|k| eq_ci(text, k))
}

/// Reject a reserved keyword used where PHP's grammar wants a plain
/// identifier (`function die()`, `const fn = 1`, `class static {}`,
/// `exit:` labels, `use A as exit`). `expecting` is the grammar's
/// continuation for the message.
pub(crate) fn check_reserved(ctx: &mut Ctx<'_, '_>, id: &LocalIdentifier<'_>, expecting: &str) {
    if is_reserved(id.value) {
        let word = String::from_utf8_lossy(id.value).to_ascii_lowercase();
        let word = if word == "die" { "exit".to_string() } else { word };
        let s = ctx.sp(id.span);
        ctx.reject(
            format!("syntax error, unexpected token \"{word}\", expecting \"{expecting}\""),
            s,
        );
    }
}

/// `readonly` is allowed as a function name but not as a class-like name.
pub(crate) fn check_readonly_name(ctx: &mut Ctx<'_, '_>, id: &LocalIdentifier<'_>) {
    if eq_ci(id.value, "readonly") {
        let s = ctx.sp(id.span);
        ctx.reject(
            "syntax error, unexpected token \"readonly\", expecting identifier",
            s,
        );
    }
}

/// `true` when the identifier spells one of the literal keywords, which a
/// fully qualified `\true` / `\null` still is.
pub(crate) fn literal_keyword(text: &[u8]) -> Option<LiteralKw> {
    if eq_ci(text, "true") {
        Some(LiteralKw::True)
    } else if eq_ci(text, "false") {
        Some(LiteralKw::False)
    } else if eq_ci(text, "null") {
        Some(LiteralKw::Null)
    } else {
        None
    }
}

/// The three literal keywords.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LiteralKw {
    /// `true`
    True,
    /// `false`
    False,
    /// `null`
    Null,
}
