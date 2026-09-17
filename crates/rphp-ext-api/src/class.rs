//! Class skeleton descriptors: [`ClassSig`] with its constants, properties,
//! methods and enum cases. The engine builds the runtime class entry (vtable,
//! slot layout, object handlers) from a skeleton; the skeleton only carries
//! what the PHP oracle reports through reflection.

use crate::bitflags::bitflags;
use crate::{ConstValue, DefaultVal, FnSig, TypeMask};

/// What kind of class-like entity a [`ClassSig`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClassKind {
    /// `class`.
    Class,
    /// `interface`.
    Interface,
    /// `trait`.
    Trait,
    /// `enum` (backed when [`ClassSig::backing`] is non-empty).
    Enum,
}

/// Member visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Vis {
    /// `public`.
    Public,
    /// `protected`.
    Protected,
    /// `private`.
    Private,
}

impl Vis {
    /// The keyword as PHP spells it.
    pub const fn keyword(self) -> &'static str {
        match self {
            Vis::Public => "public",
            Vis::Protected => "protected",
            Vis::Private => "private",
        }
    }
}

bitflags! {
    /// Class-level modifiers.
    pub struct ClassSigFlags: u8 {
        /// `abstract class`.
        const ABSTRACT = 1;
        /// `final class`.
        const FINAL = 2;
        /// `readonly class`.
        const READONLY = 4;
    }
}

/// A class constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassConstSig {
    /// The constant name (case-sensitive).
    pub name: &'static str,
    /// Its value ([`ConstValue::Runtime`] when object-valued).
    pub value: ConstValue,
    /// Visibility.
    pub vis: Vis,
    /// `final const`.
    pub is_final: bool,
}

/// A declared property.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropSig {
    /// The property name without `$`.
    pub name: &'static str,
    /// Declared builtin types ([`TypeMask::EMPTY`] with `class == None` for
    /// an untyped property).
    pub ty: TypeMask,
    /// The class part of the declared type, as spelled.
    pub class: Option<&'static str>,
    /// Visibility.
    pub vis: Vis,
    /// `static`.
    pub is_static: bool,
    /// `readonly`.
    pub readonly: bool,
    /// The default value; `None` for a typed property without one
    /// (uninitialized) — an untyped property without a default is `null`.
    pub default: Option<DefaultVal>,
}

/// A method: its [`FnSig`] (named `"class::method"`) plus modifiers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MethodSig {
    /// The signature.
    pub sig: &'static FnSig,
    /// Visibility.
    pub vis: Vis,
    /// `static`.
    pub is_static: bool,
    /// `abstract` (always true for interface methods).
    pub is_abstract: bool,
    /// `final`.
    pub is_final: bool,
}

/// An enum case.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnumCaseSig {
    /// The case name.
    pub name: &'static str,
    /// The backing value; [`ConstValue::Null`] for a pure (unit) enum.
    pub value: ConstValue,
}

/// The skeleton of an internal class, interface, trait or enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassSig {
    /// The declared name with namespace, case preserved
    /// (`ArrayIterator`, `Random\Randomizer`).
    pub name: &'static str,
    /// Class, interface, trait or enum.
    pub kind: ClassKind,
    /// `abstract` / `final` / `readonly`.
    pub flags: ClassSigFlags,
    /// The parent class, if any.
    pub parent: Option<&'static str>,
    /// Directly implemented interfaces (for an interface: the interfaces it
    /// extends), in declaration order.
    pub interfaces: &'static [&'static str],
    /// Constants declared by this class (not inherited), in declaration order.
    pub consts: &'static [ClassConstSig],
    /// Properties declared by this class, in declaration order.
    pub props: &'static [PropSig],
    /// Methods declared by this class, in declaration order.
    pub methods: &'static [MethodSig],
    /// For a backed enum, `int` or `string`; otherwise [`TypeMask::EMPTY`].
    pub backing: TypeMask,
    /// Enum cases in declaration order (empty unless `kind == Enum`).
    pub cases: &'static [EnumCaseSig],
}

impl ClassSig {
    /// Whether the skeleton describes an abstract class.
    pub const fn is_abstract(&self) -> bool {
        self.flags.contains(ClassSigFlags::ABSTRACT)
    }

    /// Whether the skeleton describes a final class.
    pub const fn is_final(&self) -> bool {
        self.flags.contains(ClassSigFlags::FINAL)
    }

    /// Whether the skeleton describes a readonly class.
    pub const fn is_readonly(&self) -> bool {
        self.flags.contains(ClassSigFlags::READONLY)
    }

    /// Whether this is a backed enum.
    pub const fn is_backed_enum(&self) -> bool {
        matches!(self.kind, ClassKind::Enum) && !self.backing.is_empty()
    }

    /// The method declared here with this (case-insensitive) name.
    pub fn method(&self, name: &str) -> Option<&'static MethodSig> {
        self.methods
            .iter()
            .find(|m| m.sig.short_name().eq_ignore_ascii_case(name))
    }

    /// The constant declared here with this (case-sensitive) name.
    pub fn constant(&self, name: &str) -> Option<&'static ClassConstSig> {
        self.consts.iter().find(|c| c.name == name)
    }

    /// The property declared here with this (case-sensitive) name.
    pub fn prop(&self, name: &str) -> Option<&'static PropSig> {
        self.props.iter().find(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static COUNT: FnSig = FnSig {
        name: "countable::count",
        params: &[],
        required: 0,
        ret: TypeMask::INT,
        ret_class: None,
        deprecated: false,
        returns_ref: false,
    };

    static COUNTABLE: ClassSig = ClassSig {
        name: "Countable",
        kind: ClassKind::Interface,
        flags: ClassSigFlags::EMPTY,
        parent: None,
        interfaces: &[],
        consts: &[ClassConstSig {
            name: "X",
            value: ConstValue::Int(1),
            vis: Vis::Public,
            is_final: false,
        }],
        props: &[],
        methods: &[MethodSig {
            sig: &COUNT,
            vis: Vis::Public,
            is_static: false,
            is_abstract: true,
            is_final: false,
        }],
        backing: TypeMask::EMPTY,
        cases: &[],
    };

    #[test]
    fn lookups() {
        assert!(!COUNTABLE.is_abstract());
        assert!(!COUNTABLE.is_backed_enum());
        assert_eq!(
            COUNTABLE.method("COUNT").map(|m| m.sig.name),
            Some("countable::count")
        );
        assert!(COUNTABLE.method("size").is_none());
        assert_eq!(
            COUNTABLE.constant("X").map(|c| c.value),
            Some(ConstValue::Int(1))
        );
        assert!(COUNTABLE.prop("x").is_none());
        assert_eq!(Vis::Protected.keyword(), "protected");
        assert_eq!(
            format!("{:?}", ClassSigFlags::FINAL | ClassSigFlags::READONLY),
            "ClassSigFlags(FINAL | READONLY)"
        );
    }
}
