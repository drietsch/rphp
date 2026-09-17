//! Names as written in source, class references, member names and magic
//! constants.
//!
//! A [`Name`] is *syntax*: it records the spelling and whether it was
//! unqualified, qualified, fully qualified or `namespace\`-relative. The
//! resolver (F4) fills [`Name::resolved`] in place; nothing in this crate
//! resolves anything.

use rphp_intern::IdentId;
use rphp_span::Span;

use super::expr::Expr;

/// How a name was written, which decides how it resolves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NameKind {
    /// A single segment without a leading `\`: `Foo`. Subject to imports and
    /// (for functions/constants) the namespace-then-global fallback.
    Unqualified,
    /// Several segments without a leading `\`: `Foo\Bar`. The first segment is
    /// subject to `use` imports; the rest is appended.
    Qualified,
    /// Leading `\`: `\Foo\Bar`. Never subject to imports or the current
    /// namespace.
    FullyQualified,
    /// `namespace\Foo`: relative to the current namespace, never to imports.
    Relative,
}

/// The outcome of name resolution, stored on the [`Name`] by the resolver.
///
/// `key` fields are the *lookup keys* the runtime tables use: class and
/// function keys are lowercased, constant keys keep their case (the namespace
/// part of a constant key is lowercased by the resolver; the last segment is
/// not). `fqn` is the display spelling (original case) used for `::class` and
/// messages.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Resolved {
    /// A class-like name (class, interface, trait, enum): statically known.
    Class {
        /// Fully qualified name without a leading `\`, original case.
        fqn: IdentId,
        /// Lowercased `fqn`, the class-table key.
        key: IdentId,
    },
    /// A function name. Unqualified names inside a namespace resolve at
    /// runtime with the two-step rule (`ns_key` first, then `global_key`);
    /// otherwise `ns_key` is `None` and `global_key` is the single candidate.
    Func {
        /// Namespaced candidate key (lowercased), if the two-step rule applies.
        ns_key: Option<IdentId>,
        /// The fallback (or only) key, lowercased.
        global_key: IdentId,
    },
    /// A constant name, with the same two-step shape as [`Resolved::Func`].
    /// Constant keys are case-sensitive in their last segment.
    Const {
        /// Namespaced candidate key, if the two-step rule applies.
        ns_key: Option<IdentId>,
        /// The fallback (or only) key.
        global_key: IdentId,
    },
}

/// A name as written in source.
///
/// `text` is the spelling *without* the leading `\` (fully qualified) and
/// *without* the `namespace\` prefix (relative); segments are joined by `\`.
/// `kind` carries the stripped information. The adapter never resolves
/// `self`/`static`/`parent` into a `Name` where a [`ClassRef`] is expected, but
/// they can legitimately appear as `Name`s in type positions and in
/// `instanceof`; the resolver handles those.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Name {
    /// Interned spelling, see the type-level docs.
    pub text: IdentId,
    /// Qualification as written.
    pub kind: NameKind,
    /// Span of the whole name including any leading `\` or `namespace\`.
    pub span: Span,
    /// Filled by the resolver; `None` straight out of the parser.
    pub resolved: Option<Resolved>,
}

impl Name {
    /// An unresolved name.
    pub fn new(text: IdentId, kind: NameKind, span: Span) -> Self {
        Self {
            text,
            kind,
            span,
            resolved: None,
        }
    }

    /// `true` for `\Foo`-style names, which bypass imports and the namespace.
    pub fn is_fully_qualified(&self) -> bool {
        self.kind == NameKind::FullyQualified
    }

    /// `true` for single-segment names without a leading `\`.
    pub fn is_unqualified(&self) -> bool {
        self.kind == NameKind::Unqualified
    }
}

/// The class part of a scoped construct: `X::CONST`, `X::$prop`, `X::m()`,
/// `new X`, `$v instanceof X`.
#[derive(Clone, PartialEq, Debug)]
pub enum ClassRef {
    /// A written class name (`Foo`, `\Foo\Bar`, `namespace\Foo`).
    Named(Name),
    /// The `self` keyword (case-insensitive in source).
    SelfKw(Span),
    /// The `static` keyword (late static binding).
    Static(Span),
    /// The `parent` keyword.
    Parent(Span),
    /// A dynamic class: `$obj::X`, `$name::m()`, `new $cls`, `new (expr)`.
    Expr(Box<Expr>),
}

impl ClassRef {
    /// The span of the class reference as written.
    pub fn span(&self) -> Span {
        match self {
            ClassRef::Named(n) => n.span,
            ClassRef::SelfKw(s) | ClassRef::Static(s) | ClassRef::Parent(s) => *s,
            ClassRef::Expr(e) => e.span(),
        }
    }

    /// `true` for `self`/`static`/`parent`, which need a class scope.
    pub fn is_scope_keyword(&self) -> bool {
        matches!(
            self,
            ClassRef::SelfKw(_) | ClassRef::Static(_) | ClassRef::Parent(_)
        )
    }
}

/// A property or method name after `->`, `?->` or `::`.
#[derive(Clone, PartialEq, Debug)]
pub enum MemberName {
    /// A literal identifier: `$o->foo`, `$o->foo()`, `X::$foo`, `X::foo()`.
    /// Keywords are allowed here by PHP (`$o->class`, `$o->list()`), so the
    /// adapter interns whatever identifier-like token it sees.
    Ident(IdentId, Span),
    /// A computed name: `$o->$name`, `$o->{'a' . 'b'}`, `$o->$name()`,
    /// `X::$$name`, `X::{expr}()`.
    Expr(Box<Expr>),
}

impl MemberName {
    /// The span of the member name as written (for `Expr`, the inner
    /// expression's span).
    pub fn span(&self) -> Span {
        match self {
            MemberName::Ident(_, s) => *s,
            MemberName::Expr(e) => e.span(),
        }
    }

    /// The identifier if the name is literal.
    pub fn as_ident(&self) -> Option<IdentId> {
        match self {
            MemberName::Ident(id, _) => Some(*id),
            MemberName::Expr(_) => None,
        }
    }
}

/// A magic constant (`__LINE__` etc.). The value is a resolver/compiler
/// concern; the parser only records which one was written.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MagicKind {
    /// `__LINE__`
    Line,
    /// `__FILE__`
    File,
    /// `__DIR__`
    Dir,
    /// `__CLASS__` (stays dynamic inside traits)
    Class,
    /// `__FUNCTION__` (PHP 8.4 closure naming applies)
    Function,
    /// `__METHOD__`
    Method,
    /// `__NAMESPACE__`
    Namespace,
    /// `__TRAIT__`
    Trait,
    /// `__PROPERTY__` (PHP 8.4, inside property hooks)
    Property,
}

impl MagicKind {
    /// The source spelling, e.g. `"__LINE__"`.
    pub fn as_str(self) -> &'static str {
        match self {
            MagicKind::Line => "__LINE__",
            MagicKind::File => "__FILE__",
            MagicKind::Dir => "__DIR__",
            MagicKind::Class => "__CLASS__",
            MagicKind::Function => "__FUNCTION__",
            MagicKind::Method => "__METHOD__",
            MagicKind::Namespace => "__NAMESPACE__",
            MagicKind::Trait => "__TRAIT__",
            MagicKind::Property => "__PROPERTY__",
        }
    }
}
