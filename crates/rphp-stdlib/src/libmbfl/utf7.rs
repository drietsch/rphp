//! UTF-7 (`mbfilter_utf7.c`) and UTF7-IMAP (`mbfilter_utf7imap.c`):
//! modified-Base64 sections with php's rules for where a section may end,
//! surrogate handling and non-zero padding bits.

use super::{illegal_output, ConvertBuf, Encoding, BAD_INPUT, UTF32_MAX};

const DASH: u8 = 0xFC;
const DIRECT: u8 = 0xFD;
const ASCII: u8 = 0xFE;
const ILLEGAL: u8 = 0xFF;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_IMAP: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn is_optional_direct(c: u8) -> bool {
    matches!(c, b'!' | b'"' | b'#' | b'$' | b'%' | b'&' | b'*' | b';' | b'<' | b'=' | b'>' | b'@' | b'[' | b']' | b'^' | b'_' | b'`' | b'{' | b'|' | b'}')
}

fn can_end_base64(c: u32) -> bool {
    matches!(c, 0x20 | 0x09 | 0x0D | 0x0A | 0x27 | 0x28 | 0x29 | 0x2C | 0x2E | 0x3A | 0x3F)
}

fn decode_b64(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - 65,
        b'a'..=b'z' => c - 71,
        b'0'..=b'9' => c + 4,
        b'+' => 62,
        b'/' => 63,
        b'-' => DASH,
        _ if can_end_base64(c as u32) || is_optional_direct(c) || c == 0 => DIRECT,
        _ if c <= 0x7F => ASCII,
        _ => ILLEGAL,
    }
}

fn decode_b64_imap(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - 65,
        b'a'..=b'z' => c - 71,
        b'0'..=b'9' => c + 4,
        b'+' => 62,
        b',' => 63,
        b'-' => DASH,
        _ => ILLEGAL,
    }
}

fn is_end(n: u8) -> bool {
    n >= DASH
}

/// `handle_utf16_cp`: fold a UTF-16 unit into the output, pairing surrogates.
fn handle_cp(cp: u16, out: &mut Vec<u32>, surrogate1: &mut u16, imap: bool) {
    loop {
        if *surrogate1 != 0 {
            if (0xDC00..=0xDFFF).contains(&cp) {
                out.push((((*surrogate1 as u32) & 0x3FF) << 10) + (cp as u32 & 0x3FF) + 0x10000);
                *surrogate1 = 0;
            } else {
                out.push(BAD_INPUT);
                *surrogate1 = 0;
                continue;
            }
        } else if (0xD800..=0xDBFF).contains(&cp) {
            *surrogate1 = cp;
        } else if (0xDC00..=0xDFFF).contains(&cp) {
            out.push(BAD_INPUT);
        } else if imap && (0x20..=0x7E).contains(&cp) && cp != b'&' as u16 {
            out.push(BAD_INPUT);
        } else {
            out.push(cp as u32);
        }
        return;
    }
}

struct Dec<'a> {
    input: &'a [u8],
    p: usize,
    imap: bool,
}

impl Dec<'_> {
    fn next(&mut self) -> u8 {
        let c = self.input[self.p];
        self.p += 1;
        if self.imap {
            decode_b64_imap(c)
        } else {
            decode_b64(c)
        }
    }

    fn at_end(&self) -> bool {
        self.p >= self.input.len()
    }
}

/// `handle_base64_end` for both variants.
fn base64_end(d: &mut Dec, n: u8, out: &mut Vec<u32>, base64: &mut bool, abrupt: bool, surrogate1: &mut u16) {
    if d.imap {
        if abrupt || n == ILLEGAL || *surrogate1 != 0 {
            out.push(BAD_INPUT);
            *surrogate1 = 0;
        }
    } else {
        if abrupt || *surrogate1 != 0 {
            out.push(BAD_INPUT);
            *surrogate1 = 0;
        }
        if n == ILLEGAL {
            out.push(BAD_INPUT);
        } else if n == DIRECT || n == ASCII {
            d.p -= 1;
        }
    }
    *base64 = false;
}

fn decode_impl(input: &[u8], out: &mut Vec<u32>, imap: bool) {
    let mut d = Dec { input, p: 0, imap };
    let e = input.len();
    let mut base64 = false;
    let mut surrogate1: u16 = 0;

    while d.p < e {
        if base64 {
            let n1 = d.next();
            if is_end(n1) {
                base64_end(&mut d, n1, out, &mut base64, false, &mut surrogate1);
                continue;
            } else if d.at_end() {
                base64_end(&mut d, n1, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n2 = d.next();
            if is_end(n2) || d.at_end() {
                base64_end(&mut d, n2, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n3 = d.next();
            if is_end(n3) {
                base64_end(&mut d, n3, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            handle_cp(((n1 as u16) << 10) | ((n2 as u16) << 4) | ((n3 as u16 & 0x3C) >> 2), out, &mut surrogate1, imap);
            if d.at_end() {
                if n3 & 0x3 != 0 || surrogate1 != 0 {
                    out.push(BAD_INPUT);
                    surrogate1 = 0;
                }
                break;
            }
            let n4 = d.next();
            if is_end(n4) {
                base64_end(&mut d, n4, out, &mut base64, n3 & 0x3 != 0, &mut surrogate1);
                continue;
            } else if d.at_end() {
                base64_end(&mut d, n4, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n5 = d.next();
            if is_end(n5) || d.at_end() {
                base64_end(&mut d, n5, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n6 = d.next();
            if is_end(n6) {
                base64_end(&mut d, n6, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            handle_cp(
                ((n3 as u16) << 14) | ((n4 as u16) << 8) | ((n5 as u16) << 2) | ((n6 as u16 & 0x30) >> 4),
                out,
                &mut surrogate1,
                imap,
            );
            if d.at_end() {
                if n6 & 0xF != 0 || surrogate1 != 0 {
                    out.push(BAD_INPUT);
                    surrogate1 = 0;
                }
                break;
            }
            let n7 = d.next();
            if is_end(n7) {
                base64_end(&mut d, n7, out, &mut base64, n6 & 0xF != 0, &mut surrogate1);
                continue;
            } else if d.at_end() {
                base64_end(&mut d, n7, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n8 = d.next();
            if is_end(n8) {
                base64_end(&mut d, n8, out, &mut base64, true, &mut surrogate1);
                continue;
            }
            handle_cp(((n6 as u16) << 12) | ((n7 as u16) << 6) | n8 as u16, out, &mut surrogate1, imap);
        } else {
            let c = input[d.p];
            d.p += 1;
            if imap {
                if c == b'&' {
                    if d.p < e && input[d.p] == b'-' {
                        out.push(b'&' as u32);
                        d.p += 1;
                    } else {
                        base64 = true;
                    }
                } else if (0x20..=0x7E).contains(&c) {
                    out.push(c as u32);
                } else {
                    out.push(BAD_INPUT);
                }
            } else if c == b'+' {
                if d.p < e {
                    if input[d.p] == b'-' {
                        out.push(b'+' as u32);
                        d.p += 1;
                    } else {
                        base64 = true;
                    }
                }
            } else if c <= 0x7F {
                out.push(c as u32);
            } else {
                out.push(BAD_INPUT);
            }
        }
    }

    if d.p >= e {
        if imap {
            if base64 {
                out.push(BAD_INPUT);
            }
        } else if surrogate1 != 0 {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_utf7_to_wchar`.
pub fn decode_utf7(input: &[u8], out: &mut Vec<u32>) {
    decode_impl(input, out, false)
}

/// `mb_utf7imap_to_wchar`.
pub fn decode_utf7imap(input: &[u8], out: &mut Vec<u32>) {
    decode_impl(input, out, true)
}

fn should_direct_encode(c: u32) -> bool {
    (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c) || (0x30..=0x39).contains(&c) || c == 0 || c == 0x2F || c == 0x2D || can_end_base64(c)
}

fn save_state(buf: &mut ConvertBuf, base64: bool, nbits: u32, cache: u32) {
    buf.state = (cache << 4) | (nbits << 1) | base64 as u32;
}

fn load_state(buf: &ConvertBuf) -> (bool, u32, u32) {
    (buf.state & 1 != 0, (buf.state >> 1) & 0x7, buf.state >> 4)
}

fn encode_impl(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, end: bool, imap: bool) {
    let table = if imap { BASE64_IMAP } else { BASE64 };
    let (mut base64, mut nbits, mut cache) = load_state(buf);
    let mut i = 0;
    while i < input.len() {
        let w = input[i];
        i += 1;
        if base64 {
            let direct = if imap { (0x20..=0x7E).contains(&w) } else { should_direct_encode(w) };
            if direct {
                base64 = false;
                i -= 1;
                if nbits != 0 {
                    buf.out.push(table[((cache << (6 - nbits)) & 0x3F) as usize]);
                }
                nbits = 0;
                cache = 0;
                if imap || !can_end_base64(w) {
                    buf.out.push(b'-');
                }
            } else if w >= UTF32_MAX {
                save_state(buf, base64, nbits, cache);
                illegal_output(w, enc, buf);
                let s = load_state(buf);
                base64 = s.0;
                nbits = s.1;
                cache = s.2;
            } else {
                let bits: u64;
                if w >= 0x10000 {
                    let w = w - 0x10000;
                    bits = ((cache as u64) << 32) | 0xD800_DC00 | (((w & 0xFFC00) as u64) << 6) | (w & 0x3FF) as u64;
                    nbits += 32;
                } else {
                    bits = ((cache as u64) << 16) | w as u64;
                    nbits += 16;
                }
                while nbits >= 6 {
                    buf.out.push(table[((bits >> (nbits - 6)) & 0x3F) as usize]);
                    nbits -= 6;
                }
                cache = (bits & 0xFF) as u32;
            }
        } else if imap && w == b'&' as u32 {
            buf.out.push(b'&');
            buf.out.push(b'-');
        } else if if imap { (0x20..=0x7E).contains(&w) } else { should_direct_encode(w) } {
            buf.out.push(w as u8);
        } else if w >= UTF32_MAX {
            buf.state = 0;
            illegal_output(w, enc, buf);
            let s = load_state(buf);
            base64 = s.0;
            nbits = s.1;
            cache = s.2;
        } else {
            buf.out.push(if imap { b'&' } else { b'+' });
            base64 = true;
            i -= 1;
        }
    }

    if end {
        if nbits != 0 {
            buf.out.push(table[((cache << (6 - nbits)) & 0x3F) as usize]);
        }
        if base64 {
            buf.out.push(b'-');
        }
        buf.state = 0;
    } else {
        save_state(buf, base64, nbits, cache);
    }
}

/// `mb_wchar_to_utf7`.
pub fn encode_utf7(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, end: bool) {
    encode_impl(enc, input, buf, end, false)
}

/// `mb_wchar_to_utf7imap`.
pub fn encode_utf7imap(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf, end: bool) {
    encode_impl(enc, input, buf, end, true)
}

fn is_utf16_cp_valid(cp: u16, is_surrogate: bool, imap: bool) -> bool {
    if is_surrogate {
        (0xDC00..=0xDFFF).contains(&cp)
    } else if (0xDC00..=0xDFFF).contains(&cp) {
        false
    } else { !(imap && (0x20..=0x7E).contains(&cp) && cp != b'&' as u16) }
}

fn has_surrogate(cp: u16, is_surrogate: bool) -> bool {
    !is_surrogate && (0xD800..=0xDBFF).contains(&cp)
}

fn is_base64_end_valid(n: u8, gap: bool, is_surrogate: bool) -> bool {
    !(gap || is_surrogate || n == ASCII || n == ILLEGAL)
}

fn check_impl(input: &[u8], imap: bool) -> bool {
    let mut d = Dec { input, p: 0, imap };
    let e = input.len();
    let mut base64 = false;
    let mut is_surrogate = false;

    while d.p < e {
        if base64 {
            let n1 = d.next();
            if is_end(n1) {
                if imap {
                    if n1 == ILLEGAL || is_surrogate {
                        return false;
                    }
                } else if !is_base64_end_valid(n1, false, is_surrogate) {
                    return false;
                }
                base64 = false;
                continue;
            } else if d.at_end() {
                return false;
            }
            let n2 = d.next();
            if is_end(n2) || d.at_end() {
                return false;
            }
            let n3 = d.next();
            if is_end(n3) {
                return false;
            }
            let cp1 = ((n1 as u16) << 10) | ((n2 as u16) << 4) | ((n3 as u16 & 0x3C) >> 2);
            if !is_utf16_cp_valid(cp1, is_surrogate, imap) {
                return false;
            }
            is_surrogate = has_surrogate(cp1, is_surrogate);
            if d.at_end() {
                return !(n3 & 0x3 != 0 || is_surrogate);
            }
            let n4 = d.next();
            if is_end(n4) {
                if imap {
                    if n4 == ILLEGAL || n3 & 0x3 != 0 || is_surrogate {
                        return false;
                    }
                } else if !is_base64_end_valid(n4, n3 & 0x3 != 0, is_surrogate) {
                    return false;
                }
                base64 = false;
                continue;
            } else if d.at_end() {
                return false;
            }
            let n5 = d.next();
            if is_end(n5) || d.at_end() {
                return false;
            }
            let n6 = d.next();
            if is_end(n6) {
                return false;
            }
            let cp2 = ((n3 as u16) << 14) | ((n4 as u16) << 8) | ((n5 as u16) << 2) | ((n6 as u16 & 0x30) >> 4);
            if !is_utf16_cp_valid(cp2, is_surrogate, imap) {
                return false;
            }
            is_surrogate = has_surrogate(cp2, is_surrogate);
            if d.at_end() {
                return !(n6 & 0xF != 0 || is_surrogate);
            }
            let n7 = d.next();
            if is_end(n7) {
                if imap {
                    if n7 == ILLEGAL || n6 & 0xF != 0 || is_surrogate {
                        return false;
                    }
                } else if !is_base64_end_valid(n7, n6 & 0xF != 0, is_surrogate) {
                    return false;
                }
                base64 = false;
                continue;
            } else if d.at_end() {
                return false;
            }
            let n8 = d.next();
            if is_end(n8) {
                return false;
            }
            let cp3 = ((n6 as u16) << 12) | ((n7 as u16) << 6) | n8 as u16;
            if !is_utf16_cp_valid(cp3, is_surrogate, imap) {
                return false;
            }
            is_surrogate = has_surrogate(cp3, is_surrogate);
        } else {
            let c = input[d.p];
            d.p += 1;
            if imap {
                if c == b'&' {
                    if d.p == e {
                        return false;
                    }
                    if input[d.p] == b'-' {
                        d.p += 1;
                    } else {
                        base64 = true;
                    }
                } else if !(0x20..=0x7E).contains(&c) {
                    return false;
                }
            } else if c == b'+' {
                if d.p == e {
                    return !is_surrogate;
                }
                let n = decode_b64(input[d.p]);
                if n == DASH {
                    d.p += 1;
                } else if n > DASH {
                    return false;
                } else {
                    base64 = true;
                }
            } else if !(should_direct_encode(c as u32) || is_optional_direct(c) || c == 0) {
                return false;
            }
        }
    }
    if imap {
        !base64 && !is_surrogate
    } else {
        !is_surrogate
    }
}

/// `mb_check_utf7`.
pub fn check_utf7(input: &[u8]) -> bool {
    check_impl(input, false)
}

/// `mb_check_utf7imap`.
pub fn check_utf7imap(input: &[u8]) -> bool {
    check_impl(input, true)
}

#[cfg(test)]
mod tests {
    use super::super::{name2encoding, ConvertBuf};
    use super::*;

    #[test]
    fn utf7_round_trip() {
        let enc = name2encoding(b"UTF-7").unwrap();
        let mut buf = ConvertBuf::default_subst();
        enc.encode(&[0x61, 0xE9, 0x31, b'+' as u32, 0x1F600, 0x2E], &mut buf, true);
        assert_eq!(buf.out, b"a+AOk-1+-+2D3eAA.");
        let mut out = Vec::new();
        decode_utf7(&buf.out, &mut out);
        assert_eq!(out, vec![0x61, 0xE9, 0x31, b'+' as u32, 0x1F600, 0x2E]);
        assert!(check_utf7(b"a+AOk-1"));
        assert!(!check_utf7(b"\xe9"));
    }

    #[test]
    fn utf7imap_round_trip() {
        let enc = name2encoding(b"UTF7-IMAP").unwrap();
        let mut buf = ConvertBuf::default_subst();
        enc.encode(&[0x61, 0xE9, b'&' as u32], &mut buf, true);
        assert_eq!(buf.out, b"a&AOk-&-");
        let mut out = Vec::new();
        decode_utf7imap(&buf.out, &mut out);
        assert_eq!(out, vec![0x61, 0xE9, b'&' as u32]);
        assert!(check_utf7imap(b"a&AOk-"));
        assert!(!check_utf7imap(b"a&AOk"));
    }
}
