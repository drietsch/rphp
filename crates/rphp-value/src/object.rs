//! An object value: an instance of a class with slot-laid-out properties.
//!
//! Unlike [`crate::Array`] (a copy-on-write value type), PHP objects have
//! **reference** semantics: `$b = $a` aliases the same instance, and a method
//! mutating `$this->x` is visible through every handle. We model that with a
//! shared, interior-mutable cell (`Rc<RefCell<ObjectData>>`); cloning a
//! [`Value::Object`] is a refcount bump onto the same instance, and `===` is
//! pointer identity (matching PHP, where two distinct objects are never
//! identical).
//!
//! **Layout.** `rphp-value` sits below the class table, so the class is an
//! opaque `u32` id and the *shape* of an instance comes from a caller-provided
//! [`Layout`] (built by the runtime once per class): declared properties live
//! in `slots` at fixed indices, undeclared ones in an insertion-ordered
//! [`DynProps`] bag. [`ObjectData::props_in_order`] yields declared slots in
//! layout order then dynamic properties — the order PHP exposes to
//! `foreach`/`var_dump`/`json_encode`. A slot may hold a [`Value::Ref`]
//! (`$r = &$o->p`) or [`Value::Uninit`] (typed property never written).
//!
//! **Borrowing rule.** [`Object::with_data`] / [`Object::with_data_mut`] run a
//! closure under the `RefCell` guard and never leak it. Code that re-enters
//! the VM (magic methods, callbacks, destructors) must **copy out, drop the
//! borrow, then call** — holding a guard across re-entry panics on the next
//! access to the same object.
//!
//! **Destructors (plan D7).** Safe Rust cannot call the VM from `Drop`, so an
//! object flagged [`ObjFlags::HAS_DESTRUCTOR`] is *resurrected* when its last
//! handle drops: its data moves into a fresh handle flagged
//! [`ObjFlags::DESTRUCTED`] and is pushed onto a thread-local queue the runtime
//! drains at op boundaries and native returns
//! ([`take_pending_destructors`]). A DESTRUCTED object dropping again is
//! simply freed.
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};
use std::rc::{Rc, Weak};

use crate::{PhpRef, Value};

/// Property visibility, carried on each layout slot so the value formatters
/// (`json_encode` emits public only; `var_dump` annotates) can honour it without
/// reaching back into the class table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Vis {
    Public,
    Protected,
    Private,
}

/// One declared property in a [`Layout`].
#[derive(Clone, Debug)]
pub struct PropMeta {
    pub name: Box<[u8]>,
    pub vis: Vis,
    /// The class that declares the property (visibility checks, and the
    /// `["p":"Decl":private]` annotation `var_dump` prints).
    pub decl_class: u32,
    /// `decl_class`'s name, for the formatters.
    pub decl_class_name: Rc<[u8]>,
    /// The declared type as written, for `uninitialized(int)` in a dump;
    /// `None` for an untyped property.
    pub ty: Option<Rc<str>>,
    /// A php 8.4 *virtual* property (hooks that never touch a backing
    /// value): the slot exists to keep the layout aligned with the class's
    /// property list, but no consumer ever sees it.
    pub is_virtual: bool,
}

/// php's storage key for a private property, `"\0Class\0name"` — the key
/// an ancestor's private property that a subclass re-declared lives under.
pub fn mangled_key(decl_class_name: &[u8], name: &[u8]) -> Box<[u8]> {
    let mut k = Vec::with_capacity(decl_class_name.len() + name.len() + 2);
    k.push(0);
    k.extend_from_slice(decl_class_name);
    k.push(0);
    k.extend_from_slice(name);
    k.into_boxed_slice()
}

/// The shape of every instance of one class: declared properties in slot
/// order plus a name → slot index. Built once per class by the runtime and
/// shared (`Rc`) by all instances.
#[derive(Debug)]
pub struct Layout {
    class_name: Rc<[u8]>,
    props: Vec<PropMeta>,
    index: HashMap<Box<[u8]>, u16>,
}

impl Layout {
    /// Build a layout; `props` are in slot order (parent-first). Panics if more
    /// than `u16::MAX` properties are declared.
    pub fn new(class_name: Rc<[u8]>, props: Vec<PropMeta>) -> Layout {
        assert!(props.len() <= usize::from(u16::MAX), "too many declared properties");
        // php keeps an ancestor's *private* property beside a subclass's
        // property of the same name — two slots, one object. The slots
        // arrive parent-first, so of two entries that share a name the
        // earlier is the ancestor's private one: it is reachable by the
        // plain name only from that ancestor's own code, and it lives in the
        // index under php's mangled key, `"\0Decl\0name"`. Its displayed
        // name stays plain, which is what the formatters print and mangle
        // themselves.
        let mut index: HashMap<Box<[u8]>, u16> = HashMap::with_capacity(props.len());
        for (i, p) in props.iter().enumerate() {
            if let Some(earlier) = index.remove(&p.name) {
                let shadowed = &props[usize::from(earlier)];
                index.insert(mangled_key(&shadowed.decl_class_name, &shadowed.name), earlier);
            }
            index.insert(p.name.clone(), i as u16);
        }
        Layout { class_name, props, index }
    }

    /// A layout with no declared properties (e.g. `stdClass`).
    pub fn empty(class_name: Rc<[u8]>) -> Layout {
        Layout::new(class_name, Vec::new())
    }

    /// The class name as declared (for `var_dump`/`print_r`/`get_class`).
    pub fn class_name(&self) -> &[u8] {
        &self.class_name
    }

    /// Declared properties in slot order.
    pub fn props(&self) -> &[PropMeta] {
        &self.props
    }

    /// Number of declared slots.
    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// The slot index of declared property `name`.
    pub fn slot_of(&self, name: &[u8]) -> Option<u16> {
        self.index.get(name).copied()
    }

    /// The metadata of slot `i`.
    pub fn prop(&self, i: u16) -> &PropMeta {
        &self.props[usize::from(i)]
    }
}

/// Undeclared ("dynamic") properties, in insertion order. Objects carry few of
/// them, so a linear scan beats a map and preserves the order PHP exposes.
#[derive(Clone, Default, Debug)]
pub struct DynProps {
    entries: Vec<(Box<[u8]>, Value)>,
}

impl DynProps {
    /// Number of dynamic properties.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value (as stored) of `name`.
    pub fn get(&self, name: &[u8]) -> Option<&Value> {
        self.entries.iter().find(|(n, _)| n.as_ref() == name).map(|(_, v)| v)
    }

    /// Mutable access to the stored value of `name` (may be a `Ref`; write with
    /// [`Value::assign`] to honour it).
    pub fn get_mut(&mut self, name: &[u8]) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(n, _)| n.as_ref() == name).map(|(_, v)| v)
    }

    /// Assign by value (writing through a reference binding), appending a new
    /// entry if absent.
    pub fn set(&mut self, name: &[u8], value: Value) {
        match self.get_mut(name) {
            Some(slot) => Value::assign(slot, value),
            None => self.entries.push((name.into(), value.unref())),
        }
    }

    /// Bind `name` to the reference cell `r`, appending if absent.
    pub fn set_ref(&mut self, name: &[u8], r: PhpRef) {
        match self.get_mut(name) {
            Some(slot) => *slot = Value::Ref(r),
            None => self.entries.push((name.into(), Value::Ref(r))),
        }
    }

    /// `&$o->name`: make the entry a reference cell (creating it as `null`).
    pub fn get_ref(&mut self, name: &[u8]) -> PhpRef {
        if self.get(name).is_none() {
            self.entries.push((name.into(), Value::Null));
        }
        Value::make_ref(self.get_mut(name).expect("just inserted"))
    }

    /// Remove `name`, returning its value (as stored).
    pub fn unset(&mut self, name: &[u8]) -> Option<Value> {
        let i = self.entries.iter().position(|(n, _)| n.as_ref() == name)?;
        Some(self.entries.remove(i).1)
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Value)> {
        self.entries.iter().map(|(n, v)| (n.as_ref(), v))
    }
}

/// Native state attached to an instance (closure data arrives in plan E6;
/// SPL/DOM/PDO objects keep their engine-side state here).
#[derive(Default)]
pub enum Payload {
    #[default]
    None,
    Native(Box<dyn Any>),
}

impl Payload {
    /// Whether no native state is attached.
    pub fn is_none(&self) -> bool {
        matches!(self, Payload::None)
    }

    /// The native payload downcast to `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        match self {
            Payload::Native(b) => b.downcast_ref::<T>(),
            Payload::None => None,
        }
    }

    /// The native payload mutably downcast to `T`.
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        match self {
            Payload::Native(b) => b.downcast_mut::<T>(),
            Payload::None => None,
        }
    }
}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Payload::None => f.write_str("None"),
            Payload::Native(_) => f.write_str("Native(..)"),
        }
    }
}

/// Per-instance flag bits.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ObjFlags(u8);

impl ObjFlags {
    pub const NONE: ObjFlags = ObjFlags(0);
    /// The class defines `__destruct`: the last drop resurrects the object onto
    /// the destructor queue.
    pub const HAS_DESTRUCTOR: ObjFlags = ObjFlags(1 << 0);
    /// `__destruct` has run (or the object is queued for it); never re-queued.
    pub const DESTRUCTED: ObjFlags = ObjFlags(1 << 1);
    /// Instance of a `readonly class`.
    pub const READONLY_CLASS: ObjFlags = ObjFlags(1 << 2);
    /// Lazy object (8.4 `ReflectionClass::newLazyGhost/newLazyProxy`), not
    /// yet initialized.
    pub const IS_LAZY: ObjFlags = ObjFlags(1 << 3);
    /// An enum case singleton. The formatters render these specially
    /// (`enum(S::A)`, `\S::A`, `S Enum:string`) and `clone` refuses them, so
    /// the flag rides on the object: the printers never see the class table.
    pub const ENUM_CASE: ObjFlags = ObjFlags(1 << 4);

    /// The raw bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Flags from raw bits (unknown bits are kept, reserved for later flags).
    pub const fn from_bits(bits: u8) -> ObjFlags {
        ObjFlags(bits)
    }

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: ObjFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Set the bits of `other`.
    pub fn insert(&mut self, other: ObjFlags) {
        self.0 |= other.0;
    }

    /// Clear the bits of `other`.
    pub fn remove(&mut self, other: ObjFlags) {
        self.0 &= !other.0;
    }
}

impl BitOr for ObjFlags {
    type Output = ObjFlags;
    fn bitor(self, rhs: ObjFlags) -> ObjFlags {
        ObjFlags(self.0 | rhs.0)
    }
}

impl BitOrAssign for ObjFlags {
    fn bitor_assign(&mut self, rhs: ObjFlags) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for ObjFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = [
            (ObjFlags::HAS_DESTRUCTOR, "HAS_DESTRUCTOR"),
            (ObjFlags::DESTRUCTED, "DESTRUCTED"),
            (ObjFlags::READONLY_CLASS, "READONLY_CLASS"),
            (ObjFlags::IS_LAZY, "IS_LAZY"),
        ];
        let set: Vec<&str> =
            names.iter().filter(|(b, _)| self.contains(*b)).map(|(_, n)| *n).collect();
        write!(f, "ObjFlags({})", set.join("|"))
    }
}

/// One property as seen by iteration and the formatters.
pub struct PropEntry<'a> {
    pub name: &'a [u8],
    /// The value as stored (may be a `Ref` or `Uninit`).
    pub value: &'a Value,
    pub vis: Vis,
    /// `Some` for a declared slot, `None` for a dynamic property.
    pub meta: Option<&'a PropMeta>,
}

/// The instance state behind an [`Object`] handle.
pub struct ObjectData {
    class: u32,
    id: u32,
    layout: Rc<Layout>,
    slots: Vec<Value>,
    dyn_props: Option<Box<DynProps>>,
    payload: Payload,
    flags: ObjFlags,
    /// php 8.4 lazy-object state, when the object is (or was made) lazy.
    lazy: Option<Box<LazyState>>,
    /// php's `cast_object` handler, for the native classes whose instances
    /// convert by their own rule (`SimpleXMLElement`'s truthiness and
    /// numbers); `None` for every other object.
    cast: Option<CastHandler>,
    /// Which declared slots `unset()` emptied (php's `IS_PROP_UNINIT` flag
    /// cleared): a typed property in that state goes to the magic accessors
    /// where a never-initialized one bypasses them. `None` until the first
    /// `unset()`.
    unset_marks: Option<Box<[bool]>>,
}

/// What a [`CastHandler`] is asked to convert to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CastTarget {
    Bool,
    Int,
    Float,
    Str,
}

/// A native class's cast rule: `None` leaves the conversion to the
/// engine's default for objects.
pub type CastHandler = fn(&Object, CastTarget) -> Option<Value>;

/// Which of php 8.4's two lazy shapes an object is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LazyKind {
    /// `newLazyGhost`: the object *is* the instance; the initializer fills
    /// it in place.
    Ghost,
    /// `newLazyProxy`: the object stands in for a real instance the factory
    /// builds; property accesses forward to it afterwards.
    Proxy,
}

/// php 8.4 lazy-object state (`ReflectionClass::newLazyGhost` /
/// `newLazyProxy`). The runtime runs the initializer the first time a
/// property is touched — reading, writing, `isset`, `unset`, `foreach`,
/// `serialize`, `json_encode`, `clone`, `==` — and never for a plain method
/// call; the props the caller marked ahead of time
/// (`ReflectionProperty::skipLazyInitialization`) do not trigger it.
#[derive(Clone, Debug)]
pub struct LazyState {
    pub kind: LazyKind,
    /// The initializer (ghost) or factory (proxy) callable.
    pub initializer: Value,
    /// `ReflectionClass::SKIP_*` bits.
    pub options: i64,
    /// Whether the initializer has run (or the object was marked so).
    pub initialized: bool,
    /// A proxy's real instance, once the factory has built it.
    pub real: Option<Object>,
    /// Properties initialized ahead of the initializer, which an access
    /// does not trigger it for.
    pub skipped: Vec<Box<[u8]>>,
    /// The declared slots' default values, in layout order: what a ghost's
    /// slots hold when its initializer starts, and what
    /// `ReflectionProperty::skipLazyInitialization()` puts in one.
    pub defaults: Vec<Value>,
}

impl LazyState {
    /// A fresh, uninitialized state.
    pub fn new(kind: LazyKind, initializer: Value, options: i64, defaults: Vec<Value>) -> LazyState {
        LazyState {
            kind,
            initializer,
            options,
            initialized: false,
            real: None,
            skipped: Vec::new(),
            defaults,
        }
    }

    /// Whether declared slot `name` still waits on the initializer.
    pub fn pending(&self, name: &[u8]) -> bool {
        !self.initialized && !self.skipped.iter().any(|s| s.as_ref() == name)
    }
}

impl ObjectData {
    /// The id of the class this object instantiates.
    pub fn class_id(&self) -> u32 {
        self.class
    }

    /// The object handle number (`spl_object_id`, the `#N` in `var_dump`).
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The class's instance layout.
    pub fn layout(&self) -> &Rc<Layout> {
        &self.layout
    }

    /// The instance flags.
    pub fn flags(&self) -> ObjFlags {
        self.flags
    }

    /// The instance flags, mutably.
    pub fn flags_mut(&mut self) -> &mut ObjFlags {
        &mut self.flags
    }

    /// The php 8.4 lazy-object state, if the object is or was made lazy.
    pub fn lazy(&self) -> Option<&LazyState> {
        self.lazy.as_deref()
    }

    /// Declared slots in layout order (values as stored).
    pub fn slots(&self) -> &[Value] {
        &self.slots
    }

    /// Declared slots, mutably (a slot may hold a `Ref`; write with
    /// [`Value::assign`] to honour it).
    pub fn slots_mut(&mut self) -> &mut [Value] {
        &mut self.slots
    }

    /// The dynamic-property bag, if any property was ever added.
    pub fn dyn_props(&self) -> Option<&DynProps> {
        self.dyn_props.as_deref()
    }

    /// The dynamic-property bag, created on first use.
    pub fn dyn_props_mut(&mut self) -> &mut DynProps {
        self.dyn_props.get_or_insert_with(Default::default)
    }

    /// The native payload.
    pub fn payload(&self) -> &Payload {
        &self.payload
    }

    /// The native payload, mutably.
    pub fn payload_mut(&mut self) -> &mut Payload {
        &mut self.payload
    }

    /// Replace the native payload.
    pub fn set_payload(&mut self, payload: Payload) {
        self.payload = payload;
    }

    /// Property `name` as stored: the declared slot, else the dynamic entry.
    pub fn get(&self, name: &[u8]) -> Option<&Value> {
        match self.layout.slot_of(name) {
            Some(i) => self.slots.get(usize::from(i)),
            None => self.dyn_props.as_ref().and_then(|d| d.get(name)),
        }
    }

    /// Mutable access to property `name` as stored (may be a `Ref`; write with
    /// [`Value::assign`] to honour it).
    pub fn get_mut(&mut self, name: &[u8]) -> Option<&mut Value> {
        match self.layout.slot_of(name) {
            Some(i) => self.slots.get_mut(usize::from(i)),
            None => self.dyn_props.as_mut().and_then(|d| d.get_mut(name)),
        }
    }

    /// Assign property `name` by value: a declared slot (or existing dynamic
    /// entry) is written through its reference binding if any; an unknown name
    /// appends a **public** dynamic property (PHP lets you assign arbitrary
    /// properties onto an instance; the 8.2 deprecation is the runtime's job).
    pub fn set(&mut self, name: &[u8], value: Value) {
        match self.layout.slot_of(name) {
            Some(i) => {
                self.clear_unset_mark(i);
                Value::assign(&mut self.slots[usize::from(i)], value)
            }
            None => self.dyn_props_mut().set(name, value),
        }
    }

    fn clear_unset_mark(&mut self, i: u16) {
        if let Some(m) = &mut self.unset_marks {
            m[usize::from(i)] = false;
        }
    }

    /// Whether declared slot `name` was emptied by `unset()` (and not
    /// written since).
    pub fn was_unset(&self, name: &[u8]) -> bool {
        match (self.layout.slot_of(name), &self.unset_marks) {
            (Some(i), Some(m)) => m[usize::from(i)],
            _ => false,
        }
    }

    /// `&$o->name`: make the property a reference cell in place (a dynamic
    /// property is created as `null`) and return it.
    pub fn prop_ref(&mut self, name: &[u8]) -> PhpRef {
        match self.layout.slot_of(name) {
            Some(i) => {
                self.clear_unset_mark(i);
                Value::make_ref(&mut self.slots[usize::from(i)])
            }
            None => self.dyn_props_mut().get_ref(name),
        }
    }

    /// `unset($o->name)`: a declared slot becomes [`Value::Uninit`] (PHP keeps
    /// the declaration; the next read goes to `__get` or errors), a dynamic
    /// property is removed. Returns the previous value as stored.
    pub fn unset(&mut self, name: &[u8]) -> Option<Value> {
        match self.layout.slot_of(name) {
            Some(i) => {
                let n = self.slots.len();
                self.unset_marks.get_or_insert_with(|| vec![false; n].into_boxed_slice())[usize::from(i)] = true;
                let slot = &mut self.slots[usize::from(i)];
                if slot.is_uninit() {
                    None
                } else {
                    Some(std::mem::replace(slot, Value::Uninit))
                }
            }
            None => self.dyn_props.as_mut().and_then(|d| d.unset(name)),
        }
    }

    /// Number of initialized properties (declared slots that are not `Uninit`,
    /// plus dynamic ones) — the `(N)` in `var_dump`'s header.
    pub fn prop_count(&self) -> usize {
        self.layout
            .props
            .iter()
            .zip(self.slots.iter())
            .filter(|(m, v)| !m.is_virtual && !v.is_uninit())
            .count()
            + self.dyn_props.as_ref().map_or(0, |d| d.len())
    }

    /// All properties in PHP order: declared slots (layout order, including
    /// `Uninit` ones — consumers decide whether to show or skip them), then
    /// dynamic properties in insertion order.
    pub fn props_in_order(&self) -> impl Iterator<Item = PropEntry<'_>> {
        let declared = self
            .layout
            .props
            .iter()
            .zip(self.slots.iter())
            .filter(|(m, _)| !m.is_virtual)
            .map(|(m, v)| PropEntry {
                name: &m.name,
                value: v,
                vis: m.vis,
                meta: Some(m),
            });
        let dynamic = self
            .dyn_props
            .iter()
            .flat_map(|d| d.iter())
            .map(|(n, v)| PropEntry { name: n, value: v, vis: Vis::Public, meta: None });
        declared.chain(dynamic)
    }
}

impl fmt::Debug for ObjectData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Object({} #{}, {} props, {:?})",
            String::from_utf8_lossy(&self.layout.class_name),
            self.id,
            self.prop_count(),
            self.flags
        )
    }
}

impl Drop for ObjectData {
    fn drop(&mut self) {
        if self.flags.contains(ObjFlags::HAS_DESTRUCTOR) && !self.flags.contains(ObjFlags::DESTRUCTED) {
            // Resurrect: move the state into a fresh handle that the runtime
            // will run `__destruct` on, then free.
            let resurrected = ObjectData {
                class: self.class,
                id: self.id,
                layout: self.layout.clone(),
                slots: std::mem::take(&mut self.slots),
                dyn_props: self.dyn_props.take(),
                payload: std::mem::take(&mut self.payload),
                flags: self.flags | ObjFlags::DESTRUCTED,
                lazy: self.lazy.take(),
                cast: self.cast,
                unset_marks: self.unset_marks.take(),
            };
            let obj = Object(Rc::new(RefCell::new(resurrected)));
            DESTRUCT_QUEUE.with(|q| q.borrow_mut().push(obj));
            DESTRUCT_PENDING.with(|p| p.set(true));
        }
    }
}

thread_local! {
    static DESTRUCT_QUEUE: RefCell<Vec<Object>> = const { RefCell::new(Vec::new()) };
    static DESTRUCT_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Whether any object is waiting for its `__destruct` (a cheap flag check for
/// the interpreter's op boundary).
pub fn has_pending_destructors() -> bool {
    DESTRUCT_PENDING.with(Cell::get)
}

/// Take every object queued for `__destruct` (each flagged `DESTRUCTED`), in
/// the order their last handles dropped. The runtime calls `__destruct` on
/// each and then drops it for good.
pub fn take_pending_destructors() -> Vec<Object> {
    DESTRUCT_PENDING.with(|p| p.set(false));
    DESTRUCT_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Allocator for object handle numbers. PHP numbers handles from 1 and reuses
/// freed ones; this one is monotonic (a documented divergence visible only in
/// `spl_object_id`/`var_dump` after objects are freed). The runtime owns one
/// per run.
#[derive(Debug, Default)]
pub struct ObjectIdAllocator {
    last: u32,
}

impl ObjectIdAllocator {
    pub fn new() -> Self {
        ObjectIdAllocator::default()
    }

    /// The next id (starting at 1).
    pub fn alloc(&mut self) -> u32 {
        self.last += 1;
        self.last
    }

    /// The most recently allocated id (0 if none).
    pub fn last(&self) -> u32 {
        self.last
    }
}

/// A handle onto an instance; cloning aliases the same object.
#[derive(Clone)]
pub struct Object(Rc<RefCell<ObjectData>>);

impl Object {
    /// Create instance `id` of `class` with the given `layout`, seeding the
    /// declared slots from `defaults` (one per layout slot, in slot order).
    pub fn new(class: u32, id: u32, layout: Rc<Layout>, defaults: Vec<Value>) -> Self {
        assert_eq!(defaults.len(), layout.len(), "one default per declared property");
        Object(Rc::new(RefCell::new(ObjectData {
            class,
            id,
            layout,
            slots: defaults,
            dyn_props: None,
            payload: Payload::None,
            flags: ObjFlags::NONE,
            lazy: None,
            cast: None,
            unset_marks: None,
        })))
    }

    /// Install the class's cast rule on this instance.
    pub fn set_cast_handler(&self, h: CastHandler) {
        self.0.borrow_mut().cast = Some(h);
    }

    /// Convert through the cast rule, when the instance has one.
    pub fn cast_via_handler(&self, target: CastTarget) -> Option<Value> {
        let h = self.0.borrow().cast?;
        h(self, target)
    }

    /// Run `f` under a shared borrow of the instance state. Never leaks the
    /// guard; `f` must not re-enter the VM (see the module docs).
    pub fn with_data<R>(&self, f: impl FnOnce(&ObjectData) -> R) -> R {
        f(&self.0.borrow())
    }

    /// Run `f` under an exclusive borrow of the instance state. Never leaks the
    /// guard; `f` must not re-enter the VM (see the module docs).
    pub fn with_data_mut<R>(&self, f: impl FnOnce(&mut ObjectData) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }

    /// The id of the class this object instantiates.
    pub fn class_id(&self) -> u32 {
        self.0.borrow().class
    }

    /// The object handle number (`spl_object_id`).
    pub fn id(&self) -> u32 {
        self.0.borrow().id
    }

    /// The class's instance layout (an `Rc` bump).
    pub fn layout(&self) -> Rc<Layout> {
        self.0.borrow().layout.clone()
    }

    /// The instance flags.
    pub fn flags(&self) -> ObjFlags {
        self.0.borrow().flags
    }

    /// Replace the instance flags.
    pub fn set_flags(&self, flags: ObjFlags) {
        self.0.borrow_mut().flags = flags;
    }

    /// OR `flags` into the instance flags.
    pub fn add_flags(&self, flags: ObjFlags) {
        self.0.borrow_mut().flags |= flags;
    }

    pub fn remove_flags(&self, flags: ObjFlags) {
        self.0.borrow_mut().flags.remove(flags);
    }

    /// Read property `name` as stored (a bound slot yields the `Ref`), or
    /// `None` if the object has no such property.
    pub fn get(&self, name: &[u8]) -> Option<Value> {
        self.0.borrow().get(name).cloned()
    }

    /// Read property `name`, dereferencing a reference binding.
    pub fn get_deref(&self, name: &[u8]) -> Option<Value> {
        self.0.borrow().get(name).map(|v| v.deref().into_owned())
    }

    /// Assign property `name` by value (see [`ObjectData::set`]).
    pub fn set(&self, name: &[u8], value: Value) {
        self.0.borrow_mut().set(name, value);
    }

    /// Declared slot `i`, as stored.
    pub fn slot(&self, i: u16) -> Value {
        self.0.borrow().slots[usize::from(i)].clone()
    }

    /// Assign declared slot `i` by value (writing through a binding).
    pub fn set_slot(&self, i: u16, value: Value) {
        let mut d = self.0.borrow_mut();
        d.clear_unset_mark(i);
        Value::assign(&mut d.slots[usize::from(i)], value);
    }

    /// See [`ObjectData::was_unset`].
    pub fn was_unset(&self, name: &[u8]) -> bool {
        self.0.borrow().was_unset(name)
    }

    /// Copy another object's `unset()` marks (a clone keeps them).
    pub fn copy_unset_marks_from(&self, other: &Object) {
        let marks = other.0.borrow().unset_marks.clone();
        self.0.borrow_mut().unset_marks = marks;
    }

    /// Make declared slot `i` a reference cell in place and return it.
    pub fn slot_ref(&self, i: u16) -> PhpRef {
        let mut d = self.0.borrow_mut();
        d.clear_unset_mark(i);
        Value::make_ref(&mut d.slots[usize::from(i)])
    }

    /// `&$o->name` over declared and dynamic properties.
    pub fn prop_ref(&self, name: &[u8]) -> PhpRef {
        self.0.borrow_mut().prop_ref(name)
    }

    /// Read dynamic property `name` as stored.
    pub fn dyn_get(&self, name: &[u8]) -> Option<Value> {
        self.0.borrow().dyn_props.as_ref().and_then(|d| d.get(name)).cloned()
    }

    /// Assign dynamic property `name` by value (appending if absent).
    pub fn dyn_set(&self, name: &[u8], value: Value) {
        self.0.borrow_mut().dyn_props_mut().set(name, value);
    }

    /// Bind dynamic property `name` to the reference cell `r`.
    pub fn dyn_set_ref(&self, name: &[u8], r: PhpRef) {
        self.0.borrow_mut().dyn_props_mut().set_ref(name, r);
    }

    /// Remove dynamic property `name`, returning it.
    pub fn dyn_unset(&self, name: &[u8]) -> Option<Value> {
        self.0.borrow_mut().dyn_props.as_mut().and_then(|d| d.unset(name))
    }

    /// `unset($o->name)` (see [`ObjectData::unset`]).
    pub fn unset(&self, name: &[u8]) -> Option<Value> {
        self.0.borrow_mut().unset(name)
    }

    /// Number of initialized properties.
    pub fn prop_count(&self) -> usize {
        self.0.borrow().prop_count()
    }

    /// A snapshot of the properties in PHP order — `(name, value as stored,
    /// visibility)` — for consumers that will re-enter the VM while walking
    /// them (a `__debugInfo` call, a `foreach` body): cloned out so no borrow
    /// is held. `Uninit` slots are skipped.
    /// [`props_snapshot`](Self::props_snapshot) with each slot's declaring
    /// class, which is what tells an ancestor's private property apart from
    /// a subclass's of the same name. `None` for a dynamic property.
    pub fn props_snapshot_with_decl(&self) -> Vec<(Box<[u8]>, Value, Vis, Option<u32>)> {
        self.0
            .borrow()
            .props_in_order()
            .filter(|p| !p.value.is_uninit())
            .map(|p| (Box::from(p.name), p.value.clone(), p.vis, p.meta.map(|m| m.decl_class)))
            .collect()
    }

    pub fn props_snapshot(&self) -> Vec<(Box<[u8]>, Value, Vis)> {
        self.0
            .borrow()
            .props_in_order()
            .filter(|p| !p.value.is_uninit())
            .map(|p| (Box::from(p.name), p.value.clone(), p.vis))
            .collect()
    }

    // ---- php 8.4 lazy objects ------------------------------------------------

    /// The lazy state, if the object is or was made lazy.
    pub fn lazy(&self) -> Option<LazyState> {
        self.0.borrow().lazy.as_deref().cloned()
    }

    /// Whether the object carries lazy state at all (uninitialized, or an
    /// initialized proxy).
    pub fn is_lazy(&self) -> bool {
        self.0.borrow().lazy.is_some()
    }

    /// Whether an access to `prop` (any property when `None`) must run the
    /// initializer first: the object is lazy, not yet initialized, and the
    /// property was not initialized ahead of time.
    pub fn lazy_needs_init(&self, prop: Option<&[u8]>) -> bool {
        match self.0.borrow().lazy.as_deref() {
            Some(l) if !l.initialized => match prop {
                Some(p) => !l.skipped.iter().any(|s| s.as_ref() == p),
                None => true,
            },
            _ => false,
        }
    }

    /// `ReflectionClass::isUninitializedLazyObject()`.
    pub fn is_uninitialized_lazy(&self) -> bool {
        self.0.borrow().lazy.as_deref().is_some_and(|l| !l.initialized)
    }

    /// The real instance behind an initialized proxy, if this is one.
    pub fn lazy_real(&self) -> Option<Object> {
        self.0.borrow().lazy.as_deref().and_then(|l| l.real.clone())
    }

    /// Make the object lazy (`newLazyGhost` / `newLazyProxy` /
    /// `resetAsLazy*`): php empties every declared slot (they read as
    /// `uninitialized(T)` until the initializer runs) and drops the dynamic
    /// properties. The old values are released after the borrow ends, so a
    /// destructor they trigger can look at this object.
    pub fn set_lazy(&self, state: LazyState) {
        let old: Vec<Value> = {
            let mut d = self.0.borrow_mut();
            let mut old: Vec<Value> = d.slots.iter_mut().map(|s| std::mem::replace(s, Value::Uninit)).collect();
            if let Some(dp) = d.dyn_props.take() {
                old.extend(dp.iter().map(|(_, v)| v.clone()));
            }
            d.lazy = Some(Box::new(state));
            old
        };
        drop(old);
    }

    /// Drop the lazy state altogether: the object is an ordinary one again
    /// (every slot initialized ahead of time, or an initialized ghost).
    pub fn clear_lazy(&self) {
        let old = self.0.borrow_mut().lazy.take();
        drop(old);
    }

    /// Update the lazy state in place.
    pub fn with_lazy_mut<R>(&self, f: impl FnOnce(&mut LazyState) -> R) -> Option<R> {
        self.0.borrow_mut().lazy.as_deref_mut().map(f)
    }

    /// Put the defaults back into every slot still waiting on the
    /// initializer — what a ghost's initializer starts from.
    pub fn lazy_restore_defaults(&self) {
        let mut d = self.0.borrow_mut();
        let Some(l) = d.lazy.as_deref() else { return };
        let fill: Vec<(usize, Value)> = d
            .layout
            .props
            .iter()
            .enumerate()
            .filter(|(_, m)| !l.skipped.iter().any(|s| s.as_ref() == &*m.name))
            .map(|(i, _)| (i, l.defaults.get(i).cloned().unwrap_or(Value::Uninit)))
            .collect();
        for (i, v) in fill {
            d.slots[i] = v;
        }
    }

    /// Empty every slot still waiting on the initializer again — the state
    /// php reverts to when a ghost's initializer throws.
    pub fn lazy_undef_pending(&self) {
        let old: Vec<Value> = {
            let mut d = self.0.borrow_mut();
            let Some(l) = d.lazy.as_deref() else { return };
            let idx: Vec<usize> = d
                .layout
                .props
                .iter()
                .enumerate()
                .filter(|(_, m)| !l.skipped.iter().any(|s| s.as_ref() == &*m.name))
                .map(|(i, _)| i)
                .collect();
            let mut old = Vec::with_capacity(idx.len());
            for i in idx {
                old.push(std::mem::replace(&mut d.slots[i], Value::Uninit));
            }
            if let Some(dp) = d.dyn_props.take() {
                old.extend(dp.iter().map(|(_, v)| v.clone()));
            }
            old
        };
        drop(old);
    }

    /// Initialize declared slot `name` ahead of the initializer
    /// (`ReflectionProperty::skipLazyInitialization()` /
    /// `setRawValueWithoutLazyInitialization()`): the slot takes `value`, or
    /// its default, and no longer triggers the initializer. Answers whether
    /// every declared slot is now initialized ahead — php then treats the
    /// object as initialized.
    pub fn lazy_skip(&self, name: &[u8], value: Option<Value>) -> bool {
        let mut d = self.0.borrow_mut();
        let Some(i) = d.layout.slot_of(name) else { return false };
        let Some(l) = d.lazy.as_deref_mut() else { return false };
        if !l.skipped.iter().any(|s| s.as_ref() == name) {
            l.skipped.push(Box::from(name));
        }
        let v = value.unwrap_or_else(|| l.defaults.get(usize::from(i)).cloned().unwrap_or(Value::Uninit));
        let all = l.skipped.len() >= d.layout.props.len();
        d.slots[usize::from(i)] = v;
        all
    }

    /// Replace the native payload.
    pub fn set_payload(&self, payload: Payload) {
        self.0.borrow_mut().payload = payload;
    }

    /// Run `f` on the native payload downcast to `T`; `None` if there is none
    /// or it is of another type. The borrow is released before returning.
    pub fn with_payload<T: Any, R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.0.borrow_mut().payload.downcast_mut::<T>().map(f)
    }

    /// Identity (`===`).
    pub fn ptr_eq(&self, other: &Object) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Number of live strong handles.
    pub fn strong_count(&self) -> usize {
        Rc::strong_count(&self.0)
    }

    /// A non-owning handle (`WeakReference`, `WeakMap` keys, the GC registry).
    pub fn downgrade(&self) -> WeakObject {
        WeakObject(Rc::downgrade(&self.0))
    }
}

impl PartialEq for Object {
    /// Identity comparison: the same instance, never two distinct ones. PHP `===`
    /// on objects is reference identity; loose `==` (same class + equal props) is
    /// a documented divergence handled at the `Value` layer when it lands.
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.try_borrow() {
            Ok(d) => d.fmt(f),
            Err(_) => f.write_str("Object(<borrowed>)"),
        }
    }
}

/// A non-owning handle onto an object; `upgrade` fails once the object is gone.
#[derive(Clone)]
pub struct WeakObject(Weak<RefCell<ObjectData>>);

impl WeakObject {
    /// The object, if it is still alive.
    pub fn upgrade(&self) -> Option<Object> {
        self.0.upgrade().map(Object)
    }

    /// Whether this weak handle points at `obj`.
    pub fn ptr_eq_obj(&self, obj: &Object) -> bool {
        std::ptr::eq(self.0.as_ptr(), Rc::as_ptr(&obj.0))
    }
}

impl fmt::Debug for WeakObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.upgrade() {
            Some(o) => write!(f, "Weak({o:?})"),
            None => f.write_str("Weak(<dead>)"),
        }
    }
}

/// The part of a class name php shows.
///
/// An anonymous class is named `class@anonymous\0<file>:<line>$<n>`, and php
/// formats a class name with `%s` nearly everywhere — in `var_dump`, in
/// `print_r`, in every error message — so the synthesized tail after the NUL
/// never appears. The places that use the string's real length instead
/// (`get_class()`, `::class`, `var_export`, Reflection) keep the whole name,
/// which is what makes two anonymous classes distinguishable.
pub fn display_class_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == 0) {
        Some(i) => &name[..i],
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> Rc<[u8]> {
        Rc::from(s.as_bytes())
    }

    fn meta(n: &str, vis: Vis) -> PropMeta {
        PropMeta { name: Box::from(n.as_bytes()), vis, decl_class: 0, decl_class_name: name("Foo"), ty: None, is_virtual: false }
    }

    fn layout() -> Rc<Layout> {
        Rc::new(Layout::new(name("Foo"), vec![meta("x", Vis::Public), meta("p", Vis::Private)]))
    }

    fn obj() -> Object {
        Object::new(0, 1, layout(), vec![Value::Int(1), Value::Int(2)])
    }

    fn names(o: &Object) -> Vec<String> {
        o.with_data(|d| {
            d.props_in_order().map(|p| String::from_utf8_lossy(p.name).into_owned()).collect()
        })
    }

    #[test]
    fn get_returns_default_then_updated_value() {
        let o = obj();
        assert_eq!(o.get(b"x"), Some(Value::Int(1)));
        o.set(b"x", Value::Int(9));
        assert_eq!(o.get(b"x"), Some(Value::Int(9)));
        assert_eq!(o.slot(0), Value::Int(9));
        assert_eq!(o.get(b"missing"), None);
        assert_eq!(o.class_id(), 0);
        assert_eq!(o.id(), 1);
        assert_eq!(o.layout().class_name(), b"Foo");
        assert_eq!(o.layout().slot_of(b"p"), Some(1));
    }

    #[test]
    fn set_appends_dynamic_public_property_in_order() {
        let o = obj();
        o.set(b"y", Value::Int(2));
        o.dyn_set(b"z", Value::Int(3));
        assert_eq!(names(&o), vec!["x", "p", "y", "z"]);
        assert_eq!(o.prop_count(), 4);
        o.with_data(|d| {
            let vis: Vec<Vis> = d.props_in_order().map(|p| p.vis).collect();
            assert_eq!(vis, vec![Vis::Public, Vis::Private, Vis::Public, Vis::Public]);
            assert!(d.props_in_order().nth(1).unwrap().meta.is_some());
            assert!(d.props_in_order().nth(2).unwrap().meta.is_none());
        });
        assert_eq!(o.dyn_get(b"y"), Some(Value::Int(2)));
        assert_eq!(o.dyn_get(b"x"), None); // declared, not dynamic
        assert_eq!(o.dyn_unset(b"y"), Some(Value::Int(2)));
        assert_eq!(names(&o), vec!["x", "p", "z"]);
        assert_eq!(o.props_snapshot().len(), 3);
    }

    #[test]
    fn clone_aliases_the_same_instance() {
        // Reference semantics: a clone is the same cell, so a write is shared and
        // identity (`PartialEq`) holds; a distinct object is never equal.
        let a = obj();
        let b = a.clone();
        b.set(b"x", Value::Int(42));
        assert_eq!(a.get(b"x"), Some(Value::Int(42)));
        assert_eq!(a, b);
        assert_ne!(a, obj());
        assert_eq!(a.strong_count(), 2);
    }

    #[test]
    fn slot_refs_bind_and_writes_go_through() {
        // $r = &$o->x; $r = 5; echo $o->x;  => 5 ; $o->x = 6; echo $r; => 6
        let o = obj();
        let r = o.slot_ref(0);
        assert!(o.slot(0).is_ref());
        assert!(o.get(b"x").unwrap().is_ref());
        assert_eq!(o.get_deref(b"x"), Some(Value::Int(1)));
        r.set(Value::Int(5));
        assert_eq!(o.get_deref(b"x"), Some(Value::Int(5)));
        o.set(b"x", Value::Int(6));
        assert_eq!(r.get(), Value::Int(6));
        o.set_slot(0, Value::Int(7));
        assert_eq!(r.get(), Value::Int(7));
        assert!(o.prop_ref(b"x").ptr_eq(&r));
        // A dynamic property can be bound too, created as null.
        let d = o.prop_ref(b"dyn");
        assert_eq!(d.get(), Value::Null);
        assert!(o.dyn_get(b"dyn").unwrap().is_ref());
        o.set(b"dyn", Value::Int(1));
        assert_eq!(d.get(), Value::Int(1));
    }

    #[test]
    fn unset_makes_declared_slots_uninit_and_removes_dynamic_ones() {
        let o = obj();
        o.set(b"d", Value::Int(1));
        assert_eq!(o.unset(b"x"), Some(Value::Int(1)));
        assert_eq!(o.unset(b"x"), None);
        assert!(o.slot(0).is_uninit());
        assert_eq!(o.prop_count(), 2); // p + d
        assert_eq!(o.props_snapshot().len(), 2);
        assert_eq!(o.unset(b"d"), Some(Value::Int(1)));
        assert_eq!(names(&o), vec!["x", "p"]); // uninit slot still listed
        assert_eq!(o.prop_count(), 1);
    }

    #[test]
    fn payload_flags_and_weak_handles() {
        let o = obj();
        assert!(o.with_payload(|_: &mut Vec<u8>| ()).is_none());
        o.set_payload(Payload::Native(Box::new(vec![1u8])));
        assert_eq!(o.with_payload(|v: &mut Vec<u8>| { v.push(2); v.len() }), Some(2));
        assert!(o.with_payload(|_: &mut String| ()).is_none());
        o.add_flags(ObjFlags::READONLY_CLASS);
        assert!(o.flags().contains(ObjFlags::READONLY_CLASS));
        assert!(!o.flags().contains(ObjFlags::IS_LAZY));
        assert_eq!(format!("{:?}", o.flags()), "ObjFlags(READONLY_CLASS)");
        let w = o.downgrade();
        assert!(w.ptr_eq_obj(&o));
        assert!(w.upgrade().is_some());
        drop(o);
        assert!(w.upgrade().is_none());
    }

    #[test]
    fn destructor_queue_resurrects_once() {
        // Drain anything a previous test on this thread left behind.
        let _ = take_pending_destructors();
        assert!(!has_pending_destructors());

        // An object without __destruct is simply freed.
        drop(obj());
        assert!(!has_pending_destructors());

        let o = obj();
        o.set(b"x", Value::Int(99));
        o.add_flags(ObjFlags::HAS_DESTRUCTOR);
        let w = o.downgrade();
        drop(o);
        assert!(has_pending_destructors());
        assert!(w.upgrade().is_none()); // the original cell is gone ...
        let queued = take_pending_destructors();
        assert!(!has_pending_destructors());
        assert_eq!(queued.len(), 1);
        // ... and the state moved into a fresh handle flagged DESTRUCTED.
        let q = &queued[0];
        assert_eq!(q.id(), 1);
        assert_eq!(q.class_id(), 0);
        assert_eq!(q.get(b"x"), Some(Value::Int(99)));
        assert!(q.flags().contains(ObjFlags::HAS_DESTRUCTOR));
        assert!(q.flags().contains(ObjFlags::DESTRUCTED));
        // Dropping the resurrected object does not re-queue it.
        drop(queued);
        assert!(!has_pending_destructors());
        assert!(take_pending_destructors().is_empty());

        // A handle that survives (e.g. resurrected by __destruct into a global)
        // keeps living; dropping it later is a plain free.
        let o2 = obj();
        o2.add_flags(ObjFlags::HAS_DESTRUCTOR);
        let keep = o2.clone();
        drop(o2);
        assert!(!has_pending_destructors()); // still one handle
        drop(keep);
        let queued = take_pending_destructors();
        assert_eq!(queued.len(), 1);
        let survivor = queued[0].clone();
        drop(queued);
        assert!(!has_pending_destructors());
        drop(survivor);
        assert!(!has_pending_destructors());
    }

    #[test]
    #[should_panic(expected = "one default per declared property")]
    fn new_checks_default_count() {
        let _ = Object::new(0, 1, layout(), vec![Value::Null]);
    }
}
