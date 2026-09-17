//! Doc-comment attachment via mago's trivia.
//!
//! PHP keeps the last `/** */` seen before a declaration keyword (attributes
//! in between do not reset it), so a declaration with attributes is checked
//! at two positions: before the first attribute group, then — when nothing
//! was found there — before the token that follows the attributes.

use mago_span::HasSpan;
use mago_syntax::comments::docblock::get_docblock_before_position;
use mago_syntax::cst::{AttributeList, Sequence};
use rphp_intern::IdentId;

use super::ctx::Ctx;

/// The docblock directly preceding `offset`, if any, interned.
pub(crate) fn docblock_before(ctx: &mut Ctx<'_, '_>, offset: u32) -> Option<IdentId> {
    let trivia = get_docblock_before_position(ctx.trivia, offset)?;
    Some(ctx.intern(trivia.value))
}

/// The docblock of a declaration: before its attributes, else before the
/// first token after them (`after_attrs` is that token's start offset).
pub(crate) fn decl_doc(
    ctx: &mut Ctx<'_, '_>,
    attrs: &Sequence<'_, AttributeList<'_>>,
    after_attrs: u32,
) -> Option<IdentId> {
    match attrs.first() {
        Some(first) => docblock_before(ctx, first.span().start.offset)
            .or_else(|| docblock_before(ctx, after_attrs)),
        None => docblock_before(ctx, after_attrs),
    }
}
