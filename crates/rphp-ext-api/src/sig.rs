//! Function/method signatures: [`FnSig`], its [`ParamInfo`] rows, the
//! [`DefaultVal`] a parameter defaults to, and the optimizer's [`FnFlags`].

use crate::bitflags::bitflags;
use crate::TypeMask;

/// A parameter's default value, as declared in the stub. Constant expressions
/// that are not plain literals are kept **unevaluated** as their source text
/// (`PHP_INT_MAX`, `ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401`,
/// `RoundingMode::HalfAwayFromZero`) so reflection prints them verbatim and
/// the engine evaluates them against its own constant table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DefaultVal {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An integer literal.
    Int(i64),
    /// A float literal.
    Float(f64),
    /// A string literal (internal defaults are always valid UTF-8).
    Str(&'static str),
    /// `[]`.
    EmptyArray,
    /// An unevaluated constant expression, e.g. `"PHP_INT_MAX"` or
    /// `"ENT_QUOTES | ENT_HTML401"`.
    Const(&'static str),
}

/// One parameter of a native function or method (Zend's `arginfo` row plus
/// what named arguments and reflection need).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamInfo {
    /// The parameter name without `$` (named arguments, `TypeError` text).
    pub name: &'static str,
    /// The declared builtin types; [`TypeMask::EMPTY`] with `class == None`
    /// means untyped.
    pub ty: TypeMask,
    /// The class part of the declared type, exactly as spelled
    /// (`"Traversable"`, `"Odbc\\Connection|Odbc\\Result"`, `"self"`).
    pub class: Option<&'static str>,
    /// `&$param` — the handler may write back through the argument slot.
    pub by_ref: bool,
    /// `ZEND_ARG_SEND_PREFER_REF`: passed by reference when the argument is
    /// an lvalue, by value otherwise (`array_multisort`).
    pub prefer_ref: bool,
    /// `...$rest`.
    pub variadic: bool,
    /// Whether `null` is accepted (`?T`, `T|null`, `mixed`, or untyped).
    pub nullable: bool,
    /// The default when the argument is omitted; `None` for required and for
    /// optional parameters whose stub declares no default (variadics, some
    /// by-ref outputs).
    pub default: Option<DefaultVal>,
}

impl ParamInfo {
    /// Whether the argument may be omitted.
    pub const fn is_optional(&self) -> bool {
        self.default.is_some() || self.variadic
    }
}

/// The static signature of a native function or method — the single source
/// for arity checks, named-argument binding, defaults, `TypeError` messages
/// and reflection. Methods are keyed `"class::method"` (both lowercase).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FnSig {
    /// Lowercase function name, or `"class::method"` (lowercase) for methods.
    pub name: &'static str,
    /// The declared parameters in order.
    pub params: &'static [ParamInfo],
    /// How many leading parameters are required.
    pub required: u8,
    /// The declared builtin return types ([`TypeMask::EMPTY`] with
    /// `ret_class == None` means no declared return type).
    pub ret: TypeMask,
    /// The class part of the return type, if any.
    pub ret_class: Option<&'static str>,
    /// `#[\Deprecated]` / `ZEND_ACC_DEPRECATED`.
    pub deprecated: bool,
    /// `function &name()` — returns by reference.
    pub returns_ref: bool,
}

impl FnSig {
    /// Whether the last parameter is variadic (no upper argument bound).
    pub const fn is_variadic(&self) -> bool {
        match self.params.last() {
            Some(p) => p.variadic,
            None => false,
        }
    }

    /// The maximum number of arguments accepted, `None` if variadic.
    pub const fn max_args(&self) -> Option<usize> {
        if self.is_variadic() {
            None
        } else {
            Some(self.params.len())
        }
    }

    /// The minimum number of arguments accepted.
    pub const fn min_args(&self) -> usize {
        self.required as usize
    }

    /// The parameter binding argument position `i` (the variadic parameter
    /// absorbs every trailing position).
    pub fn param(&self, i: usize) -> Option<&'static ParamInfo> {
        match self.params.get(i) {
            Some(p) => Some(p),
            None => self.params.last().filter(|p| p.variadic),
        }
    }

    /// Whether argument position `i` is passed by reference.
    pub fn is_by_ref(&self, i: usize) -> bool {
        self.param(i).is_some_and(|p| p.by_ref)
    }

    /// For a method signature, the `(class, method)` halves of the name.
    pub fn split_method(&self) -> Option<(&'static str, &'static str)> {
        self.name.split_once("::")
    }

    /// The function's own name (the method half for methods).
    pub fn short_name(&self) -> &'static str {
        self.split_method().map_or(self.name, |(_, m)| m)
    }

    /// PHP's spelling of the return type (`""` when undeclared).
    pub fn ret_display(&self) -> String {
        self.ret.display_with_class(self.ret_class)
    }
}

bitflags! {
    /// Effect/purity flags an extension attaches to a native function for
    /// the optimizer and the diagnostic pass (spec 08 §15.1). Conservative by
    /// default: an un-annotated function is impure and effectful.
    pub struct FnFlags: u16 {
        /// Same arguments ⇒ same result; reads no observable world
        /// (const-foldable at compile time).
        const DETERMINISTIC = 1 << 0;
        /// Writes no observable state (I/O, globals, properties, statics):
        /// hoistable / CSE-able.
        const NO_SIDE_EFFECT = 1 << 1;
        /// Cannot raise an exception or `Error`.
        const NO_THROW = 1 << 2;
        /// Depends on locale, timezone, ini or other ambient state — never
        /// const-folded.
        const READS_ENV = 1 << 3;
        /// `exit`/`die`-shaped: terminates the frame.
        const NORETURN = 1 << 4;
        /// `#[\NoDiscard]`: warn when the result is unused.
        const NODISCARD = 1 << 5;
        /// `#[\Deprecated]`: emit `E_DEPRECATED` on call.
        const DEPRECATED = 1 << 6;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: ParamInfo = ParamInfo {
        name: "s",
        ty: TypeMask::STRING,
        class: None,
        by_ref: false,
        prefer_ref: false,
        variadic: false,
        nullable: false,
        default: None,
    };

    static F: FnSig = FnSig {
        name: "arrayobject::__construct",
        params: &[
            P,
            ParamInfo {
                name: "flags",
                default: Some(DefaultVal::Int(0)),
                ..P
            },
            ParamInfo {
                name: "rest",
                variadic: true,
                by_ref: true,
                ..P
            },
        ],
        required: 1,
        ret: TypeMask::INT.union(TypeMask::FALSE),
        ret_class: None,
        deprecated: false,
        returns_ref: false,
    };

    #[test]
    fn arity_and_lookup() {
        assert!(F.is_variadic());
        assert_eq!(F.max_args(), None);
        assert_eq!(F.min_args(), 1);
        assert_eq!(F.param(7).map(|p| p.name), Some("rest"));
        assert!(F.is_by_ref(9));
        assert!(!F.is_by_ref(0));
        assert!(F.params[1].is_optional());
        assert!(!F.params[0].is_optional());
        assert_eq!(F.split_method(), Some(("arrayobject", "__construct")));
        assert_eq!(F.short_name(), "__construct");
        assert_eq!(F.ret_display(), "int|false");
    }

    #[test]
    fn flags() {
        let f = FnFlags::DETERMINISTIC | FnFlags::NO_THROW;
        assert!(f.contains(FnFlags::NO_THROW));
        assert!(!f.contains(FnFlags::READS_ENV));
        assert_eq!(format!("{f:?}"), "FnFlags(DETERMINISTIC | NO_THROW)");
    }
}
