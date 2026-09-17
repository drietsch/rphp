//! Integer and float literals: every PHP form (`0x`, `0o`, `0b`, legacy
//! octal, `_` separators), with integer overflow widening to float exactly
//! as Zend does.

use mago_syntax::cst::{LiteralFloat, LiteralInteger};
use rphp_ast::v2::Expr;

use super::ctx::Ctx;

/// The decoded value of an integer literal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum IntValue {
    /// Fits in a PHP `int`.
    Int(i64),
    /// Overflowed: PHP produces a float of the literal's magnitude.
    Float(f64),
    /// Not a numeric literal at all (lexer/adapter disagreement).
    Invalid,
}

/// Decode the raw spelling of an integer literal.
///
/// Decimal overflow goes through `strtod` semantics (correctly rounded, like
/// `zend_strtod`); the other bases accumulate in a double one digit at a time
/// like `zend_hex_strtod` / `zend_oct_strtod` / `zend_bin_strtod`.
pub(crate) fn parse_int(raw: &[u8]) -> IntValue {
    let digits: Vec<u8> = raw.iter().copied().filter(|&b| b != b'_').collect();
    if digits.is_empty() {
        return IntValue::Invalid;
    }
    let (radix, body): (u32, &[u8]) = match digits.as_slice() {
        [b'0', b'x' | b'X', rest @ ..] => (16, rest),
        [b'0', b'o' | b'O', rest @ ..] => (8, rest),
        [b'0', b'b' | b'B', rest @ ..] => (2, rest),
        [b'0', rest @ ..] if !rest.is_empty() => (8, rest),
        all => (10, all),
    };
    if body.is_empty() {
        return if radix == 8 { IntValue::Int(0) } else { IntValue::Invalid };
    }
    let mut acc: u128 = 0;
    let mut overflow = false;
    for &b in body {
        let d = match (b as char).to_digit(radix) {
            Some(d) => d,
            None => return IntValue::Invalid,
        };
        if !overflow {
            match acc
                .checked_mul(u128::from(radix))
                .and_then(|v| v.checked_add(u128::from(d)))
            {
                Some(v) => acc = v,
                None => overflow = true,
            }
        }
    }
    if !overflow && acc <= i64::MAX as u128 {
        return IntValue::Int(acc as i64);
    }
    if radix == 10 {
        let text = std::str::from_utf8(body).unwrap_or("0");
        return IntValue::Float(text.parse::<f64>().unwrap_or(f64::INFINITY));
    }
    let mut f = 0.0f64;
    for &b in body {
        let d = (b as char).to_digit(radix).unwrap_or(0);
        f = f * f64::from(radix) + f64::from(d);
    }
    IntValue::Float(f)
}

/// An integer literal node.
pub(crate) fn int(ctx: &mut Ctx<'_, '_>, lit: &LiteralInteger<'_>) -> Expr {
    let span = ctx.sp(lit.span);
    if let Some(i) = bad_separator(lit.raw) {
        let tail: String = lit.raw[i..].iter().map(|&c| c as char).collect();
        ctx.reject(
            format!("syntax error, unexpected identifier \"{tail}\""),
            ctx.span(span.lo + i as u32, span.hi),
        );
    }
    match parse_int(lit.raw) {
        IntValue::Int(v) => Expr::Int(v, span),
        IntValue::Float(f) => Expr::Float(f, span),
        IntValue::Invalid => {
            ctx.invalid_literal("Invalid numeric literal", span);
            Expr::Int(0, span)
        }
    }
}

/// PHP only allows `_` between two digits; mago's lexer also takes `100._0`
/// and `1_.5`. Returns the offending tail for the message.
fn bad_separator(raw: &[u8]) -> Option<usize> {
    for (i, &b) in raw.iter().enumerate() {
        if b != b'_' {
            continue;
        }
        let before = i.checked_sub(1).map(|j| raw[j]).is_some_and(|c| c.is_ascii_hexdigit());
        let after = raw.get(i + 1).is_some_and(|c| c.is_ascii_hexdigit());
        if !before || !after {
            return Some(i);
        }
    }
    None
}

/// A float literal node (`_` separators removed, `strtod` semantics).
pub(crate) fn float(ctx: &mut Ctx<'_, '_>, lit: &LiteralFloat<'_>) -> Expr {
    let span = ctx.sp(lit.span);
    if let Some(i) = bad_separator(lit.raw) {
        let tail: String = lit.raw[i..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
            .map(|&c| c as char)
            .collect();
        ctx.reject(
            format!("syntax error, unexpected identifier \"{tail}\""),
            ctx.span(span.lo + i as u32, span.hi),
        );
    }
    let text: String = lit
        .raw
        .iter()
        .filter(|&&b| b != b'_')
        .map(|&b| b as char)
        .collect();
    match text.parse::<f64>() {
        Ok(f) => Expr::Float(f, span),
        Err(_) => {
            ctx.invalid_literal("Invalid numeric literal", span);
            Expr::Float(0.0, span)
        }
    }
}
