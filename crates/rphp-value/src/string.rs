//! [`Str`]: the PHP byte string.
//!
//! PHP strings are **byte** strings (never assumed UTF-8). A `Str` is a thin
//! pointer to a refcounted heap block holding the bytes and a lazily computed
//! hash, so a [`crate::Value`] stays 16 bytes; cloning is a refcount bump,
//! matching the eventual COW container. Mutation-in-place, the small-string
//! optimization and interning (`specs/base/03-heap-types.md` §11.1) arrive with
//! the real `PhpStr` in `rphp-heap` behind this same API.
use std::borrow::Cow;
use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;

/// The heap block behind a [`Str`].
struct StrData {
    bytes: Box<[u8]>,
    /// FNV-1a hash of `bytes`, computed on first use. `0` means "not computed
    /// yet" (a computed hash of zero is stored as `1`).
    hash: Cell<u64>,
}

/// A PHP string value: an immutable, refcounted byte buffer.
#[derive(Clone)]
pub struct Str(Rc<StrData>);

impl Str {
    /// Build a string by copying `bytes`.
    pub fn new(bytes: &[u8]) -> Self {
        Str::from_boxed(Box::from(bytes))
    }

    /// Build a string from an owned byte vector without re-copying.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Str::from_boxed(bytes.into_boxed_slice())
    }

    fn from_boxed(bytes: Box<[u8]>) -> Self {
        Str(Rc::new(StrData { bytes, hash: Cell::new(0) }))
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    /// Length in bytes (PHP `strlen`).
    pub fn len(&self) -> usize {
        self.0.bytes.len()
    }

    /// `true` for the empty string.
    pub fn is_empty(&self) -> bool {
        self.0.bytes.is_empty()
    }

    /// The cached FNV-1a hash of the bytes (computed on first call, never `0`).
    /// Meant for the runtime's symbol and property tables; [`Hash`] hashes the
    /// bytes through the caller's hasher so `Str` and `[u8]` keys agree.
    pub fn hash64(&self) -> u64 {
        let cached = self.0.hash.get();
        if cached != 0 {
            return cached;
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in self.0.bytes.iter() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let h = if h == 0 { 1 } else { h };
        self.0.hash.set(h);
        h
    }

    /// Whether two handles share one heap block.
    pub fn ptr_eq(&self, other: &Str) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// The bytes as UTF-8 text, lossily (for diagnostics; never for output).
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0.bytes)
    }
}

impl Default for Str {
    fn default() -> Self {
        Str::new(b"")
    }
}

impl Deref for Str {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl AsRef<[u8]> for Str {
    fn as_ref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl From<&[u8]> for Str {
    fn from(b: &[u8]) -> Self {
        Str::new(b)
    }
}

impl From<&str> for Str {
    fn from(s: &str) -> Self {
        Str::new(s.as_bytes())
    }
}

impl From<Vec<u8>> for Str {
    fn from(v: Vec<u8>) -> Self {
        Str::from_vec(v)
    }
}

impl From<String> for Str {
    fn from(s: String) -> Self {
        Str::from_vec(s.into_bytes())
    }
}

impl From<Box<[u8]>> for Str {
    fn from(b: Box<[u8]>) -> Self {
        Str::from_boxed(b)
    }
}

impl PartialEq for Str {
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        // Two already-hashed strings with different hashes cannot be equal.
        let (h1, h2) = (self.0.hash.get(), other.0.hash.get());
        if h1 != 0 && h2 != 0 && h1 != h2 {
            return false;
        }
        self.0.bytes == other.0.bytes // byte-wise
    }
}

impl Eq for Str {}

impl Hash for Str {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.bytes.hash(state);
    }
}

impl PartialOrd for Str {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Str {
    /// Byte-wise (PHP `strcmp`) order.
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.bytes.cmp(&other.0.bytes)
    }
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Readable in `--emit=bytecode` dumps and test failures; lossy for the
        // (rare) non-UTF-8 byte string.
        write!(f, "Str({:?})", self.to_string_lossy())
    }
}

impl fmt::Display for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_is_thin_and_compares_bytewise() {
        assert_eq!(std::mem::size_of::<Str>(), std::mem::size_of::<usize>());
        let a = Str::new(b"abc");
        let b = Str::from_vec(b"abc".to_vec());
        assert_eq!(a, b);
        assert!(!a.ptr_eq(&b));
        assert!(a.clone().ptr_eq(&a));
        assert_ne!(a, Str::new(b"abd"));
        assert!(a < Str::new(b"abd"));
        assert_eq!(&*a, b"abc");
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn hash_is_cached_and_nonzero() {
        let s = Str::new(b"");
        let h = s.hash64();
        assert_ne!(h, 0);
        assert_eq!(s.hash64(), h);
        assert_eq!(Str::new(b"x").hash64(), Str::new(b"x").hash64());
        // Different cached hashes short-circuit equality.
        let (a, b) = (Str::new(b"a"), Str::new(b"b"));
        a.hash64();
        b.hash64();
        assert_ne!(a, b);
    }
}
