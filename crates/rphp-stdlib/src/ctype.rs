//! `ctype` extension — character-class predicates (`ctype_alpha`, `ctype_digit`,
//! …). Each tests whether **every byte** of its argument belongs to a class and
//! treats the empty string as `false`, exactly like php-src's `ctype.c`.
//!
//! Classification is ASCII-only and locale-independent (the `C`/`POSIX` locale
//! that production PHP runs under). PHP's `ctype` is nominally locale-sensitive,
//! but the portable behaviour rphp targets — and what these predicates encode —
//! is the ASCII table, mirroring Rust's `u8::is_ascii_*` family.
//!
//! A legacy quirk is replicated: a **non-string** argument never matches except
//! for integers, which PHP reinterprets — an int in `-128..=255` is the byte of
//! that code, anything else is rendered as its decimal string and checked
//! digit-by-digit (so `ctype_digit(48)` is `true` via the byte `'0'`, and
//! `ctype_digit(256)` is `true` via the string `"256"`).
use rphp_value::Value;

use crate::{nf, Ctx, NativeFn, NativeResult};

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

pub(crate) fn ctype_alnum(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_alphanumeric())
}

pub(crate) fn ctype_alpha(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_alphabetic())
}

pub(crate) fn ctype_cntrl(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_control())
}

pub(crate) fn ctype_digit(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_digit())
}

pub(crate) fn ctype_graph(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_graphic())
}

pub(crate) fn ctype_lower(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_lowercase())
}

pub(crate) fn ctype_print(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // Printable = graphic plus the space; unlike `is_ascii_graphic`, `0x20` counts.
    predicate(&args[0], |b| (0x20..=0x7e).contains(&b))
}

pub(crate) fn ctype_punct(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_punctuation())
}

pub(crate) fn ctype_space(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // php-src's whitespace set: space, \t, \n, \v (0x0b), \f (0x0c), \r.
    predicate(&args[0], |b| {
        matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
    })
}

pub(crate) fn ctype_upper(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_uppercase())
}

pub(crate) fn ctype_xdigit(_: &mut Ctx, args: &[Value]) -> NativeResult {
    predicate(&args[0], |b| b.is_ascii_hexdigit())
}

// ---- helpers ----------------------------------------------------------------

/// Apply a per-byte class test with PHP's argument rules and wrap the result.
fn predicate(arg: &Value, class: impl Fn(u8) -> bool) -> NativeResult {
    Ok(Value::Bool(matches_class(arg, class)))
}

/// `true` when every byte of `arg` is in the class. A string is taken verbatim
/// (empty → `false`); an integer follows PHP's legacy reinterpretation; any
/// other type (`null`, `bool`, `float`, `array`) never matches.
fn matches_class(arg: &Value, class: impl Fn(u8) -> bool) -> bool {
    match arg {
        Value::Str(s) => {
            let bytes = s.as_bytes();
            !bytes.is_empty() && bytes.iter().all(|&b| class(b))
        }
        Value::Int(n) => int_matches(*n, class),
        _ => false,
    }
}

/// PHP's integer special case: a code in `-128..=255` is that single byte
/// (negatives wrap by `+256`); otherwise the int is rendered as its decimal
/// string and every byte of it must be in the class. The decimal form is never
/// empty, so the "empty is false" rule needs no extra guard here.
fn int_matches(n: i64, class: impl Fn(u8) -> bool) -> bool {
    if (-128..=255).contains(&n) {
        let byte = if n < 0 { (n + 256) as u8 } else { n as u8 };
        class(byte)
    } else {
        n.to_string().bytes().all(class)
    }
}
