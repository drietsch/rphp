//! Named symbol tables (ADR-019): the globals table and the per-frame
//! tables of `NEEDS_SYMTAB` functions. A table maps a variable name to the
//! shared [`PhpRef`] cell the frame's register is bound to, so name-based
//! access (`$$x`, `compact`, `extract`, `get_defined_vars`, an included
//! file) and register access alias one cell. Entries keep insertion order
//! (`get_defined_vars()` / `$GLOBALS` list variables in declaration order).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rphp_value::{Array, ArrayKey, PhpRef, Value};

/// The entries of a symbol table.
#[derive(Default)]
pub struct SymtabData {
    entries: Vec<(Box<[u8]>, PhpRef)>,
    index: HashMap<Box<[u8]>, usize>,
}

impl SymtabData {
    /// The cell bound to `name`.
    pub fn get(&self, name: &[u8]) -> Option<PhpRef> {
        self.index.get(name).map(|&i| self.entries[i].1.clone())
    }

    /// Whether `name` has an entry.
    pub fn contains(&self, name: &[u8]) -> bool {
        self.index.contains_key(name)
    }

    /// Bind `name` to `cell`, replacing an earlier binding.
    pub fn insert(&mut self, name: &[u8], cell: PhpRef) {
        match self.index.get(name) {
            Some(&i) => self.entries[i].1 = cell,
            None => {
                self.index.insert(Box::from(name), self.entries.len());
                self.entries.push((Box::from(name), cell));
            }
        }
    }

    /// The cell bound to `name`, creating a null cell if absent.
    pub fn get_or_create(&mut self, name: &[u8]) -> PhpRef {
        if let Some(c) = self.get(name) {
            return c;
        }
        let c = PhpRef::new(Value::Null);
        self.insert(name, c.clone());
        c
    }

    /// Remove `name` (`unset`); the cell survives for other holders.
    pub fn remove(&mut self, name: &[u8]) -> Option<PhpRef> {
        let i = self.index.remove(name)?;
        let (_, cell) = self.entries.remove(i);
        for (j, (n, _)) in self.entries.iter().enumerate().skip(i) {
            self.index.insert(n.clone(), j);
        }
        Some(cell)
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &PhpRef)> {
        self.entries.iter().map(|(n, c)| (n.as_ref(), c))
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A snapshot array (`name => value`) of the initialized entries, in order.
    pub fn to_array(&self) -> Array {
        let mut out = Array::new();
        for (name, cell) in &self.entries {
            let v = cell.get();
            if v.is_uninit() {
                continue;
            }
            out.set(ArrayKey::str(name), v);
        }
        out
    }
}

/// A shared symbol table handle (an `Include` frame shares its includer's;
/// the entry script's `{main}` uses the globals table).
#[derive(Clone, Default)]
pub struct Symtab(pub Rc<RefCell<SymtabData>>);

impl Symtab {
    /// An empty table.
    pub fn new() -> Symtab {
        Symtab::default()
    }

    /// Run `f` under a shared borrow.
    pub fn with<R>(&self, f: impl FnOnce(&SymtabData) -> R) -> R {
        f(&self.0.borrow())
    }

    /// Run `f` under an exclusive borrow (never across a VM call).
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut SymtabData) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }

    /// The cell bound to `name`.
    pub fn get(&self, name: &[u8]) -> Option<PhpRef> {
        self.0.borrow().get(name)
    }

    /// The cell bound to `name`, creating a null cell if absent.
    pub fn get_or_create(&self, name: &[u8]) -> PhpRef {
        self.0.borrow_mut().get_or_create(name)
    }

    /// Bind `name` to `cell`.
    pub fn insert(&self, name: &[u8], cell: PhpRef) {
        self.0.borrow_mut().insert(name, cell);
    }

    /// Whether two handles are the same table.
    pub fn ptr_eq(&self, other: &Symtab) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_keep_order() {
        let t = Symtab::new();
        let a = t.get_or_create(b"a");
        let b = t.get_or_create(b"b");
        t.get_or_create(b"c");
        a.set(Value::Int(1));
        b.set(Value::Int(2));
        assert!(t.get(b"a").unwrap().ptr_eq(&a));
        assert_eq!(t.with(|d| d.len()), 3);
        assert!(t.with_mut(|d| d.remove(b"b")).is_some());
        let names: Vec<Vec<u8>> = t.with(|d| d.iter().map(|(n, _)| n.to_vec()).collect());
        assert_eq!(names, vec![b"a".to_vec(), b"c".to_vec()]);
        let arr = t.with(|d| d.to_array());
        assert_eq!(arr.len(), 2);
        assert_eq!(arr.get_deref(&ArrayKey::str(b"a")), Some(Value::Int(1)));
        t.insert(b"c", a.clone());
        assert!(t.get(b"c").unwrap().ptr_eq(&a));
        assert_eq!(t.with(|d| d.len()), 2);
    }
}
