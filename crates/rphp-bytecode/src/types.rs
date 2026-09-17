//! Owned, engine-independent type declarations ([`TypeDecl`]) as they appear
//! on parameters, return types, properties and class constants.
//!
//! These are metadata: the runtime coerces/checks against them (plan E6
//! `coerce(value, ty, strict)`) and Reflection renders them. The [`Display`]
//! impl produces PHP's canonical spelling — the string `ReflectionType::__toString`
//! and `TypeError` messages use — which reorders union members the way
//! `zend_type_to_string` does rather than echoing the source order.

use std::fmt::{self, Display};

/// A built-in (non-class) type keyword.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BuiltinType {
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
    /// `mixed` (stands alone; never combined)
    Mixed,
    /// `void` (return types only)
    Void,
    /// `never` (return types only)
    Never,
    /// `null` (standalone since 8.2, or as a union member)
    Null,
    /// `true` (standalone since 8.2)
    True,
    /// `false`
    False,
    /// `callable` (parameters/returns only)
    Callable,
    /// `iterable` — kept as its own keyword so it renders as `iterable`; the
    /// runtime treats it as `Traversable|array`.
    Iterable,
    /// `self` — resolved against the lexical class at check time.
    SelfTy,
    /// `static` — return types only; resolved against the late-static-bound
    /// class.
    StaticTy,
    /// `parent`.
    ParentTy,
}

impl BuiltinType {
    /// The keyword as PHP spells it.
    pub const fn name(self) -> &'static str {
        match self {
            BuiltinType::Int => "int",
            BuiltinType::Float => "float",
            BuiltinType::String => "string",
            BuiltinType::Bool => "bool",
            BuiltinType::Array => "array",
            BuiltinType::Object => "object",
            BuiltinType::Mixed => "mixed",
            BuiltinType::Void => "void",
            BuiltinType::Never => "never",
            BuiltinType::Null => "null",
            BuiltinType::True => "true",
            BuiltinType::False => "false",
            BuiltinType::Callable => "callable",
            BuiltinType::Iterable => "iterable",
            BuiltinType::SelfTy => "self",
            BuiltinType::StaticTy => "static",
            BuiltinType::ParentTy => "parent",
        }
    }

    /// Whether this keyword is a class-like name (`self`/`parent`), which PHP
    /// renders in the "class names" group of a union in declaration order.
    const fn is_class_like(self) -> bool {
        matches!(self, BuiltinType::SelfTy | BuiltinType::ParentTy)
    }

    /// PHP's rendering order for the non-class members of a union
    /// (`zend_type_to_string`): `static`, `callable`, `iterable`, `object`,
    /// `array`, `string`, `int`, `float`, `bool`, `false`, `true`, `void`,
    /// `never`. `null` is handled separately (last, or as a `?` prefix);
    /// `mixed`, `self` and `parent` never appear in this group.
    pub const CANONICAL_ORDER: [BuiltinType; 13] = [
        BuiltinType::StaticTy,
        BuiltinType::Callable,
        BuiltinType::Iterable,
        BuiltinType::Object,
        BuiltinType::Array,
        BuiltinType::String,
        BuiltinType::Int,
        BuiltinType::Float,
        BuiltinType::Bool,
        BuiltinType::False,
        BuiltinType::True,
        BuiltinType::Void,
        BuiltinType::Never,
    ];
}

impl Display for BuiltinType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A declared type. Class names are fully qualified without a leading
/// backslash and in their declared case (PHP preserves it for display; lookup
/// is case-insensitive).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TypeDecl {
    /// A class/interface/enum name (FQN, no leading `\`).
    Named(Box<[u8]>),
    /// A built-in keyword.
    Builtin(BuiltinType),
    /// `?T` — sugar for `T|null`.
    Nullable(Box<TypeDecl>),
    /// `A|B|…` — members are `Named`, `Builtin` or `Intersection` (DNF).
    Union(Vec<TypeDecl>),
    /// `A&B&…` — members are `Named` only.
    Intersection(Vec<TypeDecl>),
}

impl TypeDecl {
    /// A class type by name.
    pub fn named(name: &[u8]) -> TypeDecl {
        TypeDecl::Named(Box::from(name))
    }

    /// Wrap in `?T` (no-op if the type already admits null).
    pub fn nullable(self) -> TypeDecl {
        if self.allows_null() {
            self
        } else {
            TypeDecl::Nullable(Box::new(self))
        }
    }

    /// Whether `null` satisfies this type (`?T`, `…|null`, `null`, `mixed`).
    pub fn allows_null(&self) -> bool {
        match self {
            TypeDecl::Named(_) | TypeDecl::Intersection(_) => false,
            TypeDecl::Builtin(b) => matches!(b, BuiltinType::Null | BuiltinType::Mixed),
            TypeDecl::Nullable(_) => true,
            TypeDecl::Union(parts) => parts.iter().any(TypeDecl::allows_null),
        }
    }

    /// Flatten into PHP's rendering groups: class-like names (in declaration
    /// order, DNF intersections parenthesized), the set of builtin keywords, and
    /// whether null is admitted.
    fn collect(
        &self,
        names: &mut Vec<String>,
        builtins: &mut Vec<BuiltinType>,
        nullable: &mut bool,
    ) {
        match self {
            TypeDecl::Named(n) => names.push(String::from_utf8_lossy(n).into_owned()),
            TypeDecl::Builtin(BuiltinType::Null) => *nullable = true,
            TypeDecl::Builtin(b) if b.is_class_like() => names.push(b.name().to_owned()),
            TypeDecl::Builtin(b) => {
                if !builtins.contains(b) {
                    builtins.push(*b);
                }
            }
            TypeDecl::Nullable(inner) => {
                *nullable = true;
                inner.collect(names, builtins, nullable);
            }
            TypeDecl::Union(parts) => {
                for p in parts {
                    p.collect(names, builtins, nullable);
                }
            }
            TypeDecl::Intersection(parts) => {
                names.push(format!("({})", Self::join_intersection(parts)))
            }
        }
    }

    fn join_intersection(parts: &[TypeDecl]) -> String {
        parts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("&")
    }
}

impl Display for TypeDecl {
    /// PHP's canonical spelling: class names first in declaration order, then
    /// builtins in [`BuiltinType::CANONICAL_ORDER`], then `null` — rendered as a
    /// `?` prefix when the rest is a single non-intersection type, otherwise as a
    /// trailing `|null`. A pure intersection renders as `A&B`; inside a union it
    /// is parenthesized (`(A&B)|null`). `mixed` always renders alone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let TypeDecl::Intersection(parts) = self {
            return f.write_str(&Self::join_intersection(parts));
        }
        let mut names = Vec::new();
        let mut builtins = Vec::new();
        let mut nullable = false;
        self.collect(&mut names, &mut builtins, &mut nullable);
        if builtins.contains(&BuiltinType::Mixed) {
            return f.write_str("mixed");
        }
        let mut parts = names;
        for b in BuiltinType::CANONICAL_ORDER {
            if builtins.contains(&b) {
                parts.push(b.name().to_owned());
            }
        }
        if !nullable {
            return f.write_str(&parts.join("|"));
        }
        match parts.as_slice() {
            [] => f.write_str("null"),
            [single] if !single.starts_with('(') => write!(f, "?{single}"),
            _ => {
                parts.push("null".to_owned());
                f.write_str(&parts.join("|"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BuiltinType as B;

    fn b(t: B) -> TypeDecl {
        TypeDecl::Builtin(t)
    }
    fn n(s: &str) -> TypeDecl {
        TypeDecl::named(s.as_bytes())
    }

    #[test]
    fn simple_types() {
        assert_eq!(b(B::Int).to_string(), "int");
        assert_eq!(n("Foo\\Bar").to_string(), "Foo\\Bar");
        assert_eq!(b(B::Mixed).to_string(), "mixed");
        assert_eq!(b(B::Null).to_string(), "null");
        assert_eq!(b(B::SelfTy).to_string(), "self");
        assert_eq!(b(B::StaticTy).to_string(), "static");
    }

    #[test]
    fn nullable_prefix_form() {
        assert_eq!(b(B::Int).nullable().to_string(), "?int");
        assert_eq!(n("A").nullable().to_string(), "?A");
        assert_eq!(
            TypeDecl::Union(vec![b(B::Null), b(B::Int)]).to_string(),
            "?int"
        );
        assert_eq!(
            TypeDecl::Union(vec![b(B::Int), b(B::Null)]).to_string(),
            "?int"
        );
        // Already nullable: wrapping again is a no-op.
        assert_eq!(b(B::Int).nullable().nullable().to_string(), "?int");
        assert_eq!(b(B::Mixed).nullable().to_string(), "mixed");
    }

    #[test]
    fn unions_use_php_canonical_order() {
        assert_eq!(
            TypeDecl::Union(vec![b(B::Int), b(B::String)]).to_string(),
            "string|int"
        );
        assert_eq!(
            TypeDecl::Union(vec![b(B::Float), b(B::Int)]).to_string(),
            "int|float"
        );
        assert_eq!(
            TypeDecl::Union(vec![b(B::Int), n("Foo")]).to_string(),
            "Foo|int"
        );
        assert_eq!(
            TypeDecl::Union(vec![b(B::False), n("A")]).to_string(),
            "A|false"
        );
        assert_eq!(
            TypeDecl::Union(vec![b(B::Bool), b(B::Array)]).to_string(),
            "array|bool"
        );
        assert_eq!(TypeDecl::Union(vec![n("B"), n("A")]).to_string(), "B|A");
        assert_eq!(
            TypeDecl::Union(vec![b(B::Int), b(B::StaticTy), b(B::Callable)]).to_string(),
            "static|callable|int"
        );
    }

    #[test]
    fn nullable_unions_use_trailing_null() {
        assert_eq!(
            TypeDecl::Union(vec![n("A"), n("B"), b(B::Null)]).to_string(),
            "A|B|null"
        );
        assert_eq!(
            TypeDecl::Union(vec![n("A"), n("B")]).nullable().to_string(),
            "A|B|null"
        );
        assert_eq!(
            TypeDecl::Union(vec![b(B::Null), b(B::Int), b(B::String)]).to_string(),
            "string|int|null"
        );
    }

    #[test]
    fn intersections_and_dnf() {
        let ab = TypeDecl::Intersection(vec![n("A"), n("B")]);
        assert_eq!(ab.to_string(), "A&B");
        assert_eq!(
            TypeDecl::Union(vec![ab.clone(), b(B::Null)]).to_string(),
            "(A&B)|null"
        );
        assert_eq!(ab.clone().nullable().to_string(), "(A&B)|null");
        assert_eq!(TypeDecl::Union(vec![ab, n("C")]).to_string(), "(A&B)|C");
    }

    #[test]
    fn allows_null() {
        assert!(b(B::Null).allows_null());
        assert!(b(B::Mixed).allows_null());
        assert!(b(B::Int).nullable().allows_null());
        assert!(TypeDecl::Union(vec![n("A"), b(B::Null)]).allows_null());
        assert!(!TypeDecl::Union(vec![n("A"), b(B::Int)]).allows_null());
        assert!(!TypeDecl::Intersection(vec![n("A"), n("B")]).allows_null());
    }
}
