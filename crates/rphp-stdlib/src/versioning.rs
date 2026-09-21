//! php-src `ext/standard/versioning.c`: `version_compare`.
//!
//! A transcription of `php_canonicalize_version` (every `-`/`_`/`+` becomes
//! a `.`, and a `.` is inserted at each digit/non-digit boundary) and
//! `php_version_compare` (numeric parts compare as numbers, the special
//! forms `dev < alpha = a < beta = b < RC = rc < # < pl = p` compare by
//! rank, any other word is `-1`, and a longer version wins unless its extra
//! part is one of the pre-release forms).

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::Value;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[nf!("version_compare", 2, Some(3), version_compare)];

/// `php_canonicalize_version`: `-`/`_`/`+` and any other non-alphanumeric
/// byte become a single `.`, and a `.` is inserted at each digit/non-digit
/// boundary (`1.0rc1` → `1.0.rc.1`, `1..2` → `1.2`).
fn canonicalize(v: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(v.len() * 2);
    let Some((&first, rest)) = v.split_first() else {
        return out;
    };
    out.push(first);
    let mut lp = first;
    let isdig = |x: u8| x.is_ascii_digit();
    let isndig = |x: u8| !x.is_ascii_digit() && x != b'.';
    for &c in rest {
        let lq = *out.last().expect("non-empty");
        if c == b'-' || c == b'_' || c == b'+' {
            if lq != b'.' {
                out.push(b'.');
            }
        } else if (isndig(lp) && isdig(c)) || (isdig(lp) && isndig(c)) {
            if lq != b'.' {
                out.push(b'.');
            }
            out.push(c);
        } else if !c.is_ascii_alphanumeric() {
            if lq != b'.' {
                out.push(b'.');
            }
        } else {
            out.push(c);
        }
        lp = c;
    }
    // 8.5.10: a trailing `.` (`1.0.`, `1.0-`) is not an empty last part.
    if out.last() == Some(&b'.') {
        out.pop();
    }
    out
}

/// `compare_special_version_forms`: the rank of a pre-release / patch word
/// (`-1` for anything else).
fn special_rank(form: &[u8]) -> i64 {
    const FORMS: &[(&[u8], i64)] = &[
        (b"dev", 0),
        (b"alpha", 1),
        (b"a", 1),
        (b"beta", 2),
        (b"b", 2),
        (b"RC", 3),
        (b"rc", 3),
        (b"#", 4),
        (b"pl", 5),
        (b"p", 5),
    ];
    FORMS
        .iter()
        .find(|(name, _)| form.starts_with(name))
        .map_or(-1, |(_, order)| *order)
}

fn sign(a: i64, b: i64) -> i64 {
    match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// `strtol` on a version part: saturating, so an oversized part still
/// orders after every smaller one.
fn part_number(p: &[u8]) -> i64 {
    let mut v: i64 = 0;
    for &b in p {
        if !b.is_ascii_digit() {
            break;
        }
        v = v.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    v
}

/// The part starting at `from` (up to the next `.`), and where the next part
/// starts (`None` when this was the last one).
fn part(s: &[u8], from: usize) -> (&[u8], Option<usize>) {
    match s[from..].iter().position(|&b| b == b'.') {
        Some(off) => (&s[from..from + off], Some(from + off + 1)),
        None => (&s[from..], None),
    }
}

/// `php_version_compare`.
pub(crate) fn compare(v1: &[u8], v2: &[u8]) -> i64 {
    if v1.is_empty() || v2.is_empty() {
        return match (v1.is_empty(), v2.is_empty()) {
            (true, true) => 0,
            (true, false) => -1,
            _ => 1,
        };
    }
    // A version starting with `#` is the `#N#` marker of the recursion below
    // and is compared verbatim.
    let c1 = if v1[0] == b'#' { v1.to_vec() } else { canonicalize(v1) };
    let c2 = if v2[0] == b'#' { v2.to_vec() } else { canonicalize(v2) };
    let (mut p1, mut p2) = (0usize, 0usize);
    let (mut n1, mut n2) = (Some(0usize), Some(0usize));
    let mut compare = 0;
    while p1 < c1.len() && p2 < c2.len() && n1.is_some() && n2.is_some() {
        let (e1, next1) = part(&c1, p1);
        let (e2, next2) = part(&c2, p2);
        n1 = next1;
        n2 = next2;
        let d1 = e1.first().is_some_and(u8::is_ascii_digit);
        let d2 = e2.first().is_some_and(u8::is_ascii_digit);
        compare = match (d1, d2) {
            (true, true) => sign(part_number(e1), part_number(e2)),
            (false, false) => sign(special_rank(e1), special_rank(e2)),
            (true, false) => sign(special_rank(b"#N#"), special_rank(e2)),
            (false, true) => sign(special_rank(e1), special_rank(b"#N#")),
        };
        if compare != 0 {
            break;
        }
        if let Some(n) = n1 {
            p1 = n;
        }
        if let Some(n) = n2 {
            p2 = n;
        }
    }
    if compare == 0 {
        if n1.is_some() {
            // v1 has more parts: it is newer unless the extra part is a
            // pre-release form (`1.0-dev` < `1.0`).
            compare = if c1.get(p1).is_some_and(u8::is_ascii_digit) {
                1
            } else {
                self::compare(&c1[p1..], b"#N#")
            };
        } else if n2.is_some() {
            compare = if c2.get(p2).is_some_and(u8::is_ascii_digit) {
                -1
            } else {
                self::compare(b"#N#", &c2[p2..])
            };
        }
    }
    compare
}

/// `version_compare(string $version1, string $version2, ?string $operator = null): int|bool`
pub(crate) fn version_compare(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v1 = args[0].to_php_bytes();
    let v2 = args[1].to_php_bytes();
    let c = compare(&v1, &v2);
    let op = match args.get(2) {
        Some(v) if !matches!(*v.deref(), Value::Null) => v.to_php_bytes(),
        _ => return Ok(Value::Int(c)),
    };
    let r = match &op[..] {
        b"<" | b"lt" => c == -1,
        b"<=" | b"le" => c != 1,
        b">" | b"gt" => c == 1,
        b">=" | b"ge" => c != -1,
        b"==" | b"eq" => c == 0,
        b"!=" | b"<>" | b"ne" => c != 0,
        _ => {
            return Err(Unwind::value_error(
                "version_compare(): Argument #3 ($operator) must be a valid comparison operator",
            ))
        }
    };
    Ok(Value::Bool(r))
}

#[cfg(test)]
mod tests {
    use super::compare;

    #[test]
    fn version_compare_matches_php() {
        let cases: &[(&str, &str, i64)] = &[
            ("1.0", "1.0.0", -1),
            ("5.2", "5.10", -1),
            ("1.0rc1", "1.0", -1),
            ("1.0RC1", "1.0rc1", 0),
            ("1.0-dev", "1.0", -1),
            ("1.0dev", "1.0alpha", -1),
            ("1.0beta", "1.0RC", -1),
            ("1.0RC", "1.0#", -1),
            ("1.0#", "1.0pl", -1),
            ("1.0pl", "1.0p", 0),
            ("", "", 0),
            ("", "1", -1),
            ("abc", "abd", 0),
            ("1..2", "1.2", 0),
            ("1_0", "1.0", 0),
            ("8.5.0", "8.4.99", 1),
            ("v1.0", "1.0", -1),
            ("1.0 ", "1.0", -1),
            ("1.0.", "1.0", 0),
            ("1.0-", "1.0", 0),
            (".1", "0.1", -1),
            ("1.0.0-stable", "1.0.0", -1),
            ("1.0zzz", "1.0", -1),
            ("#", "pl", -1),
            ("1.0.0-0", "1.0.0", 1),
            ("0", "0.0", -1),
            ("a", "1", -1),
            ("1", "a", 1),
            ("9007199254740993", "9007199254740992", 1),
            ("99999999999999999999", "1", 1),
            ("1.0.", "1.0.", 0),
            ("1.0.0-alpha", "1.0.0-beta", -1),
            ("1.10a", "1.10b", -1),
            ("2.0", "10.0", -1),
        ];
        for (a, b, expected) in cases {
            assert_eq!(compare(a.as_bytes(), b.as_bytes()), *expected, "{a} vs {b}");
        }
    }
}
