//! [`Resource`]: PHP's legacy opaque handle type (streams, curl handles,
//! process handles, …).
//!
//! A resource is a refcounted cell with a process-unique integer id, a kind
//! string (`"stream"`, `"curl"`, …) and an arbitrary native payload. Closing a
//! resource (`fclose`) takes the payload; a closed resource keeps its id but
//! reports kind `Unknown` and type `resource (closed)`, exactly as PHP prints
//! it. Ids are allocated by the extension that creates the resource (PHP's
//! resource list), not here.
use std::any::Any;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

/// The shared cell behind a [`Resource`].
pub struct ResourceCell {
    id: u32,
    kind: Cell<&'static str>,
    /// `None` once closed.
    payload: RefCell<Option<Box<dyn Any>>>,
}

/// A handle onto a [`ResourceCell`]; cloning is a refcount bump onto the same
/// cell (`$b = $a` aliases the resource, as in PHP).
#[derive(Clone)]
pub struct Resource(Rc<ResourceCell>);

/// The kind PHP reports for a closed resource.
pub const CLOSED_KIND: &str = "Unknown";

impl Resource {
    /// A new open resource with the caller-allocated `id`.
    pub fn new(id: u32, kind: &'static str, payload: Box<dyn Any>) -> Self {
        Resource(Rc::new(ResourceCell {
            id,
            kind: Cell::new(kind),
            payload: RefCell::new(Some(payload)),
        }))
    }

    /// The resource id (`(int)$r`, the `#N` in `Resource id #N`).
    pub fn id(&self) -> u32 {
        self.0.id
    }

    /// The kind (`var_dump`'s `of type (…)`); `Unknown` once closed.
    pub fn kind(&self) -> &'static str {
        self.0.kind.get()
    }

    /// Change the kind (e.g. a stream promoted to a persistent stream).
    pub fn set_kind(&self, kind: &'static str) {
        self.0.kind.set(kind);
    }

    /// Whether the payload has been taken by [`Resource::close`].
    pub fn is_closed(&self) -> bool {
        self.0.payload.borrow().is_none()
    }

    /// `gettype`: `resource` or `resource (closed)`.
    pub fn type_name(&self) -> &'static str {
        if self.is_closed() {
            "resource (closed)"
        } else {
            "resource"
        }
    }

    /// Close: take the payload out (returned to the caller for release) and
    /// mark the kind `Unknown`. A second close returns `None`.
    pub fn close(&self) -> Option<Box<dyn Any>> {
        let taken = self.0.payload.borrow_mut().take();
        if taken.is_some() {
            self.0.kind.set(CLOSED_KIND);
        }
        taken
    }

    /// Borrow the payload slot (`None` when closed).
    pub fn payload(&self) -> Ref<'_, Option<Box<dyn Any>>> {
        self.0.payload.borrow()
    }

    /// Mutably borrow the payload slot.
    pub fn payload_mut(&self) -> RefMut<'_, Option<Box<dyn Any>>> {
        self.0.payload.borrow_mut()
    }

    /// Run `f` on the payload downcast to `T`; `None` if closed or of another
    /// type. The borrow is released before returning, so `f` must not call
    /// back into anything that touches this resource.
    pub fn with<T: Any, R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut slot = self.0.payload.borrow_mut();
        slot.as_mut().and_then(|b| b.downcast_mut::<T>()).map(f)
    }

    /// Identity (`===`).
    pub fn ptr_eq(&self, other: &Resource) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl PartialEq for Resource {
    /// Identity, as PHP's `===` on resources.
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
    }
}

impl fmt::Debug for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Resource(#{} {})", self.0.id, self.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn stream(id: u32) -> Value {
        Value::Resource(Resource::new(id, "stream", Box::new(vec![1u8, 2, 3])))
    }

    #[test]
    fn casts_and_type_names_match_php() {
        let r = stream(1);
        assert_eq!(r.type_name(), "resource");
        assert_eq!(r.to_int(), 1);
        assert!(r.to_bool());
        assert_eq!(r.to_float(), 1.0);
        assert_eq!(r.to_php_string(), "Resource id #1");
        assert_eq!(r.concat(&Value::string(b"x")), Value::string(b"Resource id #1x"));
        assert!(!r.is_numeric());
        assert_eq!(r.to_number(), Value::Int(1));
        // Arithmetic on a resource is a TypeError.
        assert!(r.add(&Value::Int(1)).is_err());
        assert!(r.neg().is_err());
    }

    #[test]
    fn comparison_uses_the_id_against_scalars_and_identity_for_strict() {
        let (a, b) = (stream(1), stream(5));
        // $r == 1, $r == "1", $r == "1abc", $r == 1.0, $r == true, $r < 2, $r <=> 5
        assert!(a.loose_eq(&Value::Int(1)));
        assert!(a.loose_eq(&Value::string(b"1")));
        assert!(a.loose_eq(&Value::string(b"1abc")));
        assert!(!a.loose_eq(&Value::string(b"abc")));
        assert!(a.loose_eq(&Value::Float(1.0)));
        assert!(a.loose_eq(&Value::Bool(true)));
        assert!(!a.loose_eq(&Value::Null));
        assert!(a.lt(&Value::Int(2)));
        assert!(a.lt(&Value::Float(1.5)));
        assert_eq!(a.spaceship(&Value::Int(5)), -1);
        assert_eq!(Value::Int(5).spaceship(&a), 1);
        assert_eq!(a.spaceship(&Value::string(b"abc")), 1);
        assert_eq!(a.spaceship(&b), -1);
        assert_eq!(a.spaceship(&Value::empty_array()), -1);
        assert!(!a.loose_eq(&Value::empty_array()));
        // Two resources: `==` by id, `===` by identity.
        assert!(a.loose_eq(&a.clone()));
        assert!(!a.loose_eq(&b));
        assert!(a.identical(&a.clone()));
        assert!(!a.identical(&Value::Int(1)));
        let same_id = stream(1);
        assert!(a.loose_eq(&same_id));
        assert!(!a.identical(&same_id));
    }

    #[test]
    fn closing_takes_the_payload_and_changes_the_reported_type() {
        let r = Resource::new(5, "stream", Box::new(String::from("fd")));
        assert_eq!(r.with(|s: &mut String| s.len()), Some(2));
        assert_eq!(r.with(|_: &mut i32| ()), None); // wrong payload type
        assert!(!r.is_closed());
        let taken = r.close().expect("open");
        assert_eq!(taken.downcast_ref::<String>().map(String::as_str), Some("fd"));
        assert!(r.is_closed());
        assert!(r.close().is_none());
        assert_eq!(r.kind(), "Unknown");
        assert_eq!(r.type_name(), "resource (closed)");
        assert_eq!(r.with(|_: &mut String| ()), None);
        // The id and the scalar casts survive closing (PHP: (int)$closed == 5).
        let v = Value::Resource(r);
        assert_eq!(v.to_int(), 5);
        assert!(v.to_bool());
        assert_eq!(v.to_php_string(), "Resource id #5");
        assert!(v.loose_eq(&Value::Int(5)));
    }
}
