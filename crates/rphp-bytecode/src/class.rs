//! Class declarations: the M0 [`Class`] (linked at compile time by
//! [`ClassId`]) and the v2 [`ClassDecl`] (compiled, not yet linked; the runtime
//! builds its `ClassDef` from it at declaration time, plan E6).

use rphp_intern::IdentId;
use rphp_span::Span;
use rphp_value::Value;

use crate::func::bitflags_newtype;
use crate::{AttrDef, BuiltinType, ClassId, Const, FuncId, InitRef, TypeDecl, Visibility};

// ---- M0 (old path) --------------------------------------------------------------

/// A declared property: its name (without the `$`), default value, and
/// visibility. Only constant defaults are modelled so far, so the default is a
/// ready-made [`Value`] rather than an initializer expression.
#[derive(Clone, Debug)]
pub struct PropDef {
    pub name: Box<[u8]>,
    pub default: Value,
    pub visibility: Visibility,
    /// E6: `static` — the property lives on the class, not the instance.
    pub is_static: bool,
    /// E6: `readonly`.
    pub readonly: bool,
    /// E6: the `set` visibility of an asymmetric declaration
    /// (`public private(set) int $n`), when it differs from `visibility`.
    pub set_vis: Option<Visibility>,
    /// E6: the declared type, checked on assignment.
    pub ty: Option<TypeDecl>,
    /// E6: property hooks (`get`/`set`), compiled as methods of the class.
    pub hooks: Option<Hooks>,
    /// E6: a non-constant default (`new Foo`, `self::CONST`, …) compiled as a
    /// zero-argument thunk evaluated at `new` in the class's scope. When set,
    /// `default` is ignored.
    pub default_thunk: Option<FuncId>,
}

impl PropDef {
    /// A public instance property with a constant default.
    pub fn new(name: &[u8], default: Value, visibility: Visibility) -> PropDef {
        PropDef {
            name: Box::from(name),
            default,
            visibility,
            is_static: false,
            readonly: false,
            set_vis: None,
            ty: None,
            hooks: None,
            default_thunk: None,
        }
    }
}

/// E6: one class constant of an M0 [`Class`]. The initializer is a ready value
/// when it folds, otherwise a zero-argument thunk evaluated on first use in the
/// declaring class's scope (so `self::` and `static::` resolve).
#[derive(Clone, Debug)]
pub struct ClassConstDef {
    pub name: Box<[u8]>,
    pub visibility: Visibility,
    pub is_final: bool,
    /// The declared type (8.3 typed constants), if any.
    pub ty: Option<TypeDecl>,
    /// The folded value, when the initializer is constant.
    pub value: Option<Value>,
    /// The initializer thunk, when it is not.
    pub thunk: Option<FuncId>,
}

/// E6: one case of an enum declaration.
#[derive(Clone, Debug)]
pub struct EnumCaseDef {
    pub name: Box<[u8]>,
    /// The backing value (`None` for a pure enum).
    pub value: Option<Value>,
}

/// E6: what an enum's cases are backed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnumBackingType {
    /// A pure enum, or a non-enum class.
    #[default]
    None,
    Int,
    String,
}

/// A method: its name (for `obj->m()` dispatch), the [`FuncId`] of its compiled
/// body, and its visibility. The body takes `$this` as register 0, so its
/// [`Function`](crate::Function)'s `num_params` is `1 + declared parameters`.
#[derive(Clone, Debug)]
pub struct Method {
    pub name_bytes: Box<[u8]>,
    pub func: FuncId,
    pub visibility: Visibility,
    /// E6: `static` — no `$this`, dispatched against the called scope.
    pub is_static: bool,
    /// E6: `abstract` — no body; instantiating a class that still has one is
    /// an `Error`.
    pub is_abstract: bool,
    /// E6: `final` — an override is a link-time fatal.
    pub is_final: bool,
}

impl Method {
    /// A public, non-static, concrete method.
    pub fn new(name_bytes: &[u8], func: FuncId, visibility: Visibility) -> Method {
        Method {
            name_bytes: Box::from(name_bytes),
            func,
            visibility,
            is_static: false,
            is_abstract: false,
            is_final: false,
        }
    }
}

/// A compiled class: its (optional) parent, declared properties, and methods.
/// Interfaces, traits, statics, and constants are later refinements.
/// (M0; superseded by [`ClassDecl`].)
#[derive(Clone, Debug)]
pub struct Class {
    pub name: IdentId,
    pub name_bytes: Box<[u8]>,
    pub parent: Option<ClassId>,
    pub props: Vec<PropDef>,
    pub methods: Vec<Method>,
    /// Source line of the declaration (E3 addition, for `Cannot redeclare
    /// class X (previously declared in file:line)`); 0 when unknown.
    pub line: u32,
    /// E4 additions (CONTRACT.md §8.1): the parent's *name* (FQN, no leading
    /// `\`), resolved by the runtime against its process-wide class table at
    /// declaration time — so a class may extend a native class (`Exception`)
    /// or one declared by another unit; `parent` (unit-local) is the
    /// compile-time hint for classes of the same unit.
    pub parent_name: Option<Box<[u8]>>,
    /// `implements` names (FQN), resolved at declaration time.
    pub interfaces: Vec<Box<[u8]>>,
    /// Class/interface/trait/enum.
    pub kind: ClassKind,
    /// Modifiers (`abstract`, `final`, `readonly`, `#[AllowDynamicProperties]`).
    pub flags: ClassFlags,
    /// E6: class constants, in declaration order.
    pub consts: Vec<ClassConstDef>,
    /// E6: `use T1, T2 { ... }` statements, in order. Resolved and copied in
    /// at declaration time.
    pub traits: Vec<TraitUse>,
    /// E6: enum cases, in declaration order (empty for non-enums).
    pub enum_cases: Vec<EnumCaseDef>,
    /// E6: the enum's backing type.
    pub enum_backing: EnumBackingType,
}

impl Class {
    /// A bare `class Name {}` for struct-update construction.
    pub fn new_minimal(name: IdentId, name_bytes: &[u8]) -> Class {
        Class {
            name,
            name_bytes: Box::from(name_bytes),
            parent: None,
            props: Vec::new(),
            methods: Vec::new(),
            line: 0,
            parent_name: None,
            interfaces: Vec::new(),
            kind: ClassKind::Class,
            flags: ClassFlags::NONE,
            consts: Vec::new(),
            traits: Vec::new(),
            enum_cases: Vec::new(),
            enum_backing: EnumBackingType::None,
        }
    }
}

// ---- v2 ------------------------------------------------------------------------

/// What kind of class-like declaration this is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClassKind {
    /// `class`
    Class,
    /// `interface`
    Interface,
    /// `trait`
    Trait,
    /// `enum`, optionally backed (`enum E: int`), in which case every case has
    /// a value. Enums implicitly implement `UnitEnum`/`BackedEnum`.
    Enum {
        /// `Some(Int)` or `Some(String)` for a backed enum.
        backing: Option<BuiltinType>,
    },
}

bitflags_newtype! {
    /// Class modifiers.
    pub struct ClassFlags {
        /// `abstract class` (also set on interfaces and traits, which cannot be
        /// instantiated either).
        const ABSTRACT = 0;
        /// `final class`.
        const FINAL = 1;
        /// `readonly class` (every property is readonly; no dynamic props).
        const READONLY = 2;
        /// `#[AllowDynamicProperties]`: dynamic properties without the 8.2
        /// deprecation. (`stdClass` and its descendants have it implicitly.)
        const ALLOW_DYNAMIC = 3;
        /// `new class { … }`: the name is compiler-generated
        /// (`class@anonymous` + a NUL byte + `file:line$n`).
        const ANONYMOUS = 4;
    }
}

/// One `use A, B { … }` statement in a class body.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TraitUse {
    /// The trait names (FQN, no leading `\`) this statement uses. (The plan's
    /// `name` became `names`: adaptations belong to the `use` statement, which
    /// may name several traits.)
    pub names: Vec<Box<[u8]>>,
    /// The `insteadof` / `as` rules in the statement's block.
    pub adaptations: Vec<TraitAdaptation>,
}

/// A trait conflict-resolution or aliasing rule.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TraitAdaptation {
    /// `T::m insteadof U, V;` — take `m` from `trait_`, excluding it from the
    /// listed traits.
    Precedence {
        /// The winning trait.
        trait_: Box<[u8]>,
        /// The method name.
        method: Box<[u8]>,
        /// The traits whose `method` is excluded.
        insteadof: Vec<Box<[u8]>>,
    },
    /// `[T::]m as [vis] [alias];` — add `alias` (or change visibility of `m`).
    Alias {
        /// The trait, when qualified (`T::m`); `None` for a bare `m`.
        trait_: Option<Box<[u8]>>,
        /// The method name.
        method: Box<[u8]>,
        /// New visibility, if given.
        vis: Option<Visibility>,
        /// The alias name, if given.
        alias: Option<Box<[u8]>>,
    },
}

/// A class constant declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct ConstDecl {
    /// Name (case-sensitive).
    pub name: Box<[u8]>,
    /// Initializer, evaluated lazily on first access (cycle ⇒ `Error`).
    pub init: InitRef,
    /// Visibility.
    pub vis: Visibility,
    /// `final const`.
    pub is_final: bool,
    /// Typed class constant (PHP 8.3).
    pub ty: Option<TypeDecl>,
    /// Attributes.
    pub attrs: Vec<AttrDef>,
    /// Doc comment.
    pub doc: Option<Box<[u8]>>,
}

/// Property hooks (PHP 8.4): each is a compiled method-like function in the
/// unit (`$this` bound; `set` takes the incoming value as its single parameter
/// and `get` returns the value).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hooks {
    /// The `get { … }` hook.
    pub get: Option<FuncId>,
    /// The `set { … }` hook.
    pub set: Option<FuncId>,
}

/// A property declaration (promoted constructor parameters are *also* listed
/// here by the compiler, so the class's slot layout is complete without
/// scanning the constructor).
#[derive(Clone, PartialEq, Debug)]
pub struct PropDecl {
    /// Name without the `$`.
    pub name: Box<[u8]>,
    /// Read visibility.
    pub vis: Visibility,
    /// Asymmetric write visibility (`public private(set)`), if narrower.
    pub set_vis: Option<Visibility>,
    /// `static`.
    pub is_static: bool,
    /// `readonly` (explicit or from a `readonly class`).
    pub readonly: bool,
    /// Declared type. A typed property without a default starts `Uninit`.
    pub ty: Option<TypeDecl>,
    /// Default value; an untyped property without one defaults to null.
    pub default: Option<InitRef>,
    /// Property hooks, if any (virtual when no hook body references the
    /// backing value).
    pub hooks: Option<Hooks>,
    /// Attributes.
    pub attrs: Vec<AttrDef>,
    /// Doc comment.
    pub doc: Option<Box<[u8]>>,
}

/// A method declaration; the body is `funcs[func]` (with `in_class` pointing
/// back here and `FnFlags::STATIC` mirroring `is_static`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MethodDecl {
    /// Name as declared (dispatch is case-insensitive).
    pub name: Box<[u8]>,
    /// The compiled body (an abstract method still has a `Function`, with empty
    /// code, so its signature metadata is available).
    pub func: FuncId,
    /// Visibility.
    pub vis: Visibility,
    /// `static`.
    pub is_static: bool,
    /// `abstract` (or an interface method).
    pub is_abstract: bool,
    /// `final`.
    pub is_final: bool,
}

/// An enum case.
#[derive(Clone, PartialEq, Debug)]
pub struct EnumCase {
    /// Case name.
    pub name: Box<[u8]>,
    /// Backing value (required iff the enum is backed).
    pub value: Option<InitRef>,
    /// Attributes.
    pub attrs: Vec<AttrDef>,
    /// Doc comment.
    pub doc: Option<Box<[u8]>>,
}

/// A compiled, not-yet-linked class-like declaration. Parent, interfaces and
/// traits are names resolved (with autoload) when the class is declared —
/// hoisted at unit load when early-bindable, otherwise by
/// [`Op::DeclareClass`](crate::Op::DeclareClass).
#[derive(Clone, PartialEq, Debug)]
pub struct ClassDecl {
    /// Fully-qualified name, no leading `\`, declared case.
    pub name: Box<[u8]>,
    /// Class/interface/trait/enum.
    pub kind: ClassKind,
    /// Modifiers.
    pub flags: ClassFlags,
    /// `extends` (FQN).
    pub parent: Option<Box<[u8]>>,
    /// `implements` (or, for an interface, `extends`) names (FQN), in order.
    pub interfaces: Vec<Box<[u8]>>,
    /// `use` statements, in order.
    pub traits: Vec<TraitUse>,
    /// Class constants, in order.
    pub consts: Vec<ConstDecl>,
    /// Properties (static and instance), in order.
    pub props: Vec<PropDecl>,
    /// Methods, in order (hooks and thunks are *not* listed; they are reachable
    /// from `props`/`InitRef::Thunk`).
    pub methods: Vec<MethodDecl>,
    /// Enum cases, in order (empty for non-enums).
    pub enum_cases: Vec<EnumCase>,
    /// Attributes on the class.
    pub attrs: Vec<AttrDef>,
    /// Doc comment.
    pub doc: Option<Box<[u8]>>,
    /// First line of the declaration.
    pub line: u32,
    /// Last line of the declaration.
    pub end_line: u32,
    /// Source span of the whole declaration.
    pub span: Span,
    /// The constant pool that every `InitRef::Const` inside this declaration
    /// (constants, property defaults, enum case values, attribute arguments)
    /// indexes.
    pub pool: Vec<Const>,
}

impl ClassDecl {
    /// A bare `class Name {}` with no members, for struct-update construction.
    pub fn new_minimal(name: &[u8], kind: ClassKind) -> ClassDecl {
        ClassDecl {
            name: Box::from(name),
            kind,
            flags: ClassFlags::NONE,
            parent: None,
            interfaces: Vec::new(),
            traits: Vec::new(),
            consts: Vec::new(),
            props: Vec::new(),
            methods: Vec::new(),
            enum_cases: Vec::new(),
            attrs: Vec::new(),
            doc: None,
            line: 0,
            end_line: 0,
            span: Span::dummy(),
            pool: Vec::new(),
        }
    }

    /// Find a method by (case-insensitive) name.
    pub fn method(&self, name: &[u8]) -> Option<&MethodDecl> {
        self.methods
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// Find a property by (case-sensitive) name.
    pub fn prop(&self, name: &[u8]) -> Option<&PropDecl> {
        self.props.iter().find(|p| p.name.as_ref() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_decl_lookups() {
        let mut c = ClassDecl::new_minimal(b"Foo", ClassKind::Class);
        c.methods.push(MethodDecl {
            name: Box::from(&b"getBar"[..]),
            func: 1,
            vis: Visibility::Public,
            is_static: false,
            is_abstract: false,
            is_final: false,
        });
        assert!(c.method(b"GETBAR").is_some());
        assert!(c.method(b"nope").is_none());
        assert!(c.prop(b"x").is_none());
        assert!(c.flags.is_empty());
        assert_eq!(
            format!("{:?}", ClassFlags::FINAL | ClassFlags::READONLY),
            "ClassFlags(FINAL | READONLY)"
        );
        assert_eq!(c.kind, ClassKind::Class);
        assert_ne!(
            ClassKind::Enum { backing: None },
            ClassKind::Enum {
                backing: Some(BuiltinType::Int)
            }
        );
    }
}
