//! String builtins, all **byte-oriented** (PHP strings are byte strings, never
//! assumed UTF-8). ASCII case mapping is locale-independent, matching PHP 8's
//! `strtoupper`/`strtolower`. Multibyte-aware variants belong to `mbstring`
//! (ICU-backed) and are a separate extension.
use rphp_value::{Array, ArrayKey, Str, Value};

use rphp_runtime::{Ctx, NativeFn, NativeResult, nf, nf_ref, Unwind};

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
    nf_ref!("str_replace", 3, Some(4), 0b1000, str_replace),
    nf_ref!("str_ireplace", 3, Some(4), 0b1000, str_ireplace),
    nf!("trim", 1, Some(2), trim),
    nf!("ltrim", 1, Some(2), ltrim),
    nf!("rtrim", 1, Some(2), rtrim),
    nf!("chop", 1, Some(2), rtrim),
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
    nf!("strrchr", 2, Some(3), strrchr),
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
];

/// The byte string an argument coerces to (the `(string)` cast). Lets every
/// builtin accept any scalar the way PHP's weak typing does.
fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

/// Wrap owned bytes as a string value.
pub(crate) fn str_value(bytes: Vec<u8>) -> Value {
    Value::Str(Str::from_vec(bytes))
}

/// php's `php_charmask`: expand a character list with `a..z` ranges into a
/// 256-entry membership table, warning (as `$func`) about malformed ranges
/// exactly as php does — the malformed pieces still contribute their bytes.
pub(crate) fn charmask(ctx: &mut Ctx, func: &str, input: &[u8]) -> Result<[bool; 256], Unwind> {
    let mut mask = [false; 256];
    let n = input.len();
    let mut i = 0;
    while i < n {
        let c = input[i];
        if i + 3 < n && input[i + 1] == b'.' && input[i + 2] == b'.' && input[i + 3] >= c {
            for b in c..=input[i + 3] {
                mask[b as usize] = true;
            }
            i += 4;
            continue;
        }
        if i + 1 < n && input[i] == b'.' && input[i + 1] == b'.' {
            let what = if i == 0 {
                "no character to the left of '..'"
            } else if i + 2 >= n {
                "no character to the right of '..'"
            } else if input[i - 1] > input[i + 2] {
                "'..'-range needs to be incrementing"
            } else {
                ""
            };
            if what.is_empty() {
                ctx.warn(&format!("{func}(): Invalid '..'-range"))?;
            } else {
                ctx.warn(&format!("{func}(): Invalid '..'-range, {what}"))?;
            }
            i += 1;
            continue;
        }
        mask[c as usize] = true;
        i += 1;
    }
    Ok(mask)
}

pub(crate) fn strlen(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(bytes(&args[0]).len() as i64))
}

pub(crate) fn strtoupper(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    b.make_ascii_uppercase();
    Ok(str_value(b))
}

pub(crate) fn strtolower(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    b.make_ascii_lowercase();
    Ok(str_value(b))
}

pub(crate) fn ucfirst(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    if let Some(first) = b.first_mut() {
        first.make_ascii_uppercase();
    }
    Ok(str_value(b))
}

pub(crate) fn lcfirst(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    if let Some(first) = b.first_mut() {
        first.make_ascii_lowercase();
    }
    Ok(str_value(b))
}

pub(crate) fn str_repeat(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let times = args[1].to_int();
    if times < 0 {
        return Err(Unwind::value_error(
            "str_repeat(): Argument #2 ($times) must be greater than or equal to 0",
        ));
    }
    Ok(str_value(s.repeat(times as usize)))
}

pub(crate) fn substr(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let n = s.len() as i64;
    let mut start = args[1].to_int();
    if start < 0 {
        start = n.saturating_add(start).max(0);
    } else {
        start = start.min(n);
    }
    let end = match args.get(2) {
        // `null` length (or absent) means "to the end".
        None | Some(Value::Null) => n,
        Some(len) => {
            let l = len.to_int();
            if l < 0 {
                n.saturating_add(l).max(start)
            } else {
                start.saturating_add(l).min(n)
            }
        }
    };
    if end <= start {
        Ok(str_value(Vec::new()))
    } else {
        Ok(str_value(s[start as usize..end as usize].to_vec()))
    }
}

pub(crate) fn strpos(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let n = haystack.len() as i64;
    let mut start = args.get(2).map_or(0, Value::to_int);
    if start < 0 {
        start += n;
    }
    if start < 0 || start > n {
        return Err(Unwind::value_error(
            "strpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
        ));
    }
    match find(&haystack[start as usize..], &needle) {
        Some(pos) => Ok(Value::Int(start + pos as i64)),
        None => Ok(Value::Bool(false)),
    }
}

/// Replace every occurrence of `search` in `subject`, counting the hits; the
/// match is byte-exact, or ASCII case-insensitive when `ci`.
fn replace_counted(subject: &[u8], search: &[u8], replace: &[u8], ci: bool, count: &mut i64) -> Vec<u8> {
    if search.is_empty() {
        return subject.to_vec();
    }
    let hay: std::borrow::Cow<[u8]> = if ci { ascii_lower(subject).into() } else { subject.into() };
    let needle: std::borrow::Cow<[u8]> = if ci { ascii_lower(search).into() } else { search.into() };
    let mut out = Vec::with_capacity(subject.len());
    let mut i = 0;
    while let Some(pos) = find(&hay[i..], &needle) {
        out.extend_from_slice(&subject[i..i + pos]);
        out.extend_from_slice(replace);
        i += pos + search.len();
        *count += 1;
    }
    out.extend_from_slice(&subject[i..]);
    out
}

/// Apply one (search, replace) plan to a single subject string.
fn replace_subject(ctx: &mut Ctx, subject: &[u8], plan: &[(Vec<u8>, Vec<u8>)], ci: bool, count: &mut i64) -> Result<Vec<u8>, Unwind> {
    let mut cur = subject.to_vec();
    for (search, replace) in plan {
        cur = replace_counted(&cur, search, replace, ci, count);
    }
    let _ = ctx;
    Ok(cur)
}

/// Shared body of `str_replace`/`str_ireplace`: array or string search and
/// replace, array or string subject, `&$count` in position 3.
fn str_replace_impl(ctx: &mut Ctx, args: &mut [Value], func: &str, ci: bool) -> NativeResult {
    let search = args[0].deref().into_owned();
    let replace = args[1].deref().into_owned();
    // The list of (search, replace) pairs applied in order.
    let plan: Vec<(Vec<u8>, Vec<u8>)> = match (&search, &replace) {
        (Value::Array(s), Value::Array(r)) => {
            let reps: Vec<Vec<u8>> = r.iter().map(|(_, v)| v.to_php_bytes()).collect();
            s.iter()
                .enumerate()
                .map(|(i, (_, sv))| (sv.to_php_bytes(), reps.get(i).cloned().unwrap_or_default()))
                .collect()
        }
        (Value::Array(s), r) => {
            let rb = r.to_php_bytes();
            s.iter().map(|(_, sv)| (sv.to_php_bytes(), rb.clone())).collect()
        }
        (_, Value::Array(_)) => {
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #2 ($replace) must be of type string when argument #1 ($search) is a string"
            )))
        }
        (s, r) => vec![(s.to_php_bytes(), r.to_php_bytes())],
    };
    let mut count = 0i64;
    let result = match args[2].deref().into_owned() {
        Value::Array(subjects) => {
            let mut out = Array::new();
            for (k, v) in subjects.iter() {
                let v = v.deref().into_owned();
                if matches!(v, Value::Array(_)) {
                    ctx.warn("Array to string conversion")?;
                }
                let replaced = replace_subject(ctx, &v.to_php_bytes(), &plan, ci, &mut count)?;
                out.set(k.clone(), str_value(replaced));
            }
            Value::Array(out)
        }
        v => str_value(replace_subject(ctx, &v.to_php_bytes(), &plan, ci, &mut count)?),
    };
    if args.len() > 3 {
        args[3] = Value::Int(count);
    }
    Ok(result)
}

/// `str_replace(array|string $search, array|string $replace, string|array
/// $subject, &$count = null): string|array`.
pub(crate) fn str_replace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    str_replace_impl(ctx, args, "str_replace", false)
}

/// `str_ireplace(...)`: the ASCII case-insensitive twin of `str_replace`.
pub(crate) fn str_ireplace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    str_replace_impl(ctx, args, "str_ireplace", true)
}

pub(crate) fn trim(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    trim_impl(ctx, "trim", args, true, true)
}

pub(crate) fn ltrim(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    trim_impl(ctx, "ltrim", args, true, false)
}

pub(crate) fn rtrim(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    trim_impl(ctx, "rtrim", args, false, true)
}

pub(crate) fn implode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // `implode($array)` (glue ""), `implode($glue, $array)`, and the legacy
    // reversed `implode($array, $glue)` order are all accepted.
    // PHP 8: `implode($array)` or `implode($separator, $array)`; the legacy
    // reversed order is gone and each shape has its own TypeError text.
    let a0 = args[0].deref().into_owned();
    let (glue, array) = if args.len() == 1 {
        match a0 {
            Value::Array(a) => (Vec::new(), a),
            _ => {
                return Err(Unwind::type_error(
                    "implode(): If argument #1 ($separator) is of type string, argument #2 ($array) must be of type array, null given",
                ))
            }
        }
    } else {
        if matches!(a0, Value::Array(_)) {
            return Err(Unwind::type_error(
                "implode(): Argument #1 ($separator) must be of type string, array given",
            ));
        }
        match args[1].deref().into_owned() {
            Value::Array(a) => (bytes(&a0), a),
            Value::Null => {
                return Err(Unwind::type_error(
                    "implode(): If argument #1 ($separator) is of type string, argument #2 ($array) must be of type array, null given",
                ))
            }
            other => {
                return Err(Unwind::type_error(format!(
                    "implode(): Argument #2 ($array) must be of type ?array, {} given",
                    other.type_name()
                )))
            }
        }
    };
    let mut out = Vec::new();
    for (i, (_, v)) in array.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(&glue);
        }
        match &*v.deref() {
            Value::Array(_) => {
                ctx.warn("Array to string conversion")?;
                v.append_php_bytes(&mut out);
            }
            // A `Stringable` element is its `__toString()`; any other object
            // is php's "could not be converted to string" error.
            Value::Object(_) => out.extend_from_slice(ctx.to_string(&v)?.as_bytes()),
            _ => v.append_php_bytes(&mut out),
        }
    }
    Ok(str_value(out))
}

pub(crate) fn explode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let sep = bytes(&args[0]);
    let subject = bytes(&args[1]);
    if sep.is_empty() {
        return Err(Unwind::value_error(
            "explode(): Argument #1 ($separator) cannot be empty",
        ));
    }
    let mut limit = args.get(2).map_or(i64::MAX, Value::to_int);
    if limit == 0 {
        limit = 1;
    }
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

pub(crate) fn ord(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let b = bytes(&args[0]);
    // PHP 8.5 deprecates anything but a one-byte string.
    if b.is_empty() {
        ctx.deprecated("ord(): Providing an empty string is deprecated")?;
    } else if b.len() > 1 {
        ctx.deprecated("ord(): Providing a string that is not one byte long is deprecated. Use ord($str[0]) instead")?;
    }
    Ok(Value::Int(b.first().copied().unwrap_or(0) as i64))
}

pub(crate) fn chr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // PHP reduces the codepoint modulo 256 (with a deprecation since 8.5).
    let v = args[0].to_int();
    if !(0..=255).contains(&v) {
        ctx.deprecated("chr(): Providing a value not in-between 0 and 255 is deprecated, this is because a byte value must be in the [0, 255] interval. The value used will be constrained using % 256")?;
    }
    Ok(str_value(vec![v.rem_euclid(256) as u8]))
}

pub(crate) fn str_contains(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    Ok(Value::Bool(find(&haystack, &needle).is_some()))
}

pub(crate) fn str_starts_with(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    Ok(Value::Bool(haystack.starts_with(&needle)))
}

pub(crate) fn str_ends_with(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

fn trim_impl(ctx: &mut Ctx, func: &str, args: &[Value], left: bool, right: bool) -> NativeResult {
    let s = bytes(&args[0]);
    // Default trim set: " \t\n\r\0\x0B" (matches php-src); an explicit list
    // may carry `a..z` ranges.
    let mask = match args.get(1) {
        Some(c) => charmask(ctx, func, &bytes(c))?,
        None => {
            let mut m = [false; 256];
            for b in [b' ', b'\t', b'\n', b'\r', 0, 0x0b] {
                m[b as usize] = true;
            }
            m
        }
    };
    let in_set = |b: u8| mask[b as usize];
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

pub(crate) fn strrev(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut b = bytes(&args[0]);
    b.reverse();
    Ok(str_value(b))
}

pub(crate) fn ucwords(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn str_pad(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let target = args[1].to_int();
    let pad = match args.get(2) {
        Some(p) => bytes(p),
        None => vec![b' '],
    };
    // 0 = STR_PAD_LEFT, 1 = STR_PAD_RIGHT (default), 2 = STR_PAD_BOTH.
    let ptype = args.get(3).map_or(1, Value::to_int);
    if pad.is_empty() {
        return Err(Unwind::value_error(
            "str_pad(): Argument #3 ($pad_string) must not be empty",
        ));
    }
    if !(0..=2).contains(&ptype) {
        return Err(Unwind::value_error(
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

pub(crate) fn str_split(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let size = args.get(1).map_or(1, Value::to_int);
    if size < 1 {
        return Err(Unwind::value_error(
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

pub(crate) fn substr_count(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    if needle.is_empty() {
        return Err(Unwind::value_error(
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
        return Err(Unwind::value_error(
            "substr_count(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
        ));
    }
    let end = match args.get(3) {
        None | Some(Value::Null) => n,
        Some(l) => {
            let l = l.to_int();
            let e = if l < 0 { n + l } else { offset + l };
            if e < offset || e > n {
                return Err(Unwind::value_error(
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
            return Err(Unwind::value_error(format!(
                "{name}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
            )));
        }
        (offset, n - nl)
    } else {
        if -offset > n {
            return Err(Unwind::value_error(format!(
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

pub(crate) fn strrpos(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    rpos_impl(args, false, "strrpos")
}

pub(crate) fn strripos(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    rpos_impl(args, true, "strripos")
}

pub(crate) fn stripos(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let n = haystack.len() as i64;
    let mut start = args.get(2).map_or(0, Value::to_int);
    if start < 0 {
        start += n;
    }
    if start < 0 || start > n {
        return Err(Unwind::value_error(
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

pub(crate) fn strstr(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let before = args.get(2).is_some_and(Value::to_bool);
    match find(&haystack, &needle) {
        Some(pos) if before => Ok(str_value(haystack[..pos].to_vec())),
        Some(pos) => Ok(str_value(haystack[pos..].to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn stristr(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn strrchr(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let needle = bytes(&args[1]);
    let before = args.get(2).is_some_and(Value::to_bool);
    // Only the first byte of the needle is significant; an empty needle
    // searches for a NUL byte (its C terminator), as php does.
    let first = needle.first().copied().unwrap_or(0);
    match haystack.iter().rposition(|&c| c == first) {
        Some(pos) if before => Ok(str_value(haystack[..pos].to_vec())),
        Some(pos) => Ok(str_value(haystack[pos..].to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

pub(crate) fn strpbrk(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let haystack = bytes(&args[0]);
    let charlist = bytes(&args[1]);
    if charlist.is_empty() {
        return Err(Unwind::value_error(
            "strpbrk(): Argument #2 ($characters) must be a non-empty string",
        ));
    }
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

pub(crate) fn strcmp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(cmp_bytes(&bytes(&args[0]), &bytes(&args[1]), false)))
}

pub(crate) fn strcasecmp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(cmp_bytes(&bytes(&args[0]), &bytes(&args[1]), true)))
}

fn ncmp_impl(args: &[Value], ci: bool, name: &str) -> NativeResult {
    let a = bytes(&args[0]);
    let b = bytes(&args[1]);
    let len = args[2].to_int();
    if len < 0 {
        return Err(Unwind::value_error(format!(
            "{name}(): Argument #3 ($length) must be greater than or equal to 0"
        )));
    }
    let len = len as usize;
    // Compare at most `len` bytes; the length tiebreak also caps at `len`.
    let a2 = &a[..a.len().min(len)];
    let b2 = &b[..b.len().min(len)];
    Ok(Value::Int(cmp_bytes(a2, b2, ci)))
}

pub(crate) fn strncmp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ncmp_impl(args, false, "strncmp")
}

pub(crate) fn strncasecmp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn bin2hex(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let b = bytes(&args[0]);
    let mut out = Vec::with_capacity(b.len() * 2);
    for &byte in &b {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0xf));
    }
    Ok(str_value(out))
}

pub(crate) fn hex2bin(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let b = bytes(&args[0]);
    // PHP warns and returns false on odd length or a non-hex byte.
    if !b.len().is_multiple_of(2) {
        ctx.warn("hex2bin(): Hexadecimal input string must have an even length")?;
        return Ok(Value::Bool(false));
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        let (Some(hi), Some(lo)) = (hex_val(b[i]), hex_val(b[i + 1])) else {
            ctx.warn("hex2bin(): Input string must be hexadecimal string")?;
            return Ok(Value::Bool(false));
        };
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(str_value(out))
}

pub(crate) fn nl2br(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn strtr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if args.len() == 2 {
        // Array form: replace whole substrings, longest key first, one
        // left-to-right pass (replaced regions are never re-scanned).
        let Value::Array(map) = &args[1] else {
            return Err(Unwind::type_error(format!(
                "strtr(): Argument #2 ($from) must be of type array, {} given",
                args[1].type_name()
            )));
        };
        let subject = bytes(&args[0]);
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (k, v) in map.iter() {
            let kb = k.to_value().to_php_bytes();
            if kb.is_empty() {
                // PHP warns about and ignores an empty-string key.
                ctx.warn("strtr(): Ignoring replacement of empty string")?;
                continue;
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

/// Splice `replace` into `s` at substr()-style `start`/`length`.
fn splice_bytes(s: &[u8], replace: &[u8], start: i64, length: Option<i64>) -> Vec<u8> {
    let n = s.len() as i64;
    // `start`/`length` follow substr()'s negative-from-the-end semantics.
    let mut start = start;
    if start < 0 {
        start = n.saturating_add(start).max(0);
    } else {
        start = start.min(n);
    }
    let end = match length {
        None => n,
        Some(l) => {
            if l < 0 {
                n.saturating_add(l).max(start)
            } else {
                start.saturating_add(l).min(n)
            }
        }
    };
    let mut out = Vec::with_capacity(s.len() + replace.len());
    out.extend_from_slice(&s[..start as usize]);
    out.extend_from_slice(replace);
    out.extend_from_slice(&s[end as usize..]);
    out
}

/// `substr_replace(array|string $string, array|string $replace, array|int
/// $offset, array|int|null $length = null): string|array`.
pub(crate) fn substr_replace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let subject = args[0].deref().into_owned();
    let replace = args[1].deref().into_owned();
    let offset = args[2].deref().into_owned();
    let length = args.get(3).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if let Some(l) = args.get(3) {
        if !matches!(&*l.deref(), Value::Null | Value::Int(_) | Value::Array(_) | Value::Bool(_) | Value::Float(_)) {
            return Err(Unwind::type_error(format!(
                "substr_replace(): Argument #4 ($length) must be of type array|int|null, {} given",
                l.type_name()
            )));
        }
    }
    let Value::Array(subjects) = &subject else {
        if matches!(offset, Value::Array(_)) {
            return Err(Unwind::type_error(
                "substr_replace(): Argument #3 ($offset) cannot be an array when working on a single string",
            ));
        }
        if matches!(length, Value::Array(_)) {
            return Err(Unwind::type_error(
                "substr_replace(): Argument #4 ($length) cannot be an array when working on a single string",
            ));
        }
        // A replacement array contributes its first element (or nothing).
        let rep = match &replace {
            Value::Array(r) => r.first().map(|(_, v)| v.to_php_bytes()).unwrap_or_default(),
            v => v.to_php_bytes(),
        };
        let len = match &length {
            Value::Null => None,
            v => Some(v.to_int()),
        };
        return Ok(str_value(splice_bytes(&subject.to_php_bytes(), &rep, offset.to_int(), len)));
    };
    // Array subject: each element is spliced with the matching (by position)
    // replacement / offset / length, which fall back to "" / 0 / whole string
    // once their array runs out.
    let reps: Option<Vec<Vec<u8>>> = match &replace {
        Value::Array(r) => {
            let mut out = Vec::new();
            for (_, v) in r.iter() {
                out.push(crate::array2::sort_string_of(ctx, v)?);
            }
            Some(out)
        }
        _ => None,
    };
    let rep_scalar = replace.to_php_bytes();
    let offs: Option<Vec<i64>> = match &offset {
        Value::Array(o) => Some(o.iter().map(|(_, v)| v.to_int()).collect()),
        _ => None,
    };
    let lens: Option<Vec<i64>> = match &length {
        Value::Array(l) => Some(l.iter().map(|(_, v)| v.to_int()).collect()),
        _ => None,
    };
    let mut out = Array::new();
    for (i, (k, v)) in subjects.iter().enumerate() {
        let sb = crate::array2::sort_string_of(ctx, v)?;
        let rep: Vec<u8> = match &reps {
            Some(r) => r.get(i).cloned().unwrap_or_default(),
            None => rep_scalar.clone(),
        };
        let off = match &offs {
            Some(o) => o.get(i).copied().unwrap_or(0),
            None => offset.to_int(),
        };
        let len = match &lens {
            Some(l) => Some(l.get(i).copied().unwrap_or(sb.len() as i64)),
            None => match &length {
                Value::Null => None,
                v => Some(v.to_int()),
            },
        };
        out.set(k.clone(), str_value(splice_bytes(&sb, &rep, off, len)));
    }
    Ok(Value::Array(out))
}

pub(crate) fn quotemeta(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn addslashes(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn stripslashes(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn number_format(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let num = args[0].to_float();
    // PHP prints non-finite values as a bare "nan"/"inf" (no sign, no grouping).
    if num.is_nan() {
        return Ok(str_value(b"nan".to_vec()));
    }
    if num.is_infinite() {
        return Ok(str_value(b"inf".to_vec()));
    }
    let dec_arg = args.get(1).map_or(0, Value::to_int);
    let dec = dec_arg.max(0) as usize;
    let dec_point = match args.get(2) {
        Some(v) => bytes(v),
        None => vec![b'.'],
    };
    let thousands = match args.get(3) {
        Some(v) => bytes(v),
        None => vec![b','],
    };
    // php rounds first (half away from zero on the decimal the float
    // denotes), then prints the rounded double's exact digits with `%.NF`.
    let (neg, intpart, fracpart) = if dec_arg < 0 {
        // PHP 8.3: a negative precision rounds to that power of ten, e.g.
        // number_format(1234.5678, -2) is "1,200".
        let scale = 10f64.powi((-dec_arg).min(400) as i32);
        let (neg, scaled, _) = format_decimal(num / scale, 0);
        if scaled == b"0" {
            (false, vec![b'0'], Vec::new())
        } else {
            let mut ip = scaled;
            ip.extend(std::iter::repeat_n(b'0', (-dec_arg) as usize));
            (neg, ip, Vec::new())
        }
    } else {
        let (neg, ip, fp) = format_decimal(num, dec);
        // Re-read the rounded decimal as a double and print it exactly, so
        // magnitudes beyond 2^53 show their true digits as php does.
        let mut text = String::from_utf8(ip).unwrap_or_default();
        if !fp.is_empty() {
            text.push('.');
            text.push_str(&String::from_utf8(fp).unwrap_or_default());
        }
        let rounded: f64 = text.parse().unwrap_or(num.abs());
        let exact = format!("{:.*}", dec, rounded);
        let (ip, fp) = match exact.split_once('.') {
            Some((a, b)) => (a.as_bytes().to_vec(), b.as_bytes().to_vec()),
            None => (exact.into_bytes(), Vec::new()),
        };
        (neg, ip, fp)
    };
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

pub(crate) fn str_word_count(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = bytes(&args[0]);
    let format = args.get(1).map_or(0, Value::to_int);
    if !(0..=2).contains(&format) {
        return Err(Unwind::value_error(
            "str_word_count(): Argument #2 ($format) must be a valid format value",
        ));
    }
    let extra = match args.get(2) {
        Some(Value::Null) | None => [false; 256],
        Some(v) => charmask(ctx, "str_word_count", &bytes(v))?,
    };
    let is_word = |c: u8| c.is_ascii_alphabetic() || extra[c as usize] || c == b'\'' || c == b'-';
    // Only the very first byte may not be an apostrophe or dash, and only the
    // very last byte may not be a dash (unless the char list allows them).
    let mut start = 0;
    let mut end = s.len();
    if !s.is_empty() && ((s[0] == b'\'' && !extra[b'\'' as usize]) || (s[0] == b'-' && !extra[b'-' as usize])) {
        start = 1;
    }
    if end > start && s[end - 1] == b'-' && !extra[b'-' as usize] {
        end -= 1;
    }
    let mut words: Vec<(usize, &[u8])> = Vec::new();
    let mut i = start;
    while i < end {
        let ws = i;
        while i < end && is_word(s[i]) {
            i += 1;
        }
        if i > ws {
            words.push((ws, &s[ws..i]));
        }
        i += 1;
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
        _ => Err(Unwind::value_error(
            "str_word_count(): Argument #2 ($format) must be a valid format value",
        )),
    }
}
