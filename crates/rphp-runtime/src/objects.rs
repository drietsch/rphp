//! Object lifetime operations (plan E6): `clone`, the object/array casts and
//! the `stdClass` conversions.
//!
//! **This module owns `clone`.** `exec.rs` decodes `Op::Clone` and calls
//! [`Interp::clone_object`] (or [`Interp::clone_object_with`] when the op
//! carries a `with` register); everything about what a copy *is* lives here.
//!
//! php's protocol, in the order `php -r` shows it happening:
//!
//! 1. the operand must be an object, else
//!    `TypeError: clone(): Argument #1 ($object) must be of type object, int given`;
//! 2. `__clone` must be **visible** from the calling scope, else
//!    `Error: Call to private method C::__clone() from global scope` — php
//!    raises that before anything is copied;
//! 3. the slots and the dynamic properties are copied into a fresh instance
//!    with a new object id (a destructor is re-registered for the copy);
//! 4. `__clone` runs on the **copy** — never on the original;
//! 5. php 8.5's `clone($o, ['p' => $v])` replacements are applied **after**
//!    `__clone`, as ordinary property writes judged from the calling scope
//!    (`clone($s, ['p' => 1])` on a `private $p` from outside is
//!    `Error: Cannot access private property S::$p`, and an undeclared name
//!    is the 8.2 dynamic-property deprecation).
//!
//! Steps 4 and 5 run inside the copy's **clone window** ([`in_clone_window`]):
//! php 8.3 lets a `__clone` body re-initialize `readonly` properties, and 8.5
//! extends that to the `clone(…, […])` replacements, which is how
//! `clone($this, ['v' => $n])` produces a modified copy of a readonly value
//! object. The slots freeze again when the window closes.
//!
//! ## The casts
//!
//! `(object)` and `(array)` are decoded by `Op::Cast` and implemented by
//! [`Interp::cast`](crate::Interp::cast) / `Interp::object_to_array` in
//! `ops.rs`, which already produce php's mangled keys — `\0Class\0prop` for
//! private, `\0*\0prop` for protected — skip `Uninit` slots, round-trip
//! `stdClass`, and match php on `(object)` of a scalar (`->scalar`), of null
//! (an empty `stdClass`) and of an object (identity). That was re-verified
//! against php 8.5 for this phase and left where it is rather than
//! duplicated here; the one divergence found is in the *formatter*, not the
//! cast: `var_dump((object)(array)$objWithPrivate)` prints the mangled key
//! raw instead of php's `["p":"W":private]`, because a `stdClass` dynamic
//! property carries no declaring class.

use std::cell::RefCell;

use rphp_value::{Object, Value};

use crate::class::MagicFlags;
use crate::ops::value_name;
use crate::registry::Unwind;
use crate::Interp;

thread_local! {
    /// The object ids whose `clone` is in progress, innermost last. A stack,
    /// so a `__clone` body that clones another object nests correctly and an
    /// unwinding `__clone` can never leave a window open.
    static CLONING: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// Whether object `id` is inside its clone window — between the copy being
/// made and the `clone` expression finishing.
///
/// **This is the readonly re-initialization gate** (php 8.3): the write path
/// in `props.rs` must let a `readonly` property that already holds a value be
/// written while its object's window is open, i.e.
/// `if p.readonly && initialized && !crate::objects::in_clone_window(o.id())`.
pub(crate) fn in_clone_window(id: u32) -> bool {
    CLONING.with(|c| c.borrow().contains(&id))
}

/// Opens the clone window for one object and closes it on drop, so an
/// unwinding `__clone` cannot leave a readonly property writable.
struct CloneWindow(u32);

impl CloneWindow {
    fn open(id: u32) -> CloneWindow {
        CLONING.with(|c| c.borrow_mut().push(id));
        CloneWindow(id)
    }
}

impl Drop for CloneWindow {
    fn drop(&mut self) {
        CLONING.with(|c| {
            let mut v = c.borrow_mut();
            if let Some(i) = v.iter().rposition(|&x| x == self.0) {
                v.remove(i);
            }
        });
    }
}

impl Interp {
    /// `clone $obj` — php's full protocol (see the module header).
    pub fn clone_object_with(
        &mut self,
        obj: &Value,
        with: Option<&Value>,
    ) -> Result<Value, Unwind> {
        let o = match obj.deref().into_owned() {
            Value::Object(o) => o,
            // A closure clones to a new closure object over the same body
            // and captures.
            Value::Closure(c) => {
                return Ok(Value::Closure(self.new_closure(c.func(), c.captures().to_vec())));
            }
            other => {
                return Err(Unwind::type_error(format!(
                    "clone(): Argument #1 ($object) must be of type object, {} given",
                    value_name(&other)
                )))
            }
        };
        // php refuses to clone an enum case: the cases are singletons.
        if o.flags().contains(rphp_value::ObjFlags::ENUM_CASE) {
            return Err(Unwind::error(format!(
                "Trying to clone an uncloneable object of class {}",
                self.class_name_of(&o)
            )));
        }
        // php 8.4: a lazy object initializes before it is cloned, and a
        // proxy's clone is a new proxy over a clone of the real instance.
        if o.is_lazy() {
            let target = self.lazy_resolve(&o)?;
            if let Some(real) = o.lazy_real() {
                let Value::Object(real_copy) = self.clone_object_with(&Value::Object(real), with)? else {
                    unreachable!("clone answers an object")
                };
                let copy = self.shallow_copy(&o)?;
                let options = o.lazy().map_or(0, |l| l.options);
                let mut state = rphp_value::LazyState::new(
                    rphp_value::LazyKind::Proxy,
                    Value::Null,
                    options,
                    Vec::new(),
                );
                state.initialized = true;
                state.real = Some(real_copy);
                copy.set_lazy(state);
                return Ok(Value::Object(copy));
            }
            debug_assert!(target.ptr_eq(&o));
        }
        let class = self.class_of(&o).clone();
        // php checks `__clone`'s visibility before it copies anything.
        if let Some(m) = self.resolve_method(class.id, b"__clone") {
            self.check_method_access(m.vis, m.decl, b"__clone")?;
        }
        let copy = self.shallow_copy(&o)?;
        let window = CloneWindow::open(copy.id());
        let r = self.run_clone_body(&copy, class.magic, with);
        drop(window);
        r?;
        Ok(Value::Object(copy))
    }

    /// `__clone` on the copy, then the php 8.5 replacements — both inside the
    /// copy's clone window, so both may re-initialize `readonly` slots.
    fn run_clone_body(
        &mut self,
        copy: &Object,
        magic: MagicFlags,
        with: Option<&Value>,
    ) -> Result<(), Unwind> {
        if magic.contains(MagicFlags::CLONE) {
            self.call_method(copy, b"__clone", &[])?;
        }
        let Some(with) = with else { return Ok(()) };
        let a = match with.deref().into_owned() {
            Value::Array(a) => a,
            Value::Null | Value::Uninit => return Ok(()),
            other => {
                return Err(Unwind::type_error(format!(
                    "clone(): Argument #2 ($withProperties) must be of type array, {} given",
                    value_name(&other)
                )))
            }
        };
        let holder = Value::Object(copy.clone());
        for (k, v) in a.iter() {
            let name: Vec<u8> = match k {
                rphp_value::ArrayKey::Int(i) => i.to_string().into_bytes(),
                rphp_value::ArrayKey::Str(s) => s.to_vec(),
            };
            let v = v.deref().into_owned();
            self.assign_prop(&holder, &name, v)?;
        }
        Ok(())
    }

    /// A fresh instance of `o`'s class holding copies of its slots and
    /// dynamic properties, with a new object id and its own destructor
    /// registration.
    ///
    /// **A native payload is not copied.** `ClassDef` has no clone hook, so
    /// an object whose engine-side state lives in `Payload` (`WeakMap`,
    /// `SplObjectStorage`, the SPL containers) clones to an empty payload —
    /// see the module report for the hook this needs.
    fn shallow_copy(&mut self, o: &Object) -> Result<Object, Unwind> {
        let id = self.object_ids.alloc();
        let layout = o.layout();
        // php's `zend_objects_clone_members`: a property that is a reference
        // nobody else holds (the cell a by-reference call such as
        // `end($this->list)` left behind) is copied as a value, so the clone
        // and the original stop sharing it.
        let unwrap = |v: &Value| match v {
            Value::Ref(r) if r.strong_count() == 1 => r.get(),
            other => other.clone(),
        };
        let slots = o.with_data(|d| d.slots().iter().map(unwrap).collect());
        let copy = Object::new(o.class_id(), id, layout, slots);
        copy.copy_unset_marks_from(o);
        if self.class_of(o).magic.contains(MagicFlags::DESTRUCT) {
            copy.add_flags(rphp_value::ObjFlags::HAS_DESTRUCTOR);
            self.destructibles.push(copy.downgrade());
        }
        let dyns: Vec<(Box<[u8]>, Value)> = o.with_data(|d| {
            d.dyn_props()
                .map(|p| p.iter().map(|(n, v)| (Box::from(n), unwrap(v))).collect())
                .unwrap_or_default()
        });
        for (n, v) in dyns {
            // A shared reference stays shared with the clone, as php's does.
            match v {
                Value::Ref(r) => copy.dyn_set_ref(&n, r),
                v => copy.dyn_set(&n, v),
            }
        }
        // A class whose state lives in a native payload copies it here;
        // without the hook the copy would silently start empty.
        if let Some(hook) = self.class_of(o).payload_clone {
            hook(self, o, &copy)?;
        }
        Ok(copy)
    }
}

// ---- ArrayAccess (E6 object protocols) -------------------------------------
//
// `$o[$k]` on an object implementing `ArrayAccess` dispatches to
// `offsetGet`/`offsetSet`/`offsetExists`/`offsetUnset`. php's rules, probed
// against 8.5:
//
// * `$o[] = $v` calls `offsetSet(null, $v)`;
// * `isset($o[$k])` is `offsetExists($k)` alone, but `empty($o[$k])` is
//   `offsetExists($k)` *and then* `offsetGet($k)` when it returned true;
// * an object that does NOT implement the interface is
//   `Cannot use object of type C as array`, which is what the callers already
//   raise — these helpers only take over when `is_array_access` is true.

impl Interp {
    /// Whether `o`'s class implements `ArrayAccess`.
    pub(crate) fn is_array_access(&self, o: &Object) -> bool {
        self.well_known
            .array_access
            .is_some_and(|c| self.object_instanceof(o, c))
    }

    /// `$o[$key]` → `offsetGet($key)`.
    pub(crate) fn offset_get(&mut self, o: &Object, key: &Value) -> Result<Value, Unwind> {
        let o = o.clone();
        self.call_method(&o, b"offsetGet", std::slice::from_ref(key))
    }

    /// `$o[$key] = $v` → `offsetSet($key, $v)`; `$o[] = $v` passes `null`.
    pub(crate) fn offset_set(
        &mut self,
        o: &Object,
        key: Option<&Value>,
        v: Value,
    ) -> Result<(), Unwind> {
        let o = o.clone();
        let k = key.cloned().unwrap_or(Value::Null);
        self.call_method(&o, b"offsetSet", &[k, v])?;
        Ok(())
    }

    /// `isset($o[$key])` → `offsetExists($key)`.
    pub(crate) fn offset_exists(&mut self, o: &Object, key: &Value) -> Result<bool, Unwind> {
        let o = o.clone();
        Ok(self.call_method(&o, b"offsetExists", std::slice::from_ref(key))?.to_bool())
    }

    /// `empty($o[$key])` — php asks `offsetExists` first and only then reads.
    pub(crate) fn offset_empty(&mut self, o: &Object, key: &Value) -> Result<bool, Unwind> {
        if !self.offset_exists(o, key)? {
            return Ok(true);
        }
        Ok(!self.offset_get(o, key)?.to_bool())
    }

    /// `unset($o[$key])` → `offsetUnset($key)`.
    pub(crate) fn offset_unset(&mut self, o: &Object, key: &Value) -> Result<(), Unwind> {
        let o = o.clone();
        self.call_method(&o, b"offsetUnset", std::slice::from_ref(key))?;
        Ok(())
    }
}
