//! The Chinese and Korean double-byte charsets. The code-point tables come
//! from `encoding_rs` (WHATWG Encoding Standard); the stateful
//! escape-sequence encodings php layers over those tables — ISO-2022-KR
//! and HZ (`mbfilter_cjk.c`) — and the GB 2312 subset (`EUC-CN`) are
//! implemented here. (The Japanese family lives in [`super::jis`].)
//!
//! Known divergences from libmbfl's own tables (see `COVERAGE.md`): `BIG-5`
//! uses the Big5-HKSCS superset, `EUC-KR` the UHC superset, and `EUC-TW`
//! is recognised by name only.

use super::{illegal_output, ConvertBuf, Encoding, BAD_INPUT, UTF32_MAX};

macro_rules! mblen {
    ($($lo:literal..=$hi:literal => $n:literal),* $(,)?) => {{
        let mut t = [1u8; 256];
        $( let mut i = $lo; while i <= $hi { t[i] = $n; i += 1; } )*
        t
    }};
}

/// `mblen_table_sjis` (0x81–0x9F, 0xE0–0xEF).
pub static MBLEN_SJIS: [u8; 256] = mblen!(0x81..=0x9F => 2, 0xE0..=0xEF => 2);
/// `mblen_table_sjismac` (0x81–0x9F, 0xE0–0xED).
pub static MBLEN_SJIS_MAC: [u8; 256] = mblen!(0x81..=0x9F => 2, 0xE0..=0xED => 2);
/// `mblen_table_sjis_mobile` (0x81–0x9F, 0xE0–0xFC).
pub static MBLEN_SJIS_MOBILE: [u8; 256] = mblen!(0x81..=0x9F => 2, 0xE0..=0xFC => 2);
/// `mblen_table_sjiswin` (0x81–0x9F, 0xE0–0xFF).
pub static MBLEN_SJISWIN: [u8; 256] = mblen!(0x81..=0x9F => 2, 0xE0..=0xFF => 2);
/// `mblen_table_eucjp` (0xA1–0xFE, 0x8E → 2, 0x8F → 3).
pub static MBLEN_EUCJP: [u8; 256] = mblen!(0x8E..=0x8E => 2, 0x8F..=0x8F => 3, 0xA1..=0xFE => 2);
/// `mblen_table_euccn` (0xA1–0xFE).
pub static MBLEN_EUCCN: [u8; 256] = mblen!(0xA1..=0xFE => 2);
/// `mblen_table_81_to_fe`.
pub static MBLEN_81_TO_FE: [u8; 256] = mblen!(0x81..=0xFE => 2);

/// Single bytes libmbfl rejects but the WHATWG decoders map (Shift_JIS:
/// 0x80, 0xA0, 0xFD–0xFF to U+0080 / U+F8F0–U+F8F3; GBK: 0x80 to the euro
/// sign, 0xFF to U+F8F5).
fn is_rejected_single(enc: &'static encoding_rs::Encoding, b: u8) -> bool {
    if std::ptr::eq(enc, encoding_rs::SHIFT_JIS) {
        matches!(b, 0x80 | 0xA0 | 0xFD..=0xFF)
    } else if std::ptr::eq(enc, encoding_rs::GBK) {
        matches!(b, 0x80 | 0xFF)
    } else {
        false
    }
}

/// Decode with an `encoding_rs` decoder; each malformed sequence becomes one
/// [`BAD_INPUT`].
pub fn decode_rs(enc: &'static encoding_rs::Encoding, input: &[u8], out: &mut Vec<u32>) {
    // Split around the single bytes php rejects, decoding the runs between.
    let mut start = 0;
    for (i, &b) in input.iter().enumerate() {
        if is_rejected_single(enc, b) {
            if i > start {
                decode_rs_run(enc, &input[start..i], out);
            }
            out.push(BAD_INPUT);
            start = i + 1;
        }
    }
    if start < input.len() || input.is_empty() {
        decode_rs_run(enc, &input[start..], out);
    }
}

fn decode_rs_run(enc: &'static encoding_rs::Encoding, input: &[u8], out: &mut Vec<u32>) {
    let mut dec = enc.new_decoder_without_bom_handling();
    let mut src = input;
    let mut dst = vec![0u16; 64];
    let mut pending_hi: Option<u16> = None;
    loop {
        let (res, read, written) = dec.decode_to_utf16_without_replacement(src, &mut dst, true);
        for &u in &dst[..written] {
            match pending_hi.take() {
                Some(hi) => out.push((((hi as u32) & 0x3FF) << 10) + (u as u32 & 0x3FF) + 0x10000),
                None if (0xD800..=0xDBFF).contains(&u) => pending_hi = Some(u),
                None => out.push(u as u32),
            }
        }
        src = &src[read..];
        match res {
            encoding_rs::DecoderResult::InputEmpty => break,
            encoding_rs::DecoderResult::OutputFull => {}
            encoding_rs::DecoderResult::Malformed(_, _) => out.push(BAD_INPUT),
        }
    }
    if pending_hi.is_some() {
        out.push(BAD_INPUT);
    }
}

/// Encode with an `encoding_rs` encoder; unmappable code points (and wchars
/// that are not Unicode scalar values) go through [`illegal_output`].
pub fn encode_rs(enc: &Encoding, rs: &'static encoding_rs::Encoding, input: &[u32], buf: &mut ConvertBuf) {
    let mut encoder = rs.new_encoder();
    let mut units: Vec<u16> = Vec::with_capacity(input.len());
    let mut dst = vec![0u8; 64];
    let flush = |encoder: &mut encoding_rs::Encoder, units: &mut Vec<u16>, buf: &mut ConvertBuf, dst: &mut Vec<u8>| {
        let mut src: &[u16] = units;
        loop {
            let (res, read, written) = encoder.encode_from_utf16_without_replacement(src, dst, false);
            buf.out.extend_from_slice(&dst[..written]);
            src = &src[read..];
            match res {
                encoding_rs::EncoderResult::InputEmpty => break,
                encoding_rs::EncoderResult::OutputFull => {}
                encoding_rs::EncoderResult::Unmappable(c) => illegal_output(c as u32, enc, buf),
            }
        }
        units.clear();
    };
    let rejects_c1 = std::ptr::eq(rs, encoding_rs::SHIFT_JIS) || std::ptr::eq(rs, encoding_rs::GBK);
    for &w in input {
        if w >= UTF32_MAX || (0xD800..=0xDFFF).contains(&w) || (rejects_c1 && (w == 0x80 || (0xF8F0..=0xF8F5).contains(&w) || (w == 0x20AC && std::ptr::eq(rs, encoding_rs::GBK)))) {
            flush(&mut encoder, &mut units, buf, &mut dst);
            illegal_output(w, enc, buf);
        } else if w >= 0x10000 {
            let v = w - 0x10000;
            units.push((0xD800 | (v >> 10)) as u16);
            units.push((0xDC00 | (v & 0x3FF)) as u16);
        } else {
            units.push(w as u16);
        }
    }
    flush(&mut encoder, &mut units, buf, &mut dst);
}

/// Decode one double-byte character through `rs`, if it is exactly one
/// valid character.
fn decode_one(rs: &'static encoding_rs::Encoding, bytes: &[u8]) -> Option<u32> {
    let mut out = Vec::with_capacity(2);
    decode_rs(rs, bytes, &mut out);
    match out.as_slice() {
        [w] if *w != BAD_INPUT => Some(*w),
        _ => None,
    }
}

/// Encode one code point through `rs` (`None` when unmappable).
fn encode_one(rs: &'static encoding_rs::Encoding, w: u32) -> Option<Vec<u8>> {
    let c = char::from_u32(w)?;
    let mut s = [0u8; 4];
    let (cow, _, had_errors) = rs.encode(c.encode_utf8(&mut s));
    if had_errors {
        None
    } else {
        Some(cow.into_owned())
    }
}

// ---- EUC-CN (GB 2312) ---------------------------------------------------------------

/// Whether a GBK double-byte code lies in the GB 2312 area.
fn is_gb2312(a: u8, b: u8) -> bool {
    (0xA1..=0xF7).contains(&a) && (0xA1..=0xFE).contains(&b)
}

/// `mb_euccn_to_wchar`: GB 2312 through the GBK tables; GBK-only codes are
/// illegal.
pub fn decode_euccn(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c < 0x80 {
            out.push(c as u32);
        } else if (0xA1..=0xF7).contains(&c) && p < e {
            let c2 = input[p];
            p += 1;
            if is_gb2312(c, c2) {
                out.push(decode_one(encoding_rs::GBK, &[c, c2]).unwrap_or(BAD_INPUT));
            } else {
                out.push(BAD_INPUT);
            }
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_wchar_to_euccn`.
pub fn encode_euccn(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w < 0x80 {
            buf.out.push(w as u8);
            continue;
        }
        match encode_one(encoding_rs::GBK, w).as_deref() {
            Some([a, b]) if is_gb2312(*a, *b) => {
                buf.out.push(*a);
                buf.out.push(*b);
            }
            _ => illegal_output(w, enc, buf),
        }
    }
}

// ---- ISO-2022-KR ------------------------------------------------------------------

const KR_ASCII: u32 = 0;
const KSC5601: u32 = 1;
const EMITTED_ESC: u32 = 0x10;

/// `mb_iso2022kr_to_wchar`.
pub fn decode_iso2022kr(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    let mut state = KR_ASCII;
    while p < e {
        let c = input[p];
        p += 1;
        if c == 0x1B {
            if e - p < 3 {
                out.push(BAD_INPUT);
                if p < e {
                    let n = input[p];
                    p += 1;
                    if n == b'$' && p < e {
                        p += 1;
                    }
                }
                continue;
            }
            let c2 = input[p];
            let c3 = input[p + 1];
            let c4 = input[p + 2];
            p += 3;
            if c2 == b'$' && c3 == b')' && c4 == b'C' {
                state = KR_ASCII;
            } else {
                if c3 != b')' {
                    p -= 1;
                    if c2 != b'$' {
                        p -= 1;
                    }
                }
                out.push(BAD_INPUT);
            }
        } else if c == 0xF {
            state = KR_ASCII;
        } else if c == 0xE {
            state = KSC5601;
        } else if (0x21..=0x7E).contains(&c) && state == KSC5601 {
            if p == e {
                out.push(BAD_INPUT);
                break;
            }
            let c2 = input[p];
            p += 1;
            if !(0x21..=0x7E).contains(&c2) {
                out.push(BAD_INPUT);
                continue;
            }
            out.push(decode_one(encoding_rs::EUC_KR, &[c | 0x80, c2 | 0x80]).unwrap_or(BAD_INPUT));
        } else if c < 0x80 && state == KR_ASCII {
            out.push(c as u32);
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_wchar_to_iso2022kr`.
pub fn encode_iso2022kr(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, end: bool) {
    if !input.is_empty() && buf.state & EMITTED_ESC == 0 {
        buf.out.extend_from_slice(b"\x1b$)C");
        buf.state |= EMITTED_ESC;
    }
    for &w in input {
        let s = if w < 0x80 {
            Some(w)
        } else {
            match encode_one(encoding_rs::EUC_KR, w).as_deref() {
                Some([a, b]) if *a >= 0xA1 && *b >= 0xA1 => Some((((*a & 0x7F) as u32) << 8) | (*b & 0x7F) as u32),
                _ => None,
            }
        };
        match s {
            None => illegal_output(w, enc, buf),
            Some(s) if s < 0x80 => {
                if buf.state & 1 != KR_ASCII {
                    buf.out.push(0xF);
                    buf.state &= !KSC5601;
                }
                buf.out.push(s as u8);
            }
            Some(s) => {
                if buf.state & 1 != KSC5601 {
                    buf.out.push(0xE);
                    buf.state |= KSC5601;
                }
                buf.out.push((s >> 8) as u8);
                buf.out.push((s & 0xFF) as u8);
            }
        }
    }
    if end && buf.state & 1 != KR_ASCII {
        buf.out.push(0xF);
        buf.state &= !KSC5601;
    }
}

// ---- HZ ------------------------------------------------------------------------------

const HZ_ASCII: u32 = 0;
const GB2312: u32 = 1;

/// `mb_hz_to_wchar`.
pub fn decode_hz(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    let mut state = HZ_ASCII;
    while p < e {
        let c = input[p];
        p += 1;
        if c == b'~' {
            if p == e {
                break;
            }
            let c2 = input[p];
            p += 1;
            if c2 == b'}' && state == GB2312 {
                state = HZ_ASCII;
            } else if c2 == b'{' && state == HZ_ASCII {
                state = GB2312;
            } else if c2 == b'~' && state == HZ_ASCII {
                out.push(b'~' as u32);
            } else if c2 == b'\n' {
            } else {
                out.push(BAD_INPUT);
            }
        } else if ((c > 0x20 && c <= 0x29) || (0x30..=0x77).contains(&c)) && p < e && state == GB2312 {
            let c2 = input[p];
            p += 1;
            if c > 0x20 && c < 0x7F && c2 > 0x20 && c2 < 0x7F && is_gb2312(c | 0x80, c2 | 0x80) {
                out.push(decode_one(encoding_rs::GBK, &[c | 0x80, c2 | 0x80]).unwrap_or(BAD_INPUT));
            } else {
                out.push(BAD_INPUT);
            }
        } else if c < 0x80 && state == HZ_ASCII {
            out.push(c as u32);
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_wchar_to_hz`.
pub fn encode_hz(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let s = if w < 0x80 {
            Some(w)
        } else {
            match encode_one(encoding_rs::GBK, w).as_deref() {
                Some([a, b]) if is_gb2312(*a, *b) => Some((((*a & 0x7F) as u32) << 8) | (*b & 0x7F) as u32),
                _ => None,
            }
        };
        match s {
            None => illegal_output(w, enc, buf),
            Some(s) if s < 0x80 => {
                if buf.state != HZ_ASCII {
                    buf.out.extend_from_slice(b"~}");
                    buf.state = HZ_ASCII;
                }
                if s == b'~' as u32 {
                    buf.out.extend_from_slice(b"~~");
                } else {
                    buf.out.push(s as u8);
                }
            }
            Some(s) => {
                if buf.state != GB2312 {
                    buf.out.extend_from_slice(b"~{");
                    buf.state = GB2312;
                }
                buf.out.push(((s >> 8) & 0x7F) as u8);
                buf.out.push((s & 0x7F) as u8);
            }
        }
    }
    if end && buf.state != HZ_ASCII {
        buf.out.extend_from_slice(b"~}");
        buf.state = HZ_ASCII;
    }
}

/// `step_through_gb18030_str`.
fn step_gb18030(s: &[u8], mut p: usize, limit: usize) -> usize {
    while p < limit {
        let c = s[p];
        if c < 0x81 || c == 0xFF {
            p += 1;
        } else {
            if limit - p == 1 {
                break;
            }
            let c2 = s[p + 1];
            let w = if (0x30..=0x39).contains(&c2) { 4 } else { 2 };
            if limit - p < w {
                break;
            }
            p += w;
        }
    }
    p
}

/// `mb_cut_gb18030`.
pub fn cut_gb18030(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    let from = from.min(s.len());
    let start = step_gb18030(s, 0, from);
    let len = len.min(s.len() - from);
    if start + len >= s.len() {
        s[start..].to_vec()
    } else {
        let end = step_gb18030(s, start, start + len);
        s[start..end].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{convert, name2encoding, ErrorMode};

    fn conv(s: &[u8], from: &str, to: &str) -> Vec<u8> {
        let f = name2encoding(from.as_bytes()).unwrap();
        let t = name2encoding(to.as_bytes()).unwrap();
        convert(s, f, t, b'?' as u32, ErrorMode::Char).0
    }

    #[test]
    fn sjis_and_eucjp() {
        assert_eq!(conv("日本語".as_bytes(), "UTF-8", "SJIS"), b"\x93\xfa\x96\x7b\x8c\xea");
        assert_eq!(conv(b"\x93\xfa\x96\x7b\x8c\xea", "SJIS", "UTF-8"), "日本語".as_bytes());
        assert_eq!(conv("日本語".as_bytes(), "UTF-8", "EUC-JP"), b"\xc6\xfc\xcb\xdc\xb8\xec");
        assert_eq!(conv(b"\x93\xfa\xff", "SJIS", "UTF-8"), "日?".as_bytes());
    }

    #[test]
    fn iso2022kr_and_hz() {
        let kr = conv("한a".as_bytes(), "UTF-8", "ISO-2022-KR");
        assert_eq!(kr, b"\x1b$)C\x0eGQ\x0fa");
        assert_eq!(conv(&kr, "ISO-2022-KR", "UTF-8"), "한a".as_bytes());
        let hz = conv("中~a".as_bytes(), "UTF-8", "HZ");
        assert_eq!(hz, b"~{VP~}~~a");
        assert_eq!(conv(&hz, "HZ", "UTF-8"), "中~a".as_bytes());
    }
}
