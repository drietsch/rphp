//! AST v2 — the full PHP 8.4/8.5 typed syntax tree (roadmap Track F, F1).
//!
//! This module is the contract between three parties:
//!
//! * the parser adapter (`rphp-parser`, F2) that builds it from `mago-syntax`,
//! * the HIR pass (`rphp-hir`, F4) that resolves names and desugars *in place*,
//! * the compiler (F3) that lowers it to bytecode.
//!
//! Design rules, in order of importance:
//!
//! * **Owned tree.** `Box`/`Vec` only, no lifetimes, no arena. Every node
//!   carries a [`Span`](rphp_span::Span); [`Expr::span`] and [`Stmt::span`]
//!   give uniform access.
//! * **One vocabulary for AST and HIR.** The HIR is this same tree restricted
//!   to a canonical subset, plus exactly three HIR-only nodes: [`Expr::Let`],
//!   [`Expr::Temp`] and [`Expr::Seq`]. There is therefore one printer
//!   ([`pretty`]), one visitor ([`visit`]) and one compiler input.
//! * **Syntax, not semantics.** The parser adapter stores what was written:
//!   no constant folding, no desugaring, no name resolution. Names carry an
//!   optional [`Resolved`] slot that the resolver fills through
//!   [`visit::VisitorMut`].
//! * **Bytes, not strings.** Identifiers, variable names, decoded string
//!   literal contents, inline HTML and doc comments are interned as
//!   [`IdentId`](rphp_intern::IdentId) in the [`Interner`](rphp_intern::Interner)
//!   that the adapter is handed; the tree never owns text.
//! * **Lvalues are expressions.** Assignment targets, foreach targets, `unset`,
//!   `global` and by-reference arguments are ordinary [`Expr`]s restricted by
//!   [`lvalue::validate`], which mirrors Zend's compile-time rejections.
//!
//! Module map:
//!
//! | module     | contents                                                          |
//! |------------|-------------------------------------------------------------------|
//! | [`name`]   | `Name`, `NameKind`, `Resolved`, `ClassRef`, `MemberName`, `MagicKind` |
//! | [`types`]  | `Type`, `TypeKind`, `Builtin`                                     |
//! | [`expr`]   | `Expr` and its satellites (`Arg`, `ArrayItem`, `Closure`, ...)     |
//! | [`stmt`]   | `Program`, `Stmt` and its satellites (`Catch`, `Case`, ...)        |
//! | [`decl`]   | `FuncDecl`, `ClassLike`, `Member`, `Param`, `Hook`, `Modifiers`    |
//! | [`attr`]   | `AttrGroup`, `Attr`                                               |
//! | [`lvalue`] | write-context validation                                          |
//! | [`pretty`] | deterministic S-expression printer for `--emit=ast`               |
//! | [`visit`]  | `Visitor` / `VisitorMut` with `walk_*` defaults                    |
//!
//! Every node type is re-exported at `rphp_ast::v2::*` for convenience.
//!
//! The M0 AST at the crate root is untouched and remains the compiler's input
//! until F3 migrates it to this module.

pub mod attr;
pub mod decl;
pub mod expr;
pub mod lvalue;
pub mod name;
pub mod pretty;
pub mod stmt;
pub mod types;
pub mod visit;

#[cfg(test)]
mod tests;

pub use attr::{Attr, AttrGroup};
pub use decl::{
    Adaptation, ClassKind, ClassLike, ConstMember, EnumCase, FuncDecl, Hook, HookBody, HookKind,
    Member, MethodDecl, Modifiers, Param, PropItem, PropMember, TraitUse, Visibility,
};
pub use expr::{
    Arg, ArrayItem, ArraySyntax, ArrowFn, BinOp, CallableTarget, Callee, CastKind, Closure,
    ClosureUse, ConstSel, Expr, IncludeKind, InterpPart, MatchArm, NewTarget, TempId, UnOp,
};
pub use name::{ClassRef, MagicKind, MemberName, Name, NameKind, Resolved};
pub use stmt::{
    Case, Catch, ConstItem, Directive, ElseIf, Program, StaticVar, Stmt, UseItem, UseKind,
};
pub use types::{Builtin, Type, TypeKind};
pub use visit::{Visitor, VisitorMut};
