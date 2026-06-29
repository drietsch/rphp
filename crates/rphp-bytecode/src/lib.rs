//! Register bytecode for the M0 slice.
//!
//! Three-address, register-based (per `specs/base/05-bytecode-isa.md`). M0 keeps
//! the program as in-memory `Vec<Op>` rather than the encoded byte format; the
//! variable-length encoding, IC slots, and metadata blocks come later. This is
//! the contract shared by `rphp-compiler` (producer) and `rphp-runtime`
//! (consumer).
//!
//! ## Calling convention
//! Registers are local to a frame. A `Call { dst, func, base, argc }` evaluates
//! arguments into the contiguous window `base ..= base+argc-1` of the *caller's*
//! frame, then a fresh callee frame is created whose registers `0 .. argc` are
//! initialized from that window (M0 copies; the spec's zero-copy window is a
//! later refinement). The callee returns into the caller's `dst` register via
//! `Ret`.
#![forbid(unsafe_code)]

use rphp_intern::IdentId;
use rphp_span::Span;
use rphp_value::{Str, Value, Vis};

/// A register index within a frame.
pub type Reg = u16;
/// An index into a function's constant pool.
pub type ConstIdx = u32;
/// An index into `Module::funcs`.
pub type FuncId = u32;
/// An index into `Module::classes`.
pub type ClassId = u32;
/// An instruction index within `Function::code` (a branch target).
pub type CodeAddr = u32;

/// Member visibility. `protected`/`private` are enforced at runtime against the
/// executing class context; `public` is always accessible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

/// Map bytecode visibility to the value-layer [`Vis`] stored on instances.
fn vis_to_value(v: Visibility) -> Vis {
    match v {
        Visibility::Public => Vis::Public,
        Visibility::Protected => Vis::Protected,
        Visibility::Private => Vis::Private,
    }
}

/// A compile-time constant in a function's constant pool.
#[derive(Clone, PartialEq, Debug)]
pub enum Const {
    Int(i64),
    Float(f64),
    Str(Str),
}

impl Const {
    /// Materialize a runtime [`Value`]. For `Str` this is a cheap refcount bump,
    /// so loading a string constant in a loop does not re-allocate.
    pub fn to_value(&self) -> Value {
        match self {
            Const::Int(i) => Value::Int(*i),
            Const::Float(f) => Value::Float(*f),
            Const::Str(s) => Value::Str(s.clone()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    // --- moves / constants ---
    LoadConst { dst: Reg, k: ConstIdx },
    LoadNull { dst: Reg },
    LoadBool { dst: Reg, val: bool },
    Move { dst: Reg, src: Reg },

    // --- arithmetic (dst = a OP b) ---
    Add { dst: Reg, a: Reg, b: Reg },
    Sub { dst: Reg, a: Reg, b: Reg },
    Mul { dst: Reg, a: Reg, b: Reg },
    Div { dst: Reg, a: Reg, b: Reg },
    Mod { dst: Reg, a: Reg, b: Reg },
    Pow { dst: Reg, a: Reg, b: Reg },
    Neg { dst: Reg, src: Reg },

    // --- strings ---
    /// `dst = (string) a . (string) b`
    Concat { dst: Reg, a: Reg, b: Reg },

    // --- arrays ---
    /// `dst = []` (a fresh empty array).
    NewArray { dst: Reg },
    /// `dst = base[key]` (null if absent; a 1-byte substring for string bases).
    ArrayGet { dst: Reg, base: Reg, key: Reg },
    /// `arr[key] = value`, mutating the array in register `arr` in place (COW).
    /// Auto-vivifies a fresh array when `arr` holds null.
    ArraySet { arr: Reg, key: Reg, value: Reg },
    /// `arr[] = value` (append under the next integer key).
    ArrayPush { arr: Reg, value: Reg },
    /// `foreach` step: if `cursor >= len(arr)` jump to `target`; otherwise load
    /// the entry at position `cursor` into `key_dst`/`val_dst` and advance
    /// `cursor`.
    ForeachNext { arr: Reg, cursor: Reg, key_dst: Reg, val_dst: Reg, target: CodeAddr },

    // --- comparison (dst = bool) ---
    CmpEq { dst: Reg, a: Reg, b: Reg },
    CmpNe { dst: Reg, a: Reg, b: Reg },
    CmpIdentical { dst: Reg, a: Reg, b: Reg },
    CmpNotIdentical { dst: Reg, a: Reg, b: Reg },
    CmpLt { dst: Reg, a: Reg, b: Reg },
    CmpLe { dst: Reg, a: Reg, b: Reg },
    CmpGt { dst: Reg, a: Reg, b: Reg },
    CmpGe { dst: Reg, a: Reg, b: Reg },
    Spaceship { dst: Reg, a: Reg, b: Reg },
    Not { dst: Reg, src: Reg },

    // --- control flow ---
    Jmp { target: CodeAddr },
    JmpIfTrue { cond: Reg, target: CodeAddr },
    JmpIfFalse { cond: Reg, target: CodeAddr },

    // --- calls ---
    /// Call `func` with `argc` args staged in `base ..= base+argc-1`; result -> `dst`.
    Call { dst: Reg, func: FuncId, base: Reg, argc: u16 },
    /// Call the builtin with registry id `native` (see `rphp-stdlib`), with the
    /// same `base ..= base+argc-1` argument staging as [`Op::Call`]; result ->
    /// `dst`. The compiler range-checks `argc` against the descriptor's arity, so
    /// the runtime can pass the window through to the handler unchecked.
    CallNative { dst: Reg, native: u32, base: Reg, argc: u16 },
    /// Build a closure value from the enclosing function's `closures[proto]`
    /// template: snapshot the captured registers and bind them to the closure's
    /// compiled function. Result -> `dst`.
    MakeClosure { dst: Reg, proto: u32 },
    /// Call the callable in register `callee` (a closure or callable string) with
    /// `argc` args staged in `base ..= base+argc-1`; result -> `dst`.
    CallDynamic { dst: Reg, callee: Reg, base: Reg, argc: u16 },

    // --- objects ---
    /// `dst = new <class>` — allocate an instance with its declared properties
    /// initialized to their defaults. The constructor (if any) is invoked by a
    /// separate `MethodCall` the compiler emits right after.
    New { dst: Reg, class: ClassId },
    /// `dst = obj->{name}` where `name` is the string constant `consts[name]`
    /// (null if the property is absent, or `obj` is not an object).
    PropGet { dst: Reg, obj: Reg, name: ConstIdx },
    /// `obj->{name} = value` (a no-op if `obj` is not an object). `name` is the
    /// string constant `consts[name]`.
    PropSet { obj: Reg, name: ConstIdx, value: Reg },
    /// Call method `consts[method]` on the object in `obj`, with `argc` args
    /// staged in `base ..= base+argc-1`; the object is bound to the callee's
    /// `$this` (register 0). Virtual dispatch — the method is resolved on the
    /// object's runtime class, walking up the inheritance chain. Result -> `dst`.
    MethodCall { dst: Reg, obj: Reg, method: ConstIdx, base: Reg, argc: u16 },
    /// A scoped call (`self::m()` / `parent::m()` / `Class::m()`): invoke the
    /// statically-resolved `func` non-virtually, binding register `this` as the
    /// callee's `$this`. Args staged in `base ..= base+argc-1`; result -> `dst`.
    StaticCall { dst: Reg, this: Reg, func: FuncId, base: Reg, argc: u16 },
    /// `dst = (obj instanceof <class>)` — true iff `obj` is an object whose class
    /// is `class` or a descendant of it.
    InstanceOf { dst: Reg, obj: Reg, class: ClassId },

    /// Return `src` (or null) to the caller.
    Ret { src: Option<Reg> },

    // --- io ---
    Echo { src: Reg },
}

/// A template for [`Op::MakeClosure`]: the closure's compiled function plus the
/// enclosing-frame registers whose current values are captured (in the order the
/// closure binds them to its [`Function::capture_regs`]).
#[derive(Clone, Debug)]
pub struct ClosureProto {
    pub func: FuncId,
    pub src_regs: Vec<Reg>,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: IdentId,
    /// The function's name as raw bytes. The interner is a compile-time artifact,
    /// so the runtime keeps the bytes here to resolve a callable string
    /// (`'my_func'`) to a [`FuncId`] without it. Empty for the synthetic `{main}`.
    pub name_bytes: Box<[u8]>,
    pub num_params: u16,
    /// Total registers this frame needs (params occupy `0 .. num_params`).
    pub num_regs: u16,
    pub code: Vec<Op>,
    pub consts: Vec<Const>,
    /// For a closure body: the registers that captured variables bind to, in
    /// capture order (the runtime fills them from the closure's environment
    /// before running). Empty for an ordinary function.
    pub capture_regs: Vec<Reg>,
    /// Closure templates referenced by this function's [`Op::MakeClosure`]s.
    pub closures: Vec<ClosureProto>,
    pub span: Span,
}

/// A declared property: its name (without the `$`), default value, and
/// visibility. Only constant defaults are modelled so far, so the default is a
/// ready-made [`Value`] rather than an initializer expression.
#[derive(Clone, Debug)]
pub struct PropDef {
    pub name: Box<[u8]>,
    pub default: Value,
    pub visibility: Visibility,
}

/// A method: its name (for `obj->m()` dispatch), the [`FuncId`] of its compiled
/// body, and its visibility. The body takes `$this` as register 0, so its
/// [`Function`]'s `num_params` is `1 + declared parameters`.
#[derive(Clone, Debug)]
pub struct Method {
    pub name_bytes: Box<[u8]>,
    pub func: FuncId,
    pub visibility: Visibility,
}

/// A compiled class: its (optional) parent, declared properties, and methods.
/// Interfaces, traits, statics, and constants are later refinements.
#[derive(Clone, Debug)]
pub struct Class {
    pub name: IdentId,
    pub name_bytes: Box<[u8]>,
    pub parent: Option<ClassId>,
    pub props: Vec<PropDef>,
    pub methods: Vec<Method>,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub funcs: Vec<Function>,
    /// Declared classes, indexed by [`ClassId`].
    pub classes: Vec<Class>,
    /// The synthetic top-level `{main}` function id.
    pub main: FuncId,
}

impl Module {
    pub fn func(&self, id: FuncId) -> &Function {
        &self.funcs[id as usize]
    }

    pub fn class(&self, id: ClassId) -> &Class {
        &self.classes[id as usize]
    }

    /// Resolve a function name (case-insensitive, as PHP) to its id. Used to turn
    /// a callable string into a callable target at runtime.
    pub fn func_by_name(&self, name: &[u8]) -> Option<FuncId> {
        self.funcs
            .iter()
            .position(|f| f.name_bytes.eq_ignore_ascii_case(name))
            .map(|i| i as FuncId)
    }

    /// Resolve a class name (case-insensitive, as PHP) to its id.
    pub fn class_by_name(&self, name: &[u8]) -> Option<ClassId> {
        self.classes
            .iter()
            .position(|c| c.name_bytes.eq_ignore_ascii_case(name))
            .map(|i| i as ClassId)
    }

    /// Resolve a method by name on `class`, walking up the inheritance chain.
    /// Returns the compiled function, its visibility, and the class in the chain
    /// that *declares* it (the lexical context for visibility checks).
    pub fn resolve_method(&self, class: ClassId, name: &[u8]) -> Option<(FuncId, Visibility, ClassId)> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.class(cid);
            if let Some(m) = c.methods.iter().find(|m| m.name_bytes.eq_ignore_ascii_case(name)) {
                return Some((m.func, m.visibility, cid));
            }
            cur = c.parent;
        }
        None
    }

    /// Resolve a declared property's visibility and declaring class, walking up
    /// the chain. `None` for an undeclared (dynamic) property — those are public.
    pub fn resolve_prop(&self, class: ClassId, name: &[u8]) -> Option<(Visibility, ClassId)> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.class(cid);
            if let Some(p) = c.props.iter().find(|p| p.name.as_ref() == name) {
                return Some((p.visibility, cid));
            }
            cur = c.parent;
        }
        None
    }

    /// The [`ClassId`] that declares the method compiled to `func`, if any.
    pub fn method_owner(&self, func: FuncId) -> Option<ClassId> {
        self.classes
            .iter()
            .position(|c| c.methods.iter().any(|m| m.func == func))
            .map(|i| i as ClassId)
    }

    /// An instance's full property set (name, default, visibility), parent-first
    /// with a subclass redeclaration overriding the inherited default — the
    /// layout a fresh `new <class>` is seeded with.
    pub fn instance_props(&self, class: ClassId) -> Vec<(Box<[u8]>, Value, Vis)> {
        // Collect the chain root-first so children override parents by name.
        let mut chain = Vec::new();
        let mut cur = Some(class);
        while let Some(cid) = cur {
            chain.push(cid);
            cur = self.class(cid).parent;
        }
        let mut out: Vec<(Box<[u8]>, Value, Vis)> = Vec::new();
        for &cid in chain.iter().rev() {
            for p in &self.class(cid).props {
                let vis = vis_to_value(p.visibility);
                if let Some(slot) = out.iter_mut().find(|(n, _, _)| n.as_ref() == p.name.as_ref()) {
                    slot.1 = p.default.clone();
                    slot.2 = vis;
                } else {
                    out.push((p.name.clone(), p.default.clone(), vis));
                }
            }
        }
        out
    }

    /// Whether `class` is `ancestor` or descends from it.
    pub fn is_subclass_or_eq(&self, class: ClassId, ancestor: ClassId) -> bool {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            if cid == ancestor {
                return true;
            }
            cur = self.class(cid).parent;
        }
        false
    }
}
