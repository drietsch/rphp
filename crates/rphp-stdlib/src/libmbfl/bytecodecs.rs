//! The "encodings" that are really byte transforms: BASE64
//! (`mbfilter_base64.c`), Quoted-Printable (`mbfilter_qprint.c`), UUENCODE
//! (`mbfilter_uuencode.c`), HTML-ENTITIES (`mbfilter_htmlent.c`), 7bit and
//! 8bit. Their wchars are bytes 0x00–0xFF (HTML-ENTITIES: code points).

use super::tables::html_entities::ENTITIES;
use super::{illegal_output, ConvertBuf, Encoding, BAD_INPUT};

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `mb_wchar_to_8bit`.
pub fn encode_8bit(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w <= 0xFF {
            buf.out.push(w as u8);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_wchar_to_7bit` / `mb_wchar_to_ascii`.
pub fn encode_7bit(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w <= 0x7F {
            buf.out.push(w as u8);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

fn decode_b64(c: u8) -> i8 {
    match c {
        b'A'..=b'Z' => (c - b'A') as i8,
        b'a'..=b'z' => (c - b'a' + 26) as i8,
        b'0'..=b'9' => (c - b'0' + 52) as i8,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    }
}

/// `mb_base64_to_wchar`: whitespace and `=` are skipped, other non-alphabet
/// bytes are illegal.
pub fn decode_base64(input: &[u8], out: &mut Vec<u32>) {
    let mut bits = 0u32;
    let mut cache = 0u32;
    for &c in input {
        if matches!(c, b'\r' | b'\n' | b' ' | b'\t' | b'=') {
            continue;
        }
        let v = decode_b64(c);
        if v < 0 {
            out.push(BAD_INPUT);
        } else {
            bits += 6;
            cache = (cache << 6) | (v as u32 & 0x3F);
            if bits == 24 {
                out.push((cache >> 16) & 0xFF);
                out.push((cache >> 8) & 0xFF);
                out.push(cache & 0xFF);
                bits = 0;
                cache = 0;
            }
        }
    }
    if bits == 18 {
        out.push((cache >> 10) & 0xFF);
        out.push((cache >> 2) & 0xFF);
    } else if bits == 12 {
        out.push((cache >> 4) & 0xFF);
    }
}

/// `mb_wchar_to_base64`: 76-column lines with CRLF.
pub fn encode_base64(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let mut bits = (buf.state & 0x3) * 8;
    let mut chars_output = ((buf.state >> 2) & 0x3F) * 4;
    let mut cache = buf.state >> 8;
    for &w in input {
        cache = (cache << 8) | (w & 0xFF);
        bits += 8;
        if bits == 24 {
            if chars_output > 72 {
                buf.out.extend_from_slice(b"\r\n");
                chars_output = 0;
            }
            buf.out.push(BASE64[((cache >> 18) & 0x3F) as usize]);
            buf.out.push(BASE64[((cache >> 12) & 0x3F) as usize]);
            buf.out.push(BASE64[((cache >> 6) & 0x3F) as usize]);
            buf.out.push(BASE64[(cache & 0x3F) as usize]);
            chars_output += 4;
            bits = 0;
            cache = 0;
        }
    }
    if end && bits != 0 {
        if chars_output > 72 {
            buf.out.extend_from_slice(b"\r\n");
        }
        if bits == 8 {
            buf.out.push(BASE64[((cache >> 2) & 0x3F) as usize]);
            buf.out.push(BASE64[((cache & 0x3) << 4) as usize]);
            buf.out.extend_from_slice(b"==");
        } else {
            buf.out.push(BASE64[((cache >> 10) & 0x3F) as usize]);
            buf.out.push(BASE64[((cache >> 4) & 0x3F) as usize]);
            buf.out.push(BASE64[((cache & 0xF) << 2) as usize]);
            buf.out.push(b'=');
        }
        buf.state = 0;
    } else {
        buf.state = (cache << 8) | (((chars_output / 4) & 0x3F) << 2) | ((bits / 8) & 0x3);
    }
}

fn hex_val(c: u8) -> i32 {
    match c {
        b'0'..=b'9' => (c - b'0') as i32,
        b'A'..=b'F' => (c - b'A' + 10) as i32,
        b'a'..=b'f' => (c - b'a' + 10) as i32,
        _ => -1,
    }
}

/// `mb_qprint_to_wchar`.
pub fn decode_qprint(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c == b'=' && p < e {
            let c2 = input[p];
            p += 1;
            if hex_val(c2) >= 0 && p < e {
                let c3 = input[p];
                p += 1;
                if hex_val(c3) >= 0 {
                    out.push(((hex_val(c2) << 4) | hex_val(c3)) as u32);
                } else {
                    out.push(b'=' as u32);
                    out.push(c2 as u32);
                    out.push(c3 as u32);
                }
            } else if c2 == b'\r' && p < e {
                let c3 = input[p];
                p += 1;
                if c3 != b'\n' {
                    out.push(c3 as u32);
                }
            } else if c2 != b'\n' {
                out.push(b'=' as u32);
                out.push(c2 as u32);
            }
        } else {
            out.push(c as u32);
        }
    }
}

/// `mb_wchar_to_qprint`.
pub fn encode_qprint(input: &[u32], buf: &mut ConvertBuf) {
    let mut chars_output = buf.state;
    for &w in input {
        if w == 0 {
            buf.out.push(0);
            chars_output = 0;
            continue;
        } else if w == b'\n' as u32 {
            buf.out.extend_from_slice(b"\r\n");
            chars_output = 0;
            continue;
        } else if w == b'\r' as u32 {
            continue;
        }
        if chars_output >= 72 {
            buf.out.extend_from_slice(b"=\r\n");
            chars_output = 0;
        }
        if w >= 0x80 || w == b'=' as u32 {
            buf.out.push(b'=');
            buf.out.push(b"0123456789ABCDEF"[((w >> 4) & 0xF) as usize]);
            buf.out.push(b"0123456789ABCDEF"[(w & 0xF) as usize]);
            chars_output += 3;
        } else {
            buf.out.push(w as u8);
            chars_output += 1;
        }
    }
    buf.state = chars_output;
}

fn uudec(c: u8) -> u32 {
    (c.wrapping_sub(b' ') & 0o77) as u32
}

/// `mb_uuencode_to_wchar`.
pub fn decode_uuencode(input: &[u8], out: &mut Vec<u32>) {
    const GROUND: u32 = 0;
    const SIZE: u32 = 3;
    const A: u32 = 4;
    const SKIP_NEWLINE: u32 = 8;
    let e = input.len();
    let mut p = 0;
    let mut state = GROUND;
    let mut size = 0u32;
    while p < e {
        let c = input[p];
        p += 1;
        match state {
            GROUND => {
                if c == b'b' && e - p >= 5 && &input[p..p + 5] == b"egin " {
                    p += 5;
                    while p < e {
                        let n = input[p];
                        p += 1;
                        if n == b'\n' {
                            break;
                        }
                    }
                    state = SIZE;
                }
            }
            SIZE => {
                size = uudec(c);
                state = A;
            }
            A => {
                if e - p < 4 {
                    break;
                }
                let a = uudec(c);
                let b = uudec(input[p]);
                let cc = uudec(input[p + 1]);
                let d = uudec(input[p + 2]);
                p += 3;
                if size > 0 {
                    out.push(((a << 2) | (b >> 4)) & 0xFF);
                    size -= 1;
                }
                if size > 0 {
                    out.push(((b << 4) | (cc >> 2)) & 0xFF);
                    size -= 1;
                }
                if size > 0 {
                    out.push(((cc << 6) | d) & 0xFF);
                    size -= 1;
                }
                state = if size != 0 { A } else { SKIP_NEWLINE };
            }
            SKIP_NEWLINE => state = SIZE,
            _ => {}
        }
    }
}

fn uuenc(bits: u32) -> u8 {
    if bits == 0 {
        b'`'
    } else {
        (bits + 32) as u8
    }
}

/// `mb_wchar_to_uuencode`: a `begin 0644 filename` header and 45-byte lines.
pub fn encode_uuencode(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let mut bytes_encoded = (buf.state >> 1) & 0x7F;
    let mut n_cached_bits = (buf.state >> 8) & 0xFF;
    let mut cached_bits = buf.state >> 16;
    let mut len = input.len();
    let mut i = 0;

    if buf.state == 0 {
        buf.out.extend_from_slice(b"begin 0644 filename\n");
        buf.out.push((len.min(45) + 32) as u8);
        buf.state |= 1;
    } else if len == 0 && end && bytes_encoded == 0 && n_cached_bits == 0 {
        buf.out.pop();
        return;
    } else {
        let mut len_byte = buf.out.len() - (bytes_encoded as usize * 4 / 3) - 1;
        if n_cached_bits != 0 {
            len_byte -= if n_cached_bits == 2 { 1 } else { 2 };
        }
        let extra = if n_cached_bits != 0 { if n_cached_bits == 2 { 1 } else { 2 } } else { 0 };
        buf.out[len_byte] = ((bytes_encoded as usize + len + extra).min(45) + 32) as u8;

        if n_cached_bits != 0 {
            if n_cached_bits == 2 {
                let w = cached_bits;
                let mut w2 = 0;
                let mut w3 = 0;
                if len != 0 {
                    w2 = input[i];
                    i += 1;
                    len -= 1;
                }
                if len != 0 {
                    w3 = input[i];
                    i += 1;
                    len -= 1;
                }
                buf.out.push(uuenc((w << 4) + ((w2 >> 4) & 0xF)));
                buf.out.push(uuenc(((w2 & 0xF) << 2) + ((w3 >> 6) & 0x3)));
                buf.out.push(uuenc(w3 & 0x3F));
            } else {
                let w2 = cached_bits;
                let mut w3 = 0;
                if len != 0 {
                    w3 = input[i];
                    i += 1;
                    len -= 1;
                }
                buf.out.push(uuenc((w2 << 2) + ((w3 >> 6) & 0x3)));
                buf.out.push(uuenc(w3 & 0x3F));
            }
            n_cached_bits = 0;
            cached_bits = 0;
            bytes_encoded += 3;
            if bytes_encoded >= 45 {
                buf.out.push(b'\n');
                if len != 0 || !end {
                    buf.out.push((len.min(45) + 32) as u8);
                }
                bytes_encoded = 0;
            }
        }
    }

    while len > 0 {
        len -= 1;
        let w = input[i];
        i += 1;
        let mut w2 = 0;
        let mut w3 = 0;
        if len == 0 {
            if !end {
                buf.out.push(uuenc((w >> 2) & 0x3F));
                cached_bits = w & 0x3;
                n_cached_bits = 2;
                break;
            }
        } else {
            w2 = input[i];
            i += 1;
            len -= 1;
        }
        if len == 0 {
            if !end {
                buf.out.push(uuenc((w >> 2) & 0x3F));
                buf.out.push(uuenc(((w & 0x3) << 4) + ((w2 >> 4) & 0xF)));
                cached_bits = w2 & 0xF;
                n_cached_bits = 4;
                break;
            }
        } else {
            w3 = input[i];
            i += 1;
            len -= 1;
        }
        buf.out.push(uuenc((w >> 2) & 0x3F));
        buf.out.push(uuenc(((w & 0x3) << 4) + ((w2 >> 4) & 0xF)));
        buf.out.push(uuenc(((w2 & 0xF) << 2) + ((w3 >> 6) & 0x3)));
        buf.out.push(uuenc(w3 & 0x3F));
        bytes_encoded += 3;
        if bytes_encoded >= 45 {
            buf.out.push(b'\n');
            if len != 0 || !end {
                buf.out.push((len.min(45) + 32) as u8);
            }
            bytes_encoded = 0;
        }
    }

    if bytes_encoded != 0 && end {
        buf.out.push(b'\n');
    }
    buf.state = ((cached_bits & 0xFF) << 16) | ((n_cached_bits & 0xFF) << 8) | ((bytes_encoded & 0x7F) << 1) | (buf.state & 1);
}

fn is_entity_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'#'
}

/// `mb_htmlent_to_wchar`: named entities from php's list, `&#NNN;` and
/// `&#xHHH;`; anything else is passed through literally.
pub fn decode_htmlent(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c != b'&' {
            out.push(c as u32);
            continue;
        }
        let mut terminator = p;
        while terminator < e && is_entity_char(input[terminator]) {
            terminator += 1;
        }
        if terminator < e && input[terminator] == b';' {
            if p < e && input[p] == b'#' && e - p >= 2 {
                let mut digits = p + 1;
                let mut value: u32 = 0;
                let mut bad = false;
                if digits < e && (input[digits] == b'x' || input[digits] == b'X') {
                    digits += 1;
                    if digits == terminator {
                        bad = true;
                    }
                    while !bad && digits < terminator {
                        let d = input[digits];
                        digits += 1;
                        match hex_val(d) {
                            v if v >= 0 => value = value.wrapping_mul(16).wrapping_add(v as u32),
                            _ => bad = true,
                        }
                    }
                } else {
                    if digits == terminator {
                        bad = true;
                    }
                    while !bad && digits < terminator {
                        let d = input[digits];
                        digits += 1;
                        if d.is_ascii_digit() {
                            value = value.wrapping_mul(10).wrapping_add((d - b'0') as u32);
                        } else {
                            bad = true;
                        }
                    }
                }
                if !bad && value <= 0x10FFFF {
                    out.push(value);
                    p = terminator + 1;
                    continue;
                }
            } else if terminator > p {
                let name = &input[p..terminator];
                if let Some(&(_, code)) = ENTITIES.iter().find(|(n, _)| n.as_bytes() == name) {
                    out.push(code);
                    p = terminator + 1;
                    continue;
                }
            }
        }
        out.push(b'&' as u32);
        while p < terminator {
            out.push(input[p] as u32);
            p += 1;
        }
        if terminator < e && input[terminator] == b';' {
            out.push(b';' as u32);
            p += 1;
        }
    }
}

/// `mb_wchar_to_htmlent`.
pub fn encode_htmlent(input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        // php's `htmlentitifieds` table: only bytes >= 0x80 (and every
        // non-byte code point) become entities; `& < >` stay literal.
        if w < 0x80 {
            buf.out.push(w as u8);
            continue;
        }
        buf.out.push(b'&');
        if let Some(&(name, _)) = ENTITIES.iter().find(|(_, c)| *c == w) {
            buf.out.extend_from_slice(name.as_bytes());
            buf.out.push(b';');
            continue;
        }
        buf.out.push(b'#');
        buf.out.extend_from_slice(w.to_string().as_bytes());
        buf.out.push(b';');
    }
}

#[cfg(test)]
mod tests {
    use super::super::{name2encoding, ConvertBuf};
    use super::*;

    #[test]
    fn base64_and_qprint_round_trip() {
        let b64 = name2encoding(b"BASE64").unwrap();
        let mut buf = ConvertBuf::default_subst();
        b64.encode(&"Hello, World".bytes().map(|b| b as u32).collect::<Vec<_>>(), &mut buf, true);
        assert_eq!(buf.out, b"SGVsbG8sIFdvcmxk");
        let mut out = Vec::new();
        decode_base64(b"SGVsbG8sIFdvcmxk", &mut out);
        assert_eq!(out, "Hello, World".bytes().map(|b| b as u32).collect::<Vec<_>>());
        let mut buf = ConvertBuf::default_subst();
        encode_qprint(&[0xE9, b'=' as u32, b'a' as u32, b'\n' as u32], &mut buf);
        assert_eq!(buf.out, b"=E9=3Da\r\n");
        let mut out = Vec::new();
        decode_qprint(b"=E9=3Da=\r\nb", &mut out);
        assert_eq!(out, vec![0xE9, b'=' as u32, b'a' as u32, b'b' as u32]);
    }

    #[test]
    fn uuencode_round_trip() {
        let data: Vec<u32> = (0..100u32).collect();
        let mut buf = ConvertBuf::default_subst();
        encode_uuencode(&data, &mut buf, true);
        assert!(buf.out.starts_with(b"begin 0644 filename\nM"));
        let mut out = Vec::new();
        decode_uuencode(&buf.out, &mut out);
        assert_eq!(out, data);
    }

    #[test]
    fn html_entities() {
        let mut out = Vec::new();
        decode_htmlent(b"a&amp;&#233;&#x1F600;&bogus;&#;&", &mut out);
        assert_eq!(out, vec![0x61, 0x26, 0xE9, 0x1F600, 0x26, 0x62, 0x6F, 0x67, 0x75, 0x73, 0x3B, 0x26, 0x23, 0x3B, 0x26]);
        let mut buf = ConvertBuf::default_subst();
        encode_htmlent(&[0x61, 0x26, 0xE9, 0x1F600, 0x3C], &mut buf);
        assert_eq!(buf.out, b"a&&eacute;&#128512;<");
    }
}
