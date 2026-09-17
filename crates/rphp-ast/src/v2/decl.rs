//! Declarations: functions, class-likes and their members, parameters,
//! property hooks and modifiers.

use rphp_intern::IdentId;
use rphp_span::Span;

use super::attr::AttrGroup;
use super::expr::Expr;
use super::name::Name;
use super::stmt::{ConstItem, Stmt};
use super::types::Type;

/// Member visibility.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Visibility {
    /// `public`
    Public,
    /// `protected`
    Protected,
    /// `private`
    Private,
}

impl Visibility {
    /// The keyword spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Protected => "protected",
            Visibility::Private => "private",
        }
    }
}

/// The modifier keywords written before a declaration, member or promoted
/// parameter. Absent modifiers are `None`/`false`; defaults (implicit
/// `public`) are applied by later passes, not stored here.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Modifiers {
    /// `public` / `protected` / `private` (the read visibility).
    pub vis: Option<Visibility>,
    /// `public(set)` / `protected(set)` / `private(set)` (PHP 8.4 asymmetric
    /// visibility).
    pub set_vis: Option<Visibility>,
    /// `static`
    pub static_: bool,
    /// `readonly` (property, promoted parameter, or class)
    pub readonly: bool,
    /// `final`
    pub final_: bool,
    /// `abstract`
    pub abstract_: bool,
    /// Span covering all modifier keywords; [`Span::dummy`]-like empty span
    /// when none were written.
    pub span: Span,
}

impl Default for Modifiers {
    /// No modifiers, with an empty placeholder span.
    fn default() -> Self {
        Self {
            vis: None,
            set_vis: None,
            static_: false,
            readonly: false,
            final_: false,
            abstract_: false,
            span: Span::dummy(),
        }
    }
}

impl Modifiers {
    /// `true` if no modifier keyword was written.
    pub fn is_empty(&self) -> bool {
        self.vis.is_none()
            && self.set_vis.is_none()
            && !self.static_
            && !self.readonly
            && !self.final_
            && !self.abstract_
    }
}

/// A declared parameter of a function, method, closure, arrow function or
/// `set` hook.
#[derive(Clone, PartialEq, Debug)]
pub struct Param {
    /// Attributes before the parameter.
    pub attrs: Vec<AttrGroup>,
    /// The parameter name without `$`.
    pub name: IdentId,
    /// The declared type.
    pub ty: Option<Type>,
    /// The default value (a constant expression).
    pub default: Option<Expr>,
    /// `&$x`
    pub by_ref: bool,
    /// `...$x`
    pub variadic: bool,
    /// Constructor promotion modifiers (`public`, `private(set)`, `readonly`,
    /// ...); `Some` iff any promotion modifier was written.
    pub promote: Option<Modifiers>,
    /// Property hooks on a promoted parameter (PHP 8.4).
    pub hooks: Vec<Hook>,
    /// Span from the first attribute/modifier to the end of the default.
    pub span: Span,
}

/// Which property hook.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HookKind {
    /// `get`
    Get,
    /// `set`
    Set,
}

impl HookKind {
    /// The keyword spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            HookKind::Get => "get",
            HookKind::Set => "set",
        }
    }
}

/// The body of a property hook.
#[derive(Clone, PartialEq, Debug)]
pub enum HookBody {
    /// `get => expr;` / `set => expr;`
    Expr(Expr),
    /// `get { ... }` / `set { ... }`
    Block(Vec<Stmt>),
    /// `get;` / `set;` (abstract hook in an interface or abstract property).
    Abstract,
}

/// A property hook (PHP 8.4): `[final] [&]get|set [(params)] body`.
#[derive(Clone, PartialEq, Debug)]
pub struct Hook {
    /// `get` or `set`.
    pub kind: HookKind,
    /// Attributes before the hook.
    pub attrs: Vec<AttrGroup>,
    /// `final get ...`
    pub final_: bool,
    /// `&get ...` (returns by reference).
    pub by_ref: bool,
    /// The explicit parameter list of `set(Type $value)`; `None` means the
    /// implicit `$value` parameter with the property's type.
    pub params: Option<Vec<Param>>,
    /// The body.
    pub body: HookBody,
    /// Span from the first attribute/modifier to the end of the body.
    pub span: Span,
}

/// A named function declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct FuncDecl {
    /// Attributes before `function`.
    pub attrs: Vec<AttrGroup>,
    /// The function name as written (unqualified; the namespace is applied by
    /// the resolver).
    pub name: IdentId,
    /// `function &name(...)`.
    pub by_ref: bool,
    /// Declared parameters.
    pub params: Vec<Param>,
    /// Declared return type.
    pub ret: Option<Type>,
    /// The body.
    pub body: Vec<Stmt>,
    /// The docblock immediately preceding the declaration.
    pub doc: Option<IdentId>,
    /// Span from the first attribute to the closing `}`.
    pub span: Span,
}

/// Which kind of class-like declaration.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClassKind {
    /// `class`
    Class,
    /// `interface`
    Interface,
    /// `trait`
    Trait,
    /// `enum`
    Enum,
}

impl ClassKind {
    /// The keyword spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ClassKind::Class => "class",
            ClassKind::Interface => "interface",
            ClassKind::Trait => "trait",
            ClassKind::Enum => "enum",
        }
    }
}

/// A class, interface, trait or enum declaration (named or anonymous).
#[derive(Clone, PartialEq, Debug)]
pub struct ClassLike {
    /// Which kind of declaration.
    pub kind: ClassKind,
    /// Attributes before the modifiers/keyword.
    pub attrs: Vec<AttrGroup>,
    /// `abstract`, `final`, `readonly` (only `vis`-less flags apply).
    pub modifiers: Modifiers,
    /// The declared name as written (unqualified); `None` for an anonymous
    /// class.
    pub name: Option<IdentId>,
    /// `extends`: at most one name for a class, any number for an interface.
    pub extends: Vec<Name>,
    /// `implements` (classes, enums).
    pub implements: Vec<Name>,
    /// The backing type of a backed enum (`enum X: int`).
    pub backing: Option<Type>,
    /// The members in source order.
    pub members: Vec<Member>,
    /// The docblock immediately preceding the declaration.
    pub doc: Option<IdentId>,
    /// Span from the first attribute to the closing `}`.
    pub span: Span,
}

/// A class constant declaration `[modifiers] const [Type] A = 1, B = 2;`.
#[derive(Clone, PartialEq, Debug)]
pub struct ConstMember {
    /// Attributes before the modifiers.
    pub attrs: Vec<AttrGroup>,
    /// `public`/`protected`/`private`, `final`.
    pub modifiers: Modifiers,
    /// The declared type (PHP 8.3 typed class constants).
    pub ty: Option<Type>,
    /// The declared constants (at least one).
    pub items: Vec<ConstItem>,
    /// The docblock immediately preceding the declaration.
    pub doc: Option<IdentId>,
    /// Span from the first attribute to `;`.
    pub span: Span,
}

/// One `$name [= default]` of a property declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct PropItem {
    /// The property name without `$`.
    pub name: IdentId,
    /// The default value (a constant expression).
    pub default: Option<Expr>,
    /// Span of `$name = default`.
    pub span: Span,
}

/// A property declaration `[modifiers] [Type] $a = 1, $b;` or a hooked
/// property `[modifiers] [Type] $a [= default] { get ...; set ...; }`.
#[derive(Clone, PartialEq, Debug)]
pub struct PropMember {
    /// Attributes before the modifiers.
    pub attrs: Vec<AttrGroup>,
    /// Visibility, `set` visibility, `static`, `readonly`, `final`, `abstract`.
    pub modifiers: Modifiers,
    /// The declared type.
    pub ty: Option<Type>,
    /// The declared properties (at least one; exactly one when `hooks` is
    /// non-empty).
    pub items: Vec<PropItem>,
    /// Property hooks (PHP 8.4). Empty for a plain property.
    pub hooks: Vec<Hook>,
    /// The docblock immediately preceding the declaration.
    pub doc: Option<IdentId>,
    /// Span from the first attribute to `;` or `}`.
    pub span: Span,
}

/// A method declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct MethodDecl {
    /// Attributes before the modifiers.
    pub attrs: Vec<AttrGroup>,
    /// Visibility, `static`, `final`, `abstract`.
    pub modifiers: Modifiers,
    /// `function &name(...)`.
    pub by_ref: bool,
    /// The method name as written (case preserved).
    pub name: IdentId,
    /// Declared parameters.
    pub params: Vec<Param>,
    /// Declared return type.
    pub ret: Option<Type>,
    /// The body; `None` for abstract and interface methods (`;`).
    pub body: Option<Vec<Stmt>>,
    /// The docblock immediately preceding the declaration.
    pub doc: Option<IdentId>,
    /// Span from the first attribute to `}` or `;`.
    pub span: Span,
}

/// An enum case `case NAME [= value];`.
#[derive(Clone, PartialEq, Debug)]
pub struct EnumCase {
    /// Attributes before `case`.
    pub attrs: Vec<AttrGroup>,
    /// The case name.
    pub name: IdentId,
    /// The backing value for backed enums.
    pub value: Option<Expr>,
    /// The docblock immediately preceding the case.
    pub doc: Option<IdentId>,
    /// Span from the first attribute to `;`.
    pub span: Span,
}

/// A trait adaptation inside `use T { ... }`.
#[derive(Clone, PartialEq, Debug)]
pub enum Adaptation {
    /// `T::m insteadof U, V;`
    Precedence {
        /// The trait whose method wins.
        trait_: Name,
        /// The method name.
        method: IdentId,
        /// The traits whose copies are excluded (at least one).
        insteadof: Vec<Name>,
        /// Span of the adaptation including `;`.
        span: Span,
    },
    /// `[T::]m as [visibility] [alias];`
    Alias {
        /// The trait the method comes from, if qualified.
        trait_: Option<Name>,
        /// The method name.
        method: IdentId,
        /// The new name, if any.
        alias: Option<IdentId>,
        /// The changed visibility, if any (at least one of `alias`/`vis` is
        /// present).
        vis: Option<Visibility>,
        /// Span of the adaptation including `;`.
        span: Span,
    },
}

impl Adaptation {
    /// The span of the adaptation.
    pub fn span(&self) -> Span {
        match self {
            Adaptation::Precedence { span, .. } | Adaptation::Alias { span, .. } => *span,
        }
    }
}

/// `use T1, T2 { adaptations }` inside a class-like.
#[derive(Clone, PartialEq, Debug)]
pub struct TraitUse {
    /// The used traits (at least one).
    pub traits: Vec<Name>,
    /// The adaptation block; empty for `use T;`.
    pub adaptations: Vec<Adaptation>,
    /// Span from `use` to `;` or `}`.
    pub span: Span,
}

/// A member of a class-like body.
#[derive(Clone, PartialEq, Debug)]
pub enum Member {
    /// A class constant declaration.
    Const(ConstMember),
    /// A property declaration (plain or hooked).
    Prop(PropMember),
    /// A method declaration.
    Method(MethodDecl),
    /// An enum case.
    EnumCase(EnumCase),
    /// A trait use.
    TraitUse(TraitUse),
}

impl Member {
    /// The span of the member.
    pub fn span(&self) -> Span {
        match self {
            Member::Const(c) => c.span,
            Member::Prop(p) => p.span,
            Member::Method(m) => m.span,
            Member::EnumCase(c) => c.span,
            Member::TraitUse(t) => t.span,
        }
    }
}
