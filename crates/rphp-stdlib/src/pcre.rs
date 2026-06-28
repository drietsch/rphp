//! `pcre` extension — `preg_*` over PCRE2 (the engine php-src links, so pattern
//! semantics match byte-for-byte). PHP patterns are **delimited** (`/body/ims`):
//! the first non-whitespace byte is the delimiter, the body runs to its matching
//! close, and trailing letters are modifiers. Everything here operates on bytes,
//! since PHP strings (patterns, subjects, replacements) are byte strings.
//!
//! Only the pure, value-returning forms land in this wave. `preg_match`'s
//! `$matches` out-param, `preg_match_all`, `preg_replace_callback`, and the
//! array forms of `preg_replace` wait on by-reference parameters / callables.
use pcre2::bytes::{Captures, Regex, RegexBuilder};
use rphp_value::{Array, Str, Value};

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("preg_quote", 1, Some(2), preg_quote),
    // `$matches` is by-reference, so only the boolean form ships now (max 2).
    nf!("preg_match", 2, Some(2), preg_match),
    nf!("preg_replace", 3, Some(3), preg_replace),
    nf!("preg_split", 2, Some(4), preg_split),
    nf!("preg_grep", 2, Some(3), preg_grep),
];

/// The byte string an argument coerces to (the `(string)` cast), so every
/// builtin accepts any scalar the way PHP's weak typing does.
fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

fn str_value(bytes: Vec<u8>) -> Value {
    Value::Str(Str::from_vec(bytes))
}

/// The single error every malformed pattern / compile failure surfaces as.
fn bad_pattern() -> NativeError {
    NativeError::new("preg: invalid pattern")
}

// ---- pattern parsing & compilation -----------------------------------------

/// Split a PHP delimited pattern (e.g. `/body/ims`) into `(body, modifiers)`.
/// Leading ASCII whitespace is skipped (PHP does the same). The first remaining
/// byte is the delimiter, which must not be alphanumeric or a backslash. A
/// bracket-style delimiter (`( [ { <`) closes with its mate and nests; any other
/// delimiter closes on its next unescaped occurrence. `\x` escapes are skipped
/// while scanning, so an escaped delimiter inside the body is not mistaken for
/// the close. Returns `None` for any malformed pattern.
fn parse_pattern(pat: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let mut start = 0;
    while start < pat.len() && pat[start].is_ascii_whitespace() {
        start += 1;
    }
    let pat = &pat[start..];
    if pat.len() < 2 {
        return None;
    }
    let delim = pat[0];
    if delim.is_ascii_alphanumeric() || delim == b'\\' {
        return None;
    }
    let (close, bracketed) = match delim {
        b'(' => (b')', true),
        b'[' => (b']', true),
        b'{' => (b'}', true),
        b'<' => (b'>', true),
        _ => (delim, false),
    };
    let mut i = 1;
    let mut depth = 1usize;
    let close_idx = loop {
        if i >= pat.len() {
            return None; // no closing delimiter
        }
        let c = pat[i];
        if c == b'\\' && i + 1 < pat.len() {
            i += 2; // an escape consumes the next byte
            continue;
        }
        if bracketed {
            if c == delim {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    break i;
                }
            }
        } else if c == close {
            break i;
        }
        i += 1;
    };
    Some((pat[1..close_idx].to_vec(), &pat[close_idx + 1..]))
}

/// Parse a PHP pattern and compile it through PCRE2 with the right options.
/// Modifiers map as: `i`=caseless, `m`=multi_line, `s`=dotall, `x`=extended,
/// `u`=utf (+ucp). PHP's other valid letters (`A D S U X J n`) are accepted but
/// not all expressible through this builder; whitespace between modifiers is
/// ignored; any other modifier is an error, as in PHP.
fn compile(pattern: &[u8]) -> Result<Regex, NativeError> {
    let (body, mods) = parse_pattern(pattern).ok_or_else(bad_pattern)?;
    // PCRE2's builder takes a `&str`; PHP patterns are almost always UTF-8/ASCII.
    let body = std::str::from_utf8(&body).map_err(|_| bad_pattern())?;
    let mut builder = RegexBuilder::new();
    for &m in mods {
        match m {
            b'i' => {
                builder.caseless(true);
            }
            b'm' => {
                builder.multi_line(true);
            }
            b's' => {
                builder.dotall(true);
            }
            b'x' => {
                builder.extended(true);
            }
            b'u' => {
                builder.utf(true).ucp(true);
            }
            b'A' | b'D' | b'S' | b'U' | b'X' | b'J' | b'n' => {}
            _ if m.is_ascii_whitespace() => {}
            _ => return Err(bad_pattern()),
        }
    }
    builder.build(body).map_err(|_| bad_pattern())
}

// ---- handlers ---------------------------------------------------------------

/// `preg_quote($str, $delimiter = null)`: backslash-escape every PCRE
/// metacharacter (and, if given, the delimiter); a NUL byte becomes `\000`.
pub(crate) fn preg_quote(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    // The delimiter's first byte is escaped too (PHP ignores any rest / null).
    let delim_arg = args.get(1).filter(|v| !matches!(v, Value::Null)).map(bytes);
    let delim = delim_arg.as_deref().and_then(|d| d.first().copied());
    let mut out = Vec::with_capacity(s.len());
    for &b in &s {
        match b {
            b'.' | b'\\' | b'+' | b'*' | b'?' | b'[' | b'^' | b']' | b'$'
            | b'(' | b')' | b'{' | b'}' | b'=' | b'!' | b'<' | b'>' | b'|'
            | b':' | b'-' | b'#' => {
                out.push(b'\\');
                out.push(b);
            }
            0 => out.extend_from_slice(b"\\000"),
            _ if Some(b) == delim => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    Ok(str_value(out))
}

/// `preg_match($pattern, $subject)`: 1 if the pattern matches, else 0. (The
/// `$matches` capture-array form is by-reference and deferred.)
pub(crate) fn preg_match(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let re = compile(&bytes(&args[0]))?;
    let subject = bytes(&args[1]);
    Ok(Value::Int(i64::from(re.is_match(&subject).unwrap_or(false))))
}

/// `preg_replace($pattern, $replacement, $subject)`: replace every match.
/// Backreferences are written `$1`/`${1}`/`\1`; `$0` (or `\0`) is the whole
/// match. (String operands only this wave; array forms are deferred.)
pub(crate) fn preg_replace(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let re = compile(&bytes(&args[0]))?;
    let replacement = bytes(&args[1]);
    let subject = bytes(&args[2]);
    let mut out = Vec::with_capacity(subject.len());
    let mut last = 0usize;
    for caps in re.captures_iter(&subject) {
        let caps = match caps {
            Ok(c) => c,
            Err(_) => break,
        };
        // Group 0 is always the full match; it bounds the slice we keep verbatim.
        let whole = match caps.get(0) {
            Some(m) => m,
            None => continue,
        };
        out.extend_from_slice(&subject[last..whole.start()]);
        expand_replacement(&replacement, &caps, &mut out);
        last = whole.end();
    }
    out.extend_from_slice(&subject[last..]);
    Ok(str_value(out))
}

/// `preg_split($pattern, $subject, $limit = -1, $flags = 0)`: split on matches.
/// `$limit <= 0` means unlimited; a positive limit caps the pieces (the last
/// holds the remainder). `PREG_SPLIT_NO_EMPTY` (flag `1`) drops empty pieces;
/// empties never count against the limit, matching PHP.
pub(crate) fn preg_split(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let re = compile(&bytes(&args[0]))?;
    let subject = bytes(&args[1]);
    let limit = args.get(2).map_or(-1, Value::to_int);
    let flags = args.get(3).map_or(0, Value::to_int);
    let no_empty = flags & 1 != 0;
    let unlimited = limit <= 0;
    let mut remaining = limit;
    let mut out = Array::new();
    let mut last = 0usize;
    for m in re.find_iter(&subject) {
        let m = match m {
            Ok(m) => m,
            Err(_) => break,
        };
        // Once a single slot is left, stop so the tail becomes the final piece.
        if !unlimited && remaining <= 1 {
            break;
        }
        let piece = &subject[last..m.start()];
        last = m.end();
        if no_empty && piece.is_empty() {
            continue; // skipped pieces don't consume a limit slot
        }
        out.push(str_value(piece.to_vec()));
        if !unlimited {
            remaining -= 1;
        }
    }
    let tail = &subject[last..];
    if !(no_empty && tail.is_empty()) {
        out.push(str_value(tail.to_vec()));
    }
    Ok(Value::Array(out))
}

/// `preg_grep($pattern, $array, $flags = 0)`: keep entries whose value matches
/// (or, with `PREG_GREP_INVERT` = flag `1`, those that do not). Keys preserved.
pub(crate) fn preg_grep(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let re = compile(&bytes(&args[0]))?;
    let arr = match &args[1] {
        Value::Array(a) => a,
        other => {
            return Err(NativeError::new(format!(
                "preg_grep(): Argument #2 ($array) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    let invert = args.get(2).map_or(0, Value::to_int) & 1 != 0;
    let mut out = Array::new();
    for (k, v) in arr.iter() {
        let hay = v.to_php_bytes();
        let matched = re.is_match(&hay).unwrap_or(false);
        if matched != invert {
            out.set(k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

// ---- replacement expansion --------------------------------------------------

/// Expand one `$replacement` against `caps`, appending to `out`. Mirrors PHP's
/// `php_pcre_replace_impl`: a `\` or `$` starting a backreference is resolved
/// against the captures; a `\` immediately preceding a `\` or `$` escapes it
/// (`\\` -> `\`, `\$` -> `$`); anything else is copied verbatim.
fn expand_replacement(repl: &[u8], caps: &Captures<'_>, out: &mut Vec<u8>) {
    let mut prev = 0u8; // the byte we last emitted, for the escape rule
    let mut i = 0;
    while i < repl.len() {
        let c = repl[i];
        if c == b'\\' || c == b'$' {
            if prev == b'\\' {
                // The backslash we just emitted escapes this metacharacter:
                // overwrite that backslash with the literal `\` or `$`.
                if let Some(last) = out.last_mut() {
                    *last = c;
                }
                prev = 0;
                i += 1;
                continue;
            }
            if let Some((group, consumed)) = parse_backref(&repl[i..]) {
                if let Some(m) = caps.get(group) {
                    out.extend_from_slice(m.as_bytes());
                }
                prev = 0;
                i += consumed;
                continue;
            }
        }
        out.push(c);
        prev = c;
        i += 1;
    }
}

/// Parse a `$n`, `${n}`, or `\n` backreference at the start of `s` (whose first
/// byte is `$` or `\`). Reads at most two digits — PHP's rule — so `$123` is
/// group 12 followed by a literal `3`. Returns `(group, bytes_consumed)`, or
/// `None` if no digit follows (so the `$`/`\` is a literal).
fn parse_backref(s: &[u8]) -> Option<(usize, usize)> {
    let mut w = 1; // skip the leading '$' or '\\'
    let in_brace = s.get(w) == Some(&b'{');
    if in_brace {
        w += 1;
    }
    let d0 = *s.get(w)?;
    if !d0.is_ascii_digit() {
        return None;
    }
    let mut group = (d0 - b'0') as usize;
    w += 1;
    if let Some(&d1) = s.get(w) {
        if d1.is_ascii_digit() {
            group = group * 10 + (d1 - b'0') as usize;
            w += 1;
        }
    }
    if in_brace {
        if s.get(w) == Some(&b'}') {
            w += 1;
        } else {
            return None; // `${1` without the close is not a backreference
        }
    }
    Some((group, w))
}
