//! Attribute groups `#[A, B(1, x: 2)]`.

use mago_span::HasSpan;
use mago_syntax::cst::{AttributeList, Sequence};
use rphp_ast::v2::{Attr, AttrGroup};

use super::conformance::{check_const_expr, ConstCtx};
use super::ctx::Ctx;
use super::expr;
use super::names;

/// Convert every attribute group of a declaration.
pub(crate) fn groups(ctx: &mut Ctx<'_, '_>, lists: &Sequence<'_, AttributeList<'_>>) -> Vec<AttrGroup> {
    lists.iter().map(|l| group(ctx, l)).collect()
}

/// Convert one `#[...]` group.
pub(crate) fn group(ctx: &mut Ctx<'_, '_>, list: &AttributeList<'_>) -> AttrGroup {
    let attrs = list
        .attributes
        .iter()
        .map(|a| {
            let name = names::name(ctx, &a.name);
            let args = match &a.argument_list {
                Some(al) => {
                    ctx.const_depth += 1;
                    let args = expr::partial_args(ctx, al, "Cannot create Closure as attribute argument");
                    ctx.const_depth -= 1;
                    for arg in &args {
                        if arg.spread {
                            ctx.reject("Cannot use unpacking in attribute argument list", arg.span);
                        }
                        check_const_expr(ctx, &arg.value, ConstCtx::AttrArg);
                    }
                    args
                }
                None => Vec::new(),
            };
            Attr {
                name,
                args,
                span: ctx.sp(a.span()),
            }
        })
        .collect();
    AttrGroup {
        attrs,
        span: ctx.sp(list.span()),
    }
}
