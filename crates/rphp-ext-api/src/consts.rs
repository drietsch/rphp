//! Global constants an extension registers: [`ConstDef`] / [`ConstValue`].

/// The value of a registered constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstValue {
    /// `null`.
    Null,
    /// A bool.
    Bool(bool),
    /// An int.
    Int(i64),
    /// A float (`NAN`, `INF` and `M_PI` are plain values).
    Float(f64),
    /// A string.
    Str(&'static str),
    /// Computed by the engine at startup because the value depends on the
    /// process or build environment: `PHP_BINARY`, `PHP_OS`,
    /// `DIRECTORY_SEPARATOR`, `STDIN`/`STDOUT`/`STDERR`, the `PHP_*DIR`
    /// install paths, and any resource- or object-valued constant.
    Runtime,
}

/// A global constant registered by an extension.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConstDef {
    /// The constant name as declared (case-sensitive, e.g. `E_ALL`,
    /// `Random\Engine::...` are class constants and live in [`crate::ClassSig`]).
    pub name: &'static str,
    /// Its value, or [`ConstValue::Runtime`].
    pub value: ConstValue,
    /// `#[\Deprecated]` — accessing it emits `E_DEPRECATED`.
    pub deprecated: bool,
}

impl ConstDef {
    /// A non-deprecated value constant.
    pub const fn new(name: &'static str, value: ConstValue) -> Self {
        ConstDef {
            name,
            value,
            deprecated: false,
        }
    }
}
