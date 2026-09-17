//! `ArrayIterator` and `ArrayObject` (php-src `ext/spl/spl_array.c`) — the
//! two SPL containers everything else leans on.
//!
//! **Where the state lives.** php exposes the backing array of both classes
//! as a *private* property named `storage`, declared on the class itself:
//!
//! ```text
//! object(ArrayObject)#1 (1) {
//!   ["storage":"ArrayObject":private]=>
//!   array(2) { … }
//! }
//! ```
//!
//! So that is exactly how it is stored here — a real private slot — and
//! `var_dump`, `print_r` and `(array)` match php without a special case.
//! The parts php does *not* show (the `ARRAY_AS_PROPS` flag word, the
//! iterator cursor, `ArrayObject`'s iterator class) live in the instance's
//! native [`Payload`], where they stay invisible, as in php.
//!
//! The two classes share almost every method; the implementations below are
//! written once and registered on both.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{array_key, Array, ArrayKey, Object, Payload, Value};

/// Everything about an instance php keeps out of sight.
struct ContainerState {
    /// The `STD_PROP_LIST` / `ARRAY_AS_PROPS` word.
    flags: i64,
    /// The iterator cursor as a *raw* position into the backing array
    /// (`Array::raw_entry` / `Array::next_live_from`), so an `unset` during
    /// iteration does not shift the cursor — php's `HashPosition`.
    pos: usize,
    /// `ArrayObject::setIteratorClass`.
    iterator_class: Vec<u8>,
}

impl Default for ContainerState {
    fn default() -> ContainerState {
        ContainerState {
            flags: 0,
            pos: 0,
            iterator_class: b"ArrayIterator".to_vec(),
        }
    }
}

/// Run `f` on the instance's hidden state, installing the default first when
/// there is none (an instance built by [`rphp_runtime::Interp::instantiate`]
/// — `unserialize`, a cast — has no payload).
fn with_state<R>(o: &Object, f: impl FnOnce(&mut ContainerState) -> R) -> R {
    let present = o.with_payload::<ContainerState, _>(|_| ()).is_some();
    if !present {
        o.set_payload(Payload::Native(Box::new(ContainerState::default())));
    }
    o.with_payload::<ContainerState, _>(f)
        .expect("payload installed just above")
}

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// The backing array.
fn storage(o: &Object) -> Array {
    match o.get_deref(b"storage") {
        Some(Value::Array(a)) => a,
        _ => Array::new(),
    }
}

/// Replace the backing array.
fn set_storage(o: &Object, a: Array) {
    o.set(b"storage", Value::Array(a));
}

/// An offset argument as an array key (php: `Illegal offset type` for an
/// array or object offset).
fn offset(v: &Value) -> Result<ArrayKey, Unwind> {
    array_key(v).ok_or_else(|| Unwind::type_error("Illegal offset type"))
}

/// php's `Undefined array key <k>` warning text.
fn undefined_key(k: &ArrayKey) -> String {
    match k {
        ArrayKey::Int(i) => format!("Undefined array key {i}"),
        ArrayKey::Str(s) => format!("Undefined array key \"{}\"", String::from_utf8_lossy(s)),
    }
}

/// The backing array a constructor / `exchangeArray` argument supplies.
/// `$array` may be an array, or — deprecated since 8.something and warned
/// about by php — an object, whose properties become the entries.
fn backing(ctx: &mut Ctx, who: &str, arg: &Value) -> Result<Array, Unwind> {
    match &*arg.deref() {
        Value::Array(a) => Ok(a.clone()),
        Value::Object(src) => {
            ctx.deprecated(&format!(
                "{who}(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants"
            ))?;
            // An `ArrayObject`/`ArrayIterator` source contributes its
            // backing array, anything else its properties.
            if let Some(Value::Array(a)) = src.get_deref(b"storage") {
                return Ok(a);
            }
            let mut out = Array::new();
            for (name, value, _) in src.props_snapshot() {
                out.set(array_key(&Value::string(&name)).expect("a property name is a valid key"), value);
            }
            Ok(out)
        }
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #1 ($array) must be of type array, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

// ---- construction ------------------------------------------------------

/// The shared constructor body: `$array`, `$flags`, and (ArrayObject only)
/// `$iteratorClass`.
fn construct(ctx: &mut Ctx, who: &str, o: &Object, args: &mut [Value]) -> NativeResult {
    let first = args.first().cloned();
    if let Some(a) = first {
        let items = backing(ctx, who, &a)?;
        set_storage(o, items);
    }
    let flags = args.get(1).map(|f| f.to_int());
    let iter_class = args.get(2).map(|c| c.to_php_bytes());
    with_state(o, |s| {
        if let Some(f) = flags {
            s.flags = f;
        }
        if let Some(c) = iter_class {
            s.iterator_class = c;
        }
    });
    Ok(Value::Null)
}

fn array_iterator_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    construct(ctx, "ArrayIterator::__construct", o, args)
}

fn array_object_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    construct(ctx, "ArrayObject::__construct", o, args)
}

// ---- ArrayAccess -------------------------------------------------------

/// `offsetExists(mixed $key): bool` — php tests *key presence*, not `isset`
/// semantics: a stored `null` still exists.
fn offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let k = offset(&args[0])?;
    Ok(Value::Bool(storage(o).contains_key(&k)))
}

/// `offsetGet(mixed $key): mixed` — a missing key warns and yields null.
fn offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let k = offset(&args[0])?;
    match storage(o).get_deref(&k) {
        Some(v) => Ok(v),
        None => {
            ctx.warn(&undefined_key(&k))?;
            Ok(Value::Null)
        }
    }
}

/// `offsetSet(mixed $key, mixed $value): void` — a null key appends.
fn offset_set(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let value = args[1].deref().into_owned();
    let mut a = storage(o);
    match &*args[0].deref() {
        Value::Null | Value::Uninit => a.push(value),
        k => {
            let k = offset(k)?;
            a.set(k, value);
        }
    }
    set_storage(o, a);
    Ok(Value::Null)
}

/// `offsetUnset(mixed $key): void` — removing an absent key is a no-op.
fn offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let k = offset(&args[0])?;
    let mut a = storage(o);
    a.unset(&k);
    set_storage(o, a);
    Ok(Value::Null)
}

/// `append(mixed $value): void`
fn append(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let v = args[0].deref().into_owned();
    let mut a = storage(o);
    a.push(v);
    set_storage(o, a);
    Ok(Value::Null)
}

// ---- Countable and the array accessors ---------------------------------

/// `count(): int`
fn count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(storage(o).len() as i64))
}

/// `getArrayCopy(): array`
fn get_array_copy(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(storage(o)))
}

/// `getFlags(): int`
fn get_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |s| s.flags)))
}

/// `setFlags(int $flags): void`
fn set_flags(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let f = args[0].to_int();
    with_state(o, |s| s.flags = f);
    Ok(Value::Null)
}

/// `exchangeArray(object|array $array): array` — swap the backing array in,
/// returning the old one.
fn exchange_array(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let old = storage(o);
    let a = args[0].clone();
    let new = backing(ctx, "ArrayObject::exchangeArray", &a)?;
    set_storage(o, new);
    Ok(Value::Array(old))
}

// ---- sorting -----------------------------------------------------------

/// Run one of the array-sorting builtins over the backing array and store
/// the result. The builtin takes its subject by reference, so `call_native`
/// writes the sorted array straight back into `argv[0]`.
///
/// Divergence: a fault raised inside the sort names the builtin
/// (`uasort(): …`) where php names the method (`ArrayObject::uasort(): …`).
fn sort_with(ctx: &mut Ctx, o: &Object, native: &[u8], extra: &[Value]) -> NativeResult {
    let id = ctx
        .native_by_name(native)
        .expect("the array sorting builtins are registered");
    let mut argv: Vec<Value> = Vec::with_capacity(1 + extra.len());
    argv.push(Value::Array(storage(o)));
    argv.extend(extra.iter().cloned());
    ctx.call_native(id, &mut argv)?;
    if let Value::Array(a) = &argv[0] {
        set_storage(o, a.clone());
    }
    Ok(Value::Bool(true))
}

fn asort(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let extra: Vec<Value> = args.first().map(|f| vec![Value::Int(f.to_int())]).unwrap_or_default();
    sort_with(ctx, o, b"asort", &extra)
}

fn ksort(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let extra: Vec<Value> = args.first().map(|f| vec![Value::Int(f.to_int())]).unwrap_or_default();
    sort_with(ctx, o, b"ksort", &extra)
}

fn uasort(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cb = args[0].clone();
    sort_with(ctx, o, b"uasort", &[cb])
}

fn uksort(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cb = args[0].clone();
    sort_with(ctx, o, b"uksort", &[cb])
}

fn natsort(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    sort_with(ctx, o, b"natsort", &[])
}

fn natcasesort(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    sort_with(ctx, o, b"natcasesort", &[])
}

// ---- Iterator (ArrayIterator) ------------------------------------------

/// The `(key, value)` under the cursor, if the cursor is on a live entry.
fn at_cursor(o: &Object) -> Option<(Value, Value)> {
    let a = storage(o);
    let pos = with_state(o, |s| s.pos);
    a.raw_entry(pos).map(|(k, v)| (k.to_value(), v.deref().into_owned()))
}

/// `rewind(): void`
fn rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let a = storage(o);
    let first = a.next_live_from(0).map(|(p, _, _)| p).unwrap_or(a.raw_len());
    with_state(o, |s| s.pos = first);
    Ok(Value::Null)
}

/// `valid(): bool`
fn valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(at_cursor(o).is_some()))
}

/// `current(): mixed`
fn current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(at_cursor(o).map_or(Value::Null, |(_, v)| v))
}

/// `key(): mixed`
fn key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(at_cursor(o).map_or(Value::Null, |(k, _)| k))
}

/// `next(): void`
fn next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let a = storage(o);
    let pos = with_state(o, |s| s.pos);
    let following = a
        .next_live_from(pos.saturating_add(1))
        .map(|(p, _, _)| p)
        .unwrap_or(a.raw_len());
    with_state(o, |s| s.pos = following);
    Ok(Value::Null)
}

/// `seek(int $offset): void` — position the cursor on the `$offset`-th live
/// entry; php throws `OutOfBoundsException` past the end.
fn seek(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let want = args[0].to_int();
    let a = storage(o);
    let mut found = None;
    if want >= 0 {
        let mut raw = 0usize;
        let mut n = 0i64;
        while let Some((p, _, _)) = a.next_live_from(raw) {
            if n == want {
                found = Some(p);
                break;
            }
            n += 1;
            raw = p + 1;
        }
    }
    match found {
        Some(p) => {
            with_state(o, |s| s.pos = p);
            Ok(Value::Null)
        }
        None => Err(Unwind::exception(
            "OutOfBoundsException",
            format!("Seek position {want} is out of range"),
        )),
    }
}

// ---- ArrayObject only --------------------------------------------------

/// `getIterator(): Iterator` — a fresh instance of the iterator class over
/// the current backing array.
///
/// Divergence: php's iterator shares the backing array with the
/// `ArrayObject`, so writes through one are visible in the other; this one
/// gets a copy-on-write snapshot.
fn get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (name, flags) = with_state(o, |s| (s.iterator_class.clone(), s.flags));
    let cid = ctx.class_by_name(&name).ok_or_else(|| {
        Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(&name)
        ))
    })?;
    let it = ctx.new_object(cid)?;
    let args = [Value::Array(storage(o)), Value::Int(flags)];
    ctx.call_method(&it, b"__construct", &args)?;
    Ok(Value::Object(it))
}

/// `getIteratorClass(): string`
fn get_iterator_class(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::string(&with_state(o, |s| s.iterator_class.clone())))
}

/// `setIteratorClass(string $iteratorClass): void`
fn set_iterator_class(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = args[0].to_php_bytes();
    if ctx.class_by_name(&name).is_none() {
        return Err(Unwind::exception(
            "InvalidArgumentException",
            format!(
                "Class \"{}\" does not exist",
                String::from_utf8_lossy(&name)
            ),
        ));
    }
    with_state(o, |s| s.iterator_class = name);
    Ok(Value::Null)
}

// ---- debugging and serialization ---------------------------------------

/// `__debugInfo(): array` — php reports the backing array under the mangled
/// private name (`"\0ArrayObject\0storage"`).
fn debug_info(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut name = vec![0u8];
    name.extend_from_slice(ctx.class_name_of(o).as_bytes());
    name.push(0);
    name.extend_from_slice(b"storage");
    let mut out = Array::new();
    out.set(ArrayKey::Str(name.into_boxed_slice()), Value::Array(storage(o)));
    Ok(Value::Array(out))
}

/// `__serialize(): array` — php's `[flags, storage, properties, iterator
/// class]` quadruple.
fn magic_serialize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (flags, iter_class) = with_state(o, |s| (s.flags, s.iterator_class.clone()));
    let mut out = Array::new();
    out.push(Value::Int(flags));
    out.push(Value::Array(storage(o)));
    out.push(Value::empty_array());
    out.push(if iter_class.as_slice() == &b"ArrayIterator"[..] {
        Value::Null
    } else {
        Value::string(&iter_class)
    });
    Ok(Value::Array(out))
}

/// `__unserialize(array $data): void`
fn magic_unserialize(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let data = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "__unserialize(): Argument #1 ($data) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    if let Some(f) = data.get_deref(&ArrayKey::Int(0)) {
        let f = f.to_int();
        with_state(o, |s| s.flags = f);
    }
    match data.get_deref(&ArrayKey::Int(1)) {
        Some(Value::Array(a)) => set_storage(o, a),
        _ => set_storage(o, Array::new()),
    }
    if let Some(Value::Str(c)) = data.get_deref(&ArrayKey::Int(3)) {
        let name = c.as_bytes().to_vec();
        with_state(o, |s| s.iterator_class = name);
    }
    Ok(Value::Null)
}


// ---- SplObjectStorage --------------------------------------------------
//
// php stores the set in a private `storage` property too, as a list of
// `["obj" => $object, "inf" => $info]` pairs — which is exactly what
// `var_dump` prints — so that is the representation here. Lookup is by
// object identity over that list; the iterator cursor is the list index and
// lives in the hidden [`ContainerState::pos`].

/// The entry list as `(object, info)` pairs.
fn sos_load(o: &Object) -> Vec<(Object, Value)> {
    let entries = storage(o);
    entries
        .values()
        .filter_map(|v| match &*v.deref() {
            Value::Array(e) => {
                let obj = match e.get_deref(&ArrayKey::str(b"obj")) {
                    Some(Value::Object(x)) => x,
                    _ => return None,
                };
                let inf = e.get_deref(&ArrayKey::str(b"inf")).unwrap_or(Value::Null);
                Some((obj, inf))
            }
            _ => None,
        })
        .collect()
}

/// Write the entry list back in php's shape.
fn sos_store(o: &Object, items: &[(Object, Value)]) {
    let mut a = Array::new();
    for (obj, inf) in items {
        let mut e = Array::new();
        e.set(ArrayKey::str(b"obj"), Value::Object(obj.clone()));
        e.set(ArrayKey::str(b"inf"), inf.clone());
        a.push(Value::Array(e));
    }
    set_storage(o, a);
}

/// An `$object` argument (every `SplObjectStorage` entry point takes one).
fn sos_object_arg(who: &str, v: &Value) -> Result<Object, Unwind> {
    match &*v.deref() {
        Value::Object(o) => Ok(o.clone()),
        other => Err(Unwind::type_error(format!(
            "SplObjectStorage::{who}(): Argument #1 ($object) must be of type object, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// `offsetSet(object $object, mixed $info = null): void`
fn sos_offset_set(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetSet", &args[0])?;
    let info = args.get(1).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    let mut items = sos_load(o);
    let found = items.iter().position(|(k, _)| k.ptr_eq(&key));
    match found {
        Some(i) => items[i].1 = info,
        None => items.push((key, info)),
    }
    sos_store(o, &items);
    Ok(Value::Null)
}

/// `offsetGet(object $object): mixed` — php throws when the object is absent.
fn sos_offset_get(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetGet", &args[0])?;
    let items = sos_load(o);
    match items.iter().find(|(k, _)| k.ptr_eq(&key)) {
        Some((_, info)) => Ok(info.clone()),
        None => Err(Unwind::exception("UnexpectedValueException", "Object not found")),
    }
}

/// `offsetExists(object $object): bool`
fn sos_offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetExists", &args[0])?;
    Ok(Value::Bool(sos_load(o).iter().any(|(k, _)| k.ptr_eq(&key))))
}

/// `offsetUnset(object $object): void`
fn sos_offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetUnset", &args[0])?;
    let mut items = sos_load(o);
    let found = items.iter().position(|(k, _)| k.ptr_eq(&key));
    if let Some(i) = found {
        items.remove(i);
        sos_store(o, &items);
    }
    Ok(Value::Null)
}

/// `count(int $mode = COUNT_NORMAL): int`
fn sos_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(sos_load(o).len() as i64))
}

/// `getHash(object $object): string` — php's `spl_object_hash` shape: the
/// object handle in 16 hex digits followed by 16 zeros.
fn sos_get_hash(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let _ = this(o)?;
    let target = sos_object_arg("getHash", &args[0])?;
    Ok(Value::string(
        format!("{:016x}{:016x}", target.id(), 0).as_bytes(),
    ))
}

/// The deprecated 8.5 aliases: `attach`/`detach`/`contains`.
fn sos_deprecated(ctx: &mut Ctx, old: &str, new: &str) -> Result<(), Unwind> {
    ctx.deprecated(&format!(
        "Method SplObjectStorage::{old}() is deprecated since 8.5, use method SplObjectStorage::{new}() instead"
    ))
}

fn sos_attach(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    sos_deprecated(ctx, "attach", "offsetSet")?;
    sos_offset_set(ctx, o, args)
}

fn sos_detach(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    sos_deprecated(ctx, "detach", "offsetUnset")?;
    sos_offset_unset(ctx, o, args)
}

fn sos_contains(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    sos_deprecated(ctx, "contains", "offsetExists")?;
    sos_offset_exists(ctx, o, args)
}

/// The `SplObjectStorage` argument `addAll`/`removeAll`/`removeAllExcept`
/// take.
fn sos_storage_arg(ctx: &Ctx, who: &str, v: &Value) -> Result<Vec<(Object, Value)>, Unwind> {
    let sos = ctx
        .class_by_name(b"SplObjectStorage")
        .expect("SplObjectStorage is registered");
    match &*v.deref() {
        Value::Object(other) if ctx.object_instanceof(other, sos) => Ok(sos_load(other)),
        other => Err(Unwind::type_error(format!(
            "SplObjectStorage::{who}(): Argument #1 ($storage) must be of type SplObjectStorage, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// `addAll(SplObjectStorage $storage): int` — returns the resulting count.
fn sos_add_all(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let incoming = sos_storage_arg(ctx, "addAll", &args[0])?;
    let mut items = sos_load(o);
    for (obj, inf) in incoming {
        let found = items.iter().position(|(k, _)| k.ptr_eq(&obj));
        match found {
            Some(i) => items[i].1 = inf,
            None => items.push((obj, inf)),
        }
    }
    sos_store(o, &items);
    Ok(Value::Int(items.len() as i64))
}

/// `removeAll(SplObjectStorage $storage): int`
fn sos_remove_all(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let drop_these = sos_storage_arg(ctx, "removeAll", &args[0])?;
    let mut items = sos_load(o);
    items.retain(|(k, _)| !drop_these.iter().any(|(d, _)| d.ptr_eq(k)));
    sos_store(o, &items);
    Ok(Value::Int(items.len() as i64))
}

/// `removeAllExcept(SplObjectStorage $storage): int`
fn sos_remove_all_except(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let keep = sos_storage_arg(ctx, "removeAllExcept", &args[0])?;
    let mut items = sos_load(o);
    items.retain(|(k, _)| keep.iter().any(|(d, _)| d.ptr_eq(k)));
    sos_store(o, &items);
    Ok(Value::Int(items.len() as i64))
}

/// `getInfo(): mixed` — the info of the entry under the cursor.
fn sos_get_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    Ok(sos_load(o)
        .get(pos)
        .map_or(Value::Null, |(_, inf)| inf.clone()))
}

/// `setInfo(mixed $info): void`
fn sos_set_info(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    let info = args[0].deref().into_owned();
    let mut items = sos_load(o);
    if pos < items.len() {
        items[pos].1 = info;
        sos_store(o, &items);
    }
    Ok(Value::Null)
}

fn sos_rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    with_state(o, |s| s.pos = 0);
    Ok(Value::Null)
}

fn sos_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    Ok(Value::Bool(pos < sos_load(o).len()))
}

fn sos_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |s| s.pos) as i64))
}

fn sos_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    Ok(sos_load(o)
        .get(pos)
        .map_or(Value::Null, |(obj, _)| Value::Object(obj.clone())))
}

fn sos_next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    with_state(o, |s| s.pos += 1);
    Ok(Value::Null)
}

/// `seek(int $offset): void`
fn sos_seek(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let want = args[0].to_int();
    let len = sos_load(o).len() as i64;
    if want < 0 || want >= len {
        return Err(Unwind::exception(
            "OutOfBoundsException",
            format!("Seek position {want} is out of range"),
        ));
    }
    with_state(o, |s| s.pos = want as usize);
    Ok(Value::Null)
}

/// `__serialize(): array` — php's `[[[object, info], …], []]`.
fn sos_magic_serialize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut entries = Array::new();
    for (obj, inf) in sos_load(o) {
        let mut pair = Array::new();
        pair.push(Value::Object(obj));
        pair.push(inf);
        entries.push(Value::Array(pair));
    }
    let mut out = Array::new();
    out.push(Value::Array(entries));
    out.push(Value::empty_array());
    Ok(Value::Array(out))
}

/// `__unserialize(array $data): void`
fn sos_magic_unserialize(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let data = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "SplObjectStorage::__unserialize(): Argument #1 ($data) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    let mut items: Vec<(Object, Value)> = Vec::new();
    if let Some(Value::Array(entries)) = data.get_deref(&ArrayKey::Int(0)) {
        for v in entries.values() {
            if let Value::Array(pair) = &*v.deref() {
                if let Some(Value::Object(obj)) = pair.get_deref(&ArrayKey::Int(0)) {
                    let inf = pair.get_deref(&ArrayKey::Int(1)).unwrap_or(Value::Null);
                    items.push((obj, inf));
                }
            }
        }
    }
    sos_store(o, &items);
    Ok(Value::Null)
}

// ---- registration ------------------------------------------------------

/// Register `ArrayIterator` and `ArrayObject` (after `spl_interfaces` and
/// `spl_exceptions` — `seek` throws `OutOfBoundsException`).
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ArrayIterator")
        .implements(&["SeekableIterator", "ArrayAccess", "Serializable", "Countable"])
        .prop("storage", Visibility::Private, Value::empty_array())
        .method("__construct", nm!(0, Some(2), array_iterator_construct))
        .method("offsetExists", nm!(1, Some(1), offset_exists))
        .method("offsetGet", nm!(1, Some(1), offset_get))
        .method("offsetSet", nm!(2, Some(2), offset_set))
        .method("offsetUnset", nm!(1, Some(1), offset_unset))
        .method("append", nm!(1, Some(1), append))
        .method("getArrayCopy", nm!(0, Some(0), get_array_copy))
        .method("count", nm!(0, Some(0), count))
        .method("getFlags", nm!(0, Some(0), get_flags))
        .method("setFlags", nm!(1, Some(1), set_flags))
        .method("asort", nm!(0, Some(1), asort))
        .method("ksort", nm!(0, Some(1), ksort))
        .method("uasort", nm!(1, Some(1), uasort))
        .method("uksort", nm!(1, Some(1), uksort))
        .method("natsort", nm!(0, Some(0), natsort))
        .method("natcasesort", nm!(0, Some(0), natcasesort))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("rewind", nm!(0, Some(0), rewind))
        .method("current", nm!(0, Some(0), current))
        .method("key", nm!(0, Some(0), key))
        .method("next", nm!(0, Some(0), next))
        .method("valid", nm!(0, Some(0), valid))
        .method("seek", nm!(1, Some(1), seek))
        .method("__debugInfo", nm!(0, Some(0), debug_info))
        .finish();

    r.class("ArrayObject")
        .implements(&["IteratorAggregate", "ArrayAccess", "Serializable", "Countable"])
        .prop("storage", Visibility::Private, Value::empty_array())
        .method("__construct", nm!(0, Some(3), array_object_construct))
        .method("offsetExists", nm!(1, Some(1), offset_exists))
        .method("offsetGet", nm!(1, Some(1), offset_get))
        .method("offsetSet", nm!(2, Some(2), offset_set))
        .method("offsetUnset", nm!(1, Some(1), offset_unset))
        .method("append", nm!(1, Some(1), append))
        .method("getArrayCopy", nm!(0, Some(0), get_array_copy))
        .method("count", nm!(0, Some(0), count))
        .method("getFlags", nm!(0, Some(0), get_flags))
        .method("setFlags", nm!(1, Some(1), set_flags))
        .method("asort", nm!(0, Some(1), asort))
        .method("ksort", nm!(0, Some(1), ksort))
        .method("uasort", nm!(1, Some(1), uasort))
        .method("uksort", nm!(1, Some(1), uksort))
        .method("natsort", nm!(0, Some(0), natsort))
        .method("natcasesort", nm!(0, Some(0), natcasesort))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("getIterator", nm!(0, Some(0), get_iterator))
        .method("exchangeArray", nm!(1, Some(1), exchange_array))
        .method("setIteratorClass", nm!(1, Some(1), set_iterator_class))
        .method("getIteratorClass", nm!(0, Some(0), get_iterator_class))
        .method("__debugInfo", nm!(0, Some(0), debug_info))
        .finish();

    r.class("SplObjectStorage")
        .implements(&["Countable", "SeekableIterator", "Serializable", "ArrayAccess"])
        .prop("storage", Visibility::Private, Value::empty_array())
        .method("attach", nm!(1, Some(2), sos_attach))
        .method("detach", nm!(1, Some(1), sos_detach))
        .method("contains", nm!(1, Some(1), sos_contains))
        .method("addAll", nm!(1, Some(1), sos_add_all))
        .method("removeAll", nm!(1, Some(1), sos_remove_all))
        .method("removeAllExcept", nm!(1, Some(1), sos_remove_all_except))
        .method("getInfo", nm!(0, Some(0), sos_get_info))
        .method("setInfo", nm!(1, Some(1), sos_set_info))
        .method("count", nm!(0, Some(1), sos_count))
        .method("rewind", nm!(0, Some(0), sos_rewind))
        .method("valid", nm!(0, Some(0), sos_valid))
        .method("key", nm!(0, Some(0), sos_key))
        .method("current", nm!(0, Some(0), sos_current))
        .method("next", nm!(0, Some(0), sos_next))
        .method("seek", nm!(1, Some(1), sos_seek))
        .method("offsetExists", nm!(1, Some(1), sos_offset_exists))
        .method("offsetGet", nm!(1, Some(1), sos_offset_get))
        .method("offsetSet", nm!(1, Some(2), sos_offset_set))
        .method("offsetUnset", nm!(1, Some(1), sos_offset_unset))
        .method("getHash", nm!(1, Some(1), sos_get_hash))
        .method("__serialize", nm!(0, Some(0), sos_magic_serialize))
        .method("__unserialize", nm!(1, Some(1), sos_magic_unserialize))
        .method("__debugInfo", nm!(0, Some(0), debug_info))
        .finish();
}

#[cfg(test)]
mod tests {
    use rphp_value::{ArrayKey, Value};

    use crate::tests::{arr, interp};

    /// `new ArrayObject($items)`.
    fn array_object(it: &mut rphp_runtime::Interp, items: Value) -> rphp_value::Object {
        let cid = it.class_by_name(b"ArrayObject").unwrap();
        let o = it.new_object(cid).unwrap();
        it.call_method(&o, b"__construct", &[items]).unwrap();
        o
    }

    #[test]
    fn array_object_is_a_countable_array_access_container() {
        let mut it = interp();
        let o = array_object(&mut it, arr(&[Value::Int(1), Value::Int(2)]));
        assert_eq!(it.call_method(&o, b"count", &[]).unwrap(), Value::Int(2));
        assert_eq!(
            it.call_method(&o, b"offsetGet", &[Value::Int(0)]).unwrap(),
            Value::Int(1)
        );
        it.call_method(&o, b"offsetSet", &[Value::Null, Value::Int(3)]).unwrap();
        it.call_method(&o, b"offsetSet", &[Value::string(b"k"), Value::Int(9)]).unwrap();
        assert_eq!(it.call_method(&o, b"count", &[]).unwrap(), Value::Int(4));
        assert_eq!(
            it.call_method(&o, b"offsetExists", &[Value::string(b"k")]).unwrap(),
            Value::Bool(true)
        );
        it.call_method(&o, b"offsetUnset", &[Value::string(b"k")]).unwrap();
        assert_eq!(
            it.call_method(&o, b"offsetExists", &[Value::string(b"k")]).unwrap(),
            Value::Bool(false)
        );
        // The backing array is php's private `storage` slot, so it dumps the
        // way php dumps it.
        let props = o.props_snapshot();
        assert_eq!(props.len(), 1);
        assert_eq!(&*props[0].0, &b"storage"[..]);
        assert_eq!(props[0].2, rphp_value::Vis::Private);
    }

    #[test]
    fn array_object_interfaces_and_iterator() {
        let mut it = interp();
        let ao = it.class_by_name(b"ArrayObject").unwrap();
        for iface in [
            b"IteratorAggregate".as_slice(),
            b"Traversable".as_slice(),
            b"ArrayAccess".as_slice(),
            b"Countable".as_slice(),
            b"Serializable".as_slice(),
        ] {
            let i = it.class_by_name(iface).unwrap();
            assert!(it.instanceof_class(ao, i), "ArrayObject implements {}", String::from_utf8_lossy(iface));
        }
        let ai = it.class_by_name(b"ArrayIterator").unwrap();
        for iface in [
            b"Iterator".as_slice(),
            b"SeekableIterator".as_slice(),
            b"Traversable".as_slice(),
            b"ArrayAccess".as_slice(),
            b"Countable".as_slice(),
        ] {
            let i = it.class_by_name(iface).unwrap();
            assert!(it.instanceof_class(ai, i), "ArrayIterator implements {}", String::from_utf8_lossy(iface));
        }
        let o = array_object(&mut it, arr(&[Value::Int(7), Value::Int(8)]));
        let iter = it.call_method(&o, b"getIterator", &[]).unwrap();
        let Value::Object(iter) = iter else { panic!("getIterator returns an object") };
        assert_eq!(it.class_name_of(&iter), "ArrayIterator");
        it.call_method(&iter, b"rewind", &[]).unwrap();
        assert_eq!(it.call_method(&iter, b"valid", &[]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_method(&iter, b"key", &[]).unwrap(), Value::Int(0));
        assert_eq!(it.call_method(&iter, b"current", &[]).unwrap(), Value::Int(7));
        it.call_method(&iter, b"next", &[]).unwrap();
        assert_eq!(it.call_method(&iter, b"key", &[]).unwrap(), Value::Int(1));
        it.call_method(&iter, b"next", &[]).unwrap();
        assert_eq!(it.call_method(&iter, b"valid", &[]).unwrap(), Value::Bool(false));
        assert_eq!(it.call_method(&iter, b"key", &[]).unwrap(), Value::Null);
        it.call_method(&iter, b"seek", &[Value::Int(1)]).unwrap();
        assert_eq!(it.call_method(&iter, b"current", &[]).unwrap(), Value::Int(8));
        let err = it.call_method(&iter, b"seek", &[Value::Int(9)]).unwrap_err();
        assert_eq!(err.message(), Some("Seek position 9 is out of range"));
        assert_eq!(err.class_name().as_deref(), Some("OutOfBoundsException"));
    }

    #[test]
    fn sorting_runs_through_the_array_builtins() {
        let mut it = interp();
        let o = array_object(&mut it, arr(&[Value::Int(3), Value::Int(1), Value::Int(2)]));
        it.call_method(&o, b"asort", &[]).unwrap();
        let copy = it.call_method(&o, b"getArrayCopy", &[]).unwrap();
        let Value::Array(copy) = copy else { panic!("getArrayCopy returns an array") };
        let order: Vec<Value> = copy.values().map(|v| v.deref().into_owned()).collect();
        assert_eq!(order, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        // `asort` preserves keys.
        assert_eq!(copy.get_deref(&ArrayKey::Int(0)), Some(Value::Int(3)));
        it.call_method(&o, b"ksort", &[]).unwrap();
        let copy = it.call_method(&o, b"getArrayCopy", &[]).unwrap();
        let Value::Array(copy) = copy else { panic!("getArrayCopy returns an array") };
        let keys: Vec<i64> = copy.keys().map(|k| match k { ArrayKey::Int(i) => *i, _ => -1 }).collect();
        assert_eq!(keys, vec![0, 1, 2]);
    }

    #[test]
    fn object_storage_is_a_set_keyed_by_identity() {
        let mut it = interp();
        let std = it.class_by_name(b"stdClass").unwrap();
        let sos = it.class_by_name(b"SplObjectStorage").unwrap();
        let s = it.new_object(sos).unwrap();
        let a = it.instantiate(std);
        let b = it.instantiate(std);
        it.call_method(&s, b"offsetSet", &[Value::Object(a.clone()), Value::string(b"ia")]).unwrap();
        it.call_method(&s, b"offsetSet", &[Value::Object(b.clone()), Value::string(b"ib")]).unwrap();
        // Re-setting an existing object replaces its info, it does not append.
        it.call_method(&s, b"offsetSet", &[Value::Object(a.clone()), Value::string(b"x")]).unwrap();
        assert_eq!(it.call_method(&s, b"count", &[]).unwrap(), Value::Int(2));
        assert_eq!(
            it.call_method(&s, b"offsetGet", &[Value::Object(a.clone())]).unwrap(),
            Value::string(b"x")
        );
        assert_eq!(
            it.call_method(&s, b"offsetExists", &[Value::Object(b.clone())]).unwrap(),
            Value::Bool(true)
        );
        let missing = it.instantiate(std);
        let err = it
            .call_method(&s, b"offsetGet", &[Value::Object(missing)])
            .unwrap_err();
        assert_eq!(err.message(), Some("Object not found"));
        assert_eq!(err.class_name().as_deref(), Some("UnexpectedValueException"));
        let err = it.call_method(&s, b"offsetGet", &[Value::Int(1)]).unwrap_err();
        assert_eq!(
            err.message(),
            Some("SplObjectStorage::offsetGet(): Argument #1 ($object) must be of type object, int given")
        );
        // Iteration walks insertion order with integer keys.
        it.call_method(&s, b"rewind", &[]).unwrap();
        assert_eq!(it.call_method(&s, b"key", &[]).unwrap(), Value::Int(0));
        let cur = it.call_method(&s, b"current", &[]).unwrap();
        assert!(matches!(&cur, Value::Object(o) if o.ptr_eq(&a)));
        it.call_method(&s, b"next", &[]).unwrap();
        assert_eq!(it.call_method(&s, b"getInfo", &[]).unwrap(), Value::string(b"ib"));
        it.call_method(&s, b"next", &[]).unwrap();
        assert_eq!(it.call_method(&s, b"valid", &[]).unwrap(), Value::Bool(false));
        it.call_method(&s, b"offsetUnset", &[Value::Object(a)]).unwrap();
        assert_eq!(it.call_method(&s, b"count", &[]).unwrap(), Value::Int(1));
        // php keeps the set in the same private `storage` slot it dumps.
        let props = s.props_snapshot();
        assert_eq!(props.len(), 1);
        assert_eq!(&*props[0].0, &b"storage"[..]);
    }

    #[test]
    fn construct_rejects_a_scalar_backing_array() {
        let mut it = interp();
        let cid = it.class_by_name(b"ArrayObject").unwrap();
        let o = it.new_object(cid).unwrap();
        let err = it.call_method(&o, b"__construct", &[Value::Int(1)]).unwrap_err();
        assert_eq!(
            err.message(),
            Some("ArrayObject::__construct(): Argument #1 ($array) must be of type array, int given")
        );
    }
}
