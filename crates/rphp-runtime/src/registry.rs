//! The native ABI (spec 08 §15–16, ADR-014): the descriptor an extension
//! registers for each builtin ([`NativeFn`]), the handle a builtin receives
//! ([`Ctx`]), the single non-local control channel every runtime path returns
//! ([`Unwind`], ADR-022), and the [`Registry`] builder through which natives
//! and constants enter an [`Interp`].
//!
//! There is exactly one handler shape, `fn(&mut Ctx, &mut [Value]) ->
//! NativeResult`: a builtin that does not write back through a by-reference
//! parameter simply leaves its argument slots untouched.

use std::fmt;
use std::ops::{Deref, DerefMut};

pub use rphp_ext_api::FnFlags;
use rphp_value::{Object, Value};

use crate::Interp;

/// Process-local index of a registered native, assigned at registration
/// (the position in [`Interp::natives`]). Not stable across processes; the
/// compiler bakes it into `Op::CallNative` only for the `Interp` it was
/// resolved against (ADR-014: names are what a *stored* unit would carry).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NativeId(pub u32);

/// What every native returns: a value, or an [`Unwind`].
pub type NativeResult = Result<Value, Unwind>;

/// The handler signature shared by every native function.
pub type NativeHandler = fn(&mut Ctx, &mut [Value]) -> NativeResult;

/// A native-function descriptor. Every field is `Copy`, so per-extension
/// `FUNCTIONS` slices are `'static` data that the registry copies by value.
#[derive(Clone, Copy)]
pub struct NativeFn {
    /// The PHP-visible name (any case; matched case-insensitively).
    pub name: &'static str,
    /// Minimum number of arguments.
    pub min_args: u8,
    /// Maximum number of arguments; `None` means variadic.
    pub max_args: Option<u8>,
    /// Bitmask of by-reference parameter positions (bit `i` ⇒ argument `i`
    /// is passed by reference and written back to the caller's variable).
    pub by_ref: u32,
    /// Parameter names, for named arguments and `TypeError` texts (may be
    /// empty until the arginfo generator fills them).
    pub params: &'static [&'static str],
    /// Optimizer / diagnostic flags (spec 08 §15.1).
    pub flags: FnFlags,
    /// The implementation.
    pub handler: NativeHandler,
}

impl NativeFn {
    /// Whether argument position `i` is declared by-reference.
    pub fn is_by_ref(&self, i: usize) -> bool {
        i < 32 && self.by_ref & (1 << i) != 0
    }

    /// Whether `argc` arguments satisfy the declared arity range.
    pub fn accepts(&self, argc: usize) -> bool {
        argc >= self.min_args as usize && self.max_args.is_none_or(|m| argc <= m as usize)
    }

    /// PHP's `ArgumentCountError` text for a call with `argc` arguments,
    /// e.g. `strlen() expects exactly 1 argument, 2 given`.
    pub fn arity_message(&self, argc: usize) -> String {
        let min = self.min_args as usize;
        let (kind, n) = match self.max_args {
            Some(max) if max as usize == min => ("exactly", min),
            Some(max) if argc > max as usize => ("at most", max as usize),
            _ => ("at least", min),
        };
        let plural = if n == 1 { "argument" } else { "arguments" };
        format!("{}() expects {kind} {n} {plural}, {argc} given", self.name)
    }
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeFn")
            .field("name", &self.name)
            .field("min_args", &self.min_args)
            .field("max_args", &self.max_args)
            .field("by_ref", &self.by_ref)
            .finish_non_exhaustive()
    }
}

/// The per-call handle a native receives: the interpreter itself. Derefs to
/// [`Interp`], so a handler calls `ctx.out()`, `ctx.warn(..)`,
/// `ctx.call_value(..)`, `ctx.ini_get(..)` … directly.
pub struct Ctx<'a>(pub &'a mut Interp);

impl Deref for Ctx<'_> {
    type Target = Interp;
    fn deref(&self) -> &Interp {
        self.0
    }
}

impl DerefMut for Ctx<'_> {
    fn deref_mut(&mut self) -> &mut Interp {
        self.0
    }
}

/// The `Throwable` class an engine fault constructs (E5 turns these into real
/// objects; until then the class name is rendered from this tag).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ErrorKind {
    Error,
    TypeError,
    ValueError,
    ArgumentCountError,
    DivisionByZeroError,
    ArithmeticError,
    UnhandledMatchError,
    /// Any other exception class, by name (`"JsonException"`, `"Exception"`).
    Exception(&'static str),
}

impl ErrorKind {
    /// The PHP class name of the throwable this kind constructs.
    pub fn class_name(self) -> &'static str {
        match self {
            ErrorKind::Error => "Error",
            ErrorKind::TypeError => "TypeError",
            ErrorKind::ValueError => "ValueError",
            ErrorKind::ArgumentCountError => "ArgumentCountError",
            ErrorKind::DivisionByZeroError => "DivisionByZeroError",
            ErrorKind::ArithmeticError => "ArithmeticError",
            ErrorKind::UnhandledMatchError => "UnhandledMatchError",
            ErrorKind::Exception(name) => name,
        }
    }
}

/// Where a fault happened, captured at the first frame boundary the unwind
/// crosses (so the innermost frame is still on the stack): the file, the
/// line, and the rendered PHP stack trace (`#0 …\n#1 {main}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultSite {
    pub file: String,
    pub line: u32,
    pub trace: String,
}

/// An engine fault that has not been materialized as a `Throwable` object yet
/// (the class is `kind`, the message `message`). `site` is filled in by the
/// interpreter when the unwind crosses its first frame boundary.
#[derive(Clone, Debug)]
pub struct PendingThrow {
    pub kind: ErrorKind,
    pub message: String,
    pub site: Option<Box<FaultSite>>,
}

/// The single non-local control channel (ADR-022): every runtime path returns
/// `Result<_, Unwind>`.
pub enum Unwind {
    /// A thrown `Throwable` object.
    Throw(Object),
    /// An engine fault awaiting materialization (see [`PendingThrow`]).
    Pending(PendingThrow),
    /// `exit(code)` / a fatal error: unwind to the top and end the request
    /// with this exit code.
    Exit(i32),
}

impl Unwind {
    fn pending(kind: ErrorKind, message: impl Into<String>) -> Unwind {
        Unwind::Pending(PendingThrow {
            kind,
            message: message.into(),
            site: None,
        })
    }

    /// `throw new Error(msg)`.
    pub fn error(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::Error, message)
    }

    /// `throw new TypeError(msg)`.
    pub fn type_error(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::TypeError, message)
    }

    /// `throw new ValueError(msg)`.
    pub fn value_error(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::ValueError, message)
    }

    /// `throw new ArgumentCountError(msg)`.
    pub fn argument_count_error(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::ArgumentCountError, message)
    }

    /// `throw new DivisionByZeroError(msg)` (`"Division by zero"` /
    /// `"Modulo by zero"`).
    pub fn division_by_zero(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::DivisionByZeroError, message)
    }

    /// `throw new ArithmeticError(msg)`.
    pub fn arithmetic_error(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::ArithmeticError, message)
    }

    /// `throw new UnhandledMatchError(msg)`.
    pub fn unhandled_match_error(message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::UnhandledMatchError, message)
    }

    /// `throw new <class>(msg)` for any other exception class.
    pub fn exception(class: &'static str, message: impl Into<String>) -> Unwind {
        Unwind::pending(ErrorKind::Exception(class), message)
    }

    /// `exit(code)`.
    pub fn exit(code: i32) -> Unwind {
        Unwind::Exit(code)
    }

    /// The fault message, when this is a pending fault.
    pub fn message(&self) -> Option<&str> {
        match self {
            Unwind::Pending(p) => Some(&p.message),
            _ => None,
        }
    }

    /// The fault kind, when this is a pending fault.
    pub fn kind(&self) -> Option<ErrorKind> {
        match self {
            Unwind::Pending(p) => Some(p.kind),
            _ => None,
        }
    }

    /// `Uncaught <Class>: <message>` — the one-line description used when an
    /// unwind reaches the top without a PHP-shaped renderer (tests,
    /// embedders).
    pub fn describe(&self) -> String {
        match self {
            Unwind::Throw(o) => format!(
                "Uncaught {}",
                String::from_utf8_lossy(o.layout().class_name())
            ),
            Unwind::Pending(p) => format!("Uncaught {}: {}", p.kind.class_name(), p.message),
            Unwind::Exit(code) => format!("exit({code})"),
        }
    }
}

impl fmt::Debug for Unwind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unwind::Throw(o) => write!(
                f,
                "Throw({})",
                String::from_utf8_lossy(o.layout().class_name())
            ),
            Unwind::Pending(p) => f.debug_tuple("Pending").field(p).finish(),
            Unwind::Exit(c) => f.debug_tuple("Exit").field(c).finish(),
        }
    }
}

impl fmt::Display for Unwind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// The builder through which extensions add natives and constants to an
/// [`Interp`] — the only way in (ADR-014). `Registry(&mut interp)`.
pub struct Registry<'a>(pub &'a mut Interp);

impl Registry<'_> {
    /// Register one native; re-registering a name replaces the earlier
    /// descriptor under the same id.
    pub fn function(&mut self, f: NativeFn) -> NativeId {
        self.0.register_native(f)
    }

    /// Register a slice of natives (an extension module's `FUNCTIONS`).
    pub fn functions(&mut self, fs: &[NativeFn]) {
        for f in fs {
            self.0.register_native(*f);
        }
    }

    /// Register a global constant (case-sensitive name).
    pub fn constant(&mut self, name: &str, value: Value) {
        self.0.constants.insert(Box::from(name.as_bytes()), value);
    }

    /// The interpreter being populated.
    pub fn interp(&mut self) -> &mut Interp {
        self.0
    }
}

/// Build an ordinary [`NativeFn`] row: `nf!("strlen", 1, Some(1), strlen)`.
/// `$max` is `None` for a variadic function.
#[macro_export]
macro_rules! nf {
    ($name:literal, $min:expr, $max:expr, $f:path) => {
        $crate::NativeFn {
            name: $name,
            min_args: $min,
            max_args: $max,
            by_ref: 0,
            params: &[],
            flags: $crate::FnFlags::EMPTY,
            handler: $f,
        }
    };
}

/// Build a [`NativeFn`] row with by-reference parameters:
/// `nf_ref!("sort", 1, Some(2), 0b1, sort)` — `$mask` has bit `i` set when
/// argument `i` is passed by reference and written back after the call.
#[macro_export]
macro_rules! nf_ref {
    ($name:literal, $min:expr, $max:expr, $mask:expr, $f:path) => {
        $crate::NativeFn {
            name: $name,
            min_args: $min,
            max_args: $max,
            by_ref: $mask,
            params: &[],
            flags: $crate::FnFlags::EMPTY,
            handler: $f,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
        Ok(Value::Null)
    }

    #[test]
    fn arity_messages_match_php() {
        let exact = nf!("strlen", 1, Some(1), noop);
        assert_eq!(
            exact.arity_message(0),
            "strlen() expects exactly 1 argument, 0 given"
        );
        assert_eq!(
            exact.arity_message(2),
            "strlen() expects exactly 1 argument, 2 given"
        );
        let range = nf!("substr", 2, Some(3), noop);
        assert_eq!(
            range.arity_message(1),
            "substr() expects at least 2 arguments, 1 given"
        );
        assert_eq!(
            range.arity_message(4),
            "substr() expects at most 3 arguments, 4 given"
        );
        let variadic = nf!("max", 1, None, noop);
        assert_eq!(
            variadic.arity_message(0),
            "max() expects at least 1 argument, 0 given"
        );
        assert!(variadic.accepts(7));
        assert!(!range.accepts(4));
    }

    #[test]
    fn by_ref_mask_and_constructors() {
        let f = nf_ref!("preg_match", 2, Some(5), 0b100, noop);
        assert!(f.is_by_ref(2));
        assert!(!f.is_by_ref(0));
        assert_eq!(Unwind::type_error("x").kind(), Some(ErrorKind::TypeError));
        assert_eq!(
            Unwind::division_by_zero("Modulo by zero").message(),
            Some("Modulo by zero")
        );
        assert_eq!(Unwind::error("boom").describe(), "Uncaught Error: boom");
        assert_eq!(
            ErrorKind::Exception("JsonException").class_name(),
            "JsonException"
        );
        assert!(matches!(Unwind::exit(3), Unwind::Exit(3)));
    }
}
