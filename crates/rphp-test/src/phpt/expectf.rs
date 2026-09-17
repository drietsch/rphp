//! Output comparison: `--EXPECT--` (exact), `--EXPECTF--` (run-tests
//! wildcards) and `--EXPECTREGEX--` (raw regex), plus a readable diff.
//!
//! [`expectf_to_regex`] is a port of run-tests.php's `expectf_to_regex()`:
//! the expected text is regex-quoted except inside `%r…%r` regions, then the
//! wildcards are substituted in one left-to-right pass (PHP `strtr` with a
//! table of two-byte keys). The result is matched with `^…$` and the `s`
//! flag. The `%unicode|string%` / `%binary_string%` variants of PHP 5 are gone
//! from 8.5's run-tests, so they are not supported here either.
//!
//! Matching is byte-oriented and non-Unicode (`(?-u)`), which is what PCRE
//! without the `u` modifier does: `.` is any byte, `\d`/`\s` are ASCII, and
//! non-UTF-8 output compares fine. Two differences from PCRE remain: the
//! `regex` crate has no look-around or back-references (two tests in the
//! whole 8.5.0 corpus use them inside `%r`/EXPECTREGEX), and `%f` is spelled
//! without PCRE's look-ahead, with the same language. Octal escapes (`\0`)
//! are enabled and a literal `{` is escaped on the way through, since PCRE
//! accepts both where `regex` would not.
//!
//! For EXPECTF blocks whose wildcards cannot cross a line (`%s %S %d %i %x
//! %f %e %0` — everything but `%a %A %w %c %r`) the comparison is done line
//! by line with tiny regexes, which is equivalent to the single anchored regex
//! run-tests builds but avoids the quadratic cost of a lazy `.+?` chain on
//! large outputs.

use serde::{Deserialize, Serialize};

use super::parse::ExpectKind;

/// The bytes PHP's `trim()` strips by default.
fn is_php_trim_byte(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\0' | 0x0B)
}

/// PHP `trim()` with the default character list.
pub fn php_trim(bytes: &[u8]) -> &[u8] {
    php_trim_end(php_trim_start(bytes))
}

/// PHP `ltrim()` with the default character list.
pub fn php_trim_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&b| !is_php_trim_byte(b))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

/// PHP `rtrim()` with the default character list.
pub fn php_trim_end(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&b| !is_php_trim_byte(b))
        .map_or(0, |i| i + 1);
    &bytes[..end]
}

/// Replace every `\r\n` by `\n`.
pub fn crlf_to_lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// What run-tests does to the process output before comparing:
/// `preg_replace("/\r\n/", "\n", trim($out))`.
pub fn normalize_output(raw: &[u8]) -> Vec<u8> {
    crlf_to_lf(php_trim(raw))
}

/// What run-tests does to the expectation before comparing: trim, then
/// `\r\n` → `\n`.
pub fn normalize_expected(raw: &[u8]) -> Vec<u8> {
    crlf_to_lf(php_trim(raw))
}

/// The regex text a wildcard stands for.
fn wildcard(c: u8) -> Option<&'static str> {
    Some(match c {
        b'e' => {
            if cfg!(windows) {
                r"\\"
            } else {
                "/"
            }
        }
        b's' => r"[^\r\n]+",
        b'S' => r"[^\r\n]*",
        b'a' => r".+?",
        b'A' => r".*?",
        b'w' => r"\s*",
        b'i' => r"[+-]?\d+",
        b'd' => r"\d+",
        b'x' => r"[0-9a-fA-F]+",
        b'f' => r"[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:[Ee][+-]?\d+)?",
        b'c' => r".",
        b'0' => r"\x00",
        _ => return None,
    })
}

/// Does this wildcard match across line boundaries? (`%c` is `.` under
/// PCRE's `s` flag, so it does.)
fn is_multiline_wildcard(c: u8) -> bool {
    matches!(c, b'a' | b'A' | b'w' | b'c' | b'r')
}

/// Push one literal byte, escaped for the `regex` crate in `(?-u)` mode.
fn push_quoted_byte(out: &mut String, b: u8) {
    match b {
        b'\\' | b'.' | b'+' | b'*' | b'?' | b'(' | b')' | b'|' | b'[' | b']' | b'{' | b'}'
        | b'^' | b'$' | b'#' | b'&' | b'-' | b'~' => {
            out.push('\\');
            out.push(b as char);
        }
        0x20..=0x7E => out.push(b as char),
        _ => out.push_str(&format!("\\x{b:02X}")),
    }
}

/// `preg_quote` for the `regex` crate: metacharacters escaped, control and
/// non-ASCII bytes spelled as `\xNN` so the pattern stays ASCII. `%` is left
/// alone so wildcard substitution can run afterwards, as in run-tests.
pub fn quote(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 8);
    for &b in bytes {
        push_quoted_byte(&mut out, b);
    }
    out
}

/// Pass raw PCRE text (an EXPECTREGEX section or the inside of `%r…%r`)
/// through to the `regex` crate's dialect: non-ASCII bytes become `\xNN`, a
/// `{` that does not open a counted repetition is escaped (PCRE treats it as
/// a literal, `regex` rejects it) and PCRE2's `{,n}` becomes `{0,n}`.
/// Escapes and character classes are left untouched.
pub fn passthrough(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 8);
    let mut i = 0;
    let mut in_class = false;
    while i < bytes.len() {
        let b = bytes[i];
        if !b.is_ascii() {
            out.push_str(&format!("\\x{b:02X}"));
            i += 1;
            continue;
        }
        match b {
            b'\\' => {
                out.push('\\');
                if let Some(&n) = bytes.get(i + 1) {
                    if n.is_ascii() {
                        out.push(n as char);
                    } else {
                        out.push_str(&format!("x{n:02X}"));
                    }
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            b'[' if !in_class => {
                in_class = true;
                out.push('[');
                i += 1;
                // A leading `^` and/or `]` are literal members.
                if bytes.get(i) == Some(&b'^') {
                    out.push('^');
                    i += 1;
                }
                if bytes.get(i) == Some(&b']') {
                    out.push_str("\\]");
                    i += 1;
                }
                continue;
            }
            b']' if in_class => {
                in_class = false;
            }
            b'{' if !in_class => match counted_repetition(&bytes[i..]) {
                Some((len, fixed)) => {
                    out.push_str(&fixed);
                    i += len;
                    continue;
                }
                None => {
                    out.push_str("\\{");
                    i += 1;
                    continue;
                }
            },
            _ => {}
        }
        out.push(b as char);
        i += 1;
    }
    out
}

/// If `s` starts with a PCRE counted repetition (`{n}`, `{n,}`, `{n,m}`,
/// `{,m}`), return its length and its spelling for the `regex` crate.
fn counted_repetition(s: &[u8]) -> Option<(usize, String)> {
    let close = s.iter().position(|&b| b == b'}')?;
    let inner = std::str::from_utf8(&s[1..close]).ok()?;
    let ok_digits = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
    let spelled = match inner.split_once(',') {
        None if ok_digits(inner) => format!("{{{inner}}}"),
        Some((lo, hi)) if lo.is_empty() && ok_digits(hi) => format!("{{0,{hi}}}"),
        Some((lo, hi)) if ok_digits(lo) && (hi.is_empty() || ok_digits(hi)) => {
            format!("{{{lo},{hi}}}")
        }
        _ => return None,
    };
    Some((close + 1, spelled))
}

/// Substitute the wildcards in already-quoted text: a single left-to-right
/// pass over two-byte keys, like PHP's `strtr()` with an array.
fn substitute_wildcards(quoted: &str) -> String {
    let bytes = quoted.as_bytes();
    let mut out = String::with_capacity(quoted.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 1 < bytes.len() {
            if let Some(re) = wildcard(bytes[i + 1]) {
                out.push_str(re);
                i += 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Convert an EXPECTF block (already CRLF-normalised and trimmed by the
/// caller, or not — `\r\n` is normalised here too) into regex text, exactly
/// like run-tests.php's `expectf_to_regex()`.
pub fn expectf_to_regex(wanted: &[u8]) -> String {
    let wanted = crlf_to_lf(wanted);
    let mut out = String::with_capacity(wanted.len() + 32);
    let r = b"%r";
    let mut offset = 0;
    let len = wanted.len();
    while offset < len {
        let (start, end) = match find(&wanted, r, offset) {
            Some(start) => match find(&wanted, r, start + 2) {
                Some(end) => (start, end),
                // Unbalanced tag: keep the rest literally.
                None => (len, len),
            },
            None => (len, len),
        };
        out.push_str(&quote(&wanted[offset..start]));
        if end > start {
            out.push('(');
            out.push_str(&passthrough(&wanted[start + 2..end]));
            out.push(')');
        }
        offset = end + 2;
    }
    substitute_wildcards(&out)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Does an EXPECTF block contain a wildcard that can span lines (`%a`, `%A`,
/// `%w`, `%c`) or a raw `%r…%r` region? If not, line-wise matching is exact.
pub fn has_multiline_wildcard(wanted: &[u8]) -> bool {
    let mut i = 0;
    while i + 1 < wanted.len() {
        if wanted[i] == b'%' {
            let c = wanted[i + 1];
            if is_multiline_wildcard(c) {
                return true;
            }
            if wildcard(c).is_some() {
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    false
}

/// Compile `pattern` anchored (`^…$`) with PCRE's `s` flag, non-Unicode,
/// generous size limits.
pub fn build_regex(pattern: &str) -> Result<regex::bytes::Regex, regex::Error> {
    regex::bytes::RegexBuilder::new(&format!("^{pattern}$"))
        .dot_matches_new_line(true)
        .unicode(false)
        .octal(true)
        .size_limit(1 << 30)
        .dfa_size_limit(1 << 26)
        .build()
}

/// Why matching could not be decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchError(pub String);

impl std::fmt::Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MatchError {}

/// Compare normalised output with a normalised expectation.
pub fn matches(kind: ExpectKind, wanted: &[u8], actual: &[u8]) -> Result<bool, MatchError> {
    match kind {
        ExpectKind::Exact => Ok(wanted == actual),
        ExpectKind::Regex => {
            let re = build_regex(&passthrough(wanted)).map_err(|e| MatchError(e.to_string()))?;
            Ok(re.is_match(actual))
        }
        ExpectKind::Format => {
            if has_multiline_wildcard(wanted) {
                let re = build_regex(&expectf_to_regex(wanted))
                    .map_err(|e| MatchError(e.to_string()))?;
                return Ok(re.is_match(actual));
            }
            let w: Vec<&[u8]> = wanted.split(|&b| b == b'\n').collect();
            let a: Vec<&[u8]> = actual.split(|&b| b == b'\n').collect();
            if w.len() != a.len() {
                return Ok(false);
            }
            for (wl, al) in w.iter().zip(a.iter()) {
                if !format_line_matches(wl, al)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

/// Match one EXPECTF line against one output line.
fn format_line_matches(wanted: &[u8], actual: &[u8]) -> Result<bool, MatchError> {
    if !wanted.contains(&b'%') {
        return Ok(wanted == actual);
    }
    let re = build_regex(&expectf_to_regex(wanted)).map_err(|e| MatchError(e.to_string()))?;
    Ok(re.is_match(actual))
}

/// A precompiled per-line matcher used by the differ.
enum LinePattern {
    Literal(Vec<u8>),
    Re(regex::bytes::Regex),
    Broken,
}

impl LinePattern {
    fn new(kind: ExpectKind, line: &[u8]) -> Self {
        match kind {
            ExpectKind::Exact => LinePattern::Literal(line.to_vec()),
            ExpectKind::Format if !line.contains(&b'%') => LinePattern::Literal(line.to_vec()),
            ExpectKind::Format => match build_regex(&expectf_to_regex(line)) {
                Ok(re) => LinePattern::Re(re),
                Err(_) => LinePattern::Broken,
            },
            ExpectKind::Regex => match build_regex(&passthrough(line)) {
                Ok(re) => LinePattern::Re(re),
                Err(_) => LinePattern::Literal(line.to_vec()),
            },
        }
    }

    fn matches(&self, actual: &[u8]) -> bool {
        match self {
            LinePattern::Literal(l) => l == actual,
            LinePattern::Re(re) => re.is_match(actual),
            LinePattern::Broken => false,
        }
    }
}

/// A readable description of a mismatch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    /// 1-based line number in the expected output where the first mismatch
    /// happens (one past the end when the output is longer than expected).
    pub line: usize,
    /// The expected line there (lossy; `<end of expected>` past the end).
    pub expected: String,
    /// The actual line there (lossy; `<end of output>` past the end).
    pub actual: String,
    /// A unified-style excerpt: `NNN- expected` / `NNN+ actual` with a little
    /// context, truncated for very long outputs.
    pub text: String,
}

impl Diff {
    /// One-line summary for reports.
    pub fn summary(&self) -> String {
        format!(
            "line {}: expected {:?}, got {:?}",
            self.line,
            truncate(&self.expected, 120),
            truncate(&self.actual, 120)
        )
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Maximum number of DP cells the line-diff spends before falling back to a
/// first-divergence excerpt.
const MAX_LCS_CELLS: usize = 1_000_000;
/// Maximum number of rendered diff lines.
const MAX_DIFF_LINES: usize = 60;

/// Explain why `actual` does not match `wanted` (both normalised).
pub fn diff(kind: ExpectKind, wanted: &[u8], actual: &[u8]) -> Diff {
    let w: Vec<&[u8]> = wanted.split(|&b| b == b'\n').collect();
    let a: Vec<&[u8]> = actual.split(|&b| b == b'\n').collect();
    let pats: Vec<LinePattern> = w.iter().map(|l| LinePattern::new(kind, l)).collect();

    // First mismatch, walking in lockstep.
    let mut first = 0;
    while first < w.len() && first < a.len() && pats[first].matches(a[first]) {
        first += 1;
    }
    let expected = w
        .get(first)
        .map(|l| lossy(l))
        .unwrap_or_else(|| "<end of expected>".to_string());
    let actual_line = a
        .get(first)
        .map(|l| lossy(l))
        .unwrap_or_else(|| "<end of output>".to_string());

    // Strip the common prefix/suffix, LCS the middle when it is small enough.
    let mut suffix = 0;
    while suffix < w.len() - first
        && suffix < a.len() - first
        && pats[w.len() - 1 - suffix].matches(a[a.len() - 1 - suffix])
    {
        suffix += 1;
    }
    let (wm0, wm1) = (first, w.len() - suffix);
    let (am0, am1) = (first, a.len() - suffix);
    let n = wm1 - wm0;
    let m = am1 - am0;

    let mut lines: Vec<String> = Vec::new();
    let ctx_start = first.saturating_sub(3);
    for (i, line) in w.iter().enumerate().take(first).skip(ctx_start) {
        lines.push(format!("{:03}  {}", i + 1, lossy(line)));
    }

    let ops: Vec<Op> = if n.saturating_mul(m) <= MAX_LCS_CELLS {
        lcs_ops(&pats[wm0..wm1], &a[am0..am1])
    } else {
        // Too big: show everything as removed/added.
        (0..n)
            .map(|_| Op::Del)
            .chain((0..m).map(|_| Op::Add))
            .collect()
    };
    let (mut wi, mut ai) = (wm0, am0);
    let mut truncated = 0usize;
    for op in ops {
        let line = match op {
            Op::Keep => {
                let s = format!("{:03}  {}", wi + 1, lossy(w[wi]));
                wi += 1;
                ai += 1;
                s
            }
            Op::Del => {
                let s = format!("{:03}- {}", wi + 1, lossy(w[wi]));
                wi += 1;
                s
            }
            Op::Add => {
                let s = format!("{:03}+ {}", ai + 1, lossy(a[ai]));
                ai += 1;
                s
            }
        };
        if lines.len() < MAX_DIFF_LINES {
            lines.push(line);
        } else {
            truncated += 1;
        }
    }
    if truncated > 0 {
        lines.push(format!("… ({truncated} more diff lines)"));
    } else {
        for (i, line) in w.iter().enumerate().take(wm1 + 2).skip(wm1) {
            lines.push(format!("{:03}  {}", i + 1, lossy(line)));
        }
    }

    Diff {
        line: first + 1,
        expected,
        actual: actual_line,
        text: lines.join("\n"),
    }
}

#[derive(Clone, Copy)]
enum Op {
    Keep,
    Del,
    Add,
}

/// Classic LCS on (pattern, line) pairs; returns the edit script.
fn lcs_ops(pats: &[LinePattern], lines: &[&[u8]]) -> Vec<Op> {
    let n = pats.len();
    let m = lines.len();
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[idx(i, j)] = if pats[i].matches(lines[j]) {
                table[idx(i + 1, j + 1)] + 1
            } else {
                table[idx(i + 1, j)].max(table[idx(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if pats[i].matches(lines[j]) {
            ops.push(Op::Keep);
            i += 1;
            j += 1;
        } else if table[idx(i + 1, j)] >= table[idx(i, j + 1)] {
            ops.push(Op::Del);
            i += 1;
        } else {
            ops.push(Op::Add);
            j += 1;
        }
    }
    ops.extend((i..n).map(|_| Op::Del));
    ops.extend((j..m).map(|_| Op::Add));
    ops
}
