//! The resource table: id allocation and lookup for `resource` values
//! (streams, process handles, …). The payload lives in the
//! [`rphp_value::Resource`] cell; the table keeps a handle so an id can be
//! resolved back (`get_resources()`, debugging) and closed centrally.

use std::any::Any;
use std::collections::BTreeMap;

use rphp_value::{Resource, Value};

/// Live resources by id. Ids are allocated from 1 upward and never reused
/// within a request (php reuses freed slots; a documented divergence).
#[derive(Default)]
pub struct ResourceTable {
    next: u32,
    live: BTreeMap<u32, Resource>,
}

impl ResourceTable {
    /// An empty table whose first id is 1.
    pub fn new() -> ResourceTable {
        ResourceTable {
            next: 1,
            live: BTreeMap::new(),
        }
    }

    /// Allocate an id, wrap `payload` as a resource of `kind` (its
    /// `get_resource_type()` name), and return it as a value.
    pub fn add(&mut self, kind: &'static str, payload: Box<dyn Any>) -> Value {
        let id = self.next;
        self.next += 1;
        let r = Resource::new(id, kind, payload);
        self.live.insert(id, r.clone());
        Value::Resource(r)
    }

    /// The live resource with this id.
    pub fn get(&self, id: u32) -> Option<Resource> {
        self.live.get(&id).cloned()
    }

    /// Close a resource by id (its payload is dropped, its kind becomes
    /// `Unknown`); `false` if it was not live.
    pub fn close(&mut self, id: u32) -> bool {
        match self.live.remove(&id) {
            Some(r) => {
                r.close();
                true
            }
            None => false,
        }
    }

    /// Close a resource value.
    pub fn close_value(&mut self, v: &Value) -> bool {
        match &*v.deref() {
            Value::Resource(r) => self.close(r.id()),
            _ => false,
        }
    }

    /// Number of live resources.
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Whether no resource is live.
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Every live resource, by ascending id.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Resource)> {
        self.live.iter().map(|(id, r)| (*id, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_get_close() {
        let mut t = ResourceTable::new();
        let v = t.add("stream", Box::new(42u32));
        let Value::Resource(r) = &v else {
            panic!("not a resource")
        };
        assert_eq!(r.id(), 1);
        assert_eq!(r.kind(), "stream");
        assert_eq!(t.get(1).map(|r| r.id()), Some(1));
        assert_eq!(t.len(), 1);
        assert!(t.close_value(&v));
        assert!(r.is_closed());
        assert!(t.get(1).is_none());
        assert!(!t.close(1));
        let v2 = t.add("stream", Box::new(()));
        let Value::Resource(r2) = &v2 else { panic!() };
        assert_eq!(r2.id(), 2, "ids are not reused");
    }
}
