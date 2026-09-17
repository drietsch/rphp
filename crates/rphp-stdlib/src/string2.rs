//! The second half of php-src `ext/standard/string.c` (the first lives in
//! `strings.rs`): C-style slashes, path functions, phonetic and distance
//! functions, `str_getcsv`, `strip_tags`, `wordwrap`, `sscanf`, `strtok`,
//! natural-order comparison, the C-locale stubs of the locale functions, and
//! the `ENT_*`/`HTML_*`/`STR_PAD_*`/`PATHINFO_*`/`LC_*`/`CRYPT_*` constants.
//! Everything is byte-oriented; "alphabetic"/"space" always mean the C locale.
use std::cell::RefCell;

use rphp_value::{Array, ArrayKey, Str, Value};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, Unwind};

use crate::strings::{charmask, str_value};

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("addcslashes", 2, Some(2), addcslashes),
    nf!("stripcslashes", 1, Some(1), stripcslashes),
    nf!("chunk_split", 1, Some(3), chunk_split),
    nf!("count_chars", 1, Some(2), count_chars),
    nf!("dirname", 1, Some(2), dirname),
    nf!("basename", 1, Some(2), basename),
    nf!("pathinfo", 1, Some(2), pathinfo),
    nf!("levenshtein", 2, Some(5), levenshtein),
    nf_ref!("similar_text", 2, Some(3), 0b100, similar_text),
    nf!("soundex", 1, Some(1), soundex),
    nf!("metaphone", 1, Some(2), metaphone),
    nf!("str_getcsv", 1, Some(4), str_getcsv),
    nf!("str_rot13", 1, Some(1), str_rot13),
    nf!("strcoll", 2, Some(2), strcoll),
    nf!("strspn", 2, Some(4), strspn),
    nf!("strcspn", 2, Some(4), strcspn),
    nf!("strip_tags", 1, Some(2), strip_tags),
    nf!("strnatcmp", 2, Some(2), strnatcmp),
    nf!("strnatcasecmp", 2, Some(2), strnatcasecmp),
    nf!("strtok", 1, Some(2), strtok),
    nf!("substr_compare", 3, Some(5), substr_compare),
    nf!("utf8_encode", 1, Some(1), utf8_encode),
    nf!("utf8_decode", 1, Some(1), utf8_decode),
    nf!("wordwrap", 1, Some(4), wordwrap),
    // `sscanf($string, $format, &...$vars)`: every position from #2 on is
    // an out-parameter.
    nf_ref!("sscanf", 2, None, 0xFFFF_FFFC, sscanf),
    nf!("setlocale", 2, None, setlocale),
    nf!("localeconv", 0, Some(0), localeconv),
];

/// The string-extension constants (values from `manifest/php-8.5.0/constants.json`).
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("STR_PAD_LEFT", 0),
        ("STR_PAD_RIGHT", 1),
        ("STR_PAD_BOTH", 2),
        ("PATHINFO_DIRNAME", 1),
        ("PATHINFO_BASENAME", 2),
        ("PATHINFO_EXTENSION", 4),
        ("PATHINFO_FILENAME", 8),
        ("PATHINFO_ALL", 15),
        ("CHAR_MAX", 127),
        ("LC_CTYPE", 2),
        ("LC_NUMERIC", 4),
        ("LC_TIME", 5),
        ("LC_COLLATE", 1),
        ("LC_MONETARY", 3),
        ("LC_ALL", 0),
        ("LC_MESSAGES", 6),
        ("CRYPT_SALT_LENGTH", 123),
        ("CRYPT_STD_DES", 1),
        ("CRYPT_EXT_DES", 1),
        ("CRYPT_MD5", 1),
        ("CRYPT_BLOWFISH", 1),
        ("CRYPT_SHA256", 1),
        ("CRYPT_SHA512", 1),
        ("ENT_HTML401", 0),
        ("ENT_XML1", 16),
        ("ENT_XHTML", 32),
        ("ENT_HTML5", 48),
        ("ENT_COMPAT", 2),
        ("ENT_QUOTES", 3),
        ("ENT_NOQUOTES", 0),
        ("ENT_IGNORE", 4),
        ("ENT_SUBSTITUTE", 8),
        ("ENT_DISALLOWED", 128),
        ("HTML_SPECIALCHARS", 0),
        ("HTML_ENTITIES", 1),
    ] {
        r.constant(name, Value::Int(v));
    }
}

fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

/// C-locale `isspace`: space, `\t`, `\n`, `\v`, `\f`, `\r`.
pub(crate) fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

// ---- C-style slashes ----------------------------------------------------------

/// `addcslashes(string $string, string $characters): string`.
pub(crate) fn addcslashes(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let chars = bytes(&args[1]);
    if chars.is_empty() {
        return Ok(str_value(s));
    }
    let mask = charmask(ctx, "addcslashes", &chars)?;
    let mut out = Vec::with_capacity(s.len());
    for &c in &s {
        if !mask[c as usize] {
            out.push(c);
            continue;
        }
        out.push(b'\\');
        if !(32..127).contains(&c) {
            match c {
                b'\n' => out.push(b'n'),
                b'\t' => out.push(b't'),
                b'\r' => out.push(b'r'),
                0x07 => out.push(b'a'),
                0x0b => out.push(b'v'),
                0x08 => out.push(b'b'),
                0x0c => out.push(b'f'),
                _ => out.extend_from_slice(format!("{:03o}", c).as_bytes()),
            }
        } else {
            out.push(c);
        }
    }
    Ok(str_value(out))
}

/// `stripcslashes(string $string): string` — decodes `\a \b \f \n \r \t \v
/// \\`, `\xHH` and up to three octal digits; any other escaped byte is itself.
pub(crate) fn stripcslashes(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] != b'\\' || i + 1 >= s.len() {
            out.push(s[i]);
            i += 1;
            continue;
        }
        i += 1;
        match s[i] {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(0x07),
            b'v' => out.push(0x0b),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'x' if i + 1 < s.len() && s[i + 1].is_ascii_hexdigit() => {
                let mut v: u32 = 0;
                let mut n = 0;
                while n < 2 && i + 1 < s.len() && s[i + 1].is_ascii_hexdigit() {
                    v = v * 16 + (s[i + 1] as char).to_digit(16).unwrap();
                    i += 1;
                    n += 1;
                }
                out.push(v as u8);
            }
            b'0'..=b'7' => {
                let mut v: u32 = 0;
                let mut n = 0;
                while n < 3 && i < s.len() && (b'0'..=b'7').contains(&s[i]) {
                    v = v * 8 + (s[i] - b'0') as u32;
                    i += 1;
                    n += 1;
                }
                out.push(v as u8);
                continue;
            }
            other => out.push(other),
        }
        i += 1;
    }
    Ok(str_value(out))
}

// ---- chunking / counting ------------------------------------------------------

/// `chunk_split(string $string, int $length = 76, string $separator = "\r\n"): string`.
pub(crate) fn chunk_split(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let len = args.get(1).map_or(76, Value::to_int);
    let sep = args.get(2).map_or_else(|| b"\r\n".to_vec(), bytes);
    if len < 1 {
        return Err(Unwind::value_error(
            "chunk_split(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let len = len as usize;
    if len >= s.len() {
        let mut out = s;
        out.extend_from_slice(&sep);
        return Ok(str_value(out));
    }
    let mut out = Vec::with_capacity(s.len() + sep.len() * (s.len() / len + 1));
    for chunk in s.chunks(len) {
        out.extend_from_slice(chunk);
        out.extend_from_slice(&sep);
    }
    Ok(str_value(out))
}

/// `count_chars(string $string, int $mode = 0): array|string`.
pub(crate) fn count_chars(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let mode = args.get(1).map_or(0, Value::to_int);
    if !(0..=4).contains(&mode) {
        return Err(Unwind::value_error(
            "count_chars(): Argument #2 ($mode) must be between 0 and 4 (inclusive)",
        ));
    }
    let mut counts = [0i64; 256];
    for &b in &s {
        counts[b as usize] += 1;
    }
    if mode < 3 {
        let mut out = Array::new();
        for (b, &n) in counts.iter().enumerate() {
            if mode == 0 || (mode == 1 && n > 0) || (mode == 2 && n == 0) {
                out.set(ArrayKey::Int(b as i64), Value::Int(n));
            }
        }
        return Ok(Value::Array(out));
    }
    let out: Vec<u8> = counts
        .iter()
        .enumerate()
        .filter(|(_, &n)| if mode == 3 { n > 0 } else { n == 0 })
        .map(|(b, _)| b as u8)
        .collect();
    Ok(str_value(out))
}

// ---- paths -----------------------------------------------------------------

/// One `dirname` step (php's `zend_dirname`, `/` separators).
fn dirname_once(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut end = path.len();
    // Strip trailing slashes.
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return b"/".to_vec();
    }
    // Strip the file name.
    while end > 0 && path[end - 1] != b'/' {
        end -= 1;
    }
    if end == 0 {
        return b".".to_vec();
    }
    // Strip the slashes before it.
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return b"/".to_vec();
    }
    path[..end].to_vec()
}

/// `dirname(string $path, int $levels = 1): string`.
pub(crate) fn dirname(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut path = bytes(&args[0]);
    let levels = args.get(1).map_or(1, Value::to_int);
    if levels < 1 {
        return Err(Unwind::value_error(
            "dirname(): Argument #2 ($levels) must be greater than or equal to 1",
        ));
    }
    for _ in 0..levels {
        let next = dirname_once(&path);
        if next == path {
            break;
        }
        path = next;
    }
    Ok(str_value(path))
}

/// The last path component (php's `php_basename` with `/` separators), before
/// any suffix stripping.
fn basename_bytes(path: &[u8]) -> Vec<u8> {
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && path[start - 1] != b'/' {
        start -= 1;
    }
    path[start..end].to_vec()
}

/// `basename(string $path, string $suffix = ""): string`.
pub(crate) fn basename(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let path = bytes(&args[0]);
    let suffix = args.get(1).map_or_else(Vec::new, bytes);
    let mut base = basename_bytes(&path);
    // The suffix is only removed when it is a proper (shorter) tail.
    if !suffix.is_empty() && suffix.len() < base.len() && base.ends_with(&suffix) {
        base.truncate(base.len() - suffix.len());
    }
    Ok(str_value(base))
}

/// `pathinfo(string $path, int $flags = PATHINFO_ALL): array|string`.
pub(crate) fn pathinfo(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let path = bytes(&args[0]);
    let flags = args.get(1).map_or(15, Value::to_int);
    let mut out = Array::new();
    if flags & 1 != 0 {
        let d = dirname_once(&path);
        if !d.is_empty() {
            out.set(ArrayKey::str(b"dirname"), str_value(d));
        }
    }
    let base = basename_bytes(&path);
    if flags & 2 != 0 {
        out.set(ArrayKey::str(b"basename"), str_value(base.clone()));
    }
    let dot = base.iter().rposition(|&b| b == b'.');
    if flags & 4 != 0 {
        if let Some(p) = dot {
            out.set(ArrayKey::str(b"extension"), str_value(base[p + 1..].to_vec()));
        }
    }
    if flags & 8 != 0 {
        let stem = match dot {
            Some(p) => base[..p].to_vec(),
            None => base.clone(),
        };
        out.set(ArrayKey::str(b"filename"), str_value(stem));
    }
    if flags == 15 {
        return Ok(Value::Array(out));
    }
    // A single (or partial) request returns the first element built, or "".
    Ok(out.first().map_or_else(|| str_value(Vec::new()), |(_, v)| v.clone()))
}

// ---- distance / phonetics ------------------------------------------------------

/// `levenshtein(string $string1, string $string2, int $insertion_cost = 1,
/// int $replacement_cost = 1, int $deletion_cost = 1): int`.
pub(crate) fn levenshtein(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let a = bytes(&args[0]);
    let b = bytes(&args[1]);
    let cost_ins = args.get(2).map_or(1, Value::to_int);
    let cost_rep = args.get(3).map_or(1, Value::to_int);
    let cost_del = args.get(4).map_or(1, Value::to_int);
    if a.is_empty() {
        return Ok(Value::Int(b.len() as i64 * cost_ins));
    }
    if b.is_empty() {
        return Ok(Value::Int(a.len() as i64 * cost_del));
    }
    let mut p1: Vec<i64> = (0..=b.len() as i64).map(|i| i * cost_ins).collect();
    let mut p2 = vec![0i64; b.len() + 1];
    for &ca in &a {
        p2[0] = p1[0] + cost_del;
        for (j, &cb) in b.iter().enumerate() {
            let mut c0 = p1[j] + if ca == cb { 0 } else { cost_rep };
            let c1 = p1[j + 1] + cost_del;
            if c1 < c0 {
                c0 = c1;
            }
            let c2 = p2[j] + cost_ins;
            if c2 < c0 {
                c0 = c2;
            }
            p2[j + 1] = c0;
        }
        std::mem::swap(&mut p1, &mut p2);
    }
    Ok(Value::Int(p1[b.len()]))
}

/// The first longest common substring of `a` and `b`: (pos in a, pos in b,
/// length, number of times the running maximum improved).
fn similar_str(a: &[u8], b: &[u8]) -> (usize, usize, usize, usize) {
    let (mut pos1, mut pos2, mut max, mut count) = (0, 0, 0, 0);
    for p in 0..a.len() {
        for q in 0..b.len() {
            let mut l = 0;
            while p + l < a.len() && q + l < b.len() && a[p + l] == b[q + l] {
                l += 1;
            }
            if l > max {
                max = l;
                count += 1;
                pos1 = p;
                pos2 = q;
            }
        }
    }
    (pos1, pos2, max, count)
}

/// php's `php_similar_char`: the longest common substring plus the similarity
/// of what lies to its left and right (the left side only when the maximum
/// was reached more than once, a quirk kept for exactness).
fn similar_char(a: &[u8], b: &[u8]) -> usize {
    let (pos1, pos2, max, count) = similar_str(a, b);
    if max == 0 {
        return 0;
    }
    let mut sum = max;
    if pos1 > 0 && pos2 > 0 && count > 1 {
        sum += similar_char(&a[..pos1], &b[..pos2]);
    }
    if pos1 + max < a.len() && pos2 + max < b.len() {
        sum += similar_char(&a[pos1 + max..], &b[pos2 + max..]);
    }
    sum
}

/// `similar_text(string $string1, string $string2, float &$percent = null): int`.
pub(crate) fn similar_text(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let a = bytes(&args[0]);
    let b = bytes(&args[1]);
    let sim = if a.is_empty() && b.is_empty() { 0 } else { similar_char(&a, &b) };
    if args.len() > 2 {
        let pct = if a.is_empty() && b.is_empty() {
            0.0
        } else {
            sim as f64 * 2.0 * 100.0 / (a.len() + b.len()) as f64
        };
        args[2] = Value::Float(pct);
    }
    Ok(Value::Int(sim as i64))
}

/// `soundex(string $string): string`.
pub(crate) fn soundex(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const TABLE: [u8; 26] = [
        0, b'1', b'2', b'3', 0, b'1', b'2', 0, 0, b'2', b'2', b'4', b'5', b'5', 0, b'1', b'2',
        b'6', b'2', b'3', 0, b'1', 0, b'2', 0, b'2',
    ];
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(4);
    let mut last = 0xffu8;
    for &c in &s {
        if out.len() >= 4 {
            break;
        }
        let code = c.to_ascii_uppercase();
        if !code.is_ascii_uppercase() {
            continue;
        }
        if out.is_empty() {
            out.push(code);
            last = TABLE[(code - b'A') as usize];
        } else {
            let code = TABLE[(code - b'A') as usize];
            if code != last {
                if code != 0 {
                    out.push(code);
                }
                last = code;
            }
        }
    }
    while out.len() < 4 {
        out.push(b'0');
    }
    Ok(str_value(out))
}

/// `metaphone(string $string, int $max_phonemes = 0): string` — php's
/// (traditional) metaphone: `X` stands for "sh", `0` for "th".
pub(crate) fn metaphone(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let word = bytes(&args[0]);
    let max = args.get(1).map_or(0, Value::to_int);
    if max < 0 {
        return Err(Unwind::value_error(
            "metaphone(): Argument #2 ($max_phonemes) must be greater than or equal to 0",
        ));
    }
    Ok(str_value(metaphone_impl(&word, max as usize)))
}

fn metaphone_impl(word: &[u8], max: usize) -> Vec<u8> {
    // Per-letter classes: vowel, no-change, affects-H, makes-soft, no-GH-to-F.
    const CODES: [u8; 26] = [
        1, 16, 4, 16, 9, 2, 4, 16, 9, 2, 0, 2, 2, 2, 1, 4, 0, 2, 4, 4, 1, 0, 0, 0, 8, 0,
    ];
    let encode = |c: u8| -> u8 {
        if c.is_ascii_alphabetic() {
            CODES[(c.to_ascii_uppercase() - b'A') as usize]
        } else {
            0
        }
    };
    let is_vowel = |c: u8| encode(c) & 1 != 0;
    let affect_h = |c: u8| encode(c) & 4 != 0;
    let make_soft = |c: u8| encode(c) & 8 != 0;
    let no_gh_to_f = |c: u8| encode(c) & 16 != 0;
    // Letters at an offset from the cursor, uppercased; 0 past either end.
    let at = |i: isize| -> u8 {
        if i < 0 || i as usize >= word.len() {
            0
        } else {
            word[i as usize].to_ascii_uppercase()
        }
    };
    let mut out: Vec<u8> = Vec::new();
    let mut w: isize = 0;
    // Skip leading non-letters.
    while (w as usize) < word.len() && !word[w as usize].is_ascii_alphabetic() {
        w += 1;
    }
    if w as usize >= word.len() {
        return out;
    }
    // Prefix rules.
    match at(w) {
        b'A' => {
            if at(w + 1) == b'E' {
                out.push(b'E');
                w += 2;
            } else {
                out.push(b'A');
                w += 1;
            }
        }
        b'G' | b'K' | b'P' => {
            if at(w + 1) == b'N' {
                out.push(b'N');
                w += 2;
            }
        }
        b'W' => {
            if at(w + 1) == b'R' {
                out.push(b'R');
                w += 2;
            } else if at(w + 1) == b'H' || is_vowel(at(w + 1)) {
                out.push(b'W');
                w += 2;
            }
        }
        b'X' => {
            out.push(b'S');
            w += 1;
        }
        b'E' | b'I' | b'O' | b'U' => {
            out.push(at(w));
            w += 1;
        }
        _ => {}
    }
    while at(w) != 0 && (max == 0 || out.len() < max) {
        let cur = at(w);
        let mut skip = 0;
        if !cur.is_ascii_alphabetic() || (cur == at(w - 1) && cur != b'C') {
            w += 1;
            continue;
        }
        let next = at(w + 1);
        let after = if next != 0 { at(w + 2) } else { 0 };
        let prev = at(w - 1);
        match cur {
            b'B' => {
                if prev != b'M' {
                    out.push(b'B');
                }
            }
            b'C' => {
                if make_soft(next) {
                    if after == b'A' && next == b'I' {
                        out.push(b'X');
                    } else if prev != b'S' {
                        out.push(b'S');
                    }
                } else if next == b'H' {
                    out.push(b'X');
                    skip += 1;
                } else {
                    out.push(b'K');
                }
            }
            b'D' => {
                if next == b'G' && make_soft(after) {
                    out.push(b'J');
                    skip += 1;
                } else {
                    out.push(b'T');
                }
            }
            b'G' => {
                if next == b'H' {
                    if !(no_gh_to_f(at(w - 3)) || at(w - 4) == b'H') {
                        out.push(b'F');
                        skip += 1;
                    }
                } else if next == b'N' {
                    if !(!after.is_ascii_alphabetic() || (after == b'E' && at(w + 3) == b'D')) {
                        out.push(b'K');
                    }
                } else if make_soft(next) && prev != b'G' {
                    out.push(b'J');
                } else {
                    out.push(b'K');
                }
            }
            b'H' => {
                if is_vowel(next) && !affect_h(prev) {
                    out.push(b'H');
                }
            }
            b'K' => {
                if prev != b'C' {
                    out.push(b'K');
                }
            }
            b'P' => out.push(if next == b'H' { b'F' } else { b'P' }),
            b'Q' => out.push(b'K'),
            b'S' => {
                if next == b'I' && (after == b'O' || after == b'A') {
                    out.push(b'X');
                } else if next == b'H' {
                    out.push(b'X');
                    skip += 1;
                } else {
                    out.push(b'S');
                }
            }
            b'T' => {
                if next == b'I' && (after == b'O' || after == b'A') {
                    out.push(b'X');
                } else if next == b'H' {
                    out.push(b'0');
                    skip += 1;
                } else if !(next == b'C' && after == b'H') {
                    out.push(b'T');
                }
            }
            b'V' => out.push(b'F'),
            b'W' => {
                if is_vowel(next) {
                    out.push(b'W');
                }
            }
            b'X' => {
                out.push(b'K');
                out.push(b'S');
            }
            b'Y' => {
                if is_vowel(next) {
                    out.push(b'Y');
                }
            }
            b'Z' => out.push(b'S'),
            b'F' | b'J' | b'L' | b'M' | b'N' | b'R' => out.push(cur),
            _ => {}
        }
        w += 1 + skip;
    }
    out
}

// ---- rot13 / collation / spans ---------------------------------------------------

/// `str_rot13(string $string): string`.
pub(crate) fn str_rot13(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let out: Vec<u8> = bytes(&args[0])
        .into_iter()
        .map(|c| match c {
            b'a'..=b'z' => (c - b'a' + 13) % 26 + b'a',
            b'A'..=b'Z' => (c - b'A' + 13) % 26 + b'A',
            _ => c,
        })
        .collect();
    Ok(str_value(out))
}

/// `strcoll(string $string1, string $string2): int` — the C locale, so a
/// plain unsigned byte comparison (the difference of the first mismatch).
pub(crate) fn strcoll(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let a = bytes(&args[0]);
    let b = bytes(&args[1]);
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return Ok(Value::Int(a[i] as i64 - b[i] as i64));
        }
    }
    Ok(Value::Int(a[n..].first().map_or(0, |&c| c as i64) - b[n..].first().map_or(0, |&c| c as i64)))
}

/// Shared window resolution of `strspn`/`strcspn`: the `[offset, offset+len)`
/// slice of the subject, or `None` when it is empty.
fn spn_window(args: &[Value]) -> Option<(Vec<u8>, usize, usize)> {
    let s = bytes(&args[0]);
    let n = s.len() as i64;
    let mut offset = args.get(2).map_or(0, Value::to_int);
    if offset < 0 {
        offset = (offset + n).max(0);
    }
    if offset > n {
        return None;
    }
    let len = match args.get(3) {
        None | Some(Value::Null) => n - offset,
        Some(l) => {
            let l = l.to_int();
            if l < 0 {
                l.saturating_add(n - offset).max(0)
            } else {
                l.min(n - offset)
            }
        }
    };
    if len == 0 {
        return None;
    }
    Some((s, offset as usize, len as usize))
}

/// `strspn(string $string, string $characters, int $offset = 0, ?int $length = null): int`.
pub(crate) fn strspn(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some((s, off, len)) = spn_window(args) else {
        return Ok(Value::Int(0));
    };
    let set = bytes(&args[1]);
    let n = s[off..off + len].iter().take_while(|c| set.contains(c)).count();
    Ok(Value::Int(n as i64))
}

/// `strcspn(string $string, string $characters, int $offset = 0, ?int $length = null): int`.
pub(crate) fn strcspn(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some((s, off, len)) = spn_window(args) else {
        return Ok(Value::Int(0));
    };
    let set = bytes(&args[1]);
    let n = s[off..off + len].iter().take_while(|c| !set.contains(c)).count();
    Ok(Value::Int(n as i64))
}

// ---- natural order ---------------------------------------------------------

/// php's `strnatcmp_ex`: digit runs compare numerically (left-aligned when
/// either starts with `0`, otherwise by magnitude), runs of whitespace
/// collapse, leading zeros are skipped once at the very start.
pub(crate) fn strnatcmp_bytes(a: &[u8], b: &[u8], ci: bool) -> i64 {
    if a.is_empty() || b.is_empty() {
        return match a.len().cmp(&b.len()) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
        };
    }
    let at = |s: &[u8], i: usize| -> u8 { s.get(i).copied().unwrap_or(0) };
    let (mut ap, mut bp) = (0usize, 0usize);
    let mut leading = true;
    loop {
        let (mut ca, mut cb) = (at(a, ap), at(b, bp));
        while leading && ca == b'0' && ap + 1 < a.len() && a[ap + 1].is_ascii_digit() {
            ap += 1;
            ca = a[ap];
        }
        while leading && cb == b'0' && bp + 1 < b.len() && b[bp + 1].is_ascii_digit() {
            bp += 1;
            cb = b[bp];
        }
        leading = false;
        while is_c_space(ca) {
            ap += 1;
            ca = at(a, ap);
        }
        while is_c_space(cb) {
            bp += 1;
            cb = at(b, bp);
        }
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let fractional = ca == b'0' || cb == b'0';
            let result = if fractional {
                compare_left(a, &mut ap, b, &mut bp)
            } else {
                compare_right(a, &mut ap, b, &mut bp)
            };
            if result != 0 {
                return result;
            }
            if ap >= a.len() && bp >= b.len() {
                return 0;
            }
            if ap >= a.len() {
                return -1;
            }
            if bp >= b.len() {
                return 1;
            }
            ca = a[ap];
            cb = b[bp];
        }
        if ci {
            ca = ca.to_ascii_uppercase();
            cb = cb.to_ascii_uppercase();
        }
        if ca < cb {
            return -1;
        }
        if ca > cb {
            return 1;
        }
        ap += 1;
        bp += 1;
        if ap >= a.len() && bp >= b.len() {
            return 0;
        }
        if ap >= a.len() {
            return -1;
        }
        if bp >= b.len() {
            return 1;
        }
    }
}

/// Compare two right-aligned digit runs: the longer run wins, then the first
/// differing digit.
fn compare_right(a: &[u8], ap: &mut usize, b: &[u8], bp: &mut usize) -> i64 {
    let mut bias = 0;
    loop {
        let da = a.get(*ap).filter(|c| c.is_ascii_digit());
        let db = b.get(*bp).filter(|c| c.is_ascii_digit());
        match (da, db) {
            (None, None) => return bias,
            (None, Some(_)) => return -1,
            (Some(_), None) => return 1,
            (Some(x), Some(y)) => {
                if bias == 0 {
                    bias = match x.cmp(y) {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Greater => 1,
                        std::cmp::Ordering::Equal => 0,
                    };
                }
            }
        }
        *ap += 1;
        *bp += 1;
    }
}

/// Compare two left-aligned digit runs: the first differing digit wins.
fn compare_left(a: &[u8], ap: &mut usize, b: &[u8], bp: &mut usize) -> i64 {
    loop {
        let da = a.get(*ap).filter(|c| c.is_ascii_digit());
        let db = b.get(*bp).filter(|c| c.is_ascii_digit());
        match (da, db) {
            (None, None) => return 0,
            (None, Some(_)) => return -1,
            (Some(_), None) => return 1,
            (Some(x), Some(y)) => {
                if x < y {
                    return -1;
                }
                if x > y {
                    return 1;
                }
            }
        }
        *ap += 1;
        *bp += 1;
    }
}

/// `strnatcmp(string $string1, string $string2): int`.
pub(crate) fn strnatcmp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(strnatcmp_bytes(&bytes(&args[0]), &bytes(&args[1]), false)))
}

/// `strnatcasecmp(string $string1, string $string2): int`.
pub(crate) fn strnatcasecmp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(strnatcmp_bytes(&bytes(&args[0]), &bytes(&args[1]), true)))
}

// ---- strtok ----------------------------------------------------------------

thread_local! {
    /// The `strtok` cursor: the string being tokenized and the position of the
    /// next byte to consider. Per thread until `ExtState` grows a slot.
    static STRTOK: RefCell<Option<(Vec<u8>, usize)>> = const { RefCell::new(None) };
}

/// `strtok(string $string, ?string $token = null): string|false`.
pub(crate) fn strtok(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let token = if args.len() > 1 && !matches!(args[1], Value::Null) {
        let t = bytes(&args[1]);
        STRTOK.with(|s| *s.borrow_mut() = Some((bytes(&args[0]), 0)));
        t
    } else {
        bytes(&args[0])
    };
    if STRTOK.with(|s| s.borrow().is_none()) {
        ctx.warn("strtok(): Both arguments must be provided when starting tokenization")?;
        return Ok(Value::Bool(false));
    }
    STRTOK.with(|state| {
        let mut state = state.borrow_mut();
        let Some((s, pos)) = state.as_mut() else {
            return Ok(Value::Bool(false));
        };
        // A string already consumed to its end stays set (further calls are
        // false without the warning); running out while skipping delimiters
        // forgets it, as php does.
        if *pos >= s.len() {
            return Ok(Value::Bool(false));
        }
        let mut i = *pos;
        while i < s.len() && token.contains(&s[i]) {
            i += 1;
        }
        if i >= s.len() {
            *state = None;
            return Ok(Value::Bool(false));
        }
        let start = i;
        while i < s.len() && !token.contains(&s[i]) {
            i += 1;
        }
        let tok = s[start..i].to_vec();
        // The delimiter that ended the token is consumed.
        *pos = (i + 1).min(s.len());
        Ok(str_value(tok))
    })
}

// ---- substr_compare / utf8 ---------------------------------------------------

/// `substr_compare(string $haystack, string $needle, int $offset, ?int $length
/// = null, bool $case_insensitive = false): int`.
pub(crate) fn substr_compare(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let hay = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let mut offset = args[2].to_int();
    let ci = args.get(4).is_some_and(Value::to_bool);
    let n = hay.len() as i64;
    let length = match args.get(3) {
        None | Some(Value::Null) => None,
        Some(l) => {
            let l = l.to_int();
            if l < 0 {
                return Err(Unwind::value_error(
                    "substr_compare(): Argument #4 ($length) must be greater than or equal to 0",
                ));
            }
            Some(l)
        }
    };
    if offset < 0 {
        offset = n.saturating_add(offset).max(0);
    }
    if offset > n {
        return Err(Unwind::value_error(
            "substr_compare(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
        ));
    }
    let cmp_len = length.unwrap_or_else(|| (n - offset).max(needle.len() as i64)) as usize;
    let a = &hay[offset as usize..];
    let b = &needle[..];
    // Compare min(len) bytes (the first mismatch's difference), then the
    // lengths capped at `cmp_len` as -1/0/1.
    let m = a.len().min(b.len()).min(cmp_len);
    for i in 0..m {
        let (x, y) = if ci {
            (a[i].to_ascii_lowercase(), b[i].to_ascii_lowercase())
        } else {
            (a[i], b[i])
        };
        if x != y {
            return Ok(Value::Int(x as i64 - y as i64));
        }
    }
    let la = a.len().min(cmp_len) as i64;
    let lb = b.len().min(cmp_len) as i64;
    Ok(Value::Int((la - lb).signum()))
}

/// `utf8_encode(string $string): string` — Latin-1 to UTF-8 (deprecated since 8.2).
pub(crate) fn utf8_encode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated(
        "Function utf8_encode() is deprecated since 8.2, visit the php.net documentation for various alternatives",
    )?;
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(s.len());
    for &c in &s {
        if c < 0x80 {
            out.push(c);
        } else {
            out.push(0xc0 | (c >> 6));
            out.push(0x80 | (c & 0x3f));
        }
    }
    Ok(str_value(out))
}

/// `utf8_decode(string $string): string` — UTF-8 to Latin-1, `?` for anything
/// not representable or malformed (deprecated since 8.2).
pub(crate) fn utf8_decode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated(
        "Function utf8_decode() is deprecated since 8.2, visit the php.net documentation for various alternatives",
    )?;
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(s.len());
    let mut pos = 0;
    while pos < s.len() {
        match crate::html::next_utf8_char(&s, &mut pos) {
            Some(cp) if cp <= 0xff => out.push(cp as u8),
            _ => out.push(b'?'),
        }
    }
    Ok(str_value(out))
}

// ---- wordwrap ------------------------------------------------------------------

/// `wordwrap(string $string, int $width = 75, string $break = "\n", bool
/// $cut_long_words = false): string`.
pub(crate) fn wordwrap(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let text = bytes(&args[0]);
    let width = args.get(1).map_or(75, Value::to_int);
    let brk = args.get(2).map_or_else(|| b"\n".to_vec(), bytes);
    let cut = args.get(3).is_some_and(Value::to_bool);
    if brk.is_empty() {
        return Err(Unwind::value_error(
            "wordwrap(): Argument #3 ($break) must not be empty",
        ));
    }
    if width == 0 && cut {
        return Err(Unwind::value_error(
            "wordwrap(): Argument #4 ($cut_long_words) cannot be true when argument #2 ($width) is 0",
        ));
    }
    if text.is_empty() {
        return Ok(str_value(Vec::new()));
    }
    let n = text.len() as i64;
    if brk.len() == 1 && !cut {
        // Single-byte break, no cutting: rewrite spaces in place.
        let mut out = text.clone();
        let (mut laststart, mut lastspace) = (0i64, 0i64);
        for current in 0..n {
            let c = text[current as usize];
            if c == brk[0] {
                laststart = current + 1;
                lastspace = laststart;
            } else if c == b' ' {
                if current - laststart >= width {
                    out[current as usize] = brk[0];
                    laststart = current + 1;
                }
                lastspace = current;
            } else if current - laststart >= width && laststart != lastspace {
                out[lastspace as usize] = brk[0];
                laststart = lastspace + 1;
            }
        }
        return Ok(str_value(out));
    }
    let mut out = Vec::with_capacity(text.len() + text.len() / 8);
    let (mut laststart, mut lastspace) = (0i64, 0i64);
    let mut current = 0i64;
    while current < n {
        let c = text[current as usize];
        let cu = current as usize;
        // An existing break resets the line (only when it is not at the very end).
        if c == brk[0]
            && cu + brk.len() < text.len()
            && text[cu..cu + brk.len()] == brk[..]
        {
            out.extend_from_slice(&text[laststart as usize..cu + brk.len()]);
            current += brk.len() as i64 - 1;
            laststart = current + 1;
            lastspace = laststart;
        } else if c == b' ' {
            if current - laststart >= width {
                out.extend_from_slice(&text[laststart as usize..cu]);
                out.extend_from_slice(&brk);
                laststart = current + 1;
            }
            lastspace = current;
        } else if current - laststart >= width && cut && laststart >= lastspace {
            out.extend_from_slice(&text[laststart as usize..cu]);
            out.extend_from_slice(&brk);
            laststart = current;
            lastspace = current;
        } else if current - laststart >= width && laststart < lastspace {
            out.extend_from_slice(&text[laststart as usize..lastspace as usize]);
            out.extend_from_slice(&brk);
            laststart = lastspace + 1;
            lastspace = laststart;
        }
        current += 1;
    }
    if laststart != current {
        out.extend_from_slice(&text[laststart as usize..current as usize]);
    }
    Ok(str_value(out))
}

// ---- locale (C only) -------------------------------------------------------------

thread_local! {
    /// The C-locale model's per-category names, indexed by `LC_*` (1..=6):
    /// php starts with `LC_CTYPE` at `C.UTF-8` and everything else at `C`.
    static LOCALE: RefCell<[String; 7]> = RefCell::new([
        String::from("C"),
        String::from("C"),
        String::from("C.UTF-8"),
        String::from("C"),
        String::from("C"),
        String::from("C"),
        String::from("C"),
    ]);
}

/// `setlocale(int $category, $locales, ...$rest): string|false` — only the C
/// locale exists here: `"C"`, `"POSIX"`, `"C.UTF-8"`, `""`/`null` (the
/// harness environment's `LC_ALL=C`) and the `"0"` query succeed; any other
/// (system) locale name is `false`. A mixed `LC_ALL` query renders the
/// macOS composite form the oracle prints (`C/C.UTF-8/C/C/C/C`).
pub(crate) fn setlocale(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let category = args[0].to_int();
    if !(0..=6).contains(&category) {
        return Err(Unwind::value_error(
            "setlocale(): Argument #1 ($category) must be one of LC_ALL, LC_COLLATE, LC_CTYPE, LC_MONETARY, LC_NUMERIC, LC_TIME, or LC_MESSAGES",
        ));
    }
    let mut candidates: Vec<Value> = Vec::new();
    for a in &args[1..] {
        match &*a.deref() {
            Value::Array(arr) => candidates.extend(arr.iter().map(|(_, v)| v.deref().into_owned())),
            v => candidates.push(v.clone()),
        }
    }
    // LC_ALL's composite order on this platform.
    const COMPOSITE: [usize; 6] = [1, 2, 3, 4, 5, 6];
    let query = |names: &[String; 7]| -> String {
        if category != 0 {
            return names[category as usize].clone();
        }
        let first = &names[1];
        if COMPOSITE.iter().all(|&i| names[i] == *first) {
            first.clone()
        } else {
            COMPOSITE.iter().map(|&i| names[i].as_str()).collect::<Vec<_>>().join("/")
        }
    };
    LOCALE.with(|state| {
        let mut names = state.borrow_mut();
        for c in candidates {
            // `null` coerces to "" (the environment's locale, `C` here).
            let name = c.to_php_bytes();
            let set_to = match name.as_slice() {
                b"0" => return Ok(str_value(query(&names).into_bytes())),
                b"" | b"C" => "C",
                b"POSIX" => "POSIX",
                b"C.UTF-8" => "C.UTF-8",
                _ => continue,
            };
            if category == 0 {
                for n in names.iter_mut().skip(1) {
                    *n = set_to.to_string();
                }
            } else {
                names[category as usize] = set_to.to_string();
            }
            return Ok(str_value(set_to.as_bytes().to_vec()));
        }
        Ok(Value::Bool(false))
    })
}

/// `localeconv(): array` — the C locale's numeric and monetary formatting.
pub(crate) fn localeconv(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    let s = |v: &[u8]| Value::Str(Str::new(v));
    out.set(ArrayKey::str(b"decimal_point"), s(b"."));
    for k in [
        "thousands_sep",
        "int_curr_symbol",
        "currency_symbol",
        "mon_decimal_point",
        "mon_thousands_sep",
        "positive_sign",
        "negative_sign",
    ] {
        out.set(ArrayKey::str(k.as_bytes()), s(b""));
    }
    for k in [
        "int_frac_digits",
        "frac_digits",
        "p_cs_precedes",
        "p_sep_by_space",
        "n_cs_precedes",
        "n_sep_by_space",
        "p_sign_posn",
        "n_sign_posn",
    ] {
        out.set(ArrayKey::str(k.as_bytes()), Value::Int(127));
    }
    out.set(ArrayKey::str(b"grouping"), Value::empty_array());
    out.set(ArrayKey::str(b"mon_grouping"), Value::empty_array());
    Ok(Value::Array(out))
}

// ---- str_getcsv --------------------------------------------------------------

/// The end of `buf` without a trailing `\n`, `\r` or `\r\n` (php's
/// `php_fgetcsv_lookup_trailing_spaces`).
fn csv_trim_line_end(buf: &[u8]) -> usize {
    match buf {
        [.., b'\r', b'\n'] => buf.len() - 2,
        [.., b'\n'] | [.., b'\r'] => buf.len() - 1,
        _ => buf.len(),
    }
}

/// php's `php_fgetcsv` for an in-memory line: `None` for the escape means
/// "no escape character".
pub(crate) fn parse_csv_line(buf: &[u8], delim: u8, enc: u8, escape: Option<u8>) -> Array {
    let mut out = Array::new();
    let limit = csv_trim_line_end(buf);
    let line_end = &buf[limit..];
    let mut bptr = 0usize;
    let mut first_field = true;
    loop {
        let mut temp: Vec<u8> = Vec::new();
        // Leading whitespace is skipped only in front of an enclosure.
        if bptr < limit {
            let mut tmp = bptr;
            while tmp < limit && buf[tmp] != delim && is_c_space(buf[tmp]) {
                tmp += 1;
            }
            if tmp < limit && buf[tmp] == enc {
                bptr = tmp;
            }
        }
        if first_field && bptr == limit {
            out.push(Value::Null);
            break;
        }
        first_field = false;
        let inc_len = if bptr < limit { 1 } else { 0 };
        if inc_len != 0 && buf[bptr] == enc {
            // 2A. An enclosure-delimited field.
            bptr += 1;
            let mut hunk_begin = bptr;
            let mut state = 0u8;
            loop {
                if bptr >= limit {
                    match state {
                        2 => {
                            temp.extend_from_slice(&buf[hunk_begin..bptr - 1]);
                            hunk_begin = bptr;
                        }
                        _ => {
                            if state == 1 || hunk_begin != limit {
                                temp.extend_from_slice(&buf[hunk_begin..bptr]);
                                hunk_begin = bptr;
                            }
                            // An unterminated enclosure keeps the line ending.
                            temp.extend_from_slice(line_end);
                        }
                    }
                    break;
                }
                match state {
                    1 => {
                        bptr += 1;
                        state = 0;
                    }
                    2 => {
                        if buf[bptr] != enc {
                            temp.extend_from_slice(&buf[hunk_begin..bptr - 1]);
                            hunk_begin = bptr;
                            break;
                        }
                        temp.extend_from_slice(&buf[hunk_begin..bptr]);
                        bptr += 1;
                        hunk_begin = bptr;
                        state = 0;
                    }
                    _ => {
                        if buf[bptr] == enc {
                            state = 2;
                        } else if escape == Some(buf[bptr]) {
                            state = 1;
                        }
                        bptr += 1;
                    }
                }
            }
            // Everything up to the delimiter is appended verbatim.
            while bptr < limit && buf[bptr] != delim {
                bptr += 1;
            }
            temp.extend_from_slice(&buf[hunk_begin..bptr]);
            let more = bptr < limit;
            if more {
                bptr += 1;
            }
            out.push(str_value(temp));
            if !more {
                break;
            }
        } else {
            // 2B. A plain field: up to the delimiter, line ending stripped.
            let start = bptr;
            while bptr < limit && buf[bptr] != delim {
                bptr += 1;
            }
            temp.extend_from_slice(&buf[start..bptr]);
            let end = csv_trim_line_end(&temp);
            temp.truncate(end);
            let more = bptr < limit;
            if more {
                bptr += 1;
            }
            out.push(str_value(temp));
            if !more {
                break;
            }
        }
    }
    out
}

/// `str_getcsv(string $string, string $separator = ",", string $enclosure =
/// "\"", string $escape = "\\"): array`.
pub(crate) fn str_getcsv(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let delim = args.get(1).map_or_else(|| b",".to_vec(), bytes);
    let enc = args.get(2).map_or_else(|| b"\"".to_vec(), bytes);
    if delim.len() != 1 {
        return Err(Unwind::value_error(
            "str_getcsv(): Argument #2 ($separator) must be a single character",
        ));
    }
    if enc.len() != 1 {
        return Err(Unwind::value_error(
            "str_getcsv(): Argument #3 ($enclosure) must be a single character",
        ));
    }
    let escape = match args.get(3) {
        Some(e) => {
            let e = bytes(e);
            if e.len() > 1 {
                return Err(Unwind::value_error(
                    "str_getcsv(): Argument #4 ($escape) must be empty or a single character",
                ));
            }
            e.first().copied()
        }
        None => {
            ctx.deprecated(
                "str_getcsv(): the $escape parameter must be provided as its default value will change",
            )?;
            Some(b'\\')
        }
    };
    Ok(Value::Array(parse_csv_line(&s, delim[0], enc[0], escape)))
}

// ---- strip_tags ----------------------------------------------------------------

/// php's `php_tag_find`: normalize the tag found in the input to `<name>`
/// (lowercased, attributes and the closing slash dropped) and look it up in
/// the (lowercased) allow string.
fn tag_allowed(tag: &[u8], allow: &[u8]) -> bool {
    if tag.is_empty() {
        return false;
    }
    let mut norm: Vec<u8> = Vec::with_capacity(tag.len() + 1);
    let mut state = 0;
    let mut i = 0;
    while i < tag.len() {
        let c = tag[i].to_ascii_lowercase();
        match c {
            b'<' => norm.push(c),
            b'>' => break,
            _ => {
                if !is_c_space(c) {
                    state = 1;
                    let prev = if i > 0 { tag[i - 1] } else { 0 };
                    let next = tag.get(i + 1).copied().unwrap_or(0);
                    if c != b'/' || (prev != b'<' && next != b'>') {
                        norm.push(c);
                    }
                } else if state == 1 {
                    break;
                }
            }
        }
        i += 1;
    }
    norm.push(b'>');
    allow.windows(norm.len()).any(|w| w == norm.as_slice())
}

/// php's `php_strip_tags_ex`: the HTML/PHP/comment state machine.
pub(crate) fn strip_tags_bytes(buf: &[u8], allow: Option<&[u8]>) -> Vec<u8> {
    let at = |i: usize| -> u8 { buf.get(i).copied().unwrap_or(0) };
    let mut rp: Vec<u8> = Vec::with_capacity(buf.len());
    let mut tbuf: Vec<u8> = Vec::new();
    let mut p = 0usize;
    let mut depth = 0i32;
    let mut in_q: u8 = 0;
    let mut lc: u8 = 0;
    let mut br = 0i32;
    let mut is_xml = false;
    let mut state = 0u8;
    let end = buf.len();
    while p < end {
        let c = buf[p];
        match state {
            0 => {
                match c {
                    0 => {}
                    b'<' => {
                        if in_q != 0 {
                            // (unreachable: quotes only count inside a tag)
                        } else if is_c_space(at(p + 1)) {
                            rp.push(c);
                        } else {
                            lc = b'<';
                            state = 1;
                            if allow.is_some() {
                                tbuf.push(b'<');
                            }
                        }
                    }
                    b'>' => {
                        if depth != 0 {
                            depth -= 1;
                        } else if in_q == 0 {
                            rp.push(c);
                        }
                    }
                    _ => rp.push(c),
                }
                p += 1;
            }
            1 => {
                let mut reg_char = false;
                match c {
                    0 => {}
                    b'<' => {
                        if in_q != 0 {
                        } else if is_c_space(at(p + 1)) {
                            reg_char = true;
                        } else {
                            depth += 1;
                        }
                    }
                    b'>' => {
                        if depth != 0 {
                            depth -= 1;
                        } else if in_q != 0 {
                        } else {
                            lc = b'>';
                            if is_xml && p >= 1 && buf[p - 1] == b'-' {
                                // inside "<?xml ... -->": keep scanning
                            } else {
                                in_q = 0;
                                state = 0;
                                is_xml = false;
                                if let Some(allow) = allow {
                                    tbuf.push(b'>');
                                    if tag_allowed(&tbuf, allow) {
                                        rp.extend_from_slice(&tbuf);
                                    }
                                    tbuf.clear();
                                }
                                p += 1;
                                continue;
                            }
                        }
                    }
                    b'"' | b'\'' => {
                        if p != 0 && (in_q == 0 || c == in_q) {
                            in_q = if in_q != 0 { 0 } else { c };
                        }
                        reg_char = true;
                    }
                    b'!' => {
                        if p >= 1 && buf[p - 1] == b'<' {
                            state = 3;
                            lc = c;
                            p += 1;
                            continue;
                        }
                        reg_char = true;
                    }
                    b'?' => {
                        if p >= 1 && buf[p - 1] == b'<' {
                            br = 0;
                            state = 2;
                            p += 1;
                            continue;
                        }
                        reg_char = true;
                    }
                    _ => reg_char = true,
                }
                if reg_char && allow.is_some() {
                    tbuf.push(c);
                }
                p += 1;
            }
            2 => {
                match c {
                    b'(' => {
                        if lc != b'"' && lc != b'\'' {
                            lc = b'(';
                            br += 1;
                        }
                    }
                    b')' => {
                        if lc != b'"' && lc != b'\'' {
                            lc = b')';
                            br -= 1;
                        }
                    }
                    b'>' => {
                        if depth != 0 {
                            depth -= 1;
                        } else if in_q != 0 {
                        } else if br == 0 && p >= 1 && lc != b'"' && buf[p - 1] == b'?' {
                            in_q = 0;
                            state = 0;
                            tbuf.clear();
                            p += 1;
                            continue;
                        }
                    }
                    b'"' | b'\'' => {
                        if p >= 1 && buf[p - 1] != b'\\' {
                            if lc == c {
                                lc = 0;
                            } else if lc != b'\\' {
                                lc = c;
                            }
                            if p != 0 && (in_q == 0 || c == in_q) {
                                in_q = if in_q != 0 { 0 } else { c };
                            }
                        }
                    }
                    // "<?xml" is not PHP: back to the tag state.
                    b'l' | b'L'
                        if p > 4
                            && buf[p - 1].eq_ignore_ascii_case(&b'm')
                            && buf[p - 2].eq_ignore_ascii_case(&b'x')
                            && buf[p - 3] == b'?'
                            && buf[p - 4] == b'<' =>
                    {
                        state = 1;
                        is_xml = true;
                        p += 1;
                        continue;
                    }
                    _ => {}
                }
                p += 1;
            }
            3 => {
                match c {
                    b'>' => {
                        if depth != 0 {
                            depth -= 1;
                        } else if in_q != 0 {
                        } else {
                            in_q = 0;
                            state = 0;
                            tbuf.clear();
                            p += 1;
                            continue;
                        }
                    }
                    b'"' | b'\'' => {
                        if p != 0 && buf[p - 1] != b'\\' && (in_q == 0 || c == in_q) {
                            in_q = if in_q != 0 { 0 } else { c };
                        }
                    }
                    b'-' => {
                        if p >= 2 && buf[p - 1] == b'-' && buf[p - 2] == b'!' {
                            state = 4;
                            p += 1;
                            continue;
                        }
                    }
                    // "<!DOCTYPE" is a tag, not a comment.
                    b'E' | b'e' if p > 6 && buf[p - 6..p].eq_ignore_ascii_case(b"doctyp") => {
                        state = 1;
                        p += 1;
                        continue;
                    }
                    _ => {}
                }
                p += 1;
            }
            _ => {
                // 4: inside "<!-- ... -->".
                if c == b'>' && in_q == 0 && p >= 2 && buf[p - 1] == b'-' && buf[p - 2] == b'-' {
                    in_q = 0;
                    state = 0;
                    tbuf.clear();
                }
                p += 1;
            }
        }
    }
    rp
}

/// `strip_tags(string $string, array|string|null $allowed_tags = null): string`.
pub(crate) fn strip_tags(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let allow: Option<Vec<u8>> = match args.get(1) {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => {
            // Each element becomes "<name>"; empty elements are dropped.
            let mut out = Vec::new();
            for (_, v) in a.iter() {
                let t = v.to_php_bytes();
                if t.is_empty() {
                    continue;
                }
                out.push(b'<');
                out.extend_from_slice(&t);
                out.push(b'>');
            }
            Some(out)
        }
        Some(v) => Some(bytes(v)),
    };
    let allow = allow.map(|a| a.to_ascii_lowercase());
    let allow = allow.as_deref().filter(|a| !a.is_empty());
    Ok(str_value(strip_tags_bytes(&s, allow)))
}

// ---- sscanf ----------------------------------------------------------------------

/// One parsed conversion of a scanf format (used by validation and scanning).
#[derive(Clone, Copy, PartialEq)]
enum ScanOp {
    Str,
    Set,
    Int,
    Float,
}

/// A `%[...]` character set.
struct CharSet {
    exclude: bool,
    chars: Vec<u8>,
    ranges: Vec<(u8, u8)>,
}

impl CharSet {
    fn contains(&self, c: u8) -> bool {
        let hit = self.chars.contains(&c) || self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi);
        hit != self.exclude
    }
}

/// Parse the set body starting after `[`, returning the set and the index of
/// the closing `]` (php's `BuildCharSet`).
fn build_char_set(fmt: &[u8], mut i: usize) -> (CharSet, usize) {
    let mut set = CharSet { exclude: false, chars: Vec::new(), ranges: Vec::new() };
    if fmt.get(i) == Some(&b'^') {
        set.exclude = true;
        i += 1;
    }
    if fmt.get(i) == Some(&b']') {
        set.chars.push(b']');
        i += 1;
    }
    let mut start = 0u8;
    while i < fmt.len() && fmt[i] != b']' {
        let c = fmt[i];
        if fmt.get(i + 1) == Some(&b'-') {
            start = c;
        } else if c == b'-' {
            if fmt.get(i + 1) == Some(&b']') {
                set.chars.push(start);
                set.chars.push(c);
            } else {
                i += 1;
                let e = fmt[i];
                if start < e {
                    set.ranges.push((start, e));
                } else {
                    set.ranges.push((e, start));
                }
            }
        } else {
            set.chars.push(c);
        }
        i += 1;
    }
    (set, i)
}

/// Validate a scanf format (php's `ValidateFormat`): returns the number of
/// result slots, or the `ValueError` php raises. `num_vars` is the count of
/// by-reference variables supplied.
fn validate_scan_format(fmt: &[u8], num_vars: usize) -> Result<usize, Unwind> {
    let mut i = 0;
    let mut got_xpg = false;
    let mut got_sequential = false;
    let mut xpg_size = 0usize;
    let mut obj_index = 0usize;
    let mut nassign: Vec<usize> = Vec::new();
    let bad = |c: u8| -> Unwind {
        // A NUL truncates php's message after the opening quote.
        let msg = if c == 0 {
            "Bad scan conversion character \"".to_string()
        } else {
            format!("Bad scan conversion character \"{}\"", c as char)
        };
        Unwind::value_error(msg)
    };
    while i < fmt.len() {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            i += 1;
            continue;
        }
        let mut suppress = false;
        if fmt.get(i) == Some(&b'*') {
            suppress = true;
            i += 1;
        } else if fmt.get(i).is_some_and(u8::is_ascii_digit) {
            let mut j = i;
            let mut value = 0usize;
            while j < fmt.len() && fmt[j].is_ascii_digit() {
                value = value.saturating_mul(10).saturating_add((fmt[j] - b'0') as usize);
                j += 1;
            }
            if fmt.get(j) == Some(&b'$') {
                i = j + 1;
                got_xpg = true;
                if got_sequential {
                    return Err(Unwind::value_error(
                        "cannot mix \"%\" and \"%n$\" conversion specifiers",
                    ));
                }
                if value == 0 || (value > num_vars && num_vars > 0) {
                    return Err(Unwind::value_error("\"%n$\" argument index out of range"));
                }
                obj_index = value - 1;
                if num_vars == 0 {
                    xpg_size = xpg_size.max(value);
                }
            } else {
                got_sequential = true;
            }
        } else {
            got_sequential = true;
        }
        if got_sequential && got_xpg {
            return Err(Unwind::value_error(
                "cannot mix \"%\" and \"%n$\" conversion specifiers",
            ));
        }
        // Width.
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            i += 1;
        }
        // Size modifier (ignored).
        if matches!(fmt.get(i), Some(b'l') | Some(b'L') | Some(b'h')) {
            i += 1;
        }
        let c = fmt.get(i).copied().unwrap_or(0);
        match c {
            b'c' | b's' | b'n' | b'd' | b'D' | b'i' | b'o' | b'x' | b'X' | b'u' | b'f' | b'e'
            | b'E' | b'g' => i += 1,
            b'[' => {
                i += 1;
                if fmt.get(i) == Some(&b'^') {
                    i += 1;
                }
                if fmt.get(i) == Some(&b']') {
                    i += 1;
                }
                while fmt.get(i) != Some(&b']') {
                    if i >= fmt.len() {
                        return Err(Unwind::value_error("Unmatched [ in format string"));
                    }
                    i += 1;
                }
                i += 1;
            }
            other => return Err(bad(other)),
        }
        if !suppress {
            // Assigning past the supplied variables is php's `badIndex`.
            if num_vars > 0 && obj_index >= num_vars {
                return Err(Unwind::value_error(if got_xpg {
                    "\"%n$\" argument index out of range"
                } else {
                    "Different numbers of variable names and field specifiers"
                }));
            }
            if obj_index >= nassign.len() {
                nassign.resize(obj_index + 1, 0);
            }
            nassign[obj_index] += 1;
            obj_index += 1;
        }
    }
    let total = if num_vars == 0 {
        if xpg_size > 0 { xpg_size } else { obj_index }
    } else {
        num_vars
    };
    if nassign.len() < total {
        nassign.resize(total, 0);
    }
    for &n in nassign.iter().take(total) {
        if n > 1 {
            return Err(Unwind::value_error(
                "Variable is assigned by multiple \"%n$\" conversion specifiers",
            ));
        }
        if n == 0 && xpg_size == 0 {
            return Err(Unwind::value_error(
                "Variable is not assigned by any conversion specifiers",
            ));
        }
    }
    Ok(total)
}

/// C `strtol`-style parse of the accumulated digits with `base` (0 = detect
/// `0x`/`0` prefixes), saturating at the `i64` bounds.
fn scan_strtol(buf: &[u8], base: u32) -> i64 {
    let mut i = 0;
    let mut neg = false;
    if i < buf.len() && (buf[i] == b'+' || buf[i] == b'-') {
        neg = buf[i] == b'-';
        i += 1;
    }
    let mut base = base;
    if (base == 0 || base == 16)
        && i + 1 < buf.len()
        && buf[i] == b'0'
        && (buf[i + 1] == b'x' || buf[i + 1] == b'X')
    {
        base = 16;
        i += 2;
    } else if base == 0 && i < buf.len() && buf[i] == b'0' {
        base = 8;
    } else if base == 0 {
        base = 10;
    }
    let mut acc: i128 = 0;
    while i < buf.len() {
        let Some(d) = (buf[i] as char).to_digit(base) else { break };
        acc = acc * base as i128 + d as i128;
        if acc > u64::MAX as i128 {
            acc = u64::MAX as i128 + 1;
        }
        i += 1;
    }
    if neg {
        (-acc).max(i64::MIN as i128) as i64
    } else {
        acc.min(i64::MAX as i128) as i64
    }
}

/// C `strtoul`: like [`scan_strtol`] but wrapping to `u64` (a negative input
/// wraps around) and saturating at `u64::MAX`.
fn scan_strtoul(buf: &[u8]) -> u64 {
    let mut i = 0;
    let mut neg = false;
    if i < buf.len() && (buf[i] == b'+' || buf[i] == b'-') {
        neg = buf[i] == b'-';
        i += 1;
    }
    let mut acc: u128 = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        acc = acc * 10 + (buf[i] - b'0') as u128;
        if acc > u64::MAX as u128 {
            acc = u64::MAX as u128 + 1;
        }
        i += 1;
    }
    if acc > u64::MAX as u128 {
        return u64::MAX;
    }
    if neg {
        (acc as u64).wrapping_neg()
    } else {
        acc as u64
    }
}

/// The scanning loop (php's `php_sscanf_internal`): fills `slots` (one per
/// result position) and returns the number of conversions, or `None` when
/// the input ran out before any conversion happened.
fn scan_internal(s: &[u8], fmt: &[u8], slots: &mut [Option<Value>]) -> Option<usize> {
    // php scans a C string: a NUL byte ends the input.
    let s = match s.iter().position(|&b| b == 0) {
        Some(p) => &s[..p],
        None => s,
    };
    let at = |i: usize| -> u8 { s.get(i).copied().unwrap_or(0) };
    let mut pos = 0usize;
    let mut fi = 0usize;
    let mut obj_index = 0usize;
    let mut nconv = 0usize;
    let mut underflow = false;
    'outer: while fi < fmt.len() {
        let ch = fmt[fi];
        fi += 1;
        if is_c_space(ch) {
            while is_c_space(at(pos)) {
                pos += 1;
            }
            continue;
        }
        if ch != b'%' || fmt.get(fi) == Some(&b'%') {
            if ch == b'%' {
                fi += 1;
            }
            // A literal must match the next input byte.
            if pos >= s.len() {
                underflow = true;
                break;
            }
            if s[pos] != ch {
                break;
            }
            pos += 1;
            continue;
        }
        let mut suppress = false;
        if fmt.get(fi) == Some(&b'*') {
            suppress = true;
            fi += 1;
        } else if fmt.get(fi).is_some_and(u8::is_ascii_digit) {
            let mut j = fi;
            let mut value = 0usize;
            while j < fmt.len() && fmt[j].is_ascii_digit() {
                value = value.saturating_mul(10).saturating_add((fmt[j] - b'0') as usize);
                j += 1;
            }
            if fmt.get(j) == Some(&b'$') {
                fi = j + 1;
                obj_index = value - 1;
            }
        }
        let mut width = 0usize;
        while fi < fmt.len() && fmt[fi].is_ascii_digit() {
            width = width.saturating_mul(10).saturating_add((fmt[fi] - b'0') as usize);
            fi += 1;
        }
        if matches!(fmt.get(fi), Some(b'l') | Some(b'L') | Some(b'h')) {
            fi += 1;
        }
        let conv = fmt[fi];
        fi += 1;
        let mut noskip = false;
        let mut base = 10u32;
        let mut unsigned = false;
        let op = match conv {
            b'n' => {
                if !suppress {
                    if let Some(slot) = slots.get_mut(obj_index) {
                        *slot = Some(Value::Int(pos as i64));
                    }
                    obj_index += 1;
                }
                nconv += 1;
                continue;
            }
            b'd' | b'D' => ScanOp::Int,
            b'i' => {
                base = 0;
                ScanOp::Int
            }
            b'o' => {
                base = 8;
                ScanOp::Int
            }
            b'x' | b'X' => {
                base = 16;
                ScanOp::Int
            }
            b'u' => {
                unsigned = true;
                ScanOp::Int
            }
            b'f' | b'e' | b'E' | b'g' => ScanOp::Float,
            b's' => ScanOp::Str,
            b'c' => {
                noskip = true;
                if width == 0 {
                    width = 1;
                }
                ScanOp::Str
            }
            _ => {
                noskip = true;
                ScanOp::Set
            }
        };
        let cset = if op == ScanOp::Set {
            let (set, close) = build_char_set(fmt, fi);
            fi = close + 1;
            Some(set)
        } else {
            None
        };
        if pos >= s.len() {
            underflow = true;
            break;
        }
        if !noskip {
            while pos < s.len() && is_c_space(s[pos]) {
                pos += 1;
            }
            if pos >= s.len() {
                underflow = true;
                break;
            }
        }
        let mut store = |v: Value, obj_index: &mut usize| {
            if !suppress {
                if let Some(slot) = slots.get_mut(*obj_index) {
                    *slot = Some(v);
                }
                *obj_index += 1;
            }
        };
        match op {
            ScanOp::Str => {
                let mut end = pos;
                let mut w = width;
                while end < s.len() {
                    if is_c_space(s[end]) {
                        break;
                    }
                    end += 1;
                    w = w.wrapping_sub(1);
                    if w == 0 {
                        break;
                    }
                }
                store(str_value(s[pos..end].to_vec()), &mut obj_index);
                pos = end;
            }
            ScanOp::Set => {
                let set = cset.as_ref().unwrap();
                let mut end = pos;
                let mut w = if width == 0 { usize::MAX } else { width };
                while end < s.len() && set.contains(s[end]) {
                    end += 1;
                    w -= 1;
                    if w == 0 {
                        break;
                    }
                }
                if end == pos {
                    break 'outer;
                }
                store(str_value(s[pos..end].to_vec()), &mut obj_index);
                pos = end;
            }
            ScanOp::Int => {
                let mut buf: Vec<u8> = Vec::new();
                let mut w = if width == 0 || width > 63 { 63 } else { width };
                let (mut signok, mut nodigits, mut nozero, mut xok) = (true, true, true, false);
                let mut base = base;
                while w > 0 && pos < s.len() {
                    let c = s[pos];
                    let take = match c {
                        b'0' => {
                            if base == 16 {
                                xok = true;
                            }
                            if base == 0 {
                                base = 8;
                                xok = true;
                            }
                            if nozero {
                                signok = false;
                                nodigits = false;
                                nozero = false;
                            } else {
                                signok = false;
                                xok = false;
                                nodigits = false;
                            }
                            true
                        }
                        b'1'..=b'7' => {
                            if base == 0 {
                                base = 10;
                            }
                            signok = false;
                            xok = false;
                            nodigits = false;
                            true
                        }
                        b'8' | b'9' => {
                            if base == 0 {
                                base = 10;
                            }
                            if base <= 8 {
                                false
                            } else {
                                signok = false;
                                xok = false;
                                nodigits = false;
                                true
                            }
                        }
                        b'a'..=b'f' | b'A'..=b'F' => {
                            if base <= 10 {
                                false
                            } else {
                                signok = false;
                                xok = false;
                                nodigits = false;
                                true
                            }
                        }
                        b'+' | b'-' => std::mem::take(&mut signok),
                        b'x' | b'X' if xok && buf.len() == 1 => {
                            base = 16;
                            xok = false;
                            true
                        }
                        _ => false,
                    };
                    if !take {
                        break;
                    }
                    buf.push(c);
                    pos += 1;
                    w -= 1;
                }
                if nodigits {
                    if pos >= s.len() {
                        underflow = true;
                    }
                    break 'outer;
                }
                if matches!(buf.last(), Some(b'x') | Some(b'X')) {
                    buf.pop();
                    pos -= 1;
                }
                if !suppress {
                    if unsigned {
                        let v = scan_strtoul(&buf);
                        if v > i64::MAX as u64 {
                            store(str_value(v.to_string().into_bytes()), &mut obj_index);
                        } else {
                            store(Value::Int(v as i64), &mut obj_index);
                        }
                    } else {
                        store(Value::Int(scan_strtol(&buf, base)), &mut obj_index);
                    }
                }
            }
            ScanOp::Float => {
                let mut buf: Vec<u8> = Vec::new();
                let mut w = if width == 0 || width > 63 { 63 } else { width };
                let (mut signok, mut nodigits, mut ptok, mut expok) = (true, true, true, true);
                while w > 0 && pos < s.len() {
                    let c = s[pos];
                    let take = match c {
                        b'0'..=b'9' => {
                            signok = false;
                            nodigits = false;
                            true
                        }
                        b'+' | b'-' => std::mem::take(&mut signok),
                        b'.' => {
                            if ptok {
                                signok = false;
                                ptok = false;
                                true
                            } else {
                                false
                            }
                        }
                        b'e' | b'E' if !nodigits && expok => {
                            expok = false;
                            ptok = false;
                            signok = true;
                            nodigits = true;
                            true
                        }
                        _ => false,
                    };
                    if !take {
                        break;
                    }
                    buf.push(c);
                    pos += 1;
                    w -= 1;
                }
                if nodigits {
                    if expok {
                        if pos >= s.len() {
                            underflow = true;
                        }
                        break 'outer;
                    }
                    // A dangling exponent ("1e", "1e+") is given back.
                    let last = buf.pop();
                    pos -= 1;
                    if !matches!(last, Some(b'e') | Some(b'E')) {
                        buf.pop();
                        pos -= 1;
                    }
                }
                if !suppress {
                    let text = String::from_utf8_lossy(&buf);
                    let v = text.parse::<f64>().unwrap_or_else(|_| {
                        // "1." / ".5" style inputs Rust rejects: normalize.
                        let t = text.trim_end_matches('.');
                        let t = if t.starts_with('.') { format!("0{t}") } else { t.to_string() };
                        let t = t.replace("-.", "-0.").replace("+.", "+0.");
                        t.parse::<f64>().unwrap_or(0.0)
                    });
                    store(Value::Float(v), &mut obj_index);
                }
            }
        }
        nconv += 1;
    }
    if underflow && nconv == 0 {
        None
    } else {
        Some(nconv)
    }
}

/// `sscanf(string $string, string $format, mixed &...$vars): array|int|null`.
pub(crate) fn sscanf(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let fmt = bytes(&args[1]);
    let num_vars = args.len() - 2;
    let total = validate_scan_format(&fmt, num_vars)?;
    let mut slots: Vec<Option<Value>> = vec![None; total];
    let result = scan_internal(&s, &fmt, &mut slots);
    if num_vars > 0 {
        for (i, slot) in slots.into_iter().enumerate() {
            if let Some(v) = slot {
                if let Some(a) = args.get_mut(2 + i) {
                    *a = v;
                }
            }
        }
        return Ok(Value::Int(result.map_or(-1, |n| n as i64)));
    }
    match result {
        None => Ok(Value::Null),
        Some(_) => {
            let mut out = Array::new();
            for slot in slots {
                out.push(slot.unwrap_or(Value::Null));
            }
            Ok(Value::Array(out))
        }
    }
}
