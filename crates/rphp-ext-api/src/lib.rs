//! Engine-independent descriptor types shared by every extension crate and by
//! the `xtask gen` code generator: function signatures (arginfo), constants,
//! ini directives, extension metadata. No engine dependency — the handler
//! types (`Ctx`, `NativeFn`, `Registry`, `Extension`) live in `rphp-runtime`.
#![forbid(unsafe_code)]
