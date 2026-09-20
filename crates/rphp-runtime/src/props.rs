//! Property access (plan E6): the read/write/isset/unset paths behind
//! `FetchProp`/`AssignProp`/`IssetProp`/`UnsetProp` and the object protocols
//! layered on them.
//!
//! **This module owns everything about reading and writing an object
//! property.** `exec.rs` only decodes the operand and calls in here.
//!
//! What E6 fills in, and how php behaves (every text below was taken from
//! `php -r` on 8.5, not guessed):
//!
//! * the magic hooks — `__get`/`__set`/`__isset`/`__unset`, consulted when a
//!   declared property is not visible from the calling scope or does not
//!   exist, with php's recursion guard: while `__get` runs for object `O` and
//!   property `P`, another read of `O::P` does **not** re-enter `__get` (it
//!   falls through to the plain behaviour). The four guards are independent —
//!   inside `__get($p)` a *write* of `$this->$p` does reach `__set` — which is
//!   what makes the `private array $data` + `__get`/`__set` pattern work;
//! * declared property **types**, coerced on assignment through
//!   [`crate::types`] under the *assigning* frame's `strict_types`
//!   (`TypeError: Cannot assign string to property C::$p of type int`), and
//!   `Error: Typed property C::$p must not be accessed before initialization`
//!   for a typed slot that was never written;
//! * `readonly` — a second write is `Error: Cannot modify readonly property
//!   C::$p`; a write from a scope that may not initialize it is `Error: Cannot
//!   modify protected(set) readonly property C::$p from global scope`
//!   (readonly implies `protected(set)`, so a subclass *may* initialize it);
//! * **asymmetric visibility** (`public private(set)`), where the `set`
//!   visibility is checked against the writing scope — see the note on
//!   `Interp::set_visibility`: the class model cannot carry an explicit
//!   `set` visibility yet, so only `readonly`'s implicit one is enforced;
//! * **property hooks** (`get`/`set`) are *not* wired: `PropInfo` has no
//!   `hooks` field, so the compiled hook `FuncId`s never reach the runtime.
//!   See the note on `Interp::write_prop` for the exact field.
//!
//! Three visibility outcomes matter, not two ([`Access`]): a `private`
//! property declared by an *ancestor* of the object's class is not merely
//! inaccessible, it is **nameless** from outside that ancestor — php reports
//! `Undefined property`, routes the access to the magic accessors, and lets a
//! dynamic property of the same name live beside the hidden slot.

use std::cell::RefCell;

use rphp_bytecode::{TypeDecl, Visibility};
use rphp_value::{Object, Value};

use crate::class::{ClassDef, MagicFlags, PropInfo};
use crate::exec::vis_word;
use crate::ops::value_name;
use crate::registry::Unwind;
use crate::types::Coerced;
use crate::Interp;

/// One of php's four property accessors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Magic {
    /// `__get($name)`.
    Get,
    /// `__set($name, $value)`.
    Set,
    /// `__isset($name)`.
    Isset,
    /// `__unset($name)`.
    Unset,
}

impl Magic {
    /// The method php calls.
    fn method(self) -> &'static [u8] {
        match self {
            Magic::Get => &b"__get"[..],
            Magic::Set => &b"__set"[..],
            Magic::Isset => &b"__isset"[..],
            Magic::Unset => &b"__unset"[..],
        }
    }

    /// The class-level flag that says the class defines it.
    fn flag(self) -> MagicFlags {
        match self {
            Magic::Get => MagicFlags::GET,
            Magic::Set => MagicFlags::SET,
            Magic::Isset => MagicFlags::ISSET,
            Magic::Unset => MagicFlags::UNSET,
        }
    }
}

thread_local! {
    /// php's `zend_property_guard`s: the `(object, property, accessor)`
    /// triples whose magic method is on the stack right now. A stack, so an
    /// unwinding magic call can never leave a stale guard behind; the depth
    /// is the nesting depth of magic accessors, which is tiny.
    static GUARDS: RefCell<Vec<(u32, Box<[u8]>, Magic)>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    /// The `(object, property)` pairs whose **hook** is running right now.
    /// php lets a hook body reach the backing store (`$this->p` inside
    /// `$p`'s own hook is a plain slot access) — without that bypass every
    /// hook would recurse forever.
    static HOOKS: RefCell<Vec<(u32, Box<[u8]>)>> = const { RefCell::new(Vec::new()) };
}

/// Whether a hook for this object / property is on the stack.
fn hook_held(id: u32, name: &[u8]) -> bool {
    HOOKS.with(|g| g.borrow().iter().any(|(i, n)| *i == id && n.as_ref() == name))
}

/// Whether the guard for this object / property / accessor is held.
fn guard_held(id: u32, name: &[u8], kind: Magic) -> bool {
    GUARDS.with(|g| {
        g.borrow()
            .iter()
            .any(|(i, n, k)| *i == id && *k == kind && n.as_ref() == name)
    })
}

/// How the calling scope sees a *declared* property.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Access {
    /// Readable and writable from here.
    Visible,
    /// Declared but not visible: the magic accessor, else
    /// `Cannot access <vis> property C::$p`.
    Hidden,
    /// A `private` property of an ancestor of the object's class. From
    /// anywhere but that ancestor php cannot even name it: the access behaves
    /// exactly as if the property did not exist (magic accessors, the
    /// `Undefined property` warning, and a *separate* dynamic property of the
    /// same name on assignment).
    Shadowed,
}

/// `ReflectionClass::SKIP_INITIALIZATION_ON_SERIALIZE`: `serialize()` does
/// not initialize the lazy object.
pub const LAZY_SKIP_INITIALIZATION_ON_SERIALIZE: i64 = 8;
/// `ReflectionClass::SKIP_DESTRUCTOR`: a `resetAsLazy*` does not run the
/// destructor on the state it discards.
pub const LAZY_SKIP_DESTRUCTOR: i64 = 16;

/// One property as `props_through_hooks` lists it: name, value, visibility
/// and declaring class (`None` for a dynamic property).
pub type PropEntry = (Box<[u8]>, Value, rphp_value::Vis, Option<u32>);

impl Interp {
    // ---- php 8.4 property hooks, for Reflection ------------------------------

    /// `ReflectionProperty::getValue()`: the `get` hook's answer when the
    /// property has one, `None` when the slot is the value.
    pub fn hooked_get(&mut self, o: &Object, name: &[u8]) -> Result<Option<Value>, Unwind> {
        let class = self.class_of(o).clone();
        let Some(p) = class.prop(name) else {
            return Ok(None);
        };
        let Some(h) = p.hooks else {
            return Ok(None);
        };
        match h.get {
            Some(g) => self.call_hook(o, p.decl, name, g, None),
            None => Ok(None),
        }
    }

    /// `ReflectionProperty::setValue()`: runs the `set` hook when there is
    /// one (answering `true`), else leaves the write to the caller.
    pub fn hooked_set(&mut self, o: &Object, name: &[u8], v: Value) -> Result<bool, Unwind> {
        let class = self.class_of(o).clone();
        let Some(p) = class.prop(name) else {
            return Ok(false);
        };
        let Some(h) = p.hooks else {
            return Ok(false);
        };
        match h.set {
            Some(setter) => Ok(self.call_hook(o, p.decl, name, setter, Some(v))?.is_some()),
            None => Ok(false),
        }
    }

    /// The properties `get_object_vars()` and `json_encode()` see: every
    /// declared property in declaration order — a hooked one through its
    /// `get` hook (virtual ones included), the rest from their slots
    /// (uninitialized ones skipped) — then the dynamic ones. Each with its
    /// visibility and declaring class (`None` for a dynamic property).
    pub fn props_through_hooks(&mut self, o: &Object) -> Result<Vec<PropEntry>, Unwind> {
        let class = self.class_of(o).clone();
        let mut out = Vec::new();
        // Each slot with its own value: two same-named private slots (an
        // ancestor's and a subclass's) are both here, in layout order.
        let slots: Vec<(Box<[u8]>, rphp_value::Vis, u32, bool, Value)> = o.with_data(|d| {
            d.layout()
                .props()
                .iter()
                .zip(d.slots().iter())
                .map(|(m, v)| (m.name.clone(), m.vis, m.decl_class, m.is_virtual, v.clone()))
                .collect()
        });
        for (name, vis, decl, is_virtual, value) in slots {
            let hooked = class
                .prop(&name)
                .filter(|p| p.decl == decl)
                .and_then(|p| p.hooks)
                .and_then(|h| h.get);
            if let Some(g) = hooked {
                if let Some(v) = self.call_hook(o, decl, &name, g, None)? {
                    out.push((name, v, vis, Some(decl)));
                    continue;
                }
            }
            if is_virtual || value.is_uninit() {
                continue;
            }
            out.push((name, value, vis, Some(decl)));
        }
        let dyns: Vec<(Box<[u8]>, Value)> = o.with_data(|d| {
            d.dyn_props()
                .map(|p| p.iter().map(|(n, v)| (Box::from(n), v.clone())).collect())
                .unwrap_or_default()
        });
        for (n, v) in dyns {
            out.push((n, v, rphp_value::Vis::Public, None));
        }
        // A native class's computed table (php's `get_properties`) comes
        // first, as the engine's own handlers run before the slots.
        if let Some(table) = self.native_property_table(o) {
            let mut native: Vec<PropEntry> = table
                .into_iter()
                .map(|(k, v)| {
                    let name: Box<[u8]> = match k {
                        rphp_value::ArrayKey::Int(i) => {
                            i.to_string().into_bytes().into_boxed_slice()
                        }
                        rphp_value::ArrayKey::Str(s) => s,
                    };
                    (name, v, rphp_value::Vis::Public, None)
                })
                .collect();
            native.extend(out);
            out = native;
        }
        Ok(out)
    }

    // ---- php 8.4 lazy objects -----------------------------------------------

    /// The object an access to `name` really touches. A lazy object that has
    /// not run its initializer runs it first — unless `name` was initialized
    /// ahead of time (`ReflectionProperty::skipLazyInitialization`) — and an
    /// initialized *proxy* hands over the real instance the factory built.
    /// A plain method call never comes through here, which is why one does
    /// not initialize a lazy object; the property accesses inside it do.
    pub(crate) fn lazy_target(&mut self, o: &Object, name: Option<&[u8]>) -> Result<Object, Unwind> {
        if !o.is_lazy() {
            return Ok(o.clone());
        }
        if o.lazy_needs_init(name) {
            self.lazy_initialize(o)?;
        }
        Ok(o.lazy_real().unwrap_or_else(|| o.clone()))
    }

    /// The object a whole-object operation (`foreach`, `clone`, `==`,
    /// `json_encode`, `serialize`, `get_object_vars`, …) really works on:
    /// a lazy object is initialized first, and an initialized proxy hands
    /// over its real instance.
    pub fn lazy_resolve(&mut self, o: &Object) -> Result<Object, Unwind> {
        self.lazy_target(o, None)
    }

    /// The declared slots' default values, in layout order — what
    /// `instantiate` seeds and `new_object` completes with the constant
    /// expressions, without allocating an object.
    pub fn default_slots(&mut self, class: u32) -> Result<Vec<Value>, Unwind> {
        let c = self.classes[class as usize].clone();
        let mut out = vec![Value::Uninit; c.layout.props().len()];
        for p in &c.props {
            out[usize::from(p.slot)] = match &p.default {
                crate::class::PropDefault::Value(v) => v.clone(),
                crate::class::PropDefault::Thunk(fid) => self.run_thunk(*fid, None, Some(p.decl))?,
            };
        }
        Ok(out)
    }

    /// Make `o` lazy (`ReflectionClass::newLazyGhost` / `newLazyProxy` /
    /// `resetAsLazyGhost` / `resetAsLazyProxy`). php refuses an instance of
    /// an internal class other than `stdClass`, or of a class inheriting
    /// one; a reset runs the destructor on the state being discarded unless
    /// `SKIP_DESTRUCTOR` says otherwise, and the object then starts a new
    /// lifecycle (its destructor runs again when it goes).
    pub fn make_lazy(
        &mut self,
        o: &Object,
        kind: rphp_value::LazyKind,
        initializer: Value,
        options: i64,
        reset: bool,
    ) -> Result<(), Unwind> {
        let cid = o.class_id();
        self.check_lazy_class(cid)?;
        if reset
            && options & LAZY_SKIP_DESTRUCTOR == 0
            && o.flags().contains(rphp_value::ObjFlags::HAS_DESTRUCTOR)
            && !o.flags().contains(rphp_value::ObjFlags::DESTRUCTED)
            && !o.lazy().is_some_and(|l| !l.initialized)
        {
            self.call_destructor(o)?;
            o.remove_flags(rphp_value::ObjFlags::DESTRUCTED);
        }
        // Laziness lives in the declared slots: a class without any has
        // nothing to defer, and php hands back an ordinary, initialized
        // object whose initializer never runs.
        if o.layout().props().is_empty() {
            o.set_lazy(rphp_value::LazyState::new(kind, Value::Null, options, Vec::new()));
            o.clear_lazy();
            return Ok(());
        }
        let defaults = self.default_slots(cid)?;
        o.set_lazy(rphp_value::LazyState::new(kind, initializer, options, defaults));
        Ok(())
    }

    /// php refuses to make an instance of an internal class other than
    /// `stdClass` lazy — or of a class that inherits one — before it looks
    /// at anything else about the class.
    pub fn check_lazy_class(&self, cid: u32) -> Result<(), Unwind> {
        let mut walk = Some(cid);
        while let Some(c) = walk {
            let def = &self.classes[c as usize];
            if def.internal && def.name.as_ref() != b"stdClass" {
                let own = self.classes[cid as usize].name_str();
                return Err(Unwind::error(if c == cid {
                    format!("Cannot make instance of internal class lazy: {own} is internal")
                } else {
                    format!(
                        "Cannot make instance of internal class lazy: {own} inherits internal class {}",
                        def.name_str()
                    )
                }));
            }
            walk = def.parent;
        }
        Ok(())
    }

    /// Run a lazy object's initializer (`ReflectionClass::initializeLazyObject`
    /// and every first access). The object counts as initialized *while* the
    /// initializer runs, so `$obj->__construct()` inside it does not recurse;
    /// a ghost's slots hold their defaults when it starts. An initializer
    /// that throws leaves the object lazy — a ghost's slots emptied again —
    /// as php does. Answers the object itself for a ghost and the real
    /// instance for a proxy.
    pub fn lazy_initialize(&mut self, o: &Object) -> Result<Object, Unwind> {
        let Some(state) = o.lazy() else {
            return Ok(o.clone());
        };
        if state.initialized {
            return Ok(o.lazy_real().unwrap_or_else(|| o.clone()));
        }
        o.with_lazy_mut(|l| l.initialized = true);
        let unmark = |o: &Object| {
            o.with_lazy_mut(|l| l.initialized = false);
        };
        if state.kind == rphp_value::LazyKind::Ghost {
            o.lazy_restore_defaults();
        }
        let result = self.call_value(&state.initializer, &[Value::Object(o.clone())]);
        match state.kind {
            rphp_value::LazyKind::Ghost => match result {
                Ok(v) if matches!(v, Value::Null) => {
                    // An initialized ghost is an ordinary object again.
                    o.clear_lazy();
                    Ok(o.clone())
                }
                Ok(_) => {
                    o.lazy_undef_pending();
                    unmark(o);
                    Err(Unwind::type_error(
                        "Lazy object initializer must return NULL or no value",
                    ))
                }
                Err(e) => {
                    o.lazy_undef_pending();
                    unmark(o);
                    Err(e)
                }
            },
            rphp_value::LazyKind::Proxy => match result {
                Ok(Value::Object(real)) => {
                    let (rc, pc) = (real.class_id(), o.class_id());
                    if !self.is_subclass_or_eq(rc, pc) {
                        unmark(o);
                        return Err(Unwind::type_error(format!(
                            "The real instance class {} is not compatible with the proxy class {}. The proxy must be a instance of the same class as the real instance, or a sub-class with no additional properties, and no overrides of the __destructor or __clone methods.",
                            self.classes[rc as usize].name_str(),
                            self.classes[pc as usize].name_str()
                        )));
                    }
                    o.with_lazy_mut(|l| {
                        l.real = Some(real.clone());
                        l.initializer = Value::Null;
                    });
                    Ok(real)
                }
                Ok(other) => {
                    unmark(o);
                    Err(Unwind::type_error(format!(
                        "Lazy proxy factory must return an instance of a class compatible with {}, {} returned",
                        self.classes[o.class_id() as usize].name_str(),
                        value_name(&other)
                    )))
                }
                Err(e) => {
                    unmark(o);
                    Err(e)
                }
            },
        }
    }

    // ---- entry points (`exec.rs` decodes the operand, we do the rest) -------

    /// `$obj->name` in a read context.
    pub(crate) fn fetch_prop(&mut self, obj: &Value, name: &[u8]) -> Result<Value, Unwind> {
        match &*obj.deref() {
            Value::Object(o) => {
                let o = self.lazy_target(&o.clone(), Some(name))?;
                self.read_prop(&o, name)
            }
            other => {
                let msg = format!(
                    "Attempt to read property \"{}\" on {}",
                    String::from_utf8_lossy(name),
                    value_name(other)
                );
                self.warn(&msg)?;
                Ok(Value::Null)
            }
        }
    }

    /// `$obj->name = v` with php's diagnostics.
    pub fn assign_prop(&mut self, obj: &Value, name: &[u8], v: Value) -> Result<(), Unwind> {
        match &*obj.deref() {
            Value::Object(o) => {
                let o = self.lazy_target(&o.clone(), Some(name))?;
                self.write_prop(&o, name, v)
            }
            other => Err(Unwind::error(format!(
                "Attempt to assign property \"{}\" on {}",
                String::from_utf8_lossy(name),
                value_name(other)
            ))),
        }
    }

    /// `isset($obj->name)`.
    pub(crate) fn isset_prop(&mut self, obj: &Value, name: &[u8]) -> Result<bool, Unwind> {
        let o = match &*obj.deref() {
            Value::Object(o) => self.lazy_target(&o.clone(), Some(name))?,
            _ => return Ok(false),
        };
        // A hooked property is `isset` when its `get` hook answers non-null.
        if let Some(v) = self.hooked_get(&o, name)? {
            return Ok(!matches!(v, Value::Null));
        }
        match self.stored_prop(&o, name) {
            Stored::Present(v) => Ok(!matches!(*v.deref(), Value::Null | Value::Uninit)),
            Stored::NotSet => Ok(false),
            Stored::Absent => {
                if let Some(np) = self.class_of(&o).native_props {
                    if let Some(f) = np.isset {
                        if let Some(b) = f(self, &o, name) {
                            return Ok(b);
                        }
                    }
                    if let Some(r) = (np.get)(self, &o, name) {
                        return Ok(!matches!(r?, Value::Null));
                    }
                }
                Ok(self.magic_isset(&o, name)?)
            }
        }
    }

    /// `empty($obj->name)`. php asks `__isset` whether there is a value at
    /// all and only then `__get` for it, so a class with `__isset` but no
    /// `__get` is always empty.
    pub(crate) fn empty_prop(&mut self, obj: &Value, name: &[u8]) -> Result<bool, Unwind> {
        let o = match &*obj.deref() {
            Value::Object(o) => self.lazy_target(&o.clone(), Some(name))?,
            _ => return Ok(true),
        };
        if let Some(v) = self.hooked_get(&o, name)? {
            return Ok(!v.to_bool());
        }
        match self.stored_prop(&o, name) {
            Stored::Present(v) => Ok(!v.deref().to_bool()),
            Stored::NotSet => Ok(true),
            Stored::Absent => {
                if let Some(np) = self.class_of(&o).native_props {
                    if let Some(f) = np.isset {
                        if f(self, &o, name) == Some(false) {
                            return Ok(true);
                        }
                    }
                    if let Some(r) = (np.get)(self, &o, name) {
                        return Ok(!r?.to_bool());
                    }
                }
                if !self.magic_isset(&o, name)? {
                    return Ok(true);
                }
                match self.call_magic(&o, Magic::Get, name, None)? {
                    Some(v) => Ok(!v.to_bool()),
                    None => Ok(true),
                }
            }
        }
    }

    /// `unset($obj->name)`. The guards mirror the write path, with php's
    /// `Cannot unset …` wording.
    pub(crate) fn unset_prop(&mut self, obj: &Value, name: &[u8]) -> Result<(), Unwind> {
        let o = match &*obj.deref() {
            Value::Object(o) => self.lazy_target(&o.clone(), Some(name))?,
            _ => return Ok(()),
        };
        let class = self.class_of(&o).clone();
        let scope = self.prop_scope();
        let key = self.prop_key(&class, name, scope);
        let name: &[u8] = &key;
        let Some(p) = class.prop(name) else {
            if let Some(f) = class.native_props.and_then(|np| np.unset) {
                if let Some(r) = f(self, &o, name) {
                    return r;
                }
            }
            return self.unset_dynamic(&o, name);
        };
        match self.access_of(&class, p, scope) {
            Access::Shadowed => self.unset_dynamic(&o, name),
            Access::Hidden => {
                if self.call_magic(&o, Magic::Unset, name, None)?.is_some() {
                    return Ok(());
                }
                Err(self.prop_access_error(&o, p.vis, name))
            }
            Access::Visible => {
                if p.hooks.is_some() {
                    return Err(Unwind::error(format!(
                        "Cannot unset hooked property {}::${}",
                        self.classes[p.decl as usize].name_str(),
                        String::from_utf8_lossy(name)
                    )));
                }
                let initialized = self.slot_initialized(&o, name);
                if p.readonly && initialized {
                    return Err(Unwind::error(format!(
                        "Cannot unset readonly property {}::${}",
                        self.classes[p.decl as usize].name_str(),
                        String::from_utf8_lossy(name)
                    )));
                }
                if let Some(e) = self.check_set_access(p, scope, "unset", name) {
                    return Err(e);
                }
                if !initialized
                    && p.ty.is_none()
                    && self.call_magic(&o, Magic::Unset, name, None)?.is_some()
                {
                    return Ok(());
                }
                o.unset(name);
                Ok(())
            }
        }
    }

    /// php 8.2: creating a dynamic property on a class without
    /// `#[AllowDynamicProperties]` is deprecated (`stdClass` is exempt).
    pub(crate) fn dynamic_prop_notice(&mut self, o: &Object, name: &[u8]) -> Result<(), Unwind> {
        if self.class_of(o).allows_dynamic_props() {
            return Ok(());
        }
        let class = self.class_name_of(o);
        self.deprecated(&format!(
            "Creation of dynamic property {class}::${} is deprecated",
            String::from_utf8_lossy(name)
        ))
    }

    /// The object in a register for a property write, or php's `Error`.
    /// php's `zend_readonly_property_indirect_modification_error`: a
    /// readonly property fetched for writing, by reference or for `unset`
    /// is refused whatever the scope and whether or not it is initialized —
    /// unless it holds an object, which is a handle (the object changes,
    /// the property does not). A `__clone` body may re-initialize.
    pub(crate) fn check_indirect_modify(&self, o: &Object, name: &[u8]) -> Result<(), Unwind> {
        let class = self.class_of(o).clone();
        let scope = self.prop_scope();
        let key = self.prop_key(&class, name, scope);
        let Some(p) = class.prop(&key) else {
            return Ok(());
        };
        if !p.readonly || crate::objects::in_clone_window(o.id()) {
            return Ok(());
        }
        if o.get(&key).is_some_and(|v| matches!(&*v.deref(), Value::Object(_))) {
            return Ok(());
        }
        Err(Unwind::error(format!(
            "Cannot indirectly modify readonly property {}::${}",
            self.classes[p.decl as usize].name_str(),
            String::from_utf8_lossy(name)
        )))
    }

    /// A native class's computed property table (php's `get_properties`),
    /// for the paths that walk an object's properties: `None` when the
    /// class computes none.
    pub fn native_property_table(
        &mut self,
        o: &Object,
    ) -> Option<Vec<(rphp_value::ArrayKey, Value)>> {
        let np = self.class_of(o).native_props?;
        let list = np.list?;
        Some(list(self, o))
    }

    /// What a dump shows for a native class (php's `get_debug_info`): the
    /// `debug` hook, else the property table, else the declared names.
    pub fn native_debug_table(&mut self, o: &Object) -> Option<Vec<(rphp_value::ArrayKey, Value)>> {
        let np = self.class_of(o).native_props?;
        if let Some(debug) = np.debug {
            return Some(debug(self, o));
        }
        if let Some(list) = np.list {
            return Some(list(self, o));
        }
        let mut out = Vec::new();
        for name in np.names {
            if let Some(Ok(v)) = (np.get)(self, o, name.as_bytes()) {
                out.push((rphp_value::ArrayKey::str(name.as_bytes()), v));
            }
        }
        Some(out)
    }

    pub(crate) fn prop_holder(&mut self, obj: &Value, name: &[u8]) -> Result<Object, Unwind> {
        match &*obj.deref() {
            Value::Object(o) => self.lazy_target(&o.clone(), Some(name)),
            other => Err(Unwind::error(format!(
                "Attempt to assign property \"{}\" on {}",
                String::from_utf8_lossy(name),
                value_name(other)
            ))),
        }
    }

    // ---- read ---------------------------------------------------------------

    fn read_prop(&mut self, o: &Object, name: &[u8]) -> Result<Value, Unwind> {
        let class = self.class_of(o).clone();
        let scope = self.prop_scope();
        let key = self.prop_key(&class, name, scope);
        let name: &[u8] = &key;
        match class.prop(name) {
            Some(p) => match self.access_of(&class, p, scope) {
                Access::Visible => {
                    // php 8.4 hooks: a `get` hook replaces the store read; a
                    // *virtual* property (no backing value) without one is
                    // write-only, while a backed set-only property reads
                    // its slot.
                    if let Some(h) = p.hooks {
                        let decl = p.decl;
                        if let Some(g) = h.get {
                            if let Some(v) = self.call_hook(o, decl, name, g, None)? {
                                return Ok(v);
                            }
                        } else if h.is_virtual && !hook_held(o.id(), name) {
                            return Err(Unwind::error(format!(
                                "Property {}::${} is write-only",
                                self.classes[decl as usize].name_str(),
                                String::from_utf8_lossy(name)
                            )));
                        }
                    }
                    if let Some(v) = o.get(name).map(|v| v.deref().into_owned()) {
                        if !v.is_uninit() {
                            return Ok(v);
                        }
                    }
                    if p.ty.is_some() {
                        // A typed slot starts out uninitialized; reading it
                        // before the first write is an `Error` even when the
                        // class has `__get`.
                        return Err(Unwind::error(format!(
                            "Typed property {}::${} must not be accessed before initialization",
                            self.classes[p.decl as usize].name_str(),
                            String::from_utf8_lossy(name)
                        )));
                    }
                    // An *untyped* slot is only uninitialized after `unset()`,
                    // and that is exactly when php hands the read to `__get`.
                    if let Some(v) = self.call_magic(o, Magic::Get, name, None)? {
                        return Ok(v);
                    }
                }
                Access::Hidden => {
                    if let Some(v) = self.call_magic(o, Magic::Get, name, None)? {
                        return Ok(v);
                    }
                    return Err(self.prop_access_error(o, p.vis, name));
                }
                Access::Shadowed => {
                    if let Some(v) = o.dyn_get(name) {
                        return Ok(v.deref().into_owned());
                    }
                    if let Some(v) = self.call_magic(o, Magic::Get, name, None)? {
                        return Ok(v);
                    }
                }
            },
            None => {
                // A native class's computed property (`$node->nodeName`).
                if let Some(np) = class.native_props {
                    if let Some(r) = (np.get)(self, o, name) {
                        return r;
                    }
                }
                if let Some(v) = o.dyn_get(name) {
                    return Ok(v.deref().into_owned());
                }
                if let Some(v) = self.call_magic(o, Magic::Get, name, None)? {
                    return Ok(v);
                }
            }
        }
        let msg = format!(
            "Undefined property: {}::${}",
            self.class_name_of(o),
            String::from_utf8_lossy(name)
        );
        self.warn(&msg)?;
        Ok(Value::Null)
    }

    // ---- write --------------------------------------------------------------

    /// The write path. php's order of judgement, established with `php -r`:
    /// visibility of the *read* name first (an invisible property is `__set`'s
    /// business), then the readonly re-initialization check, then the `set`
    /// visibility, then `__set` for a slot that `unset()` emptied, and only
    /// then the declared type.
    ///
    /// **Property hooks belong here**: a `set` hook would be called instead of
    /// the store below (and a get-only hooked property would be
    /// `Error: Property C::$p is read-only`), with a bypass guard so that the
    /// hook body's own `$this->p = …` reaches the backing store. The runtime
    /// cannot see them: `PropInfo` carries no `hooks` field.
    fn write_prop(&mut self, o: &Object, name: &[u8], v: Value) -> Result<(), Unwind> {
        let class = self.class_of(o).clone();
        let scope = self.prop_scope();
        let key = self.prop_key(&class, name, scope);
        let name: &[u8] = &key;
        let Some(p) = class.prop(name) else {
            if let Some(np) = class.native_props {
                if let Some(r) = (np.set)(self, o, name, v.clone()) {
                    return r;
                }
            }
            return self.write_dynamic(o, name, v);
        };
        match self.access_of(&class, p, scope) {
            Access::Shadowed => return self.write_dynamic(o, name, v),
            Access::Hidden => {
                if self.call_magic(o, Magic::Set, name, Some(v))?.is_some() {
                    return Ok(());
                }
                return Err(self.prop_access_error(o, p.vis, name));
            }
            Access::Visible => {}
        }
        // php 8.4 hooks: a `set` hook replaces the store write; a *virtual*
        // property without one is read-only, while a backed get-only
        // property writes its slot. Inside the hook body the bypass
        // (`hook_held`) lets `$this->p = …` reach the slot.
        if let Some(h) = p.hooks {
            let decl = p.decl;
            if let Some(setter) = h.set {
                if self.call_hook(o, decl, name, setter, Some(v.clone()))?.is_some() {
                    return Ok(());
                }
            } else if h.is_virtual && !hook_held(o.id(), name) {
                return Err(Unwind::error(format!(
                    "Property {}::${} is read-only",
                    self.classes[decl as usize].name_str(),
                    String::from_utf8_lossy(name)
                )));
            }
        }
        let initialized = self.slot_initialized(o, name);
        // php 8.3: a `__clone` body (and php 8.5's `clone(..., [...])`
        // replacements) may re-initialize the copy's readonly slots. The
        // window is open only for the object being cloned (`objects.rs`).
        if p.readonly && initialized && !crate::objects::in_clone_window(o.id()) {
            return Err(Unwind::error(format!(
                "Cannot modify readonly property {}::${}",
                self.classes[p.decl as usize].name_str(),
                String::from_utf8_lossy(name)
            )));
        }
        if let Some(e) = self.check_set_access(p, scope, "modify", name) {
            return Err(e);
        }
        if !initialized && p.ty.is_none() {
            // Untyped and uninitialized means `unset()` emptied the slot, and
            // php sends the write to `__set`.
            if self
                .call_magic(o, Magic::Set, name, Some(v.clone()))?
                .is_some()
            {
                return Ok(());
            }
        }
        let v = match &p.ty {
            Some(ty) => {
                let ty = ty.clone();
                let decl = p.decl;
                self.coerce_prop(o, &ty, decl, name, v)?
            }
            None if v.is_ref() => v.deref().into_owned(),
            None => v,
        };
        o.set(name, v);
        Ok(())
    }

    /// A name the class does not declare (or a `Shadowed` one): `__set`, else
    /// php's 8.2 deprecation and a dynamic property. An *existing* dynamic
    /// property is written straight through — php only consults `__set` when
    /// there is nothing under the name.
    fn write_dynamic(&mut self, o: &Object, name: &[u8], v: Value) -> Result<(), Unwind> {
        if o.dyn_get(name).is_none() {
            if self
                .call_magic(o, Magic::Set, name, Some(v.clone()))?
                .is_some()
            {
                return Ok(());
            }
            self.dynamic_prop_notice(o, name)?;
        }
        o.dyn_set(name, v);
        Ok(())
    }

    fn unset_dynamic(&mut self, o: &Object, name: &[u8]) -> Result<(), Unwind> {
        if o.dyn_get(name).is_some() {
            o.dyn_unset(name);
            return Ok(());
        }
        self.call_magic(o, Magic::Unset, name, None)?;
        Ok(())
    }

    /// Check / coerce an assigned value against a declared property type,
    /// under the **assigning** frame's `strict_types` (php checks the writer's
    /// declaration, not the declaring unit's). `self`/`static` in the type
    /// resolve against the declaring class and the object's runtime class.
    fn coerce_prop(
        &mut self,
        o: &Object,
        ty: &TypeDecl,
        decl: u32,
        name: &[u8],
        v: Value,
    ) -> Result<Value, Unwind> {
        let strict = self.current_user_frame().is_some_and(|f| f.strict);
        let scope = Some(decl);
        let static_class = Some(o.class_id());
        let given = v.deref().into_owned();
        match self.coerce_to_type(given.clone(), ty, strict, scope, static_class)? {
            Coerced::Ok(nv) => Ok(nv),
            Coerced::Mismatch => Err(Unwind::type_error(format!(
                "Cannot assign {} to property {}::${} of type {}",
                value_name(&given),
                self.classes[decl as usize].name_str(),
                String::from_utf8_lossy(name),
                self.type_display(ty, scope)
            ))),
        }
    }

    // ---- shared classification ----------------------------------------------

    /// The scope property visibility is judged against: the innermost frame
    /// that runs bytecode (a native in between does not change the scope).
    fn prop_scope(&self) -> Option<u32> {
        self.current_user_frame().and_then(|f| f.scope)
    }

    /// The storage key `$o->name` resolves to from the calling scope: an
    /// ancestor's private property a subclass re-declared is the mangled
    /// slot when the ancestor's own method is the one writing.
    pub(crate) fn prop_storage_key(&self, o: &Object, name: &[u8]) -> Vec<u8> {
        let class = self.class_of(o);
        self.prop_key(class, name, self.prop_scope()).into_owned()
    }

    /// The storage key `name` resolves to from `scope`.
    ///
    /// An ancestor's private property that a subclass re-declared lives
    /// under php's mangled key (`Layout::new`), and only that ancestor's own
    /// code reaches it by the plain name — every other scope's `name` is the
    /// subclass's slot. The mangled entry exists only where a re-declaration
    /// happened, so this is one hash miss for everyone else.
    fn prop_key<'n>(&self, class: &ClassDef, name: &'n [u8], scope: Option<u32>) -> std::borrow::Cow<'n, [u8]> {
        if let Some(s) = scope {
            if s != class.id && !class.prop_index.is_empty() {
                let decl_name = &self.classes[s as usize].name;
                let mangled = rphp_value::mangled_key(decl_name, name);
                if class.prop_index.contains_key(&mangled) {
                    return std::borrow::Cow::Owned(mangled.into_vec());
                }
            }
        }
        std::borrow::Cow::Borrowed(name)
    }

    /// How the calling scope sees a declared property (see [`Access`]).
    fn access_of(&self, class: &ClassDef, p: &PropInfo, scope: Option<u32>) -> Access {
        if self.access_ok(p.vis, p.decl, scope) {
            Access::Visible
        } else if p.vis == Visibility::Private && p.decl != class.id {
            Access::Shadowed
        } else {
            Access::Hidden
        }
    }

    /// Whether the declared slot currently holds a value (php's
    /// "initialized"). A typed slot starts out `Uninit`; `unset()` puts any
    /// slot back into that state.
    fn slot_initialized(&self, o: &Object, name: &[u8]) -> bool {
        o.get(name).is_some_and(|v| !v.deref().is_uninit())
    }

    /// The value `$obj->name` finds without consulting any magic accessor.
    fn stored_prop(&self, o: &Object, name: &[u8]) -> Stored {
        let class = self.class_of(o).clone();
        let scope = self.prop_scope();
        let key = self.prop_key(&class, name, scope);
        let name: &[u8] = &key;
        match class.prop(name) {
            Some(p) => match self.access_of(&class, p, scope) {
                Access::Visible => match o.get(name) {
                    Some(v) if !v.deref().is_uninit() => Stored::Present(v),
                    // A typed slot that was never written is simply not set —
                    // php does not consult `__isset` for it.
                    _ if p.ty.is_some() => Stored::NotSet,
                    _ => Stored::Absent,
                },
                Access::Hidden => Stored::Absent,
                Access::Shadowed => match o.dyn_get(name) {
                    Some(v) => Stored::Present(v),
                    None => Stored::Absent,
                },
            },
            None => match o.dyn_get(name) {
                Some(v) => Stored::Present(v),
                None => Stored::Absent,
            },
        }
    }

    /// The `set` visibility of a property (php 8.4 asymmetric visibility).
    ///
    /// `readonly` implies `protected(set)` — which is why a subclass may
    /// initialize an inherited readonly property but global code may not.
    ///
    /// **Missing data**: an explicitly declared `private(set)`/`protected(set)`
    /// cannot be honoured, because `PropInfo` has no `set_vis` field (nor does
    /// the `ClassSpec::props` tuple that builds it). With the field this
    /// becomes `p.set_vis.unwrap_or(implicit)`.
    fn set_visibility(p: &PropInfo) -> Visibility {
        // An explicit `private(set)`/`protected(set)` wins; otherwise
        // `readonly` implies `protected(set)`, which is why a subclass may
        // initialize an inherited readonly property but global code may not.
        match p.set_vis {
            Some(v) => v,
            None if p.readonly => Visibility::Protected,
            None => p.vis,
        }
    }

    /// The `set`-visibility gate. `None` when the write is allowed; the read
    /// visibility has already been checked, so this only ever fires for a
    /// property whose `set` visibility is *narrower* than its read visibility.
    fn check_set_access(
        &self,
        p: &PropInfo,
        scope: Option<u32>,
        verb: &str,
        name: &[u8],
    ) -> Option<Unwind> {
        let sv = Self::set_visibility(p);
        if sv == p.vis || self.access_ok(sv, p.decl, scope) {
            return None;
        }
        Some(Unwind::error(format!(
            "Cannot {verb} {}(set) {}property {}::${} from {}",
            vis_word(sv),
            if p.readonly { "readonly " } else { "" },
            self.classes[p.decl as usize].name_str(),
            String::from_utf8_lossy(name),
            match scope {
                Some(c) => format!("scope {}", self.classes[c as usize].name_str()),
                None => "global scope".to_string(),
            }
        )))
    }

    /// php's `Cannot access <vis> property C::$p`. The class named is the
    /// object's *runtime* class, not the declaring one (`protected` inherited
    /// from a parent still reports the child).
    fn prop_access_error(&self, o: &Object, vis: Visibility, name: &[u8]) -> Unwind {
        Unwind::error(format!(
            "Cannot access {} property {}::${}",
            vis_word(vis),
            self.class_name_of(o),
            String::from_utf8_lossy(name)
        ))
    }

    // ---- the magic accessors -------------------------------------------------

    /// `__isset`, defaulting to "not set" when the class has none.
    fn magic_isset(&mut self, o: &Object, name: &[u8]) -> Result<bool, Unwind> {
        Ok(self
            .call_magic(o, Magic::Isset, name, None)?
            .is_some_and(|v| v.to_bool()))
    }

    /// Call one of the four accessors, honouring php's recursion guard.
    /// `Ok(None)` means the class does not define it, or the guard for this
    /// object + property + accessor is already held — in both cases the caller
    /// falls through to the plain behaviour.
    /// Run a property hook (`get` or `set`) with `$this` bound and the
    /// declaring class as the scope. Returns `None` when the hook is already
    /// on the stack for this object+property, which is php's bypass: the
    /// caller then falls through to the backing store.
    fn call_hook(
        &mut self,
        o: &Object,
        p_decl: u32,
        name: &[u8],
        func_id: u32,
        arg: Option<Value>,
    ) -> Result<Option<Value>, Unwind> {
        if hook_held(o.id(), name) {
            return Ok(None);
        }
        let func = self.funcs[func_id as usize].clone();
        let args: Vec<Value> = arg.into_iter().collect();
        HOOKS.with(|g| g.borrow_mut().push((o.id(), Box::from(name))));
        let r = self.call_user_func(
            func,
            Some(o.clone()),
            Some(p_decl),
            Some(o.class_id()),
            None,
            &args,
        );
        HOOKS.with(|g| {
            g.borrow_mut().pop();
        });
        r.map(Some)
    }

    fn call_magic(
        &mut self,
        o: &Object,
        kind: Magic,
        name: &[u8],
        extra: Option<Value>,
    ) -> Result<Option<Value>, Unwind> {
        if !self.class_of(o).magic.contains(kind.flag()) || guard_held(o.id(), name, kind) {
            return Ok(None);
        }
        let mut args = vec![Value::string(name)];
        args.extend(extra);
        GUARDS.with(|g| g.borrow_mut().push((o.id(), Box::from(name), kind)));
        let r = self.call_method(o, kind.method(), &args);
        GUARDS.with(|g| {
            g.borrow_mut().pop();
        });
        r.map(Some)
    }
}

/// What a property lookup finds before any magic accessor is consulted.
enum Stored {
    /// A value is stored under the name (possibly `null`, possibly a `Ref`).
    Present(Value),
    /// The property exists and is visible but holds nothing, and php does
    /// *not* fall through to the accessors (a typed slot before its first
    /// write).
    NotSet,
    /// Nothing under the name here: the accessors decide.
    Absent,
}
