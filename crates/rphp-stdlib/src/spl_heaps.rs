//! SPL's binary heaps (php-src `ext/spl/spl_heap.c`): the abstract `SplHeap`
//! with `SplMinHeap` / `SplMaxHeap`, and the `SplPriorityQueue` that repeats
//! the same machinery over `data`/`priority` pairs.
//!
//! **Where the state lives.** php exposes all three parts of a heap's state
//! through private properties, and `var_dump` / `print_r` show them, so they
//! are real private slots here and dump byte for byte:
//!
//! ```text
//! object(SplMinHeap)#1 (3) {
//!   ["flags":"SplHeap":private]=>       int(0)
//!   ["isCorrupted":"SplHeap":private]=> bool(false)
//!   ["heap":"SplHeap":private]=>        array(2) { [0]=> int(1) [1]=> int(5) }
//! }
//! ```
//!
//! `SplPriorityQueue` is **not** a subclass of `SplHeap` — it declares its
//! own three slots, so its dump says `"SplPriorityQueue":private`, its
//! `flags` start at `EXTR_DATA` rather than `0`, and its elements are
//! `["data" => …, "priority" => …]` arrays.
//!
//! **The ordering is observable.** php's is a sift-up/sift-down binary heap
//! over one flat array, and the array itself is printed, so the algorithm
//! below reproduces php's exactly — including two details that a textbook
//! version gets wrong: `delete_top` bounds its descent with a `limit`
//! computed from the count *before* the removal, and its "is there a right
//! child" test reads the slot that just held the removed bottom element.
//! Both were pinned against php 8.5 with a differential simulation over
//! random insert/extract sequences that compared the dumped array, the
//! extracted value *and the number of `compare()` calls* at every step.
//!
//! **Corruption.** `compare()` is an ordinary method call, so a user
//! subclass can throw from it. php then latches `isCorrupted`, treats that
//! comparison and every later one in the same operation as `0` (it never
//! calls `compare` again), lets the sift finish — so the element really is
//! stored, and `count()` really does grow — and rethrows. `insert`,
//! `extract` and `top` refuse to run on a corrupted heap; `count`, `isEmpty`,
//! the iterator methods and `next()` do not check.
//!
//! **Known divergences.**
//!
//! * `(array)` of a heap yields the mangled private keys here and `[]` in
//!   php, which answers `get_properties_for(ARRAY_CAST)` with nothing and
//!   only fills the debug view.
//! * `SplHeap::compare` is `abstract protected` in php but `abstract public`
//!   here: [`ClassBuilder::abstract_method`](rphp_runtime::ClassBuilder::abstract_method)
//!   takes no visibility. Overriding it with either visibility links, but
//!   `$heap->compare(1, 2)` from outside the class answers instead of
//!   raising php's `Error: Call to protected method …`.
//! * `__serialize()`'s property bag names a subclass's non-public properties
//!   plainly (`"b"`) where php mangles them (`"\0*\0b"`), because the object
//!   model keys properties by their declared name. The round trip through
//!   this implementation is faithful; the *bytes* differ from php's.
//! * A dump of a user subclass that declares properties of its own lists
//!   them *after* the three slots here and *before* them in php: php's are
//!   not real properties at all, so its debug handler appends them to the
//!   declared set, while this layout is parent-first like any other class.

use rphp_runtime::{
    nm, ClassFlags, Ctx, NativeFn, NativeMethod, NativeResult, Registry, Unwind, Visibility,
};
use rphp_value::{Array, ArrayKey, Object, Value, Vis};

/// Functions this module provides: none. The heaps are classes, and every
/// SPL *function* lives in `spl_iterators.rs` / `spl_autoload.rs`.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides: none. `SplPriorityQueue::EXTR_*` are
/// *class* constants, declared with the class below.
pub(crate) fn register_constants(_r: &mut Registry) {}

/// `SplPriorityQueue::EXTR_DATA` — extraction yields the value.
const EXTR_DATA: i64 = 1;
/// `SplPriorityQueue::EXTR_PRIORITY` — extraction yields the priority.
const EXTR_PRIORITY: i64 = 2;
/// `SplPriorityQueue::EXTR_BOTH` — extraction yields both, as an array.
const EXTR_BOTH: i64 = 3;

/// Which family an instance belongs to. The two differ in what a heap
/// element *is*, and therefore in what `compare()` is asked about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `SplHeap`: the element is the comparand.
    Heap,
    /// `SplPriorityQueue`: the element is a `data`/`priority` pair and the
    /// priority is the comparand.
    Queue,
}

impl Kind {
    /// The class that declares the three private slots — the one php names
    /// in a dump and in `__debugInfo`'s mangled keys, whatever the receiver.
    fn owner(self) -> &'static [u8] {
        match self {
            Kind::Heap => b"SplHeap",
            Kind::Queue => b"SplPriorityQueue",
        }
    }
}

// ---- shared helpers ----------------------------------------------------

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// An instance-method signature with no body, for `SplHeap::compare`.
fn sig(min: u8, max: Option<u8>, params: &'static [&'static str]) -> NativeMethod {
    NativeMethod {
        handler: abstract_compare,
        min_args: min,
        max_args: max,
        params,
        by_ref: 0,
        is_static: false,
        is_final: false,
    }
}

/// The body of `SplHeap::compare`. Dispatch refuses an abstract method
/// before a body could run, so reaching this is an engine bug.
fn abstract_compare(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot call abstract method SplHeap::compare()"))
}

/// A declared array property as an owned [`Array`].
fn arr_prop(o: &Object, name: &[u8]) -> Array {
    match o.get_deref(name) {
        Some(Value::Array(a)) => a,
        _ => Array::new(),
    }
}

/// The values of a declared list property, in order.
fn list_prop(o: &Object, name: &[u8]) -> Vec<Value> {
    arr_prop(o, name)
        .values()
        .map(|v| v.deref().into_owned())
        .collect()
}

/// A declared int property.
fn int_prop(o: &Object, name: &[u8]) -> i64 {
    o.get_deref(name).map_or(0, |v| v.to_int())
}

/// php's mangled private-property key, `"\0Class\0prop"`.
fn mangled(class: &[u8], prop: &[u8]) -> ArrayKey {
    let mut name = vec![0u8];
    name.extend_from_slice(class);
    name.push(0);
    name.extend_from_slice(prop);
    ArrayKey::Str(name.into_boxed_slice())
}

/// php's weak `int` parameter coercion with php's diagnostics, for
/// `setExtractFlags(int $flags)`.
fn int_arg(ctx: &mut Ctx, who: &str, name: &str, v: &Value) -> Result<i64, Unwind> {
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
                "{who}(): Passing null to parameter #1 (${name}) of type int is deprecated"
            ))?;
            Ok(0)
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(s.to_int()),
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #1 (${name}) must be of type int, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// php's `Can't …` text for an operation on an empty heap.
fn empty_heap(what: &str) -> Unwind {
    Unwind::exception("RuntimeException", format!("Can't {what} an empty heap"))
}

/// Refuse the operations php refuses once a `compare()` has thrown.
fn guard_corrupted(o: &Object) -> Result<(), Unwind> {
    if o.get_deref(b"isCorrupted").is_some_and(|v| v.to_bool()) {
        return Err(Unwind::exception(
            "RuntimeException",
            "Heap is corrupted, heap properties are no longer ensured.",
        ));
    }
    Ok(())
}

// ---- the comparator ----------------------------------------------------

/// One heap operation's view of `compare()`: an ordinary virtual method
/// call, so a user subclass takes over, plus php's corruption latch — after
/// the first throw the comparator answers `0` without calling again, and the
/// operation finishes on that basis before the exception is rethrown.
struct Cmp {
    kind: Kind,
    fault: Option<Unwind>,
}

impl Cmp {
    fn new(kind: Kind) -> Cmp {
        Cmp { kind, fault: None }
    }

    /// What `compare()` is asked about: the element itself, or its priority.
    fn comparand(&self, v: &Value) -> Value {
        match self.kind {
            Kind::Heap => v.clone(),
            Kind::Queue => match &*v.deref() {
                Value::Array(a) => a
                    .get_deref(&ArrayKey::str(b"priority"))
                    .unwrap_or(Value::Null),
                _ => Value::Null,
            },
        }
    }

    /// `compare($a, $b)`, as php reads it: the sign of the returned `int`.
    fn call(&mut self, ctx: &mut Ctx, o: &Object, a: &Value, b: &Value) -> i64 {
        if self.fault.is_some() {
            return 0;
        }
        let args = [self.comparand(a), self.comparand(b)];
        match ctx.call_method(o, b"compare", &args) {
            Ok(v) => v.to_int(),
            Err(e) => {
                self.fault = Some(e);
                0
            }
        }
    }

    /// Latch `isCorrupted` and hand back the exception, if one was caught.
    fn finish(self, o: &Object) -> Result<(), Unwind> {
        match self.fault {
            Some(err) => {
                o.set(b"isCorrupted", Value::Bool(true));
                Err(err)
            }
            None => Ok(()),
        }
    }
}

// ---- the heap itself ---------------------------------------------------

/// Sift `elem` up from the end. php's loop moves the *parent* down while the
/// parent compares less than the new element, then drops the element into
/// the hole it left — so an element equal to its parent stays below it.
fn heap_insert(ctx: &mut Ctx, o: &Object, kind: Kind, elem: Value) -> Result<(), Unwind> {
    // The heap array is moved out and worked on in place (a clone would
    // copy it per insert); `compare()` cannot see the property meanwhile,
    // as php's heap storage is not a property either.
    let mut e = take_heap(o);
    let mut cmp = Cmp::new(kind);
    let mut i = e.len();
    e.push(Value::Null);
    let r = (|| {
        while i > 0 {
            let parent = (i - 1) / 2;
            let up = heap_at(&e, parent);
            if cmp.call(ctx, o, &up, &elem) >= 0 {
                break;
            }
            e.set(ArrayKey::Int(i as i64), up);
            i = parent;
        }
        e.set(ArrayKey::Int(i as i64), elem);
        Ok(())
    })();
    set_heap(o, e);
    r?;
    cmp.finish(o)
}

/// Element `i` of the heap array.
fn heap_at(e: &Array, i: usize) -> Value {
    e.get_deref(&ArrayKey::Int(i as i64)).unwrap_or(Value::Null)
}

/// The heap array moved out of the `heap` property (null left behind) for
/// an in-place change that [`set_heap`] puts back.
fn take_heap(o: &Object) -> Array {
    let taken = o.with_data_mut(|d| match d.get_mut(b"heap") {
        Some(Value::Ref(r)) => r.get(),
        Some(slot) => std::mem::replace(slot, Value::Null),
        None => Value::Null,
    });
    match taken {
        Value::Array(a) => a,
        _ => Array::new(),
    }
}

fn set_heap(o: &Object, a: Array) {
    o.set(b"heap", Value::Array(a));
}

/// Remove and return the root. Both callers refuse an empty heap first; the
/// guard below only keeps this total.
///
/// Two bounds carry php's exact shape. `limit` is derived from the count
/// *before* the removal, which is why a three-element heap still sifts where
/// the post-removal count would have stopped the descent at once. The
/// right-child test is `j != n` against the *new* count, so when `j` is the
/// last live index php reads `e[n]` — the slot the removed bottom element
/// still occupies. That comparison is `bottom` against a child, and if it
/// wins, `j` becomes `n` and the next comparison is `bottom` against itself:
/// `0`, which breaks the loop. The stale read is therefore harmless, but it
/// is a real `compare()` call that a counting subclass sees.
fn heap_delete_top(ctx: &mut Ctx, o: &Object, kind: Kind) -> Result<Value, Unwind> {
    let mut e = take_heap(o);
    let n0 = e.len();
    if n0 == 0 {
        set_heap(o, e);
        return Ok(Value::Null);
    }
    let limit = (n0 - 1) / 2;
    let n = n0 - 1;
    let top = heap_at(&e, 0);
    let bottom = heap_at(&e, n);
    let mut cmp = Cmp::new(kind);
    let mut i = 0usize;
    let r = (|| {
        while i < limit {
            let mut j = i * 2 + 1;
            if j != n {
                let (left, right) = (heap_at(&e, j), heap_at(&e, j + 1));
                if cmp.call(ctx, o, &right, &left) > 0 {
                    j += 1;
                }
            }
            let child = heap_at(&e, j);
            if cmp.call(ctx, o, &bottom, &child) >= 0 {
                break;
            }
            e.set(ArrayKey::Int(i as i64), child);
            i = j;
        }
        Ok(())
    })();
    e.pop();
    // `i == n` means the descent walked onto the removed slot; php writes
    // there too, past the live range, where nothing can observe it.
    if i < n {
        e.set(ArrayKey::Int(i as i64), bottom);
    }
    set_heap(o, e);
    r?;
    cmp.finish(o)?;
    Ok(top)
}

/// What an extraction hands back: the element for a heap, and for a queue
/// whichever half `flags` selects.
fn shaped(o: &Object, kind: Kind, v: Value) -> Value {
    if kind == Kind::Heap {
        return v;
    }
    let flags = int_prop(o, b"flags");
    if flags & EXTR_BOTH == EXTR_BOTH {
        return v;
    }
    let member: &[u8] = if flags & EXTR_DATA != 0 {
        b"data"
    } else {
        b"priority"
    };
    match &v {
        Value::Array(pair) => pair
            .get_deref(&ArrayKey::str(member))
            .unwrap_or(Value::Null),
        _ => v.clone(),
    }
}

// ---- the methods, shared by both families ------------------------------

/// `insert(mixed $value): true` (`SplHeap`).
fn heap_insert_method(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    guard_corrupted(o)?;
    heap_insert(ctx, o, Kind::Heap, args[0].deref().into_owned())?;
    Ok(Value::Bool(true))
}

/// `insert(mixed $value, mixed $priority): true` (`SplPriorityQueue`) — the
/// pair php stores, and dumps, as `["data" => …, "priority" => …]`.
fn queue_insert_method(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    guard_corrupted(o)?;
    let mut pair = Array::new();
    pair.set(ArrayKey::str(b"data"), args[0].deref().into_owned());
    pair.set(ArrayKey::str(b"priority"), args[1].deref().into_owned());
    heap_insert(ctx, o, Kind::Queue, Value::Array(pair))?;
    Ok(Value::Bool(true))
}

/// `extract(): mixed` for either family.
fn extract(ctx: &mut Ctx, o: Option<&Object>, kind: Kind) -> NativeResult {
    let o = this(o)?;
    guard_corrupted(o)?;
    if arr_prop(o, b"heap").is_empty() {
        return Err(empty_heap("extract from"));
    }
    let top = heap_delete_top(ctx, o, kind)?;
    Ok(shaped(o, kind, top))
}

fn heap_extract(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    extract(ctx, o, Kind::Heap)
}

fn queue_extract(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    extract(ctx, o, Kind::Queue)
}

/// `top(): mixed` — the root, left in place. php checks corruption before
/// emptiness, so a corrupted *empty* heap still reports the corruption.
fn top(o: Option<&Object>, kind: Kind) -> NativeResult {
    let o = this(o)?;
    guard_corrupted(o)?;
    let first = list_prop(o, b"heap")
        .first()
        .cloned()
        .ok_or_else(|| empty_heap("peek at"))?;
    Ok(shaped(o, kind, first))
}

fn heap_top(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    top(o, Kind::Heap)
}

fn queue_top(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    top(o, Kind::Queue)
}

/// `count(): int` — answers even when the heap is corrupted.
fn count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(arr_prop(o, b"heap").len() as i64))
}

/// `isEmpty(): bool`
fn is_empty(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(arr_prop(o, b"heap").is_empty()))
}

/// `isCorrupted(): bool`
fn is_corrupted(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(
        o.get_deref(b"isCorrupted").is_some_and(|v| v.to_bool()),
    ))
}

/// `recoverFromCorruption(): true` — clears the latch. The heap itself is
/// left exactly as the failed operation left it; php makes no attempt to
/// restore the invariant, which is what "recover" means here.
fn recover_from_corruption(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    o.set(b"isCorrupted", Value::Bool(false));
    Ok(Value::Bool(true))
}

/// `rewind(): void` — a heap has nothing to rewind to; iterating it consumes
/// it, and php's `rewind` is a no-op.
fn rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Null)
}

/// `valid(): bool`
fn valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(!arr_prop(o, b"heap").is_empty()))
}

/// `key(): int` — php numbers the remaining elements downwards, so the key
/// is `count() - 1` and an exhausted heap answers `-1`.
fn key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(arr_prop(o, b"heap").len() as i64 - 1))
}

/// `current(): mixed` — the root, or `null` when exhausted. Unlike `top()`
/// this does not check corruption.
fn current(o: Option<&Object>, kind: Kind) -> NativeResult {
    let o = this(o)?;
    Ok(match list_prop(o, b"heap").first().cloned() {
        Some(v) => shaped(o, kind, v),
        None => Value::Null,
    })
}

fn heap_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    current(o, Kind::Heap)
}

fn queue_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    current(o, Kind::Queue)
}

/// `next(): void` — advancing *consumes* the root. php does not check the
/// corruption latch here, so a corrupted heap can still be drained.
fn next(ctx: &mut Ctx, o: Option<&Object>, kind: Kind) -> NativeResult {
    let o = this(o)?;
    if !arr_prop(o, b"heap").is_empty() {
        heap_delete_top(ctx, o, kind)?;
    }
    Ok(Value::Null)
}

fn heap_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    next(ctx, o, Kind::Heap)
}

fn queue_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    next(ctx, o, Kind::Queue)
}

/// `SplMinHeap::compare($value1, $value2): int` — reversed, so the *smallest*
/// element sorts greatest and ends up at the root.
fn min_compare(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(args[1].spaceship(&args[0])))
}

/// `SplMaxHeap::compare($value1, $value2): int`
fn max_compare(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(args[0].spaceship(&args[1])))
}

/// `SplPriorityQueue::compare($priority1, $priority2): int` — the highest
/// priority wins. Unlike `SplHeap`'s this one is public and concrete.
fn queue_compare(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(args[0].spaceship(&args[1])))
}

/// `SplPriorityQueue::setExtractFlags(int $flags): int` — php masks the word
/// down to the two meaningful bits, refuses an empty selection, and answers
/// with what it stored.
fn set_extract_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let want = int_arg(
        ctx,
        "SplPriorityQueue::setExtractFlags",
        "flags",
        &args[0],
    )? & EXTR_BOTH;
    if want == 0 {
        return Err(Unwind::exception(
            "RuntimeException",
            "Must specify at least one extract flag",
        ));
    }
    o.set(b"flags", Value::Int(want));
    Ok(Value::Int(want))
}

/// `SplPriorityQueue::getExtractFlags(): int`
fn get_extract_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(int_prop(o, b"flags")))
}

// ---- debugging and serialization ---------------------------------------

/// `__debugInfo(): array` — the three slots under their mangled private
/// names, always attributed to the class that declares them.
fn debug_info(o: Option<&Object>, kind: Kind) -> NativeResult {
    let o = this(o)?;
    let owner = kind.owner();
    let mut out = Array::new();
    out.set(mangled(owner, b"flags"), Value::Int(int_prop(o, b"flags")));
    out.set(
        mangled(owner, b"isCorrupted"),
        Value::Bool(o.get_deref(b"isCorrupted").is_some_and(|v| v.to_bool())),
    );
    out.set(mangled(owner, b"heap"), Value::Array(arr_prop(o, b"heap")));
    Ok(Value::Array(out))
}

fn heap_debug_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    debug_info(o, Kind::Heap)
}

fn queue_debug_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    debug_info(o, Kind::Queue)
}

/// Whether a property is one of the three slots php keeps out of the
/// serialized property bag (it carries them in the second element instead).
fn is_internal_slot(name: &[u8], vis: Vis) -> bool {
    vis == Vis::Private && matches!(name, b"flags" | b"isCorrupted" | b"heap")
}

/// `__serialize(): array` — php's pair of a property bag and a
/// `["flags" => …, "heap_elements" => …]` record.
fn magic_serialize(o: Option<&Object>) -> NativeResult {
    let o = this(o)?;
    let mut props = Array::new();
    for (name, value, vis) in o.props_snapshot() {
        if is_internal_slot(&name, vis) {
            continue;
        }
        props.set(ArrayKey::Str(name), value);
    }
    let mut record = Array::new();
    record.set(ArrayKey::str(b"flags"), Value::Int(int_prop(o, b"flags")));
    record.set(
        ArrayKey::str(b"heap_elements"),
        Value::Array(arr_prop(o, b"heap")),
    );
    let mut out = Array::new();
    out.push(Value::Array(props));
    out.push(Value::Array(record));
    Ok(Value::Array(out))
}

/// Both families produce the same pair, so both register this.
fn magic_serialize_method(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    magic_serialize(o)
}

/// `__unserialize(array $data): void` — the elements are *re-inserted*, not
/// copied: php heapifies them through `compare()`, so a subclass's ordering
/// is applied to whatever order the payload happened to carry.
fn magic_unserialize(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
    kind: Kind,
) -> NativeResult {
    let o = this(o)?;
    let data = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "{}::__unserialize(): Argument #1 ($data) must be of type array, {} given",
                String::from_utf8_lossy(kind.owner()),
                rphp_runtime::value_name(other)
            )))
        }
    };
    let class = ctx.class_name_of(o);
    let invalid = || {
        Unwind::exception(
            "Exception",
            format!("Invalid serialization data for {class} object"),
        )
    };
    let Some(Value::Array(props)) = data.get_deref(&ArrayKey::Int(0)) else {
        return Err(invalid());
    };
    let Some(Value::Array(record)) = data.get_deref(&ArrayKey::Int(1)) else {
        return Err(invalid());
    };
    let Some(Value::Int(flags)) = record.get_deref(&ArrayKey::str(b"flags")) else {
        return Err(invalid());
    };
    let Some(Value::Array(elements)) = record.get_deref(&ArrayKey::str(b"heap_elements")) else {
        return Err(invalid());
    };
    o.set(b"flags", Value::Int(flags));
    o.set(b"isCorrupted", Value::Bool(false));
    o.set(b"heap", Value::empty_array());
    for (k, v) in props.iter() {
        if let ArrayKey::Str(name) = k {
            o.set(name, v.deref().into_owned());
        }
    }
    for v in elements.values() {
        heap_insert(ctx, o, kind, v.deref().into_owned())?;
    }
    Ok(Value::Null)
}

fn heap_magic_unserialize(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    magic_unserialize(ctx, o, args, Kind::Heap)
}

fn queue_magic_unserialize(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    magic_unserialize(ctx, o, args, Kind::Queue)
}

// ---- registration ------------------------------------------------------

/// Register the heap family. Runs after `spl_interfaces` (`Iterator`,
/// `Countable`) and `zend_exceptions` / `spl_exceptions` (`Exception`,
/// `RuntimeException`).
pub(crate) fn register_classes(r: &mut Registry) {
    if r.0.class_by_name(b"SplHeap").is_some() {
        return;
    }

    r.class("SplHeap")
        .flags(ClassFlags::ABSTRACT)
        .implements(&["Iterator", "Countable"])
        // php dumps all three, so they are real private slots.
        .prop("flags", Visibility::Private, Value::Int(0))
        .prop("isCorrupted", Visibility::Private, Value::Bool(false))
        .prop("heap", Visibility::Private, Value::empty_array())
        .abstract_method("compare", sig(2, Some(2), &["value1", "value2"]))
        .method("insert", nm!(1, Some(1), heap_insert_method))
        .method("extract", nm!(0, Some(0), heap_extract))
        .method("top", nm!(0, Some(0), heap_top))
        .method("count", nm!(0, Some(0), count))
        .method("isEmpty", nm!(0, Some(0), is_empty))
        .method("isCorrupted", nm!(0, Some(0), is_corrupted))
        .method("recoverFromCorruption", nm!(0, Some(0), recover_from_corruption))
        .method("rewind", nm!(0, Some(0), rewind))
        .method("valid", nm!(0, Some(0), valid))
        .method("current", nm!(0, Some(0), heap_current))
        .method("key", nm!(0, Some(0), key))
        .method("next", nm!(0, Some(0), heap_next))
        .method("__serialize", nm!(0, Some(0), magic_serialize_method))
        .method("__unserialize", nm!(1, Some(1), heap_magic_unserialize))
        .method("__debugInfo", nm!(0, Some(0), heap_debug_info))
        .finish();

    // The two concrete heaps differ by their `compare` alone.
    r.class("SplMinHeap")
        .extends("SplHeap")
        .method_vis("compare", Visibility::Protected, nm!(2, Some(2), min_compare))
        .finish();
    r.class("SplMaxHeap")
        .extends("SplHeap")
        .method_vis("compare", Visibility::Protected, nm!(2, Some(2), max_compare))
        .finish();

    // `SplPriorityQueue` repeats `SplHeap`'s shape rather than inheriting it
    // — php gives it no common ancestor, and its private slots are its own.
    r.class("SplPriorityQueue")
        .implements(&["Iterator", "Countable"])
        .class_const("EXTR_DATA", Value::Int(EXTR_DATA))
        .class_const("EXTR_PRIORITY", Value::Int(EXTR_PRIORITY))
        .class_const("EXTR_BOTH", Value::Int(EXTR_BOTH))
        .prop("flags", Visibility::Private, Value::Int(EXTR_DATA))
        .prop("isCorrupted", Visibility::Private, Value::Bool(false))
        .prop("heap", Visibility::Private, Value::empty_array())
        .method("compare", nm!(2, Some(2), queue_compare))
        .method("insert", nm!(2, Some(2), queue_insert_method))
        .method("extract", nm!(0, Some(0), queue_extract))
        .method("top", nm!(0, Some(0), queue_top))
        .method("count", nm!(0, Some(0), count))
        .method("isEmpty", nm!(0, Some(0), is_empty))
        .method("isCorrupted", nm!(0, Some(0), is_corrupted))
        .method("recoverFromCorruption", nm!(0, Some(0), recover_from_corruption))
        .method("setExtractFlags", nm!(1, Some(1), set_extract_flags))
        .method("getExtractFlags", nm!(0, Some(0), get_extract_flags))
        .method("rewind", nm!(0, Some(0), rewind))
        .method("valid", nm!(0, Some(0), valid))
        .method("current", nm!(0, Some(0), queue_current))
        .method("key", nm!(0, Some(0), key))
        .method("next", nm!(0, Some(0), queue_next))
        .method("__serialize", nm!(0, Some(0), magic_serialize_method))
        .method("__unserialize", nm!(1, Some(1), queue_magic_unserialize))
        .method("__debugInfo", nm!(0, Some(0), queue_debug_info))
        .finish();
}

#[cfg(test)]
mod tests {
    use rphp_value::{ArrayKey, Value};

    use crate::tests::interp;

    /// A heap of `class` filled with `items`, in order.
    fn filled(it: &mut rphp_runtime::Interp, class: &[u8], items: &[i64]) -> rphp_value::Object {
        let cid = it.class_by_name(class).unwrap();
        let o = it.new_object(cid).unwrap();
        for v in items {
            it.call_method(&o, b"insert", &[Value::Int(*v)]).unwrap();
        }
        o
    }

    /// The backing list as php dumps it.
    fn layout(o: &rphp_value::Object) -> Vec<i64> {
        let Some(Value::Array(a)) = o.get_deref(b"heap") else {
            panic!("the heap property is an array")
        };
        a.values().map(|v| v.to_int()).collect()
    }

    #[test]
    fn a_min_heap_has_phps_exact_layout() {
        let mut it = interp();
        let o = filled(&mut it, b"SplMinHeap", &[5, 3, 8, 1, 9, 2, 7]);
        assert_eq!(layout(&o), vec![1, 3, 2, 5, 9, 8, 7]);
        assert_eq!(it.call_method(&o, b"extract", &[]).unwrap(), Value::Int(1));
        assert_eq!(layout(&o), vec![2, 3, 7, 5, 9, 8]);
    }

    #[test]
    fn a_max_heap_drains_in_descending_order() {
        let mut it = interp();
        let o = filled(&mut it, b"SplMaxHeap", &[1, 5, 3]);
        let mut out = Vec::new();
        while it.call_method(&o, b"isEmpty", &[]).unwrap() == Value::Bool(false) {
            out.push(it.call_method(&o, b"extract", &[]).unwrap().to_int());
        }
        assert_eq!(out, vec![5, 3, 1]);
    }

    #[test]
    fn an_empty_heap_refuses_top_and_extract() {
        let mut it = interp();
        let o = filled(&mut it, b"SplMinHeap", &[]);
        assert_eq!(it.call_method(&o, b"key", &[]).unwrap(), Value::Int(-1));
        assert_eq!(it.call_method(&o, b"current", &[]).unwrap(), Value::Null);
        let err = it.call_method(&o, b"top", &[]).unwrap_err();
        assert_eq!(err.message(), Some("Can't peek at an empty heap"));
        let err = it.call_method(&o, b"extract", &[]).unwrap_err();
        assert_eq!(err.message(), Some("Can't extract from an empty heap"));
        assert_eq!(err.class_name().as_deref(), Some("RuntimeException"));
    }

    #[test]
    fn spl_heap_is_abstract() {
        let mut it = interp();
        let cid = it.class_by_name(b"SplHeap").unwrap();
        let err = it.new_object(cid).unwrap_err();
        assert_eq!(err.message(), Some("Cannot instantiate abstract class SplHeap"));
    }

    #[test]
    fn a_priority_queue_orders_by_priority_and_shapes_by_flags() {
        let mut it = interp();
        let cid = it.class_by_name(b"SplPriorityQueue").unwrap();
        let o = it.new_object(cid).unwrap();
        for (v, p) in [("a", 1), ("b", 3), ("c", 2)] {
            it.call_method(&o, b"insert", &[Value::string(v.as_bytes()), Value::Int(p)])
                .unwrap();
        }
        assert_eq!(
            it.call_method(&o, b"getExtractFlags", &[]).unwrap(),
            Value::Int(super::EXTR_DATA)
        );
        assert_eq!(
            it.call_method(&o, b"extract", &[]).unwrap(),
            Value::string(b"b")
        );
        it.call_method(&o, b"setExtractFlags", &[Value::Int(super::EXTR_PRIORITY)])
            .unwrap();
        assert_eq!(it.call_method(&o, b"extract", &[]).unwrap(), Value::Int(2));
        let err = it
            .call_method(&o, b"setExtractFlags", &[Value::Int(4)])
            .unwrap_err();
        assert_eq!(err.message(), Some("Must specify at least one extract flag"));
    }

    #[test]
    fn serialize_round_trips_through_the_magic_pair() {
        let mut it = interp();
        let o = filled(&mut it, b"SplMinHeap", &[3, 1, 2]);
        let payload = it.call_method(&o, b"__serialize", &[]).unwrap();
        let Value::Array(pair) = &payload else { panic!("__serialize returns an array") };
        let Some(Value::Array(record)) = pair.get_deref(&ArrayKey::Int(1)) else {
            panic!("the second element is the state record")
        };
        assert!(record.get_deref(&ArrayKey::str(b"heap_elements")).is_some());
        let cid = it.class_by_name(b"SplMinHeap").unwrap();
        let copy = it.new_object(cid).unwrap();
        it.call_method(&copy, b"__unserialize", &[payload]).unwrap();
        assert_eq!(layout(&copy), vec![1, 3, 2]);
        let err = it
            .call_method(&copy, b"__unserialize", &[Value::empty_array()])
            .unwrap_err();
        assert_eq!(
            err.message(),
            Some("Invalid serialization data for SplMinHeap object")
        );
    }
}
