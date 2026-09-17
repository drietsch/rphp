//! Attributes: `#[Foo, Bar(1, name: 2)]`.
//!
//! Attributes are pure metadata in the tree; validation (target flags,
//! repeatability, constant-expression arguments) happens in F7's metadata
//! pass.

use rphp_span::Span;

use super::expr::Arg;
use super::name::Name;

/// One `#[...]` group, which may contain several comma-separated attributes.
#[derive(Clone, PartialEq, Debug)]
pub struct AttrGroup {
    /// The attributes inside the group, in source order.
    pub attrs: Vec<Attr>,
    /// Span from `#[` to `]`.
    pub span: Span,
}

/// A single attribute `Name(args)` inside a group.
#[derive(Clone, PartialEq, Debug)]
pub struct Attr {
    /// The attribute class name as written (resolved like any class name).
    pub name: Name,
    /// Constructor arguments; named and positional, no spread allowed by PHP
    /// (rejected by validation, not by the tree).
    pub args: Vec<Arg>,
    /// Span of the attribute including its argument list.
    pub span: Span,
}
