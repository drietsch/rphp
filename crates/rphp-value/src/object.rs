//! An object value: an instance of a class plus its property bag.
//!
//! Unlike [`crate::Array`] (a copy-on-write value type), PHP objects have
//! **reference** semantics: a `$b = $a` aliases the same instance, and a method
//! mutating `$this->x` is visible through every handle. We model that with a
//! shared, interior-mutable cell (`Rc<RefCell<…>>`); cloning a [`Value::Object`]
//! is a refcount bump onto the same instance, and `===` is pointer identity
//! (matching PHP, where two distinct objects are never identical).
//!
//! `rphp-value` sits below `rphp-bytecode`, so the class is stored as an opaque
//! `u32` id (the runtime interprets it as a `ClassId`). Properties are an
//! insertion-ordered list of `(name, value)` pairs — objects carry few
//! properties, so a linear scan is cheaper than a map and preserves the
//! declaration/insertion order PHP exposes (e.g. to `foreach`/`var_dump`).
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::Value;

/// Property visibility, carried on each instance slot so the value formatters
/// (`json_encode` emits public only; `var_dump` annotates) can honour it without
/// reaching back into the class table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vis {
    Public,
    Protected,
    Private,
}

/// One property slot: name, current value, and visibility.
#[derive(Clone)]
pub struct Prop {
    pub name: Box<[u8]>,
    pub value: Value,
    pub vis: Vis,
}

#[derive(Clone)]
pub struct Object(Rc<RefCell<ObjectData>>);

struct ObjectData {
    class: u32,
    props: Vec<Prop>,
}

impl Object {
    /// Create an instance of `class` seeded with `props` (the class's declared
    /// properties — name, default value, visibility — in declaration order).
    pub fn new(class: u32, props: Vec<(Box<[u8]>, Value, Vis)>) -> Self {
        let props = props
            .into_iter()
            .map(|(name, value, vis)| Prop { name, value, vis })
            .collect();
        Object(Rc::new(RefCell::new(ObjectData { class, props })))
    }

    /// The id of the class this object instantiates.
    pub fn class_id(&self) -> u32 {
        self.0.borrow().class
    }

    /// Read property `name`, or `None` if the object has no such property.
    pub fn get(&self, name: &[u8]) -> Option<Value> {
        self.0
            .borrow()
            .props
            .iter()
            .find(|p| p.name.as_ref() == name)
            .map(|p| p.value.clone())
    }

    /// Set property `name` to `value`, appending it as a **public** dynamic
    /// property if the object does not already declare it — PHP lets you assign
    /// arbitrary properties onto an instance.
    pub fn set(&self, name: &[u8], value: Value) {
        let mut data = self.0.borrow_mut();
        if let Some(slot) = data.props.iter_mut().find(|p| p.name.as_ref() == name) {
            slot.value = value;
        } else {
            data.props.push(Prop { name: name.into(), value, vis: Vis::Public });
        }
    }

    /// The properties, in insertion order, cloned out. Used by the value
    /// formatters (`var_dump`/`print_r`/`json_encode`).
    pub fn props(&self) -> Vec<Prop> {
        self.0.borrow().props.clone()
    }
}

impl PartialEq for Object {
    /// Identity comparison: the same instance, never two distinct ones. PHP `===`
    /// on objects is reference identity; loose `==` (same class + equal props) is
    /// a documented divergence handled at the `Value` layer when it lands.
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = self.0.borrow();
        write!(f, "Object(class #{}, {} props)", data.class, data.props.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj() -> Object {
        Object::new(0, vec![(Box::from(&b"x"[..]), Value::Int(1), Vis::Public)])
    }

    #[test]
    fn get_returns_default_then_updated_value() {
        let o = obj();
        assert_eq!(o.get(b"x"), Some(Value::Int(1)));
        o.set(b"x", Value::Int(9));
        assert_eq!(o.get(b"x"), Some(Value::Int(9)));
        assert_eq!(o.get(b"missing"), None);
    }

    #[test]
    fn set_appends_dynamic_public_property_in_order() {
        let o = obj();
        o.set(b"y", Value::Int(2));
        let props = o.props();
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].name.as_ref(), b"x");
        assert_eq!(props[1].name.as_ref(), b"y");
        assert_eq!(props[1].vis, Vis::Public); // dynamic properties are public
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
        assert_eq!(a.class_id(), 0);
    }
}
