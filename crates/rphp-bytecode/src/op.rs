//! The instruction set: [`Op`] plus the compact operand types it embeds.
//!
//! Every variant is `Copy` and the whole enum is at most 16 bytes (checked by
//! a compile-time assertion at the bottom of this file, per plan E11). Keeping
//! it that small is why class and member-name operands are packed into
//! [`ClassRef`] / [`NameRef`] `u32` newtypes instead of nested enums, and why
//! inline-cache slots are `u16`.
//!
//! The variants up to and including [`Op::Echo`] are the M0 set the current
//! compiler and runtime implement. Everything after it is the v2 contract
//! (plan Track E); no producer lowers to those yet and the tier-0 interpreter
//! rejects them with a runtime error. See `CONTRACT.md` next to `Cargo.toml`
//! for the calling convention, frame kinds and unwinding rules the v2 ops
//! assume.

use std::fmt;

use crate::{ClassId, CodeAddr, ConstIdx, FuncId, InitRef, Reg};

// ---- compact operands -------------------------------------------------------

/// How an op names a class, packed into 32 bits so it costs no more than a
/// [`ConstIdx`] inside [`Op`].
///
/// Construct with [`ClassRef::named`], [`ClassRef::reg`] or the `SELF_KW` /
/// `PARENT` / `STATIC` constants; inspect with [`ClassRef::kind`]. `Named`
/// indices are limited to 29 bits (`ClassRef::MAX_NAMED`), which no realistic
/// constant pool approaches. The `Named` index points at a
/// [`Const::Name`](crate::Const::Name) entry holding the fully-qualified class
/// name without a leading backslash.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClassRef(u32);

/// The unpacked form of a [`ClassRef`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClassRefKind {
    /// A class named by the [`Const::Name`](crate::Const::Name) at this pool
    /// index. Resolved at runtime through the class table (with autoload where
    /// the op says so).
    Named(ConstIdx),
    /// `self` — the class whose method body is executing (lexical scope).
    SelfKw,
    /// `parent` — the parent of the lexical scope class.
    Parent,
    /// `static` — the late-static-bound class of the frame.
    Static,
    /// A class held in a register: a class-name string or an object (whose
    /// class is used), as in `new $cls`, `$cls::m()`, `$obj::CONST`.
    Reg(Reg),
}

impl ClassRef {
    const TAG_SHIFT: u32 = 29;
    const PAYLOAD_MASK: u32 = (1 << Self::TAG_SHIFT) - 1;
    const TAG_NAMED: u32 = 0;
    const TAG_SELF: u32 = 1;
    const TAG_PARENT: u32 = 2;
    const TAG_STATIC: u32 = 3;
    const TAG_REG: u32 = 4;

    /// The largest constant-pool index a `Named` reference can hold.
    pub const MAX_NAMED: u32 = Self::PAYLOAD_MASK;
    /// `self`.
    pub const SELF_KW: ClassRef = ClassRef(Self::TAG_SELF << Self::TAG_SHIFT);
    /// `parent`.
    pub const PARENT: ClassRef = ClassRef(Self::TAG_PARENT << Self::TAG_SHIFT);
    /// `static`.
    pub const STATIC: ClassRef = ClassRef(Self::TAG_STATIC << Self::TAG_SHIFT);

    /// A class named by the [`Const::Name`](crate::Const::Name) at `idx`.
    ///
    /// # Panics
    /// If `idx > ClassRef::MAX_NAMED`.
    pub const fn named(idx: ConstIdx) -> ClassRef {
        assert!(
            idx <= Self::MAX_NAMED,
            "ClassRef::named: constant index exceeds 29 bits"
        );
        ClassRef((Self::TAG_NAMED << Self::TAG_SHIFT) | idx)
    }

    /// A class held in register `r` (name string or object).
    pub const fn reg(r: Reg) -> ClassRef {
        ClassRef((Self::TAG_REG << Self::TAG_SHIFT) | r as u32)
    }

    /// Unpack into the matchable [`ClassRefKind`].
    pub const fn kind(self) -> ClassRefKind {
        let payload = self.0 & Self::PAYLOAD_MASK;
        match self.0 >> Self::TAG_SHIFT {
            Self::TAG_NAMED => ClassRefKind::Named(payload),
            Self::TAG_SELF => ClassRefKind::SelfKw,
            Self::TAG_PARENT => ClassRefKind::Parent,
            Self::TAG_STATIC => ClassRefKind::Static,
            _ => ClassRefKind::Reg(payload as Reg),
        }
    }

    /// The raw packed bits (for a future encoded byte format).
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl From<ClassRefKind> for ClassRef {
    fn from(k: ClassRefKind) -> Self {
        match k {
            ClassRefKind::Named(i) => ClassRef::named(i),
            ClassRefKind::SelfKw => ClassRef::SELF_KW,
            ClassRefKind::Parent => ClassRef::PARENT,
            ClassRefKind::Static => ClassRef::STATIC,
            ClassRefKind::Reg(r) => ClassRef::reg(r),
        }
    }
}

impl fmt::Debug for ClassRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClassRef::{:?}", self.kind())
    }
}

/// A member name operand (property, method, class constant, static property):
/// either a constant-pool entry or a register holding the name at runtime
/// (`$o->$p`, `$o->$m()`, `A::{$expr}`). Packed into 32 bits.
///
/// Which pool entry kind a `Const` name points at depends on the member:
/// method names are [`Const::Name`](crate::Const::Name) (case-insensitive,
/// prelowercased); property, static-property and class-constant names are
/// [`Const::Str`](crate::Const::Str) (case-sensitive). `Reg` names are
/// converted to string at runtime (an `Error` if not stringable) and, for
/// methods, lowercased on the spot.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NameRef(u32);

/// The unpacked form of a [`NameRef`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NameRefKind {
    /// The name is the constant at this pool index.
    Const(ConstIdx),
    /// The name is the (stringable) value in this register.
    Reg(Reg),
}

impl NameRef {
    const REG_BIT: u32 = 1 << 31;

    /// The largest constant-pool index a `Const` name can hold (31 bits).
    pub const MAX_CONST: u32 = Self::REG_BIT - 1;

    /// A name held in the constant pool at `idx`.
    ///
    /// # Panics
    /// If `idx > NameRef::MAX_CONST`.
    pub const fn constant(idx: ConstIdx) -> NameRef {
        assert!(
            idx <= Self::MAX_CONST,
            "NameRef::constant: constant index exceeds 31 bits"
        );
        NameRef(idx)
    }

    /// A name computed at runtime, held in register `r`.
    pub const fn reg(r: Reg) -> NameRef {
        NameRef(Self::REG_BIT | r as u32)
    }

    /// Unpack into the matchable [`NameRefKind`].
    pub const fn kind(self) -> NameRefKind {
        if self.0 & Self::REG_BIT != 0 {
            NameRefKind::Reg((self.0 & !Self::REG_BIT) as Reg)
        } else {
            NameRefKind::Const(self.0)
        }
    }

    /// The raw packed bits (for a future encoded byte format).
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl From<NameRefKind> for NameRef {
    fn from(k: NameRefKind) -> Self {
        match k {
            NameRefKind::Const(i) => NameRef::constant(i),
            NameRefKind::Reg(r) => NameRef::reg(r),
        }
    }
}

impl fmt::Debug for NameRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NameRef::{:?}", self.kind())
    }
}

// ---- small operand enums ------------------------------------------------------

/// The target type of an explicit cast (`(int)$x`, `settype`-style).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CastKind {
    /// `(int)` / `(integer)`
    Int,
    /// `(float)` / `(double)` / `(real)`
    Float,
    /// `(string)` / `(binary)` — honours `__toString`.
    String,
    /// `(bool)` / `(boolean)`
    Bool,
    /// `(array)` — objects become arrays with mangled private/protected keys.
    Array,
    /// `(object)` — arrays become `stdClass`.
    Object,
    /// `(unset)` — removed in PHP 8; kept so the front end can report it with
    /// the exact fatal error text rather than failing to lower.
    Unset,
}

/// Which of the four file-inclusion statements an [`Op::Include`] is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IncludeKind {
    /// `include` — a missing file is a warning and evaluates to `false`.
    Include,
    /// `include_once` — as `include`, but a second inclusion of the same
    /// canonical path evaluates to `true` without running it.
    IncludeOnce,
    /// `require` — a missing file is a fatal error.
    Require,
    /// `require_once` — as `require`, with the once-semantics of `include_once`.
    RequireOnce,
}

/// The binary operator of a compound assignment (`$a OP= $b`). Every kind maps
/// to the corresponding plain binary op's semantics; `Coalesce` is `??=`, which
/// only evaluates and assigns the right-hand side when the target is unset or
/// null (the compiler emits the short-circuit branch; the op itself is the
/// assignment).
/// The comparison an [`Op::JmpUnless`] makes (the `Cmp*` ops, by name).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CmpKind {
    Eq,
    Ne,
    Identical,
    NotIdentical,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AssignOpKind {
    /// `+=`
    Add,
    /// `-=`
    Sub,
    /// `*=`
    Mul,
    /// `/=`
    Div,
    /// `%=`
    Mod,
    /// `**=`
    Pow,
    /// `.=`
    Concat,
    /// `&=`
    BitAnd,
    /// `|=`
    BitOr,
    /// `^=`
    BitXor,
    /// `<<=`
    Shl,
    /// `>>=`
    Shr,
    /// `??=`
    Coalesce,
}

/// The pending-action codes an [`ExRegion`](crate::ExRegion)'s `state` register
/// holds while its `finally` body runs, consumed by [`Op::FinallyEnd`]. Stored
/// in the register as `Value::Int(code)`.
///
/// | state | `payload` register holds | `FinallyEnd` does |
/// |---|---|---|
/// | `None` | — | falls through to the next op (`Finally::end`) |
/// | `Throw` | the pending Throwable | rethrows it from the `FinallyEnd` pc (enclosing regions apply normally) |
/// | `Return` | the return value | returns it from the frame (no further finally lookup — see below) |
/// | `Jump` | `Int(k)` | continues at the address `targets[k]` (a [`Const::JumpTable`](crate::Const::JumpTable)) |
///
/// The runtime never chains finally blocks itself: when a `return`, `break`,
/// `continue` or `goto` has to cross *several* `finally` bodies, the compiler
/// lowers the inner one as `Jump` to a trampoline stub that moves the payload
/// into the outer region's registers, sets the outer state, and jumps to the
/// outer `Finally::entry`. Only the outermost crossing uses `Return`. A `return`
/// written inside a `finally` body itself simply overwrites whatever action was
/// pending (PHP semantics: it discards a pending exception).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(i64)]
pub enum FinallyState {
    /// Nothing pending: the try/catch body completed normally.
    None = 0,
    /// An exception is pending in `payload`.
    Throw = 1,
    /// A `return` is pending; the value is in `payload`.
    Return = 2,
    /// A `break`/`continue`/`goto` is pending; `payload` indexes `targets`.
    Jump = 3,
}

impl FinallyState {
    /// The integer stored in the `state` register.
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// Decode a `state` register value; `None` for anything else (a compiler
    /// bug).
    pub const fn from_code(code: i64) -> Option<FinallyState> {
        match code {
            0 => Some(FinallyState::None),
            1 => Some(FinallyState::Throw),
            2 => Some(FinallyState::Return),
            3 => Some(FinallyState::Jump),
            _ => None,
        }
    }
}

// ---- the instruction set ------------------------------------------------------

/// One three-address instruction. Register operands are frame-local; constant
/// operands index the owning [`Function`](crate::Function)'s `consts` pool;
/// `CodeAddr` operands index its `code`.
///
/// Reads of a register that holds a `Value::Ref` dereference it (all value
/// kernels "deref first", plan E2). Only the ops whose docs say so write
/// *through* a reference in the destination register (`AssignThroughRef`,
/// `AssignOp`, `IncDec`, `RecvInit`); a plain `Move`/`LoadConst`/arithmetic
/// destination replaces the register's contents, reference cell included.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    // --- moves / constants ---
    LoadConst {
        dst: Reg,
        k: ConstIdx,
    },
    LoadNull {
        dst: Reg,
    },
    /// Release the temporaries `from..to` (set them to null): a statement's
    /// temporaries die with it, as php frees its `TMP_VAR`s — a temporary
    /// left holding an array would make the next write to that array copy
    /// it, and an object would outlive its statement.
    FreeTemps {
        from: Reg,
        to: Reg,
    },
    LoadBool {
        dst: Reg,
        val: bool,
    },
    Move {
        dst: Reg,
        src: Reg,
    },

    // --- arithmetic (dst = a OP b) ---
    Add {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Sub {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mul {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Div {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mod {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Pow {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Neg {
        dst: Reg,
        src: Reg,
    },

    // --- strings ---
    /// `dst = (string) a . (string) b`
    Concat {
        dst: Reg,
        a: Reg,
        b: Reg,
    },

    // --- arrays ---
    /// `dst = []` (a fresh empty array).
    NewArray {
        dst: Reg,
    },
    /// `dst = base[key]` (null if absent; a 1-byte substring for string bases).
    ArrayGet {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// `dst = base[key]` fetched to be modified and stored back
    /// (`$a['k']++`, php's `RW` fetch): an array element as `ArrayGet`
    /// (undefined-key warning included); an `ArrayAccess` object as the
    /// `W` fetch — its storage, or `offsetGet()`'s temporary with php's
    /// "Indirect modification" notice, which the `WriteBackElem` then
    /// drops.
    FetchElemRW {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// `arr[key] = value`, mutating the array in register `arr` in place (COW).
    /// Auto-vivifies a fresh array when `arr` holds null.
    ArraySet {
        arr: Reg,
        key: Reg,
        value: Reg,
    },
    /// `arr[] = value` (append under the next integer key).
    ArrayPush {
        arr: Reg,
        value: Reg,
    },
    /// The write-back of a nested write (`$a['x']['y'] = 1` stores the
    /// modified `$a['x']` back; `key` `None` is an append): an array is
    /// stored like `ArraySet`/`ArrayPush`; an `ArrayAccess` object gets
    /// `offsetSet()` only when its element storage is real (`ArrayObject`,
    /// `WeakMap`, …) — for any other object php modified a temporary
    /// (with its "Indirect modification" notice at the fetch) and stores
    /// nothing.
    WriteBackElem {
        arr: Reg,
        key: Option<Reg>,
        val: Reg,
    },
    /// `foreach` step: if `cursor >= len(arr)` jump to `target`; otherwise load
    /// the entry at position `cursor` into `key_dst`/`val_dst` and advance
    /// `cursor`.
    ForeachNext {
        arr: Reg,
        cursor: Reg,
        key_dst: Reg,
        val_dst: Reg,
        target: CodeAddr,
    },

    // --- comparison (dst = bool) ---
    CmpEq {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpNe {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpIdentical {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpNotIdentical {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpLt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpLe {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpGt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    CmpGe {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Spaceship {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Not {
        dst: Reg,
        src: Reg,
    },

    // --- control flow ---
    Jmp {
        target: CodeAddr,
    },
    JmpIfTrue {
        cond: Reg,
        target: CodeAddr,
    },
    JmpIfFalse {
        cond: Reg,
        target: CodeAddr,
    },
    /// Jump to `target` unless `a <kind> b` holds — a comparison and the
    /// `JmpIfFalse` on its result fused, php's smart branch. The operands
    /// may be constants ([`CONST_OPERAND`](crate::CONST_OPERAND)).
    JmpUnless {
        kind: CmpKind,
        a: Reg,
        b: Reg,
        target: CodeAddr,
    },

    // --- calls (M0 form; superseded by the Init*/Send*/DoCall sequence below) ---
    /// Call `func` with `argc` args staged in `base ..= base+argc-1`; result -> `dst`.
    Call {
        dst: Reg,
        func: FuncId,
        base: Reg,
        argc: u16,
    },
    /// Call the builtin with registry id `native` (see `rphp-stdlib`), with the
    /// same `base ..= base+argc-1` argument staging as [`Op::Call`]; result ->
    /// `dst`. The compiler range-checks `argc` against the descriptor's arity, so
    /// the runtime can pass the window through to the handler unchecked.
    CallNative {
        dst: Reg,
        native: u32,
        base: Reg,
        argc: u16,
    },
    /// Build a closure value from the enclosing function's `closures[proto]`
    /// template: snapshot the captured registers and bind them to the closure's
    /// compiled function. Result -> `dst`.
    MakeClosure {
        dst: Reg,
        proto: u32,
    },
    /// Call the callable in register `callee` (a closure or callable string) with
    /// `argc` args staged in `base ..= base+argc-1`; result -> `dst`.
    CallDynamic {
        dst: Reg,
        callee: Reg,
        base: Reg,
        argc: u16,
    },

    // --- objects (M0 form; superseded by InitNew/Fetch*/Assign* below) ---
    /// `dst = new <class>` — allocate an instance with its declared properties
    /// initialized to their defaults. The constructor (if any) is invoked by a
    /// separate `MethodCall` the compiler emits right after.
    New {
        dst: Reg,
        class: ClassId,
    },
    /// `dst = obj->{name}` where `name` is the string constant `consts[name]`
    /// (null if the property is absent, or `obj` is not an object).
    PropGet {
        dst: Reg,
        obj: Reg,
        name: ConstIdx,
    },
    /// `obj->{name} = value` (a no-op if `obj` is not an object). `name` is the
    /// string constant `consts[name]`.
    PropSet {
        obj: Reg,
        name: ConstIdx,
        value: Reg,
    },
    /// Call method `consts[method]` on the object in `obj`, with `argc` args
    /// staged in `base ..= base+argc-1`; the object is bound to the callee's
    /// `$this` (register 0). Virtual dispatch — the method is resolved on the
    /// object's runtime class, walking up the inheritance chain. Result -> `dst`.
    MethodCall {
        dst: Reg,
        obj: Reg,
        method: ConstIdx,
        base: Reg,
        argc: u16,
    },
    /// A scoped call (`self::m()` / `parent::m()` / `Class::m()`): invoke the
    /// statically-resolved `func` non-virtually, binding register `this` as the
    /// callee's `$this`. Args staged in `base ..= base+argc-1`; result -> `dst`.
    StaticCall {
        dst: Reg,
        this: Reg,
        func: FuncId,
        base: Reg,
        argc: u16,
    },
    /// `dst = (obj instanceof <class>)` — true iff `obj` is an object whose class
    /// is `class` or a descendant of it.
    InstanceOf {
        dst: Reg,
        obj: Reg,
        class: ClassId,
    },

    /// Return `src` (or null) to the caller. In a function with a declared
    /// `ret_ty` the runtime coerces/checks the value here (callee's
    /// `STRICT_TYPES` governs); in a generator this is a compiler error — use
    /// [`Op::GenReturn`].
    Ret {
        src: Option<Reg>,
    },

    // --- io ---
    Echo {
        src: Reg,
    },

    // =========================================================================
    // v2 contract (plan Track E). Nothing below is lowered by the current
    // compiler or executed by the tier-0 interpreter yet.
    // =========================================================================

    // --- call sequence (E3): Init* → Send* × n → DoCall / MakeCallableClosure ---
    /// Begin a call to the function named by `consts[name]` (a
    /// [`Const::Name`](crate::Const::Name), fully qualified, no leading `\`).
    /// If the name is not found and `ns_fallback` is `Some`, retry with that
    /// (global, unqualified) name — the namespace fallback rule. Undefined ⇒
    /// `Error: Call to undefined function X()`. Pushes a pending-call record on
    /// the frame; `ic` is the inline-cache slot for the resolved target.
    InitFCall {
        name: ConstIdx,
        ns_fallback: Option<ConstIdx>,
        ic: u16,
    },
    /// Begin a call to the callable value in `callee`: a `Closure` object, an
    /// invokable object, a `'func'` / `'A::m'` string, or a `[$objOrClass, 'm']`
    /// array (resolved via the one shared `resolve_callable` path, visibility
    /// checked against the caller's scope).
    InitDynCall {
        callee: Reg,
        ic: u16,
    },
    /// Begin `obj->name(...)`: virtual dispatch on the runtime class of `obj`,
    /// `__call` fallback; `Error` if `obj` is not an object. The callee frame
    /// gets `obj` as `$this`.
    InitMethodCall {
        obj: Reg,
        name: NameRef,
        ic: u16,
    },
    /// Begin `class::name(...)`: non-virtual on the resolved class, `__callStatic`
    /// fallback, or a forwarding instance call when the current `$this` is an
    /// instance of that class (`parent::foo()`, `self::foo()`, `A::foo()` from a
    /// subclass method). Late static binding: `parent::`/`self::` forward the
    /// caller's `static`, a named class rebinds it.
    InitStaticCall {
        class: ClassRef,
        name: NameRef,
        ic: u16,
    },
    /// Begin `new class(...)`: resolve the class (autoloading), reject
    /// abstract/interface/trait/enum instantiation, allocate the instance and
    /// begin a call to its constructor (or a no-op call when it has none — the
    /// args are still evaluated). `DoCall` yields the new object.
    InitNew {
        class: ClassRef,
        ic: u16,
    },
    /// Pass the value in `src` as positional argument `pos` (0-based; sends
    /// occur in ascending `pos` order). Used when the argument is not a
    /// variable/element/property; passing to a by-reference parameter raises
    /// `Error: X(): Argument #n could not be passed by reference` (compiler
    /// catches the literal cases as a fatal error where PHP does). `src` may
    /// be a constant operand ([`CONST_OPERAND`](crate::CONST_OPERAND)).
    SendVal {
        pos: u16,
        src: Reg,
    },
    /// Pass variable register `var` as argument `pos`: by reference (the register
    /// is turned into a `Ref` and shared) if the resolved callee declares
    /// parameter `pos` by-ref, otherwise by value (a deref'd copy).
    SendVar {
        pos: u16,
        var: Reg,
    },
    /// Pass a function-call result (`f(g())`, `f(new X)`) as argument `pos`
    /// (E5 addition, CONTRACT.md §8.1): by value, unless the resolved callee
    /// declares the parameter by-reference, in which case php's `Notice:
    /// Only variables should be passed by reference` is emitted and a fresh
    /// reference cell holding the value is passed.
    SendFuncResult {
        pos: u16,
        src: Reg,
    },
    /// Pass `arr[key]` as argument `pos`: a `Ref` to the (autovivified) element
    /// if the parameter is by-ref, else its value (with the usual undefined-key
    /// warning).
    SendRefElem {
        pos: u16,
        arr: Reg,
        key: Reg,
    },
    /// Pass `obj->name` as argument `pos`: a `Ref` to the property if the
    /// parameter is by-ref, else its value.
    SendRefProp {
        pos: u16,
        obj: Reg,
        name: NameRef,
    },
    /// Pass `Class::$name` as argument `pos`: the shared cell if the
    /// parameter is by-ref (php's "Cannot access uninitialized … by
    /// reference" for a typed property with no value), else its value
    /// (the plain read and its own error).
    SendRefStaticProp {
        pos: u16,
        class: ClassRef,
        name: NameRef,
    },
    /// Branch to `target` unless the innermost pending call takes positional
    /// argument `pos` by reference: php's `FETCH_*_FUNC_ARG` decision, made
    /// once here so a nested place (`f($this->list['k'])`) is fetched for
    /// writing only when the parameter really is by-reference, and read
    /// (no autovivification, no readonly violation) otherwise.
    JmpUnlessArgByRef {
        pos: u16,
        target: CodeAddr,
    },
    /// Spread `src` (`...$args`): an array or `Traversable`; string keys become
    /// named arguments (PHP 8.1), which must come after all positional ones.
    SendUnpack {
        src: Reg,
    },
    /// Pass `src` as the named argument `consts[name]` (a `Const::Str`). The
    /// runtime maps it to the parameter position of the resolved callee or, for
    /// a variadic callee, into the variadic array under that string key. Named
    /// sends follow all positional sends.
    SendNamed {
        name: ConstIdx,
        src: Reg,
    },
    /// Execute the pending call: arity/type checks (`ArgumentCountError`,
    /// `TypeError` per the caller's `STRICT_TYPES`), push the callee frame whose
    /// registers `0 .. argc` *are* the sent argument window (zero-copy), run it,
    /// and store the result in `dst`. For `InitNew` the result is the new object.
    DoCall {
        dst: Reg,
    },
    /// First-class callable syntax `f(...)` / `$o->m(...)` / `A::m(...)`: turn
    /// the pending-call record (which must have no sent arguments) into a
    /// `Closure` object bound to the resolved target, `$this` and scope, without
    /// calling it. Result -> `dst`.
    MakeCallableClosure {
        dst: Reg,
    },
    /// `return` from a `function &f()`: return the `Ref` in `var` (making the
    /// register a reference if it is not one yet) so the caller can bind to it
    /// (`$x = &f()`). Return-type checks apply as for [`Op::Ret`].
    RetRef {
        var: Reg,
    },
    /// `return <expression>` from a `function &f()` when the expression is
    /// not a place: php's `Notice: Only variable references should be
    /// returned by reference`, then the value in a fresh cell.
    RetRefTemp {
        src: Option<Reg>,
    },

    // --- prologue (E3) ---
    /// Runs only when parameter `param` (0-based) was not passed
    /// (`argc <= param`): evaluate `init` — a pool constant, or a 0-argument
    /// thunk function run in the callee's class scope — into the parameter's
    /// register, then coerce it to the parameter type like a passed value.
    /// Positioned by the compiler at the top of the body, one per defaulted
    /// parameter, in parameter order.
    RecvInit {
        param: u16,
        init: InitRef,
    },
    /// Collect the arguments beyond the declared parameters (positional extras
    /// in order, then named extras under their string keys) into an array in
    /// `reg` — the `...$rest` parameter. A `VARIADIC` function has exactly one.
    RecvVariadic {
        reg: Reg,
    },
    /// `static $x = init;` — bind register `reg` to the per-function (per
    /// closure object / per class for methods) static cell
    /// `Function::statics[idx]`, creating it from the cell's `init` on first
    /// execution. The register then holds a `Ref` to the cell.
    BindStatic {
        reg: Reg,
        idx: u16,
    },
    /// Warn if the variable in `reg` has never been assigned: php's
    /// `Warning: Undefined variable $name`, where `consts[name]` is a
    /// `Const::Str`. Emitted before a *read* of a local the compiler cannot
    /// prove assigned; the value stays uninitialized (and reads as `null`),
    /// so a second read warns again, as php does.
    CheckVar {
        reg: Reg,
        name: ConstIdx,
    },
    /// `global $x;` — bind register `reg` to the global symbol table entry
    /// named `consts[name]` (a `Const::Str`), creating a null entry if absent.
    /// The register then holds a `Ref` to it.
    BindGlobal {
        reg: Reg,
        name: ConstIdx,
    },
    /// Prologue of a `NEEDS_SYMTAB` function: give the frame a named symbol
    /// table and bind every `Function::var_names` register to a `Ref` cell in
    /// it (an `Include` frame reuses the includer's table instead of creating
    /// one; `{main}` of the entry script binds to the globals table).
    BindSymtab,

    // --- references / lvalues (E2, E3) ---
    /// Ensure register `var` holds a `Ref`: wrap its current value (null if
    /// unset) in a fresh reference cell in place. No-op if already a `Ref`.
    MakeRef {
        var: Reg,
    },
    /// `$dst = &$src`: make `src` a reference (as [`Op::MakeRef`]) and bind `dst`
    /// to the same cell, discarding whatever `dst` held.
    AssignRef {
        dst: Reg,
        src: Reg,
    },
    /// The general variable assignment `$dst = src`: if `dst` holds a `Ref`
    /// write the (deref'd) value into the cell; otherwise store it in the
    /// register. The compiler uses this for every named variable that may be a
    /// reference and the cheaper [`Op::Move`] for the rest.
    AssignThroughRef {
        dst: Reg,
        src: Reg,
    },
    /// `dst = *src`: copy the value behind `src`'s reference (or `src` itself if
    /// not a reference). Never produces a `Ref`.
    Deref {
        dst: Reg,
        src: Reg,
    },
    /// `$dst = &$arr[$key]` / `$dst = &$arr[]` (`key` `None`, which appends a
    /// fresh null element first): turn the element (autovivified: null base
    /// becomes an array, missing key becomes null) into a `Ref` in place and
    /// bind `dst` to it. `ArrayAccess` objects raise the usual
    /// indirect-modification notice.
    RefElem {
        dst: Reg,
        arr: Reg,
        key: Option<Reg>,
    },
    /// `$dst = &$obj->name`: as [`Op::RefElem`] for a property (declared or
    /// dynamic; `__get` results cannot be referenced).
    RefProp {
        dst: Reg,
        obj: Reg,
        name: NameRef,
    },
    /// `$dst = &Class::$name`: bind `dst` to the static property's shared cell.
    RefStaticProp {
        dst: Reg,
        class: ClassRef,
        name: NameRef,
    },
    /// `[..., ...$src, ...]`: append `src`'s elements to the array being built
    /// in `arr`. php **renumbers integer keys** (so two spreads never collide)
    /// and **preserves string keys** (8.1+), with a later one overwriting an
    /// earlier. `src` may be any array or `Traversable`, including a
    /// `Generator`; anything else is a `TypeError`.
    ArrayUnpack {
        arr: Reg,
        src: Reg,
    },
    /// `Class::$name = &src`: bind the static property's slot to the reference
    /// cell in `src`, so both names share one value afterwards.
    AssignRefStaticProp {
        class: ClassRef,
        name: NameRef,
        src: Reg,
    },
    /// `unset(Class::$name)`: php never actually removes a static property —
    /// it throws `Error: Attempt to unset static property C::$p`, naming the
    /// **resolved** class and the property whether or not it exists. The op
    /// exists so the compiler can defer that resolution to run time.
    UnsetStaticProp {
        class: ClassRef,
        name: NameRef,
    },
    /// Fetch-for-write: make `arr[key]` (or, when `key` is `None`, a freshly
    /// appended element) a write handle in `dst` so a nested lvalue op can
    /// modify it in place (`$a[i][j] = v` ⇒ `FetchElemW t,a,i; ArraySet t,j,v`).
    /// Autovivifies a null/unset base into an array. The handle is a `Ref` to the
    /// slot; the compiler must consume it with the immediately following lvalue
    /// op(s) and never let it escape into a user-visible variable.
    FetchElemW {
        dst: Reg,
        arr: Reg,
        key: Option<Reg>,
    },
    /// Fetch-for-write of `obj->name` (see [`Op::FetchElemW`]); a null base is an
    /// `Error` (PHP 8 no longer autovivifies objects).
    FetchPropW {
        dst: Reg,
        obj: Reg,
        name: NameRef,
    },
    /// `unset($var)`: reset the register to `Uninit`, dropping any reference
    /// binding (the cell itself survives for other holders).
    UnsetVar {
        var: Reg,
    },
    /// `unset($$name)`: remove the entry named by the string in `name` from
    /// the frame's symbol table, and reset the frame's own register of that
    /// name, if it has one, as [`Op::UnsetVar`] would. The frame is
    /// `NEEDS_SYMTAB`.
    UnsetDynVar {
        name: Reg,
    },
    /// `unset($arr[$key])`: remove the key (tombstone); silent if absent;
    /// `ArrayAccess::offsetUnset` on objects; `Error` on strings.
    UnsetElem {
        arr: Reg,
        key: Reg,
    },
    /// `unset($obj->name)`: remove the property (`__unset` fallback); `Error` on
    /// readonly properties.
    UnsetProp {
        obj: Reg,
        name: NameRef,
    },
    /// `dst = isset($var)`: true iff the register holds a non-null, initialized
    /// value (through references).
    IssetVar {
        dst: Reg,
        var: Reg,
    },
    /// `dst = isset($arr[$key])`: no warnings; `ArrayAccess::offsetExists` (then
    /// `offsetGet !== null`) on objects; byte-offset check on strings.
    IssetElem {
        dst: Reg,
        arr: Reg,
        key: Reg,
    },
    /// `dst = isset($obj->name)`: no warnings; `__isset` fallback; visibility
    /// respected (an inaccessible property counts as unset).
    IssetProp {
        dst: Reg,
        obj: Reg,
        name: NameRef,
    },
    /// `dst = isset(Class::$name)`: false (no error) if the class or property does
    /// not exist or is inaccessible.
    IssetStaticProp {
        dst: Reg,
        class: ClassRef,
        name: NameRef,
    },
    /// `dst = empty($var)`: `!isset || !truthy`, never warns.
    EmptyVar {
        dst: Reg,
        var: Reg,
    },
    /// `dst = empty($arr[$key])`.
    EmptyElem {
        dst: Reg,
        arr: Reg,
        key: Reg,
    },
    /// `dst = empty($obj->name)`.
    EmptyProp {
        dst: Reg,
        obj: Reg,
        name: NameRef,
    },
    /// `dst = base[key]` without any "undefined key/offset" warning — the read
    /// half of `??` on array elements; null when absent or when `base` is not
    /// indexable.
    ArrayGetQuiet {
        dst: Reg,
        base: Reg,
        key: Reg,
    },

    // --- names (E3, E6, E7) ---
    /// `dst = CONSTANT`: look `consts[name]` (a `Const::Name`, fully qualified)
    /// up in the constant table; the namespace part is case-insensitive, the
    /// last segment is not. On a miss with `ns_fallback` set, retry with that
    /// global name. Still undefined ⇒ `Error: Undefined constant "X"`.
    FetchConst {
        dst: Reg,
        name: ConstIdx,
        ns_fallback: Option<ConstIdx>,
    },
    /// `dst = Class::NAME` (also enum cases and `Class::class` when the class
    /// operand is dynamic): resolves and autoloads the class, evaluates the
    /// constant's initializer lazily on first access (cycle ⇒ `Error`), checks
    /// visibility against the scope. `ic` caches the resolved (class, value).
    FetchClassConst {
        dst: Reg,
        class: ClassRef,
        name: NameRef,
        ic: u16,
    },
    /// `dst = Class::$name` (read; static props are shared cells, so this derefs).
    FetchStaticProp {
        dst: Reg,
        class: ClassRef,
        name: NameRef,
        ic: u16,
    },
    /// `Class::$name = src` (writes through the shared cell; type coercion and
    /// readonly-ness apply).
    AssignStaticProp {
        class: ClassRef,
        name: NameRef,
        src: Reg,
        ic: u16,
    },
    /// `dst = obj->name` (read, with an inline cache): declared-slot or dynamic
    /// property, `__get` fallback, "Undefined property" warning, visibility
    /// and property-hook (`get`) semantics. Supersedes [`Op::PropGet`].
    FetchProp {
        dst: Reg,
        obj: Reg,
        name: NameRef,
        ic: u16,
    },
    /// `obj->name = src` with an inline cache: typed-property coercion, readonly
    /// and asymmetric-visibility checks, `set` hooks, `__set` fallback, the
    /// dynamic-property deprecation. Supersedes [`Op::PropSet`].
    AssignProp {
        obj: Reg,
        name: NameRef,
        src: Reg,
        ic: u16,
    },
    /// Resolve the class named `consts[name]` (a `Const::Name`) — or, when the
    /// constant is absent from the class table and `autoload` is set, through
    /// the autoloader stack — into `dst` as a class handle for a following op
    /// with `ClassRef::reg`. Undefined ⇒ `Error: Class "X" not found`.
    FetchClass {
        dst: Reg,
        name: ConstIdx,
        autoload: bool,
    },
    /// `dst = $GLOBALS` — a read-only array copy of the global symbol table
    /// (PHP 8.1 semantics); writes go through [`Op::AssignGlobal`].
    FetchGlobals {
        dst: Reg,
    },
    /// `$GLOBALS[key] = src` — write-through into the global symbol table.
    AssignGlobal {
        key: Reg,
        src: Reg,
    },
    /// `dst = $this`; `Error: Using $this when not in object context` when the
    /// frame has no bound object. (`$this` lives in the frame, not in a
    /// register, so that the argument window can start at register 0.)
    LoadThis {
        dst: Reg,
    },

    // --- control / misc (E5, E7, E8) ---
    /// Multi-way branch: compare `src` against the keys of the
    /// [`Const::JumpTable`](crate::Const::JumpTable) at `table` in order, jump
    /// to the first match, else to `default`. `strict` selects `===` (`match`)
    /// over `==` (`switch`); a `match` with no arm hit lowers its `default` to
    /// an [`Op::MatchError`].
    Switch {
        src: Reg,
        table: ConstIdx,
        default: CodeAddr,
        strict: bool,
    },
    /// Throw `UnhandledMatchError` for the value in `src` with PHP's message
    /// (`Unhandled match case 5`, `... 'str'`, `... of type Foo`).
    MatchError {
        src: Reg,
    },
    /// `@`-operator bracket: `begin` pushes a silence level (error_reporting
    /// masked to fatal errors), `!begin` pops it. Unwinding through the bracket
    /// restores the level (the frame records its `silence_base`).
    Silence {
        begin: bool,
    },
    /// `include`/`require` (and `_once`): resolve `path` (absolute → include_path
    /// → the including file's directory → cwd), compile/load the unit and run
    /// its `{main}` as an `Include` frame sharing this frame's symbol table,
    /// `$this` and scope. `dst` receives the file's `return` value, or `1`, or
    /// `false` / `true` per [`IncludeKind`].
    Include {
        dst: Reg,
        path: Reg,
        kind: IncludeKind,
    },
    /// `eval(src)`: compile the string as a unit named `<file>(<line>) : eval()'d
    /// code`, `ParseError` on failure, run it like an include sharing the symbol
    /// table; `dst` = its return value (null when none).
    Eval {
        dst: Reg,
        src: Reg,
    },
    /// Declare the unit function `funcs[idx]` in the function table now (a
    /// conditional or nested declaration; unconditional top-level ones are
    /// hoisted at unit load). Redeclaration ⇒ fatal error.
    DeclareFunction {
        idx: FuncId,
    },
    /// Declare and link the unit class `classes[idx]` now (parent/interfaces
    /// resolved with autoload). Redeclaration ⇒ fatal error.
    DeclareClass {
        idx: ClassId,
    },
    /// `const NAME = src;` at file/namespace scope: define the constant named
    /// `consts[name]` (a `Const::Name`); a redefinition is a warning.
    DeclareConst {
        name: ConstIdx,
        src: Reg,
    },
    /// `exit`/`die`: an int is the exit code; a string is echoed with code 0;
    /// `None` is code 0. Unwinds every frame (running `finally` blocks is *not*
    /// PHP behaviour; shutdown functions and destructors still run).
    Exit {
        src: Option<Reg>,
    },
    /// Throw the `Throwable` object in `src` (`Error: Can only throw objects`
    /// otherwise). Unwinding follows [`ExRegion`](crate::ExRegion).
    Throw {
        src: Reg,
    },
    /// The last op of every `finally` body: dispatch on the [`FinallyState`]
    /// code in `state` with `payload`; `targets` is the `Const::JumpTable` used
    /// by the `Jump` state.
    FinallyEnd {
        state: Reg,
        payload: Reg,
        targets: ConstIdx,
    },
    /// `dst = yield key => val` in a `GENERATOR` function: park the frame,
    /// publish (`key` or the auto-key, `val` or null) as the current pair, and
    /// on resumption store the sent value (null for `next()`) in `dst`, or throw
    /// the exception passed to `Generator::throw()` here.
    Yield {
        dst: Reg,
        key: Option<Reg>,
        val: Option<Reg>,
    },
    /// `dst = yield from src`: delegate to an array/`Traversable`/generator;
    /// `dst` = the inner generator's return value (null for non-generators).
    YieldFrom {
        dst: Reg,
        src: Reg,
    },
    /// `return` inside a generator: record the value for `getReturn()` and
    /// finish the generator (no frame result).
    GenReturn {
        src: Option<Reg>,
    },
    /// `dst = clone src` (`Error` on non-objects; `__clone` runs on the copy;
    /// readonly re-initialization allowed inside it). `with` is the PHP 8.5
    /// `clone($o, [...])` property-override array, applied before `__clone`.
    Clone {
        dst: Reg,
        src: Reg,
        with: Option<Reg>,
    },
    /// Start iterating `src` into iterator register `it`, positioned *before*
    /// the first element: arrays (a snapshot, or in-place with `by_ref`),
    /// `Iterator` (`rewind()`), `IteratorAggregate` (`getIterator()` chain),
    /// generators, and native iterator payloads. A non-iterable raises the
    /// "foreach() argument must be of type array|object" warning and the loop
    /// is skipped (the following `IterNext` jumps out).
    IterInit {
        it: Reg,
        src: Reg,
        by_ref: bool,
    },
    /// Advance `it`; if exhausted jump to `target`, else store the current key
    /// (when `key` is given) and value (a `Ref` for by-ref loops) into the
    /// registers. Objects go through `valid()/current()/key()/next()`.
    IterNext {
        it: Reg,
        key: Option<Reg>,
        val: Reg,
        target: CodeAddr,
    },
    /// Release the iterator in `it` (also emitted on every `break`/`return`
    /// path out of the loop; unwinding releases it implicitly).
    IterFree {
        it: Reg,
    },
    /// `dst = obj instanceof class` for a late-bound class operand: false (never
    /// an error, never autoloads) when the class is not loaded or `obj` is not
    /// an object. Supersedes [`Op::InstanceOf`]; a `ClassRef::reg` operand
    /// covers `instanceof $x` (`Error` if the value is neither string nor
    /// object).
    InstanceOfRef {
        dst: Reg,
        obj: Reg,
        class: ClassRef,
    },

    // --- operators (E3) ---
    /// `$var OP= src` on a variable register (writes through a reference).
    AssignOp {
        op: AssignOpKind,
        var: Reg,
        src: Reg,
    },
    /// `$arr[$key] OP= src` (autovivifies; `ArrayAccess` read-modify-write).
    AssignOpElem {
        op: AssignOpKind,
        arr: Reg,
        key: Reg,
        src: Reg,
    },
    /// `$obj->name OP= src` (typed-property coercion after the operation).
    AssignOpProp {
        op: AssignOpKind,
        obj: Reg,
        name: NameRef,
        src: Reg,
    },
    /// `++$var` / `$var++` / `--` (`pre` = prefix form, `inc` = increment):
    /// mutate `var` in place (through a reference) with PHP's inc/dec rules
    /// (string increment, null++ ⇒ 1, null-- ⇒ null) and put the expression's
    /// value — the new value for prefix, the old one for postfix — in `dst`
    /// when given.
    IncDec {
        var: Reg,
        dst: Option<Reg>,
        pre: bool,
        inc: bool,
    },
    /// `dst = (kind) src`.
    Cast {
        dst: Reg,
        src: Reg,
        kind: CastKind,
    },
    /// `dst = a & b` (ints, or bytewise on two strings).
    BitAnd {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `dst = a | b`.
    BitOr {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `dst = a ^ b`.
    BitXor {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `dst = a << b` (`ArithmeticError` on a negative shift).
    Shl {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `dst = a >> b`.
    Shr {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `dst = ~src` (ints, floats truncated, bytewise on strings; `TypeError`
    /// otherwise).
    BitNot {
        dst: Reg,
        src: Reg,
    },
    /// Unary `+src`: numeric conversion (`TypeError` for arrays/objects).
    Plus {
        dst: Reg,
        src: Reg,
    },
    /// `dst = base . base+1 . … . base+n-1` — an interpolated string or a chain
    /// of `.` in one op (each part stringified via `__toString` where needed).
    ConcatN {
        dst: Reg,
        base: Reg,
        n: u16,
    },

    // --- E3 additions (CONTRACT.md §8): reference stores into containers and
    // symbol-table access by a runtime name ---
    /// `$arr[$key] = &$src` / `$arr[] = &$src` (`key` `None`): make `src` a
    /// reference (as [`Op::MakeRef`]) and bind the element to the same cell,
    /// replacing whatever the element held. Autovivifies a null base.
    AssignRefElem {
        arr: Reg,
        key: Option<Reg>,
        src: Reg,
    },
    /// `$obj->name = &$src`: make `src` a reference and bind the property
    /// (declared or dynamic) to the same cell.
    AssignRefProp {
        obj: Reg,
        name: NameRef,
        src: Reg,
    },
    /// `dst = $$name` (`global` = `$GLOBALS[$name]`): read the symbol-table
    /// entry named by the string in `name` from the frame's symbol table (or
    /// the globals table); null (no entry created) when absent. The frame must
    /// be `NEEDS_SYMTAB` for the local form.
    FetchDynVar {
        dst: Reg,
        name: Reg,
        global: bool,
    },
    /// Bind register `reg` to the symbol-table cell named by the string in
    /// `name` (local table, or the globals table when `global`), creating a
    /// null entry if absent, so that a following [`Op::AssignThroughRef`] /
    /// [`Op::Deref`] / lvalue op on `reg` reads and writes the named variable
    /// (`$$name = v`, `$GLOBALS[$name][..] = v`).
    BindDynVar {
        reg: Reg,
        name: Reg,
        global: bool,
    },
    /// `static $x = <expr>;` with a non-constant initializer, php 8.3
    /// semantics (`BIND_INIT_STATIC_OR_JMP`): if the cell
    /// `Function::statics[idx]` already exists, bind `reg` to it and jump to
    /// `target` (skipping the inline initializer code that follows);
    /// otherwise fall through. The initializer is compiled inline in the
    /// function's own scope (it may read its variables and recurse), and is
    /// followed by a second `BindStaticOrJmp` (a recursive call may have
    /// created the cell meanwhile — that one wins) and a plain
    /// [`Op::BindStatic`] + [`Op::AssignThroughRef`] storing the value.
    BindStaticOrJmp {
        reg: Reg,
        idx: u16,
        target: CodeAddr,
    },
    /// `dst = base[key]` as a destructuring pattern reads it (`[$a] = $x`):
    /// like [`Op::ArrayGet`] on arrays (missing key warns), silently null on
    /// a null source, `Cannot use int as array` warning + null on other
    /// scalars, `Error` on objects.
    ListGet {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
}

// `Op` is copied on every dispatch and stored in `Vec<Op>`; keeping it at two
// words is the E11 budget. `Option<Reg>` is 4 bytes and `Option<ConstIdx>` is 8
// bytes (no niche in plain integers), which is what bounds the widest variants
// (`InitFCall`, `FetchConst`) at 14 bytes of payload plus the tag.
const _: () = assert!(
    std::mem::size_of::<Op>() <= 16,
    "Op must stay within 16 bytes (plan E11)"
);
impl Op {
    /// The register an op computes its result into, for the ops whose
    /// result can be redirected to a variable's register (`$x = $a + $b`
    /// computes straight into `$x`, see `Function::var_count`): the
    /// operators, a constant load, an element or property read.
    pub fn result_reg(&self) -> Option<Reg> {
        match self {
            Op::LoadConst { dst, .. }
            | Op::Add { dst, .. }
            | Op::Sub { dst, .. }
            | Op::Mul { dst, .. }
            | Op::Div { dst, .. }
            | Op::Mod { dst, .. }
            | Op::Pow { dst, .. }
            | Op::Neg { dst, .. }
            | Op::Concat { dst, .. }
            | Op::ArrayGet { dst, .. }
            | Op::CmpEq { dst, .. }
            | Op::CmpNe { dst, .. }
            | Op::CmpIdentical { dst, .. }
            | Op::CmpNotIdentical { dst, .. }
            | Op::CmpLt { dst, .. }
            | Op::CmpLe { dst, .. }
            | Op::CmpGt { dst, .. }
            | Op::CmpGe { dst, .. }
            | Op::Spaceship { dst, .. }
            | Op::Not { dst, .. }
            | Op::FetchProp { dst, .. }
            | Op::BitAnd { dst, .. }
            | Op::BitOr { dst, .. }
            | Op::BitXor { dst, .. }
            | Op::Shl { dst, .. }
            | Op::Shr { dst, .. }
            | Op::BitNot { dst, .. }
            | Op::Plus { dst, .. } => Some(*dst),
            _ => None,
        }
    }

    /// Redirect the result of an op [`Op::result_reg`] answers for.
    pub fn set_result_reg(&mut self, r: Reg) {
        match self {
            Op::LoadConst { dst, .. }
            | Op::Add { dst, .. }
            | Op::Sub { dst, .. }
            | Op::Mul { dst, .. }
            | Op::Div { dst, .. }
            | Op::Mod { dst, .. }
            | Op::Pow { dst, .. }
            | Op::Neg { dst, .. }
            | Op::Concat { dst, .. }
            | Op::ArrayGet { dst, .. }
            | Op::CmpEq { dst, .. }
            | Op::CmpNe { dst, .. }
            | Op::CmpIdentical { dst, .. }
            | Op::CmpNotIdentical { dst, .. }
            | Op::CmpLt { dst, .. }
            | Op::CmpLe { dst, .. }
            | Op::CmpGt { dst, .. }
            | Op::CmpGe { dst, .. }
            | Op::Spaceship { dst, .. }
            | Op::Not { dst, .. }
            | Op::FetchProp { dst, .. }
            | Op::BitAnd { dst, .. }
            | Op::BitOr { dst, .. }
            | Op::BitXor { dst, .. }
            | Op::Shl { dst, .. }
            | Op::Shr { dst, .. }
            | Op::BitNot { dst, .. }
            | Op::Plus { dst, .. } => *dst = r,
            _ => {}
        }
    }
}

const _: () = assert!(std::mem::size_of::<ClassRef>() == 4);
const _: () = assert!(std::mem::size_of::<NameRef>() == 4);
const _: () = assert!(std::mem::size_of::<InitRef>() == 8);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ref_round_trips() {
        assert_eq!(ClassRef::named(0).kind(), ClassRefKind::Named(0));
        assert_eq!(
            ClassRef::named(ClassRef::MAX_NAMED).kind(),
            ClassRefKind::Named(ClassRef::MAX_NAMED)
        );
        assert_eq!(ClassRef::SELF_KW.kind(), ClassRefKind::SelfKw);
        assert_eq!(ClassRef::PARENT.kind(), ClassRefKind::Parent);
        assert_eq!(ClassRef::STATIC.kind(), ClassRefKind::Static);
        assert_eq!(ClassRef::reg(u16::MAX).kind(), ClassRefKind::Reg(u16::MAX));
        for k in [
            ClassRefKind::Named(42),
            ClassRefKind::SelfKw,
            ClassRefKind::Parent,
            ClassRefKind::Static,
            ClassRefKind::Reg(7),
        ] {
            assert_eq!(ClassRef::from(k).kind(), k);
        }
        assert_ne!(ClassRef::named(0), ClassRef::SELF_KW);
        assert_eq!(format!("{:?}", ClassRef::reg(3)), "ClassRef::Reg(3)");
    }

    #[test]
    fn name_ref_round_trips() {
        assert_eq!(NameRef::constant(0).kind(), NameRefKind::Const(0));
        assert_eq!(
            NameRef::constant(NameRef::MAX_CONST).kind(),
            NameRefKind::Const(NameRef::MAX_CONST)
        );
        assert_eq!(NameRef::reg(0).kind(), NameRefKind::Reg(0));
        assert_eq!(NameRef::reg(u16::MAX).kind(), NameRefKind::Reg(u16::MAX));
        assert_ne!(NameRef::constant(5), NameRef::reg(5));
        assert_eq!(format!("{:?}", NameRef::constant(9)), "NameRef::Const(9)");
    }

    #[test]
    fn finally_state_codes_round_trip() {
        for s in [
            FinallyState::None,
            FinallyState::Throw,
            FinallyState::Return,
            FinallyState::Jump,
        ] {
            assert_eq!(FinallyState::from_code(s.code()), Some(s));
        }
        assert_eq!(FinallyState::from_code(4), None);
        assert_eq!(FinallyState::from_code(-1), None);
    }

    #[test]
    fn op_is_two_words() {
        assert!(std::mem::size_of::<Op>() <= 16);
    }
}
