//! The Japanese charsets, ported from `mbfilter_cjk.c` over php's own JIS
//! X 0208 / 0212 / CP932 tables ([`super::tables::jis`]): Shift_JIS
//! (`SJIS`), `CP932` and `SJIS-win`, `EUC-JP`, `eucJP-win`, `CP51932`,
//! `ISO-2022-JP` and `JIS`. These are byte-exact with libmbfl, including
//! its special cases (`U+00A5`/`U+203E` handling, the CP932 NEC/IBM
//! extension rows, user-defined areas) and its illegal-sequence rules.

use super::tables::jis::{
    CP932EXT1, CP932EXT1_PAIRED, CP932EXT2, CP932EXT3, CP932EXT3_EUCJP, CP932EXT3_PAIRED, JISX0208, JISX0212, SJIS_DECODE_TBL1,
    SJIS_DECODE_TBL2, SJIS_MOBILE_DECODE_TBL1, UCS_A1_JIS, UCS_A2_JIS, UCS_I_JIS, UCS_R_JIS,
};
use super::{illegal_output, ConvertBuf, Encoding, BAD_INPUT};

const CP932EXT1_MIN: usize = (13 - 1) * 94;
const CP932EXT2_MIN: usize = (89 - 1) * 94;
const CP932EXT3_MIN: usize = (115 - 1) * 94;

fn jisx0208(s: usize) -> u32 {
    JISX0208.get(s).copied().unwrap_or(0) as u32
}

fn jisx0212(s: usize) -> u32 {
    JISX0212.get(s).copied().unwrap_or(0) as u32
}

/// The reverse lookup shared by every encoder: the JIS code for `w` from
/// the `ucs_*_jis_table`s (`0` when absent).
fn ucs_to_jis(w: u32) -> u32 {
    let w = w as usize;
    if w < UCS_A1_JIS.len() {
        UCS_A1_JIS[w] as u32
    } else if (0x2000..0x2000 + UCS_A2_JIS.len()).contains(&w) {
        UCS_A2_JIS[w - 0x2000] as u32
    } else if (0x4E00..0x4E00 + UCS_I_JIS.len()).contains(&w) {
        UCS_I_JIS[w - 0x4E00] as u32
    } else if (0xFF00..0xFF00 + UCS_R_JIS.len()).contains(&w) {
        UCS_R_JIS[w - 0xFF00] as u32
    } else {
        0
    }
}

/// The code points libmbfl special-cases in every JIS-family encoder.
fn jis_special(w: u32) -> u32 {
    match w {
        0xFF3C => 0x2140,
        0x2225 => 0x2142,
        0xFF0D => 0x215D,
        0xFFE0 => 0x2171,
        0xFFE1 => 0x2172,
        0xFFE2 => 0x224C,
        _ => 0,
    }
}

/// `SJIS_ENCODE`: JIS row/cell bytes to Shift_JIS bytes.
fn sjis_encode(c1: u32, c2: u32) -> (u8, u8) {
    let s1 = ((c1 - 1) >> 1) + if c1 < 0x5F { 0x71 } else { 0xB1 };
    let mut s2 = c2;
    if c1 & 1 != 0 {
        if c2 < 0x60 {
            s2 -= 1;
        }
        s2 += 0x20;
    } else {
        s2 += 0x7E;
    }
    (s1 as u8, s2 as u8)
}

fn paired_lookup(table: &[(u16, u16)], w: u32) -> Option<u32> {
    if w > 0xFFFF {
        return None;
    }
    table.binary_search_by_key(&(w as u16), |&(c, _)| c).ok().map(|i| table[i].1 as u32)
}

// ---- Shift_JIS -------------------------------------------------------------------------

/// `mb_sjis_to_wchar`.
pub fn decode_sjis(input: &[u8], out: &mut Vec<u32>) {
    let n = input.len();
    if n == 0 {
        return;
    }
    let e = n - 1;
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c <= 0x7F {
            out.push(c as u32);
        } else if (0xA1..=0xDF).contains(&c) {
            out.push(0xFEC0 + c as u32);
        } else {
            let c2 = input[p];
            p += 1;
            let w = SJIS_DECODE_TBL1[c as usize] as u32 + SJIS_DECODE_TBL2[c2 as usize] as u32;
            if (w as usize) < JISX0208.len() {
                let w = jisx0208(w as usize);
                out.push(if w == 0 { BAD_INPUT } else { w });
            } else {
                if c == 0x80 || c == 0xA0 || c > 0xEF {
                    p -= 1;
                }
                out.push(BAD_INPUT);
            }
        }
    }
    if p == e {
        let c = input[p];
        if c <= 0x7F {
            out.push(c as u32);
        } else if (0xA1..=0xDF).contains(&c) {
            out.push(0xFEC0 + c as u32);
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_wchar_to_sjis`.
pub fn encode_sjis(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        let mut s = ucs_to_jis(w);
        if s == 0 {
            s = match w {
                0xA5 => 0x216F,
                0xAF | 0x203E => 0x2131,
                0 => 0,
                _ => jis_special(w),
            };
            if s == 0 && w != 0 {
                illegal_output(w, enc, buf);
                continue;
            }
        } else if s >= 0x8080 {
            illegal_output(w, enc, buf);
            continue;
        }
        if s <= 0xFF {
            buf.out.push(s as u8);
        } else {
            let (a, b) = sjis_encode((s >> 8) & 0xFF, s & 0xFF);
            buf.out.push(a);
            buf.out.push(b);
        }
    }
}

// ---- CP932 / SJIS-win ------------------------------------------------------------------

/// The CP932 tweaks to the first JIS rows (`s <= 137`).
fn cp932_low(s: usize) -> u32 {
    match s {
        31 => 0xFF3C,
        32 => 0xFF5E,
        33 => 0x2225,
        60 => 0xFF0D,
        80 => 0xFFE0,
        81 => 0xFFE1,
        137 => 0xFFE2,
        _ => 0,
    }
}

/// `mb_cp932_to_wchar` (also `SJIS-win`).
pub fn decode_cp932(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c < 0x80 {
            out.push(c as u32);
        } else if c > 0xA0 && c < 0xE0 {
            out.push(0xFEC0 + c as u32);
        } else {
            if p == e {
                out.push(BAD_INPUT);
                break;
            }
            let c2 = input[p];
            p += 1;
            let s = (SJIS_MOBILE_DECODE_TBL1[c as usize] as u32 + SJIS_DECODE_TBL2[c2 as usize] as u32) as usize;
            let mut w = if s <= 137 { cp932_low(s) } else { 0 };
            if w == 0 {
                w = if (CP932EXT1_MIN..CP932EXT1_MIN + CP932EXT1.len()).contains(&s) {
                    CP932EXT1[s - CP932EXT1_MIN] as u32
                } else if s < JISX0208.len() {
                    jisx0208(s)
                } else if (CP932EXT2_MIN..CP932EXT2_MIN + CP932EXT2.len()).contains(&s) {
                    CP932EXT2[s - CP932EXT2_MIN] as u32
                } else if (CP932EXT3_MIN..CP932EXT3_MIN + CP932EXT3.len()).contains(&s) {
                    CP932EXT3[s - CP932EXT3_MIN] as u32
                } else if (94 * 94..114 * 94).contains(&s) {
                    (s - 94 * 94) as u32 + 0xE000
                } else {
                    0
                };
            }
            if w == 0 {
                if c == 0x80 || c == 0xA0 || c >= 0xFD {
                    p -= 1;
                }
                w = BAD_INPUT;
            }
            out.push(w);
        }
    }
}

/// `mb_wchar_to_cp932` / `mb_wchar_to_sjiswin` (`win` selects the latter's
/// `U+00A5` → fullwidth yen mapping instead of `0x5C`).
pub fn encode_cp932(enc: &Encoding, win: bool, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        let mut s2 = 0u32;
        let mut s1 = if w == 0x203E && !win {
            0x7E
        } else if (0xE000..0xE000 + 20 * 94).contains(&w) {
            let v = w - 0xE000;
            s2 = 1;
            ((v / 94 + 0x7F) << 8) | (v % 94 + 0x21)
        } else {
            ucs_to_jis(w)
        };
        match w {
            0xA5 => s1 = if win { 0x216F } else { 0x5C },
            0 => {
                buf.out.push(0);
                continue;
            }
            _ => {
                let sp = jis_special(w);
                if sp != 0 {
                    s1 = sp;
                }
            }
        }
        if s1 == 0 || (s1 >= 0x8080 && s2 == 0) {
            if let Some(i) = paired_lookup(CP932EXT1_PAIRED, w) {
                s1 = ((i / 94 + 0x2D) << 8) + (i % 94 + 0x21);
            } else if let Some(i) = paired_lookup(CP932EXT3_PAIRED, w) {
                s1 = ((i / 94 + 0x93) << 8) + (i % 94 + 0x21);
            } else {
                illegal_output(w, enc, buf);
                continue;
            }
        }
        if s1 < 0x100 {
            buf.out.push(s1 as u8);
        } else {
            let (a, b) = sjis_encode((s1 >> 8) & 0xFF, s1 & 0xFF);
            buf.out.push(a);
            buf.out.push(b);
        }
    }
}

// ---- EUC-JP ---------------------------------------------------------------------------------

/// `mb_eucjp_to_wchar`.
pub fn decode_eucjp(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c < 0x80 {
            out.push(c as u32);
        } else if (0xA1..=0xFE).contains(&c) && p < e {
            let c2 = input[p];
            p += 1;
            if (0xA1..=0xFE).contains(&c2) {
                let s = (c as usize - 0xA1) * 94 + c2 as usize - 0xA1;
                let w = jisx0208(s);
                out.push(if w == 0 { BAD_INPUT } else { w });
            } else {
                out.push(BAD_INPUT);
            }
        } else if c == 0x8E && p < e {
            let c2 = input[p];
            p += 1;
            out.push(if (0xA1..=0xDF).contains(&c2) { 0xFEC0 + c2 as u32 } else { BAD_INPUT });
        } else if c == 0x8F {
            if e - p >= 2 {
                let c2 = input[p];
                let c3 = input[p + 1];
                p += 2;
                if (0xA1..=0xFE).contains(&c3) && (0xA1..=0xFE).contains(&c2) {
                    let s = (c2 as usize - 0xA1) * 94 + c3 as usize - 0xA1;
                    let w = jisx0212(s);
                    out.push(if w == 0 { BAD_INPUT } else { w });
                } else {
                    out.push(BAD_INPUT);
                }
            } else {
                out.push(BAD_INPUT);
                p = e;
            }
        } else {
            out.push(BAD_INPUT);
        }
    }
}

fn push_eucjp(s: u32, buf: &mut ConvertBuf) {
    if s < 0x80 {
        buf.out.push(s as u8);
    } else if s < 0x100 {
        buf.out.push(0x8E);
        buf.out.push(s as u8);
    } else if s < 0x8080 {
        buf.out.push(((s >> 8) & 0xFF) as u8 | 0x80);
        buf.out.push((s & 0xFF) as u8 | 0x80);
    } else {
        buf.out.push(0x8F);
        buf.out.push(((s >> 8) & 0xFF) as u8 | 0x80);
        buf.out.push((s & 0xFF) as u8 | 0x80);
    }
}

/// `mb_wchar_to_eucjp`.
pub fn encode_eucjp(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        let mut s = if w == 0xAF { 0xA2B4 } else { ucs_to_jis(w) };
        if s == 0 {
            s = jis_special(w);
            if s == 0 {
                if w == 0 {
                    buf.out.push(0);
                } else {
                    illegal_output(w, enc, buf);
                }
                continue;
            }
        }
        push_eucjp(s, buf);
    }
}

// ---- eucJP-win / CP51932 ---------------------------------------------------------------------

/// `mb_eucjpwin_to_wchar`, or `mb_cp51932_to_wchar` when `cp51932`.
pub fn decode_eucjpwin(input: &[u8], cp51932: bool, out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    while p < e {
        let c = input[p];
        p += 1;
        if c < 0x80 {
            out.push(c as u32);
        } else if (0xA1..=0xFE).contains(&c) && p < e {
            let c2 = input[p];
            p += 1;
            if (0xA1..=0xFE).contains(&c2) {
                let s = (c as usize - 0xA1) * 94 + c2 as usize - 0xA1;
                let mut w = if s <= 137 { cp932_low(s) } else { 0 };
                if w == 0 {
                    w = if (CP932EXT1_MIN..CP932EXT1_MIN + CP932EXT1.len()).contains(&s) {
                        CP932EXT1[s - CP932EXT1_MIN] as u32
                    } else if s < JISX0208.len() {
                        jisx0208(s)
                    } else if cp51932 {
                        if (CP932EXT2_MIN..CP932EXT2_MIN + CP932EXT2.len()).contains(&s) {
                            CP932EXT2[s - CP932EXT2_MIN] as u32
                        } else {
                            0
                        }
                    } else if s >= 84 * 94 {
                        (s - 84 * 94) as u32 + 0xE000
                    } else {
                        0
                    };
                }
                out.push(if w == 0 { BAD_INPUT } else { w });
            } else {
                out.push(BAD_INPUT);
            }
        } else if c == 0x8E && p < e {
            let c2 = input[p];
            p += 1;
            out.push(if (0xA1..=0xDF).contains(&c2) { 0xFEC0 + c2 as u32 } else { BAD_INPUT });
        } else if c == 0x8F && p < e && !cp51932 {
            let c2 = input[p];
            p += 1;
            if p == e {
                out.push(BAD_INPUT);
                continue;
            }
            let c3 = input[p];
            p += 1;
            if (0xA1..=0xFE).contains(&c2) && (0xA1..=0xFE).contains(&c3) {
                let s = (c2 as usize - 0xA1) * 94 + c3 as usize - 0xA1;
                let mut w = 0;
                if s < JISX0212.len() {
                    w = jisx0212(s);
                    if w == 0x7E {
                        w = 0xFF5E;
                    }
                } else if (82 * 94..84 * 94).contains(&s) {
                    let code = ((c2 as u16) << 8) | c3 as u16;
                    if let Some(i) = CP932EXT3_EUCJP.iter().position(|&t| t == code) {
                        w = CP932EXT3.get(i).copied().unwrap_or(0) as u32;
                    }
                } else if s >= 84 * 94 {
                    w = (s - 84 * 94) as u32 + 0xE000 + 94 * 10;
                }
                if w == 0xA6 {
                    w = 0xFFE4;
                }
                out.push(if w == 0 { BAD_INPUT } else { w });
            } else {
                out.push(BAD_INPUT);
            }
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_wchar_to_eucjpwin`.
pub fn encode_eucjpwin(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        let mut s;
        if w == 0 {
            buf.out.push(0);
            continue;
        } else if w == 0xAF {
            s = 0xA2B4;
        } else if w == 0x203E {
            s = 0x7E;
        } else if (0xE000..0xE000 + 10 * 94).contains(&w) {
            let v = w - 0xE000;
            s = ((v / 94 + 0x75) << 8) + (v % 94) + 0x21;
        } else if (0xE000 + 10 * 94..0xE000 + 20 * 94).contains(&w) {
            let v = w - (0xE000 + 10 * 94);
            s = ((v / 94 + 0xF5) << 8) + (v % 94) + 0xA1;
        } else {
            s = ucs_to_jis(w);
        }
        if s == 0xA2F1 {
            s = 0x2D62;
        }
        if s == 0 {
            s = match w {
                0xA5 => 0x5C,
                0x2014 => 0x213D,
                _ => jis_special(w),
            };
            if s == 0 {
                if let Some(i) = CP932EXT1.iter().position(|&t| t as u32 == w) {
                    s = ((((i / 94) + CP932EXT1_MIN / 94 + 0x21) as u32) << 8) + (i % 94) as u32 + 0x21;
                } else if let Some(i) = CP932EXT3.iter().position(|&t| t as u32 == w) {
                    s = CP932EXT3_EUCJP.get(i).copied().unwrap_or(0) as u32;
                }
            }
        }
        if s == 0 {
            illegal_output(w, enc, buf);
        } else {
            push_eucjp(s, buf);
        }
    }
}

/// `mb_wchar_to_cp51932`.
pub fn encode_cp51932(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w == 0 {
            buf.out.push(0);
            continue;
        }
        let mut s = ucs_to_jis(w);
        if s >= 0x8080 {
            s = 0;
        }
        if s == 0 {
            s = match w {
                0xA5 => 0x216F,
                _ => jis_special(w),
            };
            if s == 0 {
                if let Some(i) = CP932EXT1.iter().position(|&t| t as u32 == w) {
                    s = (((i / 94) as u32 + 0x2D) << 8) + (i % 94) as u32 + 0x21;
                } else if let Some(i) = CP932EXT2.iter().position(|&t| t as u32 == w) {
                    s = (((i / 94) as u32 + 0x79) << 8) + (i % 94) as u32 + 0x21;
                }
            }
        }
        if s == 0 || s >= 0x8080 {
            illegal_output(w, enc, buf);
        } else {
            push_eucjp(s, buf);
        }
    }
}

// ---- ISO-2022-JP / JIS -----------------------------------------------------------------

const ASCII: u32 = 0;
const JISX_0201_LATIN: u32 = 1;
const JISX_0201_KANA: u32 = 2;
const JISX_0208: u32 = 3;
const JISX_0212: u32 = 4;

/// `mb_iso2022jp_to_wchar` (shared by `JIS`).
pub fn decode_iso2022jp(input: &[u8], out: &mut Vec<u32>) {
    let e = input.len();
    let mut p = 0;
    let mut state = ASCII;
    while p < e {
        let c = input[p];
        p += 1;
        if c == 0x1B {
            if e - p < 2 {
                out.push(BAD_INPUT);
                if p != e && (input[p] == b'$' || input[p] == b'(') {
                    p += 1;
                }
                continue;
            }
            let c2 = input[p];
            p += 1;
            if c2 == b'$' {
                let c3 = input[p];
                p += 1;
                if c3 == b'@' || c3 == b'B' {
                    state = JISX_0208;
                } else if c3 == b'(' {
                    if p == e {
                        out.push(BAD_INPUT);
                        break;
                    }
                    let c4 = input[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        state = JISX_0208;
                    } else if c4 == b'D' {
                        state = JISX_0212;
                    } else {
                        out.push(BAD_INPUT);
                        out.push(b'$' as u32);
                        out.push(b'(' as u32);
                        p -= 1;
                    }
                } else {
                    out.push(BAD_INPUT);
                    out.push(b'$' as u32);
                    p -= 1;
                }
            } else if c2 == b'(' {
                let c3 = input[p];
                p += 1;
                if c3 == b'B' || c3 == b'H' {
                    state = ASCII;
                } else if c3 == b'J' {
                    state = JISX_0201_LATIN;
                } else if c3 == b'I' {
                    state = JISX_0201_KANA;
                } else {
                    out.push(BAD_INPUT);
                    out.push(b'(' as u32);
                    p -= 1;
                }
            } else {
                out.push(BAD_INPUT);
                p -= 1;
            }
        } else if c == 0xE {
            state = JISX_0201_KANA;
        } else if c == 0xF {
            state = ASCII;
        } else if state == JISX_0201_LATIN && c == 0x5C {
            out.push(0xA5);
        } else if state == JISX_0201_LATIN && c == 0x7E {
            out.push(0x203E);
        } else if state == JISX_0201_KANA && c > 0x20 && c < 0x60 {
            out.push(0xFF40 + c as u32);
        } else if state >= JISX_0208 && c > 0x20 && c < 0x7F {
            if p == e {
                out.push(BAD_INPUT);
                break;
            }
            let c2 = input[p];
            p += 1;
            if c2 > 0x20 && c2 < 0x7F {
                let s = (c as usize - 0x21) * 94 + c2 as usize - 0x21;
                let w = if state == JISX_0208 { jisx0208(s) } else { jisx0212(s) };
                out.push(if w == 0 { BAD_INPUT } else { w });
            } else {
                out.push(BAD_INPUT);
            }
        } else if c < 0x80 {
            out.push(c as u32);
        } else if (0xA1..=0xDF).contains(&c) {
            out.push(0xFEC0 + c as u32);
        } else {
            out.push(BAD_INPUT);
        }
    }
}

/// `mb_wchar_to_iso2022jp` / `mb_wchar_to_jis` (`jis` enables JIS X 0201
/// kana via `ESC ( I` and the overline mapping).
pub fn encode_iso2022jp(enc: &Encoding, jis: bool, input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let mut s = if jis && w == 0x203E { 0x1007E } else { ucs_to_jis(w) };
        if s == 0 {
            s = match w {
                0xA5 => 0x1005C,
                0 => 0,
                _ => jis_special(w),
            };
            if s == 0 && w != 0 {
                illegal_output(w, enc, buf);
                continue;
            }
        } else if !jis && ((0x80..0x2121).contains(&s) || s > 0x8080) {
            illegal_output(w, enc, buf);
            continue;
        }
        if s < 0x80 {
            if buf.state != ASCII {
                buf.out.extend_from_slice(b"\x1b(B");
                buf.state = ASCII;
            }
            buf.out.push(s as u8);
        } else if jis && (0xA1..=0xDF).contains(&s) {
            if buf.state != JISX_0201_KANA {
                buf.out.extend_from_slice(b"\x1b(I");
                buf.state = JISX_0201_KANA;
            }
            buf.out.push((s & 0x7F) as u8);
        } else if s < 0x8080 {
            if buf.state != JISX_0208 {
                buf.out.extend_from_slice(b"\x1b$B");
                buf.state = JISX_0208;
            }
            buf.out.push(((s >> 8) & 0x7F) as u8);
            buf.out.push((s & 0x7F) as u8);
        } else if s < 0x10000 {
            if buf.state != JISX_0212 {
                buf.out.extend_from_slice(b"\x1b$(D");
                buf.state = JISX_0212;
            }
            buf.out.push(((s >> 8) & 0x7F) as u8);
            buf.out.push((s & 0x7F) as u8);
        } else {
            if buf.state != JISX_0201_LATIN {
                buf.out.extend_from_slice(b"\x1b(J");
                buf.state = JISX_0201_LATIN;
            }
            buf.out.push((s & 0x7F) as u8);
        }
    }
    if end && buf.state != ASCII {
        buf.out.extend_from_slice(b"\x1b(B");
        buf.state = ASCII;
    }
}

/// `mb_check_jis` / `mb_check_iso2022jp`.
pub fn check_iso2022jp(input: &[u8], jis: bool) -> bool {
    const KANA_SO: u32 = 5;
    let e = input.len();
    let mut p = 0;
    let mut state = ASCII;
    while p < e {
        let c = input[p];
        p += 1;
        if c == 0x1B {
            if jis && state == KANA_SO {
                return false;
            }
            if e - p < 2 {
                return false;
            }
            let c2 = input[p];
            p += 1;
            if c2 == b'$' {
                let c3 = input[p];
                p += 1;
                if c3 == b'@' || c3 == b'B' {
                    state = JISX_0208;
                } else if jis && c3 == b'(' {
                    if p == e {
                        return false;
                    }
                    let c4 = input[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        state = JISX_0208;
                    } else if c4 == b'D' {
                        state = JISX_0212;
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            } else if c2 == b'(' {
                let c3 = input[p];
                p += 1;
                if c3 == b'B' || (jis && c3 == b'H') {
                    state = ASCII;
                } else if c3 == b'J' {
                    state = JISX_0201_LATIN;
                } else if jis && c3 == b'I' {
                    state = JISX_0201_KANA;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        } else if c == 0xE {
            if !jis || state != ASCII {
                return false;
            }
            state = KANA_SO;
        } else if c == 0xF {
            if !jis || state != KANA_SO {
                return false;
            }
            state = ASCII;
        } else if (state == JISX_0208 || state == JISX_0212) && c > 0x20 && c < 0x7F {
            if p == e {
                return false;
            }
            let c2 = input[p];
            p += 1;
            if !(c2 > 0x20 && c2 < 0x7F) {
                return false;
            }
            let s = (c as usize - 0x21) * 94 + c2 as usize - 0x21;
            let w = if state == JISX_0208 { jisx0208(s) } else { jisx0212(s) };
            if w == 0 {
                return false;
            }
        } else if c < 0x80 {
            continue;
        } else if jis && (0xA1..=0xDF).contains(&c) {
            continue;
        } else {
            return false;
        }
    }
    state == ASCII
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
    fn sjis_matches_php() {
        assert_eq!(conv(b"\x5c\x7e\x81\x60\x81\x5f", "SJIS", "UTF-8"), b"\x5c\x7e\xe3\x80\x9c\xef\xbc\xbc");
        assert_eq!(conv(b"\x5c\x7e\x81\x60\x81\x5f", "CP932", "UTF-8"), b"\x5c\x7e\xef\xbd\x9e\xef\xbc\xbc");
        assert_eq!(conv("〜～\\¥‾".as_bytes(), "UTF-8", "SJIS"), b"\x81\x60\x81\x60\x5c\x81\x8f\x81\x50");
        assert_eq!(conv("〜～\\¥‾".as_bytes(), "UTF-8", "CP932"), b"\x81\x60\x81\x60\x5c\x5c\x7e");
        assert_eq!(conv(b"\x87\x90\xfa\x40", "CP932", "UTF-8"), "≒ⅰ".as_bytes());
        assert_eq!(conv(b"\x87\x90", "SJIS", "UTF-8"), b"?");
        assert_eq!(conv(b"\xe3\x81\x82\xff", "SJIS", "UTF-8"), b"\xe7\xb8\xba?");
        assert_eq!(conv(b"\x80\xa0\xfd\xfe\xff", "SJIS", "UTF-8"), b"?????");
    }

    #[test]
    fn eucjp_and_iso2022jp_match_php() {
        assert_eq!(conv("〜～\\¥‾".as_bytes(), "UTF-8", "EUC-JP"), b"\xa1\xc1\xa1\xc1\x5c\x3f\xa1\xb1");
        assert_eq!(conv("¦№".as_bytes(), "UTF-8", "EUC-JP"), b"\x8f\xa2\xc3\x8f\xa2\xf1");
        assert_eq!(conv("№".as_bytes(), "UTF-8", "eucJP-win"), b"\xad\xe2");
        assert_eq!(conv("〜～\\¥‾ｱ".as_bytes(), "UTF-8", "ISO-2022-JP"), b"\x1b$B!A!A\x1b(B\\\x1b(J\\\x1b$B!1\x1b(B?");
        assert_eq!(conv("〜～\\¥‾ｱ".as_bytes(), "UTF-8", "JIS"), b"\x1b$B!A!A\x1b(B\\\x1b(J\\~\x1b(I1\x1b(B");
        assert_eq!(conv(b"\x1b$BF|K\\8l\x1b(B", "ISO-2022-JP", "UTF-8"), "日本語".as_bytes());
    }
}
