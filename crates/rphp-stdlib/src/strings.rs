//! String builtins, all **byte-oriented** (PHP strings are byte strings, never
//! assumed UTF-8). ASCII case mapping is locale-independent, matching PHP 8's
//! `strtoupper`/`strtolower`. Multibyte-aware variants belong to `mbstring`
//! (ICU-backed) and are a separate extension.
use rphp_value::{Array, ArrayKey, Str, Value};

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`). New byte-string
/// functions are added here alongside their handler below.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("strlen", 1, Some(1), strlen),
    nf!("strtoupper", 1, Some(1), strtoupper),
    nf!("strtolower", 1, Some(1), strtolower),
    nf!("ucfirst", 1, Some(1), ucfirst),
    nf!("lcfirst", 1, Some(1), lcfirst),
    nf!("str_repeat", 2, Some(2), str_repeat),
    nf!("substr", 2, Some(3), substr),
    nf!("strpos", 2, Some(3), strpos),
    nf!("str_replace", 3, Some(3), str_replace),
    nf!("trim", 1, Some(2), trim),
    nf!("ltrim", 1, Some(2), ltrim),
    nf!("rtrim", 1, Some(2), rtrim),
    nf!("implode", 1, Some(2), implode),
    nf!("join", 1, Some(2), implode),
    nf!("explode", 2, Some(3), explode),
    nf!("ord", 1, Some(1), ord),
    nf!("chr", 1, Some(1), chr),
    nf!("str_contains", 2, Some(2), str_contains),
    nf!("str_starts_with", 2, Some(2), str_starts_with),
    nf!("str_ends_with", 2, Some(2), str_ends_with),
    nf!("strrev", 1, Some(1), strrev),
    nf!("ucwords", 1, Some(2), ucwords),
    nf!("str_pad", 2, Some(4), str_pad),
    nf!("str_split", 1, Some(2), str_split),
    nf!("substr_count", 2, Some(4), substr_count),
    nf!("strrpos", 2, Some(3), strrpos),
    nf!("stripos", 2, Some(3), stripos),
    nf!("strripos", 2, Some(3), strripos),
    nf!("strstr", 2, Some(3), strstr),
    nf!("stristr", 2, Some(3), stristr),
    nf!("strrchr", 2, Some(2), strrchr),
    nf!("strpbrk", 2, Some(2), strpbrk),
    nf!("strcmp", 2, Some(2), strcmp),
    nf!("strcasecmp", 2, Some(2), strcasecmp),
    nf!("strncmp", 3, Some(3), strncmp),
    nf!("strncasecmp", 3, Some(3), strncasecmp),
    nf!("bin2hex", 1, Some(1), bin2hex),
    nf!("hex2bin", 1, Some(1), hex2bin),
    nf!("nl2br", 1, Some(2), nl2br),
    nf!("strtr", 2, Some(3), strtr),
    nf!("substr_replace", 3, Some(4), substr_replace),
    nf!("quotemeta", 1, Some(1), quotemeta),
    nf!("addslashes", 1, Some(1), addslashes),
    nf!("stripslashes", 1, Some(1), stripslashes),
    nf!("number_format", 1, Some(4), number_format),
    nf!("str_word_count", 1, Some(3), str_word_count),
    nf!("sprintf", 1, None, sprintf),
    nf!("printf", 1, None, printf),
    nf!("vsprintf", 2, Some(2), vsprintf),
];

/// The byte string an argument coerces to (the `(string)` cast). Lets every
/// builtin accept any scalar the way PHP's weak typing does.
fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

fn str_value(bytes: Vec<u8>) -> Value {
    Value::Str(Str::from_vec(bytes))
}

pub(crate) fn strlen(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Int(bytes(&args[0]).len() as i64))
}

pub(crate) fn strtoupper(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    b.make_ascii_uppercase();
    Ok(str_value(b))
}

pub(crate) fn strtolower(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    b.make_ascii_lowercase();
    Ok(str_value(b))
}

pub(crate) fn ucfirst(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    if let Some(first) = b.first_mut() {
        first.make_ascii_uppercase();
    }
    Ok(str_value(b))
}

pub(crate) fn lcfirst(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    if let Some(first) = b.first_mut() {
        first.make_ascii_lowercase();
    }
    Ok(str_value(b))
}

pub(crate) fn str_repeat(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let times = args[1].to_int();
    if times < 0 {
        return Err(NativeError::new(
            "str_repeat(): Argument #2 ($times) must be greater than or equal to 0",
        ));
    }
    Ok(str_value(s.repeat(times as usize)))
}

pub(crate) fn substr(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let n = s.len() as i64;
    let mut start = args[1].to_int();
    if start < 0 {
        start = (n + start).max(0);
    } else {
        start = start.min(n);
    }
    let end = match args.get(2) {
        // `null` length (or absent) means "to the end".
        None | Some(Value::Null) => n,
        Some(len) => {
            let l = len.to_int();
            if l < 0 {
                (n + l).max(start)
            } else {
                (start + l).min(n)
            }
        }
    };
    if end <= start {
        Ok(str_value(Vec::new()))
    } else {
        Ok(str_value(s[start as usize..end as usize].to_vec()))
    }
}

pub(crate) fn strpos(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let n = haystack.len() as i64;
    let mut start = args.get(2).map_or(0, Value::to_int);
    if start < 0 {
        start = (n + start).max(0);
    }
    if start > n {
        return Ok(Value::Bool(false));
    }
    match find(&haystack[start as usize..], &needle) {
        Some(pos) => Ok(Value::Int(start + pos as i64)),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn str_replace(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let search = bytes(&args[0]);
    let replace = bytes(&args[1]);
    let subject = bytes(&args[2]);
    Ok(str_value(replace_all(&subject, &search, &replace)))
}

pub(crate) fn trim(_: &mut Ctx, args: &[Value]) -> NativeResult {
    trim_impl(args, true, true)
}

pub(crate) fn ltrim(_: &mut Ctx, args: &[Value]) -> NativeResult {
    trim_impl(args, true, false)
}

pub(crate) fn rtrim(_: &mut Ctx, args: &[Value]) -> NativeResult {
    trim_impl(args, false, true)
}

pub(crate) fn implode(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // `implode($array)` (glue ""), `implode($glue, $array)`, and the legacy
    // reversed `implode($array, $glue)` order are all accepted.
    let (glue, array) = match args {
        [Value::Array(a)] => (Vec::new(), a),
        [glue, Value::Array(a)] => (bytes(glue), a),
        [Value::Array(a), glue] => (bytes(glue), a),
        _ => {
            return Err(NativeError::new(
                "implode(): Argument must be of type array",
            ))
        }
    };
    let mut out = Vec::new();
    for (i, (_, v)) in array.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(&glue);
        }
        v.append_php_bytes(&mut out);
    }
    Ok(str_value(out))
}

pub(crate) fn explode(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let sep = bytes(&args[0]);
    let subject = bytes(&args[1]);
    if sep.is_empty() {
        return Err(NativeError::new(
            "explode(): Argument #1 ($separator) cannot be empty",
        ));
    }
    let limit = args.get(2).map_or(i64::MAX, Value::to_int);
    let mut parts: Vec<&[u8]> = Vec::new();
    let mut rest = &subject[..];
    // Split greedily; a positive limit caps the piece count with the remainder
    // kept whole in the last piece.
    loop {
        if limit > 0 && parts.len() as i64 == limit - 1 {
            break;
        }
        match find(rest, &sep) {
            Some(pos) => {
                parts.push(&rest[..pos]);
                rest = &rest[pos + sep.len()..];
            }
            None => break,
        }
    }
    parts.push(rest);
    // A negative limit drops that many trailing pieces.
    if limit < 0 {
        let drop = (-limit) as usize;
        if drop >= parts.len() {
            parts.clear();
        } else {
            parts.truncate(parts.len() - drop);
        }
    }
    let mut out = Array::new();
    for p in parts {
        out.push(str_value(p.to_vec()));
    }
    Ok(Value::Array(out))
}

pub(crate) fn ord(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let b = bytes(&args[0]);
    Ok(Value::Int(b.first().copied().unwrap_or(0) as i64))
}

pub(crate) fn chr(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // PHP reduces the codepoint modulo 256.
    let byte = args[0].to_int().rem_euclid(256) as u8;
    Ok(str_value(vec![byte]))
}

pub(crate) fn str_contains(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    Ok(Value::Bool(find(&haystack, &needle).is_some()))
}

pub(crate) fn str_starts_with(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    Ok(Value::Bool(haystack.starts_with(&needle)))
}

pub(crate) fn str_ends_with(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    Ok(Value::Bool(haystack.ends_with(&needle)))
}

// ---- helpers ----------------------------------------------------------------

/// First byte-offset of `needle` in `haystack`. An empty needle matches at 0
/// (PHP's `strpos`/`str_contains` semantics).
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Replace every non-overlapping occurrence of `search` in `subject`. An empty
/// search leaves the subject unchanged (no infinite loop), as PHP does.
fn replace_all(subject: &[u8], search: &[u8], replace: &[u8]) -> Vec<u8> {
    if search.is_empty() {
        return subject.to_vec();
    }
    let mut out = Vec::with_capacity(subject.len());
    let mut rest = subject;
    while let Some(pos) = find(rest, search) {
        out.extend_from_slice(&rest[..pos]);
        out.extend_from_slice(replace);
        rest = &rest[pos + search.len()..];
    }
    out.extend_from_slice(rest);
    out
}

fn trim_impl(args: &[Value], left: bool, right: bool) -> NativeResult {
    let s = bytes(&args[0]);
    // Default trim set: " \t\n\r\0\x0B" (matches php-src).
    let chars: Vec<u8> = match args.get(1) {
        Some(c) => bytes(c),
        None => vec![b' ', b'\t', b'\n', b'\r', 0, 0x0b],
    };
    let in_set = |b: u8| chars.contains(&b);
    let mut start = 0;
    let mut end = s.len();
    if left {
        while start < end && in_set(s[start]) {
            start += 1;
        }
    }
    if right {
        while end > start && in_set(s[end - 1]) {
            end -= 1;
        }
    }
    Ok(str_value(s[start..end].to_vec()))
}

// ---- extension: search, comparison, transformation, formatting ----------------

/// ASCII-lowercased copy, for the case-insensitive search/compare variants.
fn ascii_lower(b: &[u8]) -> Vec<u8> {
    b.iter().map(u8::to_ascii_lowercase).collect()
}

pub(crate) fn strrev(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    b.reverse();
    Ok(str_value(b))
}

pub(crate) fn ucwords(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    // Default word delimiters match php-src: " \t\r\n\f\v".
    let delims: Vec<u8> = match args.get(1) {
        Some(d) => bytes(d),
        None => vec![b' ', b'\t', b'\r', b'\n', 0x0c, 0x0b],
    };
    // A byte is capitalized iff it follows a delimiter (or starts the string).
    // PHP inspects the *previous, already-modified* byte, so we do the same.
    let mut prev_delim = true;
    for c in b.iter_mut() {
        if prev_delim {
            c.make_ascii_uppercase();
        }
        prev_delim = delims.contains(c);
    }
    Ok(str_value(b))
}

pub(crate) fn str_pad(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let target = args[1].to_int();
    let pad = match args.get(2) {
        Some(p) => bytes(p),
        None => vec![b' '],
    };
    // 0 = STR_PAD_LEFT, 1 = STR_PAD_RIGHT (default), 2 = STR_PAD_BOTH.
    let ptype = args.get(3).map_or(1, Value::to_int);
    if pad.is_empty() {
        return Err(NativeError::new(
            "str_pad(): Argument #3 ($pad_string) must not be empty",
        ));
    }
    if !(0..=2).contains(&ptype) {
        return Err(NativeError::new(
            "str_pad(): Argument #4 ($pad_type) must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH",
        ));
    }
    let cur = s.len() as i64;
    if target <= cur {
        return Ok(str_value(s));
    }
    let total = (target - cur) as usize;
    // Build `n` bytes by cycling through `pad` (a partial final copy is allowed).
    let make = |n: usize| -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            let take = (n - v.len()).min(pad.len());
            v.extend_from_slice(&pad[..take]);
        }
        v
    };
    let out = match ptype {
        0 => {
            let mut v = make(total);
            v.extend_from_slice(&s);
            v
        }
        2 => {
            let left = total / 2;
            let right = total - left;
            let mut v = make(left);
            v.extend_from_slice(&s);
            v.extend_from_slice(&make(right));
            v
        }
        _ => {
            let mut v = s.clone();
            v.extend_from_slice(&make(total));
            v
        }
    };
    Ok(str_value(out))
}

pub(crate) fn str_split(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let size = args.get(1).map_or(1, Value::to_int);
    if size < 1 {
        return Err(NativeError::new(
            "str_split(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let size = size as usize;
    let mut out = Array::new();
    // PHP 8.2+: an empty subject yields an empty array (not `[""]`).
    if s.is_empty() {
        return Ok(Value::Array(out));
    }
    let mut i = 0;
    while i < s.len() {
        let end = (i + size).min(s.len());
        out.push(str_value(s[i..end].to_vec()));
        i = end;
    }
    Ok(Value::Array(out))
}

pub(crate) fn substr_count(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    if needle.is_empty() {
        return Err(NativeError::new(
            "substr_count(): Argument #2 ($needle) must not be empty",
        ));
    }
    let n = haystack.len() as i64;
    // Resolve the [offset, end) window the way PHP does (negatives count from
    // the end), erroring when the window escapes the string.
    let mut offset = args.get(2).map_or(0, Value::to_int);
    if offset < 0 {
        offset += n;
    }
    if offset < 0 || offset > n {
        return Err(NativeError::new(
            "substr_count(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
        ));
    }
    let end = match args.get(3) {
        None | Some(Value::Null) => n,
        Some(l) => {
            let l = l.to_int();
            let e = if l < 0 { n + l } else { offset + l };
            if e < offset || e > n {
                return Err(NativeError::new(
                    "substr_count(): Argument #4 ($length) must be contained in argument #1 ($haystack)",
                ));
            }
            e
        }
    };
    let window = &haystack[offset as usize..end as usize];
    let mut count = 0i64;
    let mut rest = window;
    while let Some(pos) = find(rest, &needle) {
        count += 1;
        rest = &rest[pos + needle.len()..];
    }
    Ok(Value::Int(count))
}

/// Shared backend for `strrpos`/`strripos`: the last match at or before the
/// offset-derived window, searching right to left.
fn rpos_impl(args: &[Value], ci: bool, name: &str) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let n = haystack.len() as i64;
    let nl = needle.len() as i64;
    let offset = args.get(2).map_or(0, Value::to_int);
    // `hi` is the greatest start position considered; `lo` the least.
    let (lo, mut hi) = if offset >= 0 {
        if offset > n {
            return Err(NativeError::new(format!(
                "{name}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
            )));
        }
        (offset, n - nl)
    } else {
        if -offset > n {
            return Err(NativeError::new(format!(
                "{name}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
            )));
        }
        // A negative offset skips that many trailing bytes from the search.
        let hi = if -offset < nl { n - nl } else { n + offset };
        (0, hi)
    };
    if hi > n - nl {
        hi = n - nl;
    }
    if hi < lo || hi < 0 {
        return Ok(Value::Bool(false));
    }
    let (hb, nb) = if ci {
        (ascii_lower(&haystack), ascii_lower(&needle))
    } else {
        (haystack.clone(), needle.clone())
    };
    let mut p = hi;
    while p >= lo {
        let start = p as usize;
        if hb[start..start + nl as usize] == nb[..] {
            return Ok(Value::Int(p));
        }
        p -= 1;
    }
    Ok(Value::Bool(false))
}

pub(crate) fn strrpos(_: &mut Ctx, args: &[Value]) -> NativeResult {
    rpos_impl(args, false, "strrpos")
}

pub(crate) fn strripos(_: &mut Ctx, args: &[Value]) -> NativeResult {
    rpos_impl(args, true, "strripos")
}

pub(crate) fn stripos(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let n = haystack.len() as i64;
    let mut start = args.get(2).map_or(0, Value::to_int);
    if start < 0 {
        start += n;
    }
    if start < 0 || start > n {
        return Err(NativeError::new(
            "stripos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
        ));
    }
    let hb = ascii_lower(&haystack);
    let nb = ascii_lower(&needle);
    match find(&hb[start as usize..], &nb) {
        Some(pos) => Ok(Value::Int(start + pos as i64)),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn strstr(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let before = args.get(2).is_some_and(Value::to_bool);
    match find(&haystack, &needle) {
        Some(pos) if before => Ok(str_value(haystack[..pos].to_vec())),
        Some(pos) => Ok(str_value(haystack[pos..].to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn stristr(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let before = args.get(2).is_some_and(Value::to_bool);
    // Locate case-insensitively, but return a slice of the original bytes.
    let hb = ascii_lower(&haystack);
    let nb = ascii_lower(&needle);
    match find(&hb, &nb) {
        Some(pos) if before => Ok(str_value(haystack[..pos].to_vec())),
        Some(pos) => Ok(str_value(haystack[pos..].to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn strrchr(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    // Only the first byte of the needle is significant; an empty needle fails.
    let Some(&first) = needle.first() else {
        return Ok(Value::Bool(false));
    };
    match haystack.iter().rposition(|&c| c == first) {
        Some(pos) => Ok(str_value(haystack[pos..].to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn strpbrk(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let charlist = bytes(&args[1]);
    match haystack.iter().position(|c| charlist.contains(c)) {
        Some(pos) => Ok(str_value(haystack[pos..].to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

/// PHP's binary string comparison: the signed difference of the first differing
/// bytes (case-folded when `ci`), or the sign of the length difference when one
/// is a prefix of the other. Mirrors `zend_binary_strcmp` byte-for-byte on this
/// build (the differing-byte magnitude is the platform `memcmp` value).
fn cmp_bytes(a: &[u8], b: &[u8], ci: bool) -> i64 {
    let n = a.len().min(b.len());
    for i in 0..n {
        let (x, y) = if ci {
            (a[i].to_ascii_lowercase(), b[i].to_ascii_lowercase())
        } else {
            (a[i], b[i])
        };
        if x != y {
            return x as i64 - y as i64;
        }
    }
    (a.len() as i64 - b.len() as i64).signum()
}

pub(crate) fn strcmp(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Int(cmp_bytes(&bytes(&args[0]), &bytes(&args[1]), false)))
}

pub(crate) fn strcasecmp(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Int(cmp_bytes(&bytes(&args[0]), &bytes(&args[1]), true)))
}

fn ncmp_impl(args: &[Value], ci: bool, name: &str) -> NativeResult {
    let a = bytes(&args[0]);
    let b = bytes(&args[1]);
    let len = args[2].to_int();
    if len < 0 {
        return Err(NativeError::new(format!(
            "{name}(): Argument #3 ($length) must be greater than or equal to 0"
        )));
    }
    let len = len as usize;
    // Compare at most `len` bytes; the length tiebreak also caps at `len`.
    let a2 = &a[..a.len().min(len)];
    let b2 = &b[..b.len().min(len)];
    Ok(Value::Int(cmp_bytes(a2, b2, ci)))
}

pub(crate) fn strncmp(_: &mut Ctx, args: &[Value]) -> NativeResult {
    ncmp_impl(args, false, "strncmp")
}

pub(crate) fn strncasecmp(_: &mut Ctx, args: &[Value]) -> NativeResult {
    ncmp_impl(args, true, "strncasecmp")
}

fn hex_digit(n: u8) -> u8 {
    if n < 10 {
        b'0' + n
    } else {
        b'a' + (n - 10)
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn bin2hex(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let b = bytes(&args[0]);
    let mut out = Vec::with_capacity(b.len() * 2);
    for &byte in &b {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0xf));
    }
    Ok(str_value(out))
}

pub(crate) fn hex2bin(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let b = bytes(&args[0]);
    // PHP warns and returns false on odd length or a non-hex byte; we mirror the
    // (stdout-visible) `false` result without the warning.
    if !b.len().is_multiple_of(2) {
        return Ok(Value::Bool(false));
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        let (Some(hi), Some(lo)) = (hex_val(b[i]), hex_val(b[i + 1])) else {
            return Ok(Value::Bool(false));
        };
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(str_value(out))
}

pub(crate) fn nl2br(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let xhtml = args.get(1).is_none_or(Value::to_bool);
    let br: &[u8] = if xhtml { b"<br />" } else { b"<br>" };
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'\r' || c == b'\n' {
            out.extend_from_slice(br);
            // Treat "\r\n" / "\n\r" as a single break, copied through verbatim.
            if i + 1 < s.len() && (s[i + 1] == b'\r' || s[i + 1] == b'\n') && s[i + 1] != c {
                out.push(c);
                out.push(s[i + 1]);
                i += 2;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    Ok(str_value(out))
}

pub(crate) fn strtr(_: &mut Ctx, args: &[Value]) -> NativeResult {
    if args.len() == 2 {
        // Array form: replace whole substrings, longest key first, one
        // left-to-right pass (replaced regions are never re-scanned).
        let Value::Array(map) = &args[1] else {
            return Err(NativeError::new(format!(
                "strtr(): Argument #2 ($from) must be of type array, {} given",
                args[1].type_name()
            )));
        };
        let subject = bytes(&args[0]);
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (k, v) in map.iter() {
            let kb = k.to_value().to_php_bytes();
            if kb.is_empty() {
                continue; // PHP ignores an empty-string key.
            }
            pairs.push((kb, v.to_php_bytes()));
        }
        // Longest keys win; the sort is stable so equal-length insertion order
        // is preserved (immaterial, since equal-length keys can't both match).
        pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
        let mut out = Vec::with_capacity(subject.len());
        let mut i = 0;
        'outer: while i < subject.len() {
            for (kb, vb) in &pairs {
                if subject[i..].starts_with(kb) {
                    out.extend_from_slice(vb);
                    i += kb.len();
                    continue 'outer;
                }
            }
            out.push(subject[i]);
            i += 1;
        }
        Ok(str_value(out))
    } else {
        // Char form: map single bytes; uses min(|from|,|to|) pairs, and on a
        // duplicate source byte the last mapping wins (matches php-src).
        let subject = bytes(&args[0]);
        let from = bytes(&args[1]);
        let to = bytes(&args[2]);
        let len = from.len().min(to.len());
        let mut map: [u8; 256] = core::array::from_fn(|i| i as u8);
        for idx in 0..len {
            map[from[idx] as usize] = to[idx];
        }
        let out: Vec<u8> = subject.iter().map(|&c| map[c as usize]).collect();
        Ok(str_value(out))
    }
}

pub(crate) fn substr_replace(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let replace = bytes(&args[1]);
    let n = s.len() as i64;
    // `start`/`length` follow substr()'s negative-from-the-end semantics.
    let mut start = args[2].to_int();
    if start < 0 {
        start = (n + start).max(0);
    } else {
        start = start.min(n);
    }
    let end = match args.get(3) {
        None | Some(Value::Null) => n,
        Some(l) => {
            let l = l.to_int();
            if l < 0 {
                (n + l).max(start)
            } else {
                (start + l).min(n)
            }
        }
    };
    let mut out = Vec::with_capacity(s.len() + replace.len());
    out.extend_from_slice(&s[..start as usize]);
    out.extend_from_slice(&replace);
    out.extend_from_slice(&s[end as usize..]);
    Ok(str_value(out))
}

pub(crate) fn quotemeta(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(s.len());
    for &c in &s {
        if matches!(
            c,
            b'.' | b'\\' | b'+' | b'*' | b'?' | b'[' | b'^' | b']' | b'$' | b'(' | b')'
        ) {
            out.push(b'\\');
        }
        out.push(c);
    }
    Ok(str_value(out))
}

pub(crate) fn addslashes(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(s.len());
    for &c in &s {
        match c {
            b'\'' | b'"' | b'\\' => {
                out.push(b'\\');
                out.push(c);
            }
            // The NUL byte is escaped as the two characters backslash + '0'.
            0 => {
                out.push(b'\\');
                out.push(b'0');
            }
            _ => out.push(c),
        }
    }
    Ok(str_value(out))
}

pub(crate) fn stripslashes(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' {
            i += 1;
            if i >= s.len() {
                break; // a trailing backslash is dropped
            }
            // "\0" decodes to a NUL byte; "\X" decodes to the literal X.
            out.push(if s[i] == b'0' { 0 } else { s[i] });
            i += 1;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    Ok(str_value(out))
}

/// Round `value` to `dec` fractional digits, rounding halves away from zero,
/// and split it into (negative?, integer-digit bytes, fractional-digit bytes).
/// Works on the *shortest* decimal that round-trips to the f64 (via `{:e}`),
/// matching PHP's number_format intent-based rounding (e.g. 1.005 -> 1.01).
fn format_decimal(num: f64, dec: usize) -> (bool, Vec<u8>, Vec<u8>) {
    let neg = num < 0.0;
    let value = num.abs();
    if value == 0.0 {
        return (false, vec![b'0'], vec![b'0'; dec]);
    }
    let sci = format!("{:e}", value);
    let (mant, exp) = sci.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let mut digits: Vec<u8> = mant.bytes().filter(|&b| b != b'.').collect();
    // `point_pos` is the count of significant digits left of the decimal point.
    let mut point_pos: i32 = e + 1;
    let keep = point_pos + dec as i32;
    if keep < 0 {
        return (false, vec![b'0'], vec![b'0'; dec]);
    }
    let keep = keep as usize;
    if keep < digits.len() {
        let round_up = digits[keep] >= b'5';
        digits.truncate(keep);
        if round_up {
            let mut i = keep as i32 - 1;
            loop {
                if i < 0 {
                    digits.insert(0, b'1');
                    point_pos += 1;
                    break;
                }
                if digits[i as usize] == b'9' {
                    digits[i as usize] = b'0';
                    i -= 1;
                } else {
                    digits[i as usize] += 1;
                    break;
                }
            }
        }
    } else {
        while digits.len() < keep {
            digits.push(b'0');
        }
    }
    // Integer part: the first `point_pos` digits (zero-filled if needed).
    let mut intpart = Vec::new();
    if point_pos <= 0 {
        intpart.push(b'0');
    } else {
        for i in 0..point_pos as usize {
            intpart.push(*digits.get(i).unwrap_or(&b'0'));
        }
    }
    // Fractional part: the next `dec` digits.
    let mut fracpart = Vec::new();
    for p in 0..dec {
        let idx = point_pos + p as i32;
        let d = if idx >= 0 {
            *digits.get(idx as usize).unwrap_or(&b'0')
        } else {
            b'0'
        };
        fracpart.push(d);
    }
    let all_zero =
        intpart.iter().all(|&b| b == b'0') && fracpart.iter().all(|&b| b == b'0');
    (neg && !all_zero, intpart, fracpart)
}

pub(crate) fn number_format(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let num = args[0].to_float();
    // PHP prints non-finite values as a bare "nan"/"inf" (no sign, no grouping).
    if num.is_nan() {
        return Ok(str_value(b"nan".to_vec()));
    }
    if num.is_infinite() {
        return Ok(str_value(b"inf".to_vec()));
    }
    let dec = args.get(1).map_or(0, Value::to_int).max(0) as usize;
    let dec_point = match args.get(2) {
        Some(v) => bytes(v),
        None => vec![b'.'],
    };
    let thousands = match args.get(3) {
        Some(v) => bytes(v),
        None => vec![b','],
    };
    let (neg, intpart, fracpart) = format_decimal(num, dec);
    let mut out = Vec::new();
    if neg {
        out.push(b'-');
    }
    let len = intpart.len();
    for (i, &b) in intpart.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.extend_from_slice(&thousands);
        }
        out.push(b);
    }
    if dec > 0 {
        out.extend_from_slice(&dec_point);
        out.extend_from_slice(&fracpart);
    }
    Ok(str_value(out))
}

pub(crate) fn str_word_count(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let format = args.get(1).map_or(0, Value::to_int);
    let extra = match args.get(2) {
        Some(v) => bytes(v),
        None => Vec::new(),
    };
    let is_letter = |c: u8| c.is_ascii_alphabetic() || extra.contains(&c);
    // "'" and "-" join a word only when surrounded by letters, so a word ends at
    // its last letter (any trailing connectors are consumed but dropped).
    let is_inner = |c: u8| c == b'\'' || c == b'-';
    let mut words: Vec<(usize, &[u8])> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if is_letter(s[i]) {
            let start = i;
            let mut last = i;
            i += 1;
            while i < s.len() && (is_letter(s[i]) || is_inner(s[i])) {
                if is_letter(s[i]) {
                    last = i;
                }
                i += 1;
            }
            words.push((start, &s[start..=last]));
        } else {
            i += 1;
        }
    }
    match format {
        0 => Ok(Value::Int(words.len() as i64)),
        1 => {
            let mut out = Array::new();
            for (_, w) in &words {
                out.push(str_value(w.to_vec()));
            }
            Ok(Value::Array(out))
        }
        2 => {
            let mut out = Array::new();
            for (pos, w) in &words {
                out.set(ArrayKey::Int(*pos as i64), str_value(w.to_vec()));
            }
            Ok(Value::Array(out))
        }
        _ => Err(NativeError::new(
            "str_word_count(): Argument #2 ($format) must be a valid format value",
        )),
    }
}

// ---- printf family ----------------------------------------------------------

/// Lay out `%e`/`%E`: PHP uses a signed exponent with no leading zeros and a
/// minimum of one digit (e.g. `1.234568e+4`), unlike C's two-digit exponent.
fn format_exp(value: f64, prec: usize, upper: bool) -> Vec<u8> {
    let s = format!("{:.*e}", prec, value);
    let (mant, exp) = s.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let echar = if upper { b'E' } else { b'e' };
    let mut out = mant.as_bytes().to_vec();
    out.push(echar);
    out.push(if e < 0 { b'-' } else { b'+' });
    out.extend_from_slice(e.unsigned_abs().to_string().as_bytes());
    out
}

/// PHP's `%g`/`%G` (modeled on php_gcvt + zend_dtoa mode 2): render with at most
/// `ndigit` significant digits, switching to exponential form when the decimal
/// exponent is `< -4` or `>= ndigit`. Trailing zeros are dropped — except that an
/// exact half-way value rounded down to `ndigit` digits keeps them (so `%.4g` of
/// 71905 is `"7.190e+4"`), matching zend_dtoa. Exponential mantissas force a `.0`.
fn php_gcvt(value: f64, ndigit: usize, upper: bool) -> Vec<u8> {
    if value == 0.0 {
        return vec![b'0'];
    }
    let exp_char = if upper { b'E' } else { b'e' };
    // Shortest round-tripping significant digits. An exact tie at `ndigit` shows
    // up here as exactly `ndigit + 1` digits ending in '5'.
    let short = format!("{:e}", value);
    let (sm, _) = short.split_once('e').expect("LowerExp always has 'e'");
    let mut sd: Vec<u8> = sm.bytes().filter(|&b| b != b'.').collect();
    while sd.len() > 1 && *sd.last().unwrap() == b'0' {
        sd.pop();
    }
    let is_tie = sd.len() == ndigit + 1 && sd[ndigit] == b'5';
    // Round to exactly `ndigit` significant digits.
    let sci = format!("{:.*e}", ndigit.saturating_sub(1), value);
    let (mant, exp) = sci.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let mut digits: Vec<u8> = mant.bytes().filter(|&b| b != b'.').collect();
    let decpt = e + 1;
    let nd = ndigit as i32;
    let e_style = (decpt >= 0 && decpt > nd) || decpt < -3;
    // Keep the rounding-induced trailing zeros only for an exact tie shown in
    // exponential form (never for a power-of-ten carry, whose tail is all zeros).
    let keep = is_tie && e_style && digits[1..].iter().any(|&b| b != b'0');
    if !keep {
        while digits.len() > 1 && *digits.last().unwrap() == b'0' {
            digits.pop();
        }
    }
    let mut out = Vec::new();
    if e_style {
        // Exponential: "d.ddde±X".
        let mut d = decpt - 1;
        let sign = if d < 0 {
            d = -d;
            b'-'
        } else {
            b'+'
        };
        out.push(digits[0]);
        out.push(b'.');
        if digits.len() == 1 {
            out.push(b'0');
        } else {
            out.extend_from_slice(&digits[1..]);
        }
        out.push(exp_char);
        out.push(sign);
        out.extend_from_slice(d.to_string().as_bytes());
    } else if decpt < 0 {
        // "0.00ddd": -decpt leading fractional zeros.
        out.push(b'0');
        out.push(b'.');
        out.resize(out.len() + (-decpt) as usize, b'0');
        out.extend_from_slice(&digits);
    } else {
        // Plain fixed notation.
        let dp = decpt as usize;
        let mut idx = 0;
        for _ in 0..dp {
            out.push(*digits.get(idx).unwrap_or(&b'0'));
            if idx < digits.len() {
                idx += 1;
            }
        }
        if idx < digits.len() {
            if idx == 0 {
                out.push(b'0');
            }
            out.push(b'.');
            out.extend_from_slice(&digits[idx..]);
        }
    }
    out
}

/// The shared engine behind `sprintf`/`printf`/`vsprintf`. Operates entirely on
/// bytes so binary strings round-trip; `args` are the values after the format.
fn do_sprintf(format: &[u8], args: &[Value]) -> Result<Vec<u8>, NativeError> {
    let mut out = Vec::new();
    let mut argi = 0usize;
    let n = format.len();
    let mut i = 0;
    while i < n {
        if format[i] != b'%' {
            out.push(format[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i < n && format[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }
        // Optional positional "N$".
        let mut explicit: Option<usize> = None;
        {
            let mut j = i;
            while j < n && format[j].is_ascii_digit() {
                j += 1;
            }
            if j > i && j < n && format[j] == b'$' {
                explicit = std::str::from_utf8(&format[i..j]).ok().and_then(|s| s.parse().ok());
                i = j + 1;
            }
        }
        // Flags.
        let mut left = false;
        let mut plus = false;
        let mut zero = false;
        let mut pad = b' ';
        let mut custom_pad = false;
        loop {
            match format.get(i) {
                Some(b'-') => left = true,
                Some(b'+') => plus = true,
                Some(b' ') => {} // PHP's space flag means "pad with spaces" (default).
                Some(b'0') => zero = true,
                Some(b'\'') => {
                    if let Some(&p) = format.get(i + 1) {
                        pad = p;
                        custom_pad = true;
                        i += 2;
                        continue;
                    }
                }
                _ => break,
            }
            i += 1;
        }
        // Width.
        let mut width = 0usize;
        while i < n && format[i].is_ascii_digit() {
            width = width * 10 + (format[i] - b'0') as usize;
            i += 1;
        }
        // Precision.
        let mut precision: Option<usize> = None;
        if i < n && format[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            while i < n && format[i].is_ascii_digit() {
                p = p * 10 + (format[i] - b'0') as usize;
                i += 1;
            }
            precision = Some(p);
        }
        let Some(&conv) = format.get(i) else {
            break; // a dangling '%' at end of format
        };
        i += 1;

        // Resolve the argument for this conversion.
        let arg: Value = match explicit {
            Some(num) => {
                if num == 0 || num > args.len() {
                    return Err(NativeError::new(format!(
                        "{num} arguments are required, {} given",
                        args.len()
                    )));
                }
                args[num - 1].clone()
            }
            None => {
                let Some(a) = args.get(argi) else {
                    return Err(NativeError::new(format!(
                        "{} arguments are required, {} given",
                        argi + 1,
                        args.len()
                    )));
                };
                argi += 1;
                a.clone()
            }
        };

        let signed = |neg: bool| -> Vec<u8> {
            if neg {
                vec![b'-']
            } else if plus {
                vec![b'+']
            } else {
                Vec::new()
            }
        };

        let (prefix, body): (Vec<u8>, Vec<u8>) = match conv {
            b's' => {
                let mut b = arg.to_php_bytes();
                if let Some(p) = precision {
                    if b.len() > p {
                        b.truncate(p);
                    }
                }
                (Vec::new(), b)
            }
            b'd' | b'i' => {
                let v = arg.to_int();
                let mag = (v as i128).unsigned_abs();
                (signed(v < 0), mag.to_string().into_bytes())
            }
            b'u' => (Vec::new(), (arg.to_int() as u64).to_string().into_bytes()),
            b'x' => (Vec::new(), format!("{:x}", arg.to_int() as u64).into_bytes()),
            b'X' => (Vec::new(), format!("{:X}", arg.to_int() as u64).into_bytes()),
            b'o' => (Vec::new(), format!("{:o}", arg.to_int() as u64).into_bytes()),
            b'b' => (Vec::new(), format!("{:b}", arg.to_int() as u64).into_bytes()),
            b'c' => (Vec::new(), vec![(arg.to_int() & 0xff) as u8]),
            b'f' | b'F' => {
                let f = arg.to_float();
                let prec = precision.unwrap_or(6);
                (signed(f < 0.0), format!("{:.*}", prec, f.abs()).into_bytes())
            }
            b'e' | b'E' => {
                let f = arg.to_float();
                let prec = precision.unwrap_or(6);
                (signed(f < 0.0), format_exp(f.abs(), prec, conv == b'E'))
            }
            b'g' | b'G' => {
                let f = arg.to_float();
                let nd = precision.unwrap_or(6).max(1);
                (signed(f < 0.0), php_gcvt(f.abs(), nd, conv == b'G'))
            }
            other => {
                return Err(NativeError::new(format!(
                    "Unknown format specifier \"{}\"",
                    other as char
                )));
            }
        };

        // Apply width with the chosen padding.
        let content = prefix.len() + body.len();
        if content >= width {
            out.extend_from_slice(&prefix);
            out.extend_from_slice(&body);
        } else {
            let padlen = width - content;
            if left {
                // Left-justify pads on the right (zero flag reverts to spaces).
                let pc = if custom_pad { pad } else { b' ' };
                out.extend_from_slice(&prefix);
                out.extend_from_slice(&body);
                out.extend(std::iter::repeat_n(pc, padlen));
            } else if custom_pad {
                out.extend(std::iter::repeat_n(pad, padlen));
                out.extend_from_slice(&prefix);
                out.extend_from_slice(&body);
            } else if zero {
                // Zero padding sits after the sign.
                out.extend_from_slice(&prefix);
                out.extend(std::iter::repeat_n(b'0', padlen));
                out.extend_from_slice(&body);
            } else {
                out.extend(std::iter::repeat_n(b' ', padlen));
                out.extend_from_slice(&prefix);
                out.extend_from_slice(&body);
            }
        }
    }
    Ok(out)
}

pub(crate) fn sprintf(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let format = bytes(&args[0]);
    Ok(str_value(do_sprintf(&format, &args[1..])?))
}

pub(crate) fn printf(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let format = bytes(&args[0]);
    let rendered = do_sprintf(&format, &args[1..])?;
    let len = rendered.len() as i64;
    ctx.out().extend_from_slice(&rendered);
    Ok(Value::Int(len))
}

pub(crate) fn vsprintf(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let format = bytes(&args[0]);
    let Value::Array(arr) = &args[1] else {
        return Err(NativeError::new(format!(
            "vsprintf(): Argument #2 ($values) must be of type array, {} given",
            args[1].type_name()
        )));
    };
    let vals: Vec<Value> = arr.iter().map(|(_, v)| v.clone()).collect();
    Ok(str_value(do_sprintf(&format, &vals)?))
}
