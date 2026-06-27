//! Byte spans and source ids — the lowest layer, no dependencies.
//!
//! Every IR node from the CST down to bytecode carries a [`Span`] for
//! diagnostics, backtraces, and (eventually) JIT source maps. Offsets are
//! byte offsets with a 4 GiB per-file ceiling (`u32`), which is ample.
#![forbid(unsafe_code)]

/// Identifies a source file within a `SourceMap` (see `rphp-source`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// A half-open byte range `[lo, hi)` within a single file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Span {
    pub file: FileId,
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    #[inline]
    pub const fn new(file: FileId, lo: u32, hi: u32) -> Self {
        Self { file, lo, hi }
    }

    /// A placeholder span (file 0, empty range). Used for synthesized nodes.
    #[inline]
    pub const fn dummy() -> Self {
        Self { file: FileId(0), lo: 0, hi: 0 }
    }

    #[inline]
    pub const fn len(&self) -> u32 {
        self.hi - self.lo
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.hi == self.lo
    }

    /// The smallest span covering both `self` and `other` (assumes same file).
    #[inline]
    pub fn to(self, other: Span) -> Span {
        Span { file: self.file, lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn len_and_join() {
        let a = Span::new(FileId(1), 3, 7);
        let b = Span::new(FileId(1), 10, 12);
        assert_eq!(a.len(), 4);
        assert!(!a.is_empty());
        let j = a.to(b);
        assert_eq!(j, Span::new(FileId(1), 3, 12));
    }

    #[test]
    fn dummy_is_empty() {
        assert!(Span::dummy().is_empty());
    }
}
