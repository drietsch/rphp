//! Array builtins, built directly on [`rphp_value::Array`] so key coercion,
//! ordering, and loose/strict comparison match the engine exactly.
use std::cmp::Ordering;
use std::collections::HashSet;

use rphp_value::{array_key, Array, ArrayKey, Str, Value};

use rphp_runtime::{Ctx, NativeFn, NativeResult, nf, nf_ref, Unwind};

use crate::array2::{
    compare_keys, compare_values, is_callable, rebuild as rebuild_entries, sort_entries, user_compare,
    zend_sort, SORT_REGULAR, SORT_STRING,
};

/// This extension's registry contribution (see `lib.rs`). Value-returning array
/// functions live here; in-place mutators (sort, array_push, …) wait on
/// by-reference parameters in the call ABI.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("count", 1, Some(2), count),
    nf!("sizeof", 1, Some(2), count),
    nf!("in_array", 2, Some(3), in_array),
    nf!("array_key_exists", 2, Some(2), array_key_exists),
    nf!("array_keys", 1, Some(3), array_keys),
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
    nf!("array_diff", 1, None, array_diff),
    nf!("array_intersect", 1, None, array_intersect),
    nf!("array_replace", 1, None, array_replace),
    // --- by-reference mutators: write the result back through $array (#0) ---
    nf_ref!("sort", 1, Some(2), 0b1, sort),
    nf_ref!("rsort", 1, Some(2), 0b1, rsort),
    nf_ref!("asort", 1, Some(2), 0b1, asort),
    nf_ref!("arsort", 1, Some(2), 0b1, arsort),
    nf_ref!("ksort", 1, Some(2), 0b1, ksort),
    nf_ref!("krsort", 1, Some(2), 0b1, krsort),
    nf_ref!("array_push", 1, None, 0b1, array_push),
    nf_ref!("array_pop", 1, Some(1), 0b1, array_pop),
    nf_ref!("array_shift", 1, Some(1), 0b1, array_shift),
    nf_ref!("array_unshift", 1, None, 0b1, array_unshift),
    nf_ref!("array_splice", 2, Some(4), 0b1, array_splice),
    // --- higher-order: invoke a callable through the interpreter ---
    nf!("array_map", 2, None, array_map),
    nf!("array_filter", 1, Some(3), array_filter),
    nf!("array_reduce", 2, Some(3), array_reduce),
    nf_ref!("usort", 2, Some(2), 0b1, usort),
    nf_ref!("uasort", 2, Some(2), 0b1, uasort),
    nf_ref!("uksort", 2, Some(2), 0b1, uksort),
];

/// Borrow the first argument as an array, or produce PHP's TypeError message.
fn want_array<'a>(func: &str, v: &'a Value) -> Result<&'a Array, Unwind> {
    want_array_at(func, 1, "$array", v)
}

/// [`want_array`] for argument `pos` named `name` (`""` for a variadic
/// position, which php names by number alone).
fn want_array_at<'a>(func: &str, pos: usize, name: &str, v: &'a Value) -> Result<&'a Array, Unwind> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #{pos}{} must be of type array, {} given",
            if name.is_empty() { String::new() } else { format!(" ({name})") },
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// `count()` with `COUNT_RECURSIVE`: elements plus those of nested arrays. A
/// reference cycle (`$a[] = &$a`) is detected through the reference cells on
/// the current path (php protects the hashtable itself, which this crate
/// cannot identify yet; the direct self-reference therefore counts one level
/// deeper than php before the warning).
fn count_recursive(ctx: &mut Ctx, a: &Array, path: &mut Vec<rphp_value::PhpRef>) -> Result<i64, Unwind> {
    let mut n = a.len() as i64;
    for (_, v) in entries(a) {
        let cell = match &v {
            Value::Ref(r) => Some(r.clone()),
            _ => None,
        };
        if let Some(r) = &cell {
            if path.iter().any(|p| p.ptr_eq(r)) || path.len() > 512 {
                ctx.warn("count(): Recursion detected")?;
                continue;
            }
        }
        if let Value::Array(sub) = &*v.deref() {
            if let Some(r) = cell {
                path.push(r);
                n += count_recursive(ctx, sub, path)?;
                path.pop();
            } else {
                n += count_recursive(ctx, sub, path)?;
            }
        }
    }
    Ok(n)
}

pub(crate) fn count(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let recursive = args.get(1).map_or(0, Value::to_int) == 1;
    match &args[0] {
        Value::Array(a) if recursive => Ok(Value::Int(count_recursive(ctx, a, &mut Vec::new())?)),
        Value::Array(a) => Ok(Value::Int(a.len() as i64)),
        // E6: an object implementing `Countable` answers with its `count()`.
        // `COUNT_RECURSIVE` does not recurse into it — php calls the method
        // either way.
        Value::Object(o) => {
            let o = o.clone();
            let countable = ctx.class_by_name(b"Countable");
            match countable {
                Some(c) if ctx.object_instanceof(&o, c) => {
                    Ok(Value::Int(ctx.call_method(&o, b"count", &[])?.to_int()))
                }
                _ => Err(Unwind::type_error(format!(
                    "count(): Argument #1 ($value) must be of type Countable|array, {} given",
                    ctx.class_name_of(&o)
                ))),
            }
        }
        other => Err(Unwind::type_error(format!(
            "count(): Argument #1 ($value) must be of type Countable|array, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

pub(crate) fn in_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let needle = args[0].clone();
    let haystack = want_array_at("in_array", 2, "$haystack", &args[1])?;
    let strict = args.get(2).is_some_and(Value::to_bool);
    for (_, v) in haystack.iter() {
        let hit = if strict {
            needle.identical(v)
        } else {
            // `==`, with php's object-beside-number notice and conversion.
            let (l, r) = ctx.cmp_operands(needle.clone(), v.clone())?;
            l.loose_eq(&r)
        };
        if hit {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

pub(crate) fn array_key_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Array(arr) = &args[1] else {
        return Err(Unwind::type_error(format!(
            "array_key_exists(): Argument #2 ($array) must be of type array, {} given",
            rphp_runtime::value_name(&args[1])
        )));
    };
    let key = args[0].deref().into_owned();
    match &key {
        Value::Null => ctx.deprecated("Using null as the key parameter for array_key_exists() is deprecated, use an empty string instead")?,
        Value::Float(f) if f.fract() != 0.0 || !f.is_finite() => {
            ctx.deprecated(&format!("Implicit conversion from float {} to int loses precision", Value::Float(*f).to_php_string()))?
        }
        Value::Array(_) | Value::Object(_) | Value::Closure(_) => {
            return Err(Unwind::type_error("Illegal offset type"));
        }
        _ => {}
    }
    Ok(Value::Bool(match array_key(&key) {
        Some(k) => arr.get(&k).is_some(),
        None => false,
    }))
}

pub(crate) fn array_keys(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_keys", &args[0])?.clone();
    let strict = args.get(2).is_some_and(Value::to_bool);
    let needle = args.get(1).cloned();
    let mut out = Array::new();
    for (k, v) in arr.iter() {
        // With a filter value only the keys of matching elements are listed.
        if let Some(needle) = &needle {
            let hit = if strict {
                needle.identical(v)
            } else {
                // `==`, with php's object-beside-number notice and conversion.
                let (l, r) = ctx.cmp_operands(needle.clone(), v.clone())?;
                l.loose_eq(&r)
            };
            if !hit {
                continue;
            }
        }
        out.push(k.to_value());
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_values(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_values", &args[0])?;
    // php returns a packed array as it is (refcount + 1); a reference
    // element would be unwrapped by the copy, so only a plain list shares.
    if arr.is_pristine_list() && !arr.iter().any(|(_, v)| matches!(v, Value::Ref(_))) {
        return Ok(Value::Array(arr.clone()));
    }
    let mut out = Array::new();
    for (_, v) in arr.iter() {
        push_any(&mut out, v.clone());
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_merge(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (i, arg) in args.iter().enumerate() {
        let arr = want_array_at("array_merge", i + 1, "", arg)?;
        for (k, v) in arr.iter() {
            match k {
                // Integer keys are renumbered consecutively.
                ArrayKey::Int(_) => push_any(&mut out, v.clone()),
                // String keys keep their name; a later one overwrites an earlier.
                ArrayKey::Str(_) => put(&mut out, k.clone(), v.clone()),
            }
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_reverse(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_reverse", &args[0])?;
    let preserve = args.get(1).is_some_and(Value::to_bool);
    let mut out = Array::new();
    // `Array::iter()` is not double-ended; collect then walk it backwards.
    let entries: Vec<(&ArrayKey, &Value)> = arr.iter().collect();
    for (k, v) in entries.into_iter().rev() {
        match (preserve, k) {
            // Integer keys are renumbered unless preservation is requested;
            // string keys are always kept.
            (false, ArrayKey::Int(_)) => push_any(&mut out, v.clone()),
            _ => put(&mut out, k.clone(), v.clone()),
        }
    }
    Ok(Value::Array(out))
}

/// The numeric operand `array_sum`/`array_product` use for an element, with
/// php 8.3's diagnostics: arrays and non-numeric strings are skipped with a
/// warning, leading-numeric strings warn and contribute their number.
fn binop_operand(ctx: &mut Ctx, func: &str, op: &str, v: &Value) -> Result<Option<Value>, Unwind> {
    let v = v.deref().into_owned();
    match &v {
        Value::Str(s) => {
            if v.is_numeric() {
                return Ok(Some(v.to_number()));
            }
            let lead = v.to_number();
            let has_lead = {
                let t = s.as_bytes();
                let t = t.iter().position(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)).map_or(&t[t.len()..], |i| &t[i..]);
                let t = if t.first().is_some_and(|b| *b == b'+' || *b == b'-') { &t[1..] } else { t };
                t.first().is_some_and(u8::is_ascii_digit) || (t.first() == Some(&b'.') && t.get(1).is_some_and(u8::is_ascii_digit))
            };
            if has_lead {
                ctx.warn("A non-numeric value encountered")?;
                Ok(Some(lead))
            } else {
                ctx.warn(&format!("{func}(): {op} is not supported on type string"))?;
                Ok(None)
            }
        }
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => {
            ctx.warn(&format!("{func}(): {op} is not supported on type {}", rphp_runtime::value_name(&v)))?;
            Ok(None)
        }
        _ => Ok(Some(v.to_number())),
    }
}

pub(crate) fn array_sum(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_sum", &args[0])?;
    let mut acc = Value::Int(0);
    for (_, v) in entries(arr) {
        if let Some(n) = binop_operand(ctx, "array_sum", "Addition", &v)? {
            // Both operands are numeric, so `add` never errors.
            if let Ok(sum) = acc.add(&n) {
                acc = sum;
            }
        }
    }
    Ok(acc)
}

/// The numeric form of a `range()` bound: `Int`, `Float`, or `Str` (kept as
/// the original bytes).
enum Bound {
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
}

/// php 8.3's `range()`: character ranges for two single-byte strings,
/// otherwise integer or float ranges, with its warnings for odd inputs and
/// `ValueError`s for zero, backwards or oversized steps.
pub(crate) fn range(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const HT_MAX_SIZE: f64 = 1_073_741_824.0;
    // --- the step ---
    let mut step: i64 = 1;
    let mut step_f: f64 = 1.0;
    let mut is_step_double = false;
    let mut is_step_negative = false;
    if let Some(st) = args.get(2) {
        let st = st.deref().into_owned();
        let num = match &st {
            Value::Int(i) => Value::Int(*i),
            Value::Float(f) => Value::Float(*f),
            Value::Bool(b) => Value::Int(*b as i64),
            Value::Null => {
                ctx.deprecated("range(): Passing null to parameter #3 ($step) of type int|float is deprecated")?;
                Value::Int(0)
            }
            Value::Str(_) if st.is_numeric() => st.to_number(),
            other => {
                return Err(Unwind::type_error(format!(
                    "range(): Argument #3 ($step) must be of type int|float, {} given",
                    rphp_runtime::value_name(&other)
                )))
            }
        };
        match num {
            Value::Float(f) => {
                if f.is_infinite() {
                    return Err(Unwind::value_error("range(): Argument #3 ($step) must be a finite number, INF provided"));
                }
                if f.is_nan() {
                    return Err(Unwind::value_error("range(): Argument #3 ($step) must be a finite number, NAN provided"));
                }
                step_f = f;
                if step_f < 0.0 {
                    is_step_negative = true;
                    step_f = -step_f;
                }
                step = step_f as i64;
                if step_f.fract() != 0.0 || step_f >= 9.2e18 {
                    is_step_double = true;
                }
            }
            Value::Int(i) => {
                step = i;
                if step < 0 {
                    is_step_negative = true;
                    step = step.wrapping_neg();
                }
                step_f = step as f64;
            }
            _ => unreachable!(),
        }
    }
    // --- the bounds ---
    let bound = |ctx: &mut Ctx, v: &Value, n: usize, name: &str| -> Result<Bound, Unwind> {
        Ok(match &*v.deref() {
            Value::Int(i) => Bound::Int(*i),
            Value::Float(f) => Bound::Float(*f),
            Value::Bool(b) => Bound::Int(*b as i64),
            Value::Null => {
                ctx.deprecated(&format!("range(): Passing null to parameter #{n} (${name}) of type string|int|float is deprecated"))?;
                Bound::Int(0)
            }
            Value::Str(s) => Bound::Str(s.as_bytes().to_vec()),
            other => {
                return Err(Unwind::type_error(format!(
                    "range(): Argument #{n} (${name}) must be of type string|int|float, {} given",
                    rphp_runtime::value_name(&other)
                )))
            }
        })
    };
    let mut start = bound(ctx, &args[0], 1, "start")?;
    let mut end = bound(ctx, &args[1], 2, "end")?;
    if step == 0 && !is_step_double || (is_step_double && step_f == 0.0) {
        return Err(Unwind::value_error("range(): Argument #3 ($step) cannot be 0"));
    }
    // Empty strings are 0.
    if matches!(&start, Bound::Str(s) if s.is_empty()) {
        ctx.warn("range(): Argument #1 ($start) must not be empty, casted to 0")?;
        start = Bound::Int(0);
    }
    if matches!(&end, Bound::Str(s) if s.is_empty()) {
        ctx.warn("range(): Argument #2 ($end) must not be empty, casted to 0")?;
        end = Bound::Int(0);
    }
    // --- character ranges ---
    // Two strings make a character range unless a numeric one is longer than
    // a byte (or both are numeric and the step is a float); a float step on
    // a character range degrades to numbers with the non-numeric side as 0.
    let mut forced: Option<(Value, Value)> = None;
    if let (Bound::Str(a), Bound::Str(b)) = (&start, &end) {
        let sn = Value::string(a).is_numeric();
        let en = Value::string(b).is_numeric();
        let numeric_path = (sn && en && (a.len() > 1 || b.len() > 1 || is_step_double))
            || (sn && !en && a.len() > 1)
            || (en && !sn && b.len() > 1);
        if !numeric_path {
            if a.len() > 1 {
                ctx.warn("range(): Argument #1 ($start) must be a single byte, subsequent bytes are ignored")?;
            }
            if b.len() > 1 {
                ctx.warn("range(): Argument #2 ($end) must be a single byte, subsequent bytes are ignored")?;
            }
            if is_step_double {
                ctx.warn("range(): Argument #3 ($step) must be of type int when generating an array of characters, inputs converted to 0")?;
                let num = |s: &[u8], numeric: bool| if numeric { Value::string(s).to_number() } else { Value::Int(0) };
                forced = Some((num(a, sn), num(b, en)));
            } else {
                let (low, high) = (a[0] as i64, b[0] as i64);
                let mut out = Array::new();
                if low == high {
                    out.push(char_value(low as u8));
                } else {
                    if low < high && is_step_negative {
                        return Err(Unwind::value_error("range(): Argument #3 ($step) must be greater than 0 for increasing ranges"));
                    }
                    if (low - high).abs() < step {
                        return Err(Unwind::value_error("range(): Argument #3 ($step) must be less than the range spanned by argument #1 ($start) and argument #2 ($end)"));
                    }
                    let mut c = low;
                    if low < high {
                        while c <= high {
                            out.push(char_value(c as u8));
                            c += step;
                        }
                    } else {
                        while c >= high {
                            out.push(char_value(c as u8));
                            c -= step;
                        }
                    }
                }
                return Ok(Value::Array(out));
            }
        }
    }
    // --- numeric ranges: resolve string bounds ---
    let resolve = |ctx: &mut Ctx, b: Bound, n: usize, name: &str, other: usize, other_name: &str| -> Result<Value, Unwind> {
        Ok(match b {
            Bound::Int(i) => Value::Int(i),
            Bound::Float(f) => Value::Float(f),
            Bound::Str(s) => {
                let v = Value::string(&s);
                if v.is_numeric() {
                    v.to_number()
                } else {
                    if s.len() > 1 {
                        ctx.warn(&format!("range(): Argument #{n} (${name}) must be a single byte, subsequent bytes are ignored"))?;
                    }
                    ctx.warn(&format!(
                        "range(): Argument #{other} (${other_name}) must be a single byte string if argument #{n} (${name}) is a single byte string, argument #{n} (${name}) converted to 0"
                    ))?;
                    Value::Int(0)
                }
            }
        })
    };
    let (start, end) = match forced {
        Some(pair) => pair,
        None => (resolve(ctx, start, 1, "start", 2, "end")?, resolve(ctx, end, 2, "end", 1, "start")?),
    };
    // An integral float becomes an int bound.
    let norm = |v: Value| -> Value {
        match v {
            Value::Float(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Value::Int(f as i64),
            v => v,
        }
    };
    let (start, end) = (norm(start), norm(end));
    let mut out = Array::new();
    if matches!(start, Value::Float(_)) || matches!(end, Value::Float(_)) || is_step_double {
        let (low, high) = (start.to_float(), end.to_float());
        for (v, n, name) in [(low, 1, "start"), (high, 2, "end")] {
            if v.is_infinite() {
                return Err(Unwind::value_error(format!("range(): Argument #{n} (${name}) must be a finite number, INF provided")));
            }
            if v.is_nan() {
                return Err(Unwind::value_error(format!("range(): Argument #{n} (${name}) must be a finite number, NAN provided")));
            }
        }
        if low == high {
            out.push(Value::Float(low));
            return Ok(Value::Array(out));
        }
        if low < high && is_step_negative {
            return Err(Unwind::value_error("range(): Argument #3 ($step) must be greater than 0 for increasing ranges"));
        }
        if (high - low).abs() < step_f {
            return Err(Unwind::value_error("range(): Argument #3 ($step) must be less than the range spanned by argument #1 ($start) and argument #2 ($end)"));
        }
        let calc = ((high - low).abs() / step_f) + 1.0;
        if calc >= HT_MAX_SIZE {
            return Err(Unwind::value_error(format!(
                "The supplied range exceeds the maximum array size by {:.1} elements: start={:.1}, end={:.1}, step={:.1}. Max size: 1073741824",
                calc - HT_MAX_SIZE, low, high, step_f
            )));
        }
        let size = (calc + 0.5).floor() as usize;
        let mut i = 0usize;
        loop {
            if i >= size {
                break;
            }
            let element = if low < high { low + i as f64 * step_f } else { low - i as f64 * step_f };
            if (low < high && element > high) || (low > high && element < high) {
                break;
            }
            out.push(Value::Float(element));
            i += 1;
        }
        return Ok(Value::Array(out));
    }
    let (low, high) = (start.to_int(), end.to_int());
    if low == high {
        out.push(Value::Int(low));
        return Ok(Value::Array(out));
    }
    if low < high && is_step_negative {
        return Err(Unwind::value_error("range(): Argument #3 ($step) must be greater than 0 for increasing ranges"));
    }
    let span = (low as i128 - high as i128).unsigned_abs();
    if span < step as u128 {
        return Err(Unwind::value_error("range(): Argument #3 ($step) must be less than the range spanned by argument #1 ($start) and argument #2 ($end)"));
    }
    let calc = span / step as u128 + 1;
    if calc >= HT_MAX_SIZE as u128 {
        return Err(Unwind::value_error(format!(
            "The supplied range exceeds the maximum array size by {} elements: start={}, end={}, step={}. Calculated size: {}. Maximum size: 1073741824.",
            calc - HT_MAX_SIZE as u128, low, high, step, calc - 1
        )));
    }
    for i in 0..calc as i64 {
        out.push(Value::Int(if low < high { low.wrapping_add(i.wrapping_mul(step)) } else { low.wrapping_sub(i.wrapping_mul(step)) }));
    }
    Ok(Value::Array(out))
}

// ---- helpers ----------------------------------------------------------------

fn char_value(b: u8) -> Value {
    Value::Str(Str::from_vec(vec![b]))
}

/// Borrow the `n`-th (0-based) argument as an array, with PHP's *positional*
/// TypeError used by the variadic set operators — those omit the parameter name
/// for every argument after the first (`array_diff(): Argument #2 must be …`).
fn want_array_n<'a>(func: &str, n: usize, v: &'a Value) -> Result<&'a Array, Unwind> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #{} must be of type array, {} given",
            n + 1,
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// Append every entry of `src` to `out`, renumbering integer keys from `out`'s
/// next slot while preserving string keys — the rule `array_merge`/`array_pad`
/// apply to a single source array.
fn append_reindexed(out: &mut Array, src: &Array) {
    for (k, v) in src.iter() {
        match k {
            ArrayKey::Int(_) => push_any(out, v.clone()),
            ArrayKey::Str(_) => put(out, k.clone(), v.clone()),
        }
    }
}

pub(crate) fn array_slice(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_slice", &args[0])?;
    let n = arr.len() as i64;
    // Snapshot once; `Array::iter()` is single-pass and we index it by position.
    let entries: Vec<(&ArrayKey, &Value)> = arr.iter().collect();

    // Resolve the start offset (a negative offset counts from the end).
    let mut start = args[1].to_int();
    if start < 0 {
        start = n.saturating_add(start).max(0);
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
                n.saturating_add(l).max(start)
            } else {
                start.saturating_add(l).min(n)
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
            (false, ArrayKey::Int(_)) => push_any(&mut out, v.clone()),
            _ => put(&mut out, k.clone(), v.clone()),
        }
        i += 1;
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_flip(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_flip", &args[0])?;
    let mut out = Array::new();
    for (k, v) in entries(arr) {
        // Only int/string values can become keys; PHP warns and skips the rest.
        // A value like "5" normalizes to the int key 5 via `array_key`.
        if matches!(&*v.deref(), Value::Int(_) | Value::Str(_)) {
            if let Some(key) = array_key(&v) {
                out.set(key, k.to_value());
            }
        } else {
            ctx.warn("array_flip(): Can only flip string and integer values, entry skipped")?;
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_unique(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_unique", &args[0])?;
    let flags = args.get(1).map_or(SORT_STRING, Value::to_int);
    if flags == SORT_STRING {
        // Two values are duplicates iff their `(string)` casts are byte-equal.
        // The first occurrence wins and its key is preserved.
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        let mut out = Array::new();
        for (k, v) in entries(arr) {
            if seen.insert(crate::array2::sort_string_of(ctx, &v)?) {
                put(&mut out, k, v);
            }
        }
        return Ok(Value::Array(out));
    }
    // Other flags: sort a copy with the flag's comparator, keep the first
    // (by position) of every run of equal elements, as php does.
    let ents = entries(arr);
    let mut idx: Vec<usize> = (0..ents.len()).collect();
    let mut err: Option<Unwind> = None;
    zend_sort(&mut idx, &mut |&a, &b| {
        if err.is_some() {
            return Ordering::Equal;
        }
        match compare_values(ctx, flags, &ents[a].1, &ents[b].1) {
            Ok(o) => o,
            Err(e) => {
                err = Some(e);
                Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    let mut drop = vec![false; ents.len()];
    let mut last_kept = idx[0];
    for &cur in idx.iter().skip(1) {
        if compare_values(ctx, flags, &ents[last_kept].1, &ents[cur].1)? != Ordering::Equal {
            last_kept = cur;
        } else if last_kept > cur {
            drop[last_kept] = true;
            last_kept = cur;
        } else {
            drop[cur] = true;
        }
    }
    let mut out = Array::new();
    for (i, (k, v)) in ents.into_iter().enumerate() {
        if !drop[i] {
            put(&mut out, k, v);
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_search(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let needle = args[0].clone();
    let haystack = match &args[1] {
        Value::Array(a) => a,
        other => {
            return Err(Unwind::type_error(format!(
                "array_search(): Argument #2 ($haystack) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let strict = args.get(2).is_some_and(Value::to_bool);
    let haystack = haystack.clone();
    for (k, v) in haystack.iter() {
        let hit = if strict {
            needle.identical(v)
        } else {
            let (l, r) = ctx.cmp_operands(needle.clone(), v.clone())?;
            l.loose_eq(&r)
        };
        if hit {
            return Ok(k.to_value());
        }
    }
    Ok(Value::Bool(false))
}

pub(crate) fn array_fill(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let start = args[0].to_int();
    let count = args[1].to_int();
    if count < 0 {
        return Err(Unwind::value_error(
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

pub(crate) fn array_fill_keys(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let keys = want_array("array_fill_keys", &args[0])?;
    let value = &args[1];
    let mut out = Array::new();
    // Each *value* of `keys` becomes a key mapped to `value`: ints as they
    // are, anything else through its string form (so 1.5 is the key "1.5").
    for (_, k) in keys.iter() {
        let key = match &*k.deref() {
            Value::Int(i) => ArrayKey::Int(*i),
            other => match array_key(&Value::Str(Str::from_vec(other.to_php_bytes()))) {
                Some(key) => key,
                None => continue,
            },
        };
        out.set(key, value.clone());
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_combine(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let keys = match &args[0] {
        Value::Array(a) => a,
        other => {
            return Err(Unwind::type_error(format!(
                "array_combine(): Argument #1 ($keys) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let values = match &args[1] {
        Value::Array(a) => a,
        other => {
            return Err(Unwind::type_error(format!(
                "array_combine(): Argument #2 ($values) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    if keys.len() != values.len() {
        return Err(Unwind::value_error(
            "array_combine(): Argument #1 ($keys) and argument #2 ($values) must have the same number of elements",
        ));
    }
    let mut out = Array::new();
    for ((_, k), (_, v)) in keys.iter().zip(values.iter()) {
        // Ints stay ints; every other key goes through its string form.
        let key = match &*k.deref() {
            Value::Int(i) => ArrayKey::Int(*i),
            other => match array_key(&Value::Str(Str::from_vec(other.to_php_bytes()))) {
                Some(key) => key,
                None => continue,
            },
        };
        put(&mut out, key, v.clone());
    }
    Ok(Value::Array(out))
}

/// Store `v` under `k` the way php copies an element: a reference cell that
/// something else still holds stays a reference (`$a = [&$x]; array_values($a)`
/// still holds `$x`), a cell nobody else holds is copied as a plain value.
/// `v` is an owned clone of the element, so the source plus this clone
/// account for two handles.
pub(crate) fn put(out: &mut Array, k: ArrayKey, v: Value) {
    match v {
        Value::Ref(r) if r.strong_count() > 2 => out.set_ref(k, r),
        Value::Ref(r) => out.set(k, r.get()),
        v => out.set(k, v),
    }
}

/// Append `v` with the reference rule of [`put`].
pub(crate) fn push_any(out: &mut Array, v: Value) {
    match v {
        Value::Ref(r) if r.strong_count() > 2 => out.push_ref(r),
        Value::Ref(r) => out.push(r.get()),
        v => out.push(v),
    }
}

pub(crate) fn array_pad(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_pad", &args[0])?;
    let size = args[1].to_int();
    let value = &args[2];
    // Number of padding elements to add; if the array already meets the target
    // size PHP returns it untouched (keys preserved, no reindex).
    let pad = size.unsigned_abs() as i64 - arr.len() as i64;
    if pad <= 0 {
        return Ok(Value::Array(arr.clone()));
    }
    if size.unsigned_abs() > 1_073_741_824 {
        return Err(Unwind::value_error(
            "array_pad(): Argument #2 ($length) must not exceed the maximum allowed array size",
        ));
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

pub(crate) fn array_column(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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
    for (_, raw) in arr.iter() {
        let raw = raw.deref().into_owned();
        let row = match &raw {
            Value::Array(a) => Some(a),
            _ => None,
        };
        // Pull the column value (or the whole row, even a non-array one);
        // skip rows lacking the column.
        let val = if whole_row {
            raw.clone()
        } else {
            match row.and_then(|r| column_key.as_ref().and_then(|k| r.get(k))) {
                Some(v) => v.clone(),
                None => continue,
            }
        };
        match &index_key {
            // No index requested: append under the next integer key.
            None => out.push(val),
            // Index requested: key by the row's index value, falling back to an
            // append when the row lacks it or it is not a usable key.
            Some(ik) => match row.and_then(|r| ik.as_ref().and_then(|k| r.get(k))).and_then(array_key) {
                Some(key) => out.set(key, val),
                None => out.push(val),
            },
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_chunk(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_chunk", &args[0])?;
    let size = args[1].to_int();
    if size < 1 {
        return Err(Unwind::value_error(
            "array_chunk(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let preserve = args.get(2).is_some_and(Value::to_bool);
    let mut out = Array::new();
    let mut chunk = Array::new();
    let mut count = 0i64;
    for (k, v) in arr.iter() {
        if preserve {
            put(&mut chunk, k.clone(), v.clone());
        } else {
            push_any(&mut chunk, v.clone());
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

pub(crate) fn array_product(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_product", &args[0])?;
    // The empty-array product is the int 1, per PHP.
    let mut acc = Value::Int(1);
    for (_, v) in entries(arr) {
        if let Some(n) = binop_operand(ctx, "array_product", "Multiplication", &v)? {
            if let Ok(p) = acc.mul(&n) {
                acc = p;
            }
        }
    }
    Ok(acc)
}

/// Owned entries of an array (so a callback can run while iterating).
fn entries(a: &Array) -> Vec<(ArrayKey, Value)> {
    a.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

pub(crate) fn array_count_values(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_count_values", &args[0])?;
    let mut out = Array::new();
    for (_, v) in entries(arr) {
        // Only int/string values are countable; PHP warns and skips the rest.
        if !matches!(&*v.deref(), Value::Int(_) | Value::Str(_)) {
            ctx.warn("array_count_values(): Can only count string and integer values, entry skipped")?;
            continue;
        }
        if let Some(key) = array_key(&v) {
            let next = out.get(&key).map_or(0, Value::to_int) + 1;
            out.set(key, Value::Int(next));
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_key_first(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_key_first", &args[0])?;
    Ok(arr.iter().next().map_or(Value::Null, |(k, _)| k.to_value()))
}

pub(crate) fn array_key_last(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_key_last", &args[0])?;
    Ok(arr.iter().last().map_or(Value::Null, |(k, _)| k.to_value()))
}

pub(crate) fn array_is_list(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn array_diff(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let base = want_array("array_diff", &args[0])?;
    // Two elements are equal iff `(string)$a === (string)$b`; gather the string
    // form of every value across the remaining arrays.
    let mut others: HashSet<Vec<u8>> = HashSet::new();
    for (i, arg) in args.iter().enumerate().skip(1) {
        let a = want_array_n("array_diff", i, arg)?;
        for (_, v) in entries(a) {
            others.insert(crate::array2::sort_string_of(ctx, &v)?);
        }
    }
    let mut out = Array::new();
    for (k, v) in entries(base) {
        if !others.contains(&crate::array2::sort_string_of(ctx, &v)?) {
            put(&mut out, k, v);
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_intersect(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let base = want_array("array_intersect", &args[0])?;
    // Keep a base value iff its string form appears in *every* other array.
    let mut sets: Vec<HashSet<Vec<u8>>> = Vec::new();
    for (i, arg) in args.iter().enumerate().skip(1) {
        let a = want_array_n("array_intersect", i, arg)?;
        let mut set = HashSet::new();
        for (_, v) in entries(a) {
            set.insert(crate::array2::sort_string_of(ctx, &v)?);
        }
        sets.push(set);
    }
    let mut out = Array::new();
    for (k, v) in entries(base) {
        let s = crate::array2::sort_string_of(ctx, &v)?;
        if sets.iter().all(|set| set.contains(&s)) {
            put(&mut out, k, v);
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_replace(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // Start from a copy of the first array, then overwrite matching keys from
    // each later array in place (new keys are appended; no integer renumbering).
    let mut out = want_array("array_replace", &args[0])?.clone();
    for (i, arg) in args.iter().enumerate().skip(1) {
        let a = want_array_n("array_replace", i, arg)?;
        for (k, v) in a.iter() {
            put(&mut out, k.clone(), v.clone());
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
fn take_entries(func: &str, v: &Value) -> Result<Vec<(ArrayKey, Value)>, Unwind> {
    match v {
        Value::Array(a) => Ok(a.iter().map(|(k, val)| (k.clone(), val.clone())).collect()),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($array) must be of type array, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// Shared body for the six sort builtins: sort the entries by `key`/`value` per
/// the `SORT_*` flags (php's own sort algorithm, ties by original position),
/// then write the (reindexed or key-preserving) array back.
fn sort_impl(
    ctx: &mut Ctx,
    func: &str,
    args: &mut [Value],
    by_key: bool,
    reverse: bool,
    reindex: bool,
) -> NativeResult {
    let mut entries = take_entries(func, &args[0])?;
    let flags = args.get(1).map_or(SORT_REGULAR, Value::to_int);
    sort_entries(&mut entries, |a, b| {
        let o = if by_key {
            compare_keys(flags, &a.0, &b.0)
        } else {
            compare_values(ctx, flags, &a.1, &b.1)?
        };
        Ok(if reverse { o.reverse() } else { o })
    })?;
    // `sort`/`rsort` produce a list (every key is dropped).
    args[0] = if reindex { as_list(entries) } else { Value::Array(rebuild_entries(entries, false)) };
    Ok(Value::Bool(true))
}

/// The values of `entries` as a list (`sort()`'s result shape).
fn as_list(entries: Vec<(ArrayKey, Value)>) -> Value {
    let mut out = Array::new();
    for (_, v) in entries {
        push_any(&mut out, v);
    }
    Value::Array(out)
}

pub(crate) fn sort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, "sort", args, false, false, true)
}

pub(crate) fn rsort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, "rsort", args, false, true, true)
}

pub(crate) fn asort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, "asort", args, false, false, false)
}

pub(crate) fn arsort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, "arsort", args, false, true, false)
}

pub(crate) fn ksort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, "ksort", args, true, false, false)
}

pub(crate) fn krsort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, "krsort", args, true, true, false)
}

/// Rebuild a value array, optionally renumbering integer keys (string keys are
/// always kept). `reindex` mirrors the difference between `sort` (renumber) and
/// `asort`/`ksort` (preserve).
fn rebuild(entries: Vec<(ArrayKey, Value)>, reindex: bool) -> Value {
    Value::Array(rebuild_entries(entries, reindex))
}

pub(crate) fn array_push(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut a = match &args[0] {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "array_push(): Argument #1 ($array) must be of type array, {} given",
                rphp_runtime::value_name(&other)
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
        offset = n.saturating_add(offset).max(0);
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
                n.saturating_add(l).max(offset as i64) as usize
            } else {
                (offset as i64).saturating_add(l).min(n) as usize
            }
        }
    };
    let replacement: Vec<Value> = match args.get(3) {
        Some(Value::Array(a)) => a.iter().map(|(_, v)| v.clone()).collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.clone()],
    };

    // Reassemble: entries before the cut, then the replacement, then entries
    // after the cut — integer keys renumbered, string keys kept.
    let mut out = Array::new();
    let keep = |out: &mut Array, (k, v): &(ArrayKey, Value)| match k {
        ArrayKey::Int(_) => push_any(out, v.clone()),
        ArrayKey::Str(_) => put(out, k.clone(), v.clone()),
    };
    for e in &entries[..offset] {
        keep(&mut out, e);
    }
    for v in &replacement {
        push_any(&mut out, v.clone());
    }
    for e in &entries[end..] {
        keep(&mut out, e);
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
// These invoke a PHP callable through `ctx.call_value` (the interpreter re-entry point). The
// callable is a function-name string for now; closures arrive with the closure
// value type. Multi-array `array_map`, `array_filter` modes, and `array_walk`
// (by-ref callback) are cataloged in COVERAGE.md.

pub(crate) fn array_map(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let cb = args[0].deref().into_owned();
    let has_cb = !matches!(cb, Value::Null);
    if has_cb && !is_callable(ctx, &cb) {
        return Err(Unwind::type_error(format!(
            "array_map(): Argument #1 ($callback) must be a valid callback or null, {}",
            match &cb {
                Value::Str(s) => format!("function \"{}\" not found or invalid function name", s.to_string_lossy()),
                _ => "no array or string given".to_string(),
            }
        )));
    }
    for (i, a) in args.iter().enumerate().skip(1) {
        if !matches!(&*a.deref(), Value::Array(_)) {
            return Err(Unwind::type_error(if i == 1 {
                format!("array_map(): Argument #2 ($array) must be of type array, {} given", rphp_runtime::value_name(&a))
            } else {
                format!("array_map(): Argument #{} must be of type array, {} given", i + 1, rphp_runtime::value_name(&a))
            }));
        }
    }
    if args.len() == 2 {
        // Single-array form preserves keys; a null callback returns the array.
        let entries = take_entries("array_map", &args[1])?;
        if !has_cb {
            return Ok(args[1].deref().into_owned());
        }
        let mut out = Array::new();
        for (k, v) in entries {
            let mapped = ctx.call_value(&cb, &[v])?;
            out.set(k, mapped);
        }
        return Ok(Value::Array(out));
    }
    // Several arrays: walk them in parallel (shorter ones padded with null),
    // the result is a list; a null callback zips the rows.
    let columns: Vec<Vec<Value>> = args[1..]
        .iter()
        .map(|a| match &*a.deref() {
            Value::Array(arr) => arr.iter().map(|(_, v)| v.deref().into_owned()).collect(),
            _ => unreachable!(),
        })
        .collect();
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Array::new();
    for r in 0..rows {
        let row: Vec<Value> = columns.iter().map(|c| c.get(r).cloned().unwrap_or(Value::Null)).collect();
        if has_cb {
            out.push(ctx.call_value(&cb, &row)?);
        } else {
            let mut tuple = Array::new();
            for v in row {
                tuple.push(v);
            }
            out.push(Value::Array(tuple));
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_filter(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let entries = take_entries("array_filter", &args[0])?;
    // Without a callback, keep the truthy elements; with one, keep where it
    // returns true (fed the value, the key, or both per `$mode`). Keys are
    // preserved either way.
    let callback = args.get(1).filter(|v| !matches!(v, Value::Null)).cloned();
    if let Some(cb) = &callback {
        if !is_callable(ctx, cb) {
            return Err(Unwind::type_error(format!(
                "array_filter(): Argument #2 ($callback) must be a valid callback or null, {}",
                match cb {
                    Value::Str(s) => format!("function \"{}\" not found or invalid function name", s.to_string_lossy()),
                    _ => "no array or string given".to_string(),
                }
            )));
        }
    }
    let mode = args.get(2).map_or(0, Value::to_int);
    let mut out = Array::new();
    for (k, v) in entries {
        let keep = match &callback {
            Some(cb) => {
                let r = match mode {
                    2 => ctx.call_value(cb, &[k.to_value()])?,
                    1 => ctx.call_value(cb, &[v.clone(), k.to_value()])?,
                    _ => ctx.call_value(cb, std::slice::from_ref(&v))?,
                };
                r.to_bool()
            }
            None => v.to_bool(),
        };
        if keep {
            put(&mut out, k, v);
        }
    }
    Ok(Value::Array(out))
}

pub(crate) fn array_reduce(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let entries = take_entries("array_reduce", &args[0])?;
    let mut acc = args.get(2).cloned().unwrap_or(Value::Null);
    for (_, v) in entries {
        acc = ctx.call_value(&args[1], &[acc, v])?;
    }
    Ok(acc)
}

/// Shared body for the user-comparator sorts (`usort`/`uasort`/`uksort`): sort by
/// the callback (its sign is the order; a `bool` return is handled the
/// deprecated php way), then write back. A callback error aborts the sort and
/// surfaces. `reindex` distinguishes `usort` (renumber) from the key-preserving
/// `u*sort` pair.
fn user_sort(
    ctx: &mut Ctx,
    args: &mut [Value],
    func: &str,
    by_key: bool,
    reindex: bool,
) -> NativeResult {
    let mut entries = take_entries(func, &args[0])?;
    let cb = args[1].clone();
    if !is_callable(ctx, &cb) {
        return Err(Unwind::type_error(format!(
            "{func}(): Argument #2 ($callback) must be a valid callback, {}",
            match &cb {
                Value::Str(s) => format!("function \"{}\" not found or invalid function name", s.to_string_lossy()),
                _ => "no array or string given".to_string(),
            }
        )));
    }
    crate::array2::begin_user_sort();
    sort_entries(&mut entries, |a, b| {
        if by_key {
            user_compare(ctx, func, &cb, &a.0.to_value(), &b.0.to_value())
        } else {
            user_compare(ctx, func, &cb, &a.1, &b.1)
        }
    })?;
    args[0] = if reindex { as_list(entries) } else { rebuild(entries, false) };
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
