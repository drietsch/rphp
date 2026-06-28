//! The standard library as a **registry of native-function descriptors**
//! (`specs/base/08-stdlib-ext.md`): the engine never hard-codes a builtin, it
//! resolves a name to a [`NativeId`] and calls the descriptor's `func`. Adding a
//! function is one table row; removing an extension will be dropping a feature
//! gate, never editing the engine.
//!
//! Each [`NativeFn`] declares an arity range so the **compiler** can range-check
//! call sites (mirroring the user-function arg-count check), and a `func` the
//! **runtime** invokes with the evaluated arguments and a [`Ctx`] (today just
//! the output buffer, for `echo`-style builtins like `var_dump`).
#![forbid(unsafe_code)]

use std::sync::OnceLock;

use rphp_value::Value;

mod arrays;
mod ctype;
mod hash;
mod json;
mod math;
mod output;
mod pcre;
mod strings;
mod types;

/// Index of a builtin within the registry table. Stored verbatim in the
/// `CallNative` opcode, so it must stay stable for a compiled artifact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NativeId(pub u32);

/// Side-channel a native function may use — currently the run's stdout buffer.
/// Grows as builtins need more (interner, isolate state, error sink …).
pub struct Ctx<'a> {
    pub out: &'a mut Vec<u8>,
}

/// A recoverable native-call fault (wrong type, domain error …). The runtime
/// surfaces it as a PHP-level error; once exceptions exist it becomes a throw.
#[derive(Debug)]
pub struct NativeError {
    pub message: String,
}

impl NativeError {
    pub fn new(message: impl Into<String>) -> Self {
        NativeError { message: message.into() }
    }
}

pub type NativeResult = Result<Value, NativeError>;

/// A builtin's implementation. Most functions are **pure** in the sense that
/// they only read their arguments (`&[Value]`); a few take `&mut [Value]` so they
/// can write back through a **by-reference** parameter (`sort($a)`,
/// `array_push($a, …)`, `preg_match($p, $s, $m)`). Keeping the two forms distinct
/// makes "does not mutate its args" visible in the type.
#[derive(Clone, Copy)]
pub enum Handler {
    Pure(fn(&mut Ctx, &[Value]) -> NativeResult),
    ByRef(fn(&mut Ctx, &mut [Value]) -> NativeResult),
}

/// A native-function descriptor. All fields are `Copy`, so the per-extension
/// `FUNCTIONS` slices flatten into the registry by value.
#[derive(Clone, Copy)]
pub struct NativeFn {
    pub name: &'static str,
    pub min_args: usize,
    /// `None` means variadic (no upper bound).
    pub max_args: Option<usize>,
    /// Bitmask of by-reference parameter positions (bit `i` ⇒ argument `i` is
    /// passed by reference and written back to the caller's variable). `0` for an
    /// ordinary function. The compiler reads this to require an lvalue and emit
    /// the write-back; the runtime reads it to copy mutated args back.
    pub by_ref: u32,
    pub handler: Handler,
}

impl NativeFn {
    /// Whether argument position `i` is declared by-reference.
    pub fn is_by_ref(&self, i: usize) -> bool {
        i < 32 && self.by_ref & (1 << i) != 0
    }
}

/// Build an ordinary (pure) [`NativeFn`] registry row. Each extension module uses
/// this in its `FUNCTIONS` slice (`use crate::{nf, NativeFn};`).
macro_rules! nf {
    ($name:literal, $min:expr, $max:expr, $f:path) => {
        NativeFn {
            name: $name,
            min_args: $min,
            max_args: $max,
            by_ref: 0,
            handler: $crate::Handler::Pure($f),
        }
    };
}
pub(crate) use nf;

/// Build a by-reference [`NativeFn`] registry row. `$byref` is the bitmask of
/// by-reference parameter positions (e.g. `0b001` for `&$arg0`, `0b100` for the
/// third argument). The handler takes `&mut [Value]` and writes results back into
/// those positions.
macro_rules! nf_mut {
    ($name:literal, $min:expr, $max:expr, $byref:expr, $f:path) => {
        NativeFn {
            name: $name,
            min_args: $min,
            max_args: $max,
            by_ref: $byref,
            handler: $crate::Handler::ByRef($f),
        }
    };
}
pub(crate) use nf_mut;

/// The flattened builtin registry. Each extension module owns a `FUNCTIONS`
/// slice; they are concatenated here in a fixed order, and `NativeId(i)` indexes
/// the result. Keeping each extension's functions in its own module (rather than
/// one shared table) is what lets the parity burn-down add extensions without
/// editing a shared list. Built once, on first lookup.
fn table() -> &'static [NativeFn] {
    static TABLE: OnceLock<Vec<NativeFn>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let groups: &[&[NativeFn]] = &[
            output::FUNCTIONS,
            types::FUNCTIONS,
            strings::FUNCTIONS,
            arrays::FUNCTIONS,
            math::FUNCTIONS,
            ctype::FUNCTIONS,
            hash::FUNCTIONS,
            json::FUNCTIONS,
            pcre::FUNCTIONS,
        ];
        groups.iter().flat_map(|g| g.iter().copied()).collect()
    })
}

/// Resolve a (case-insensitive) function name to its registry id.
pub fn resolve(name: &[u8]) -> Option<NativeId> {
    table()
        .iter()
        .position(|f| f.name.as_bytes().eq_ignore_ascii_case(name))
        .map(|i| NativeId(i as u32))
}

/// The descriptor for an id (its name and arity), for compiler diagnostics.
pub fn descriptor(id: NativeId) -> &'static NativeFn {
    &table()[id.0 as usize]
}

/// Invoke a builtin with already-evaluated arguments. `args` is `&mut` so a
/// by-reference builtin can write back through its argument slots; the caller is
/// responsible for propagating those mutations (the interpreter copies the
/// by-ref positions back into the caller's variables — see `descriptor().by_ref`).
pub fn call(id: NativeId, ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match descriptor(id).handler {
        Handler::Pure(f) => f(ctx, args),
        Handler::ByRef(f) => f(ctx, args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_is_case_insensitive() {
        assert!(resolve(b"strlen").is_some());
        assert!(resolve(b"STRLEN").is_some());
        assert!(resolve(b"StrLen").is_some());
        assert!(resolve(b"no_such_function").is_none());
    }

    /// Call a builtin by name with the given args, returning its result.
    fn call_named(name: &[u8], args: &[Value]) -> Value {
        let mut out = Vec::new();
        let mut ctx = Ctx { out: &mut out };
        let mut args = args.to_vec();
        call(resolve(name).unwrap(), &mut ctx, &mut args).unwrap()
    }

    fn arr(items: &[Value]) -> Value {
        let mut a = rphp_value::Array::new();
        for v in items {
            a.push(v.clone());
        }
        Value::Array(a)
    }

    #[test]
    fn substr_negative_length_trims_the_tail() {
        let s = Value::string(b"abcdef");
        assert_eq!(call_named(b"substr", &[s.clone(), Value::Int(1), Value::Int(-1)]), Value::string(b"bcde"));
        assert_eq!(call_named(b"substr", &[s.clone(), Value::Int(-2)]), Value::string(b"ef"));
        // length past the end clamps; an empty window yields "".
        assert_eq!(call_named(b"substr", &[s, Value::Int(0), Value::Int(-10)]), Value::string(b""));
    }

    #[test]
    fn explode_respects_a_positive_limit() {
        let parts = call_named(
            b"explode",
            &[Value::string(b","), Value::string(b"a,b,c,d"), Value::Int(2)],
        );
        // The remainder is kept whole in the final piece.
        assert_eq!(parts, arr(&[Value::string(b"a"), Value::string(b"b,c,d")]));
    }

    #[test]
    fn range_descends_and_supports_floats() {
        assert_eq!(
            call_named(b"range", &[Value::Int(3), Value::Int(1)]),
            arr(&[Value::Int(3), Value::Int(2), Value::Int(1)])
        );
        assert_eq!(
            call_named(b"range", &[Value::Int(0), Value::Int(1), Value::Float(0.5)]),
            arr(&[Value::Float(0.0), Value::Float(0.5), Value::Float(1.0)])
        );
    }

    #[test]
    fn str_repeat_rejects_negative_counts() {
        let mut out = Vec::new();
        let mut ctx = Ctx { out: &mut out };
        let r = call(resolve(b"str_repeat").unwrap(), &mut ctx, &mut [Value::string(b"x"), Value::Int(-1)]);
        assert!(r.is_err());
    }

    #[test]
    fn intdiv_by_zero_errors() {
        let mut out = Vec::new();
        let mut ctx = Ctx { out: &mut out };
        let r = call(resolve(b"intdiv").unwrap(), &mut ctx, &mut [Value::Int(1), Value::Int(0)]);
        assert!(r.is_err());
    }

    #[test]
    fn max_min_over_array_and_args() {
        assert_eq!(call_named(b"max", &[Value::Int(3), Value::Int(9), Value::Int(2)]), Value::Int(9));
        assert_eq!(call_named(b"min", &[arr(&[Value::Int(4), Value::Int(1), Value::Int(8)])]), Value::Int(1));
    }

    #[test]
    fn aliases_share_an_implementation() {
        // sizeof is an alias of count, join of implode.
        let mut out = Vec::new();
        let mut ctx = Ctx { out: &mut out };
        let arr = {
            let mut a = rphp_value::Array::new();
            a.push(Value::Int(1));
            a.push(Value::Int(2));
            Value::Array(a)
        };
        let by_count = call(resolve(b"count").unwrap(), &mut ctx, &mut [arr.clone()]).unwrap();
        let by_sizeof = call(resolve(b"sizeof").unwrap(), &mut ctx, &mut [arr.clone()]).unwrap();
        assert_eq!(by_count, Value::Int(2));
        assert_eq!(by_count, by_sizeof);
    }
}
