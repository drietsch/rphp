//! Weak object references (php-src `Zend/zend_weakrefs.c`): `WeakReference`,
//! `WeakMap`, and the `InternalIterator` `WeakMap::getIterator()` returns.
//!
//! Both classes hang off [`rphp_value::WeakObject`] — the non-owning handle
//! `Object::downgrade()` produces, which the destructor registry already
//! uses. The handle lives in the instance's native [`Payload`], not in a
//! declared property, because a declared property would be a *strong*
//! reference and defeat the point.
//!
//! php renders both through its `get_properties_for` handler on a dump
//! only (`var_dump($ref)` shows `["object"]=> …`, `var_dump($map)` shows
//! `[0]=> ["key" => …, "value" => …]`; `(array)` and `json_encode()` see
//! nothing), which is the `debug` hook of each class's [`NativeProps`].

use rphp_runtime::{
    nm, ClassFlags, Ctx, Interp, NativeMethod, NativeProps, NativeResult, Registry, Unwind,
    Visibility,
};
use rphp_value::{Array, ArrayKey, Closure, Object, Payload, Value, WeakClosure, WeakObject};

// ---- payloads ----------------------------------------------------------

/// A `WeakReference`'s target.
struct WeakRefState(WeakObject);

/// A `WeakMap` key: an object or a closure (an object to php), held
/// strongly while it is an argument and weakly once stored.
#[derive(Clone)]
enum MapKey {
    Object(Object),
    Closure(Closure),
}

impl MapKey {
    fn downgrade(&self) -> WeakKey {
        match self {
            MapKey::Object(o) => WeakKey::Object(o.downgrade()),
            MapKey::Closure(c) => WeakKey::Closure(c.downgrade()),
        }
    }

    fn value(&self) -> Value {
        match self {
            MapKey::Object(o) => Value::Object(o.clone()),
            MapKey::Closure(c) => Value::Closure(c.clone()),
        }
    }

    fn id(&self) -> u32 {
        match self {
            MapKey::Object(o) => o.id(),
            MapKey::Closure(c) => c.id(),
        }
    }
}

enum WeakKey {
    Object(WeakObject),
    Closure(WeakClosure),
}

impl WeakKey {
    /// The object handle the key was made from.
    fn id(&self) -> u32 {
        match self {
            WeakKey::Object(w) => w.id(),
            WeakKey::Closure(w) => w.id(),
        }
    }

    fn upgrade(&self) -> Option<MapKey> {
        match self {
            WeakKey::Object(w) => w.upgrade().map(MapKey::Object),
            WeakKey::Closure(w) => w.upgrade().map(MapKey::Closure),
        }
    }

    fn is(&self, key: &MapKey) -> bool {
        match (self, key) {
            (WeakKey::Object(w), MapKey::Object(o)) => w.ptr_eq_obj(o),
            (WeakKey::Closure(w), MapKey::Closure(c)) => w.ptr_eq_closure(c),
            _ => false,
        }
    }
}

/// A `WeakMap`'s entries, in insertion order (`None` where one was
/// removed), indexed by the key's object handle — handles are never
/// reused within an interpreter, so a dead entry can never be mistaken
/// for a new object's. The key is weak; the value is held strongly, as
/// php does.
#[derive(Default)]
struct WeakMapState {
    entries: Vec<Option<(WeakKey, Value)>>,
    index: hashbrown::HashMap<u32, usize>,
}

impl WeakMapState {
    /// Drop entries whose key has been collected. php does this eagerly from
    /// the object's free handler; we have no such hook, so the operations
    /// that observe the whole map (`count`, iteration) prune first, and the
    /// others when the dead outnumber the live.
    fn prune(&mut self) {
        let old = std::mem::take(&mut self.entries);
        self.index.clear();
        for e in old.into_iter().flatten() {
            if e.0.upgrade().is_some() {
                self.index.insert(e.0.id(), self.entries.len());
                self.entries.push(Some(e));
            }
        }
    }

    fn maybe_prune(&mut self) {
        if self.entries.len() > 64 && self.entries.len() > 2 * self.index.len() {
            self.prune();
        }
    }

    /// The position of `key`'s entry, if present and alive.
    fn find(&self, key: &MapKey) -> Option<usize> {
        let i = *self.index.get(&key.id())?;
        let (k, _) = self.entries.get(i)?.as_ref()?;
        k.is(key).then_some(i)
    }

    fn value_at(&self, i: usize) -> Option<&Value> {
        self.entries.get(i)?.as_ref().map(|(_, v)| v)
    }

    fn set(&mut self, key: &MapKey, value: Value) {
        match self.find(key) {
            Some(i) => {
                if let Some(e) = self.entries[i].as_mut() {
                    e.1 = value;
                }
            }
            None => {
                self.maybe_prune();
                self.index.insert(key.id(), self.entries.len());
                self.entries.push(Some((key.downgrade(), value)));
            }
        }
    }

    fn remove(&mut self, key: &MapKey) {
        if let Some(i) = self.find(key) {
            self.entries[i] = None;
            self.index.remove(&key.id());
        }
    }

    fn live(&mut self) -> usize {
        self.prune();
        self.entries.len()
    }
}

/// An `InternalIterator`'s snapshot and cursor. php's is a live view onto
/// the producing object; a snapshot is what we can express without an
/// engine-side iterator protocol, and it matches for every use that does not
/// mutate the map during the loop.
struct InternalIterState {
    items: Vec<(Value, Value)>,
    pos: usize,
}

/// Run `f` on the instance's payload, installing `T::default()` first when
/// there is none (an instance built by [`Interp::instantiate`], e.g. by
/// `unserialize`, has no payload).
fn with_state<T: Default + 'static, R>(o: &Object, f: impl FnOnce(&mut T) -> R) -> R {
    let present = o.with_payload::<T, _>(|_| ()).is_some();
    if !present {
        o.set_payload(Payload::Native(Box::new(T::default())));
    }
    o.with_payload::<T, _>(f)
        .expect("payload installed just above")
}

// ---- helpers -----------------------------------------------------------

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// A `WeakMap` key argument: php accepts objects only.
fn key_arg(v: &Value) -> Result<MapKey, Unwind> {
    match &*v.deref() {
        Value::Object(o) => Ok(MapKey::Object(o.clone())),
        Value::Closure(c) => Ok(MapKey::Closure(c.clone())),
        _ => Err(Unwind::type_error("WeakMap key must be an object")),
    }
}

/// php's `Object <Class>#<handle> not contained in WeakMap`.
fn not_contained(ctx: &Ctx, key: &MapKey) -> Unwind {
    let class = match key {
        MapKey::Object(o) => ctx.class_name_of(o),
        MapKey::Closure(_) => "Closure".to_string(),
    };
    Unwind::error(format!(
        "Object {}#{} not contained in WeakMap",
        class,
        key.id()
    ))
}

/// A `static` native method row.
fn static_method(min: u8, max: Option<u8>, params: &'static [&'static str], f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
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

// ---- WeakReference -----------------------------------------------------

/// `new WeakReference` is refused; `WeakReference::create()` builds
/// instances through [`Interp::instantiate`], which skips this hook.
fn weakref_not_instantiable(_: &mut Interp, _: &Object) -> Result<(), Unwind> {
    Err(Unwind::error(
        "Direct instantiation of WeakReference is not allowed, use WeakReference::create instead",
    ))
}

/// `WeakReference::__construct()` — php refuses in the create handler; this
/// is the same message for a direct call.
fn weakref_construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "Direct instantiation of WeakReference is not allowed, use WeakReference::create instead",
    ))
}

/// `WeakReference::create(object $object): WeakReference`
fn weakref_create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let target = match args[0].deref().into_owned() {
        Value::Object(t) => t,
        other => {
            return Err(Unwind::type_error(format!(
                "WeakReference::create(): Argument #1 ($object) must be of type object, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let cid = ctx
        .class_by_name(b"WeakReference")
        .expect("WeakReference is registered");
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(WeakRefState(target.downgrade()))));
    Ok(Value::Object(obj))
}

/// `WeakReference::get(): ?object`
fn weakref_get(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let alive = o
        .with_payload::<WeakRefState, _>(|s| s.0.upgrade())
        .flatten();
    Ok(match alive {
        Some(t) => Value::Object(t),
        None => Value::Null,
    })
}

/// The `debug` hook: `["object" => the target or null]`.
fn weakref_debug_table(_: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    let alive = o
        .with_payload::<WeakRefState, _>(|s| s.0.upgrade())
        .flatten();
    vec![(
        ArrayKey::str(b"object"),
        alive.map_or(Value::Null, Value::Object),
    )]
}

fn no_prop_get(_: &mut Interp, _: &Object, _: &[u8]) -> Option<Result<Value, Unwind>> {
    None
}

fn no_prop_set(_: &mut Interp, _: &Object, _: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    None
}

const WEAKREF_PROPS: NativeProps = NativeProps {
    names: &[],
    get: no_prop_get,
    set: no_prop_set,
    isset: None,
    unset: None,
    list: None,
    debug: Some(weakref_debug_table),
    cast: None,
};

// ---- WeakMap -----------------------------------------------------------

/// The `debug` hook: the live entries as `["key" => …, "value" => …]`
/// pairs at integer keys.
fn weakmap_debug_table(_: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    let items = with_state::<WeakMapState, _>(o, |s| {
        s.prune();
        let out: Vec<(Value, Value)> = s
            .entries
            .iter()
            .flatten()
            .filter_map(|(k, v)| k.upgrade().map(|k| (k.value(), v.clone())))
            .collect();
        out
    });
    items
        .into_iter()
        .enumerate()
        .map(|(i, (k, v))| {
            let mut pair = Array::new();
            pair.set(ArrayKey::str(b"key"), k);
            pair.set(ArrayKey::str(b"value"), v);
            (ArrayKey::Int(i as i64), Value::Array(pair))
        })
        .collect()
}

const WEAKMAP_PROPS: NativeProps = NativeProps {
    names: &[],
    get: no_prop_get,
    set: no_prop_set,
    isset: None,
    unset: None,
    list: None,
    debug: Some(weakmap_debug_table),
    cast: None,
};

/// `WeakMap::offsetGet(object $object): mixed`
fn weakmap_offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = key_arg(&args[0])?;
    let found = with_state::<WeakMapState, _>(o, |s| s.find(&key).and_then(|i| s.value_at(i).cloned()));
    found.ok_or_else(|| not_contained(ctx, &key))
}

/// `WeakMap::offsetSet(object $object, mixed $value): void`
fn weakmap_offset_set(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = key_arg(&args[0])?;
    let value = args[1].deref().into_owned();
    with_state::<WeakMapState, _>(o, |s| s.set(&key, value));
    Ok(Value::Null)
}

/// `WeakMap::offsetExists(object $object): bool`
fn weakmap_offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = key_arg(&args[0])?;
    let found = with_state::<WeakMapState, _>(o, |s| s.find(&key).is_some());
    Ok(Value::Bool(found))
}

/// `WeakMap::offsetUnset(object $object): void` — removing a key that is not
/// there is a no-op in php.
fn weakmap_offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let key = key_arg(&args[0])?;
    with_state::<WeakMapState, _>(o, |s| s.remove(&key));
    Ok(Value::Null)
}

/// `WeakMap::count(): int`
fn weakmap_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let n = with_state::<WeakMapState, _>(o, |s| s.live());
    Ok(Value::Int(n as i64))
}

/// `WeakMap::getIterator(): Iterator` — an `InternalIterator` over the live
/// entries, keyed by the object.
fn weakmap_get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let items = with_state::<WeakMapState, _>(o, |s| {
        s.prune();
        let out: Vec<(Value, Value)> = s
            .entries
            .iter()
            .flatten()
            .filter_map(|(k, v)| k.upgrade().map(|k| (k.value(), v.clone())))
            .collect();
        out
    });
    Ok(Value::Object(new_internal_iterator(ctx, items)))
}

// ---- InternalIterator --------------------------------------------------

/// Build an `InternalIterator` over `items` from native code.
/// [`Interp::instantiate`] is the right door: the class's `__construct` is
/// private, exactly as in php.
pub fn new_internal_iterator(ctx: &mut Ctx, items: Vec<(Value, Value)>) -> Object {
    let cid = ctx
        .class_by_name(b"InternalIterator")
        .expect("InternalIterator is registered");
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(InternalIterState { items, pos: 0 })));
    obj
}

/// `InternalIterator::__construct()` — private in php.
fn internal_iterator_construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "Cannot instantiate internal class InternalIterator",
    ))
}

/// Read `(key, value)` under the cursor.
fn internal_iterator_at(o: &Object) -> Option<(Value, Value)> {
    o.with_payload::<InternalIterState, _>(|s| s.items.get(s.pos).cloned())
        .flatten()
}

fn internal_iterator_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(internal_iterator_at(o).map_or(Value::Null, |(_, v)| v))
}

fn internal_iterator_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(internal_iterator_at(o).map_or(Value::Null, |(k, _)| k))
}

fn internal_iterator_next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let _ = o.with_payload::<InternalIterState, _>(|s| s.pos += 1);
    Ok(Value::Null)
}

fn internal_iterator_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let valid = o
        .with_payload::<InternalIterState, _>(|s| s.pos < s.items.len())
        .unwrap_or(false);
    Ok(Value::Bool(valid))
}

fn internal_iterator_rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let _ = o.with_payload::<InternalIterState, _>(|s| s.pos = 0);
    Ok(Value::Null)
}

// ---- registration ------------------------------------------------------

/// Register `WeakReference`, `WeakMap` and `InternalIterator` (after
/// `spl_interfaces`, whose interfaces they implement).
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("WeakReference")
        .flags(ClassFlags::FINAL)
        .native_props(WEAKREF_PROPS)
        .method("__construct", nm!(0, Some(0), weakref_construct))
        .method("create", static_method(1, Some(1), &["object"], weakref_create))
        .method("get", nm!(0, Some(0), weakref_get))
        .native_init(weakref_not_instantiable)
        .finish();

    r.class("WeakMap")
        .flags(ClassFlags::FINAL)
        .implements(&["ArrayAccess", "Countable", "IteratorAggregate"])
        .native_props(WEAKMAP_PROPS)
        .method("offsetGet", nm!(1, Some(1), weakmap_offset_get))
        .method("offsetSet", nm!(2, Some(2), weakmap_offset_set))
        .method("offsetExists", nm!(1, Some(1), weakmap_offset_exists))
        .method("offsetUnset", nm!(1, Some(1), weakmap_offset_unset))
        .method("count", nm!(0, Some(0), weakmap_count))
        .method("getIterator", nm!(0, Some(0), weakmap_get_iterator))
        .finish();

    r.class("InternalIterator")
        .flags(ClassFlags::FINAL)
        .implements(&["Iterator"])
        .method_vis("__construct", Visibility::Private, nm!(0, Some(0), internal_iterator_construct))
        .method("current", nm!(0, Some(0), internal_iterator_current))
        .method("key", nm!(0, Some(0), internal_iterator_key))
        .method("next", nm!(0, Some(0), internal_iterator_next))
        .method("valid", nm!(0, Some(0), internal_iterator_valid))
        .method("rewind", nm!(0, Some(0), internal_iterator_rewind))
        .finish();
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::interp;

    #[test]
    fn weak_reference_follows_then_clears() {
        let mut it = interp();
        let std = it.class_by_name(b"stdClass").unwrap();
        let wr = it.class_by_name(b"WeakReference").unwrap();
        let target = it.instantiate(std);
        let r = it
            .call_static_method(wr, b"create", &[Value::Object(target.clone())])
            .unwrap();
        let Value::Object(r) = r else { panic!("create returns an object") };
        let got = it.call_method(&r, b"get", &[]).unwrap();
        assert!(matches!(&got, Value::Object(o) if o.ptr_eq(&target)));
        drop(got);
        drop(target);
        assert_eq!(it.call_method(&r, b"get", &[]).unwrap(), Value::Null);
        // `new WeakReference` is php's Error, not "cannot instantiate".
        let err = it.new_object(wr).unwrap_err();
        assert_eq!(
            err.message(),
            Some("Direct instantiation of WeakReference is not allowed, use WeakReference::create instead")
        );
    }

    #[test]
    fn weak_map_stores_by_object_identity() {
        let mut it = interp();
        let std = it.class_by_name(b"stdClass").unwrap();
        let wm = it.class_by_name(b"WeakMap").unwrap();
        let map = it.new_object(wm).unwrap();
        let k1 = it.instantiate(std);
        let k2 = it.instantiate(std);
        it.call_method(&map, b"offsetSet", &[Value::Object(k1.clone()), Value::Int(1)])
            .unwrap();
        it.call_method(&map, b"offsetSet", &[Value::Object(k2.clone()), Value::Int(2)])
            .unwrap();
        // Re-assigning an existing key replaces rather than appends.
        it.call_method(&map, b"offsetSet", &[Value::Object(k1.clone()), Value::Int(9)])
            .unwrap();
        assert_eq!(it.call_method(&map, b"count", &[]).unwrap(), Value::Int(2));
        assert_eq!(
            it.call_method(&map, b"offsetGet", &[Value::Object(k1.clone())]).unwrap(),
            Value::Int(9)
        );
        let err = it
            .call_method(&map, b"offsetGet", &[Value::Int(1)])
            .unwrap_err();
        assert_eq!(err.message(), Some("WeakMap key must be an object"));
        let missing = it.instantiate(std);
        let err = it
            .call_method(&map, b"offsetGet", &[Value::Object(missing.clone())])
            .unwrap_err();
        assert_eq!(
            err.message(),
            Some(&format!("Object stdClass#{} not contained in WeakMap", missing.id())[..])
        );
        // A collected key drops its entry.
        drop(k2);
        assert_eq!(it.call_method(&map, b"count", &[]).unwrap(), Value::Int(1));
    }
}
