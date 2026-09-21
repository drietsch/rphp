//! SPL's iterator decorators (php-src `ext/spl/spl_iterators.c`): the
//! `IteratorIterator` family, `RecursiveIteratorIterator`,
//! `RecursiveArrayIterator` and `MultipleIterator`.
//!
//! **The dual-iterator model.** Every class in the `IteratorIterator` tree is
//! php's `spl_dual_it_object`: an *inner* iterator plus a one-element cache of
//! the element under it — `current.data`, `current.key` and a position counter
//! `current.pos`. That cache is the whole reason the call patterns look the
//! way they do:
//!
//! * `rewind()` rewinds the inner one and then *fetches* (`valid()`,
//!   `current()`, `key()` on the inner), so the element is already in hand;
//! * `valid()`, `current()` and `key()` then answer from the cache and never
//!   touch the inner iterator at all;
//! * `next()` moves the inner one and fetches again.
//!
//! php keeps none of this in properties — `var_dump(new IteratorIterator(…))`
//! prints no members — so it all lives in the instance's native [`Payload`]
//! ([`Dual`]), and every class registers
//! [`ClassBuilder::payload_clone`](rphp_runtime::ClassBuilder::payload_clone).
//! The one real property in the whole module is `RegexIterator::$replacement`,
//! which php does dump.
//!
//! **Re-entry.** A decorator's inner iterator is usually a *user* object, so
//! nearly every method here re-enters the VM through
//! [`Interp::call_method`](rphp_runtime::Interp::call_method). The hooks a
//! subclass may override — `FilterIterator::accept`,
//! `RecursiveIteratorIterator::callHasChildren`/`beginChildren`/… — are
//! likewise dispatched on `$this` rather than called directly, which is what
//! makes overriding them work.
//!
//! **Two subtleties worth stating, because they are load-bearing:**
//!
//! * `CachingIterator` reads one element *ahead*. Its `rewind`/`next` fetch
//!   the element and then immediately move the inner iterator on, so
//!   `hasNext()` is a live `valid()` on an inner that already sits on the
//!   *following* element. `RecursiveCachingIterator` therefore also has to ask
//!   `hasChildren()`/`getChildren()` at fetch time and cache the answer.
//! * `NoRewindIterator` is the one decorator whose `rewind()` does nothing,
//!   which exposes an engine detail of php: zend's user-iterator wrapper
//!   caches the value returned by `current()` until `next()`/`rewind()`
//!   invalidates it. Because `NoRewindIterator::rewind()` never invalidates
//!   anything, a second `foreach` over one calls the inner `key()` again but
//!   *not* `current()`. That cache is modelled here ([`Dual::peek`]) and, as
//!   in php, only for an inner whose class is user-defined — an internal
//!   iterator has its own handlers and is read live.
//!
//! **Known divergences** (ADR-004):
//!
//! * `RegexIterator::__construct` does not pre-compile the pattern, so an
//!   invalid one surfaces as `preg_match()`'s warning from `accept()` instead
//!   of php's `InvalidArgumentException` naming the compile error. The
//!   compile-error text lives inside `pcre.rs`, which this module may not
//!   reach into.
//! * `IteratorIterator` over an `IteratorAggregate` whose `getIterator()`
//!   returns *another* aggregate: php's `getInnerIterator()` answers the first
//!   result while driving the fully resolved one. That is reproduced here
//!   ([`Dual::inner`] versus [`Dual::driver`]), but the general case of a
//!   `zend_object_iterator` handed around as a value has no counterpart.
//! * `RecursiveArrayIterator::hasChildren`/`getChildren` read the element
//!   through `$this->current()`; php reads its own cursor directly, so a
//!   subclass that overrides `current()` is visible here and not in php. The
//!   `ArrayIterator` cursor lives in `spl_containers.rs`'s private payload,
//!   which this module cannot open.

use rphp_runtime::{
    nm, ClassFlags, Ctx, Interp, IterRole, NativeFn, NativeIter, NativeResult, Registry, Unwind,
    Visibility,
};
use rphp_value::{array_key, Array, ArrayKey, Object, Payload, PhpRef, Value};

/// Functions this module provides: none. Every SPL *function*
/// (`iterator_to_array`, `iterator_count`, `iterator_apply`) lives in
/// `spl_iterators.rs`; this module is classes only.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides. Every SPL constant here is a *class*
/// constant (`CachingIterator::FULL_CACHE`,
/// `RecursiveIteratorIterator::SELF_FIRST`, `RegexIterator::ALL_MATCHES`),
/// declared with its class, so there is nothing global to add.
pub(crate) fn register_constants(_r: &mut Registry) {}

// ========================================================================
// class constants
// ========================================================================

/// `CachingIterator::CALL_TOSTRING`
const CIT_CALL_TOSTRING: i64 = 1;
/// `CachingIterator::TOSTRING_USE_KEY`
const CIT_TOSTRING_USE_KEY: i64 = 2;
/// `CachingIterator::TOSTRING_USE_CURRENT`
const CIT_TOSTRING_USE_CURRENT: i64 = 4;
/// `CachingIterator::TOSTRING_USE_INNER`
const CIT_TOSTRING_USE_INNER: i64 = 8;
/// `CachingIterator::CATCH_GET_CHILD`
const CIT_CATCH_GET_CHILD: i64 = 16;
/// `CachingIterator::FULL_CACHE`
const CIT_FULL_CACHE: i64 = 256;
/// The four mutually exclusive string-source bits php validates together.
const CIT_TOSTRING_MASK: i64 =
    CIT_CALL_TOSTRING | CIT_TOSTRING_USE_KEY | CIT_TOSTRING_USE_CURRENT | CIT_TOSTRING_USE_INNER;

/// `RegexIterator::USE_KEY`
const REGIT_USE_KEY: i64 = 1;
/// `RegexIterator::INVERT_MATCH`
const REGIT_INVERT_MATCH: i64 = 2;
/// `RegexIterator::MATCH`
const REGIT_MODE_MATCH: i64 = 0;
/// `RegexIterator::GET_MATCH`
const REGIT_MODE_GET_MATCH: i64 = 1;
/// `RegexIterator::ALL_MATCHES`
const REGIT_MODE_ALL_MATCHES: i64 = 2;
/// `RegexIterator::SPLIT`
const REGIT_MODE_SPLIT: i64 = 3;
/// `RegexIterator::REPLACE`
const REGIT_MODE_REPLACE: i64 = 4;

/// `RecursiveIteratorIterator::LEAVES_ONLY`
const RIT_LEAVES_ONLY: i64 = 0;
/// `RecursiveIteratorIterator::SELF_FIRST`
const RIT_SELF_FIRST: i64 = 1;
/// `RecursiveIteratorIterator::CHILD_FIRST`
const RIT_CHILD_FIRST: i64 = 2;
/// `RecursiveIteratorIterator::CATCH_GET_CHILD`
const RIT_CATCH_GET_CHILD: i64 = 16;

/// `MultipleIterator::MIT_NEED_ANY`
const MIT_NEED_ANY: i64 = 0;
/// `MultipleIterator::MIT_NEED_ALL`
const MIT_NEED_ALL: i64 = 1;
/// `MultipleIterator::MIT_KEYS_NUMERIC`
const MIT_KEYS_NUMERIC: i64 = 0;
/// `MultipleIterator::MIT_KEYS_ASSOC`
const MIT_KEYS_ASSOC: i64 = 2;

/// `RecursiveArrayIterator::CHILD_ARRAYS_ONLY`
const RAIT_CHILD_ARRAYS_ONLY: i64 = 4;

/// `PREG_PATTERN_ORDER` — php's default for `preg_match_all`, which
/// `RegexIterator::ALL_MATCHES` inherits when `$pregFlags` names no order.
const PREG_PATTERN_ORDER: i64 = 1;
/// `PREG_SET_ORDER`
const PREG_SET_ORDER: i64 = 2;

// ========================================================================
// shared helpers
// ========================================================================

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// Run `f` on the instance's hidden state, installing `T::default()` first
/// when there is none (an instance built by
/// [`Interp::instantiate`](rphp_runtime::Interp::instantiate) — `unserialize`,
/// a cast — has no payload).
///
/// The payload is borrowed for the whole call, so `f` must never re-enter the
/// VM: read what is needed out of the state, call, then write back.
fn with_state<T: Default + 'static, R>(o: &Object, f: impl FnOnce(&mut T) -> R) -> R {
    let present = o.with_payload::<T, _>(|_| ()).is_some();
    if !present {
        o.set_payload(Payload::Native(Box::new(T::default())));
    }
    o.with_payload::<T, _>(f)
        .expect("payload installed just above")
}

/// Whether `o` is an instance of the named class or interface.
fn instance_of(ctx: &Ctx, o: &Object, class: &[u8]) -> bool {
    ctx.class_by_name(class)
        .is_some_and(|c| ctx.object_instanceof(o, c))
}

/// php's mangled private-property key, `"\0Class\0prop"`.
fn mangled(class: &[u8], prop: &[u8]) -> ArrayKey {
    let mut name = vec![0u8];
    name.extend_from_slice(class);
    name.push(0);
    name.extend_from_slice(prop);
    ArrayKey::Str(name.into())
}

/// php's weak `int` parameter coercion with php's diagnostics: a numeric
/// string or a whole float converts, a fractional float is deprecated and
/// truncated, `null` is the 8.1 null-to-non-nullable deprecation, and anything
/// else is the `TypeError` `zend_parse_parameters` raises.
fn int_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<i64, Unwind> {
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
                "{who}(): Passing null to parameter #{pos} (${name}) of type int is deprecated"
            ))?;
            Ok(0)
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(s.to_int()),
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type int, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// php's `string` parameter coercion for the one string parameter in this
/// module (`RegexIterator`'s `$pattern`).
fn str_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<Vec<u8>, Unwind> {
    match &*v.deref() {
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{who}(): Passing null to parameter #{pos} (${name}) of type string is deprecated"
            ))?;
            Ok(Vec::new())
        }
        Value::Array(_) => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type string, array given"
        ))),
        v @ (Value::Object(_) | Value::Closure(_)) => {
            let v = v.clone();
            match ctx.to_string(&v) {
                Ok(s) => Ok(s.as_bytes().to_vec()),
                Err(_) => Err(Unwind::type_error(format!(
                    "{who}(): Argument #{pos} (${name}) must be of type string, {} given",
                    rphp_runtime::value_name(&v)
                ))),
            }
        }
        other => Ok(other.to_php_bytes()),
    }
}

/// An `object` parameter that must be an instance of `class`, with php's
/// `zend_parse_parameters` wording.
fn object_arg(
    ctx: &Ctx,
    who: &str,
    pos: u32,
    name: &str,
    class: &str,
    v: &Value,
) -> Result<Object, Unwind> {
    let given = v.deref().into_owned();
    if let Value::Object(o) = &given {
        if instance_of(ctx, o, class.as_bytes()) {
            return Ok(o.clone());
        }
    }
    Err(Unwind::type_error(format!(
        "{who}(): Argument #{pos} (${name}) must be of type {class}, {} given",
        rphp_runtime::value_name(&given)
    )))
}

/// php's `TypeError` for an argument that is not a valid callback.
fn callback_error(who: &str, pos: u32, name: &str, v: &Value) -> Unwind {
    let what = match &*v.deref() {
        Value::Str(s) => format!(
            "function \"{}\" not found or invalid function name",
            s.to_string_lossy()
        ),
        Value::Array(_) => "array callback must have exactly two members".to_string(),
        _ => "no array or string given".to_string(),
    };
    Unwind::type_error(format!(
        "{who}(): Argument #{pos} (${name}) must be a valid callback, {what}"
    ))
}

// ========================================================================
// the dual-iterator core (php's spl_dual_it_object)
// ========================================================================

/// Everything an `IteratorIterator`-family instance keeps out of sight. php
/// puts the per-subclass fields in a union; one struct is simpler and lets the
/// whole family — `RecursiveCachingIterator` inherits methods from three
/// levels — share one payload type, which the runtime requires.
#[derive(Clone)]
struct Dual {
    /// What `getInnerIterator()` answers: php's `inner.zobject`.
    inner: Option<Object>,
    /// What the five `Iterator` methods are actually sent to: php's
    /// `inner.iterator`. It differs from `inner` only when an
    /// `IteratorAggregate` handed back another aggregate.
    driver: Option<Object>,
    /// php's `current.data`; `None` is php's `IS_UNDEF`, which is what
    /// `valid()` reports on.
    data: Option<Value>,
    /// php's `current.key`.
    key: Option<Value>,
    /// php's `current.pos` — `LimitIterator::getPosition()`.
    pos: i64,
    /// The value zend's user-iterator wrapper caches between an inner
    /// `current()` and the next `move_forward`. Only `NoRewindIterator` can
    /// observe it (see the module header).
    peek: Option<Value>,
    /// `LimitIterator`'s `$offset`.
    offset: i64,
    /// `LimitIterator`'s `$limit` (`-1` = unbounded).
    limit: i64,
    /// `CachingIterator`'s and `RegexIterator`'s flag word.
    flags: i64,
    /// `CachingIterator`'s rendered `__toString`, built at fetch time.
    strval: Option<Vec<u8>>,
    /// `CachingIterator::FULL_CACHE`'s array.
    cache: Array,
    /// `RecursiveCachingIterator`'s eagerly built child iterator.
    children: Option<Object>,
    /// `CallbackFilterIterator`'s `$callback`.
    callback: Option<Value>,
    /// `RegexIterator`'s `$pattern`, `$mode` and `$pregFlags`.
    regex: Vec<u8>,
    mode: i64,
    preg_flags: i64,
    /// `AppendIterator`'s `ArrayIterator` over the appended iterators. Its
    /// *own* cursor is the one php walks, so `getArrayIterator()->next()`
    /// moves the `AppendIterator` too.
    array_it: Option<Object>,
}

impl Default for Dual {
    /// An instance that never ran a constructor — one
    /// [`Interp::instantiate`](rphp_runtime::Interp::instantiate) made — still
    /// has to answer sensibly, so `limit` starts unbounded rather than at the
    /// derived zero, which would be an empty window.
    fn default() -> Dual {
        Dual {
            inner: None,
            driver: None,
            data: None,
            key: None,
            pos: 0,
            peek: None,
            offset: 0,
            limit: -1,
            flags: 0,
            strval: None,
            cache: Array::new(),
            children: None,
            callback: None,
            regex: Vec::new(),
            mode: REGIT_MODE_MATCH,
            preg_flags: 0,
            array_it: None,
        }
    }
}

/// `clone $decorator` keeps the hidden state (and shares the inner iterator,
/// as php's `zend_objects_clone_members` does).
fn dual_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let copy = with_state::<Dual, _>(src, |s| s.clone());
    with_state::<Dual, _>(dst, |s| *s = copy);
    Ok(())
}

/// The object the `Iterator` methods go to.
fn driver_of(o: &Object) -> Option<Object> {
    with_state::<Dual, _>(o, |s| s.driver.clone())
}

/// Follow `getIterator()` until a real `Iterator` turns up: php resolves an
/// aggregate chain down to one `zend_object_iterator` at construction time.
fn resolve_driver(ctx: &mut Ctx, first: Object) -> Result<Object, Unwind> {
    let mut cur = first;
    // php recurses through the `get_iterator` handlers; a cycle would hang the
    // engine there too, so the only question is where to stop complaining.
    for _ in 0..256 {
        if instance_of(ctx, &cur, b"Iterator") {
            return Ok(cur);
        }
        let name = ctx.class_name_of(&cur);
        let next = ctx.call_method(&cur, b"getIterator", &[])?.deref().into_owned();
        match next {
            Value::Object(o) => cur = o,
            other => {
                return Err(Unwind::type_error(format!(
                    "{name}::getIterator(): Return value must be of type Traversable, {} returned",
                    rphp_runtime::value_name(&other)
                )))
            }
        }
    }
    Err(Unwind::error("Nesting level too deep - recursive dependency?"))
}

/// php's `spl_dual_it_free`: drop the cached element and invalidate the inner
/// user-iterator's cached `current()`.
fn dual_free(o: &Object) {
    with_state::<Dual, _>(o, |s| {
        s.data = None;
        s.key = None;
        s.peek = None;
    });
}

/// php's `spl_dual_it_valid`: ask the inner iterator, and answer `false`
/// without a call when there is none.
fn dual_valid(ctx: &mut Ctx, o: &Object) -> Result<bool, Unwind> {
    let Some(d) = driver_of(o) else {
        return Ok(false);
    };
    Ok(ctx.iter_call(&d, IterRole::Valid)?.to_bool())
}

/// php's `spl_dual_it_fetch`: drop the cache, then read `current()` and
/// `key()` into it. `check_more` is php's second argument — it re-asks
/// `valid()` first and leaves the cache empty when the inner has run out.
fn dual_fetch(ctx: &mut Ctx, o: &Object, check_more: bool) -> Result<(), Unwind> {
    dual_free(o);
    if check_more && !dual_valid(ctx, o)? {
        return Ok(());
    }
    let Some(d) = driver_of(o) else {
        return Ok(());
    };
    let data = ctx.iter_call(&d, IterRole::Current)?.deref().into_owned();
    let key = ctx.iter_call(&d, IterRole::Key)?.deref().into_owned();
    with_state::<Dual, _>(o, |s| {
        s.data = Some(data);
        s.key = Some(key);
    });
    Ok(())
}

/// php's `spl_dual_it_rewind`: free the cache, reset the position counter and
/// rewind the inner iterator — *without* fetching.
fn dual_rewind(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    dual_free(o);
    with_state::<Dual, _>(o, |s| s.pos = 0);
    if let Some(d) = driver_of(o) {
        ctx.iter_call(&d, IterRole::Rewind)?;
    }
    Ok(())
}

/// php's `spl_dual_it_next`: free the cache, move the inner iterator on and
/// count the step.
fn dual_next(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    dual_free(o);
    if let Some(d) = driver_of(o) {
        ctx.iter_call(&d, IterRole::Next)?;
    }
    with_state::<Dual, _>(o, |s| s.pos += 1);
    Ok(())
}

// ========================================================================
// IteratorIterator
// ========================================================================

/// `IteratorIterator::__construct(Traversable $iterator, ?string $class = null)`
///
/// An `Iterator` becomes the inner iterator as it stands and `$class` is
/// ignored outright; an `IteratorAggregate` is asked for one, and `$class`
/// then names the class whose `getIterator()` to use — it must be that
/// object's class or an ancestor of it, and itself be `Traversable`.
fn ii_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "IteratorIterator::__construct";
    let given = args[0].deref().into_owned();
    let obj = match &given {
        Value::Object(obj) if instance_of(ctx, obj, b"Traversable") => obj.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "{who}(): Argument #1 ($iterator) must be of type Traversable, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };

    let inner = if instance_of(ctx, &obj, b"Iterator") {
        obj
    } else {
        if let Some(cast) = args.get(1) {
            if !matches!(&*cast.deref(), Value::Null | Value::Uninit) {
                let name = str_arg(ctx, who, 2, "class", cast)?;
                let cast_to = ctx.class_by_name(&name);
                let traversable = ctx.class_by_name(b"Traversable");
                let ok = match (cast_to, traversable) {
                    (Some(c), Some(t)) => {
                        ctx.object_instanceof(&obj, c) && ctx.instanceof_class(c, t)
                    }
                    _ => false,
                };
                if !ok {
                    return Err(Unwind::exception(
                        "LogicException",
                        "Class to downcast to not found or not base class or does not implement Traversable",
                    ));
                }
            }
        }
        match ctx.call_method(&obj, b"getIterator", &[])?.deref().into_owned() {
            Value::Object(it) => it,
            other => {
                let name = ctx.class_name_of(&obj);
                return Err(Unwind::type_error(format!(
                    "{name}::getIterator(): Return value must be of type Traversable, {} returned",
                    rphp_runtime::value_name(&other)
                )));
            }
        }
    };
    let driver = resolve_driver(ctx, inner.clone())?;
    with_state::<Dual, _>(o, |s| {
        s.inner = Some(inner);
        s.driver = Some(driver);
    });
    Ok(Value::Null)
}

/// `getInnerIterator(): ?Traversable`
fn ii_get_inner(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with_state::<Dual, _>(o, |s| s.inner.clone()).map_or(Value::Null, Value::Object))
}

/// `rewind(): void` — rewind the inner iterator, then fetch.
fn ii_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_rewind(ctx, o)?;
    dual_fetch(ctx, o, true)?;
    Ok(Value::Null)
}

/// `valid(): bool` — whether the cache holds an element. The inner iterator is
/// not consulted.
fn ii_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(with_state::<Dual, _>(o, |s| s.data.is_some())))
}

/// `current(): mixed`
fn ii_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with_state::<Dual, _>(o, |s| s.data.clone()).unwrap_or(Value::Null))
}

/// `key(): mixed`
fn ii_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with_state::<Dual, _>(o, |s| s.key.clone()).unwrap_or(Value::Null))
}

/// `next(): void` — move the inner iterator on, then fetch.
fn ii_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_next(ctx, o)?;
    dual_fetch(ctx, o, true)?;
    Ok(Value::Null)
}

/// The constructor every `Iterator`-only decorator shares: one argument,
/// typed `Iterator`, stored as both the inner iterator and the driver.
fn take_iterator(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    class: &str,
    v: &Value,
) -> Result<(), Unwind> {
    let it = object_arg(ctx, who, 1, "iterator", class, v)?;
    with_state::<Dual, _>(o, |s| {
        s.inner = Some(it.clone());
        s.driver = Some(it);
    });
    Ok(())
}

// ========================================================================
// FilterIterator / CallbackFilterIterator
// ========================================================================

/// `FilterIterator::__construct(Iterator $iterator)`
fn filter_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    take_iterator(ctx, o, "FilterIterator::__construct", "Iterator", &args[0])?;
    Ok(Value::Null)
}

/// Skip forward over elements `accept()` rejects. `accept` is dispatched on
/// `$this`, so a subclass's implementation is what runs, and it reads the
/// element through the already-fetched cache.
fn filter_advance(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    while with_state::<Dual, _>(o, |s| s.data.is_some()) {
        if ctx.call_method(o, b"accept", &[])?.to_bool() {
            return Ok(());
        }
        dual_next(ctx, o)?;
        dual_fetch(ctx, o, true)?;
    }
    Ok(())
}

/// `FilterIterator::rewind(): void`
fn filter_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_rewind(ctx, o)?;
    dual_fetch(ctx, o, true)?;
    filter_advance(ctx, o)?;
    Ok(Value::Null)
}

/// `FilterIterator::next(): void`
fn filter_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_next(ctx, o)?;
    dual_fetch(ctx, o, true)?;
    filter_advance(ctx, o)?;
    Ok(Value::Null)
}

/// The body php gives an abstract signature. Dispatch refuses an abstract
/// method before a body could run, so reaching this is an engine bug.
fn abstract_body(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot call abstract method"))
}

/// The shared constructor of the two callback filters.
fn callback_filter_construct(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    class: &str,
    args: &mut [Value],
) -> NativeResult {
    take_iterator(ctx, o, who, class, &args[0])?;
    let cb = args[1].deref().into_owned();
    if !ctx.is_callable(&cb) {
        return Err(callback_error(who, 2, "callback", &cb));
    }
    with_state::<Dual, _>(o, |s| s.callback = Some(cb));
    Ok(Value::Null)
}

/// `CallbackFilterIterator::__construct(Iterator $iterator, callable $callback)`
fn cb_filter_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    callback_filter_construct(ctx, o, "CallbackFilterIterator::__construct", "Iterator", args)
}

/// `CallbackFilterIterator::accept(): bool` — php calls
/// `$callback($current, $key, $innerIterator)`; note the third argument is the
/// *inner* iterator, not `$this`.
fn cb_filter_accept(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (cb, data, key, inner) = with_state::<Dual, _>(o, |s| {
        (
            s.callback.clone(),
            s.data.clone(),
            s.key.clone(),
            s.inner.clone(),
        )
    });
    let Some(cb) = cb else {
        return Ok(Value::Bool(false));
    };
    let argv = [
        data.unwrap_or(Value::Null),
        key.unwrap_or(Value::Null),
        inner.map_or(Value::Null, Value::Object),
    ];
    Ok(Value::Bool(ctx.call_value(&cb, &argv)?.to_bool()))
}

// ========================================================================
// LimitIterator
// ========================================================================

/// `LimitIterator::__construct(Iterator $iterator, int $offset = 0, int $limit = -1)`
fn limit_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "LimitIterator::__construct";
    take_iterator(ctx, o, who, "Iterator", &args[0])?;
    let offset = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "offset", v)?,
        None => 0,
    };
    if offset < 0 {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($offset) must be greater than or equal to 0"
        )));
    }
    let limit = match args.get(2) {
        Some(v) => int_arg(ctx, who, 3, "limit", v)?,
        None => -1,
    };
    if limit < -1 {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #3 ($limit) must be greater than or equal to -1"
        )));
    }
    with_state::<Dual, _>(o, |s| {
        s.offset = offset;
        s.limit = limit;
    });
    Ok(Value::Null)
}

/// Whether the window still covers the counter — php's
/// `count == -1 || pos < offset + count`.
fn limit_in_window(o: &Object) -> bool {
    let (pos, offset, limit) = with_state::<Dual, _>(o, |s| (s.pos, s.offset, s.limit));
    limit == -1 || pos < offset + limit
}

/// php's `spl_limit_it_seek`. A `SeekableIterator` inner is asked to seek; any
/// other is walked forward with `next()`, and *backwards* by rewinding first.
fn limit_seek_to(ctx: &mut Ctx, o: &Object, pos: i64) -> Result<(), Unwind> {
    dual_free(o);
    let (offset, limit) = with_state::<Dual, _>(o, |s| (s.offset, s.limit));
    if pos < offset {
        return Err(Unwind::exception(
            "OutOfBoundsException",
            format!("Cannot seek to {pos} which is below the offset {offset}"),
        ));
    }
    if limit != -1 && pos >= offset + limit {
        return Err(Unwind::exception(
            "OutOfBoundsException",
            format!("Cannot seek to {pos} which is behind offset {offset} plus count {limit}"),
        ));
    }
    let Some(d) = driver_of(o) else {
        return Ok(());
    };
    if instance_of(ctx, &d, b"SeekableIterator") {
        ctx.call_method(&d, b"seek", &[Value::Int(pos)])?;
        with_state::<Dual, _>(o, |s| s.pos = pos);
        dual_fetch(ctx, o, true)?;
        return Ok(());
    }
    if pos < with_state::<Dual, _>(o, |s| s.pos) {
        dual_rewind(ctx, o)?;
    }
    while pos > with_state::<Dual, _>(o, |s| s.pos) && dual_valid(ctx, o)? {
        dual_next(ctx, o)?;
    }
    if dual_valid(ctx, o)? {
        dual_fetch(ctx, o, true)?;
    }
    Ok(())
}

/// `LimitIterator::rewind(): void` — rewind the inner one, then seek to
/// `$offset`. The seek runs even for offset 0, which is how
/// `new LimitIterator($it, 0, 0)` throws from `rewind()`.
fn limit_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_rewind(ctx, o)?;
    let offset = with_state::<Dual, _>(o, |s| s.offset);
    limit_seek_to(ctx, o, offset)?;
    Ok(Value::Null)
}

/// `LimitIterator::valid(): bool`
fn limit_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(
        limit_in_window(o) && with_state::<Dual, _>(o, |s| s.data.is_some()),
    ))
}

/// `LimitIterator::next(): void` — step the inner iterator, and only fetch
/// while the window still covers the counter.
fn limit_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_next(ctx, o)?;
    if limit_in_window(o) {
        dual_fetch(ctx, o, true)?;
    }
    Ok(Value::Null)
}

/// `LimitIterator::seek(int $offset): void`
fn limit_seek(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = int_arg(ctx, "LimitIterator::seek", 1, "offset", &args[0])?;
    limit_seek_to(ctx, o, pos)?;
    Ok(Value::Null)
}

/// `LimitIterator::getPosition(): int`
fn limit_get_position(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Dual, _>(o, |s| s.pos)))
}

// ========================================================================
// InfiniteIterator / NoRewindIterator / EmptyIterator
// ========================================================================

/// `InfiniteIterator::__construct(Iterator $iterator)`
fn infinite_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    take_iterator(ctx, o, "InfiniteIterator::__construct", "Iterator", &args[0])?;
    Ok(Value::Null)
}

/// `InfiniteIterator::next(): void` — when the inner iterator runs out it is
/// rewound instead, so the sequence starts over.
fn infinite_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_next(ctx, o)?;
    if dual_valid(ctx, o)? {
        dual_fetch(ctx, o, false)?;
    } else {
        dual_rewind(ctx, o)?;
        dual_fetch(ctx, o, true)?;
    }
    Ok(Value::Null)
}

/// `NoRewindIterator::__construct(Iterator $iterator)`
fn norewind_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    take_iterator(ctx, o, "NoRewindIterator::__construct", "Iterator", &args[0])?;
    Ok(Value::Null)
}

/// `NoRewindIterator::rewind(): void` — the whole point of the class.
fn norewind_rewind(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// `NoRewindIterator::valid(): bool` — asked of the inner iterator every time;
/// nothing is cached.
fn norewind_valid(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(dual_valid(ctx, o)?))
}

/// `NoRewindIterator::current(): mixed` — the inner element, through zend's
/// user-iterator value cache: a *user* iterator's `current()` runs once per
/// step, so a second `foreach` over a `NoRewindIterator` does not call it
/// again. An internal iterator has no such cache and is read live.
fn norewind_current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Some(d) = driver_of(o) else {
        return Ok(Value::Null);
    };
    let cached_kind = !ctx.class(d.class_id()).internal;
    if cached_kind {
        if let Some(v) = with_state::<Dual, _>(o, |s| s.peek.clone()) {
            return Ok(v);
        }
    }
    let v = ctx.iter_call(&d, IterRole::Current)?.deref().into_owned();
    if cached_kind {
        let stored = v.clone();
        with_state::<Dual, _>(o, |s| s.peek = Some(stored));
    }
    Ok(v)
}

/// `NoRewindIterator::key(): mixed` — never cached by zend, so always a call.
fn norewind_key(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Some(d) = driver_of(o) else {
        return Ok(Value::Null);
    };
    Ok(ctx.iter_call(&d, IterRole::Key)?.deref().into_owned())
}

/// `NoRewindIterator::next(): void`
fn norewind_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dual_next(ctx, o)?;
    Ok(Value::Null)
}

/// `EmptyIterator::valid(): bool`
fn empty_valid(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

/// `EmptyIterator::rewind(): void` / `next(): void`
fn empty_noop(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// `EmptyIterator::current(): never`
fn empty_current(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception(
        "BadMethodCallException",
        "Accessing the value of an EmptyIterator",
    ))
}

/// `EmptyIterator::key(): never`
fn empty_key(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception(
        "BadMethodCallException",
        "Accessing the key of an EmptyIterator",
    ))
}

// ========================================================================
// AppendIterator
// ========================================================================

/// The `ArrayIterator` holding the appended iterators, made on demand. php
/// walks *its* cursor, which is why `getArrayIterator()->next()` moves the
/// `AppendIterator` along with it.
fn append_array_it(ctx: &mut Ctx, o: &Object) -> Result<Object, Unwind> {
    if let Some(a) = with_state::<Dual, _>(o, |s| s.array_it.clone()) {
        return Ok(a);
    }
    let cid = ctx
        .class_by_name(b"ArrayIterator")
        .ok_or_else(|| Unwind::error("Class \"ArrayIterator\" not found"))?;
    let it = ctx.new_object(cid)?;
    ctx.call_method(&it, b"__construct", &[])?;
    let stored = it.clone();
    with_state::<Dual, _>(o, |s| s.array_it = Some(stored));
    Ok(it)
}

/// Whether the cursor over the appended iterators still points at one.
fn append_array_valid(ctx: &mut Ctx, o: &Object) -> Result<bool, Unwind> {
    let a = append_array_it(ctx, o)?;
    Ok(ctx.iter_call(&a, IterRole::Valid)?.to_bool())
}

/// php's `spl_append_it_next_iterator`: adopt the iterator under the cursor
/// and rewind it, or clear the inner one when the cursor has run off the end.
fn append_next_iterator(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    dual_free(o);
    if append_array_valid(ctx, o)? {
        let a = append_array_it(ctx, o)?;
        let cur = ctx.iter_call(&a, IterRole::Current)?.deref().into_owned();
        if let Value::Object(it) = cur {
            let driver = it.clone();
            with_state::<Dual, _>(o, |s| {
                s.inner = Some(it);
                s.driver = Some(driver);
            });
            dual_rewind(ctx, o)?;
            return Ok(());
        }
    }
    with_state::<Dual, _>(o, |s| {
        s.inner = None;
        s.driver = None;
    });
    Ok(())
}

/// php's `spl_append_it_fetch`: walk on to the next appended iterator until
/// one has an element, then cache it.
fn append_fetch(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    while driver_of(o).is_some() && !dual_valid(ctx, o)? {
        let a = append_array_it(ctx, o)?;
        ctx.iter_call(&a, IterRole::Next)?;
        append_next_iterator(ctx, o)?;
    }
    if driver_of(o).is_some() {
        dual_fetch(ctx, o, false)?;
    }
    Ok(())
}

/// `AppendIterator::__construct()`
fn append_construct(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    append_array_it(ctx, o)?;
    Ok(Value::Null)
}

/// `AppendIterator::append(Iterator $iterator): void`
///
/// Appending while the current inner iterator still has an element only
/// records it — php consults that iterator's `valid()` twice and changes
/// nothing else, which is what the probe against php 8.5 shows. Otherwise the
/// new iterator is picked up immediately, so `append()` on a fresh or
/// exhausted `AppendIterator` rewinds it and fetches.
fn append_append(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let it = object_arg(ctx, "AppendIterator::append", 1, "iterator", "Iterator", &args[0])?;
    let was_valid = driver_of(o).is_some() && dual_valid(ctx, o)?;
    let a = append_array_it(ctx, o)?;
    ctx.call_method(&a, b"append", &[Value::Object(it)])?;
    if was_valid {
        dual_valid(ctx, o)?;
        return Ok(Value::Null);
    }
    if !append_array_valid(ctx, o)? {
        let a = append_array_it(ctx, o)?;
        ctx.iter_call(&a, IterRole::Rewind)?;
    }
    append_next_iterator(ctx, o)?;
    append_fetch(ctx, o)?;
    Ok(Value::Null)
}

/// `AppendIterator::rewind(): void`
fn append_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let a = append_array_it(ctx, o)?;
    ctx.iter_call(&a, IterRole::Rewind)?;
    append_next_iterator(ctx, o)?;
    append_fetch(ctx, o)?;
    Ok(Value::Null)
}

/// `AppendIterator::current(): mixed` — re-fetches, so it is the method that
/// rolls on to the next appended iterator.
fn append_current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    append_fetch(ctx, o)?;
    Ok(with_state::<Dual, _>(o, |s| s.data.clone()).unwrap_or(Value::Null))
}

/// `AppendIterator::next(): void`
fn append_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    if dual_valid(ctx, o)? {
        dual_next(ctx, o)?;
    }
    append_fetch(ctx, o)?;
    Ok(Value::Null)
}

/// `AppendIterator::getIteratorIndex(): ?int`
fn append_index(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    if !append_array_valid(ctx, o)? {
        return Ok(Value::Null);
    }
    let a = append_array_it(ctx, o)?;
    Ok(ctx.iter_call(&a, IterRole::Key)?.deref().into_owned())
}

/// `AppendIterator::getArrayIterator(): ArrayIterator`
fn append_array_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Object(append_array_it(ctx, o)?))
}

// ========================================================================
// CachingIterator
// ========================================================================

/// php's validation of the four mutually exclusive string-source bits.
fn caching_check_flags(who: &str, flags: i64) -> Result<(), Unwind> {
    if (flags & CIT_TOSTRING_MASK).count_ones() > 1 {
        return Err(Unwind::value_error(format!(
            "{who} must contain only one of CachingIterator::CALL_TOSTRING, \
             CachingIterator::TOSTRING_USE_KEY, CachingIterator::TOSTRING_USE_CURRENT, \
             or CachingIterator::TOSTRING_USE_INNER"
        )));
    }
    Ok(())
}

/// The shared constructor of `CachingIterator` and `RecursiveCachingIterator`.
fn caching_construct_as(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    class: &str,
    args: &mut [Value],
) -> NativeResult {
    take_iterator(ctx, o, who, class, &args[0])?;
    let flags = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "flags", v)?,
        None => CIT_CALL_TOSTRING,
    };
    caching_check_flags(&format!("{who}(): Argument #2 ($flags)"), flags)?;
    with_state::<Dual, _>(o, |s| s.flags = flags);
    Ok(Value::Null)
}

/// `CachingIterator::__construct(Iterator $iterator, int $flags = CALL_TOSTRING)`
fn caching_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    caching_construct_as(ctx, o, "CachingIterator::__construct", "Iterator", args)
}

/// php's `spl_caching_it_next`: cache the element under the inner iterator —
/// with its string form, its cache entry and, for
/// `RecursiveCachingIterator`, its children — and *then* step the inner
/// iterator on. That one-ahead move is what makes `hasNext()` meaningful.
fn caching_fetch(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    if !dual_valid(ctx, o)? {
        dual_free(o);
        with_state::<Dual, _>(o, |s| {
            s.strval = None;
            s.children = None;
        });
        return Ok(());
    }
    dual_fetch(ctx, o, false)?;
    let (flags, data, key, inner) = with_state::<Dual, _>(o, |s| {
        (s.flags, s.data.clone(), s.key.clone(), s.inner.clone())
    });
    let data = data.unwrap_or(Value::Null);
    let key = key.unwrap_or(Value::Null);

    let strval = if flags & (CIT_CALL_TOSTRING | CIT_TOSTRING_USE_CURRENT) != 0 {
        Some(ctx.to_string(&data)?.as_bytes().to_vec())
    } else if flags & CIT_TOSTRING_USE_KEY != 0 {
        Some(ctx.to_string(&key)?.as_bytes().to_vec())
    } else if flags & CIT_TOSTRING_USE_INNER != 0 {
        let v = inner.clone().map_or(Value::Null, Value::Object);
        Some(ctx.to_string(&v)?.as_bytes().to_vec())
    } else {
        None
    };
    with_state::<Dual, _>(o, |s| s.strval = strval);

    if flags & CIT_FULL_CACHE != 0 {
        let entry = data.clone();
        let k = array_key(&key);
        with_state::<Dual, _>(o, |s| match k {
            Some(k) => s.cache.set(k, entry),
            None => s.cache.push(entry),
        });
    }

    // `RecursiveCachingIterator` decides about children here, while the inner
    // iterator still stands on the element it just cached. `CATCH_GET_CHILD`
    // turns a refusal into "no children" instead of an unwind.
    if instance_of(ctx, o, b"RecursiveCachingIterator") {
        let mut children = None;
        if let Some(inner) = inner {
            match caching_children(ctx, o, &inner, flags) {
                Ok(c) => children = c,
                Err(e) => {
                    if flags & CIT_CATCH_GET_CHILD == 0 || matches!(e, Unwind::Exit(_)) {
                        return Err(e);
                    }
                }
            }
        }
        with_state::<Dual, _>(o, |s| s.children = children);
    }

    if let Some(d) = driver_of(o) {
        ctx.iter_call(&d, IterRole::Next)?;
    }
    Ok(())
}

/// The child iterator a `RecursiveCachingIterator` builds ahead of time, or
/// `None` when the element under the inner iterator has no children.
fn caching_children(
    ctx: &mut Ctx,
    o: &Object,
    inner: &Object,
    flags: i64,
) -> Result<Option<Object>, Unwind> {
    if !ctx.call_method(inner, b"hasChildren", &[])?.to_bool() {
        return Ok(None);
    }
    let kids = ctx.call_method(inner, b"getChildren", &[])?.deref().into_owned();
    let child = ctx.new_object(o.class_id())?;
    ctx.call_method(&child, b"__construct", &[kids, Value::Int(flags)])?;
    Ok(Some(child))
}

/// `CachingIterator::rewind(): void` — the full cache starts over too.
fn caching_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    with_state::<Dual, _>(o, |s| s.cache = Array::new());
    dual_rewind(ctx, o)?;
    caching_fetch(ctx, o)?;
    Ok(Value::Null)
}

/// `CachingIterator::next(): void`
fn caching_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    caching_fetch(ctx, o)?;
    Ok(Value::Null)
}

/// `CachingIterator::hasNext(): bool` — the inner iterator already stands on
/// the *following* element, so this is a live `valid()`.
fn caching_has_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(dual_valid(ctx, o)?))
}

/// `CachingIterator::__toString(): string`
fn caching_to_string(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (flags, strval) = with_state::<Dual, _>(o, |s| (s.flags, s.strval.clone()));
    if flags & CIT_TOSTRING_MASK == 0 {
        return Err(Unwind::exception(
            "BadMethodCallException",
            "CachingIterator does not fetch string value (see CachingIterator::__construct)",
        ));
    }
    Ok(Value::string(&strval.unwrap_or_default()))
}

/// `CachingIterator::getFlags(): int`
fn caching_get_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Dual, _>(o, |s| s.flags)))
}

/// `CachingIterator::setFlags(int $flags): void` — the two string-source bits
/// php refuses to let go of stay where they are.
fn caching_set_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let flags = int_arg(ctx, "CachingIterator::setFlags", 1, "flags", &args[0])?;
    caching_check_flags("CachingIterator::setFlags(): Argument #1 ($flags)", flags)?;
    let current = with_state::<Dual, _>(o, |s| s.flags);
    if current & CIT_CALL_TOSTRING != 0 && flags & CIT_CALL_TOSTRING == 0 {
        return Err(Unwind::exception(
            "InvalidArgumentException",
            "Unsetting flag CALL_TO_STRING is not possible",
        ));
    }
    if current & CIT_TOSTRING_USE_INNER != 0 && flags & CIT_TOSTRING_USE_INNER == 0 {
        return Err(Unwind::exception(
            "InvalidArgumentException",
            "Unsetting flag TOSTRING_USE_INNER is not possible",
        ));
    }
    with_state::<Dual, _>(o, |s| s.flags = flags);
    Ok(Value::Null)
}

/// The full cache, or php's refusal when the instance does not keep one.
fn caching_cache(o: &Object) -> Result<Array, Unwind> {
    let (flags, cache) = with_state::<Dual, _>(o, |s| (s.flags, s.cache.clone()));
    if flags & CIT_FULL_CACHE == 0 {
        return Err(Unwind::exception(
            "BadMethodCallException",
            "CachingIterator does not use a full cache (see CachingIterator::__construct)",
        ));
    }
    Ok(cache)
}

/// `CachingIterator::getCache(): array`
fn caching_get_cache(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(caching_cache(o)?))
}

/// `CachingIterator::count(): int`
fn caching_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(caching_cache(o)?.len() as i64))
}

/// `CachingIterator::offsetExists(mixed $key): bool`
fn caching_offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cache = caching_cache(o)?;
    let k = array_key(&args[0].deref()).ok_or_else(|| Unwind::type_error("Illegal offset type"))?;
    Ok(Value::Bool(cache.contains_key(&k)))
}

/// `CachingIterator::offsetGet(mixed $key): mixed`
fn caching_offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cache = caching_cache(o)?;
    let k = array_key(&args[0].deref()).ok_or_else(|| Unwind::type_error("Illegal offset type"))?;
    match cache.get_deref(&k) {
        Some(v) => Ok(v),
        None => {
            ctx.warn(&match &k {
                ArrayKey::Int(i) => format!("Undefined array key {i}"),
                ArrayKey::Str(s) => {
                    format!("Undefined array key \"{}\"", String::from_utf8_lossy(s))
                }
            })?;
            Ok(Value::Null)
        }
    }
}

/// `CachingIterator::offsetSet(mixed $key, mixed $value): void`
fn caching_offset_set(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut cache = caching_cache(o)?;
    let k = array_key(&args[0].deref()).ok_or_else(|| Unwind::type_error("Illegal offset type"))?;
    cache.set(k, args[1].deref().into_owned());
    with_state::<Dual, _>(o, |s| s.cache = cache);
    Ok(Value::Null)
}

/// `CachingIterator::offsetUnset(mixed $key): void`
fn caching_offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut cache = caching_cache(o)?;
    let k = array_key(&args[0].deref()).ok_or_else(|| Unwind::type_error("Illegal offset type"))?;
    let _ = cache.unset(&k);
    with_state::<Dual, _>(o, |s| s.cache = cache);
    Ok(Value::Null)
}

// ========================================================================
// RegexIterator
// ========================================================================

/// php's `ValueError` listing the five modes.
fn regex_mode_error(who: &str, arg: &str) -> Unwind {
    Unwind::value_error(format!(
        "{who}(): {arg} must be RegexIterator::MATCH, RegexIterator::GET_MATCH, \
         RegexIterator::ALL_MATCHES, RegexIterator::SPLIT, or RegexIterator::REPLACE"
    ))
}

/// The shared constructor of `RegexIterator` and `RecursiveRegexIterator`.
fn regex_construct_as(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    class: &str,
    args: &mut [Value],
) -> NativeResult {
    take_iterator(ctx, o, who, class, &args[0])?;
    let regex = str_arg(ctx, who, 2, "pattern", &args[1])?;
    let mode = match args.get(2) {
        Some(v) => int_arg(ctx, who, 3, "mode", v)?,
        None => REGIT_MODE_MATCH,
    };
    if !(REGIT_MODE_MATCH..=REGIT_MODE_REPLACE).contains(&mode) {
        return Err(regex_mode_error(who, "Argument #3 ($mode)"));
    }
    let flags = match args.get(3) {
        Some(v) => int_arg(ctx, who, 4, "flags", v)?,
        None => 0,
    };
    let preg_flags = match args.get(4) {
        Some(v) => int_arg(ctx, who, 5, "pregFlags", v)?,
        None => 0,
    };
    with_state::<Dual, _>(o, |s| {
        s.regex = regex;
        s.mode = mode;
        s.flags = flags;
        s.preg_flags = preg_flags;
    });
    Ok(Value::Null)
}

/// `RegexIterator::__construct(Iterator $iterator, string $pattern, int $mode = MATCH, int $flags = 0, int $pregFlags = 0)`
fn regex_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    regex_construct_as(ctx, o, "RegexIterator::__construct", "Iterator", args)
}

/// Call a registered `preg_*` native with a by-reference output parameter at
/// `out_at`, returning both the result and what was written back.
fn call_preg(
    ctx: &mut Ctx,
    name: &[u8],
    mut argv: Vec<Value>,
    out_at: Option<usize>,
) -> Result<(Value, Value), Unwind> {
    let cell = out_at.map(|i| {
        let cell = PhpRef::new(Value::Null);
        argv[i] = Value::Ref(cell.clone());
        cell
    });
    let id = ctx
        .native_by_name(name)
        .ok_or_else(|| Unwind::error("the pcre extension is not registered"))?;
    let r = ctx.call_native(id, &mut argv)?;
    Ok((r, cell.map_or(Value::Null, |c| c.get())))
}

/// `RegexIterator::accept(): bool`
///
/// The subject is the key under `USE_KEY` and the element otherwise. Every
/// mode but `MATCH` rewrites what the iterator hands out: `REPLACE` writes
/// back over *the subject* (so with `USE_KEY` it is the key that changes),
/// and the other three always replace the element.
fn regex_accept(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (data, key, regex, mode, flags, preg_flags) = with_state::<Dual, _>(o, |s| {
        (
            s.data.clone(),
            s.key.clone(),
            s.regex.clone(),
            s.mode,
            s.flags,
            s.preg_flags,
        )
    });
    let Some(data) = data else {
        return Ok(Value::Bool(false));
    };
    let key = key.unwrap_or(Value::Null);
    let use_key = flags & REGIT_USE_KEY != 0;
    let subject = ctx
        .to_string(if use_key { &key } else { &data })?
        .as_bytes()
        .to_vec();
    let pattern = Value::string(&regex);
    let subject = Value::string(&subject);

    let mut accepted;
    match mode {
        REGIT_MODE_MATCH => {
            let (r, _) = call_preg(
                ctx,
                b"preg_match",
                vec![pattern, subject, Value::Null, Value::Int(preg_flags), Value::Int(0)],
                Some(2),
            )?;
            accepted = r.to_int() > 0;
        }
        REGIT_MODE_GET_MATCH => {
            let (r, matches) = call_preg(
                ctx,
                b"preg_match",
                vec![pattern, subject, Value::Null, Value::Int(preg_flags), Value::Int(0)],
                Some(2),
            )?;
            accepted = r.to_int() > 0;
            with_state::<Dual, _>(o, |s| s.data = Some(matches));
        }
        REGIT_MODE_ALL_MATCHES => {
            // php's `preg_match_all` defaults to `PREG_PATTERN_ORDER`; the
            // iterator passes `$pregFlags` straight through, so supply the
            // default when it names no ordering.
            let pf = if preg_flags & (PREG_PATTERN_ORDER | PREG_SET_ORDER) == 0 {
                preg_flags | PREG_PATTERN_ORDER
            } else {
                preg_flags
            };
            let (r, matches) = call_preg(
                ctx,
                b"preg_match_all",
                vec![pattern, subject, Value::Null, Value::Int(pf), Value::Int(0)],
                Some(2),
            )?;
            accepted = r.to_int() > 0;
            with_state::<Dual, _>(o, |s| s.data = Some(matches));
        }
        REGIT_MODE_SPLIT => {
            let (r, _) = call_preg(
                ctx,
                b"preg_split",
                vec![pattern, subject, Value::Int(-1), Value::Int(preg_flags)],
                None,
            )?;
            accepted = matches!(&r, Value::Array(a) if a.len() > 1);
            with_state::<Dual, _>(o, |s| s.data = Some(r));
        }
        _ => {
            let replacement = o
                .get_deref(b"replacement")
                .filter(|v| !matches!(v, Value::Null | Value::Uninit))
                .unwrap_or_else(|| Value::string(b""));
            let (r, count) = call_preg(
                ctx,
                b"preg_replace",
                vec![
                    pattern,
                    replacement,
                    subject,
                    Value::Int(-1),
                    Value::Null,
                ],
                Some(4),
            )?;
            accepted = count.to_int() > 0;
            with_state::<Dual, _>(o, |s| {
                if use_key {
                    s.key = Some(r);
                } else {
                    s.data = Some(r);
                }
            });
        }
    }
    if flags & REGIT_INVERT_MATCH != 0 {
        accepted = !accepted;
    }
    Ok(Value::Bool(accepted))
}

/// `RegexIterator::getMode(): int`
fn regex_get_mode(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Dual, _>(o, |s| s.mode)))
}

/// `RegexIterator::setMode(int $mode): void`
fn regex_set_mode(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mode = int_arg(ctx, "RegexIterator::setMode", 1, "mode", &args[0])?;
    if !(REGIT_MODE_MATCH..=REGIT_MODE_REPLACE).contains(&mode) {
        return Err(regex_mode_error(
            "RegexIterator::setMode",
            "Argument #1 ($mode)",
        ));
    }
    with_state::<Dual, _>(o, |s| s.mode = mode);
    Ok(Value::Null)
}

/// `RegexIterator::getFlags(): int`
fn regex_get_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Dual, _>(o, |s| s.flags)))
}

/// `RegexIterator::setFlags(int $flags): void`
fn regex_set_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let flags = int_arg(ctx, "RegexIterator::setFlags", 1, "flags", &args[0])?;
    with_state::<Dual, _>(o, |s| s.flags = flags);
    Ok(Value::Null)
}

/// `RegexIterator::getRegex(): string`
fn regex_get_regex(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::string(&with_state::<Dual, _>(o, |s| s.regex.clone())))
}

/// `RegexIterator::getPregFlags(): int`
fn regex_get_preg_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Dual, _>(o, |s| s.preg_flags)))
}

/// `RegexIterator::setPregFlags(int $pregFlags): void`
fn regex_set_preg_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let f = int_arg(ctx, "RegexIterator::setPregFlags", 1, "pregFlags", &args[0])?;
    with_state::<Dual, _>(o, |s| s.preg_flags = f);
    Ok(Value::Null)
}

// ========================================================================
// the recursive decorators built on the dual iterator
// ========================================================================

/// `$this->getInnerIterator()->hasChildren()` — the shared body of
/// `RecursiveFilterIterator`, `RecursiveCallbackFilterIterator`,
/// `RecursiveRegexIterator` and `ParentIterator::accept`.
fn inner_has_children(ctx: &mut Ctx, o: &Object) -> Result<bool, Unwind> {
    let Some(inner) = with_state::<Dual, _>(o, |s| s.inner.clone()) else {
        return Ok(false);
    };
    Ok(ctx.call_method(&inner, b"hasChildren", &[])?.to_bool())
}

/// `$this->getInnerIterator()->getChildren()`.
fn inner_get_children(ctx: &mut Ctx, o: &Object) -> Result<Value, Unwind> {
    let Some(inner) = with_state::<Dual, _>(o, |s| s.inner.clone()) else {
        return Ok(Value::Null);
    };
    Ok(ctx.call_method(&inner, b"getChildren", &[])?.deref().into_owned())
}

/// `hasChildren(): bool` for every recursive decorator in this family.
fn rec_has_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(inner_has_children(ctx, o)?))
}

/// `RecursiveFilterIterator::getChildren(): RecursiveFilterIterator` —
/// `new static($this->getInnerIterator()->getChildren())`, so a subclass keeps
/// its own class all the way down.
fn rec_filter_get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let kids = inner_get_children(ctx, o)?;
    let child = ctx.new_object(o.class_id())?;
    ctx.call_method(&child, b"__construct", &[kids])?;
    Ok(Value::Object(child))
}

/// `RecursiveFilterIterator::__construct(RecursiveIterator $iterator)`
fn rec_filter_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    take_iterator(
        ctx,
        o,
        "RecursiveFilterIterator::__construct",
        "RecursiveIterator",
        &args[0],
    )?;
    Ok(Value::Null)
}

/// `ParentIterator::__construct(RecursiveIterator $iterator)`
fn parent_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    take_iterator(
        ctx,
        o,
        "ParentIterator::__construct",
        "RecursiveIterator",
        &args[0],
    )?;
    Ok(Value::Null)
}

/// `ParentIterator::accept(): bool` — keep only the elements that have
/// children.
fn parent_accept(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(inner_has_children(ctx, o)?))
}

/// `RecursiveCallbackFilterIterator::__construct(RecursiveIterator $iterator, callable $callback)`
fn rec_cb_filter_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    callback_filter_construct(
        ctx,
        o,
        "RecursiveCallbackFilterIterator::__construct",
        "RecursiveIterator",
        args,
    )
}

/// `RecursiveCallbackFilterIterator::getChildren(): RecursiveCallbackFilterIterator`
fn rec_cb_filter_get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let kids = inner_get_children(ctx, o)?;
    let cb = with_state::<Dual, _>(o, |s| s.callback.clone()).unwrap_or(Value::Null);
    let child = ctx.new_object(o.class_id())?;
    ctx.call_method(&child, b"__construct", &[kids, cb])?;
    Ok(Value::Object(child))
}

/// `RecursiveRegexIterator::__construct(RecursiveIterator $iterator, string $pattern, …)`
fn rec_regex_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    regex_construct_as(
        ctx,
        o,
        "RecursiveRegexIterator::__construct",
        "RecursiveIterator",
        args,
    )
}

/// `RecursiveRegexIterator::accept(): bool` — a branch (an array element)
/// is kept when it is non-empty; a leaf goes through the regex.
fn rec_regex_accept(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match with_state::<Dual, _>(o, |s| s.data.clone()) {
        None => Ok(Value::Bool(false)),
        Some(Value::Array(a)) => Ok(Value::Bool(!a.is_empty())),
        Some(_) => regex_accept(ctx, Some(o), args),
    }
}

/// `RecursiveRegexIterator::getChildren(): RecursiveRegexIterator`
fn rec_regex_get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let kids = inner_get_children(ctx, o)?;
    let (regex, mode, flags, preg_flags) =
        with_state::<Dual, _>(o, |s| (s.regex.clone(), s.mode, s.flags, s.preg_flags));
    let child = ctx.new_object(o.class_id())?;
    ctx.call_method(
        &child,
        b"__construct",
        &[
            kids,
            Value::string(&regex),
            Value::Int(mode),
            Value::Int(flags),
            Value::Int(preg_flags),
        ],
    )?;
    Ok(Value::Object(child))
}

/// `RecursiveCachingIterator::__construct(RecursiveIterator $iterator, int $flags = CALL_TOSTRING)`
fn rec_caching_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    caching_construct_as(
        ctx,
        o,
        "RecursiveCachingIterator::__construct",
        "RecursiveIterator",
        args,
    )
}

/// `RecursiveCachingIterator::hasChildren(): bool` — answered from the child
/// iterator built at fetch time, because the inner iterator has already moved
/// on by now.
fn rec_caching_has_children(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(with_state::<Dual, _>(o, |s| {
        s.children.is_some()
    })))
}

/// `RecursiveCachingIterator::getChildren(): ?RecursiveCachingIterator`
fn rec_caching_get_children(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with_state::<Dual, _>(o, |s| s.children.clone()).map_or(Value::Null, Value::Object))
}

// ========================================================================
// RecursiveArrayIterator
// ========================================================================

/// The element under an `ArrayIterator`'s cursor. php reads its own cursor;
/// this goes through `current()` because the cursor lives in
/// `spl_containers.rs`'s private payload (see the module header).
fn rai_current(ctx: &mut Ctx, o: &Object) -> Result<Value, Unwind> {
    Ok(ctx.iter_call(o, IterRole::Current)?.deref().into_owned())
}

/// `RecursiveArrayIterator::hasChildren(): bool` — an array always, an object
/// unless `CHILD_ARRAYS_ONLY` is set.
fn rai_has_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cur = rai_current(ctx, o)?;
    let flags = ctx.call_method(o, b"getFlags", &[])?.to_int();
    Ok(Value::Bool(match cur {
        Value::Array(_) => true,
        Value::Object(_) => flags & RAIT_CHILD_ARRAYS_ONLY == 0,
        _ => false,
    }))
}

/// `RecursiveArrayIterator::getChildren(): ?RecursiveArrayIterator`
///
/// An element that already *is* a `RecursiveArrayIterator` is handed back as
/// it stands; anything else is wrapped in `new static($element, $flags)`, so a
/// leaf produces the `ArrayIterator` constructor's own `TypeError`.
fn rai_get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cur = rai_current(ctx, o)?;
    let flags = ctx.call_method(o, b"getFlags", &[])?.to_int();
    if let Value::Object(c) = &cur {
        if flags & RAIT_CHILD_ARRAYS_ONLY != 0 {
            return Ok(Value::Null);
        }
        if instance_of(ctx, c, b"RecursiveArrayIterator") {
            return Ok(cur);
        }
    }
    let child = ctx.new_object(o.class_id())?;
    ctx.call_method(&child, b"__construct", &[cur, Value::Int(flags)])?;
    Ok(Value::Object(child))
}

// ========================================================================
// RecursiveIteratorIterator
// ========================================================================

/// Where one level of the walk stands. These are php's `RS_*` states, and the
/// transitions between them are the whole class.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Look at the element under the cursor.
    Start,
    /// Ask `callHasChildren()` about it.
    Test,
    /// It was reported as itself and is still to be descended into
    /// (`SELF_FIRST` only).
    SelfDone,
    /// Descend into it.
    Child,
    /// Move the cursor on.
    Next,
}

/// One level of the walk.
#[derive(Clone)]
struct Level {
    it: Object,
    step: Step,
}

/// `RecursiveIteratorIterator`'s hidden state.
#[derive(Clone)]
struct Rii {
    levels: Vec<Level>,
    level: usize,
    mode: i64,
    flags: i64,
    /// `-1` is php's "no limit", which `getMaxDepth()` reports as `false`.
    max_depth: i64,
}

impl Default for Rii {
    /// The derived zero would mean `setMaxDepth(0)`, so the unlimited depth
    /// php starts with is spelled out.
    fn default() -> Rii {
        Rii {
            levels: Vec::new(),
            level: 0,
            mode: RIT_LEAVES_ONLY,
            flags: 0,
            max_depth: -1,
        }
    }
}

/// `clone $rii` keeps the walk exactly where it was.
fn rii_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let copy = with_state::<Rii, _>(src, |s| s.clone());
    with_state::<Rii, _>(dst, |s| *s = copy);
    Ok(())
}

/// The iterator at the current level.
fn rii_sub(o: &Object) -> Option<Object> {
    with_state::<Rii, _>(o, |s| s.levels.get(s.level).map(|l| l.it.clone()))
}

/// `RecursiveIteratorIterator::__construct(Traversable $iterator, int $mode = LEAVES_ONLY, int $flags = 0)`
///
/// php declares the parameter `object` and then insists on a
/// `RecursiveIterator` itself, so a plain `Iterator` is an
/// `InvalidArgumentException` rather than a `TypeError`.
fn rii_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "RecursiveIteratorIterator::__construct";
    let given = args[0].deref().into_owned();
    let Value::Object(obj) = &given else {
        return Err(Unwind::type_error(format!(
            "{who}(): Argument #1 ($iterator) must be of type object, {} given",
            rphp_runtime::value_name(&given)
        )));
    };
    let root = if instance_of(ctx, obj, b"RecursiveIterator") {
        obj.clone()
    } else if instance_of(ctx, obj, b"IteratorAggregate") {
        let it = resolve_driver(ctx, obj.clone())?;
        if !instance_of(ctx, &it, b"RecursiveIterator") {
            return Err(Unwind::exception(
                "InvalidArgumentException",
                "An instance of RecursiveIterator or IteratorAggregate creating it is required",
            ));
        }
        it
    } else {
        return Err(Unwind::exception(
            "InvalidArgumentException",
            "An instance of RecursiveIterator or IteratorAggregate creating it is required",
        ));
    };
    let mode = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "mode", v)?,
        None => RIT_LEAVES_ONLY,
    };
    let flags = match args.get(2) {
        Some(v) => int_arg(ctx, who, 3, "flags", v)?,
        None => 0,
    };
    with_state::<Rii, _>(o, |s| {
        s.levels = vec![Level {
            it: root,
            step: Step::Start,
        }];
        s.level = 0;
        s.mode = mode;
        s.flags = flags;
        s.max_depth = -1;
    });
    Ok(Value::Null)
}

/// Read one field of the state.
fn rii_step(o: &Object) -> Step {
    with_state::<Rii, _>(o, |s| {
        s.levels.get(s.level).map_or(Step::Next, |l| l.step)
    })
}

/// Write the current level's state.
fn rii_set_step(o: &Object, step: Step) {
    with_state::<Rii, _>(o, |s| {
        let lvl = s.level;
        if let Some(l) = s.levels.get_mut(lvl) {
            l.step = step;
        }
    });
}

/// php's `spl_recursive_it_move_forward_ex`: run the state machine until it
/// stands on an element the mode says to report, or the root runs out.
fn rii_advance(ctx: &mut Ctx, o: &Object) -> Result<(), Unwind> {
    loop {
        let (level, mode, max_depth, flags) =
            with_state::<Rii, _>(o, |s| (s.level, s.mode, s.max_depth, s.flags));
        match rii_step(o) {
            Step::Next => {
                if let Some(it) = rii_sub(o) {
                    ctx.iter_call(&it, IterRole::Next)?;
                }
                rii_set_step(o, Step::Start);
            }
            Step::Start => {
                let valid = match rii_sub(o) {
                    Some(it) => ctx.iter_call(&it, IterRole::Valid)?.to_bool(),
                    None => false,
                };
                if valid {
                    rii_set_step(o, Step::Test);
                    continue;
                }
                if level == 0 {
                    return Ok(());
                }
                ctx.call_method(o, b"endChildren", &[])?;
                with_state::<Rii, _>(o, |s| {
                    s.level -= 1;
                    let lvl = s.level;
                    if let Some(l) = s.levels.get_mut(lvl) {
                        l.step = Step::Next;
                    }
                });
                if mode == RIT_CHILD_FIRST {
                    // The parent is reported once its children are done.
                    ctx.call_method(o, b"nextElement", &[])?;
                    return Ok(());
                }
            }
            Step::Test => {
                // `CATCH_GET_CHILD` covers this call too: a branch that
                // refuses to say whether it has children is treated as a leaf.
                let has = match ctx.call_method(o, b"callHasChildren", &[]) {
                    Ok(v) => v.to_bool(),
                    Err(e) => {
                        if flags & RIT_CATCH_GET_CHILD == 0 || matches!(e, Unwind::Exit(_)) {
                            return Err(e);
                        }
                        false
                    }
                };
                let can_recurse = max_depth == -1 || max_depth > level as i64;
                if has && can_recurse {
                    if mode == RIT_SELF_FIRST {
                        rii_set_step(o, Step::SelfDone);
                        ctx.call_method(o, b"nextElement", &[])?;
                        return Ok(());
                    }
                    rii_set_step(o, Step::Child);
                    continue;
                }
                rii_set_step(o, Step::Next);
                if has && mode == RIT_LEAVES_ONLY {
                    // A branch too deep to enter is not a leaf, so it is
                    // simply skipped.
                    continue;
                }
                ctx.call_method(o, b"nextElement", &[])?;
                return Ok(());
            }
            Step::SelfDone => {
                rii_set_step(o, if mode == RIT_SELF_FIRST { Step::Child } else { Step::Next });
            }
            Step::Child => {
                let kids = match ctx.call_method(o, b"callGetChildren", &[]) {
                    Ok(v) => Some(v.deref().into_owned()),
                    Err(e) => {
                        // `CATCH_GET_CHILD` swallows the failure and moves on;
                        // `exit` is not an exception and never is swallowed.
                        if flags & RIT_CATCH_GET_CHILD == 0 || matches!(e, Unwind::Exit(_)) {
                            return Err(e);
                        }
                        None
                    }
                };
                match kids {
                    Some(Value::Object(child)) => {
                        ctx.iter_call(&child, IterRole::Rewind)?;
                        with_state::<Rii, _>(o, |s| {
                            s.level += 1;
                            let lvl = s.level;
                            s.levels.truncate(lvl);
                            s.levels.push(Level {
                                it: child,
                                step: Step::Start,
                            });
                        });
                        ctx.call_method(o, b"beginChildren", &[])?;
                    }
                    _ => rii_set_step(o, Step::Next),
                }
            }
        }
    }
}

/// `RecursiveIteratorIterator::rewind(): void`
fn rii_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    with_state::<Rii, _>(o, |s| {
        s.level = 0;
        s.levels.truncate(1);
        if let Some(l) = s.levels.first_mut() {
            l.step = Step::Start;
        }
    });
    if let Some(it) = rii_sub(o) {
        ctx.iter_call(&it, IterRole::Rewind)?;
    }
    ctx.call_method(o, b"beginIteration", &[])?;
    rii_advance(ctx, o)?;
    Ok(Value::Null)
}

/// `RecursiveIteratorIterator::valid(): bool` — walks back down the levels
/// looking for one that still has an element, and calls `endIteration()` when
/// none does.
fn rii_valid(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    loop {
        let valid = match rii_sub(o) {
            Some(it) => ctx.iter_call(&it, IterRole::Valid)?.to_bool(),
            None => false,
        };
        if valid {
            return Ok(Value::Bool(true));
        }
        let level = with_state::<Rii, _>(o, |s| s.level);
        if level == 0 {
            break;
        }
        with_state::<Rii, _>(o, |s| s.level -= 1);
    }
    ctx.call_method(o, b"endIteration", &[])?;
    with_state::<Rii, _>(o, |s| s.level = 0);
    Ok(Value::Bool(false))
}

/// `RecursiveIteratorIterator::current(): mixed`
fn rii_current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match rii_sub(o) {
        Some(it) => Ok(ctx.iter_call(&it, IterRole::Current)?.deref().into_owned()),
        None => Ok(Value::Null),
    }
}

/// `RecursiveIteratorIterator::key(): mixed`
fn rii_key(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match rii_sub(o) {
        Some(it) => Ok(ctx.iter_call(&it, IterRole::Key)?.deref().into_owned()),
        None => Ok(Value::Null),
    }
}

/// `RecursiveIteratorIterator::next(): void`
fn rii_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    rii_advance(ctx, o)?;
    Ok(Value::Null)
}

/// `RecursiveIteratorIterator::getDepth(): int`
fn rii_get_depth(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Rii, _>(o, |s| s.level as i64)))
}

/// `RecursiveIteratorIterator::getSubIterator(?int $level = null): ?RecursiveIterator`
fn rii_get_sub_iterator(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let level = match args.first() {
        Some(v) if !matches!(&*v.deref(), Value::Null | Value::Uninit) => int_arg(
            ctx,
            "RecursiveIteratorIterator::getSubIterator",
            1,
            "level",
            v,
        )?,
        _ => with_state::<Rii, _>(o, |s| s.level as i64),
    };
    if level < 0 {
        return Ok(Value::Null);
    }
    Ok(
        with_state::<Rii, _>(o, |s| s.levels.get(level as usize).map(|l| l.it.clone()))
            .map_or(Value::Null, Value::Object),
    )
}

/// `RecursiveIteratorIterator::getInnerIterator(): RecursiveIterator` — the
/// sub-iterator at the current depth.
fn rii_get_inner(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(rii_sub(o).map_or(Value::Null, Value::Object))
}

/// The `beginIteration`/`endIteration`/`beginChildren`/`endChildren`/
/// `nextElement` hooks: no-ops php provides so a subclass can override them.
fn rii_hook(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// `RecursiveIteratorIterator::callHasChildren(): bool`
fn rii_call_has_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match rii_sub(o) {
        Some(it) => Ok(Value::Bool(
            ctx.call_method(&it, b"hasChildren", &[])?.to_bool(),
        )),
        None => Ok(Value::Bool(false)),
    }
}

/// `RecursiveIteratorIterator::callGetChildren(): ?RecursiveIterator`
fn rii_call_get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match rii_sub(o) {
        Some(it) => Ok(ctx.call_method(&it, b"getChildren", &[])?.deref().into_owned()),
        None => Ok(Value::Null),
    }
}

/// `RecursiveIteratorIterator::setMaxDepth(int $maxDepth = -1): void`
fn rii_set_max_depth(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "RecursiveIteratorIterator::setMaxDepth";
    let d = match args.first() {
        Some(v) => int_arg(ctx, who, 1, "maxDepth", v)?,
        None => -1,
    };
    if d < -1 {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($maxDepth) must be greater than or equal to -1"
        )));
    }
    with_state::<Rii, _>(o, |s| s.max_depth = d);
    Ok(Value::Null)
}

/// `RecursiveIteratorIterator::getMaxDepth(): int|false`
fn rii_get_max_depth(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let d = with_state::<Rii, _>(o, |s| s.max_depth);
    Ok(if d == -1 {
        Value::Bool(false)
    } else {
        Value::Int(d)
    })
}

// ========================================================================
// MultipleIterator
// ========================================================================

/// The attached iterators and their association keys. php keeps them in an
/// `SplObjectStorage`, which is also what it dumps (see the module header).
#[derive(Clone, Default)]
struct Multi {
    flags: i64,
    items: Vec<(Object, Value)>,
}

/// `clone $multipleIterator` keeps the attachments.
fn multi_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let copy = with_state::<Multi, _>(src, |s| s.clone());
    with_state::<Multi, _>(dst, |s| *s = copy);
    Ok(())
}

/// `MultipleIterator::__construct(int $flags = MIT_NEED_ALL|MIT_KEYS_NUMERIC)`
fn multi_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let flags = match args.first() {
        Some(v) => int_arg(ctx, "MultipleIterator::__construct", 1, "flags", v)?,
        None => MIT_NEED_ALL | MIT_KEYS_NUMERIC,
    };
    with_state::<Multi, _>(o, |s| {
        s.flags = flags;
        s.items.clear();
    });
    Ok(Value::Null)
}

/// `MultipleIterator::getFlags(): int`
fn multi_get_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Multi, _>(o, |s| s.flags)))
}

/// `MultipleIterator::setFlags(int $flags): void`
fn multi_set_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let flags = int_arg(ctx, "MultipleIterator::setFlags", 1, "flags", &args[0])?;
    with_state::<Multi, _>(o, |s| s.flags = flags);
    Ok(Value::Null)
}

/// `MultipleIterator::attachIterator(Iterator $iterator, string|int|null $info = null): void`
fn multi_attach(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "MultipleIterator::attachIterator";
    let it = object_arg(ctx, who, 1, "iterator", "Iterator", &args[0])?;
    let info = match args.get(1) {
        None => Value::Null,
        Some(v) => match &*v.deref() {
            Value::Null | Value::Uninit => Value::Null,
            Value::Str(s) => Value::Str(s.clone()),
            Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => {
                return Err(Unwind::type_error(format!(
                    "{who}(): Argument #2 ($info) must be of type string|int|null, {} given",
                    rphp_runtime::value_name(&v.deref())
                )))
            }
            other => Value::Int(int_arg(ctx, who, 2, "info", other)?),
        },
    };
    let duplicate = with_state::<Multi, _>(o, |s| {
        s.items
            .iter()
            .any(|(obj, i)| !obj.ptr_eq(&it) && values_identical(i, &info))
    });
    if !matches!(info, Value::Null) && duplicate {
        return Err(Unwind::exception(
            "InvalidArgumentException",
            "Key duplication error",
        ));
    }
    with_state::<Multi, _>(o, |s| {
        // Re-attaching an iterator already in the set only re-labels it.
        let at = s.items.iter().position(|(x, _)| x.ptr_eq(&it));
        match at {
            Some(i) => s.items[i].1 = info,
            None => s.items.push((it, info)),
        }
    });
    Ok(Value::Null)
}

/// `===` over the scalar association keys php allows.
fn values_identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x.as_bytes() == y.as_bytes(),
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// `MultipleIterator::detachIterator(Iterator $iterator): void`
fn multi_detach(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let it = object_arg(
        ctx,
        "MultipleIterator::detachIterator",
        1,
        "iterator",
        "Iterator",
        &args[0],
    )?;
    with_state::<Multi, _>(o, |s| s.items.retain(|(x, _)| !x.ptr_eq(&it)));
    Ok(Value::Null)
}

/// `MultipleIterator::containsIterator(Iterator $iterator): bool`
fn multi_contains(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let it = object_arg(
        ctx,
        "MultipleIterator::containsIterator",
        1,
        "iterator",
        "Iterator",
        &args[0],
    )?;
    Ok(Value::Bool(with_state::<Multi, _>(o, |s| {
        s.items.iter().any(|(x, _)| x.ptr_eq(&it))
    })))
}

/// `MultipleIterator::countIterators(): int`
fn multi_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state::<Multi, _>(o, |s| s.items.len() as i64)))
}

/// The attached iterators, in attachment order.
fn multi_items(o: &Object) -> Vec<(Object, Value)> {
    with_state::<Multi, _>(o, |s| s.items.clone())
}

/// `MultipleIterator::rewind(): void`
fn multi_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    for (it, _) in multi_items(o) {
        ctx.iter_call(&it, IterRole::Rewind)?;
    }
    Ok(Value::Null)
}

/// `MultipleIterator::next(): void`
fn multi_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    for (it, _) in multi_items(o) {
        ctx.iter_call(&it, IterRole::Next)?;
    }
    Ok(Value::Null)
}

/// `MultipleIterator::valid(): bool` — every attached iterator under
/// `MIT_NEED_ALL`, any one under `MIT_NEED_ANY`; never with no attachments.
fn multi_valid(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (flags, items) = (with_state::<Multi, _>(o, |s| s.flags), multi_items(o));
    if items.is_empty() {
        return Ok(Value::Bool(false));
    }
    let need_all = flags & MIT_NEED_ALL != 0;
    for (it, _) in items {
        let v = ctx.iter_call(&it, IterRole::Valid)?.to_bool();
        if need_all && !v {
            return Ok(Value::Bool(false));
        }
        if !need_all && v {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(need_all))
}

/// The shared body of `current()` and `key()`: one entry per attached
/// iterator, keyed by position or by the association key.
fn multi_gather(ctx: &mut Ctx, o: &Object, method: &[u8], what: &str) -> NativeResult {
    let (flags, items) = (with_state::<Multi, _>(o, |s| s.flags), multi_items(o));
    if items.is_empty() {
        return Err(Unwind::exception(
            "RuntimeException",
            format!("Called {what}() on an invalid iterator"),
        ));
    }
    let need_all = flags & MIT_NEED_ALL != 0;
    let assoc = flags & MIT_KEYS_ASSOC != 0;
    let mut out = Array::new();
    for (n, (it, info)) in items.into_iter().enumerate() {
        let value = if ctx.iter_call(&it, IterRole::Valid)?.to_bool() {
            ctx.call_method(&it, method, &[])?.deref().into_owned()
        } else if need_all {
            return Err(Unwind::exception(
                "RuntimeException",
                format!("Called {what}() with non valid sub iterator"),
            ));
        } else {
            Value::Null
        };
        if assoc {
            // An attachment with no association key has nothing to be keyed
            // by; php only notices when the keys are actually wanted.
            let key = match &info {
                Value::Null | Value::Uninit => None,
                other => array_key(other),
            };
            let key = key.ok_or_else(|| {
                Unwind::exception(
                    "InvalidArgumentException",
                    "Sub-Iterator is associated with NULL",
                )
            })?;
            out.set(key, value);
        } else {
            out.set(ArrayKey::Int(n as i64), value);
        }
    }
    Ok(Value::Array(out))
}

/// `MultipleIterator::current(): array`
fn multi_current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    multi_gather(ctx, o, b"current", "current")
}

/// `MultipleIterator::key(): array`
fn multi_key(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    multi_gather(ctx, o, b"key", "key")
}

/// `MultipleIterator::__debugInfo(): array` — the standard properties,
/// then the backing `SplObjectStorage` under its mangled private name,
/// each attachment as `['obj' => …, 'inf' => …]`.
fn multi_debug_info(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut storage = Array::new();
    for (it, info) in multi_items(o) {
        let mut entry = Array::new();
        entry.set(ArrayKey::str(b"obj"), Value::Object(it));
        entry.set(ArrayKey::str(b"inf"), info);
        storage.push(Value::Array(entry));
    }
    let mut out = ctx.std_property_table(o);
    out.set(mangled(b"SplObjectStorage", b"storage"), Value::Array(storage));
    Ok(Value::Array(out))
}

// ========================================================================
// registration
// ========================================================================

/// Whether `name` still needs declaring.
fn needs_declaring(r: &mut Registry, name: &[u8]) -> bool {
    r.0.class_by_name(name).is_none()
}

/// A zero-argument signature for an interface method.
fn sig(min: u8, max: Option<u8>, params: &'static [&'static str]) -> rphp_runtime::NativeMethod {
    rphp_runtime::NativeMethod {
        handler: abstract_body,
        min_args: min,
        max_args: max,
        params,
        by_ref: 0,
        is_static: false,
        is_final: false,
    }
}

/// Register SPL's iterator decorators.
///
/// Must run after `spl_interfaces` (`Iterator`, `Traversable`,
/// `SeekableIterator`, `Countable`, `ArrayAccess`), `zend_exceptions`
/// (`Stringable`), `spl_exceptions` (the `LogicException` /
/// `RuntimeException` trees these throw from) and `spl_containers`
/// (`ArrayIterator`, which `RecursiveArrayIterator` extends and
/// `AppendIterator` uses).
pub(crate) fn register_classes(r: &mut Registry) {
    if r.0.class_by_name(b"IteratorIterator").is_some() {
        return;
    }

    // ---- interfaces ----------------------------------------------------
    if needs_declaring(r, b"OuterIterator") {
        r.interface("OuterIterator")
            .implements(&["Iterator"])
            .abstract_method("getInnerIterator", sig(0, Some(0), &[]))
            .finish();
    }
    if needs_declaring(r, b"RecursiveIterator") {
        r.interface("RecursiveIterator")
            .implements(&["Iterator"])
            .abstract_method("hasChildren", sig(0, Some(0), &[]))
            .abstract_method("getChildren", sig(0, Some(0), &[]))
            .finish();
    }

    // ---- IteratorIterator and its plain decorators ---------------------
    r.class("IteratorIterator")
        .implements(&["OuterIterator"])
        .payload_clone(dual_clone)
        .native_iter(NativeIter { rewind: ii_rewind, valid: ii_valid, current: ii_current, key: ii_key, next: ii_next })
        .method("__construct", nm!(1, Some(2), ii_construct))
        .method("getInnerIterator", nm!(0, Some(0), ii_get_inner))
        .method("rewind", nm!(0, Some(0), ii_rewind))
        .method("valid", nm!(0, Some(0), ii_valid))
        .method("key", nm!(0, Some(0), ii_key))
        .method("current", nm!(0, Some(0), ii_current))
        .method("next", nm!(0, Some(0), ii_next))
        .finish();

    r.class("FilterIterator")
        .extends("IteratorIterator")
        .native_iter(NativeIter { rewind: filter_rewind, valid: ii_valid, current: ii_current, key: ii_key, next: filter_next })
        .flags(ClassFlags::ABSTRACT)
        .abstract_method("accept", sig(0, Some(0), &[]))
        .method("__construct", nm!(1, Some(1), filter_construct))
        .method("rewind", nm!(0, Some(0), filter_rewind))
        .method("next", nm!(0, Some(0), filter_next))
        .finish();

    r.class("CallbackFilterIterator")
        .extends("FilterIterator")
        .method("__construct", nm!(2, Some(2), cb_filter_construct))
        .method("accept", nm!(0, Some(0), cb_filter_accept))
        .finish();

    r.class("RecursiveFilterIterator")
        .extends("FilterIterator")
        .implements(&["RecursiveIterator"])
        .flags(ClassFlags::ABSTRACT)
        .method("__construct", nm!(1, Some(1), rec_filter_construct))
        .method("hasChildren", nm!(0, Some(0), rec_has_children))
        .method("getChildren", nm!(0, Some(0), rec_filter_get_children))
        .finish();

    r.class("RecursiveCallbackFilterIterator")
        .extends("CallbackFilterIterator")
        .implements(&["RecursiveIterator"])
        .method("__construct", nm!(2, Some(2), rec_cb_filter_construct))
        .method("hasChildren", nm!(0, Some(0), rec_has_children))
        .method("getChildren", nm!(0, Some(0), rec_cb_filter_get_children))
        .finish();

    r.class("ParentIterator")
        .extends("RecursiveFilterIterator")
        .method("__construct", nm!(1, Some(1), parent_construct))
        .method("accept", nm!(0, Some(0), parent_accept))
        .finish();

    r.class("LimitIterator")
        .extends("IteratorIterator")
        .native_iter(NativeIter { rewind: limit_rewind, valid: limit_valid, current: ii_current, key: ii_key, next: limit_next })
        .method("__construct", nm!(1, Some(3), limit_construct))
        .method("rewind", nm!(0, Some(0), limit_rewind))
        .method("valid", nm!(0, Some(0), limit_valid))
        .method("next", nm!(0, Some(0), limit_next))
        .method("seek", nm!(1, Some(1), limit_seek))
        .method("getPosition", nm!(0, Some(0), limit_get_position))
        .finish();

    r.class("CachingIterator")
        .extends("IteratorIterator")
        .implements(&["ArrayAccess", "Countable", "Stringable"])
        .native_iter(NativeIter { rewind: caching_rewind, valid: ii_valid, current: ii_current, key: ii_key, next: caching_next })
        .class_const("CALL_TOSTRING", Value::Int(CIT_CALL_TOSTRING))
        .class_const("CATCH_GET_CHILD", Value::Int(CIT_CATCH_GET_CHILD))
        .class_const("TOSTRING_USE_KEY", Value::Int(CIT_TOSTRING_USE_KEY))
        .class_const("TOSTRING_USE_CURRENT", Value::Int(CIT_TOSTRING_USE_CURRENT))
        .class_const("TOSTRING_USE_INNER", Value::Int(CIT_TOSTRING_USE_INNER))
        .class_const("FULL_CACHE", Value::Int(CIT_FULL_CACHE))
        .method("__construct", nm!(1, Some(2), caching_construct))
        .method("rewind", nm!(0, Some(0), caching_rewind))
        .method("valid", nm!(0, Some(0), ii_valid))
        .method("next", nm!(0, Some(0), caching_next))
        .method("hasNext", nm!(0, Some(0), caching_has_next))
        .method("__toString", nm!(0, Some(0), caching_to_string))
        .method("getFlags", nm!(0, Some(0), caching_get_flags))
        .method("setFlags", nm!(1, Some(1), caching_set_flags))
        .method("offsetGet", nm!(1, Some(1), caching_offset_get))
        .method("offsetSet", nm!(2, Some(2), caching_offset_set))
        .method("offsetUnset", nm!(1, Some(1), caching_offset_unset))
        .method("offsetExists", nm!(1, Some(1), caching_offset_exists))
        .method("getCache", nm!(0, Some(0), caching_get_cache))
        .method("count", nm!(0, Some(0), caching_count))
        .finish();

    r.class("RecursiveCachingIterator")
        .extends("CachingIterator")
        .implements(&["RecursiveIterator"])
        .method("__construct", nm!(1, Some(2), rec_caching_construct))
        .method("hasChildren", nm!(0, Some(0), rec_caching_has_children))
        .method("getChildren", nm!(0, Some(0), rec_caching_get_children))
        .finish();

    r.class("NoRewindIterator")
        .extends("IteratorIterator")
        .native_iter(NativeIter { rewind: norewind_rewind, valid: norewind_valid, current: norewind_current, key: norewind_key, next: norewind_next })
        .method("__construct", nm!(1, Some(1), norewind_construct))
        .method("rewind", nm!(0, Some(0), norewind_rewind))
        .method("valid", nm!(0, Some(0), norewind_valid))
        .method("key", nm!(0, Some(0), norewind_key))
        .method("current", nm!(0, Some(0), norewind_current))
        .method("next", nm!(0, Some(0), norewind_next))
        .finish();

    r.class("InfiniteIterator")
        .extends("IteratorIterator")
        .native_iter(NativeIter { rewind: ii_rewind, valid: ii_valid, current: ii_current, key: ii_key, next: infinite_next })
        .method("__construct", nm!(1, Some(1), infinite_construct))
        .method("next", nm!(0, Some(0), infinite_next))
        .finish();

    r.class("EmptyIterator")
        .implements(&["Iterator"])
        .method("current", nm!(0, Some(0), empty_current))
        .method("next", nm!(0, Some(0), empty_noop))
        .method("key", nm!(0, Some(0), empty_key))
        .method("valid", nm!(0, Some(0), empty_valid))
        .method("rewind", nm!(0, Some(0), empty_noop))
        .finish();

    r.class("AppendIterator")
        .extends("IteratorIterator")
        .native_iter(NativeIter { rewind: append_rewind, valid: ii_valid, current: append_current, key: ii_key, next: append_next })
        .method("__construct", nm!(0, Some(0), append_construct))
        .method("append", nm!(1, Some(1), append_append))
        .method("rewind", nm!(0, Some(0), append_rewind))
        .method("valid", nm!(0, Some(0), ii_valid))
        .method("current", nm!(0, Some(0), append_current))
        .method("next", nm!(0, Some(0), append_next))
        .method("getIteratorIndex", nm!(0, Some(0), append_index))
        .method("getArrayIterator", nm!(0, Some(0), append_array_iterator))
        .finish();

    r.class("RegexIterator")
        .extends("FilterIterator")
        .class_const("USE_KEY", Value::Int(REGIT_USE_KEY))
        .class_const("INVERT_MATCH", Value::Int(REGIT_INVERT_MATCH))
        .class_const("MATCH", Value::Int(REGIT_MODE_MATCH))
        .class_const("GET_MATCH", Value::Int(REGIT_MODE_GET_MATCH))
        .class_const("ALL_MATCHES", Value::Int(REGIT_MODE_ALL_MATCHES))
        .class_const("SPLIT", Value::Int(REGIT_MODE_SPLIT))
        .class_const("REPLACE", Value::Int(REGIT_MODE_REPLACE))
        // php dumps this one, so it is a real public slot.
        .prop("replacement", Visibility::Public, Value::Null)
        .method("__construct", nm!(2, Some(5), regex_construct))
        .method("accept", nm!(0, Some(0), regex_accept))
        .method("getMode", nm!(0, Some(0), regex_get_mode))
        .method("setMode", nm!(1, Some(1), regex_set_mode))
        .method("getFlags", nm!(0, Some(0), regex_get_flags))
        .method("setFlags", nm!(1, Some(1), regex_set_flags))
        .method("getRegex", nm!(0, Some(0), regex_get_regex))
        .method("getPregFlags", nm!(0, Some(0), regex_get_preg_flags))
        .method("setPregFlags", nm!(1, Some(1), regex_set_preg_flags))
        .finish();

    r.class("RecursiveRegexIterator")
        .extends("RegexIterator")
        .implements(&["RecursiveIterator"])
        .method("__construct", nm!(2, Some(5), rec_regex_construct))
        .method("accept", nm!(0, Some(0), rec_regex_accept))
        .method("hasChildren", nm!(0, Some(0), rec_has_children))
        .method("getChildren", nm!(0, Some(0), rec_regex_get_children))
        .finish();

    // ---- the recursive walkers -----------------------------------------
    r.class("RecursiveIteratorIterator")
        .implements(&["OuterIterator"])
        .class_const("LEAVES_ONLY", Value::Int(RIT_LEAVES_ONLY))
        .class_const("SELF_FIRST", Value::Int(RIT_SELF_FIRST))
        .class_const("CHILD_FIRST", Value::Int(RIT_CHILD_FIRST))
        .class_const("CATCH_GET_CHILD", Value::Int(RIT_CATCH_GET_CHILD))
        .payload_clone(rii_clone)
        .native_iter(NativeIter { rewind: rii_rewind, valid: rii_valid, current: rii_current, key: rii_key, next: rii_next })
        .method("__construct", nm!(1, Some(3), rii_construct))
        .method("rewind", nm!(0, Some(0), rii_rewind))
        .method("valid", nm!(0, Some(0), rii_valid))
        .method("key", nm!(0, Some(0), rii_key))
        .method("current", nm!(0, Some(0), rii_current))
        .method("next", nm!(0, Some(0), rii_next))
        .method("getDepth", nm!(0, Some(0), rii_get_depth))
        .method("getSubIterator", nm!(0, Some(1), rii_get_sub_iterator))
        .method("getInnerIterator", nm!(0, Some(0), rii_get_inner))
        .method("beginIteration", nm!(0, Some(0), rii_hook))
        .method("endIteration", nm!(0, Some(0), rii_hook))
        .method("callHasChildren", nm!(0, Some(0), rii_call_has_children))
        .method("callGetChildren", nm!(0, Some(0), rii_call_get_children))
        .method("beginChildren", nm!(0, Some(0), rii_hook))
        .method("endChildren", nm!(0, Some(0), rii_hook))
        .method("nextElement", nm!(0, Some(0), rii_hook))
        .method("setMaxDepth", nm!(0, Some(1), rii_set_max_depth))
        .method("getMaxDepth", nm!(0, Some(0), rii_get_max_depth))
        .finish();

    r.class("RecursiveArrayIterator")
        .extends("ArrayIterator")
        .implements(&["RecursiveIterator"])
        .class_const("CHILD_ARRAYS_ONLY", Value::Int(RAIT_CHILD_ARRAYS_ONLY))
        .method("hasChildren", nm!(0, Some(0), rai_has_children))
        .method("getChildren", nm!(0, Some(0), rai_get_children))
        .finish();

    // `MultipleIterator` is guarded on its own: `spl_containers2.rs`'s module
    // header names it too, and a second registration would silently replace
    // whichever definition linked first.
    if needs_declaring(r, b"MultipleIterator") {
        r.class("MultipleIterator")
            .implements(&["Iterator"])
            .class_const("MIT_NEED_ANY", Value::Int(MIT_NEED_ANY))
            .class_const("MIT_NEED_ALL", Value::Int(MIT_NEED_ALL))
            .class_const("MIT_KEYS_NUMERIC", Value::Int(MIT_KEYS_NUMERIC))
            .class_const("MIT_KEYS_ASSOC", Value::Int(MIT_KEYS_ASSOC))
            .payload_clone(multi_clone)
            .method("__construct", nm!(0, Some(1), multi_construct))
            .method("getFlags", nm!(0, Some(0), multi_get_flags))
            .method("setFlags", nm!(1, Some(1), multi_set_flags))
            .method("attachIterator", nm!(1, Some(2), multi_attach))
            .method("detachIterator", nm!(1, Some(1), multi_detach))
            .method("containsIterator", nm!(1, Some(1), multi_contains))
            .method("countIterators", nm!(0, Some(0), multi_count))
            .method("rewind", nm!(0, Some(0), multi_rewind))
            .method("valid", nm!(0, Some(0), multi_valid))
            .method("key", nm!(0, Some(0), multi_key))
            .method("current", nm!(0, Some(0), multi_current))
            .method("next", nm!(0, Some(0), multi_next))
            .method("__debugInfo", nm!(0, Some(0), multi_debug_info))
            .finish();
    }
}
