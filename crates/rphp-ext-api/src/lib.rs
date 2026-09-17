//! Engine-independent descriptor types shared by every extension crate and by
//! the `xtask gen` code generator: function signatures (arginfo), constants,
//! ini directives, class skeletons and extension metadata. No engine
//! dependency — the handler types (`Ctx`, `NativeFn`, `Registry`, `Extension`)
//! live in `rphp-runtime`, which attaches a handler and [`FnFlags`] to a
//! generated [`FnSig`].
//!
//! Everything here is `'static` data: `cargo xtask gen` emits one `pub static`
//! per function/method/class from the PHP oracle manifest
//! (`manifest/php-8.5.0`), so an extension crate never hand-types a signature
//! and reflection, named arguments, defaults and `TypeError` messages all read
//! from the same table.
#![forbid(unsafe_code)]

mod bitflags;
mod class;
mod consts;
mod ext;
mod ini;
mod sig;
mod types;

pub use class::{
    ClassConstSig, ClassKind, ClassSig, ClassSigFlags, EnumCaseSig, MethodSig, PropSig, Vis,
};
pub use consts::{ConstDef, ConstValue};
pub use ext::ExtInfo;
pub use ini::{IniAccess, IniDef};
pub use sig::{DefaultVal, FnFlags, FnSig, ParamInfo};
pub use types::TypeMask;
