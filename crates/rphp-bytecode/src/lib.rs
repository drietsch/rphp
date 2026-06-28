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
use rphp_value::{Str, Value};

/// A register index within a frame.
pub type Reg = u16;
/// An index into a function's constant pool.
pub type ConstIdx = u32;
/// An index into `Module::funcs`.
pub type FuncId = u32;
/// An instruction index within `Function::code` (a branch target).
pub type CodeAddr = u32;

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
    /// Return `src` (or null) to the caller.
    Ret { src: Option<Reg> },

    // --- io ---
    Echo { src: Reg },
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: IdentId,
    pub num_params: u16,
    /// Total registers this frame needs (params occupy `0 .. num_params`).
    pub num_regs: u16,
    pub code: Vec<Op>,
    pub consts: Vec<Const>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub funcs: Vec<Function>,
    /// The synthetic top-level `{main}` function id.
    pub main: FuncId,
}

impl Module {
    pub fn func(&self, id: FuncId) -> &Function {
        &self.funcs[id as usize]
    }
}
