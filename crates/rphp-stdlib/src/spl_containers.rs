//! `ArrayIterator` and `ArrayObject` (php-src `ext/spl/spl_array.c`) — the
//! two SPL containers everything else leans on.
//!
//! **Where the state lives.** php keeps the backing array out of the
//! property table — `get_object_vars()`, Reflection and a user subclass's
//! `$this->storage` see nothing — and shows it through `__debugInfo()` as
//! a private-looking entry of the *base* class:
//!
//! ```text
//! object(ArrayObject)#1 (1) {
//!   ["storage":"ArrayObject":private]=>
//!   array(2) { … }
//! }
//! ```
//!
//! So the whole state — the backing array, the `ARRAY_AS_PROPS` flag word,
//! the iterator cursor, `ArrayObject`'s iterator class — lives in the
//! instance's native [`Payload`] ([`ContainerState`]); `__debugInfo()`
//! builds php's table, `(array)`, `var_export()` and `json_encode()` see the
//! backing array itself (php's `get_properties_for`, the `cast` hook),
//! `==` compares it first (the `native_compare` hook) and `&$ao[$k]`
//! reaches into it (the `dim_ref` hook). `SplObjectStorage` shares the
//! layout with its entry list in the same slot.
//!
//! The two classes share almost every method; the implementations below are
//! written once and registered on both.

use rphp_runtime::{nm, Ctx, Interp, NativeProps, NativeResult, Registry, Unwind};
use rphp_value::{array_key, Array, ArrayKey, Object, Payload, PhpRef, Value};

/// Everything about an instance php keeps out of the property table.
struct ContainerState {
    /// The backing array (`ArrayObject`/`ArrayIterator`), or the entry
    /// list (`SplObjectStorage`).
    storage: Array,
    /// The `STD_PROP_LIST` / `ARRAY_AS_PROPS` word.
    flags: i64,
    /// The iterator cursor as a *raw* position into the backing array
    /// (`Array::raw_entry` / `Array::next_live_from`), so an `unset` during
    /// iteration does not shift the cursor — php's `HashPosition`.
    pos: usize,
    /// `ArrayObject::setIteratorClass`.
    iterator_class: Vec<u8>,
    /// `SplObjectStorage`: object handle → position in the `storage` list,
    /// rebuilt from the list when absent or stale (a rebuild of the list
    /// drops it).
    sos_index: Option<hashbrown::HashMap<u32, usize>>,
}

impl Default for ContainerState {
    fn default() -> ContainerState {
        ContainerState {
            storage: Array::new(),
            flags: 0,
            pos: 0,
            iterator_class: b"ArrayIterator".to_vec(),
            sos_index: None,
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

/// The backing array (shared: a read).
fn storage(o: &Object) -> Array {
    with_state(o, |s| s.storage.clone())
}

/// Replace the backing array.
fn set_storage(o: &Object, a: Array) {
    with_state(o, |s| s.storage = a);
}

/// The backing array **moved out** of the state (an empty one is left
/// behind) for an in-place change that [`set_storage`] then puts back: a
/// clone would leave two handles and make every `$ao[$k] = $v` copy the
/// whole array.
fn take_storage(o: &Object) -> Array {
    with_state(o, |s| std::mem::take(&mut s.storage))
}

/// `clone` copies the whole state along with the ordinary properties.
fn container_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let (storage, flags, pos, iterator_class) =
        with_state(src, |s| (s.storage.clone(), s.flags, s.pos, s.iterator_class.clone()));
    with_state(dst, |s| {
        s.storage = storage;
        s.flags = flags;
        s.pos = pos;
        s.iterator_class = iterator_class;
        s.sos_index = None;
    });
    Ok(())
}

/// The `dim_ref` hook: the cell behind `$ao[$k]`, autovivified — what php
/// hands out for `&$ao['k']`.
fn dim_ref(o: &Object, k: &ArrayKey) -> Option<PhpRef> {
    Some(with_state(o, |s| s.storage.get_ref(k.clone())))
}

/// The `native_compare` hook (php's `spl_array_compare_objects`): the
/// backing arrays first; equal ones leave the verdict to the properties.
fn array_compare(a: &Object, b: &Object) -> Option<i64> {
    let r = Value::Array(storage(a)).spaceship(&Value::Array(storage(b)));
    (r != 0).then_some(r)
}

/// The `cast` hook (php's `get_properties_for` on `ARRAY_CAST`,
/// `VAR_EXPORT` and `JSON`): the backing array — unless `STD_PROP_LIST`
/// asks for the standard properties.
fn cast_table(it: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    if with_state(o, |s| s.flags) & STD_PROP_LIST != 0 {
        return it.std_property_table(o).iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    }
    storage(o).iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// A property hook that claims nothing: the containers have no computed
/// properties, only the whole-table hooks.
fn no_prop_get(_: &mut Interp, _: &Object, _: &[u8]) -> Option<Result<Value, Unwind>> {
    None
}

fn no_prop_set(_: &mut Interp, _: &Object, _: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    None
}

/// The hooks `ArrayObject`/`ArrayIterator` register.
const ARRAY_PROPS: NativeProps = NativeProps {
    names: &[],
    get: no_prop_get,
    set: no_prop_set,
    isset: None,
    unset: None,
    list: None,
    debug: None,
    cast: Some(cast_table),
};

/// `ArrayObject::STD_PROP_LIST`.
const STD_PROP_LIST: i64 = 1;

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
            if let Some(a) = src.with_payload::<ContainerState, _>(|s| s.storage.clone()) {
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
    let mut a = take_storage(o);
    match &*args[0].deref() {
        Value::Null | Value::Uninit => a.push(value),
        k => {
            let k = offset(k)?;
            // php's `zend_hash_update`: the slot is replaced, so a reference
            // taken on it earlier (`&$ao['k']`) is left behind, unlike a
            // plain array's write-through.
            a.set_slot(k, value);
        }
    }
    set_storage(o, a);
    Ok(Value::Null)
}

/// `offsetUnset(mixed $key): void` — removing an absent key is a no-op.
fn offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let k = offset(&args[0])?;
    let mut a = take_storage(o);
    a.unset(&k);
    set_storage(o, a);
    Ok(Value::Null)
}

/// `append(mixed $value): void`
fn append(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let v = args[0].deref().into_owned();
    let mut a = take_storage(o);
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

/// php's `zend_unmangle_property_name`: the plain name behind a
/// private/protected `"\0Class\0name"` key.
fn unmangled_name(key: &[u8]) -> &[u8] {
    if key.len() < 3 || key[0] != 0 || key[1] == 0 {
        return key;
    }
    match key[1..].iter().position(|&b| b == 0) {
        Some(end) => &key[end + 2..],
        None => key,
    }
}

/// php's mangled private-property key, `"\0Class\0prop"`.
fn mangled(class: &[u8], prop: &[u8]) -> ArrayKey {
    let mut name = vec![0u8];
    name.extend_from_slice(class);
    name.push(0);
    name.extend_from_slice(prop);
    ArrayKey::Str(name.into())
}

/// The class php attributes a container's `storage` entry to: the base
/// class, whatever the receiver's (`"storage":"ArrayObject":private` on a
/// subclass too).
fn storage_owner(ctx: &Ctx, o: &Object) -> &'static [u8] {
    for base in [&b"ArrayIterator"[..], b"SplObjectStorage"] {
        if ctx.class_by_name(base).is_some_and(|cid| ctx.object_instanceof(o, cid)) {
            return base;
        }
    }
    b"ArrayObject"
}

/// `__debugInfo(): array` — the standard properties, then the backing
/// array under the base class's mangled private name
/// (`"\0ArrayObject\0storage"`).
fn debug_info(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let owner = storage_owner(ctx, o);
    let mut out = ctx.std_property_table(o);
    out.set(mangled(owner, b"storage"), Value::Array(storage(o)));
    Ok(Value::Array(out))
}

/// `__serialize(): array` — php's `[flags, storage, properties, iterator
/// class]` quadruple, the properties under their mangled keys.
fn magic_serialize(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (flags, iter_class) = with_state(o, |s| (s.flags, s.iterator_class.clone()));
    let mut out = Array::new();
    out.push(Value::Int(flags));
    out.push(Value::Array(storage(o)));
    out.push(Value::Array(ctx.std_property_table(o)));
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
    if let Some(Value::Array(props)) = data.get_deref(&ArrayKey::Int(2)) {
        for (k, v) in props.iter() {
            if let ArrayKey::Str(name) = k {
                o.set(unmangled_name(name), v.deref().into_owned());
            }
        }
    }
    if let Some(Value::Str(c)) = data.get_deref(&ArrayKey::Int(3)) {
        let name = c.as_bytes().to_vec();
        with_state(o, |s| s.iterator_class = name);
    }
    Ok(Value::Null)
}


// ---- SplObjectStorage --------------------------------------------------
//
// php shows the set as a list of `["obj" => $object, "inf" => $info]`
// pairs under a private-looking `storage` entry of `__debugInfo()`, and
// that is the representation kept in [`ContainerState::storage`] here.
// Lookup is by object identity over that list through a handle index; the
// iterator cursor is the list index in [`ContainerState::pos`].

/// An entry's key: an object — a closure included, which is an object to
/// php though a value of its own here — compared by handle.
#[derive(Clone)]
struct SosKey(Value);

impl SosKey {
    fn of(v: &Value) -> Option<SosKey> {
        match v {
            Value::Object(_) | Value::Closure(_) => Some(SosKey(v.clone())),
            _ => None,
        }
    }

    /// The object handle (`spl_object_id`).
    fn id(&self) -> u32 {
        match &self.0 {
            Value::Object(o) => o.id(),
            Value::Closure(c) => c.id(),
            _ => 0,
        }
    }

    fn same(&self, other: &SosKey) -> bool {
        self.id() == other.id()
    }

    fn value(&self) -> Value {
        self.0.clone()
    }
}

/// The entry list as `(object, info)` pairs.
fn sos_load(o: &Object) -> Vec<(SosKey, Value)> {
    let entries = storage(o);
    entries
        .values()
        .filter_map(|v| match &*v.deref() {
            Value::Array(e) => {
                let obj = SosKey::of(&e.get_deref(&ArrayKey::str(b"obj"))?)?;
                let inf = e.get_deref(&ArrayKey::str(b"inf")).unwrap_or(Value::Null);
                Some((obj, inf))
            }
            _ => None,
        })
        .collect()
}

/// Write the entry list back in php's shape.
fn sos_store(o: &Object, items: &[(SosKey, Value)]) {
    let mut a = Array::new();
    for (obj, inf) in items {
        let mut e = Array::new();
        e.set(ArrayKey::str(b"obj"), obj.value());
        e.set(ArrayKey::str(b"inf"), inf.clone());
        a.push(Value::Array(e));
    }
    set_storage(o, a);
    with_state(o, |s| s.sos_index = None);
}

/// The position of the entry for object handle `id` in the `storage`
/// list, through the index (rebuilt when it does not match the list).
fn sos_position(o: &Object, list: &Array, id: u32) -> Option<usize> {
    with_state(o, |s| {
        let stale = s.sos_index.as_ref().is_none_or(|ix| ix.len() != list.len());
        if stale {
            let mut ix = hashbrown::HashMap::with_capacity(list.len());
            for (k, v) in list.iter() {
                if let (ArrayKey::Int(i), Value::Array(e)) = (k, &*v.deref()) {
                    if let Some(key) = e.get_deref(&ArrayKey::str(b"obj")).as_ref().and_then(SosKey::of) {
                        ix.insert(key.id(), *i as usize);
                    }
                }
            }
            s.sos_index = Some(ix);
        }
        s.sos_index.as_ref().and_then(|ix| ix.get(&id).copied())
    })
}

/// The `inf` of the entry at list position `i`.
fn sos_info_at(list: &Array, i: usize) -> Option<Value> {
    match list.get_deref(&ArrayKey::Int(i as i64))? {
        Value::Array(e) => Some(e.get_deref(&ArrayKey::str(b"inf")).unwrap_or(Value::Null)),
        _ => None,
    }
}

/// The object of the entry at list position `i`.
fn sos_object_at(list: &Array, i: usize) -> Option<Value> {
    match list.get_deref(&ArrayKey::Int(i as i64))? {
        Value::Array(e) => e.get_deref(&ArrayKey::str(b"obj")),
        _ => None,
    }
}

/// Set the `inf` of the entry at list position `i` in place.
fn sos_set_info_at(o: &Object, i: usize, info: Value) {
    let mut list = take_storage(o);
    if let Some(Value::Array(e)) = list.get_mut(&ArrayKey::Int(i as i64)) {
        e.set(ArrayKey::str(b"inf"), info);
    }
    set_storage(o, list);
}

/// An `$object` argument (every `SplObjectStorage` entry point takes one).
fn sos_object_arg(who: &str, v: &Value) -> Result<SosKey, Unwind> {
    match &*v.deref() {
        v @ (Value::Object(_) | Value::Closure(_)) => Ok(SosKey(v.clone())),
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
    let list = storage(o);
    match sos_position(o, &list, key.id()) {
        Some(i) => {
            drop(list);
            sos_set_info_at(o, i, info);
        }
        None => {
            drop(list);
            let mut list = take_storage(o);
            let at = list.len();
            let mut e = Array::new();
            e.set(ArrayKey::str(b"obj"), key.value());
            e.set(ArrayKey::str(b"inf"), info);
            list.push(Value::Array(e));
            set_storage(o, list);
            with_state(o, |s| {
                if let Some(ix) = s.sos_index.as_mut() {
                    ix.insert(key.id(), at);
                }
            });
        }
    }
    Ok(Value::Null)
}

/// `offsetGet(object $object): mixed` — php throws when the object is absent.
fn sos_offset_get(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetGet", &args[0])?;
    let list = storage(o);
    match sos_position(o, &list, key.id()).and_then(|i| sos_info_at(&list, i)) {
        Some(info) => Ok(info),
        None => Err(Unwind::exception("UnexpectedValueException", "Object not found")),
    }
}

/// `offsetExists(object $object): bool`
fn sos_offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetExists", &args[0])?;
    let list = storage(o);
    Ok(Value::Bool(sos_position(o, &list, key.id()).is_some()))
}

/// `offsetUnset(object $object): void` — the list is rebuilt without the
/// entry (php's storage keeps no holes either).
fn sos_offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = sos_object_arg("offsetUnset", &args[0])?;
    let list = storage(o);
    if let Some(i) = sos_position(o, &list, key.id()) {
        drop(list);
        let mut items = sos_load(o);
        items.remove(i);
        sos_store(o, &items);
    }
    Ok(Value::Null)
}

/// `count(int $mode = COUNT_NORMAL): int`
fn sos_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(storage(o).len() as i64))
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
fn sos_storage_arg(ctx: &Ctx, who: &str, v: &Value) -> Result<Vec<(SosKey, Value)>, Unwind> {
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
        let found = items.iter().position(|(k, _)| k.same(&obj));
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
    items.retain(|(k, _)| !drop_these.iter().any(|(d, _)| d.same(k)));
    sos_store(o, &items);
    Ok(Value::Int(items.len() as i64))
}

/// `removeAllExcept(SplObjectStorage $storage): int`
fn sos_remove_all_except(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let keep = sos_storage_arg(ctx, "removeAllExcept", &args[0])?;
    let mut items = sos_load(o);
    items.retain(|(k, _)| keep.iter().any(|(d, _)| d.same(k)));
    sos_store(o, &items);
    Ok(Value::Int(items.len() as i64))
}

/// `getInfo(): mixed` — the info of the entry under the cursor.
fn sos_get_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    Ok(sos_info_at(&storage(o), pos).unwrap_or(Value::Null))
}

/// `setInfo(mixed $info): void`
fn sos_set_info(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    let info = args[0].deref().into_owned();
    if pos < storage(o).len() {
        sos_set_info_at(o, pos, info);
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
    Ok(Value::Bool(pos < storage(o).len()))
}

fn sos_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |s| s.pos) as i64))
}

fn sos_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = with_state(o, |s| s.pos);
    Ok(sos_object_at(&storage(o), pos).unwrap_or(Value::Null))
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
    let len = storage(o).len() as i64;
    if want < 0 || want >= len {
        return Err(Unwind::exception(
            "OutOfBoundsException",
            format!("Seek position {want} is out of range"),
        ));
    }
    with_state(o, |s| s.pos = want as usize);
    Ok(Value::Null)
}

/// `__serialize(): array` — php's `[[object, info, object, info, …],
/// properties]`: the entries flattened into one list, the properties under
/// their mangled keys.
fn sos_magic_serialize(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut entries = Array::new();
    for (obj, inf) in sos_load(o) {
        entries.push(obj.value());
        entries.push(inf);
    }
    let mut out = Array::new();
    out.push(Value::Array(entries));
    out.push(Value::Array(ctx.std_property_table(o)));
    Ok(Value::Array(out))
}

/// The `native_compare` hook (php's `spl_object_storage_compare_objects`):
/// the entry counts, then each entry of the left set must be in the right
/// one with an equal `inf` — the verdict is final, the properties never
/// weigh in.
fn sos_compare(a: &Object, b: &Object) -> Option<i64> {
    let la = sos_load(a);
    let lb = sos_load(b);
    if la.len() != lb.len() {
        return Some(if la.len() < lb.len() { -1 } else { 1 });
    }
    for (key, inf) in &la {
        let Some((_, other)) = lb.iter().find(|(k, _)| k.same(key)) else {
            return Some(1);
        };
        let r = inf.spaceship(other);
        if r != 0 {
            return Some(r);
        }
    }
    Some(0)
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
    // php's checks, in its order: the two arrays, an even entry count, an
    // object at every even position.
    let bad = |what: &str| Unwind::exception("UnexpectedValueException", what.to_string());
    let (Some(Value::Array(entries)), Some(Value::Array(props))) =
        (data.get_deref(&ArrayKey::Int(0)), data.get_deref(&ArrayKey::Int(1)))
    else {
        return Err(bad("Incomplete or ill-typed serialization data"));
    };
    let flat: Vec<Value> = entries.values().map(|v| v.deref().into_owned()).collect();
    if flat.len() % 2 != 0 {
        return Err(bad("Odd number of elements"));
    }
    let mut items: Vec<(SosKey, Value)> = Vec::with_capacity(flat.len() / 2);
    for pair in flat.chunks(2) {
        let Some(obj) = SosKey::of(&pair[0]) else {
            return Err(bad("Non-object key"));
        };
        items.push((obj, pair[1].clone()));
    }
    sos_store(o, &items);
    for (k, v) in props.iter() {
        if let ArrayKey::Str(name) = k {
            o.set(unmangled_name(name), v.deref().into_owned());
        }
    }
    Ok(Value::Null)
}

// ---- registration ------------------------------------------------------

/// Register `ArrayIterator` and `ArrayObject` (after `spl_interfaces` and
/// `spl_exceptions` — `seek` throws `OutOfBoundsException`).
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ArrayIterator")
        .implements(&["SeekableIterator", "ArrayAccess", "Serializable", "Countable"])
        .class_const("STD_PROP_LIST", Value::Int(STD_PROP_LIST))
        .class_const("ARRAY_AS_PROPS", Value::Int(2))
        .payload_clone(container_clone)
        .native_props(ARRAY_PROPS)
        .native_compare(array_compare)
        .dim_ref(dim_ref)
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
        .class_const("STD_PROP_LIST", Value::Int(STD_PROP_LIST))
        .class_const("ARRAY_AS_PROPS", Value::Int(2))
        .payload_clone(container_clone)
        .native_props(ARRAY_PROPS)
        .native_compare(array_compare)
        .dim_ref(dim_ref)
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
        .payload_clone(container_clone)
        .native_compare(sos_compare)
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
        // The backing array is no property: php keeps it out of the table.
        assert!(o.props_snapshot().is_empty());
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
        // php keeps the set out of the property table.
        assert!(s.props_snapshot().is_empty());
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
