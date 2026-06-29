//! String interner. Identifiers are interned eagerly by the lexer so every
//! downstream stage compares symbols by integer ([`IdentId`]), never by bytes.
//!
//! PHP identifiers are bytes, not guaranteed UTF-8, so the interner keys on
//! `[u8]`. A real engine keeps a process-lifetime global interner plus a
//! per-isolate view; for M0 a single owned interner suffices.
#![forbid(unsafe_code)]

use std::collections::HashMap;

/// An interned identifier. Comparable and hashable in O(1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct IdentId(pub u32);

#[derive(Default)]
pub struct Interner {
    map: HashMap<Box<[u8]>, IdentId>,
    vec: Vec<Box<[u8]>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern raw bytes, returning a stable id. Idempotent.
    pub fn intern(&mut self, bytes: &[u8]) -> IdentId {
        if let Some(&id) = self.map.get(bytes) {
            return id;
        }
        let id = IdentId(self.vec.len() as u32);
        let boxed: Box<[u8]> = bytes.into();
        self.vec.push(boxed.clone());
        self.map.insert(boxed, id);
        id
    }

    pub fn intern_str(&mut self, s: &str) -> IdentId {
        self.intern(s.as_bytes())
    }

    /// The raw bytes behind an id.
    pub fn resolve(&self, id: IdentId) -> &[u8] {
        &self.vec[id.0 as usize]
    }

    /// Lossy UTF-8 view, for diagnostics and `--emit` dumps.
    pub fn resolve_lossy(&self, id: IdentId) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.resolve(id))
    }

    /// The id for `bytes` if it has already been interned, without interning it.
    /// Lets the compiler find a well-known name (e.g. `this`) through the
    /// immutable interner it is handed.
    pub fn get(&self, bytes: &[u8]) -> Option<IdentId> {
        self.map.get(bytes).copied()
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_idempotent() {
        let mut i = Interner::new();
        let a = i.intern_str("foo");
        let b = i.intern_str("bar");
        let a2 = i.intern_str("foo");
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(i.resolve(a), b"foo");
        assert_eq!(i.resolve_lossy(b), "bar");
        assert_eq!(i.len(), 2);
    }
}
