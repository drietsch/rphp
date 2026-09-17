//! String literals: single/double-quoted, heredoc/nowdoc (flexible
//! indentation), backticks and interpolation parts.
//!
//! mago's lexer already strips the closing-marker indentation from heredoc
//! body lines and drops the newline before the closing marker; its parser
//! decodes escapes (`LiteralStringPart::value`). What is left for the
//! adapter: merging literal runs, the in-string expression forms, and the
//! indentation rules mago does not enforce (PHP parse errors).

use mago_span::HasSpan;
use mago_syntax::cst::{
    CompositeString, DocumentIndentation, DocumentKind, DocumentString, LiteralString,
    LiteralStringKind, Sequence, StringPart,
};
use rphp_ast::v2::{Expr, InterpPart};
use rphp_span::Span;

use super::ctx::Ctx;
use super::expr;

const INVALID_UTF8_ESCAPE: &str = "Invalid UTF-8 codepoint escape sequence";

/// A plain (non-interpolated) string literal.
pub(crate) fn literal(ctx: &mut Ctx<'_, '_>, lit: &LiteralString<'_>) -> Expr {
    let span = ctx.sp(lit.span);
    let bytes: &[u8] = match lit.value {
        Some(v) => v,
        None => {
            // Only an invalid `\u{...}` produces `None`; PHP rejects it.
            ctx.invalid_literal(INVALID_UTF8_ESCAPE, span);
            strip_quotes(lit.raw, lit.kind)
        }
    };
    let id = ctx.intern(bytes);
    Expr::Str(id, span)
}

/// The raw content between the quotes (fallback when decoding failed).
fn strip_quotes(raw: &[u8], _kind: LiteralStringKind) -> &[u8] {
    let raw = match raw {
        [b'b' | b'B', rest @ ..] => rest,
        _ => raw,
    };
    if raw.len() >= 2 {
        &raw[1..raw.len() - 1]
    } else {
        raw
    }
}

/// A double-quoted string, heredoc/nowdoc or backtick command.
pub(crate) fn composite(ctx: &mut Ctx<'_, '_>, s: &CompositeString<'_>) -> Expr {
    let span = ctx.sp(s.span());
    match s {
        CompositeString::Interpolated(i) => {
            let parts = parts(ctx, &i.parts);
            finish(ctx, parts, span)
        }
        CompositeString::ShellExecute(sh) => {
            let parts = parts(ctx, &sh.parts);
            Expr::ShellExec { parts, span }
        }
        CompositeString::Document(d) => {
            check_document(ctx, d);
            let parts = parts(ctx, &d.parts);
            match d.kind {
                DocumentKind::Nowdoc => {
                    // Nowdoc parts are verbatim literals; still merge them.
                    finish(ctx, parts, span)
                }
                DocumentKind::Heredoc => finish(ctx, parts, span),
            }
        }
    }
}

/// `Str` when every part is literal, else `Interp`.
fn finish(ctx: &mut Ctx<'_, '_>, parts: Vec<InterpPart>, span: Span) -> Expr {
    if parts.iter().all(|p| matches!(p, InterpPart::Lit(..))) {
        let mut bytes = Vec::new();
        for p in &parts {
            if let InterpPart::Lit(id, _) = p {
                bytes.extend_from_slice(ctx.interner.resolve(*id));
            }
        }
        let id = ctx.intern(&bytes);
        Expr::Str(id, span)
    } else {
        Expr::Interp { parts, span }
    }
}

/// Convert the parts of a composite string, merging adjacent literal runs
/// and dropping empty ones.
pub(crate) fn parts(ctx: &mut Ctx<'_, '_>, seq: &Sequence<'_, StringPart<'_>>) -> Vec<InterpPart> {
    let mut out: Vec<InterpPart> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_span: Option<Span> = None;
    let flush = |ctx: &mut Ctx<'_, '_>,
                 out: &mut Vec<InterpPart>,
                 pending: &mut Vec<u8>,
                 pending_span: &mut Option<Span>| {
        if let Some(sp) = pending_span.take() {
            if !pending.is_empty() {
                let id = ctx.intern(pending);
                out.push(InterpPart::Lit(id, sp));
            }
            pending.clear();
        }
    };
    for part in seq.iter() {
        match part {
            StringPart::Literal(l) => {
                let sp = ctx.sp(l.span);
                let bytes: &[u8] = match l.value {
                    Some(v) => v,
                    None => {
                        ctx.invalid_literal(INVALID_UTF8_ESCAPE, sp);
                        l.raw
                    }
                };
                if bytes.is_empty() {
                    continue;
                }
                pending.extend_from_slice(bytes);
                pending_span = Some(match pending_span {
                    Some(prev) => prev.to(sp),
                    None => sp,
                });
            }
            StringPart::Expression(e) => {
                flush(ctx, &mut out, &mut pending, &mut pending_span);
                check_simple_interpolation(ctx, e);
                out.push(InterpPart::Expr(expr::expr(ctx, e)));
            }
            StringPart::BracedExpression(b) => {
                flush(ctx, &mut out, &mut pending, &mut pending_span);
                out.push(InterpPart::Expr(expr::expr(ctx, b.expression)));
            }
        }
    }
    flush(ctx, &mut out, &mut pending, &mut pending_span);
    out
}

/// The simple interpolation syntax `"$a[...]"` only takes an unquoted
/// identifier, a (possibly negative) integer or a plain variable as the
/// offset; mago parses a full expression there.
fn check_simple_interpolation(ctx: &mut Ctx<'_, '_>, e: &mago_syntax::cst::Expression<'_>) {
    use mago_syntax::cst::{Expression, Identifier, Literal, UnaryPrefix, UnaryPrefixOperator, Variable};
    let Expression::ArrayAccess(a) = e else { return };
    let ok = matches!(
        a.index,
        Expression::Identifier(Identifier::Local(_))
            | Expression::Literal(Literal::Integer(_))
            | Expression::Variable(Variable::Direct(_))
            | Expression::UnaryPrefix(UnaryPrefix {
                operator: UnaryPrefixOperator::Negation(_),
                operand: Expression::Literal(Literal::Integer(_)),
            })
    );
    if !ok {
        let s = ctx.sp(a.index.span());
        ctx.reject(
            "syntax error, unexpected string content, expecting \"-\" or identifier or variable or number",
            s,
        );
    }
}

/// PHP's flexible heredoc/nowdoc indentation rules, which mago's lexer
/// tolerates: the closing marker may not mix tabs and spaces, and every
/// non-blank body line must be indented at least as far as the marker with
/// the same kind of whitespace.
fn check_document(ctx: &mut Ctx<'_, '_>, d: &DocumentString<'_>) {
    let (indent, using_spaces) = match d.indentation {
        DocumentIndentation::None => return,
        DocumentIndentation::Whitespace(n) => (n, true),
        DocumentIndentation::Tab(n) => (n, false),
        DocumentIndentation::Mixed(..) => {
            ctx.invalid_literal(
                "Invalid indentation - tabs and spaces cannot be mixed",
                ctx.sp(d.close),
            );
            return;
        }
    };
    // Byte ranges covered by embedded expressions: their inner lines are not
    // literal body lines.
    let embedded: Vec<(u32, u32)> = d
        .parts
        .iter()
        .filter_map(|p| match p {
            StringPart::Expression(e) => Some(e.span()),
            StringPart::BracedExpression(b) => Some(b.span()),
            StringPart::Literal(_) => None,
        })
        .map(|s| (s.start.offset, s.end.offset))
        .collect();
    let body_start = d.open.end.offset as usize;
    let body_end = d.close.start.offset as usize;
    if body_start >= body_end || body_end > ctx.src.len() {
        return;
    }
    let src = ctx.src;
    let mut pos = body_start;
    // Skip the newline that terminates the opener line.
    if src.get(pos) == Some(&b'\r') {
        pos += 1;
    }
    if src.get(pos) == Some(&b'\n') {
        pos += 1;
    }
    while pos < body_end {
        let line_start = pos;
        let mut end = pos;
        while end < body_end && src[end] != b'\n' && src[end] != b'\r' {
            end += 1;
        }
        let inside_expr = embedded
            .iter()
            .any(|&(lo, hi)| (lo as usize) < line_start && line_start < (hi as usize));
        if !inside_expr {
            let line = &src[line_start..end];
            let mut skipped = 0usize;
            let mut bad_mix = false;
            for &b in line.iter().take(indent) {
                match b {
                    b' ' if using_spaces => skipped += 1,
                    b'\t' if !using_spaces => skipped += 1,
                    b' ' | b'\t' => {
                        bad_mix = true;
                        break;
                    }
                    _ => break,
                }
            }
            let span = ctx.span(line_start as u32, end as u32);
            if bad_mix {
                ctx.invalid_literal("Invalid indentation - tabs and spaces cannot be mixed", span);
                return;
            }
            if skipped < indent && line.len() > skipped {
                ctx.invalid_literal(
                    format!(
                        "Invalid body indentation level (expecting an indentation level of at least {indent})"
                    ),
                    span,
                );
                return;
            }
        }
        // Advance past the line terminator.
        pos = end;
        if src.get(pos) == Some(&b'\r') {
            pos += 1;
        }
        if src.get(pos) == Some(&b'\n') {
            pos += 1;
        }
        if pos == line_start {
            break;
        }
    }
}
