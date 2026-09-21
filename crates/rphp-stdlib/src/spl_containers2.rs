//! The rest of SPL's data structures and iterators (php-src
//! `ext/spl/spl_dllist.c`, `spl_fixedarray.c`, `spl_heap.c`,
//! `spl_iterators.c`): the doubly-linked-list family, `SplFixedArray`, the
//! heaps, and the iterator decorators.
//!
//! **Where the state lives.** php keeps a container's state out of the
//! property table — `(array)`, `get_object_vars()`, Reflection and `==`
//! see none of it — and shows it through `__debugInfo()` as private-looking
//! entries:
//!
//! ```text
//! object(SplStack)#1 (2) {
//!   ["flags":"SplDoublyLinkedList":private]=>  int(6)
//!   ["dllist":"SplDoublyLinkedList":private]=> array(2) { … }
//! }
//! ```
//!
//! So everything — the elements, the mode word, the iteration cursor —
//! lives in the instance's native [`Payload`], `__debugInfo()` builds that
//! table (a user subclass's standard properties first, as php's handler
//! appends to `zend_std_get_properties`), and every class registers
//! [`ClassBuilder::payload_clone`](rphp_runtime::ClassBuilder::payload_clone)
//! so `clone $c` copies it.

use std::collections::VecDeque;

use rphp_runtime::{nm, Ctx, Interp, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Payload, Value};

/// Functions this module provides. The SPL *functions* live in
/// `spl_iterators.rs` (`iterator_to_array` …) and `spl_autoload.rs`; this
/// module is classes only.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides. Every SPL constant is a *class* constant
/// (`SplDoublyLinkedList::IT_MODE_LIFO`, `RecursiveIteratorIterator::SELF_FIRST`),
/// declared with the class, so there is nothing global to add.
pub(crate) fn register_constants(_r: &mut Registry) {}

// ---- shared helpers ----------------------------------------------------

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// Run `f` on the instance's hidden state, installing `T::default()` first
/// when there is none (an instance built by
/// [`Interp::instantiate`](rphp_runtime::Interp::instantiate) — `unserialize`,
/// a cast — has no payload).
fn with_state<T: Default + 'static, R>(o: &Object, f: impl FnOnce(&mut T) -> R) -> R {
    let present = o.with_payload::<T, _>(|_| ()).is_some();
    if !present {
        o.set_payload(Payload::Native(Box::new(T::default())));
    }
    o.with_payload::<T, _>(f)
        .expect("payload installed just above")
}

/// php's weak `int` parameter coercion, with php's diagnostics: a numeric
/// string or a whole float converts, a fractional float is deprecated and
/// truncated, `null` is the 8.1 null-to-non-nullable deprecation, and
/// anything else is the `TypeError` php raises from `zend_parse_parameters`.
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

// ========================================================================
// SplDoublyLinkedList / SplStack / SplQueue  (php-src spl_dllist.c)
// ========================================================================
//
// The elements live in a deque — php's list is doubly linked, so `shift`
// and `unshift` are O(1) there, and `SplQueue::dequeue` in a loop must not
// renumber an array — the mode word beside them, in the payload.
//
// `flags` carries the two public mode bits plus one php keeps to itself:
// bit 2 (`IT_FIXED`) marks `SplStack`/`SplQueue`, whose LIFO/FIFO direction
// is frozen. That is why a fresh `SplStack` dumps `int(6)` (fixed | LIFO)
// and a fresh `SplQueue` `int(4)` (fixed | FIFO).

/// `SplDoublyLinkedList::IT_MODE_LIFO`.
const IT_MODE_LIFO: i64 = 2;
/// `SplDoublyLinkedList::IT_MODE_DELETE`.
const IT_MODE_DELETE: i64 = 1;
/// php's private "the LIFO/FIFO bit is frozen" flag (`SplStack`/`SplQueue`).
const IT_FIXED: i64 = 4;

/// A linked list's state: the elements, the mode word, and the iteration
/// cursor — negative means "before the start", which is what `prev()` off
/// the front produces (php reports `key() === -1`).
#[derive(Default)]
struct DllState {
    items: VecDeque<Value>,
    flags: i64,
    pos: i64,
}

/// Run `f` on the list's state.
fn dll<R>(o: &Object, f: impl FnOnce(&mut DllState) -> R) -> R {
    with_state::<DllState, _>(o, f)
}

/// The mode word.
fn dll_flags(o: &Object) -> i64 {
    dll(o, |s| s.flags)
}

/// The number of elements.
fn dll_len(o: &Object) -> usize {
    dll(o, |s| s.items.len())
}

/// Element `i` (a raw list position).
fn dll_at(o: &Object, i: i64) -> Option<Value> {
    if i < 0 {
        return None;
    }
    dll(o, |s| s.items.get(i as usize).cloned())
}

/// The elements as a packed array (dumps, serialization).
fn dll_array(o: &Object) -> Array {
    dll(o, |s| {
        let mut a = Array::new();
        for v in &s.items {
            a.push(v.clone());
        }
        a
    })
}

/// Whether the list iterates back to front.
fn dll_is_lifo(o: &Object) -> bool {
    dll_flags(o) & IT_MODE_LIFO != 0
}

/// Seed `flags` for a new instance: php freezes the direction of
/// `SplStack` (LIFO) and `SplQueue` (FIFO), and a user subclass of either
/// inherits that.
fn dll_init(it: &mut Interp, o: &Object) -> Result<(), Unwind> {
    let is = |it: &Interp, name: &[u8]| {
        it.class_by_name(name)
            .is_some_and(|cid| it.object_instanceof(o, cid))
    };
    let flags = if is(it, b"SplStack") {
        IT_FIXED | IT_MODE_LIFO
    } else if is(it, b"SplQueue") {
        IT_FIXED
    } else {
        0
    };
    dll(o, |s| s.flags = flags);
    Ok(())
}

/// `clone` copies the elements, the mode and the cursor along with the
/// ordinary properties.
fn dll_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let (items, flags, pos) = dll(src, |s| (s.items.clone(), s.flags, s.pos));
    dll(dst, |s| {
        s.items = items;
        s.flags = flags;
        s.pos = pos;
    });
    Ok(())
}

/// php's `Can't …` text for an operation on an empty list.
fn dll_empty(what: &str) -> Unwind {
    Unwind::exception("RuntimeException", format!("Can't {what} an empty datastructure"))
}

/// php's `OutOfRangeException` for an index outside the list.
fn dll_range(who: &str) -> Unwind {
    Unwind::exception(
        "OutOfRangeException",
        format!("SplDoublyLinkedList::{who}(): Argument #1 ($index) is out of range"),
    )
}

/// Translate a user-visible offset into a position in the backing list: a
/// LIFO list (an `SplStack`) numbers its offsets from the top.
fn dll_real_index(o: &Object, index: i64, len: usize) -> i64 {
    if dll_is_lifo(o) {
        len as i64 - 1 - index
    } else {
        index
    }
}

fn dll_push(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let v = args[0].deref().into_owned();
    dll(o, |s| s.items.push_back(v));
    Ok(Value::Null)
}

fn dll_unshift(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let v = args[0].deref().into_owned();
    dll(o, |s| s.items.push_front(v));
    Ok(Value::Null)
}

fn dll_pop(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dll(o, |s| s.items.pop_back()).ok_or_else(|| dll_empty("pop from"))
}

fn dll_shift(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dll(o, |s| s.items.pop_front()).ok_or_else(|| dll_empty("shift from"))
}

fn dll_top(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dll(o, |s| s.items.back().cloned()).ok_or_else(|| dll_empty("peek at"))
}

fn dll_bottom(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    dll(o, |s| s.items.front().cloned()).ok_or_else(|| dll_empty("peek at"))
}

fn dll_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(dll_len(o) as i64))
}

fn dll_is_empty(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(dll_len(o) == 0))
}

/// `add(int $index, mixed $value): void` — insert *before* `$index`, so
/// `$index === count()` appends. On a LIFO list the offsets are numbered from
/// the top, and `count()` still means "past the far end", which is the tail
/// of the backing list.
fn dll_add(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = int_arg(ctx, "SplDoublyLinkedList::add", 1, "index", &args[0])?;
    let value = args[1].deref().into_owned();
    let len = dll_len(o) as i64;
    if index < 0 || index > len {
        return Err(dll_range("add"));
    }
    let at = if dll_is_lifo(o) && index != len {
        len - 1 - index
    } else if dll_is_lifo(o) {
        len
    } else {
        index
    };
    dll(o, |s| s.items.insert(at as usize, value));
    Ok(Value::Null)
}

fn dll_offset_exists(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = int_arg(ctx, "SplDoublyLinkedList::offsetExists", 1, "index", &args[0])?;
    let len = dll_len(o);
    Ok(Value::Bool(index >= 0 && (index as usize) < len))
}

fn dll_offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = int_arg(ctx, "SplDoublyLinkedList::offsetGet", 1, "index", &args[0])?;
    let len = dll_len(o);
    if index < 0 || index as usize >= len {
        return Err(dll_range("offsetGet"));
    }
    let at = dll_real_index(o, index, len);
    Ok(dll_at(o, at).unwrap_or(Value::Null))
}

/// `offsetSet(?int $index, mixed $value): void` — a null index appends.
fn dll_offset_set(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let value = args[1].deref().into_owned();
    if matches!(&*args[0].deref(), Value::Null | Value::Uninit) {
        dll(o, |s| s.items.push_back(value));
        return Ok(Value::Null);
    }
    let index = int_arg(ctx, "SplDoublyLinkedList::offsetSet", 1, "index", &args[0])?;
    let len = dll_len(o);
    if index < 0 || index as usize >= len {
        return Err(dll_range("offsetSet"));
    }
    let at = dll_real_index(o, index, len) as usize;
    dll(o, |s| s.items[at] = value);
    Ok(Value::Null)
}

fn dll_offset_unset(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let index = int_arg(ctx, "SplDoublyLinkedList::offsetUnset", 1, "index", &args[0])?;
    let len = dll_len(o);
    if index < 0 || index as usize >= len {
        return Err(dll_range("offsetUnset"));
    }
    let at = dll_real_index(o, index, len) as usize;
    dll(o, |s| s.items.remove(at));
    Ok(Value::Null)
}

/// `setIteratorMode(int $mode): void` — the two public mode bits only; php
/// masks the rest away and refuses to flip a frozen direction.
fn dll_set_iterator_mode(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mode = int_arg(ctx, "SplDoublyLinkedList::setIteratorMode", 1, "mode", &args[0])?;
    let flags = dll_flags(o);
    let wanted = mode & (IT_MODE_LIFO | IT_MODE_DELETE);
    if flags & IT_FIXED != 0 && (wanted & IT_MODE_LIFO) != (flags & IT_MODE_LIFO) {
        return Err(Unwind::exception(
            "RuntimeException",
            "Iterators' LIFO/FIFO modes for SplStack/SplQueue objects are frozen",
        ));
    }
    dll(o, |s| s.flags = (flags & !(IT_MODE_LIFO | IT_MODE_DELETE)) | wanted);
    Ok(Value::Null)
}

fn dll_get_iterator_mode(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(dll_flags(o)))
}

fn dll_rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let lifo = dll_is_lifo(o);
    dll(o, |s| s.pos = if lifo { s.items.len() as i64 - 1 } else { 0 });
    Ok(Value::Null)
}

fn dll_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(dll(o, |s| s.pos >= 0 && s.pos < s.items.len() as i64)))
}

fn dll_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let pos = dll(o, |s| s.pos);
    Ok(dll_at(o, pos).unwrap_or(Value::Null))
}

fn dll_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(dll(o, |s| s.pos)))
}

/// `next(): void` — forwards, or backwards on a LIFO list. In `IT_MODE_DELETE`
/// the element just visited is consumed instead, so the cursor stays put
/// (FIFO) or follows the shrinking tail (LIFO).
fn dll_next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let flags = dll_flags(o);
    let lifo = flags & IT_MODE_LIFO != 0;
    if flags & IT_MODE_DELETE != 0 {
        dll(o, |s| {
            if lifo {
                s.items.pop_back();
            } else {
                s.items.pop_front();
            }
            s.pos = if lifo { s.items.len() as i64 - 1 } else { 0 };
        });
        return Ok(Value::Null);
    }
    dll(o, |s| s.pos += if lifo { -1 } else { 1 });
    Ok(Value::Null)
}

/// `prev(): void` — the mirror of `next()`; off the front php leaves the
/// cursor at `-1`, where `valid()` is false and `key()` still answers.
fn dll_prev(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let lifo = dll_is_lifo(o);
    dll(o, |s| s.pos += if lifo { 1 } else { -1 });
    Ok(Value::Null)
}

/// `__serialize(): array` — php's `[flags, elements, properties]` triple,
/// the properties under their mangled keys.
fn dll_magic_serialize(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut out = Array::new();
    out.push(Value::Int(dll_flags(o)));
    out.push(Value::Array(dll_array(o)));
    out.push(Value::Array(ctx.std_property_table(o)));
    Ok(Value::Array(out))
}

fn dll_magic_unserialize(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let data = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "SplDoublyLinkedList::__unserialize(): Argument #1 ($data) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    if let Some(f) = data.get_deref(&ArrayKey::Int(0)) {
        let flags = f.to_int();
        dll(o, |s| s.flags = flags);
    }
    let items: VecDeque<Value> = match data.get_deref(&ArrayKey::Int(1)) {
        Some(Value::Array(a)) => a.values().map(|v| v.deref().into_owned()).collect(),
        _ => VecDeque::new(),
    };
    dll(o, |s| s.items = items);
    if let Some(Value::Array(props)) = data.get_deref(&ArrayKey::Int(2)) {
        for (k, v) in props.iter() {
            if let ArrayKey::Str(name) = k {
                o.set(unmangled_name(name), v.deref().into_owned());
            }
        }
    }
    Ok(Value::Null)
}

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

/// `serialize(): string` — the legacy `Serializable` form: the flags,
/// then every element, each `serialize()`d and joined with `:`.
fn dll_serialize(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let id = ctx
        .native_by_name(b"serialize")
        .expect("serialize is registered");
    let mut out: Vec<u8> = Vec::new();
    let mut argv = [Value::Int(dll_flags(o))];
    out.extend_from_slice(&ctx.call_native(id, &mut argv)?.to_php_bytes());
    for v in dll(o, |s| s.items.iter().cloned().collect::<Vec<_>>()) {
        out.push(b':');
        let mut argv = [v];
        out.extend_from_slice(&ctx.call_native(id, &mut argv)?.to_php_bytes());
    }
    Ok(Value::string(&out))
}

/// `unserialize(string $data): void` — the inverse of [`dll_serialize`].
/// The payload is a run of serialized values separated by `:`, so it is
/// split on the *structural* boundaries ([`serialized_len`]) rather than on
/// every colon, which also occurs inside a serialized string.
fn dll_unserialize(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let data = args[0].to_php_bytes();
    let id = ctx
        .native_by_name(b"unserialize")
        .expect("unserialize is registered");
    let mut cur = 0usize;
    let mut first = true;
    let mut items: Vec<Value> = Vec::new();
    while cur < data.len() {
        let Some(n) = serialized_len(&data[cur..]) else {
            return Err(Unwind::exception(
                "UnexpectedValueException",
                "Error at offset 0 of ".to_string() + &data.len().to_string() + " bytes",
            ));
        };
        let mut argv = [Value::string(&data[cur..cur + n])];
        let v = ctx.call_native(id, &mut argv)?;
        if first {
            let flags = v.to_int();
            dll(o, |s| s.flags = flags);
            first = false;
        } else {
            items.push(v);
        }
        cur += n;
        if cur < data.len() && data[cur] == b':' {
            cur += 1;
        }
    }
    dll(o, |s| s.items = items.into());
    Ok(Value::Null)
}

/// The length of the one serialized value at the front of `s`, or `None`
/// when it is not well formed. Only the structure is parsed — enough to find
/// where a value ends — because the real decoding is `unserialize()`'s job.
fn serialized_len(s: &[u8]) -> Option<usize> {
    /// The offset just past the next `needle` at or after `from`.
    fn after(s: &[u8], from: usize, needle: u8) -> Option<usize> {
        s[from..].iter().position(|b| *b == needle).map(|i| from + i + 1)
    }
    /// The decimal integer starting at `from`, and the offset just past it.
    fn number(s: &[u8], from: usize) -> Option<(i64, usize)> {
        let mut i = from;
        if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
            i += 1;
        }
        let start = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        std::str::from_utf8(&s[from..i]).ok()?.parse().ok().map(|n| (n, i))
    }
    match *s.first()? {
        b'N' => Some(2),
        b'b' | b'i' | b'd' | b'r' | b'R' => after(s, 0, b';'),
        b's' => {
            let (len, i) = number(s, 2)?;
            let start = i + 2; // `:"`
            Some(start + len.max(0) as usize + 2) // `";`
        }
        b'E' => {
            let (len, i) = number(s, 2)?;
            Some(i + 2 + len.max(0) as usize + 2)
        }
        b'a' => {
            let (n, i) = number(s, 2)?;
            let mut cur = i + 2; // `:{`
            for _ in 0..n.max(0) * 2 {
                cur += serialized_len(s.get(cur..)?)?;
            }
            Some(cur + 1) // `}`
        }
        b'O' => {
            let (nlen, i) = number(s, 2)?;
            let mut cur = i + 2 + nlen.max(0) as usize + 2; // `:"Name":`
            let (n, j) = number(s, cur)?;
            cur = j + 2; // `:{`
            for _ in 0..n.max(0) * 2 {
                cur += serialized_len(s.get(cur..)?)?;
            }
            Some(cur + 1)
        }
        b'C' => {
            let (nlen, i) = number(s, 2)?;
            let cur = i + 2 + nlen.max(0) as usize + 2;
            let (n, j) = number(s, cur)?;
            Some(j + 2 + n.max(0) as usize + 1)
        }
        _ => None,
    }
}

/// `__debugInfo(): array` — the standard properties, then the mode word
/// and the elements under `SplDoublyLinkedList`'s mangled private names,
/// whatever the receiver's class.
fn dll_debug_info(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut out = ctx.std_property_table(o);
    out.set(mangled(b"SplDoublyLinkedList", b"flags"), Value::Int(dll_flags(o)));
    out.set(mangled(b"SplDoublyLinkedList", b"dllist"), Value::Array(dll_array(o)));
    Ok(Value::Array(out))
}

/// php's mangled private-property key, `"\0Class\0prop"`.
fn mangled(class: &[u8], prop: &[u8]) -> ArrayKey {
    let mut name = vec![0u8];
    name.extend_from_slice(class);
    name.push(0);
    name.extend_from_slice(prop);
    ArrayKey::Str(name.into())
}

/// `SplQueue::enqueue(mixed $value): void`
fn queue_enqueue(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    dll_push(ctx, o, args)
}

/// `SplQueue::dequeue(): mixed`
fn queue_dequeue(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    dll_shift(ctx, o, args)
}

// ---- registration ----------------------------------------------------------

/// Register the doubly-linked-list family. `SplStack` and `SplQueue` are
/// ordinary subclasses that differ only by the frozen direction bit their
/// `native_init` seeds, plus the two aliases php gives `SplQueue`.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.0.class_by_name(b"SplDoublyLinkedList").is_some() {
        return;
    }
    let ifaces: &[&str] = &["Iterator", "Countable", "ArrayAccess"];
    r.class("SplDoublyLinkedList")
        .implements(ifaces)
        .class_const("IT_MODE_LIFO", Value::Int(IT_MODE_LIFO))
        .class_const("IT_MODE_FIFO", Value::Int(0))
        .class_const("IT_MODE_DELETE", Value::Int(IT_MODE_DELETE))
        .class_const("IT_MODE_KEEP", Value::Int(0))
        .native_init(dll_init)
        .payload_clone(dll_clone)
        .method("push", nm!(1, Some(1), dll_push))
        .method("pop", nm!(0, Some(0), dll_pop))
        .method("shift", nm!(0, Some(0), dll_shift))
        .method("unshift", nm!(1, Some(1), dll_unshift))
        .method("top", nm!(0, Some(0), dll_top))
        .method("bottom", nm!(0, Some(0), dll_bottom))
        .method("count", nm!(0, Some(0), dll_count))
        .method("isEmpty", nm!(0, Some(0), dll_is_empty))
        .method("add", nm!(2, Some(2), dll_add))
        .method("offsetExists", nm!(1, Some(1), dll_offset_exists))
        .method("offsetGet", nm!(1, Some(1), dll_offset_get))
        .method("offsetSet", nm!(2, Some(2), dll_offset_set))
        .method("offsetUnset", nm!(1, Some(1), dll_offset_unset))
        .method("setIteratorMode", nm!(1, Some(1), dll_set_iterator_mode))
        .method("getIteratorMode", nm!(0, Some(0), dll_get_iterator_mode))
        .method("rewind", nm!(0, Some(0), dll_rewind))
        .method("valid", nm!(0, Some(0), dll_valid))
        .method("current", nm!(0, Some(0), dll_current))
        .method("key", nm!(0, Some(0), dll_key))
        .method("next", nm!(0, Some(0), dll_next))
        .method("prev", nm!(0, Some(0), dll_prev))
        .method("__serialize", nm!(0, Some(0), dll_magic_serialize))
        .method("__unserialize", nm!(1, Some(1), dll_magic_unserialize))
        .method("serialize", nm!(0, Some(0), dll_serialize))
        .method("unserialize", nm!(1, Some(1), dll_unserialize))
        .method("__debugInfo", nm!(0, Some(0), dll_debug_info))
        .finish();

    // `SplStack` iterates back to front, `SplQueue` front to back; both
    // freeze that direction (`dll_init` seeds the bit).
    r.class("SplStack").extends("SplDoublyLinkedList").finish();
    r.class("SplQueue")
        .extends("SplDoublyLinkedList")
        .method("enqueue", nm!(1, Some(1), queue_enqueue))
        .method("dequeue", nm!(0, Some(0), queue_dequeue))
        .finish();
}
