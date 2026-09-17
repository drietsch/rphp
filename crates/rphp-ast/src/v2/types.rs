//! Type declarations: parameter, return, property and constant types.
//!
//! Types are stored as written. Validity rules (no `void` in unions, `never`
//! only as a return type, no nullable `mixed`, DNF shape restrictions, ...)
//! are checked by the front end's validation pass, not here.

use rphp_span::Span;

use super::name::Name;

/// A type as written, with its span.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Type {
    /// The type's shape.
    pub kind: TypeKind,
    /// The span of the whole type expression.
    pub span: Span,
}

/// The shape of a [`Type`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TypeKind {
    /// A class-like name (`Foo`, `\Foo\Bar`, `namespace\Foo`). `self`,
    /// `static` and `parent` are [`Builtin`]s, not names.
    Named(Name),
    /// A reserved type keyword.
    Builtin(Builtin),
    /// `?T`.
    Nullable(Box<Type>),
    /// `A|B|...`. Members may be [`TypeKind::Intersection`] (DNF types).
    Union(Vec<Type>),
    /// `A&B&...`. Members are never unions.
    Intersection(Vec<Type>),
}

/// Reserved type keywords. Spelling in source is case-insensitive.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Builtin {
    /// `int`
    Int,
    /// `float`
    Float,
    /// `string`
    String,
    /// `bool`
    Bool,
    /// `array`
    Array,
    /// `object`
    Object,
    /// `mixed`
    Mixed,
    /// `void`
    Void,
    /// `never`
    Never,
    /// `null`
    Null,
    /// `true`
    True,
    /// `false`
    False,
    /// `callable`
    Callable,
    /// `iterable`
    Iterable,
    /// `self`
    SelfTy,
    /// `static`
    StaticTy,
    /// `parent`
    ParentTy,
}

impl Builtin {
    /// The canonical lowercase spelling, e.g. `"int"`, `"self"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Builtin::Int => "int",
            Builtin::Float => "float",
            Builtin::String => "string",
            Builtin::Bool => "bool",
            Builtin::Array => "array",
            Builtin::Object => "object",
            Builtin::Mixed => "mixed",
            Builtin::Void => "void",
            Builtin::Never => "never",
            Builtin::Null => "null",
            Builtin::True => "true",
            Builtin::False => "false",
            Builtin::Callable => "callable",
            Builtin::Iterable => "iterable",
            Builtin::SelfTy => "self",
            Builtin::StaticTy => "static",
            Builtin::ParentTy => "parent",
        }
    }
}

impl Type {
    /// A builtin type with the given span.
    pub fn builtin(b: Builtin, span: Span) -> Self {
        Self {
            kind: TypeKind::Builtin(b),
            span,
        }
    }

    /// A class-like type from a written name; the span is the name's.
    pub fn named(name: Name) -> Self {
        let span = name.span;
        Self {
            kind: TypeKind::Named(name),
            span,
        }
    }
}
