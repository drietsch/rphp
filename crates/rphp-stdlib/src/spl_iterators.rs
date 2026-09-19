//! The SPL iterator helpers (php-src `ext/spl/php_spl.c`): turning any
//! `Traversable` — including a `Generator` — into an array, counting it, or
//! walking it with a callback.
//!
//! All three go through the engine's iteration path, so an `Iterator`, an
//! `IteratorAggregate` and a generator behave identically here.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{array_key, Array, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("iterator_to_array", 1, Some(2), iterator_to_array),
    nf!("iterator_count", 1, Some(1), iterator_count),
    nf!("iterator_apply", 2, Some(3), iterator_apply),
];

/// Drain any iterable into `(key, value)` pairs. An array is already one.
fn pairs(ctx: &mut Ctx, v: &Value) -> Result<Vec<(Value, Value)>, Unwind> {
    match &*v.deref() {
        Value::Array(a) => Ok(a
            .iter()
            .map(|(k, val)| (k.to_value(), val.deref().into_owned()))
            .collect()),
        Value::Object(o) => {
            let o = o.clone();
            ctx.iterate_traversable(&o)
        }
        other => Err(Unwind::type_error(format!(
            "iterator_to_array(): Argument #1 ($iterator) must be of type Traversable|array, {} given",
            other.type_name()
        ))),
    }
}

/// `iterator_to_array(Traversable|array $iterator, bool $preserve_keys = true): array`
fn iterator_to_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let preserve = args.get(1).is_none_or(Value::to_bool);
    let items = pairs(ctx, &args[0])?;
    let mut out = Array::new();
    for (k, v) in items {
        if preserve {
            match array_key(&k) {
                Some(key) => out.set(key, v),
                None => out.push(v),
            }
        } else {
            out.push(v);
        }
    }
    Ok(Value::Array(out))
}

/// `iterator_count(Traversable|array $iterator): int`
fn iterator_count(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(pairs(ctx, &args[0])?.len() as i64))
}

/// `iterator_apply(Traversable $iterator, callable $callback, ?array $args = null): int`
///
/// php calls `$callback` once per element **without** passing the element —
/// the callback reads it from the iterator itself — and stops early when the
/// callback returns a falsy value. The return is the number of calls made.
fn iterator_apply(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let callback = args[1].clone();
    let extra: Vec<Value> = match args.get(2).map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => a.iter().map(|(_, v)| v.deref().into_owned()).collect(),
        _ => Vec::new(),
    };
    let items = pairs(ctx, &args[0])?;
    let mut n = 0i64;
    for _ in items {
        n += 1;
        if !ctx.call_value(&callback, &extra)?.to_bool() {
            break;
        }
    }
    Ok(Value::Int(n))
}
