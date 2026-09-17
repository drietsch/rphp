//! The per-function constant pool ([`Const`]) and the prelowercased name
//! constant ([`NameConst`]) the late-binding ops key their lookups on.

use std::hash::{Hash, Hasher};

use rphp_value::{Str, Value};

use crate::{CodeAddr, TypeDecl};

/// A symbol name as the source spelled it plus its ASCII-lowercased twin and a
/// stable hash of the twin, computed once at compile time so runtime lookups
/// of functions, classes and methods (all case-insensitive in PHP) never
/// lowercase or rehash on the hot path (plan E11).
///
/// Names are fully qualified without a leading backslash (`Foo\Bar\baz`).
/// PHP's case folding is ASCII-only and locale-independent, so `lower` is
/// [`u8::to_ascii_lowercase`] applied bytewise. Constants are the one
/// case-sensitive symbol kind: [`Op::FetchConst`](crate::Op::FetchConst) uses
/// `orig` for the final segment and `lower` for the namespace part.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NameConst {
    /// The name as written (after namespace resolution), for messages and
    /// `::class`.
    pub orig: Box<[u8]>,
    /// `orig` lowercased bytewise (ASCII).
    pub lower: Box<[u8]>,
    /// [`NameConst::hash_bytes`] of `lower`. The runtime's symbol tables can use
    /// the same function (via a `BuildHasher` that returns the precomputed value)
    /// so a lookup never rehashes.
    pub hash: u64,
}

impl NameConst {
    /// Build from the resolved spelling.
    pub fn new(orig: &[u8]) -> NameConst {
        let lower: Box<[u8]> = orig.iter().map(u8::to_ascii_lowercase).collect();
        let hash = NameConst::hash_bytes(&lower);
        NameConst {
            orig: Box::from(orig),
            lower,
            hash,
        }
    }

    /// The hash function behind [`NameConst::hash`]: 64-bit FNV-1a over the
    /// bytes. Deterministic across processes and builds (it is never persisted
    /// today, but a future encoded byte format may cache it).
    pub fn hash_bytes(bytes: &[u8]) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        bytes
            .iter()
            .fold(OFFSET, |h, &b| (h ^ u64::from(b)).wrapping_mul(PRIME))
    }
}

impl Hash for NameConst {
    /// Hashes as the lowercased name only, so two spellings of one symbol
    /// collide as they should.
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.lower.hash(state);
    }
}

/// A compile-time constant in a function's (or [`ClassDecl`](crate::ClassDecl)'s)
/// constant pool.
///
/// The scalar variants (`Int`, `Float`, `Str`, `Bool`, `Null`) materialize as
/// values through [`Const::to_value`]. The remaining variants are *tables*
/// referenced by specific ops and never loaded as values: see each one.
#[derive(Clone, PartialEq, Debug)]
pub enum Const {
    Int(i64),
    Float(f64),
    Str(Str),
    /// `true` / `false` literal.
    Bool(bool),
    /// `null` literal.
    Null,
    /// A function/class/method/constant name with its lowercased twin; the
    /// operand of every late-binding op (`InitFCall`, `ClassRef::named`,
    /// `FetchConst`, method `NameRef`s, catch types, …).
    Name(NameConst),
    /// The (key, target) rows of an [`Op::Switch`](crate::Op::Switch) — matched
    /// in order with `==` or `===` — or of an
    /// [`Op::FinallyEnd`](crate::Op::FinallyEnd) `targets` table, whose keys are
    /// the `Int(k)` jump ids the compiler stores in the region's `payload`
    /// register.
    JumpTable(Vec<(Value, CodeAddr)>),
    /// Per-call-site argument names (`None` = positional) for a future fused
    /// call op (plan E11 `CallFn` peephole) and for `SendUnpack` diagnostics. No
    /// v2 op references it yet.
    ArgNames(Vec<Option<Box<[u8]>>>),
    /// An owned type declaration for ops that need a type at runtime
    /// (`settype`-like checks, future typed-variadic coercion). No v2 op
    /// references it yet; declared types live on
    /// [`ParamDef`](crate::ParamDef)/[`PropDecl`](crate::PropDecl) metadata.
    Type(TypeDecl),
}

impl Const {
    /// Materialize a runtime [`Value`]. For `Str` this is a cheap refcount bump,
    /// so loading a string constant in a loop does not re-allocate. A `Name`
    /// materializes as its original spelling (allocating a new string), which is
    /// what `Foo::class` and callable strings need.
    ///
    /// The table variants (`JumpTable`, `ArgNames`, `Type`) have no value form;
    /// they return `Value::Null` here so that callers written against the M0
    /// pool keep working, and [`Const::try_to_value`] distinguishes them.
    pub fn to_value(&self) -> Value {
        self.try_to_value().unwrap_or(Value::Null)
    }

    /// Like [`Const::to_value`] but `None` for the table variants that have no
    /// value form.
    pub fn try_to_value(&self) -> Option<Value> {
        Some(match self {
            Const::Int(i) => Value::Int(*i),
            Const::Float(f) => Value::Float(*f),
            Const::Str(s) => Value::Str(s.clone()),
            Const::Bool(b) => Value::Bool(*b),
            Const::Null => Value::Null,
            Const::Name(n) => Value::Str(Str::new(&n.orig)),
            Const::JumpTable(_) | Const::ArgNames(_) | Const::Type(_) => return None,
        })
    }

    /// The name constant, if this is one.
    pub fn as_name(&self) -> Option<&NameConst> {
        match self {
            Const::Name(n) => Some(n),
            _ => None,
        }
    }

    /// The jump table, if this is one.
    pub fn as_jump_table(&self) -> Option<&[(Value, CodeAddr)]> {
        match self {
            Const::JumpTable(t) => Some(t),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_const_lowercases_ascii_only() {
        let n = NameConst::new(b"App\\Http\\MyController");
        assert_eq!(&*n.orig, b"App\\Http\\MyController");
        assert_eq!(&*n.lower, b"app\\http\\mycontroller");
        // Non-ASCII bytes are left untouched (PHP folds ASCII only): the
        // `Ü` survives while `BER` folds.
        let u = NameConst::new("ÜBER".as_bytes());
        assert_eq!(&*u.lower, "Über".as_bytes());
    }

    #[test]
    fn name_const_hash_is_case_insensitive_and_stable() {
        let a = NameConst::new(b"strlen");
        let b = NameConst::new(b"StrLen");
        assert_eq!(a.hash, b.hash);
        assert_eq!(a.hash, NameConst::hash_bytes(b"strlen"));
        assert_ne!(a.hash, NameConst::hash_bytes(b"strlem"));
        // FNV-1a 64 reference vectors.
        assert_eq!(NameConst::hash_bytes(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(NameConst::hash_bytes(b"a"), 0xaf63_dc4c_8601_ec8c);
        // `Hash` follows `lower`, so the two spellings hash alike in std maps too.
        use std::collections::hash_map::DefaultHasher;
        let h = |n: &NameConst| {
            let mut s = DefaultHasher::new();
            n.hash(&mut s);
            s.finish()
        };
        assert_eq!(h(&a), h(&b));
        assert_ne!(a, b); // but they are not equal: `orig` differs
    }

    #[test]
    fn to_value_and_try_to_value() {
        assert_eq!(Const::Int(3).to_value(), Value::Int(3));
        assert_eq!(Const::Bool(true).to_value(), Value::Bool(true));
        assert_eq!(Const::Null.to_value(), Value::Null);
        assert_eq!(Const::Str(Str::new(b"x")).to_value(), Value::string(b"x"));
        assert_eq!(
            Const::Name(NameConst::new(b"Foo\\Bar")).to_value(),
            Value::string(b"Foo\\Bar")
        );
        let table = Const::JumpTable(vec![(Value::Int(1), 7)]);
        assert_eq!(table.to_value(), Value::Null);
        assert_eq!(table.try_to_value(), None);
        assert_eq!(table.as_jump_table().map(<[_]>::len), Some(1));
        assert_eq!(Const::ArgNames(vec![None]).try_to_value(), None);
        assert_eq!(
            Const::Type(TypeDecl::Builtin(crate::BuiltinType::Int)).try_to_value(),
            None
        );
        assert!(Const::Name(NameConst::new(b"f")).as_name().is_some());
        assert!(Const::Int(1).as_name().is_none());
    }
}
