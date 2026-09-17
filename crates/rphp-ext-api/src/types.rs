//! [`TypeMask`] — PHP's declared-type set as a bit mask, in the engine's
//! `MAY_BE_*` bit layout, with `Display` in PHP's canonical spelling.

use core::fmt;

use crate::bitflags::bitflags;

bitflags! {
    /// The set of builtin types a parameter, return or property declares.
    ///
    /// Bit positions follow Zend's `MAY_BE_<type> = 1 << IS_<type>` layout
    /// (`IS_NULL = 1` … `IS_RESOURCE = 9`, `IS_CALLABLE = 12`, `IS_ITERABLE
    /// = 13`, `IS_VOID = 14`, `IS_STATIC = 15`, `IS_NEVER = 17`) so a mask can
    /// be handed to engine code unchanged; [`TypeMask::MIXED`] is an explicit
    /// bit rather than Zend's "all value bits" alias so it round-trips through
    /// `Display` as `mixed`.
    ///
    /// Class types are *not* in the mask: a class-typed declaration carries
    /// the class name(s) separately (`ParamInfo::class`, `FnSig::ret_class`)
    /// and an otherwise empty mask. [`TypeMask::EMPTY`] therefore means
    /// "no declared builtin type" — untyped when the class slot is `None`.
    pub struct TypeMask: u32 {
        /// `null`.
        const NULL = 1 << 1;
        /// `false`.
        const FALSE = 1 << 2;
        /// `true`.
        const TRUE = 1 << 3;
        /// `int`.
        const INT = 1 << 4;
        /// `float`.
        const FLOAT = 1 << 5;
        /// `string`.
        const STRING = 1 << 6;
        /// `array`.
        const ARRAY = 1 << 7;
        /// `object` (any object; a named class is carried outside the mask).
        const OBJECT = 1 << 8;
        /// A resource (never a declarable type; used by value-typing tables).
        const RESOURCE = 1 << 9;
        /// `callable`.
        const CALLABLE = 1 << 12;
        /// `iterable` (kept as its own bit so it displays as `iterable`
        /// rather than `Traversable|array`).
        const ITERABLE = 1 << 13;
        /// `void` (return types only).
        const VOID = 1 << 14;
        /// `static` (return types only).
        const STATIC = 1 << 15;
        /// `never` (return types only).
        const NEVER = 1 << 17;
        /// `mixed`.
        const MIXED = 1 << 18;
        /// `bool` = `false | true`.
        const BOOL = (1 << 2) | (1 << 3);
        /// Every value type: what `mixed` accepts (Zend's `MAY_BE_ANY`).
        const ANY = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 9);
    }
}

impl TypeMask {
    /// The mask for a builtin type keyword as it appears in a declaration
    /// (case-insensitive: `int`, `?string` is *not* handled here, `bool`,
    /// `false`, `true`, `null`, `mixed`, `void`, `never`, `static`,
    /// `callable`, `iterable`, `array`, `object`, `float`, `string`). Returns
    /// `None` for anything else — i.e. a class name, `self` or `parent`.
    pub fn from_keyword(name: &str) -> Option<TypeMask> {
        let lower = name.to_ascii_lowercase();
        Some(match lower.as_str() {
            "null" => TypeMask::NULL,
            "false" => TypeMask::FALSE,
            "true" => TypeMask::TRUE,
            "bool" => TypeMask::BOOL,
            "int" => TypeMask::INT,
            "float" => TypeMask::FLOAT,
            "string" => TypeMask::STRING,
            "array" => TypeMask::ARRAY,
            "object" => TypeMask::OBJECT,
            "resource" => TypeMask::RESOURCE,
            "callable" => TypeMask::CALLABLE,
            "iterable" => TypeMask::ITERABLE,
            "void" => TypeMask::VOID,
            "static" => TypeMask::STATIC,
            "never" => TypeMask::NEVER,
            "mixed" => TypeMask::MIXED,
            _ => return None,
        })
    }

    /// Whether `null` is accepted (`?T`, `T|null`, `mixed`, or untyped).
    pub const fn allows_null(self) -> bool {
        self.contains(TypeMask::NULL) || self.contains(TypeMask::MIXED) || self.is_empty()
    }

    /// The type without its `null` bit.
    pub const fn without_null(self) -> TypeMask {
        self.difference(TypeMask::NULL)
    }

    /// Whether this is a union of more than one non-null member (so `null`
    /// spells as `|null` rather than `?`). Class names count as members.
    pub fn is_union(self, class: Option<&str>) -> bool {
        let mut n = class.map_or(0, |c| c.split('|').count());
        n += self.without_null().parts().count();
        n > 1
    }

    /// The builtin members in PHP's canonical display order (the order
    /// `zend_type_to_string` uses): `static`, `callable`, `object`,
    /// `iterable`/`array`, `string`, `int`, `float`, `bool`/`false`/`true`,
    /// `void`, `never`, `resource`. `null` is not included.
    pub fn parts(self) -> impl Iterator<Item = &'static str> {
        let mut out: [Option<&'static str>; 11] = [None; 11];
        let mut i = 0;
        let mut push = |s: &'static str| {
            out[i] = Some(s);
            i += 1;
        };
        if self.contains(TypeMask::MIXED) {
            push("mixed");
        } else {
            if self.contains(TypeMask::STATIC) {
                push("static");
            }
            if self.contains(TypeMask::CALLABLE) {
                push("callable");
            }
            if self.contains(TypeMask::OBJECT) {
                push("object");
            }
            if self.contains(TypeMask::ITERABLE) {
                push("iterable");
            }
            if self.contains(TypeMask::ARRAY) {
                push("array");
            }
            if self.contains(TypeMask::STRING) {
                push("string");
            }
            if self.contains(TypeMask::INT) {
                push("int");
            }
            if self.contains(TypeMask::FLOAT) {
                push("float");
            }
            if self.contains(TypeMask::BOOL) {
                push("bool");
            } else if self.contains(TypeMask::FALSE) {
                push("false");
            } else if self.contains(TypeMask::TRUE) {
                push("true");
            }
            if self.contains(TypeMask::VOID) {
                push("void");
            }
            if self.contains(TypeMask::NEVER) {
                push("never");
            }
            if self.contains(TypeMask::RESOURCE) {
                push("resource");
            }
        }
        out.into_iter().flatten()
    }

    /// PHP's canonical spelling of a declaration made of this mask plus an
    /// optional class part (`Traversable`, or `A|B` for a union of classes,
    /// exactly as `ParamInfo::class` stores it). Class names come first, then
    /// the builtin members in [`TypeMask::parts`] order; `null` becomes a `?`
    /// prefix for a single member and a trailing `|null` for a union.
    /// `mixed` absorbs everything. An empty mask with no class is `""`.
    pub fn display_with_class(self, class: Option<&str>) -> String {
        if self.contains(TypeMask::MIXED) {
            return "mixed".to_string();
        }
        let mut members: Vec<&str> = Vec::new();
        if let Some(c) = class {
            members.extend(c.split('|'));
        }
        for part in self.parts() {
            members.push(part);
        }
        if self.contains(TypeMask::NULL) {
            match members.len() {
                0 => return "null".to_string(),
                1 if !members[0].contains('&') => return format!("?{}", members[0]),
                _ => members.push("null"),
            }
        }
        members.join("|")
    }
}

impl fmt::Display for TypeMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_with_class(None))
    }
}

#[cfg(test)]
mod tests {
    use super::TypeMask as T;

    #[test]
    fn display_single_and_nullable() {
        assert_eq!(T::INT.to_string(), "int");
        assert_eq!(T::INT.union(T::NULL).to_string(), "?int");
        assert_eq!(T::STRING.union(T::NULL).to_string(), "?string");
        assert_eq!(T::NULL.to_string(), "null");
        assert_eq!(T::EMPTY.to_string(), "");
    }

    #[test]
    fn display_unions_in_zend_order() {
        let m = T::STRING.union(T::ARRAY).union(T::NULL);
        assert_eq!(m.to_string(), "array|string|null");
        assert_eq!(T::INT.union(T::FALSE).to_string(), "int|false");
        assert_eq!(T::STRING.union(T::TRUE).to_string(), "string|true");
        assert_eq!(
            T::FLOAT.union(T::INT).union(T::STRING).to_string(),
            "string|int|float"
        );
        assert_eq!(T::BOOL.union(T::INT).to_string(), "int|bool");
        assert_eq!(T::CALLABLE.union(T::STATIC).to_string(), "static|callable");
        assert_eq!(T::VOID.to_string(), "void");
        assert_eq!(T::NEVER.to_string(), "never");
        assert_eq!(T::ITERABLE.union(T::NULL).to_string(), "?iterable");
        assert_eq!(T::OBJECT.union(T::ARRAY).to_string(), "object|array");
    }

    #[test]
    fn display_mixed_absorbs() {
        assert_eq!(T::MIXED.to_string(), "mixed");
        assert_eq!(T::MIXED.union(T::NULL).union(T::INT).to_string(), "mixed");
        assert!(T::MIXED.allows_null());
    }

    #[test]
    fn display_with_class_part() {
        assert_eq!(
            T::EMPTY.display_with_class(Some("Traversable")),
            "Traversable"
        );
        assert_eq!(T::NULL.display_with_class(Some("Throwable")), "?Throwable");
        assert_eq!(
            T::ARRAY.display_with_class(Some("Traversable")),
            "Traversable|array"
        );
        assert_eq!(
            T::INT.display_with_class(Some("RoundingMode")),
            "RoundingMode|int"
        );
        assert_eq!(
            T::NULL.display_with_class(Some("Odbc\\Connection|Odbc\\Result")),
            "Odbc\\Connection|Odbc\\Result|null"
        );
        assert_eq!(T::NULL.display_with_class(Some("(A&B)")), "(A&B)|null");
    }

    #[test]
    fn bit_helpers() {
        assert_eq!(T::BOOL, T::FALSE.union(T::TRUE));
        assert!(T::BOOL.contains(T::FALSE));
        assert!(!T::FALSE.contains(T::BOOL));
        assert!(T::ANY.contains(T::BOOL.union(T::ARRAY)));
        assert!(!T::ANY.contains(T::MIXED));
        assert!(T::INT.union(T::NULL).allows_null());
        assert!(!T::INT.allows_null());
        assert!(T::EMPTY.allows_null());
        assert_eq!(T::INT.union(T::NULL).without_null(), T::INT);
        assert!(T::INT.union(T::STRING).intersects(T::STRING));
        assert!(!T::INT.intersects(T::STRING));
        assert_eq!((T::INT | T::FLOAT) - T::FLOAT, T::INT);
        assert_eq!((T::INT | T::FLOAT) & T::FLOAT, T::FLOAT);
        assert!(T::INT.union(T::STRING).is_union(None));
        assert!(!T::INT.union(T::NULL).is_union(None));
        assert!(T::INT.is_union(Some("Foo")));
        assert_eq!(T::from_keyword("Int"), Some(T::INT));
        assert_eq!(T::from_keyword("bool"), Some(T::BOOL));
        assert_eq!(T::from_keyword("Traversable"), None);
        assert_eq!(T::from_keyword("self"), None);
        assert_eq!(T::INT.bits(), 1 << 4);
        assert_eq!(T::from_bits_retain(1 << 4), T::INT);
    }

    #[test]
    fn debug_lists_flags() {
        assert_eq!(format!("{:?}", T::INT | T::NULL), "TypeMask(NULL | INT)");
        assert_eq!(format!("{:?}", T::BOOL), "TypeMask(FALSE | TRUE)");
        assert_eq!(format!("{:?}", T::EMPTY), "TypeMask(EMPTY)");
    }
}
