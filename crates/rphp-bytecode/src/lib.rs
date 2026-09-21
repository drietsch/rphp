//! Register bytecode: the contract shared by `rphp-compiler` (producer) and
//! `rphp-runtime` (consumer).
//!
//! Three-address, register-based (per `specs/base/05-bytecode-isa.md`). The
//! program is an in-memory `Vec<Op>` rather than the encoded byte format; the
//! variable-length encoding and metadata blocks come later.
//!
//! The crate holds two generations side by side:
//!
//! * the **M0** set the current compiler and runtime implement — the [`Op`]
//!   variants up to [`Op::Echo`], [`Function`]'s leading fields, [`Class`] and
//!   [`Module`] — with the calling convention described below; and
//! * the **v2 contract** (plan Track E; see `CONTRACT.md` in this crate): the
//!   `Init*`/`Send*`/`DoCall` call sequence, references and fetch-for-write,
//!   late-bound names, exception regions, generators, [`Function`]'s metadata
//!   fields, [`ClassDecl`], [`CompiledUnit`] and the table constants
//!   [`Const::Name`], [`Const::JumpTable`], [`Const::ArgNames`],
//!   [`Const::Type`]. Everything v2 is additive today: no producer lowers to
//!   it yet and the tier-0 interpreter rejects the new ops. The M0-only parts
//!   are removed in plan E3/E6 once the v2 paths are live.
//!
//! ## M0 calling convention
//! Registers are local to a frame. A `Call { dst, func, base, argc }` evaluates
//! arguments into the contiguous window `base ..= base+argc-1` of the *caller's*
//! frame, then a fresh callee frame is created whose registers `0 .. argc` are
//! initialized from that window (M0 copies; the v2 zero-copy window is the
//! `Send*`/`DoCall` sequence). The callee returns into the caller's `dst`
//! register via `Ret`.
#![forbid(unsafe_code)]

mod class;
mod consts;
mod func;
mod op;
mod types;
mod unit;

pub use class::{
    Class, ClassConstDef, ClassDecl, ClassFlags, ClassKind, ConstDecl, EnumBackingType,
    EnumCase, EnumCaseDef, Hooks, Method, MethodDecl,
    PropDecl, PropDef, TraitAdaptation, TraitUse,
};
pub use consts::{Const, NameConst};
pub use func::{
    AttrDef, AttrTarget, CaptureDesc, CatchClause, ClosureProto, ExRegion, Finally, FnFlags,
    Function, InitRef, ParamDef, PromotedProp, StaticVar,
};
pub use op::{
    AssignOpKind, CastKind, ClassRef, ClassRefKind, CmpKind, FinallyState, IncludeKind, NameRef,
    NameRefKind, Op,
};
pub use types::{BuiltinType, TypeDecl};
pub use unit::{CompiledUnit, Module};

use rphp_value::Vis;

/// A register index within a frame.
pub type Reg = u16;
/// An operand register at or above this names a **constant** instead:
/// `r - CONST_OPERAND` indexes the function's pool. Only the arithmetic,
/// bitwise, string and comparison ops (and [`Op::JmpUnless`]) take
/// constant operands, so `$i + 1` and `$i < 10` need no `LoadConst`; the
/// compiler falls back to one past the pool's 32768th entry.
pub const CONST_OPERAND: Reg = 0x8000;
/// An index into a function's (or a [`ClassDecl`]'s) constant pool.
pub type ConstIdx = u32;
/// An index into `Module::funcs` / `CompiledUnit::funcs`.
pub type FuncId = u32;
/// An index into `Module::classes` / `CompiledUnit::classes`.
pub type ClassId = u32;
/// An instruction index within `Function::code` (a branch target).
pub type CodeAddr = u32;

/// Member visibility. `protected`/`private` are enforced at runtime against the
/// executing class context; `public` is always accessible.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

/// Map bytecode visibility to the value-layer [`Vis`] stored on instances.
pub(crate) fn vis_to_value(v: Visibility) -> Vis {
    match v {
        Visibility::Public => Vis::Public,
        Visibility::Protected => Vis::Protected,
        Visibility::Private => Vis::Private,
    }
}
