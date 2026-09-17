//! Case mapping (`php_unicode.c`): the eight `MB_CASE_*` modes over php's
//! tables (`unicode_data.h`, Unicode 16 with SpecialCasing and CaseFolding),
//! including the Turkish-aware handling for ISO-8859-9, final-sigma
//! lowercasing and title-case word tracking via `Cased`/`Case_Ignorable`.

use super::tables::casemap::{EXTRA, FOLD, LOWER, TITLE, UPPER};
use super::tables::props::{CASED, CASE_IGNORABLE};
use super::{ConvertBuf, Encoding, Id};

/// `php_case_mode`, numbered as the `MB_CASE_*` constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(missing_docs)]
pub enum CaseMode {
    Upper = 0,
    Lower = 1,
    Title = 2,
    Fold = 3,
    UpperSimple = 4,
    LowerSimple = 5,
    TitleSimple = 6,
    FoldSimple = 7,
}

impl CaseMode {
    /// The mode for an `MB_CASE_*` value.
    pub fn from_i64(n: i64) -> Option<CaseMode> {
        Some(match n {
            0 => CaseMode::Upper,
            1 => CaseMode::Lower,
            2 => CaseMode::Title,
            3 => CaseMode::Fold,
            4 => CaseMode::UpperSimple,
            5 => CaseMode::LowerSimple,
            6 => CaseMode::TitleSimple,
            7 => CaseMode::FoldSimple,
            _ => return None,
        })
    }
}

fn in_ranges(ranges: &[(u32, u32)], code: u32) -> bool {
    let i = ranges.partition_point(|&(_, hi)| hi < code);
    ranges.get(i).is_some_and(|&(lo, hi)| lo <= code && code <= hi)
}

fn lookup(table: &[(u32, u32)], code: u32) -> Option<u32> {
    table.binary_search_by_key(&code, |&(c, _)| c).ok().map(|i| table[i].1)
}

/// `php_unicode_is_cased`.
pub fn is_cased(w: u32) -> bool {
    in_ranges(CASED, w)
}

/// `php_unicode_is_case_ignorable`.
pub fn is_case_ignorable(w: u32) -> bool {
    in_ranges(CASE_IGNORABLE, w)
}

fn is_turkish(enc: &Encoding) -> bool {
    enc.id == Id::Iso8859_9
}

/// `php_unicode_toupper_raw`: the mapping, possibly a special-casing
/// reference (`> 0xFFFFFF`).
fn toupper_raw(code: u32, enc: &Encoding) -> u32 {
    if code < 0xB5 {
        if (0x61..=0x7A).contains(&code) {
            if is_turkish(enc) && code == 0x69 {
                return 0x130;
            }
            return code - 0x20;
        }
        code
    } else {
        lookup(UPPER, code).unwrap_or(code)
    }
}

fn tolower_raw(code: u32, enc: &Encoding) -> u32 {
    if code < 0xC0 {
        if (0x41..=0x5A).contains(&code) {
            if is_turkish(enc) && code == 0x49 {
                return 0x131;
            }
            return code + 0x20;
        }
        code
    } else {
        match lookup(LOWER, code) {
            Some(_) if is_turkish(enc) && code == 0x130 => 0x69,
            Some(n) => n,
            None => code,
        }
    }
}

fn totitle_raw(code: u32, enc: &Encoding) -> u32 {
    lookup(TITLE, code).unwrap_or_else(|| toupper_raw(code, enc))
}

fn tofold_raw(code: u32, enc: &Encoding) -> u32 {
    if code < 0x80 {
        if (0x41..=0x5A).contains(&code) {
            if is_turkish(enc) && code == 0x49 {
                return 0x131;
            }
            return code + 0x20;
        }
        code
    } else {
        match lookup(FOLD, code) {
            Some(_) if is_turkish(enc) && code == 0x130 => 0x69,
            Some(n) => n,
            None => code,
        }
    }
}

fn simple(code: u32) -> u32 {
    if code > 0xFFFFFF {
        EXTRA[(code & 0xFFFFFF) as usize]
    } else {
        code
    }
}

fn emit(w: u32, out: &mut Vec<u32>) {
    if w > 0xFFFFFF {
        let len = (w >> 24) as usize;
        let idx = (w & 0xFFFFFF) as usize;
        out.extend_from_slice(&EXTRA[idx + 1..idx + 1 + len]);
    } else {
        out.push(w);
    }
}

/// Whether the capital sigma at `i` is word-final: preceded (skipping
/// case-ignorables) by a cased letter and not followed by one.
fn final_sigma(w: &[u32], i: usize) -> bool {
    let mut j = i;
    loop {
        if j == 0 {
            return false;
        }
        j -= 1;
        if is_case_ignorable(w[j]) {
            continue;
        }
        if !is_cased(w[j]) {
            return false;
        }
        break;
    }
    let mut k = i + 1;
    while k < w.len() {
        if is_case_ignorable(w[k]) {
            k += 1;
            continue;
        }
        return !is_cased(w[k]);
    }
    true
}

/// Apply `mode` to decoded wchars (`php_unicode_convert_case` on the
/// wchar level). Wchars above `0xFFFFFF` (bad input) pass through.
pub fn convert_case_wchars(mode: CaseMode, w: &[u32], enc: &Encoding) -> Vec<u32> {
    let mut out = Vec::with_capacity(w.len());
    let mut title_mode = false;
    for (i, &c) in w.iter().enumerate() {
        if c > 0xFFFFFF {
            out.push(c);
            continue;
        }
        match mode {
            CaseMode::UpperSimple => out.push(simple(toupper_raw(c, enc))),
            CaseMode::LowerSimple => out.push(simple(tolower_raw(c, enc))),
            CaseMode::FoldSimple => out.push(simple(tofold_raw(c, enc))),
            CaseMode::TitleSimple => {
                out.push(simple(if title_mode { tolower_raw(c, enc) } else { totitle_raw(c, enc) }));
                if !is_case_ignorable(c) {
                    title_mode = is_cased(c);
                }
            }
            CaseMode::Upper => emit(toupper_raw(c, enc), &mut out),
            CaseMode::Lower => {
                if c == 0x3A3 && final_sigma(w, i) {
                    out.push(0x3C2);
                } else {
                    emit(tolower_raw(c, enc), &mut out);
                }
            }
            CaseMode::Fold => emit(tofold_raw(c, enc), &mut out),
            CaseMode::Title => {
                if title_mode {
                    if c == 0x3A3 && final_sigma(w, i) {
                        out.push(0x3C2);
                    } else {
                        emit(tolower_raw(c, enc), &mut out);
                    }
                } else {
                    emit(totitle_raw(c, enc), &mut out);
                }
                if !is_case_ignorable(c) {
                    title_mode = is_cased(c);
                }
            }
        }
    }
    out
}

/// `php_unicode_convert_case`: decode from `src`, map, encode to `dst`.
pub fn convert_case(mode: CaseMode, s: &[u8], src: &Encoding, dst: &Encoding, buf: &mut ConvertBuf) {
    let w = src.decode(s);
    let mapped = convert_case_wchars(mode, &w, src);
    dst.encode(&mapped, buf, true);
}

#[cfg(test)]
mod tests {
    use super::super::{ErrorMode, UTF8};
    use super::*;

    fn case(mode: CaseMode, s: &str) -> String {
        let mut buf = ConvertBuf::new(b'?' as u32, ErrorMode::Char);
        convert_case(mode, s.as_bytes(), &UTF8, &UTF8, &mut buf);
        String::from_utf8(buf.out).unwrap()
    }

    #[test]
    fn matches_php() {
        assert_eq!(case(CaseMode::Upper, "straße ǆ ŉ"), "STRASSE Ǆ ʼN");
        assert_eq!(case(CaseMode::Title, "ǆ straße"), "ǅ Straße");
        assert_eq!(case(CaseMode::Lower, "Σ ΟΔΟΣ Σ."), "σ οδος σ.");
        assert_eq!(case(CaseMode::Fold, "ΣΑΣ"), "σασ");
        assert_eq!(case(CaseMode::UpperSimple, "straße"), "STRASSE".replace("SS", "ß"));
        assert_eq!(case(CaseMode::Title, "hello wORLD o'neil"), "Hello World O'neil");
        assert_eq!(case(CaseMode::Lower, "ÀÉÎ"), "àéî");
    }
}
