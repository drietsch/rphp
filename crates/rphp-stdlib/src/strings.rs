//! String builtins, all **byte-oriented** (PHP strings are byte strings, never
//! assumed UTF-8). ASCII case mapping is locale-independent, matching PHP 8's
//! `strtoupper`/`strtolower`. Multibyte-aware variants belong to `mbstring`
//! (ICU-backed) and are a separate extension.
use rphp_value::{Array, Str, Value};

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
