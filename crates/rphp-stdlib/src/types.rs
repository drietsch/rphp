//! Type-inspection and scalar-cast builtins.
use rphp_value::Value;

use crate::{nf, Ctx, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("gettype", 1, Some(1), gettype),
    nf!("is_int", 1, Some(1), is_int),
    nf!("is_integer", 1, Some(1), is_int),
    nf!("is_long", 1, Some(1), is_int),
    nf!("is_string", 1, Some(1), is_string),
    nf!("is_bool", 1, Some(1), is_bool),
    nf!("is_float", 1, Some(1), is_float),
    nf!("is_double", 1, Some(1), is_float),
    nf!("is_array", 1, Some(1), is_array),
    nf!("is_null", 1, Some(1), is_null),
    nf!("is_numeric", 1, Some(1), is_numeric),
    nf!("is_scalar", 1, Some(1), is_scalar),
    nf!("intval", 1, Some(2), intval),
    nf!("floatval", 1, Some(1), floatval),
    nf!("doubleval", 1, Some(1), floatval),
    nf!("strval", 1, Some(1), strval),
    nf!("boolval", 1, Some(1), boolval),
];

pub(crate) fn gettype(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let name: &[u8] = match &args[0] {
        Value::Null => b"NULL",
        Value::Bool(_) => b"boolean",
        Value::Int(_) => b"integer",
        Value::Float(_) => b"double",
        Value::Str(_) => b"string",
        Value::Array(_) => b"array",
    };
    Ok(Value::string(name))
}

pub(crate) fn is_int(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(args[0], Value::Int(_))))
}

pub(crate) fn is_string(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(args[0], Value::Str(_))))
}

pub(crate) fn is_bool(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(args[0], Value::Bool(_))))
}

pub(crate) fn is_float(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(args[0], Value::Float(_))))
}

pub(crate) fn is_array(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(args[0], Value::Array(_))))
}

pub(crate) fn is_null(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(args[0], Value::Null)))
}

pub(crate) fn is_numeric(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(args[0].is_numeric()))
}

pub(crate) fn is_scalar(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(matches!(
        args[0],
        Value::Int(_) | Value::Float(_) | Value::Str(_) | Value::Bool(_)
    )))
}

pub(crate) fn intval(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // `intval($s, $base)` parses a *string* in the given base (base 0 auto-detects
    // from a `0x`/`0b`/`0` prefix); for non-strings, or base 10, it is the plain
    // integer cast.
    if let (Value::Str(s), Some(base)) = (&args[0], args.get(1)) {
        let base = base.to_int();
        if base != 10 && (base == 0 || (2..=36).contains(&base)) {
            return Ok(Value::Int(parse_in_base(s.as_bytes(), base)));
        }
    }
    Ok(Value::Int(args[0].to_int()))
}

/// `strtoll`-style lenient base parse used by `intval($s, $base)`: skip leading
/// whitespace and an optional sign, strip the base prefix (`0x`/`0b`/`0o`, or
/// auto-detected when `base == 0`), then consume valid digits and stop at the
/// first one out of range. Overflow saturates, as PHP's does.
fn parse_in_base(s: &[u8], mut base: i64) -> i64 {
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    let neg = i < s.len() && {
        let sign = s[i] == b'-';
        if s[i] == b'+' || s[i] == b'-' {
            i += 1;
        }
        sign
    };
    let has_prefix = |i: usize, c: u8| i + 1 < s.len() && s[i] == b'0' && s[i + 1] | 0x20 == c;
    if base == 0 {
        // Auto-detect the base from a conventional prefix.
        if has_prefix(i, b'x') {
            base = 16;
            i += 2;
        } else if has_prefix(i, b'b') {
            base = 2;
            i += 2;
        } else if i < s.len() && s[i] == b'0' {
            base = 8;
            i += 1;
        } else {
            base = 10;
        }
    } else {
        // An explicit base may still carry its matching prefix; strip it.
        let prefix = match base {
            16 => Some(b'x'),
            2 => Some(b'b'),
            8 => Some(b'o'),
            _ => None,
        };
        if prefix.is_some_and(|c| has_prefix(i, c)) {
            i += 2;
        }
    }
    let mut acc: i64 = 0;
    while i < s.len() {
        let digit = match s[i] {
            c @ b'0'..=b'9' => (c - b'0') as i64,
            c @ b'a'..=b'z' => (c - b'a' + 10) as i64,
            c @ b'A'..=b'Z' => (c - b'A' + 10) as i64,
            _ => break,
        };
        if digit >= base {
            break;
        }
        acc = acc.saturating_mul(base).saturating_add(digit);
        i += 1;
    }
    if neg {
        -acc
    } else {
        acc
    }
}

pub(crate) fn floatval(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Float(args[0].to_float()))
}

pub(crate) fn strval(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Str(rphp_value::Str::from_vec(args[0].to_php_bytes())))
}

pub(crate) fn boolval(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_bool()))
}
