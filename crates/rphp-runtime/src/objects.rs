//! Object lifetime operations (plan E6): `clone`, the object/array casts and
//! the `stdClass` conversions.
//!
//! **This module owns `clone`.** `exec.rs` decodes `Op::Clone` and calls
//! [`Interp::clone_object`]; everything about what a copy *is* lives here.
//!
//! What lands here as E6 fills in:
//!
//! * `__clone`, called on the copy after the slots are duplicated (and *not*
//!   on the original), including the inherited/native cases;
//! * `readonly` properties, which a `__clone` body may re-initialize once
//!   (php 8.3): the copy enters `__clone` with its readonly slots unfrozen
//!   and they re-freeze when it returns;
//! * php 8.5 `clone($o, ['prop' => $v])`, which applies the replacements
//!   after the copy and before `__clone`;
//! * cloning an object with a native payload, through the class's
//!   `payload_clone` hook, so SPL/DateTime-style objects deep-copy correctly;
//! * the `(object)` and `(array)` casts, with php's mangled keys for private
//!   (`\0Class\0prop`) and protected (`\0*\0prop`) properties.
//!
//! The current body is the pre-E6 behaviour: a shallow slot + dynamic
//! property copy that re-registers a destructor.

use rphp_value::{Object, Value};

use crate::class::MagicFlags;
use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    /// `clone $obj` — a shallow copy of `obj` with a fresh object id.
    ///
    /// E6 turns this into php's full protocol (`__clone`, readonly re-init,
    /// native payload clone).
    pub(crate) fn clone_object(&mut self, obj: &Value) -> Result<Value, Unwind> {
        let Value::Object(o) = &*obj.deref() else {
            return Err(Unwind::error("__clone method called on non-object"));
        };
        let o = o.clone();
        let id = self.object_ids.alloc();
        let layout = o.layout();
        let slots = o.with_data(|d| d.slots().to_vec());
        let copy = Object::new(o.class_id(), id, layout, slots);
        if self.class_of(&o).magic.contains(MagicFlags::DESTRUCT) {
            copy.add_flags(rphp_value::ObjFlags::HAS_DESTRUCTOR);
            self.destructibles.push(copy.downgrade());
        }
        let dyns: Vec<(Box<[u8]>, Value)> = o.with_data(|d| {
            d.dyn_props()
                .map(|p| p.iter().map(|(n, v)| (Box::from(n), v.clone())).collect())
                .unwrap_or_default()
        });
        for (n, v) in dyns {
            copy.dyn_set(&n, v);
        }
        Ok(Value::Object(copy))
    }
}
