//! [`PhpRef`]: the shared cell behind PHP references (`&$x`).
//!
//! A reference is a small refcounted heap cell holding one [`Value`]; every
//! variable, array element or property slot bound to it stores a
//! [`Value::Ref`] handle onto the same cell, so a write through any handle is
//! visible through all of them. The cell **never contains a `Ref`** (a
//! reference to a reference collapses); every constructor and setter here
//! enforces that invariant, and every kernel in [`Value`] dereferences before
//! operating.
use std::cell::{Ref, RefCell};
use std::fmt;
use std::rc::Rc;

use crate::Value;

/// A shared, mutable value cell (`&$x`). Cloning is a refcount bump onto the
/// same cell.
#[derive(Clone)]
pub struct PhpRef(Rc<RefCell<Value>>);

impl PhpRef {
    /// A fresh cell holding `v` (dereferenced if it is itself a `Ref`).
    pub fn new(v: Value) -> Self {
        PhpRef(Rc::new(RefCell::new(v.unref())))
    }

    /// The current contents, cloned out (an `Rc` bump for heap values).
    pub fn get(&self) -> Value {
        self.0.borrow().clone()
    }

    /// Replace the contents (dereferencing `v` to keep the invariant).
    pub fn set(&self, v: Value) {
        *self.0.borrow_mut() = v.unref();
    }

    /// Borrow the contents. The guard must be dropped before anything can
    /// write through the reference (a live guard makes `set`/`update` panic),
    /// so do not hold it across a call back into the VM.
    pub fn borrow(&self) -> Ref<'_, Value> {
        self.0.borrow()
    }

    /// Mutate the contents in place (e.g. `$a[] = 1` where `$a` is bound to a
    /// reference: the array inside the cell is COW-separated without copying
    /// it out and back). If `f` leaves a `Ref` behind it is collapsed.
    pub fn update<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        let mut guard = self.0.borrow_mut();
        let r = f(&mut guard);
        if let Value::Ref(inner) = &*guard {
            // Re-establish the invariant; a self-cycle would deadlock the cell,
            // so it collapses to null instead.
            let v = if inner.ptr_eq(self) { Value::Null } else { inner.get() };
            *guard = v;
        }
        r
    }

    /// Whether two handles share one cell.
    /// A stable identity for the cell: the address of the shared box. Two
    /// handles to one reference answer the same number, which is what
    /// `ReflectionReference::getId()` is built on. Valid only while the cell
    /// is alive, so a caller that publishes an id keeps its handle.
    pub fn id(&self) -> usize {
        std::rc::Rc::as_ptr(&self.0) as *const () as usize
    }

    pub fn ptr_eq(&self, other: &PhpRef) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Number of live handles onto this cell. `var_dump` marks an element `&`
    /// only while more than one handle exists (PHP treats a refcount-1
    /// reference as a plain value).
    pub fn strong_count(&self) -> usize {
        Rc::strong_count(&self.0)
    }
}

impl fmt::Debug for PhpRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.try_borrow() {
            Ok(v) => write!(f, "Ref({v:?})"),
            Err(_) => f.write_str("Ref(<borrowed>)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_ref_shares_a_cell_between_slots() {
        // $a = 1; $b = &$a; $b = 2; echo $a;  => 2
        let mut a = Value::Int(1);
        let r = Value::make_ref(&mut a);
        assert!(a.is_ref());
        let mut b = Value::Ref(r.clone());
        Value::assign(&mut b, Value::Int(2));
        assert_eq!(a.deref().into_owned(), Value::Int(2));
        assert_eq!(r.get(), Value::Int(2));
        // make_ref on an already-bound slot returns the same cell.
        assert!(Value::make_ref(&mut a).ptr_eq(&r));
        assert_eq!(r.strong_count(), 3); // r, a, b
    }

    #[test]
    fn assign_replaces_a_plain_slot_and_derefs_the_source() {
        let mut slot = Value::Int(1);
        let src = Value::Ref(PhpRef::new(Value::Int(7)));
        Value::assign(&mut slot, src);
        // Assigning from a reference copies the value, it does not bind.
        assert!(!slot.is_ref());
        assert_eq!(slot, Value::Int(7));
    }

    #[test]
    fn a_ref_never_contains_a_ref() {
        let inner = PhpRef::new(Value::Int(3));
        let outer = PhpRef::new(Value::Ref(inner.clone()));
        assert!(!outer.borrow().is_ref());
        outer.set(Value::Ref(inner.clone()));
        assert!(!outer.borrow().is_ref());
        outer.update(|v| *v = Value::Ref(inner.clone()));
        assert!(!outer.borrow().is_ref());
        assert_eq!(outer.get(), Value::Int(3));
        assert_eq!(Value::Ref(outer).unref(), Value::Int(3));
    }

    #[test]
    fn kernels_dereference() {
        // $a = 1; $b = &$a; var_dump($a === $b, $a == $b, $a + $b, $a . $b);
        let mut a = Value::Int(1);
        let b = Value::Ref(Value::make_ref(&mut a));
        assert!(a.identical(&b));
        assert!(b.identical(&Value::Int(1)));
        assert!(a.loose_eq(&b));
        assert_eq!(a.add(&b).unwrap(), Value::Int(2));
        assert_eq!(a.concat(&b), Value::string(b"11"));
        assert_eq!(b.spaceship(&Value::Int(5)), -1);
        assert!(b.to_bool());
        assert_eq!(b.to_int(), 1);
        assert_eq!(b.type_name(), "int");
        assert_eq!(b.to_php_string(), "1");
        // Rust-level equality is deref-transparent too.
        assert_eq!(b, Value::Int(1));
    }
}
