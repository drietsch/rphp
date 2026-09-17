//! Property access (plan E6): the read/write/isset/unset paths behind
//! `FetchProp`/`AssignProp`/`IssetProp`/`UnsetProp` and the object protocols
//! layered on them.
//!
//! **This module owns everything about reading and writing an object
//! property.** `exec.rs` only decodes the operand and calls in here.
//!
//! What lands here as E6 fills in:
//!
//! * the magic hooks — `__get`/`__set`/`__isset`/`__unset`, consulted when a
//!   declared property is not visible from the calling scope or does not
//!   exist, with php's recursion guard (a magic method re-entering on the
//!   same object + property falls through to the plain behaviour);
//! * declared property **types**, coerced on assignment through
//!   [`crate::types`] under the assigning scope's `strict_types`, and the
//!   `Uninit` ("must not be accessed before initialization") error for a
//!   typed property with no default;
//! * `readonly` — initialization allowed once, from inside the declaring
//!   scope only; a second write is an `Error`;
//! * **asymmetric visibility** (`public private(set)`), where the `set`
//!   visibility is checked against the writing scope;
//! * **property hooks** (`get`/`set`), which turn a property access into a
//!   call of the hook method, with the hook-recursion bypass that lets a hook
//!   body touch the backing store.
//!
//! The current bodies are the pre-E6 behaviour: declared slot or dynamic
//! property, with php's undefined-property warning and dynamic-property
//! deprecation.

use rphp_value::{Object, Value};

use crate::ops::value_name;
use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    pub(crate) fn fetch_prop(&mut self, obj: &Value, name: &[u8]) -> Result<Value, Unwind> {
        match &*obj.deref() {
            Value::Object(o) => {
                self.check_prop_access(o.class_id(), name)?;
                match o.get_deref(name) {
                    Some(v) if !v.is_uninit() => Ok(v),
                    _ => {
                        let msg = format!(
                            "Undefined property: {}::${}",
                            self.class_name_of(o),
                            String::from_utf8_lossy(name)
                        );
                        self.warn(&msg)?;
                        Ok(Value::Null)
                    }
                }
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

    /// `obj->name = v` with php's diagnostics (dynamic-property deprecation).
    pub(crate) fn assign_prop(&mut self, obj: &Value, name: &[u8], v: Value) -> Result<(), Unwind> {
        match &*obj.deref() {
            Value::Object(o) => {
                self.check_prop_access(o.class_id(), name)?;
                if o.get(name).is_none() {
                    self.dynamic_prop_notice(o, name)?;
                }
                o.set(name, v);
                Ok(())
            }
            other => Err(Unwind::error(format!(
                "Attempt to assign property \"{}\" on {}",
                String::from_utf8_lossy(name),
                value_name(other)
            ))),
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
}
