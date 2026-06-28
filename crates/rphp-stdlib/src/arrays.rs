//! Array builtins, built directly on [`rphp_value::Array`] so key coercion,
//! ordering, and loose/strict comparison match the engine exactly.
use rphp_value::{array_key, Array, ArrayKey, Str, Value};

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

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
