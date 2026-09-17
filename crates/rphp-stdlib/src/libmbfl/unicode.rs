//! UTF-8, UTF-16, UTF-32, UCS-2 and UCS-4 (`mbfilter_utf8.c`,
//! `mbfilter_utf16.c`, `mbfilter_utf32.c`, `mbfilter_ucs2.c`,
//! `mbfilter_ucs4.c`): php's validation rules, its BOM handling for the
//! endian-less names (a BOM selects the byte order and is dropped; without
//! one the text is big-endian) and the `mb_strcut` helpers.

use super::{illegal_output, ConvertBuf, Encoding, BAD_INPUT, UCS2_MAX, UTF32_MAX};

/// `mblen_table_utf8`: the byte length of a UTF-8 sequence by lead byte
/// (continuation bytes and illegal leads count as 1).
pub static MBLEN_UTF8: [u8; 256] = {
    let mut t = [1u8; 256];
    let mut i = 0xC2;
    while i <= 0xDF {
        t[i] = 2;
        i += 1;
    }
    while i <= 0xEF {
        t[i] = 3;
        i += 1;
    }
    while i <= 0xF4 {
        t[i] = 4;
        i += 1;
    }
    t
};

/// `mb_utf8_to_wchar`.
pub fn decode_utf8(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c < 0x80 {
            out.push(c as u32);
        } else if c < 0xC2 {
            out.push(BAD_INPUT);
        } else if c <= 0xDF {
            if p < e {
                let c2 = input[p];
                p += 1;
                if c2 & 0xC0 != 0x80 {
                    out.push(BAD_INPUT);
                    p -= 1;
                } else {
                    out.push((((c & 0x1F) as u32) << 6) | (c2 & 0x3F) as u32);
                }
            } else {
                out.push(BAD_INPUT);
            }
        } else if c <= 0xEF {
            if e - p >= 2 {
                let c2 = input[p];
                let c3 = input[p + 1];
                p += 2;
                if c2 & 0xC0 != 0x80 || (c == 0xE0 && c2 < 0xA0) || (c == 0xED && c2 >= 0xA0) {
                    out.push(BAD_INPUT);
                    p -= 2;
                } else if c3 & 0xC0 != 0x80 {
                    out.push(BAD_INPUT);
                    p -= 1;
                } else {
                    out.push((((c & 0xF) as u32) << 12) | (((c2 & 0x3F) as u32) << 6) | (c3 & 0x3F) as u32);
                }
            } else {
                out.push(BAD_INPUT);
                if p < e && (c != 0xE0 || input[p] >= 0xA0) && (c != 0xED || input[p] < 0xA0) && input[p] & 0xC0 == 0x80 {
                    p += 1;
                    if p < e && input[p] & 0xC0 == 0x80 {
                        p += 1;
                    }
                }
            }
        } else if c <= 0xF4 {
            if e - p >= 3 {
                let c2 = input[p];
                let c3 = input[p + 1];
                let c4 = input[p + 2];
                p += 3;
                if c2 & 0xC0 != 0x80 || (c == 0xF0 && c2 < 0x90) || (c == 0xF4 && c2 >= 0x90) {
                    out.push(BAD_INPUT);
                    p -= 3;
                } else if c3 & 0xC0 != 0x80 {
                    out.push(BAD_INPUT);
                    p -= 2;
                } else if c4 & 0xC0 != 0x80 {
                    out.push(BAD_INPUT);
                    p -= 1;
                } else {
                    out.push(
                        (((c & 0x7) as u32) << 18)
                            | (((c2 & 0x3F) as u32) << 12)
                            | (((c3 & 0x3F) as u32) << 6)
                            | (c4 & 0x3F) as u32,
                    );
                }
            } else {
                out.push(BAD_INPUT);
                if p < e {
                    let c2 = input[p];
                    if (c == 0xF0 && c2 >= 0x90) || (c == 0xF4 && c2 < 0x90) || (0xF1..=0xF3).contains(&c) {
                        while p < e && input[p] & 0xC0 == 0x80 {
                            p += 1;
                        }
                    }
                }
            }
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_fast_check_utf8` (the result, not the SIMD): php's validity rules.
pub fn check_utf8(input: &[u8]) -> bool {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        if c < 0x80 {
            p += 1;
        } else if (0xC2..=0xDF).contains(&c) {
            if p + 1 >= e || input[p + 1] & 0xC0 != 0x80 {
                return false;
            }
            p += 2;
        } else if (0xE0..=0xEF).contains(&c) {
            if p + 2 >= e {
                return false;
            }
            let c2 = input[p + 1];
            if c2 & 0xC0 != 0x80 || (c == 0xE0 && c2 < 0xA0) || (c == 0xED && c2 >= 0xA0) || input[p + 2] & 0xC0 != 0x80 {
                return false;
            }
            p += 3;
        } else if (0xF0..=0xF4).contains(&c) {
            if p + 3 >= e {
                return false;
            }
            let c2 = input[p + 1];
            if c2 & 0xC0 != 0x80
                || (c == 0xF0 && c2 < 0x90)
                || (c == 0xF4 && c2 >= 0x90)
                || input[p + 2] & 0xC0 != 0x80
                || input[p + 3] & 0xC0 != 0x80
            {
                return false;
            }
            p += 4;
        } else {
            return false;
        }
    }
    true
}

/// `mb_wchar_to_utf8` (surrogate code points are encoded as-is, as php does).
pub fn encode_utf8(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w < 0x80 {
            buf.out.push(w as u8);
        } else if w < 0x800 {
            buf.out.push((((w >> 6) & 0x1F) | 0xC0) as u8);
            buf.out.push(((w & 0x3F) | 0x80) as u8);
        } else if w < 0x10000 {
            buf.out.push((((w >> 12) & 0xF) | 0xE0) as u8);
            buf.out.push((((w >> 6) & 0x3F) | 0x80) as u8);
            buf.out.push(((w & 0x3F) | 0x80) as u8);
        } else if w < UTF32_MAX {
            buf.out.push((((w >> 18) & 0x7) | 0xF0) as u8);
            buf.out.push((((w >> 12) & 0x3F) | 0x80) as u8);
            buf.out.push((((w >> 6) & 0x3F) | 0x80) as u8);
            buf.out.push(((w & 0x3F) | 0x80) as u8);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// Append a code point as UTF-8 (valid input only; used by `mb_chr`).
pub fn push_utf8(w: u32, out: &mut Vec<u8>) {
    let mut b = ConvertBuf::default_subst();
    encode_utf8(&super::UTF8, &[w], &mut b);
    out.extend_from_slice(&b.out);
}

/// `mb_cut_utf8`: byte-cut that backs up over continuation bytes.
pub fn cut_utf8(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    let mut start = from.min(s.len());
    while start > 0 && start < s.len() && (s[start] as i8) < -64 {
        start -= 1;
    }
    let end = start.saturating_add(len);
    if end >= s.len() {
        return s[start..].to_vec();
    }
    let mut end = end;
    while end > start && (s[end] as i8) < -64 {
        end -= 1;
    }
    s[start..end].to_vec()
}

fn u16_at(s: &[u8], i: usize, le: bool) -> u16 {
    if le {
        ((s[i + 1] as u16) << 8) | s[i] as u16
    } else {
        ((s[i] as u16) << 8) | s[i + 1] as u16
    }
}

fn decode_utf16_impl(input: &[u8], out: &mut Vec<u32>, le: bool) {
    let e = input.len() & !1;
    let mut p = 0;
    while p < e {
        let n = u16_at(input, p, le);
        p += 2;
        if (0xD800..=0xDBFF).contains(&n) {
            if p < e {
                let n2 = u16_at(input, p, le);
                p += 2;
                if (0xD800..=0xDBFF).contains(&n2) {
                    out.push(BAD_INPUT);
                    p -= 2;
                } else if (0xDC00..=0xDFFF).contains(&n2) {
                    out.push((((n as u32 & 0x3FF) << 10) | (n2 as u32 & 0x3FF)) + 0x10000);
                } else {
                    out.push(BAD_INPUT);
                    out.push(n2 as u32);
                }
            } else {
                out.push(BAD_INPUT);
            }
        } else if (0xDC00..=0xDFFF).contains(&n) {
            out.push(BAD_INPUT);
        } else {
            out.push(n as u32);
        }
    }
    if input.len() & 1 == 1 {
        out.push(BAD_INPUT);
    }
}

/// `mb_utf16be_to_wchar`.
pub fn decode_utf16be(input: &[u8], out: &mut Vec<u32>) {
    decode_utf16_impl(input, out, false)
}

/// `mb_utf16le_to_wchar`.
pub fn decode_utf16le(input: &[u8], out: &mut Vec<u32>) {
    decode_utf16_impl(input, out, true)
}

/// `mb_utf16_to_wchar`: a leading BOM picks the byte order (and is dropped).
pub fn decode_utf16(input: &[u8], out: &mut Vec<u32>) {
    if input.len() >= 2 {
        let n = ((input[0] as u16) << 8) | input[1] as u16;
        if n == 0xFFFE {
            return decode_utf16le(&input[2..], out);
        }
        if n == 0xFEFF {
            return decode_utf16be(&input[2..], out);
        }
    }
    decode_utf16be(input, out)
}

fn encode_utf16_impl(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, le: bool) {
    let push = |buf: &mut ConvertBuf, n: u16| {
        if le {
            buf.out.push((n & 0xFF) as u8);
            buf.out.push((n >> 8) as u8);
        } else {
            buf.out.push((n >> 8) as u8);
            buf.out.push((n & 0xFF) as u8);
        }
    };
    for &w in input {
        if w < UCS2_MAX {
            push(buf, w as u16);
        } else if w < UTF32_MAX {
            let n1 = (((w >> 10) - 0x40) | 0xD800) as u16;
            let n2 = ((w & 0x3FF) | 0xDC00) as u16;
            push(buf, n1);
            push(buf, n2);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_wchar_to_utf16be`.
pub fn encode_utf16be(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_utf16_impl(enc, input, buf, false)
}

/// `mb_wchar_to_utf16le`.
pub fn encode_utf16le(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_utf16_impl(enc, input, buf, true)
}

fn cut_utf16_impl(s: &[u8], from: usize, len: usize, le: bool) -> Vec<u8> {
    let from = from.min(s.len());
    let mut len = len.min(s.len() - from);
    let from = from & !1;
    len &= !1;
    let start = from;
    if len < 2 || s.len() - start < 2 {
        return Vec::new();
    }
    // php checks whether the first unit is the second half of a surrogate
    // pair here, but the adjustment it computes is never applied (the start
    // pointer is not moved); the same result is produced by not checking.
    let mut end = (start + len).min(s.len());
    let ending = u16_at(s, end - 2, le);
    if (0xD800..=0xDBFF).contains(&ending) {
        end -= 2;
    }
    s[start..end].to_vec()
}

/// `mb_cut_utf16be`.
pub fn cut_utf16be(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    cut_utf16_impl(s, from, len, false)
}

/// `mb_cut_utf16le`.
pub fn cut_utf16le(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    cut_utf16_impl(s, from, len, true)
}

/// `mb_cut_utf16`: honours a leading BOM.
pub fn cut_utf16(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    if len < 2 || s.len() < 2 {
        return Vec::new();
    }
    let cp = ((s[0] as u16) << 8) | s[1] as u16;
    if cp == 0xFFFE {
        cut_utf16le(s, from.max(2), len)
    } else {
        let from = if cp == 0xFEFF { from.max(2) } else { from };
        cut_utf16be(s, from, len)
    }
}

fn u32_at(s: &[u8], i: usize, le: bool) -> u32 {
    if le {
        ((s[i + 3] as u32) << 24) | ((s[i + 2] as u32) << 16) | ((s[i + 1] as u32) << 8) | s[i] as u32
    } else {
        ((s[i] as u32) << 24) | ((s[i + 1] as u32) << 16) | ((s[i + 2] as u32) << 8) | s[i + 3] as u32
    }
}

fn decode_utf32_impl(input: &[u8], out: &mut Vec<u32>, le: bool) {
    let e = input.len() & !3;
    let mut p = 0;
    while p < e {
        let w = u32_at(input, p, le);
        p += 4;
        if w < UTF32_MAX && !(0xD800..=0xDFFF).contains(&w) {
            out.push(w);
        } else {
            out.push(BAD_INPUT);
        }
    }
    if input.len() & 3 != 0 {
        out.push(BAD_INPUT);
    }
}

/// `mb_utf32be_to_wchar`.
pub fn decode_utf32be(input: &[u8], out: &mut Vec<u32>) {
    decode_utf32_impl(input, out, false)
}

/// `mb_utf32le_to_wchar`.
pub fn decode_utf32le(input: &[u8], out: &mut Vec<u32>) {
    decode_utf32_impl(input, out, true)
}

/// `mb_utf32_to_wchar`: BOM detection.
pub fn decode_utf32(input: &[u8], out: &mut Vec<u32>) {
    if input.len() >= 4 {
        let w = u32_at(input, 0, false);
        if w == 0xFFFE_0000 {
            return decode_utf32le(&input[4..], out);
        }
        if w == 0xFEFF {
            return decode_utf32be(&input[4..], out);
        }
    }
    decode_utf32be(input, out)
}

fn encode_utf32_impl(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, le: bool) {
    for &w in input {
        if w < UTF32_MAX {
            if le {
                buf.out.extend_from_slice(&w.to_le_bytes());
            } else {
                buf.out.extend_from_slice(&w.to_be_bytes());
            }
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_wchar_to_utf32be`.
pub fn encode_utf32be(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_utf32_impl(enc, input, buf, false)
}

/// `mb_wchar_to_utf32le`.
pub fn encode_utf32le(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_utf32_impl(enc, input, buf, true)
}

fn decode_ucs2_impl(input: &[u8], out: &mut Vec<u32>, le: bool) {
    let e = input.len() & !1;
    let mut p = 0;
    while p < e {
        out.push(u16_at(input, p, le) as u32);
        p += 2;
    }
    if input.len() & 1 == 1 {
        out.push(BAD_INPUT);
    }
}

/// `mb_ucs2be_to_wchar`.
pub fn decode_ucs2be(input: &[u8], out: &mut Vec<u32>) {
    decode_ucs2_impl(input, out, false)
}

/// `mb_ucs2le_to_wchar`.
pub fn decode_ucs2le(input: &[u8], out: &mut Vec<u32>) {
    decode_ucs2_impl(input, out, true)
}

/// `mb_ucs2_to_wchar`: BOM detection.
pub fn decode_ucs2(input: &[u8], out: &mut Vec<u32>) {
    if input.len() >= 2 {
        let w = ((input[0] as u16) << 8) | input[1] as u16;
        if w == 0xFFFE {
            return decode_ucs2le(&input[2..], out);
        }
        if w == 0xFEFF {
            return decode_ucs2be(&input[2..], out);
        }
    }
    decode_ucs2be(input, out)
}

fn encode_ucs2_impl(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, le: bool) {
    for &w in input {
        if w < UCS2_MAX {
            if le {
                buf.out.push((w & 0xFF) as u8);
                buf.out.push((w >> 8) as u8);
            } else {
                buf.out.push((w >> 8) as u8);
                buf.out.push((w & 0xFF) as u8);
            }
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_wchar_to_ucs2be`.
pub fn encode_ucs2be(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_ucs2_impl(enc, input, buf, false)
}

/// `mb_wchar_to_ucs2le`.
pub fn encode_ucs2le(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_ucs2_impl(enc, input, buf, true)
}

fn decode_ucs4_impl(input: &[u8], out: &mut Vec<u32>, le: bool) {
    let e = input.len() & !3;
    let mut p = 0;
    while p < e {
        out.push(u32_at(input, p, le));
        p += 4;
    }
    if input.len() & 3 != 0 {
        out.push(BAD_INPUT);
    }
}

/// `mb_ucs4be_to_wchar`.
pub fn decode_ucs4be(input: &[u8], out: &mut Vec<u32>) {
    decode_ucs4_impl(input, out, false)
}

/// `mb_ucs4le_to_wchar`.
pub fn decode_ucs4le(input: &[u8], out: &mut Vec<u32>) {
    decode_ucs4_impl(input, out, true)
}

/// `mb_ucs4_to_wchar`: BOM detection.
pub fn decode_ucs4(input: &[u8], out: &mut Vec<u32>) {
    if input.len() >= 4 {
        let w = u32_at(input, 0, false);
        if w == 0xFFFE_0000 {
            return decode_ucs4le(&input[4..], out);
        }
        if w == 0xFEFF {
            return decode_ucs4be(&input[4..], out);
        }
    }
    decode_ucs4be(input, out)
}

fn encode_ucs4_impl(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, le: bool) {
    for &w in input {
        if w != BAD_INPUT {
            if le {
                buf.out.extend_from_slice(&w.to_le_bytes());
            } else {
                buf.out.extend_from_slice(&w.to_be_bytes());
            }
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_wchar_to_ucs4be`.
pub fn encode_ucs4be(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_ucs4_impl(enc, input, buf, false)
}

/// `mb_wchar_to_ucs4le`.
pub fn encode_ucs4le(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    encode_ucs4_impl(enc, input, buf, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d8(s: &[u8]) -> Vec<u32> {
        let mut v = Vec::new();
        decode_utf8(s, &mut v);
        v
    }

    #[test]
    fn utf8_validation_matches_php() {
        assert_eq!(d8("aé日😀".as_bytes()), vec![0x61, 0xE9, 0x65E5, 0x1F600]);
        assert_eq!(d8(b"\xc0\x80"), vec![BAD_INPUT, BAD_INPUT]);
        assert_eq!(d8(b"\xed\xa0\x80"), vec![BAD_INPUT, BAD_INPUT, BAD_INPUT]);
        assert_eq!(d8(b"\xe2\x82"), vec![BAD_INPUT]);
        assert_eq!(d8(b"\xf0\x9f\x98"), vec![BAD_INPUT]);
        assert_eq!(d8(b"\xe2\x82a"), vec![BAD_INPUT, 0x61]);
        assert_eq!(d8(b"\xf8\x88\x80\x80\x80"), vec![BAD_INPUT; 5]);
        assert!(check_utf8("aé日😀".as_bytes()));
        assert!(!check_utf8(b"\xed\xa0\x80"));
        assert!(!check_utf8(b"\xe2\x82"));
    }

    #[test]
    fn utf16_surrogates_and_bom() {
        let mut v = Vec::new();
        decode_utf16(b"\xff\xfe\x61\x00\x3d\xd8\x00\xde", &mut v);
        assert_eq!(v, vec![0x61, 0x1F600]);
        v.clear();
        decode_utf16be(b"\xd8\x3d\x00\x61\x00", &mut v);
        assert_eq!(v, vec![BAD_INPUT, 0x61, BAD_INPUT]);
        let mut buf = ConvertBuf::default_subst();
        encode_utf16le(super::super::name2encoding(b"UTF-16LE").unwrap(), &[0x1F600], &mut buf);
        assert_eq!(buf.out, b"\x3d\xd8\x00\xde");
    }

    #[test]
    fn cut_utf8_backs_up() {
        // php backs a mid-character offset *up* to the character start, so
        // cutting "aé日" at byte 2 (inside "é") yields "é日", not "日".
        assert_eq!(cut_utf8("aé日".as_bytes(), 2, 10), "é日".as_bytes());
        assert_eq!(cut_utf8("aé日".as_bytes(), 0, 2), b"a");
    }
}
