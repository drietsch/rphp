//! Array builtins, built directly on [`rphp_value::Array`] so key coercion,
//! ordering, and loose/strict comparison match the engine exactly.
use std::collections::HashSet;

use rphp_value::{array_key, Array, ArrayKey, Str, Value};

use crate::{nf, nf_mut, Ctx, NativeError, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`). Value-returning array
/// functions live here; in-place mutators (sort, array_push, …) wait on
/// by-reference parameters in the call ABI.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("count", 1, Some(2), count),
    nf!("sizeof", 1, Some(2), count),
    nf!("in_array", 2, Some(3), in_array),
    nf!("array_key_exists", 2, Some(2), array_key_exists),
    nf!("array_keys", 1, Some(1), array_keys),
    nf!("array_values", 1, Some(1), array_values),
    nf!("array_merge", 0, None, array_merge),
    nf!("array_reverse", 1, Some(2), array_reverse),
    nf!("array_sum", 1, Some(1), array_sum),
    nf!("range", 2, Some(3), range),
    nf!("array_slice", 2, Some(4), array_slice),
    nf!("array_flip", 1, Some(1), array_flip),
    nf!("array_unique", 1, Some(2), array_unique),
    nf!("array_search", 2, Some(3), array_search),
    nf!("array_fill", 3, Some(3), array_fill),
    nf!("array_fill_keys", 2, Some(2), array_fill_keys),
    nf!("array_combine", 2, Some(2), array_combine),
    nf!("array_pad", 3, Some(3), array_pad),
    nf!("array_column", 2, Some(3), array_column),
    nf!("array_chunk", 2, Some(3), array_chunk),
    nf!("array_product", 1, Some(1), array_product),
    nf!("array_count_values", 1, Some(1), array_count_values),
    nf!("array_key_first", 1, Some(1), array_key_first),
    nf!("array_key_last", 1, Some(1), array_key_last),
    nf!("array_is_list", 1, Some(1), array_is_list),
    nf!("array_diff", 2, None, array_diff),
    nf!("array_intersect", 2, None, array_intersect),
    nf!("array_replace", 1, None, array_replace),
    // --- by-reference mutators: write the result back through $array (#0) ---
    nf_mut!("sort", 1, Some(2), 0b1, sort),
    nf_mut!("rsort", 1, Some(2), 0b1, rsort),
    nf_mut!("asort", 1, Some(2), 0b1, asort),
    nf_mut!("arsort", 1, Some(2), 0b1, arsort),
    nf_mut!("ksort", 1, Some(2), 0b1, ksort),
    nf_mut!("krsort", 1, Some(2), 0b1, krsort),
    nf_mut!("array_push", 1, None, 0b1, array_push),
    nf_mut!("array_pop", 1, Some(1), 0b1, array_pop),
    nf_mut!("array_shift", 1, Some(1), 0b1, array_shift),
    nf_mut!("array_unshift", 1, None, 0b1, array_unshift),
    nf_mut!("array_splice", 2, Some(4), 0b1, array_splice),
    // --- higher-order: invoke a callable through the host ---
    nf!("array_map", 2, Some(2), array_map),
    nf!("array_filter", 1, Some(2), array_filter),
    nf!("array_reduce", 2, Some(3), array_reduce),
    nf_mut!("usort", 2, Some(2), 0b1, usort),
    nf_mut!("uasort", 2, Some(2), 0b1, uasort),
    nf_mut!("uksort", 2, Some(2), 0b1, uksort),
];

/// Borrow an argument as an array, or produce PHP's TypeError message.
fn want_array<'a>(func: &str, v: &'a Value) -> Result<&'a Array, NativeError> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(NativeError::new(format!(
            "{func}(): Argument #1 ($array) must be of type array, {} given",
            other.type_name()
        ))),
    }
}

pub(crate) fn count(_: &mut Ctx, args: &[Value]) -> NativeResult {
    match &args[0] {
        Value::Array(a) => Ok(Value::Int(a.len() as i64)),
        other => Err(NativeError::new(format!(
            "count(): Argument #1 ($value) must be of type Countable|array, {} given",
            other.type_name()
        ))),
    }
}

pub(crate) fn in_array(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let needle = &args[0];
    let haystack = want_array("in_array", &args[1])?;
    let strict = args.get(2).is_some_and(Value::to_bool);
    for (_, v) in haystack.iter() {
        let hit = if strict { needle.identical(v) } else { needle.loose_eq(v) };
        if hit {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

pub(crate) fn array_key_exists(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_key_exists", &args[1])?;
    Ok(Value::Bool(match array_key(&args[0]) {
        Some(k) => arr.get(&k).is_some(),
        None => false,
    }))
}

pub(crate) fn array_keys(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_keys", &args[0])?;
    let mut out = Array::new();
    for (k, _) in arr.iter() {
        out.push(k.to_value());
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_values(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_values", &args[0])?;
    let mut out = Array::new();
    for (_, v) in arr.iter() {
        out.push(v.clone());
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_merge(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut out = Array::new();
    for arg in args {
        let arr = want_array("array_merge", arg)?;
        for (k, v) in arr.iter() {
            match k {
                // Integer keys are renumbered consecutively.
                ArrayKey::Int(_) => out.push(v.clone()),
                // String keys keep their name; a later one overwrites an earlier.
                ArrayKey::Str(_) => out.set(k.clone(), v.clone()),
            }
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_reverse(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_reverse", &args[0])?;
    let preserve = args.get(1).is_some_and(Value::to_bool);
    let mut out = Array::new();
    // `Array::iter()` is not double-ended; collect then walk it backwards.
    let entries: Vec<(&ArrayKey, &Value)> = arr.iter().collect();
    for (k, v) in entries.into_iter().rev() {
        match (preserve, k) {
            // Integer keys are renumbered unless preservation is requested;
            // string keys are always kept.
            (false, ArrayKey::Int(_)) => out.push(v.clone()),
            _ => out.set(k.clone(), v.clone()),
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_sum(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_sum", &args[0])?;
    let mut acc = Value::Int(0);
    for (_, v) in arr.iter() {
        // `to_number` keeps both operands numeric, so `add` never errors.
        if let Ok(sum) = acc.add(&v.to_number()) {
            acc = sum;
        }
    }
    Ok(acc)
}

pub(crate) fn range(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let start = &args[0];
    let end = &args[1];
    let step_arg = args.get(2);

    // Character range: both bounds are single non-numeric bytes (e.g. 'a'..'z').
    if one_char_nonnumeric(start) && one_char_nonnumeric(end) {
        let lo = first_byte(start) as i64;
        let hi = first_byte(end) as i64;
        let step = step_arg.map_or(1, |s| s.to_int().abs()).max(1);
        let mut out = Array::new();
        let mut c = lo;
        if lo <= hi {
            while c <= hi {
                out.push(char_value(c as u8));
                c += step;
            }
        } else {
            while c >= hi {
                out.push(char_value(c as u8));
                c -= step;
            }
        }
        return Ok(Value::Array(out));
    }

    // Numeric range. Float if any bound or the step is a float.
    let s = start.to_number();
    let e = end.to_number();
    let step_v = step_arg.cloned().unwrap_or(Value::Int(1));
    let float_mode = matches!(s, Value::Float(_))
        || matches!(e, Value::Float(_))
        || matches!(step_v, Value::Float(_));

    let mut out = Array::new();
    if float_mode {
        let (s, e, st) = (s.to_float(), e.to_float(), step_v.to_float().abs());
        if st == 0.0 {
            return Err(NativeError::new("range(): Argument #3 ($step) must not be 0"));
        }
        let count = ((e - s).abs() / st).floor() as i64;
        for i in 0..=count {
            let v = if s <= e { s + i as f64 * st } else { s - i as f64 * st };
            out.push(Value::Float(v));
        }
    } else {
        let (s, e) = (s.to_int(), e.to_int());
        let st = step_v.to_int().abs();
        if st == 0 {
            return Err(NativeError::new("range(): Argument #3 ($step) must not be 0"));
        }
        let mut c = s;
        if s <= e {
            while c <= e {
                out.push(Value::Int(c));
                c += st;
            }
        } else {
            while c >= e {
                out.push(Value::Int(c));
                c -= st;
            }
        }
    }
    Ok(Value::Array(out))
}

// ---- helpers ----------------------------------------------------------------

fn one_char_nonnumeric(v: &Value) -> bool {
    matches!(v, Value::Str(s) if s.len() == 1) && !v.is_numeric()
}

fn first_byte(v: &Value) -> u8 {
    match v {
        Value::Str(s) => s.as_bytes().first().copied().unwrap_or(0),
        _ => 0,
    }
}

fn char_value(b: u8) -> Value {
    Value::Str(Str::from_vec(vec![b]))
}

/// Borrow the `n`-th (0-based) argument as an array, with PHP's *positional*
/// TypeError used by the variadic set operators — those omit the parameter name
/// for every argument after the first (`array_diff(): Argument #2 must be …`).
fn want_array_n<'a>(func: &str, n: usize, v: &'a Value) -> Result<&'a Array, NativeError> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(NativeError::new(format!(
            "{func}(): Argument #{} must be of type array, {} given",
            n + 1,
            other.type_name()
        ))),
    }
}

/// Append every entry of `src` to `out`, renumbering integer keys from `out`'s
/// next slot while preserving string keys — the rule `array_merge`/`array_pad`
/// apply to a single source array.
fn append_reindexed(out: &mut Array, src: &Array) {
    for (k, v) in src.iter() {
        match k {
            ArrayKey::Int(_) => out.push(v.clone()),
            ArrayKey::Str(_) => out.set(k.clone(), v.clone()),
        }
    }
}

pub(crate) fn array_slice(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_slice", &args[0])?;
    let n = arr.len() as i64;
    // Snapshot once; `Array::iter()` is single-pass and we index it by position.
    let entries: Vec<(&ArrayKey, &Value)> = arr.iter().collect();

    // Resolve the start offset (a negative offset counts from the end).
    let mut start = args[1].to_int();
    if start < 0 {
        start = (n + start).max(0);
    } else {
        start = start.min(n);
    }
    // Resolve the exclusive end: null length means "to the end"; a negative
    // length stops that many elements short of the end.
    let end = match args.get(2) {
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
    let preserve = args.get(3).is_some_and(Value::to_bool);

    let mut out = Array::new();
    let mut i = start;
    while i < end {
        let (k, v) = entries[i as usize];
        match (preserve, k) {
            // Integer keys are renumbered unless preservation is requested;
            // string keys are always kept.
            (false, ArrayKey::Int(_)) => out.push(v.clone()),
            _ => out.set(k.clone(), v.clone()),
        }
        i += 1;
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_flip(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_flip", &args[0])?;
    let mut out = Array::new();
    for (k, v) in arr.iter() {
        // Only int/string values can become keys; PHP warns and skips the rest.
        // A value like "5" normalizes to the int key 5 via `array_key`.
        if matches!(v, Value::Int(_) | Value::Str(_)) {
            if let Some(key) = array_key(v) {
                out.set(key, k.to_value());
            }
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_unique(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_unique", &args[0])?;
    // Default SORT_STRING: two values are duplicates iff their `(string)` casts
    // are byte-equal. The first occurrence wins and its key is preserved.
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut out = Array::new();
    for (k, v) in arr.iter() {
        if seen.insert(v.to_php_bytes()) {
            out.set(k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_search(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let needle = &args[0];
    let haystack = match &args[1] {
        Value::Array(a) => a,
        other => {
            return Err(NativeError::new(format!(
                "array_search(): Argument #2 ($haystack) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    let strict = args.get(2).is_some_and(Value::to_bool);
    for (k, v) in haystack.iter() {
        let hit = if strict { needle.identical(v) } else { needle.loose_eq(v) };
        if hit {
            return Ok(k.to_value());
        }
    }
    Ok(Value::Bool(false))
}

pub(crate) fn array_fill(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let start = args[0].to_int();
    let count = args[1].to_int();
    if count < 0 {
        return Err(NativeError::new(
            "array_fill(): Argument #2 ($count) must be greater than or equal to 0",
        ));
    }
    let value = &args[2];
    let mut out = Array::new();
    // PHP 8 fills consecutive integer keys from `start` (a negative start counts
    // up through 0, e.g. -2,-1,0,1,2).
    for i in 0..count {
        out.set(ArrayKey::Int(start.wrapping_add(i)), value.clone());
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_fill_keys(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let keys = want_array("array_fill_keys", &args[0])?;
    let value = &args[1];
    let mut out = Array::new();
    // Each *value* of `keys` becomes a key (normalized) mapped to `value`.
    for (_, k) in keys.iter() {
        if let Some(key) = array_key(k) {
            out.set(key, value.clone());
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_combine(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let keys = match &args[0] {
        Value::Array(a) => a,
        other => {
            return Err(NativeError::new(format!(
                "array_combine(): Argument #1 ($keys) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    let values = match &args[1] {
        Value::Array(a) => a,
        other => {
            return Err(NativeError::new(format!(
                "array_combine(): Argument #2 ($values) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    if keys.len() != values.len() {
        return Err(NativeError::new(
            "array_combine(): Argument #1 ($keys) and argument #2 ($values) must have the same number of elements",
        ));
    }
    let mut out = Array::new();
    for ((_, k), (_, v)) in keys.iter().zip(values.iter()) {
        if let Some(key) = array_key(k) {
            out.set(key, v.clone());
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_pad(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_pad", &args[0])?;
    let size = args[1].to_int();
    let value = &args[2];
    // Number of padding elements to add; if the array already meets the target
    // size PHP returns it untouched (keys preserved, no reindex).
    let pad = size.unsigned_abs() as i64 - arr.len() as i64;
    if pad <= 0 {
        return Ok(Value::Array(arr.clone()));
    }
    let mut out = Array::new();
    if size > 0 {
        // Pad on the right: originals first (int keys renumbered), then padding.
        append_reindexed(&mut out, arr);
        for _ in 0..pad {
            out.push(value.clone());
        }
    } else {
        // Pad on the left: padding first, then the originals appended after it.
        for _ in 0..pad {
            out.push(value.clone());
        }
        append_reindexed(&mut out, arr);
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_column(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_column", &args[0])?;
    // A null column key selects the whole row; otherwise normalize it once.
    let whole_row = matches!(&args[1], Value::Null);
    let column_key = if whole_row { None } else { array_key(&args[1]) };
    // An absent or null index argument means "append with the next int key".
    let index_key = match args.get(2) {
        None | Some(Value::Null) => None,
        Some(v) => Some(array_key(v)),
    };

    let mut out = Array::new();
    for (_, row) in arr.iter() {
        let row = match row {
            Value::Array(a) => a,
            _ => continue, // non-array rows are ignored
        };
        // Pull the column value (or the whole row); skip rows lacking it.
        let val = if whole_row {
            Value::Array(row.clone())
        } else {
            match column_key.as_ref().and_then(|k| row.get(k)) {
                Some(v) => v.clone(),
                None => continue,
            }
        };
        match &index_key {
            // No index requested: append under the next integer key.
            None => out.push(val),
            // Index requested: key by the row's index value, falling back to an
            // append when the row lacks it or it is not a usable key.
            Some(ik) => match ik.as_ref().and_then(|k| row.get(k)).and_then(array_key) {
                Some(key) => out.set(key, val),
                None => out.push(val),
            },
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_chunk(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_chunk", &args[0])?;
    let size = args[1].to_int();
    if size < 1 {
        return Err(NativeError::new(
            "array_chunk(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let preserve = args.get(2).is_some_and(Value::to_bool);
    let mut out = Array::new();
    let mut chunk = Array::new();
    let mut count = 0i64;
    for (k, v) in arr.iter() {
        if preserve {
            chunk.set(k.clone(), v.clone());
        } else {
            chunk.push(v.clone());
        }
        count += 1;
        if count == size {
            out.push(Value::Array(std::mem::take(&mut chunk)));
            count = 0;
        }
    }
    // Flush a partial final chunk.
    if count > 0 {
        out.push(Value::Array(chunk));
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_product(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_product", &args[0])?;
    // The empty-array product is the int 1, per PHP.
    let mut acc = Value::Int(1);
    for (_, v) in arr.iter() {
        // `to_number` keeps both operands numeric, so `mul` never errors.
        if let Ok(p) = acc.mul(&v.to_number()) {
            acc = p;
        }
    }
    Ok(acc)
}

pub(crate) fn array_count_values(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_count_values", &args[0])?;
    let mut out = Array::new();
    for (_, v) in arr.iter() {
        // Only int/string values are countable; PHP warns and skips the rest.
        if !matches!(v, Value::Int(_) | Value::Str(_)) {
            continue;
        }
        if let Some(key) = array_key(v) {
            let next = out.get(&key).map_or(0, Value::to_int) + 1;
            out.set(key, Value::Int(next));
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_key_first(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_key_first", &args[0])?;
    Ok(arr.iter().next().map_or(Value::Null, |(k, _)| k.to_value()))
}

pub(crate) fn array_key_last(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_key_last", &args[0])?;
    Ok(arr.iter().last().map_or(Value::Null, |(k, _)| k.to_value()))
}

pub(crate) fn array_is_list(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let arr = want_array("array_is_list", &args[0])?;
    // A list has consecutive int keys 0,1,2,… in order (the empty array counts).
    let mut expected = 0i64;
    for (k, _) in arr.iter() {
        match k {
            ArrayKey::Int(i) if *i == expected => expected += 1,
            _ => return Ok(Value::Bool(false)),
        }
    }
    Ok(Value::Bool(true))
}

pub(crate) fn array_diff(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let base = want_array("array_diff", &args[0])?;
    // Two elements are equal iff `(string)$a === (string)$b`; gather the string
    // form of every value across the remaining arrays.
    let mut others: HashSet<Vec<u8>> = HashSet::new();
    for (i, arg) in args.iter().enumerate().skip(1) {
        let a = want_array_n("array_diff", i, arg)?;
        for (_, v) in a.iter() {
            others.insert(v.to_php_bytes());
        }
    }
    let mut out = Array::new();
    for (k, v) in base.iter() {
        if !others.contains(&v.to_php_bytes()) {
            out.set(k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_intersect(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let base = want_array("array_intersect", &args[0])?;
    // Keep a base value iff its string form appears in *every* other array.
    let mut sets: Vec<HashSet<Vec<u8>>> = Vec::new();
    for (i, arg) in args.iter().enumerate().skip(1) {
        let a = want_array_n("array_intersect", i, arg)?;
        sets.push(a.iter().map(|(_, v)| v.to_php_bytes()).collect());
    }
    let mut out = Array::new();
    for (k, v) in base.iter() {
        let s = v.to_php_bytes();
        if sets.iter().all(|set| set.contains(&s)) {
            out.set(k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_replace(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // Start from a copy of the first array, then overwrite matching keys from
    // each later array in place (new keys are appended; no integer renumbering).
    let mut out = want_array("array_replace", &args[0])?.clone();
    for (i, arg) in args.iter().enumerate().skip(1) {
        let a = want_array_n("array_replace", i, arg)?;
        for (k, v) in a.iter() {
            out.set(k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

// ---- by-reference mutators --------------------------------------------------
//
// These take `&mut [Value]` and write the new array back into `args[0]`; the
// interpreter copies that slot back into the caller's variable (see the call
// ABI). `Array` exposes no in-place remove, so they rebuild from owned entries.

/// Owned `(key, value)` entries of an array argument, or PHP's TypeError.
fn take_entries(func: &str, v: &Value) -> Result<Vec<(ArrayKey, Value)>, NativeError> {
    match v {
        Value::Array(a) => Ok(a.iter().map(|(k, val)| (k.clone(), val.clone())).collect()),
        other => Err(NativeError::new(format!(
            "{func}(): Argument #1 ($array) must be of type array, {} given",
            other.type_name()
        ))),
    }
}

/// PHP SORT_REGULAR comparison, reusing the engine's spaceship so ordering
/// matches `<` exactly. (Sort flags like SORT_STRING are not modelled yet.)
fn regular_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    a.spaceship(b).cmp(&0)
}

/// Rebuild a value array, optionally renumbering integer keys (string keys are
/// always kept). `reindex` mirrors the difference between `sort` (renumber) and
/// `asort`/`ksort` (preserve).
fn rebuild(entries: Vec<(ArrayKey, Value)>, reindex: bool) -> Value {
    let mut out = Array::new();
    for (k, v) in entries {
        match (reindex, &k) {
            (true, ArrayKey::Int(_)) => out.push(v),
            _ => out.set(k, v),
        }
    }
    Value::Array(out)
}

/// Shared body for the six sort builtins: sort the entries by `key`/`value` per
/// the flags, then write the (reindexed or key-preserving) array back.
fn sort_impl(
    func: &str,
    args: &mut [Value],
    by_key: bool,
    reverse: bool,
    reindex: bool,
) -> NativeResult {
    let mut entries = take_entries(func, &args[0])?;
    entries.sort_by(|a, b| {
        let ord = if by_key {
            regular_cmp(&a.0.to_value(), &b.0.to_value())
        } else {
            regular_cmp(&a.1, &b.1)
        };
        if reverse {
            ord.reverse()
        } else {
            ord
        }
    });
    args[0] = rebuild(entries, reindex);
    Ok(Value::Bool(true))
}

pub(crate) fn sort(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl("sort", args, false, false, true)
}

pub(crate) fn rsort(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl("rsort", args, false, true, true)
}

pub(crate) fn asort(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl("asort", args, false, false, false)
}

pub(crate) fn arsort(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl("arsort", args, false, true, false)
}

pub(crate) fn ksort(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl("ksort", args, true, false, false)
}

pub(crate) fn krsort(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl("krsort", args, true, true, false)
}

pub(crate) fn array_push(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut a = match &args[0] {
        Value::Array(a) => a.clone(),
        other => {
            return Err(NativeError::new(format!(
                "array_push(): Argument #1 ($array) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    for v in &args[1..] {
        a.push(v.clone());
    }
    let count = a.len() as i64;
    args[0] = Value::Array(a);
    Ok(Value::Int(count))
}

pub(crate) fn array_pop(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut entries = take_entries("array_pop", &args[0])?;
    let popped = entries.pop().map(|(_, v)| v);
    // Keys are preserved (no renumber); the next-append index resets to max+1,
    // which rebuilding via `set` reproduces.
    args[0] = rebuild(entries, false);
    Ok(popped.unwrap_or(Value::Null))
}

pub(crate) fn array_shift(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut entries = take_entries("array_shift", &args[0])?;
    if entries.is_empty() {
        return Ok(Value::Null);
    }
    let (_, first) = entries.remove(0);
    // array_shift renumbers integer keys, keeps string keys.
    args[0] = rebuild(entries, true);
    Ok(first)
}

pub(crate) fn array_unshift(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let existing = take_entries("array_unshift", &args[0])?;
    let mut combined: Vec<(ArrayKey, Value)> =
        args[1..].iter().map(|v| (ArrayKey::Int(0), v.clone())).collect();
    combined.extend(existing);
    // Prepended values plus existing integer keys are renumbered from 0.
    args[0] = rebuild(combined, true);
    let count = match &args[0] {
        Value::Array(a) => a.len() as i64,
        _ => 0,
    };
    Ok(Value::Int(count))
}

pub(crate) fn array_splice(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let entries = take_entries("array_splice", &args[0])?;
    let n = entries.len() as i64;
    // offset: negative counts from the end; clamped into 0..=n.
    let mut offset = args[1].to_int();
    if offset < 0 {
        offset = (n + offset).max(0);
    } else {
        offset = offset.min(n);
    }
    let offset = offset as usize;
    // length: absent/null => to the end; negative => stop that many from the end.
    let end = match args.get(2) {
        None | Some(Value::Null) => n as usize,
        Some(l) => {
            let l = l.to_int();
            if l < 0 {
                (n + l).max(offset as i64) as usize
            } else {
                (offset as i64 + l).min(n) as usize
            }
        }
    };
    let replacement: Vec<Value> = match args.get(3) {
        Some(Value::Array(a)) => a.iter().map(|(_, v)| v.clone()).collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.clone()],
    };

    // Reassemble: entries before the cut, then the replacement, then entries
    // after the cut — all renumbered (array_splice always reindexes).
    let mut out = Array::new();
    for (_, v) in &entries[..offset] {
        out.push(v.clone());
    }
    for v in &replacement {
        out.push(v.clone());
    }
    for (_, v) in &entries[end..] {
        out.push(v.clone());
    }
    let mut removed = Array::new();
    for (_, v) in &entries[offset..end] {
        removed.push(v.clone());
    }
    args[0] = Value::Array(out);
    Ok(Value::Array(removed))
}

// ---- higher-order (callback) functions --------------------------------------
//
// These invoke a PHP callable through `ctx.call` (the host re-entry point). The
// callable is a function-name string for now; closures arrive with the closure
// value type. Multi-array `array_map`, `array_filter` modes, and `array_walk`
// (by-ref callback) are cataloged in COVERAGE.md.

pub(crate) fn array_map(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let entries = take_entries("array_map", &args[1])?;
    let mut out = Array::new();
    for (k, v) in entries {
        // Single-array form preserves keys; the result is the callback's return.
        let mapped = ctx.call(&args[0], &[v])?;
        out.set(k, mapped);
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_filter(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let entries = take_entries("array_filter", &args[0])?;
    // Without a callback, keep the truthy elements; with one, keep where it
    // returns true. Keys are preserved either way.
    let callback = args.get(1).filter(|v| !matches!(v, Value::Null));
    let mut out = Array::new();
    for (k, v) in entries {
        let keep = match callback {
            Some(cb) => ctx.call(cb, std::slice::from_ref(&v))?.to_bool(),
            None => v.to_bool(),
        };
        if keep {
            out.set(k, v);
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_reduce(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let entries = take_entries("array_reduce", &args[0])?;
    let mut acc = args.get(2).cloned().unwrap_or(Value::Null);
    for (_, v) in entries {
        acc = ctx.call(&args[1], &[acc, v])?;
    }
    Ok(acc)
}

/// Shared body for the user-comparator sorts (`usort`/`uasort`/`uksort`): sort by
/// the callback (its sign is the order), then write back. A callback error
/// aborts the sort and surfaces. `reindex` distinguishes `usort` (renumber) from
/// the key-preserving `u*sort` pair.
fn user_sort(
    ctx: &mut Ctx,
    args: &mut [Value],
    func: &str,
    by_key: bool,
    reindex: bool,
) -> NativeResult {
    let mut entries = take_entries(func, &args[0])?;
    let cb = args[1].clone();
    let mut err: Option<NativeError> = None;
    entries.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let (x, y) = if by_key {
            (a.0.to_value(), b.0.to_value())
        } else {
            (a.1.clone(), b.1.clone())
        };
        match ctx.call(&cb, &[x, y]) {
            Ok(r) => r.to_int().cmp(&0),
            Err(e) => {
                err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    args[0] = rebuild(entries, reindex);
    Ok(Value::Bool(true))
}

pub(crate) fn usort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    user_sort(ctx, args, "usort", false, true)
}

pub(crate) fn uasort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    user_sort(ctx, args, "uasort", false, false)
}

pub(crate) fn uksort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    user_sort(ctx, args, "uksort", true, false)
}
