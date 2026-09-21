//! Compiled functions ([`Function`]) and their metadata: parameters, flags,
//! exception regions, captures, statics, attributes.

use rphp_intern::IdentId;
use rphp_span::Span;

use crate::{ClassId, CodeAddr, ConstIdx, FuncId, Reg, TypeDecl, Visibility};
use crate::{Const, Op};

/// Declare a `u32` bitflags newtype with named bits and the usual set
/// operations. Kept in-crate so the contract adds no dependency.
macro_rules! bitflags_newtype {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $( $(#[$fmeta:meta])* const $flag:ident = $bit:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $name(u32);

        impl $name {
            /// No flags set.
            pub const NONE: $name = $name(0);
            $( $(#[$fmeta])* pub const $flag: $name = $name(1 << $bit); )*

            /// The raw bits.
            pub const fn bits(self) -> u32 {
                self.0
            }

            /// Build from raw bits, dropping any bit that is not a known flag.
            pub const fn from_bits_truncate(bits: u32) -> $name {
                let known = 0 $( | (1 << $bit) )*;
                $name(bits & known)
            }

            /// Whether every flag in `other` is set in `self`.
            pub const fn contains(self, other: $name) -> bool {
                self.0 & other.0 == other.0
            }

            /// Whether any flag in `other` is set in `self`.
            pub const fn intersects(self, other: $name) -> bool {
                self.0 & other.0 != 0
            }

            /// `self` with the flags in `other` added.
            pub const fn union(self, other: $name) -> $name {
                $name(self.0 | other.0)
            }

            /// Set the flags in `other`.
            pub fn insert(&mut self, other: $name) {
                self.0 |= other.0;
            }

            /// Clear the flags in `other`.
            pub fn remove(&mut self, other: $name) {
                self.0 &= !other.0;
            }

            /// Whether no flag is set.
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// The (name, flag) pairs of every known bit, for dumps.
            pub const ALL: &'static [(&'static str, $name)] = &[ $( (stringify!($flag), $name::$flag), )* ];
        }

        impl std::ops::BitOr for $name {
            type Output = $name;
            fn bitor(self, rhs: $name) -> $name {
                self.union(rhs)
            }
        }

        impl std::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: $name) {
                self.insert(rhs);
            }
        }

        impl std::ops::BitAnd for $name {
            type Output = $name;
            fn bitand(self, rhs: $name) -> $name {
                $name(self.0 & rhs.0)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}(", stringify!($name))?;
                let mut first = true;
                for (name, flag) in $name::ALL {
                    if self.contains(*flag) {
                        if !first {
                            f.write_str(" | ")?;
                        }
                        first = false;
                        f.write_str(name)?;
                    }
                }
                if first {
                    f.write_str("NONE")?;
                }
                f.write_str(")")
            }
        }
    };
}

pub(crate) use bitflags_newtype;

bitflags_newtype! {
    /// Per-function properties the runtime consults when it builds a frame.
    pub struct FnFlags {
        /// A `static` method / `static function` closure: no `$this` is bound.
        const STATIC = 0;
        /// Contains `yield`: calling it returns a `Generator` (plan E8).
        const GENERATOR = 1;
        /// The body references `$this` (so a static-context call is an `Error`
        /// at call time and closures capture the enclosing `$this`).
        const USES_THIS = 2;
        /// The frame needs a named symbol table (uses `include`/`eval`/
        /// `compact`/`extract`/`get_defined_vars`/`$$x`, or is a file `{main}`);
        /// the compiler emits [`Op::BindSymtab`] as its first op.
        const NEEDS_SYMTAB = 3;
        /// Compiled under `declare(strict_types=1)`: governs coercion of the
        /// arguments *this function passes* and of its own return value.
        const STRICT_TYPES = 4;
        /// A closure body (`function () {}` / `fn () =>`): `captures` apply and
        /// the callable is a `Closure` object.
        const CLOSURE = 5;
        /// The last parameter is `...$rest` (`params.last().variadic`).
        const VARIADIC = 6;
        /// At least one `ex_regions` entry has a `finally` (the frame reserves
        /// state/payload registers and generator destruction must run them).
        const HAS_FINALLY = 7;
        /// `function &f()`: returns by reference ([`Op::RetRef`]).
        const RETURNS_REF = 8;
        /// Carries `#[\Deprecated]` (PHP 8.4): calling it emits the deprecation.
        const DEPRECATED = 9;
    }
}

/// A constant-expression initializer (parameter default, property default,
/// class constant, enum case value, static-variable init, attribute argument).
/// The plan calls this `Init`.
///
/// Compile-time-foldable initializers are pool constants; anything else
/// (`self::X`, `new Foo()`, `[A, B]`, enum cases, …) is compiled to a
/// zero-argument *thunk* function in the same unit that returns the value, run
/// lazily on first use in the owner's class scope (`self`/`static` resolve there)
/// and re-run per evaluation site as PHP does (each call re-evaluates a
/// parameter default; class constants are evaluated once and cached by the
/// runtime).
///
/// Which pool `Const(k)` indexes depends on the owner: a [`Function`]'s own
/// `consts` for its params/statics/attributes, and [`ClassDecl::pool`] for
/// class members.
///
/// [`ClassDecl::pool`]: crate::ClassDecl::pool
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum InitRef {
    /// A literal in the owner's constant pool.
    Const(ConstIdx),
    /// A zero-argument thunk function in the same unit.
    Thunk(FuncId),
}

/// The property a constructor parameter promotes (`public readonly int $x`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PromotedProp {
    /// Read (and, without `set_vis`, write) visibility.
    pub vis: Visibility,
    /// Asymmetric write visibility (`public private(set)`), if narrower.
    pub set_vis: Option<Visibility>,
    /// `readonly`.
    pub readonly: bool,
}

/// A declared parameter.
#[derive(Clone, PartialEq, Debug)]
pub struct ParamDef {
    /// Name without the `$`.
    pub name: Box<[u8]>,
    /// The register the argument lands in. In the v2 ABI the compiler assigns
    /// parameters registers `0 .. n` in order so the sent-argument window *is*
    /// the parameter area (`$this` is a frame slot, not a register).
    pub reg: Reg,
    /// `&$x`.
    pub by_ref: bool,
    /// `...$x` (only the last parameter; the function carries
    /// `FnFlags::VARIADIC`).
    pub variadic: bool,
    /// Default value; `None` for a required parameter. The runtime evaluates it
    /// via [`Op::RecvInit`]; Reflection reads it on demand.
    pub default: Option<InitRef>,
    /// When the default is a bare constant fetch, its name as php's
    /// `ReflectionParameter::getDefaultValueConstantName()` spells it: the
    /// namespaced candidate of an unqualified name (`N\PHP_INT_MAX`), a
    /// class constant with the class resolved (`N\C::X`), `self::X` /
    /// `parent::X` as written.
    pub default_const: Option<Box<[u8]>>,
    /// Declared type, if any.
    pub ty: Option<TypeDecl>,
    /// Constructor property promotion, if any.
    pub promoted: Option<PromotedProp>,
    /// Attributes on the parameter.
    pub attrs: Vec<AttrDef>,
}

/// Where an attribute is attached. Maps to the `Attribute::TARGET_*` bit
/// Reflection reports through [`AttrTarget::mask`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AttrTarget {
    /// `Attribute::TARGET_CLASS`
    Class,
    /// `Attribute::TARGET_FUNCTION`
    Function,
    /// `Attribute::TARGET_METHOD`
    Method,
    /// `Attribute::TARGET_PROPERTY`
    Property,
    /// `Attribute::TARGET_CLASS_CONSTANT`
    ClassConstant,
    /// `Attribute::TARGET_PARAMETER`
    Parameter,
    /// An enum case (a class constant for attribute purposes).
    EnumCase,
    /// A global `const` (PHP 8.5 `Attribute::TARGET_CONSTANT`).
    Constant,
}

impl AttrTarget {
    /// The `Attribute::TARGET_*` bit for this target.
    pub const fn mask(self) -> u32 {
        match self {
            AttrTarget::Class => 1,
            AttrTarget::Function => 2,
            AttrTarget::Method => 4,
            AttrTarget::Property => 8,
            AttrTarget::ClassConstant | AttrTarget::EnumCase => 16,
            AttrTarget::Parameter => 32,
            AttrTarget::Constant => 128,
        }
    }
}

/// A compiled `#[Name(args)]` attribute. Attributes are metadata only: the
/// runtime instantiates them when Reflection's `newInstance()` asks (validating
/// the attribute class's own `#[Attribute(flags)]` then), never at declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct AttrDef {
    /// Fully-qualified attribute class name (no leading `\`), as resolved at
    /// compile time; unresolved until `newInstance()`.
    pub name: Box<[u8]>,
    /// Arguments in order, each optionally named; values resolve through the
    /// owner's pool/thunks like any [`InitRef`].
    pub args: Vec<(Option<Box<[u8]>>, InitRef)>,
    /// What the attribute is attached to.
    pub target: AttrTarget,
    /// Source line of the attribute, for `Error` messages.
    pub line: u32,
}

/// One `catch (A|B $e)` clause of an [`ExRegion`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CatchClause {
    /// The caught class names, each a [`Const::Name`] in the function's pool.
    /// Matched by `instanceof` against the thrown object **without autoloading**
    /// (an unloaded class simply never matches). The compiler always emits at
    /// least one type.
    pub types: Vec<ConstIdx>,
    /// Where execution continues when a type matches.
    pub handler: CodeAddr,
    /// The register that receives the exception (`None` for `catch (E)` without
    /// a variable, PHP 8.0).
    pub dst: Option<Reg>,
}

/// The `finally` part of an [`ExRegion`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Finally {
    /// The finally body's code range `[lo, hi)`. An exception thrown from inside
    /// it while `state == Throw` gets the pending exception attached as its
    /// `previous` (PHP chains them); the region's own catches never apply to it.
    pub lo: CodeAddr,
    /// End (exclusive) of the finally body; `hi - 1` is the [`Op::FinallyEnd`].
    pub hi: CodeAddr,
    /// Where to jump to run the finally body (== `lo`; kept explicit so the
    /// body can be relocated by a later peephole).
    pub entry: CodeAddr,
    /// The address after the finally body — where a `FinallyState::None` end
    /// falls through to.
    pub end: CodeAddr,
    /// Register holding the pending [`FinallyState`](crate::FinallyState) code
    /// (`Int`) while the body runs. Set by the unwinder (`Throw`) or by the
    /// compiler's lowering of `return`/`break`/`continue`/`goto` before it jumps
    /// to `entry`; the compiler stores `None` on the normal path.
    pub state: Reg,
    /// Register holding the pending payload (see
    /// [`FinallyState`](crate::FinallyState)).
    pub payload: Reg,
}

/// A `try` statement's protected ranges (plan E5).
///
/// ## Unwinding rules
/// 1. The runtime walks `Function::ex_regions` **in order** and takes the first
///    region that applies to the faulting pc, so the compiler emits regions
///    innermost-first (it appends a region when its `try` statement closes,
///    which naturally puts nested regions before the enclosing ones).
/// 2. Catch matching applies when `try_lo <= pc < try_hi`: the clauses are
///    tried in order, each type by `instanceof` without autoload; the first hit
///    stores the object in `dst` and continues at `handler`.
/// 3. Otherwise, if the region has a `finally` and `try_lo <= pc <
///    finally.entry` (the try body *and* its catch handlers, which the compiler
///    lays out contiguously before the finally body), the exception becomes
///    pending: `state = Throw`, `payload = exception`, continue at
///    `finally.entry`. [`Op::FinallyEnd`] later rethrows it from its own pc,
///    which lies outside the region's ranges, so enclosing regions get their
///    turn.
/// 4. No region applies ⇒ pop the frame (running its pending `IterFree`s and,
///    for a `NEEDS_SYMTAB` frame, dropping its table) and continue in the
///    caller at *its* current pc. Popping a re-entry boundary returns
///    `Err(Unwind::Throw)` to the native that entered the VM, which propagates
///    it with `?`. An empty stack is an uncaught exception.
/// 5. A `return` lowered inside a protected range that has a `finally` jumps to
///    `finally.entry` with `state = Return`; `break`/`continue`/`goto` use
///    `state = Jump` (see [`FinallyState`](crate::FinallyState) for chaining
///    across several finally bodies, which the compiler does with trampolines).
/// 6. Destroying a suspended generator runs, innermost-first, the `finally`
///    bodies of every region whose `try_lo <= pc < finally.entry` contains the
///    parked pc, with `state = None`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ExRegion {
    /// Start of the `try` body.
    pub try_lo: CodeAddr,
    /// End (exclusive) of the `try` body.
    pub try_hi: CodeAddr,
    /// The catch clauses, in source order.
    pub catches: Vec<CatchClause>,
    /// The `finally` body, if any.
    pub finally: Option<Finally>,
}

/// A variable a closure captures from its defining scope (`use ($x, &$y)` or
/// an arrow function's implicit by-value captures).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CaptureDesc {
    /// Register in the *enclosing* frame read when the closure is created.
    pub src: Reg,
    /// Register in the closure's frame that receives it.
    pub dst: Reg,
    /// `use (&$y)`: bind to the same `Ref` cell instead of copying.
    pub by_ref: bool,
}

/// A `static $x = init;` cell declaration, bound by [`Op::BindStatic`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct StaticVar {
    /// Variable name without the `$`.
    pub name: Box<[u8]>,
    /// The register the cell is bound into.
    pub reg: Reg,
    /// Initial value (null when `None`), evaluated once per cell.
    pub init: Option<InitRef>,
    /// When the body evaluates its initializer inline (php 8.3's arbitrary
    /// initializers, `init` is `None`) and that initializer is a php
    /// constant expression: the same expression as a thunk, which is what
    /// `ReflectionFunction::getStaticVariables()` reports before the body
    /// first runs.
    pub const_expr: Option<InitRef>,
}

/// A template for [`Op::MakeClosure`]: the closure's compiled function plus the
/// enclosing-frame registers whose current values are captured (in the order the
/// closure binds them to its [`Function::capture_regs`]).
#[derive(Clone, Debug)]
pub struct ClosureProto {
    pub func: FuncId,
    pub src_regs: Vec<Reg>,
}

/// A compiled function body: code, constant pool, frame layout and (v2)
/// metadata.
///
/// The fields through `span` are the M0 set and are always populated. The
/// fields after it are the v2 contract; they default to empty/`None`
/// ([`Function::default`], [`Function::new_minimal`]) so M0 producers work
/// unchanged, and will become mandatory when the M0 fields (`capture_regs`,
/// `closures`) are removed in plan E3/E6.
#[derive(Clone, Debug)]
pub struct Function {
    pub name: IdentId,
    /// The function's name as raw bytes. The interner is a compile-time artifact,
    /// so the runtime keeps the bytes here to resolve a callable string
    /// (`'my_func'`) to a [`FuncId`] without it. Empty for the synthetic `{main}`.
    /// v2: the fully-qualified name without a leading `\` (`Ns\f`); methods
    /// carry the bare method name and `in_class`; closures `{closure}`; thunks
    /// an empty name.
    pub name_bytes: Box<[u8]>,
    /// M0: registers `0 .. num_params` receive the staged arguments (a method's
    /// register 0 is `$this`). v2: the number of declared parameters
    /// (`params.len()`); `$this` is a frame slot.
    pub num_params: u16,
    /// Total registers this frame needs (params occupy `0 .. num_params`).
    pub num_regs: u16,
    /// Registers `0 .. var_count` are the named variables (params, captures,
    /// locals), which may hold a reference cell; the rest are temporaries,
    /// which never do. An op whose result register is a variable's stores
    /// *through* a cell (`$x = …` where `$x` is by-reference), where a
    /// temporary is simply overwritten.
    pub var_count: u16,
    pub code: Vec<Op>,
    pub consts: Vec<Const>,
    /// For a closure body: the registers that captured variables bind to, in
    /// capture order (the runtime fills them from the closure's environment
    /// before running). Empty for an ordinary function. (M0; v2 uses
    /// `captures`.)
    pub capture_regs: Vec<Reg>,
    /// Closure templates referenced by this function's [`Op::MakeClosure`]s.
    /// (M0; v2 creates closures from `funcs[id]` directly.)
    pub closures: Vec<ClosureProto>,
    pub span: Span,

    // ---- v2 metadata (plan Track E) ----
    /// Declared parameters, in order.
    pub params: Vec<ParamDef>,
    /// Declared return type.
    pub ret_ty: Option<TypeDecl>,
    /// See [`FnFlags`].
    pub flags: FnFlags,
    /// Exception regions, innermost-first (see [`ExRegion`]).
    pub ex_regions: Vec<ExRegion>,
    /// Closure captures (`FnFlags::CLOSURE`).
    pub captures: Vec<CaptureDesc>,
    /// `static` variable cells, indexed by [`Op::BindStatic`]`::idx`.
    pub statics: Vec<StaticVar>,
    /// Every named variable and its register (parameters included), for
    /// symbol-table binding, `compact`/`extract`/`get_defined_vars`, and
    /// debugging. A register appears at most once.
    pub var_names: Vec<(Box<[u8]>, Reg)>,
    /// The constant pool as values (`consts[k].to_value()`), shared with
    /// every request's [`FuncRt`](../rphp_runtime) so a request's load of
    /// the function allocates nothing for it. [`Function::derive_tables`]
    /// fills it; a producer that forgets leaves it empty and the runtime
    /// derives it on load.
    pub const_values: std::rc::Rc<[rphp_value::Value]>,
    /// Register → variable name (the inverse of `var_names`), likewise.
    pub reg_names: std::rc::Rc<[Option<Box<[u8]>>]>,
    /// Source line of each op, parallel to `code` (same length), or empty when
    /// the producer has no line information yet.
    pub lines: Vec<u32>,
    /// Number of inline-cache slots the ops reference (`ic < ic_count`); the
    /// runtime allocates that many per function.
    pub ic_count: u16,
    /// The `/** … */` doc comment immediately preceding the declaration.
    pub doc: Option<Box<[u8]>>,
    /// Attributes on the function/method.
    pub attrs: Vec<AttrDef>,
    /// First line of the declaration (`ReflectionFunction::getStartLine`).
    pub decl_line: u32,
    /// Last line of the declaration.
    pub end_line: u32,
    /// The declaring class (index into the unit's `classes`) for methods, hooks
    /// and class-member thunks; `None` for free functions and closures declared
    /// outside a class body.
    pub in_class: Option<ClassId>,
}

impl Default for Function {
    /// An empty `{main}`-shaped function: no code, no registers, dummy span.
    /// Intended for struct-update construction (`Function { code, ..Default::default() }`).
    fn default() -> Self {
        Function {
            name: IdentId(0),
            name_bytes: Box::from(&b""[..]),
            num_params: 0,
            num_regs: 0,
            var_count: 0,
            code: Vec::new(),
            consts: Vec::new(),
            capture_regs: Vec::new(),
            closures: Vec::new(),
            span: Span::dummy(),
            params: Vec::new(),
            ret_ty: None,
            flags: FnFlags::NONE,
            ex_regions: Vec::new(),
            captures: Vec::new(),
            statics: Vec::new(),
            var_names: Vec::new(),
            const_values: std::rc::Rc::from(Vec::new()),
            reg_names: std::rc::Rc::from(Vec::new()),
            lines: Vec::new(),
            ic_count: 0,
            doc: None,
            attrs: Vec::new(),
            decl_line: 0,
            end_line: 0,
            in_class: None,
        }
    }
}

impl Function {
    /// Build an M0-style function from its code and pool with every v2 field
    /// empty; `name_bytes` is the resolved name (empty for `{main}`).
    pub fn new_minimal(
        name: IdentId,
        name_bytes: Box<[u8]>,
        num_params: u16,
        num_regs: u16,
        code: Vec<Op>,
        consts: Vec<Const>,
    ) -> Function {
        Function {
            name,
            name_bytes,
            num_params,
            num_regs,
            code,
            consts,
            ..Function::default()
        }
    }

    /// Number of parameters that must be passed: those before the first
    /// defaulted or variadic one.
    pub fn required_params(&self) -> usize {
        self.params
            .iter()
            .take_while(|p| p.default.is_none() && !p.variadic)
            .count()
    }

    /// Fill the derived tables (`const_values`, `reg_names`) from the pool
    /// and the variable names. The compiler calls it once per function;
    /// the runtime calls it on a function whose producer did not.
    pub fn derive_tables(&mut self) {
        self.const_values = self.consts.iter().map(Const::to_value).collect::<Vec<_>>().into();
        let mut reg_names: Vec<Option<Box<[u8]>>> = vec![None; self.num_regs as usize];
        for (name, reg) in &self.var_names {
            if let Some(slot) = reg_names.get_mut(*reg as usize) {
                *slot = Some(name.clone());
            }
        }
        self.reg_names = reg_names.into();
    }

    /// Whether [`Function::derive_tables`] has run (or nothing needs it).
    pub fn tables_derived(&self) -> bool {
        self.const_values.len() == self.consts.len() && self.reg_names.len() == self.num_regs as usize
    }

    /// The source line of the op at `pc`, if line information is present.
    pub fn line_at(&self, pc: usize) -> Option<u32> {
        self.lines.get(pc).copied()
    }

    /// Whether this function is a generator.
    pub fn is_generator(&self) -> bool {
        self.flags.contains(FnFlags::GENERATOR)
    }

    /// `function &f()`: declared to return by reference.
    pub fn returns_ref(&self) -> bool {
        self.flags.contains(FnFlags::RETURNS_REF)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fn_flags_set_operations() {
        let mut f = FnFlags::NONE;
        assert!(f.is_empty());
        f |= FnFlags::GENERATOR;
        f.insert(FnFlags::HAS_FINALLY);
        assert!(f.contains(FnFlags::GENERATOR));
        assert!(f.contains(FnFlags::GENERATOR | FnFlags::HAS_FINALLY));
        assert!(!f.contains(FnFlags::STATIC));
        assert!(f.intersects(FnFlags::STATIC | FnFlags::GENERATOR));
        f.remove(FnFlags::GENERATOR);
        assert!(!f.contains(FnFlags::GENERATOR));
        assert_eq!(f, FnFlags::HAS_FINALLY);
        assert_eq!((FnFlags::CLOSURE & FnFlags::CLOSURE), FnFlags::CLOSURE);
        assert_eq!(
            FnFlags::from_bits_truncate(u32::MAX).bits(),
            FnFlags::ALL.iter().fold(0, |a, (_, f)| a | f.bits())
        );
        assert_eq!(
            format!("{:?}", FnFlags::STATIC | FnFlags::DEPRECATED),
            "FnFlags(STATIC | DEPRECATED)"
        );
        assert_eq!(format!("{:?}", FnFlags::NONE), "FnFlags(NONE)");
    }

    #[test]
    fn default_function_is_empty_and_minimal_sets_core_fields() {
        let f = Function::default();
        assert!(f.code.is_empty() && f.params.is_empty() && f.flags.is_empty());
        assert_eq!(f.required_params(), 0);
        let g = Function::new_minimal(
            IdentId(3),
            Box::from(&b"f"[..]),
            2,
            5,
            vec![Op::Ret { src: None }],
            vec![],
        );
        assert_eq!(g.num_params, 2);
        assert_eq!(g.num_regs, 5);
        assert_eq!(g.code.len(), 1);
        assert_eq!(g.line_at(0), None);
    }

    #[test]
    fn required_params_stops_at_first_default_or_variadic() {
        let p = |default: Option<InitRef>, variadic: bool| ParamDef {
            name: Box::from(&b"x"[..]),
            reg: 0,
            by_ref: false,
            variadic,
            default,
            default_const: None,
            ty: None,
            promoted: None,
            attrs: Vec::new(),
        };
        let f = Function {
            params: vec![
                p(None, false),
                p(None, false),
                p(Some(InitRef::Const(0)), false),
                p(None, true),
            ],
            ..Function::default()
        };
        assert_eq!(f.required_params(), 2);
    }

    #[test]
    fn attr_target_masks_match_php() {
        assert_eq!(AttrTarget::Class.mask(), 1);
        assert_eq!(AttrTarget::Parameter.mask(), 32);
        assert_eq!(
            AttrTarget::EnumCase.mask(),
            AttrTarget::ClassConstant.mask()
        );
        assert_eq!(AttrTarget::Constant.mask(), 128);
    }
}
