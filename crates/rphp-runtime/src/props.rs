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

impl Interp {
    // ---- entry points (`exec.rs` decodes the operand, we do the rest) -------

    /// `$obj->name` in a read context.
    pub(crate) fn fetch_prop(&mut self, obj: &Value, name: &[u8]) -> Result<Value, Unwind> {
        match &*obj.deref() {
            Value::Object(o) => {
                let o = o.clone();
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
    pub(crate) fn assign_prop(&mut self, obj: &Value, name: &[u8], v: Value) -> Result<(), Unwind> {
        match &*obj.deref() {
            Value::Object(o) => {
                let o = o.clone();
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
            Value::Object(o) => o.clone(),
            _ => return Ok(false),
        };
        match self.stored_prop(&o, name) {
            Stored::Present(v) => Ok(!matches!(*v.deref(), Value::Null | Value::Uninit)),
            Stored::NotSet => Ok(false),
            Stored::Absent => Ok(self.magic_isset(&o, name)?),
        }
    }

    /// `empty($obj->name)`. php asks `__isset` whether there is a value at
    /// all and only then `__get` for it, so a class with `__isset` but no
    /// `__get` is always empty.
    pub(crate) fn empty_prop(&mut self, obj: &Value, name: &[u8]) -> Result<bool, Unwind> {
        let o = match &*obj.deref() {
            Value::Object(o) => o.clone(),
            _ => return Ok(true),
        };
        match self.stored_prop(&o, name) {
            Stored::Present(v) => Ok(!v.deref().to_bool()),
            Stored::NotSet => Ok(true),
            Stored::Absent => {
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
            Value::Object(o) => o.clone(),
            _ => return Ok(()),
        };
        let class = self.class_of(&o).clone();
        let scope = self.prop_scope();
        let Some(p) = class.prop(name) else {
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
    pub(crate) fn prop_holder(&self, obj: &Value, name: &[u8]) -> Result<Object, Unwind> {
        match &*obj.deref() {
            Value::Object(o) => Ok(o.clone()),
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
        match class.prop(name) {
            Some(p) => match self.access_of(&class, p, scope) {
                Access::Visible => {
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
        let Some(p) = class.prop(name) else {
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
        if p.readonly {
            Visibility::Protected
        } else {
            p.vis
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
