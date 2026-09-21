//! `SplFixedArray` (php-src `ext/spl/spl_fixedarray.c`): a vector of a fixed
//! length, indexed by `int` only, that trades `array`'s hash table for a
//! plain slot run.
//!
//! **Where the state lives.** Unlike the rest of SPL, php exposes *no*
//! property for the elements: the class has no declared slots at all
//! (`(new ReflectionClass('SplFixedArray'))->getProperties()` is empty), and
//! the numeric list a dump shows is synthesized by the `get_debug_info` /
//! `get_properties_for` handlers. So the elements live in the instance's
//! native [`Payload`] here, and [`ClassBuilder::payload_clone`] copies them
//! on `clone`.
//!
//! ```text
//! object(SplFixedArray)#1 (3) {
//!   [0]=> NULL
//!   [1]=> NULL
//!   [2]=> NULL
//! }
//! ```
//!
//! That list is what the `debug` and `cast` hooks of the class's
//! [`NativeProps`] produce — php's `get_properties_for` handler answers
//! every purpose (a dump, `(array)`, `var_export()`, `json_encode()`) with
//! the elements at their integer keys followed by the standard properties,
//! and `get_object_vars()` with the standard properties alone.
//!
//! **Known divergences.**
//!
//! * `$a[] = $v` is `Error: [] operator not supported for SplFixedArray` in
//!   php but `TypeError: Cannot access offset of type null on
//!   SplFixedArray` here: the engine routes an append to
//!   `offsetSet(null, $v)` (`objects.rs`), and php's `offsetSet(null, $v)`
//!   is exactly that `TypeError`, so one handler cannot answer both.
//! * `getIterator()` returns an `InternalIterator` over a *snapshot*, where
//!   php's is a live view onto the array (a write through the array during
//!   the loop is visible in php, and reading past the end throws
//!   `OutOfBoundsException` there instead of yielding `null`). That is the
//!   snapshot `weak.rs`'s `InternalIterator` can express.

use rphp_runtime::{
    nm, Ctx, Interp, NativeFn, NativeMethod, NativeProps, NativeResult, Registry, Unwind,
};
use rphp_value::{array_key, Array, ArrayKey, Object, Payload, Value};

/// Functions this module provides: none. `SplFixedArray` is a class, and
/// every SPL *function* lives in `spl_iterators.rs` / `spl_autoload.rs`.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides: none. `SplFixedArray` declares no class
/// constants and no global ones.
pub(crate) fn register_constants(_r: &mut Registry) {}

// ---- state -------------------------------------------------------------

/// The elements. The *size* is `items.len()`: php has no separate count, an
/// unset slot simply holds `null`.
#[derive(Default)]
struct FixedArrayState {
    items: Vec<Value>,
}

/// Run `f` on the instance's hidden state, installing an empty one first
/// when there is none — an instance built by
/// [`Interp::instantiate`](rphp_runtime::Interp::instantiate) (`unserialize`,
/// a cast) never went through `new`.
fn with_state<R>(o: &Object, f: impl FnOnce(&mut FixedArrayState) -> R) -> R {
    if o.with_payload::<FixedArrayState, _>(|_| ()).is_none() {
        o.set_payload(Payload::Native(Box::new(FixedArrayState::default())));
    }
    o.with_payload::<FixedArrayState, _>(f)
        .expect("payload installed just above")
}

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// A `static` native method row (`SplFixedArray::fromArray`).
fn static_method(
    min: u8,
    max: Option<u8>,
    params: &'static [&'static str],
    f: rphp_runtime::NativeMethodHandler,
) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params,
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

// ---- argument coercion -------------------------------------------------

/// php's `Cannot access offset of type %s on SplFixedArray`. The class name
/// is fixed: php names the class that owns the handler, not the receiver, so
/// a subclass reports `SplFixedArray` too.
fn offset_type_error(v: &Value) -> Unwind {
    Unwind::type_error(format!(
        "Cannot access offset of type {} on SplFixedArray",
        rphp_runtime::value_name(v)
    ))
}

/// An *offset*, coerced the way php's dimension handlers do — which is
/// stricter than an ordinary `int` parameter: `null` is a `TypeError` rather
/// than the 8.1 deprecation, and a string converts only when it is a
/// canonical decimal integer (`"1"` yes, `"01"`, `"+1"`, `" 1"`, `"-0"` no),
/// which is precisely [`rphp_value::array_key`]'s rule.
fn index_arg(ctx: &mut Ctx, v: &Value) -> Result<i64, Unwind> {
    match &*v.deref() {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() {
                ctx.deprecated(&format!(
                    "Implicit conversion from float {} to int loses precision",
                    Value::Float(*f).to_php_string()
                ))?;
            }
            Ok(*f as i64)
        }
        s @ Value::Str(_) => match array_key(s) {
            Some(ArrayKey::Int(i)) => Ok(i),
            _ => Err(offset_type_error(s)),
        },
        other => Err(offset_type_error(other)),
    }
}

/// php's weak `int` parameter coercion with php's diagnostics, for the two
/// declared `int $size` parameters: a numeric string or a whole float
/// converts, a fractional float is deprecated and truncated, `null` is the
/// 8.1 null-to-non-nullable deprecation, anything else is the `TypeError`
/// `zend_parse_parameters` raises.
fn int_arg(ctx: &mut Ctx, who: &str, v: &Value) -> Result<i64, Unwind> {
    match &*v.deref() {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() {
                ctx.deprecated(&format!(
                    "Implicit conversion from float {} to int loses precision",
                    Value::Float(*f).to_php_string()
                ))?;
            }
            Ok(*f as i64)
        }
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{who}(): Passing null to parameter #1 ($size) of type int is deprecated"
            ))?;
            Ok(0)
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(s.to_int()),
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #1 ($size) must be of type int, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// php's `ValueError` for a negative size.
fn negative_size(who: &str) -> Unwind {
    Unwind::value_error(format!(
        "{who}(): Argument #1 ($size) must be greater than or equal to 0"
    ))
}

/// php's `OutOfBoundsException` for an index outside the array.
fn out_of_range() -> Unwind {
    Unwind::exception("OutOfBoundsException", "Index invalid or out of range")
}

/// Grow with `null`s or truncate to `size`.
fn resize(items: &mut Vec<Value>, size: usize) {
    items.resize(size, Value::Null);
}

// ---- construction ------------------------------------------------------

/// `__construct(int $size = 0)`. php validates the size first and only then
/// bails out when the array already holds elements, so
/// `(new SplFixedArray(2))->__construct(-1)` still throws while
/// `->__construct(5)` is silently ignored.
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let size = match args.first() {
        Some(v) => int_arg(ctx, "SplFixedArray::__construct", v)?,
        None => 0,
    };
    if size < 0 {
        return Err(negative_size("SplFixedArray::__construct"));
    }
    with_state(o, |s| {
        if s.items.is_empty() {
            resize(&mut s.items, size as usize);
        }
    });
    Ok(Value::Null)
}

/// `static fromArray(array $array, bool $preserveKeys = true): SplFixedArray`
/// — with the keys preserved the size is the largest key plus one and every
/// key must be a non-negative `int`; without them the values are taken in
/// order. php builds an `SplFixedArray` even when called on a subclass.
fn from_array(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let src = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "SplFixedArray::fromArray(): Argument #1 ($array) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    let preserve = args.get(1).is_none_or(|v| v.to_bool());
    let items = if preserve {
        let mut max = -1i64;
        for k in src.keys() {
            match k {
                ArrayKey::Int(i) if *i >= 0 => max = max.max(*i),
                _ => {
                    return Err(Unwind::exception(
                        "InvalidArgumentException",
                        "array must contain only positive integer keys",
                    ))
                }
            }
        }
        let mut items = vec![Value::Null; (max + 1) as usize];
        for (k, v) in src.iter() {
            if let ArrayKey::Int(i) = k {
                items[*i as usize] = v.deref().into_owned();
            }
        }
        items
    } else {
        src.values().map(|v| v.deref().into_owned()).collect()
    };
    let cid = ctx
        .class_by_name(b"SplFixedArray")
        .expect("SplFixedArray is registered");
    let obj = ctx.new_object(cid)?;
    with_state(&obj, |s| s.items = items);
    Ok(Value::Object(obj))
}

/// `clone` copies the elements; php's copy is shallow, and so is this one.
fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let items = with_state(src, |s| s.items.clone());
    with_state(dst, |s| s.items = items);
    Ok(())
}

// ---- size and conversion -----------------------------------------------

/// `count(): int` / `getSize(): int` — php answers both with the size.
fn count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |s| s.items.len()) as i64))
}

/// `setSize(int $size): bool` — grows with `null`s or truncates, and answers
/// `true` (php returns the value, despite the `void`-looking signature).
fn set_size(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let size = int_arg(ctx, "SplFixedArray::setSize", &args[0])?;
    if size < 0 {
        return Err(negative_size("SplFixedArray::setSize"));
    }
    with_state(o, |s| resize(&mut s.items, size as usize));
    Ok(Value::Bool(true))
}

/// The elements as a packed php list.
fn elements(o: &Object) -> Array {
    let mut out = Array::new();
    with_state(o, |s| {
        for v in &s.items {
            out.push(v.clone());
        }
    });
    out
}

/// `toArray(): array`
fn to_array(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(elements(o)))
}

/// `jsonSerialize(): array` — the same list, which `json_encode` then emits
/// as a JSON array.
fn json_serialize(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    to_array(ctx, o, args)
}

// ---- ArrayAccess -------------------------------------------------------

/// `offsetExists(mixed $index): bool` — in range **and** not `null`, which
/// is what makes `isset($a[$i])` false for a slot php never filled.
fn offset_exists(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = index_arg(ctx, &args[0])?;
    let present = with_state(o, |s| {
        usize::try_from(index)
            .ok()
            .and_then(|i| s.items.get(i))
            .is_some_and(|v| !matches!(&*v.deref(), Value::Null | Value::Uninit))
    });
    Ok(Value::Bool(present))
}

/// `offsetGet(mixed $index): mixed`
fn offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = index_arg(ctx, &args[0])?;
    with_state(o, |s| {
        usize::try_from(index)
            .ok()
            .and_then(|i| s.items.get(i).cloned())
    })
    .ok_or_else(out_of_range)
}

/// `offsetSet(mixed $index, mixed $value): void`
fn offset_set(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = index_arg(ctx, &args[0])?;
    let value = args[1].deref().into_owned();
    let ok = with_state(o, |s| {
        match usize::try_from(index).ok().and_then(|i| s.items.get_mut(i)) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    });
    if ok {
        Ok(Value::Null)
    } else {
        Err(out_of_range())
    }
}

/// `offsetUnset(mixed $index): void` — the slot goes back to `null`; the
/// size never changes.
fn offset_unset(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = index_arg(ctx, &args[0])?;
    let ok = with_state(o, |s| {
        match usize::try_from(index).ok().and_then(|i| s.items.get_mut(i)) {
            Some(slot) => {
                *slot = Value::Null;
                true
            }
            None => false,
        }
    });
    if ok {
        Ok(Value::Null)
    } else {
        Err(out_of_range())
    }
}

// ---- IteratorAggregate -------------------------------------------------

/// `getIterator(): Iterator` — php hands out an `InternalIterator`, the
/// engine-private cursor class `WeakMap` also uses, keyed by the index.
fn get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let items = with_state(o, |s| {
        s.items
            .iter()
            .enumerate()
            .map(|(i, v)| (Value::Int(i as i64), v.clone()))
            .collect::<Vec<_>>()
    });
    Ok(Value::Object(crate::weak::new_internal_iterator(ctx, items)))
}

// ---- debugging and serialization ---------------------------------------

/// The elements at their integer keys, then the object's own properties by
/// name — the array php builds for both `get_debug_info` and `__serialize`.
fn elements_and_props(o: &Object) -> Array {
    let mut out = elements(o);
    for (name, value, _) in o.props_snapshot() {
        out.set(ArrayKey::Str(name.into()), value);
    }
    out
}

/// The `debug` and `cast` hooks (a dump, `(array)`, `var_export()`,
/// `json_encode()`): the elements at their integer keys, then the standard
/// properties — php's `spl_fixedarray_object_get_properties_for` order.
fn table(it: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    let mut out: Vec<(ArrayKey, Value)> =
        elements(o).iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    out.extend(it.std_property_table(o).iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

fn no_prop_get(_: &mut Interp, _: &Object, _: &[u8]) -> Option<Result<Value, Unwind>> {
    None
}

fn no_prop_set(_: &mut Interp, _: &Object, _: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    None
}

/// The class's property hooks: no computed properties, only the tables.
const PROPS: NativeProps = NativeProps {
    names: &[],
    get: no_prop_get,
    set: no_prop_set,
    isset: None,
    unset: None,
    list: None,
    debug: Some(table),
    cast: Some(table),
};

/// `__serialize(): array` — the elements at their integer keys plus any
/// property a subclass added, which is exactly what `serialize()` then
/// writes as the object's payload.
fn magic_serialize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(elements_and_props(o)))
}

/// `__unserialize(array $data): void` — the inverse split: integer keys are
/// elements (the size is the largest plus one), string keys are properties.
fn magic_unserialize(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let data = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "SplFixedArray::__unserialize(): Argument #1 ($data) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    let mut max = -1i64;
    for k in data.keys() {
        if let ArrayKey::Int(i) = k {
            max = max.max(*i);
        }
    }
    let mut items = vec![Value::Null; (max + 1).max(0) as usize];
    for (k, v) in data.iter() {
        match k {
            ArrayKey::Int(i) if *i >= 0 => items[*i as usize] = v.deref().into_owned(),
            ArrayKey::Int(_) => {}
            ArrayKey::Str(name) => o.set(name, v.deref().into_owned()),
        }
    }
    with_state(o, |s| s.items = items);
    Ok(Value::Null)
}

/// `__wakeup(): void` — obsolete since the class grew `__serialize` /
/// `__unserialize`, and deprecated as of 8.4. It does nothing but say so.
fn wakeup(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    ctx.deprecated(
        "Method SplFixedArray::__wakeup() is deprecated since 8.4, this method is obsolete, \
         as serialization hooks are provided by __unserialize() and __serialize()",
    )?;
    Ok(Value::Null)
}

// ---- registration ------------------------------------------------------

/// Register `SplFixedArray`. Runs after `spl_interfaces` (the four
/// interfaces), `spl_exceptions` (`OutOfBoundsException`,
/// `InvalidArgumentException`) and `weak` (`InternalIterator`).
pub(crate) fn register_classes(r: &mut Registry) {
    if r.0.class_by_name(b"SplFixedArray").is_some() {
        return;
    }
    r.class("SplFixedArray")
        // php 8 reaches `foreach` through `IteratorAggregate`; the class has
        // not been an `Iterator` itself since 8.0.
        .implements(&[
            "IteratorAggregate",
            "ArrayAccess",
            "Countable",
            "JsonSerializable",
        ])
        .payload_clone(payload_clone)
        .native_props(PROPS)
        .method("__construct", nm!(0, Some(1), construct))
        .method(
            "fromArray",
            static_method(1, Some(2), &["array", "preserveKeys"], from_array),
        )
        .method("count", nm!(0, Some(0), count))
        .method("getSize", nm!(0, Some(0), count))
        .method("setSize", nm!(1, Some(1), set_size))
        .method("toArray", nm!(0, Some(0), to_array))
        .method("jsonSerialize", nm!(0, Some(0), json_serialize))
        .method("offsetExists", nm!(1, Some(1), offset_exists))
        .method("offsetGet", nm!(1, Some(1), offset_get))
        .method("offsetSet", nm!(2, Some(2), offset_set))
        .method("offsetUnset", nm!(1, Some(1), offset_unset))
        .method("getIterator", nm!(0, Some(0), get_iterator))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("__wakeup", nm!(0, Some(0), wakeup))
        .finish();
}

#[cfg(test)]
mod tests {
    use rphp_value::{ArrayKey, Value};

    use crate::tests::interp;

    /// `new SplFixedArray($size)`.
    fn fixed(it: &mut rphp_runtime::Interp, size: i64) -> rphp_value::Object {
        let cid = it.class_by_name(b"SplFixedArray").unwrap();
        let o = it.new_object(cid).unwrap();
        it.call_method(&o, b"__construct", &[Value::Int(size)]).unwrap();
        o
    }

    #[test]
    fn a_fresh_array_is_a_run_of_nulls() {
        let mut it = interp();
        let o = fixed(&mut it, 3);
        assert_eq!(it.call_method(&o, b"count", &[]).unwrap(), Value::Int(3));
        let Value::Array(a) = it.call_method(&o, b"toArray", &[]).unwrap() else {
            panic!("toArray returns an array")
        };
        assert_eq!(a.len(), 3);
        assert_eq!(a.get_deref(&ArrayKey::Int(0)), Some(Value::Null));
    }

    #[test]
    fn offset_exists_is_false_for_a_null_slot() {
        let mut it = interp();
        let o = fixed(&mut it, 2);
        it.call_method(&o, b"offsetSet", &[Value::Int(0), Value::string(b"a")])
            .unwrap();
        assert_eq!(
            it.call_method(&o, b"offsetExists", &[Value::Int(0)]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            it.call_method(&o, b"offsetExists", &[Value::Int(1)]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            it.call_method(&o, b"offsetExists", &[Value::Int(5)]).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn an_index_outside_the_array_is_out_of_bounds() {
        let mut it = interp();
        let o = fixed(&mut it, 1);
        let err = it.call_method(&o, b"offsetGet", &[Value::Int(4)]).unwrap_err();
        assert_eq!(err.class_name().as_deref(), Some("OutOfBoundsException"));
        assert_eq!(err.message(), Some("Index invalid or out of range"));
        // `null` is a TypeError, not the ordinary int coercion.
        let err = it.call_method(&o, b"offsetGet", &[Value::Null]).unwrap_err();
        assert_eq!(
            err.message(),
            Some("Cannot access offset of type null on SplFixedArray")
        );
    }

    #[test]
    fn set_size_grows_with_nulls_and_truncates() {
        let mut it = interp();
        let o = fixed(&mut it, 1);
        it.call_method(&o, b"offsetSet", &[Value::Int(0), Value::Int(7)])
            .unwrap();
        it.call_method(&o, b"setSize", &[Value::Int(3)]).unwrap();
        assert_eq!(it.call_method(&o, b"count", &[]).unwrap(), Value::Int(3));
        it.call_method(&o, b"setSize", &[Value::Int(1)]).unwrap();
        assert_eq!(
            it.call_method(&o, b"offsetGet", &[Value::Int(0)]).unwrap(),
            Value::Int(7)
        );
        let err = it.call_method(&o, b"setSize", &[Value::Int(-1)]).unwrap_err();
        assert_eq!(
            err.message(),
            Some("SplFixedArray::setSize(): Argument #1 ($size) must be greater than or equal to 0")
        );
    }

    #[test]
    fn from_array_preserves_keys_by_default() {
        let mut it = interp();
        let cid = it.class_by_name(b"SplFixedArray").unwrap();
        let mut src = rphp_value::Array::new();
        src.set(ArrayKey::Int(3), Value::string(b"d"));
        src.set(ArrayKey::Int(1), Value::string(b"b"));
        let made = it
            .call_static_method(cid, b"fromArray", &[Value::Array(src.clone())])
            .unwrap();
        let Value::Object(made) = made else { panic!("fromArray returns an object") };
        assert_eq!(it.call_method(&made, b"count", &[]).unwrap(), Value::Int(4));
        let packed = it
            .call_static_method(cid, b"fromArray", &[Value::Array(src), Value::Bool(false)])
            .unwrap();
        let Value::Object(packed) = packed else { panic!("fromArray returns an object") };
        assert_eq!(it.call_method(&packed, b"count", &[]).unwrap(), Value::Int(2));
    }

    #[test]
    fn clone_copies_the_elements() {
        let mut it = interp();
        let o = fixed(&mut it, 1);
        it.call_method(&o, b"offsetSet", &[Value::Int(0), Value::Int(1)])
            .unwrap();
        let copy = it.clone_object_with(&Value::Object(o.clone()), None).unwrap();
        let Value::Object(copy) = copy else { panic!("clone yields an object") };
        it.call_method(&copy, b"offsetSet", &[Value::Int(0), Value::Int(2)])
            .unwrap();
        assert_eq!(
            it.call_method(&o, b"offsetGet", &[Value::Int(0)]).unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            it.call_method(&copy, b"offsetGet", &[Value::Int(0)]).unwrap(),
            Value::Int(2)
        );
    }
}
