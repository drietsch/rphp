//! Binary-to-text codecs (php-src `ext/standard/{base64.c, quot_print.c,
//! uuencode.c}`): `base64_encode`/`base64_decode` (lenient and strict),
//! `quoted_printable_encode`/`quoted_printable_decode`, `convert_uuencode`/
//! `convert_uudecode`. (`bin2hex`/`hex2bin` live in `strings.rs`.)
use rphp_value::Value;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};

use crate::strings::str_value;

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("base64_encode", 1, Some(1), base64_encode),
    nf!("base64_decode", 1, Some(2), base64_decode),
    nf!("quoted_printable_encode", 1, Some(1), quoted_printable_encode),
    nf!("quoted_printable_decode", 1, Some(1), quoted_printable_decode),
    nf!("convert_uuencode", 1, Some(1), convert_uuencode),
    nf!("convert_uudecode", 1, Some(1), convert_uudecode),
];

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with `=` padding.
pub(crate) fn encode_base64(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63]);
        out.push(B64[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] } else { b'=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] } else { b'=' });
    }
    out
}

/// The value of a base64 alphabet byte, `Whitespace` for the skipped
/// separators, `Invalid` otherwise.
enum B64Class {
    Value(u32),
    Whitespace,
    Invalid,
}

fn b64_class(c: u8) -> B64Class {
    match c {
        b'A'..=b'Z' => B64Class::Value((c - b'A') as u32),
        b'a'..=b'z' => B64Class::Value((c - b'a' + 26) as u32),
        b'0'..=b'9' => B64Class::Value((c - b'0' + 52) as u32),
        b'+' => B64Class::Value(62),
        b'/' => B64Class::Value(63),
        b' ' | b'\t' | b'\n' | b'\r' => B64Class::Whitespace,
        _ => B64Class::Invalid,
    }
}

/// php's `php_base64_decode_ex`: lenient mode skips anything that is not in
/// the alphabet; strict mode skips only whitespace, refuses data after
/// padding, a lone trailing sextet and malformed padding.
pub(crate) fn decode_base64(data: &[u8], strict: bool) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    let mut i = 0usize; // sextets consumed
    let mut padding = 0usize;
    let mut acc: u32 = 0;
    for &c in data {
        if c == b'=' {
            padding += 1;
            continue;
        }
        let v = match b64_class(c) {
            B64Class::Value(v) => v,
            B64Class::Whitespace => continue,
            B64Class::Invalid => {
                if strict {
                    return None;
                }
                continue;
            }
        };
        if strict && padding > 0 {
            return None;
        }
        match i % 4 {
            0 => acc = v << 2,
            1 => {
                out.push((acc | (v >> 4)) as u8);
                acc = (v & 0x0f) << 4;
            }
            2 => {
                out.push((acc | (v >> 2)) as u8);
                acc = (v & 0x03) << 6;
            }
            _ => out.push((acc | v) as u8),
        }
        i += 1;
    }
    if strict && i % 4 == 1 {
        return None;
    }
    if strict && padding > 0 && (padding > 2 || !(i + padding).is_multiple_of(4)) {
        return None;
    }
    Some(out)
}

/// `base64_encode(string $string): string`.
pub(crate) fn base64_encode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(str_value(encode_base64(&args[0].to_php_bytes())))
}

/// `base64_decode(string $string, bool $strict = false): string|false`.
pub(crate) fn base64_decode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let strict = args.get(1).is_some_and(Value::to_bool);
    Ok(match decode_base64(&args[0].to_php_bytes(), strict) {
        Some(v) => str_value(v),
        None => Value::Bool(false),
    })
}

/// `quoted_printable_encode(string $string): string` — RFC 2045 with soft
/// line breaks at 75 columns (`php_quot_print_encode`).
pub(crate) fn quoted_printable_encode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const MAXL: usize = 75;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len() + s.len() / 4);
    let mut lp = 0usize;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let next = s.get(i + 1).copied();
        if c == b'\r' && next == Some(b'\n') {
            out.extend_from_slice(b"\r\n");
            i += 2;
            lp = 0;
            continue;
        }
        let needs_hex = c < 0x20 || c == 0x7f || c >= 0x80 || c == b'=' || (c == b' ' && next == Some(b'\r'));
        if needs_hex {
            lp += 3;
            let wrap = (lp > MAXL && c <= 0x7f)
                || ((0x80..=0xdf).contains(&c) && lp + 3 > MAXL)
                || ((0xe0..=0xef).contains(&c) && lp + 6 > MAXL)
                || ((0xf0..=0xf4).contains(&c) && lp + 9 > MAXL);
            if wrap {
                out.extend_from_slice(b"=\r\n");
                lp = 3;
            }
            out.push(b'=');
            out.push(HEX[(c >> 4) as usize]);
            out.push(HEX[(c & 0xf) as usize]);
        } else {
            lp += 1;
            if lp > MAXL {
                out.extend_from_slice(b"=\r\n");
                lp = 1;
            }
            out.push(c);
        }
        i += 1;
    }
    Ok(str_value(out))
}

/// `quoted_printable_decode(string $string): string` (`php_quot_print_decode`).
pub(crate) fn quoted_printable_decode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] != b'=' {
            out.push(s[i]);
            i += 1;
            continue;
        }
        if i + 2 < s.len() && s[i + 1].is_ascii_hexdigit() && s[i + 2].is_ascii_hexdigit() {
            let hi = (s[i + 1] as char).to_digit(16).unwrap();
            let lo = (s[i + 2] as char).to_digit(16).unwrap();
            out.push(((hi << 4) | lo) as u8);
            i += 3;
            continue;
        }
        // A soft line break: "=" + optional blanks + CR[LF] / LF, or "=" +
        // blanks at the very end; anything else keeps the "=" literally.
        let mut k = i + 1;
        while k < s.len() && (s[k] == b' ' || s[k] == b'\t') {
            k += 1;
        }
        if k >= s.len() {
            i = k;
        } else if s[k] == b'\r' {
            k += 1;
            if k < s.len() && s[k] == b'\n' {
                k += 1;
            }
            i = k;
        } else if s[k] == b'\n' {
            i = k + 1;
        } else {
            out.push(b'=');
            i += 1;
        }
    }
    Ok(str_value(out))
}

/// One uuencoded 6-bit value (`0` is written as a backquote).
fn uu_enc(v: u8) -> u8 {
    if v == 0 {
        b'`'
    } else {
        (v & 0o77) + b' '
    }
}

fn uu_dec(c: u8) -> u8 {
    c.wrapping_sub(b' ') & 0o77
}

/// `convert_uuencode(string $string): string` — 45-byte lines, each with a
/// length byte, terminated by a "`" line.
pub(crate) fn convert_uuencode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len() * 4 / 3 + s.len() / 45 * 2 + 4);
    for line in s.chunks(45) {
        out.push(uu_enc(line.len() as u8));
        for group in line.chunks(3) {
            let b0 = group[0];
            let b1 = group.get(1).copied().unwrap_or(0);
            let b2 = group.get(2).copied().unwrap_or(0);
            out.push(uu_enc(b0 >> 2));
            out.push(uu_enc(((b0 << 4) & 0o60) | ((b1 >> 4) & 0o17)));
            out.push(uu_enc(((b1 << 2) & 0o74) | ((b2 >> 6) & 0o3)));
            out.push(uu_enc(b2 & 0o77));
        }
        out.push(b'\n');
    }
    out.extend_from_slice(b"`\n");
    Ok(str_value(out))
}

/// php's `php_uudecode`: `None` for malformed input.
fn uudecode(src: &[u8]) -> Option<Vec<u8>> {
    let max_len = (src.len() as f64 * 0.75).ceil() as usize;
    let mut out = Vec::with_capacity(max_len);
    let mut s = 0usize;
    let mut total_len = 0usize;
    while s < src.len() && src[s] != b'`' {
        let len = uu_dec(src[s]) as usize;
        if len > src.len() {
            return None;
        }
        total_len += len;
        if total_len > max_len {
            return None;
        }
        s += 1;
        let ee = s + if len == 45 { 60 } else { (len as f64 * 1.33).floor() as usize };
        if ee > src.len() {
            return None;
        }
        while s < ee {
            if s + 4 > src.len() {
                return None;
            }
            out.push((uu_dec(src[s]) << 2) | (uu_dec(src[s + 1]) >> 4));
            out.push((uu_dec(src[s + 1]) << 4) | (uu_dec(src[s + 2]) >> 2));
            out.push((uu_dec(src[s + 2]) << 6) | uu_dec(src[s + 3]));
            s += 4;
        }
        if len < 45 {
            break;
        }
        // Skip the newline.
        s += 1;
    }
    if total_len > out.len() {
        return None;
    }
    out.truncate(total_len);
    Some(out)
}

/// `convert_uudecode(string $string): string|false`.
pub(crate) fn convert_uudecode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let decoded = if s.is_empty() { None } else { uudecode(&s) };
    match decoded {
        Some(v) => Ok(str_value(v)),
        None => {
            ctx.warn("convert_uudecode(): Argument #1 ($data) is not a valid uuencoded string")?;
            Ok(Value::Bool(false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_strict_rules() {
        assert_eq!(encode_base64(b"foo"), b"Zm9v");
        assert_eq!(decode_base64(b" Zm 9v ", true), Some(b"foo".to_vec()));
        assert_eq!(decode_base64(b"Zm9v=", true), None);
        assert_eq!(decode_base64(b"Zm8", true), Some(b"fo".to_vec()));
        assert_eq!(decode_base64(b"Z", true), None);
        assert_eq!(decode_base64(b"Zm9v!", false), Some(b"foo".to_vec()));
        assert_eq!(decode_base64(b"Zm9v\x0b", true), None);
    }

    #[test]
    fn uuencode_round_trip() {
        let mut it = crate::tests::interp();
        let enc = it.call_function(b"convert_uuencode", &[Value::string(b"test\ntext text\r\n")]).unwrap();
        assert_eq!(enc.to_php_bytes(), b"0=&5S=`IT97AT('1E>'0-\"@``\n`\n");
        let dec = it.call_function(b"convert_uudecode", &[enc]).unwrap();
        assert_eq!(dec.to_php_bytes(), b"test\ntext text\r\n");
        assert_eq!(uudecode(b"0V%T"), None);
        assert_eq!(uudecode(b"`\n"), Some(Vec::new()));
    }
}
