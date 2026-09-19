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
use rphp_bytecode::{ClassFlags, ClassKind, Visibility};
use rphp_value::{Object, Value};

use crate::class::{ClassSpec, MethodBody, MethodSpec, NativeInit, NativeMethod, PropDefault};
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
/// line, and the backtrace as `Exception::getTrace()` shapes it (rendered
/// to php's `#0 …\n#1 {main}` by `Interp::trace_to_string`).
#[derive(Clone, Debug)]
pub struct FaultSite {
    pub file: String,
    pub line: u32,
    pub trace: rphp_value::Array,
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

    /// The fault message, when this is a pending fault (a thrown object's
    /// message is read with [`Unwind::message_string`]).
    pub fn message(&self) -> Option<&str> {
        match self {
            Unwind::Pending(p) => Some(&p.message),
            _ => None,
        }
    }

    /// The message of a pending fault or of a thrown object.
    pub fn message_string(&self) -> Option<String> {
        match self {
            Unwind::Pending(p) => Some(p.message.clone()),
            Unwind::Throw(o) => o.get_deref(b"message").map(|v| v.to_php_string()),
            Unwind::Exit(_) => None,
        }
    }

    /// The fault kind, when this is a pending fault.
    pub fn kind(&self) -> Option<ErrorKind> {
        match self {
            Unwind::Pending(p) => Some(p.kind),
            _ => None,
        }
    }

    /// The class name of the throwable this unwind carries (pending or
    /// materialized).
    pub fn class_name(&self) -> Option<String> {
        match self {
            Unwind::Pending(p) => Some(p.kind.class_name().to_string()),
            Unwind::Throw(o) => Some(String::from_utf8_lossy(o.layout().class_name()).into_owned()),
            Unwind::Exit(_) => None,
        }
    }

    /// `Uncaught <Class>: <message>` — the one-line description used when an
    /// unwind reaches the top without a PHP-shaped renderer (tests,
    /// embedders). A thrown object's `message` property is read directly.
    pub fn describe(&self) -> String {
        match self {
            Unwind::Throw(o) => {
                let class = String::from_utf8_lossy(o.layout().class_name()).into_owned();
                match o.get_deref(b"message") {
                    Some(Value::Str(m)) if !m.is_empty() => {
                        format!("Uncaught {class}: {}", m.to_string_lossy())
                    }
                    _ => format!("Uncaught {class}"),
                }
            }
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

    /// Start declaring a native class (plan E4, ADR-021):
    /// `r.class("Exception").implements(&["Throwable"]).prop("message",
    /// Protected, Value::string(b"")).method("getMessage", nm!(..))
    /// .native_init(f).finish()`. Parent and interfaces must already be
    /// registered when `finish` runs.
    pub fn class(&mut self, name: &str) -> ClassBuilder<'_> {
        ClassBuilder {
            interp: self.0,
            name: name.to_string(),
            kind: ClassKind::Class,
            parent: None,
            interfaces: Vec::new(),
            flags: ClassFlags::NONE,
            props: Vec::new(),
            consts: Vec::new(),
            methods: Vec::new(),
            native_init: None,
            payload_clone: None,
        }
    }

    /// Declare a native interface (no members needed for `instanceof`).
    /// Uninstantiable by kind, not by an `abstract` flag — php reports
    /// `ReflectionClass::isAbstract()` as `false` for an interface.
    pub fn interface(&mut self, name: &str) -> ClassBuilder<'_> {
        let mut b = self.class(name);
        b.kind = ClassKind::Interface;
        b
    }
}

/// The builder behind [`Registry::class`]: collects a native class's shape
/// and links it into the interpreter's class table on [`ClassBuilder::finish`].
pub struct ClassBuilder<'a> {
    interp: &'a mut Interp,
    name: String,
    kind: ClassKind,
    parent: Option<String>,
    interfaces: Vec<String>,
    flags: ClassFlags,
    props: Vec<(String, Visibility, Value)>,
    consts: Vec<(String, Value)>,
    methods: Vec<MethodSpec>,
    native_init: Option<NativeInit>,
    payload_clone: Option<crate::class::PayloadClone>,
}

impl ClassBuilder<'_> {
    /// Class / interface / trait / enum.
    pub fn kind(mut self, kind: ClassKind) -> Self {
        self.kind = kind;
        self
    }

    /// `extends <name>` (resolved on `finish`; the parent must be registered).
    pub fn extends(mut self, parent: &str) -> Self {
        self.parent = Some(parent.to_string());
        self
    }

    /// `implements <names>` (resolved on `finish`).
    pub fn implements(mut self, names: &[&str]) -> Self {
        self.interfaces.extend(names.iter().map(|n| n.to_string()));
        self
    }

    /// Add modifiers (`ABSTRACT`, `FINAL`, `ALLOW_DYNAMIC`, …).
    pub fn flags(mut self, flags: ClassFlags) -> Self {
        self.flags |= flags;
        self
    }

    /// Declare an instance property with a constant default.
    pub fn prop(mut self, name: &str, vis: Visibility, default: Value) -> Self {
        self.props.push((name.to_string(), vis, default));
        self
    }

    /// Declare a public class constant (`ArrayObject::ARRAY_AS_PROPS`,
    /// `SplDoublyLinkedList::IT_MODE_LIFO`, …). Native constants are always
    /// ready values, never lazy initializers.
    pub fn class_const(mut self, name: &str, value: Value) -> Self {
        self.consts.push((name.to_string(), value));
        self
    }

    /// Declare a public method.
    pub fn method(self, name: &str, m: NativeMethod) -> Self {
        self.method_vis(name, Visibility::Public, m)
    }

    /// Declare a method with an explicit visibility.
    pub fn method_vis(mut self, name: &str, vis: Visibility, m: NativeMethod) -> Self {
        self.methods.push(MethodSpec {
            name: Box::from(name.as_bytes()),
            body: MethodBody::Native(m),
            vis,
            is_static: m.is_static,
            is_abstract: false,
            is_final: m.is_final,
        });
        self
    }

    /// Declare an abstract method signature (interfaces).
    pub fn abstract_method(mut self, name: &str, m: NativeMethod) -> Self {
        self.methods.push(MethodSpec {
            name: Box::from(name.as_bytes()),
            body: MethodBody::Native(m),
            vis: Visibility::Public,
            is_static: m.is_static,
            is_abstract: true,
            is_final: false,
        });
        self
    }

    /// The hook run on every new instance (after the slots are seeded,
    /// before the constructor), inherited by user subclasses.
    pub fn native_init(mut self, f: NativeInit) -> Self {
        self.native_init = Some(f);
        self
    }

    /// The hook `clone` uses to copy this class's native payload. A class
    /// that keeps state in `Payload` and omits this clones to an empty one.
    pub fn payload_clone(mut self, f: crate::class::PayloadClone) -> Self {
        self.payload_clone = Some(f);
        self
    }

    /// Link and register the class; returns its process-wide id. Re-registering
    /// a name keeps the earlier id (the definition is replaced).
    ///
    /// # Panics
    /// When `extends`/`implements` name a class that is not registered:
    /// native class tables are static data, so that is a programming error
    /// caught at startup.
    pub fn finish(self) -> u32 {
        let ClassBuilder {
            interp,
            name,
            kind,
            parent,
            interfaces,
            flags,
            props,
            consts,
            methods,
            native_init,
            payload_clone,
        } = self;
        let lookup = |interp: &Interp, n: &str| {
            interp.class_by_name(n.as_bytes()).unwrap_or_else(|| {
                panic!("native class {name} refers to unregistered class {n}")
            })
        };
        let parent = parent.map(|p| lookup(interp, &p));
        let interfaces: Vec<u32> = interfaces.iter().map(|i| lookup(interp, i)).collect();
        let spec = ClassSpec {
            used_traits: Vec::new(),
            doc: None,
            name: Box::from(name.as_bytes()),
            kind,
            flags,
            parent,
            interfaces,
            props: props
                .into_iter()
                .map(|(n, vis, v)| {
                    crate::class::PropSpec::new(Box::from(n.as_bytes()), vis, PropDefault::Value(v))
                })
                .collect(),
            methods,
            native_init,
            payload_clone,
            declared_at: None,
            internal: true,
            static_props: Vec::new(),
            consts: consts
                .into_iter()
                .map(|(n, v)| crate::class::ConstSpec {
                    name: Box::from(n.as_bytes()),
                    vis: Visibility::Public,
                    is_final: false,
                    ty: None,
                    init: PropDefault::Value(v),
                })
                .collect(),
            enum_cases: Vec::new(),
            enum_backing: crate::class::EnumBacking::None,
        };
        interp.register_class_spec(spec)
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
