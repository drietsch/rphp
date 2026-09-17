//! `ctype` extension — character-class predicates (`ctype_alpha`, `ctype_digit`,
//! …), a transcription of php-src's `ext/ctype/ctype.c` (`ctype_impl`). Each
//! tests whether **every byte** of a string belongs to a class and treats the
//! empty string as `false`.
//!
//! Classification is ASCII-only and locale-independent (the `C`/`POSIX` locale
//! that production PHP runs under). PHP's `ctype` is nominally locale-sensitive,
//! but the portable behaviour rphp targets — and what these predicates encode —
//! is the ASCII table, mirroring Rust's `u8::is_ascii_*` family.
//!
//! A **non-string** argument raises php 8.1's deprecation (`Argument of type
//! int will be interpreted as string in the future`) and then follows the
//! legacy rules: an int in `0..=255` is the byte of that code, `-128..=-1` the
//! byte `+256`, any larger int answers the predicate's "digits allowed" bit and
//! any smaller one its "minus allowed" bit; every other type is `false`.
use rphp_value::Value;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`). Every predicate has
/// the same shape: one `mixed` argument, returns `bool`.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("ctype_alnum", 1, Some(1), ctype_alnum),
    nf!("ctype_alpha", 1, Some(1), ctype_alpha),
    nf!("ctype_cntrl", 1, Some(1), ctype_cntrl),
    nf!("ctype_digit", 1, Some(1), ctype_digit),
    nf!("ctype_graph", 1, Some(1), ctype_graph),
    nf!("ctype_lower", 1, Some(1), ctype_lower),
    nf!("ctype_print", 1, Some(1), ctype_print),
    nf!("ctype_punct", 1, Some(1), ctype_punct),
    nf!("ctype_space", 1, Some(1), ctype_space),
    nf!("ctype_upper", 1, Some(1), ctype_upper),
    nf!("ctype_xdigit", 1, Some(1), ctype_xdigit),
];

/// `ctype_alnum($text)`.
pub(crate) fn ctype_alnum(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_alnum", &args[0], |b| b.is_ascii_alphanumeric(), true, false)
}

/// `ctype_alpha($text)`.
pub(crate) fn ctype_alpha(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_alpha", &args[0], |b| b.is_ascii_alphabetic(), false, false)
}

/// `ctype_cntrl($text)`.
pub(crate) fn ctype_cntrl(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_cntrl", &args[0], |b| b.is_ascii_control(), false, false)
}

/// `ctype_digit($text)`.
pub(crate) fn ctype_digit(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_digit", &args[0], |b| b.is_ascii_digit(), true, false)
}

/// `ctype_graph($text)`.
pub(crate) fn ctype_graph(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_graph", &args[0], |b| b.is_ascii_graphic(), true, true)
}

/// `ctype_lower($text)`.
pub(crate) fn ctype_lower(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_lower", &args[0], |b| b.is_ascii_lowercase(), false, false)
}

/// `ctype_print($text)`: graphic plus the space.
pub(crate) fn ctype_print(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_print", &args[0], |b| (0x20..=0x7e).contains(&b), true, true)
}

/// `ctype_punct($text)`.
pub(crate) fn ctype_punct(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_punct", &args[0], |b| b.is_ascii_punctuation(), false, false)
}

/// `ctype_space($text)`: php-src's whitespace set — space, `\t`, `\n`, `\v`,
/// `\f`, `\r`.
pub(crate) fn ctype_space(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(
        ctx,
        "ctype_space",
        &args[0],
        |b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c),
        false,
        false,
    )
}

/// `ctype_upper($text)`.
pub(crate) fn ctype_upper(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_upper", &args[0], |b| b.is_ascii_uppercase(), false, false)
}

/// `ctype_xdigit($text)`.
pub(crate) fn ctype_xdigit(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctype_impl(ctx, "ctype_xdigit", &args[0], |b| b.is_ascii_hexdigit(), true, false)
}

// ---- helpers ----------------------------------------------------------------

/// php's `ctype_impl`: a string is tested byte by byte (empty → `false`);
/// anything else raises the 8.1 deprecation and follows the legacy integer
/// rules (`allow_digits` for ints above 255, `allow_minus` below -128).
fn ctype_impl(
    ctx: &mut Ctx,
    func: &str,
    arg: &Value,
    class: impl Fn(u8) -> bool,
    allow_digits: bool,
    allow_minus: bool,
) -> NativeResult {
    let arg = arg.deref().into_owned();
    if let Value::Str(s) = &arg {
        let bytes = s.as_bytes();
        return Ok(Value::Bool(!bytes.is_empty() && bytes.iter().all(|&b| class(b))));
    }
    ctx.deprecated(&format!(
        "{func}(): Argument of type {} will be interpreted as string in the future",
        type_name(&arg)
    ))?;
    Ok(Value::Bool(match arg {
        Value::Int(n) if (0..=255).contains(&n) => class(n as u8),
        Value::Int(n) if (-128..0).contains(&n) => class((n + 256) as u8),
        Value::Int(n) if n >= 0 => allow_digits,
        Value::Int(_) => allow_minus,
        _ => false,
    }))
}

/// `zend_zval_value_name` as the deprecation prints it: `int`, `float`,
/// `bool`, `null`, `array`, the class name of an object.
fn type_name(v: &Value) -> String {
    match v {
        Value::Bool(_) => "bool".to_string(),
        other => rphp_runtime::value_name(other),
    }
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::call_named;

    #[test]
    fn strings_are_tested_byte_by_byte() {
        assert_eq!(call_named(b"ctype_digit", &[Value::string(b"0123")]), Value::Bool(true));
        assert_eq!(call_named(b"ctype_digit", &[Value::string(b"12a")]), Value::Bool(false));
        assert_eq!(call_named(b"ctype_digit", &[Value::string(b"")]), Value::Bool(false));
        assert_eq!(call_named(b"ctype_space", &[Value::string(b" \t\x0b\x0c")]), Value::Bool(true));
    }

    #[test]
    fn ints_follow_the_legacy_rules() {
        assert_eq!(call_named(b"ctype_digit", &[Value::Int(53)]), Value::Bool(true));
        assert_eq!(call_named(b"ctype_digit", &[Value::Int(5)]), Value::Bool(false));
        assert_eq!(call_named(b"ctype_digit", &[Value::Int(256)]), Value::Bool(true));
        assert_eq!(call_named(b"ctype_punct", &[Value::Int(-500)]), Value::Bool(false));
        assert_eq!(call_named(b"ctype_graph", &[Value::Int(-300)]), Value::Bool(true));
        assert_eq!(call_named(b"ctype_space", &[Value::Int(-224)]), Value::Bool(false));
        assert_eq!(call_named(b"ctype_alpha", &[Value::Int(-159)]), Value::Bool(true));
        assert_eq!(call_named(b"ctype_digit", &[Value::Float(5.0)]), Value::Bool(false));
        assert_eq!(call_named(b"ctype_digit", &[Value::Null]), Value::Bool(false));
    }
}
