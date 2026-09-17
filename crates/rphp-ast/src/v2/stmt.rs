//! Statements and the per-file [`Program`] root.

use rphp_intern::IdentId;
use rphp_span::{FileId, Span};

use super::attr::AttrGroup;
use super::decl::{ClassLike, FuncDecl};
use super::expr::Expr;
use super::name::Name;

/// One parsed file (or `eval` string).
#[derive(Clone, PartialEq, Debug)]
pub struct Program {
    /// The file this tree was parsed from (index into the `SourceMap`).
    pub file: FileId,
    /// `declare(strict_types=1)` was present. The `Stmt::Declare` itself stays
    /// in `items` so placement rules can be validated (F4).
    pub strict_types: bool,
    /// Top-level statements in source order. Unbraced namespaces group their
    /// following statements into `Stmt::Namespace::body`.
    pub items: Vec<Stmt>,
    /// Byte offset just after `__halt_compiler();` when present: the value of
    /// `__COMPILER_HALT_OFFSET__` and the start of the trailing data.
    pub halt_offset: Option<u32>,
}

/// An `elseif` clause. Alternative syntax (`elseif:`) produces the same node.
#[derive(Clone, PartialEq, Debug)]
pub struct ElseIf {
    /// The condition.
    pub cond: Expr,
    /// The branch body.
    pub body: Vec<Stmt>,
    /// Span from `elseif` to the end of the body.
    pub span: Span,
}

/// A `case`/`default` of a `switch`.
#[derive(Clone, PartialEq, Debug)]
pub struct Case {
    /// The compared value; `None` for `default`.
    pub cond: Option<Expr>,
    /// The statements up to the next case (fall-through is implicit).
    pub body: Vec<Stmt>,
    /// Span from `case`/`default` to the end of the body.
    pub span: Span,
}

/// A `catch` clause.
#[derive(Clone, PartialEq, Debug)]
pub struct Catch {
    /// The caught class names (`A | B`); at least one.
    pub types: Vec<Name>,
    /// The variable name without `$`; `None` for PHP 8 `catch (E)`.
    pub var: Option<IdentId>,
    /// The handler body.
    pub body: Vec<Stmt>,
    /// Span from `catch` to `}`.
    pub span: Span,
}

/// One `static $x [= init]` item.
#[derive(Clone, PartialEq, Debug)]
pub struct StaticVar {
    /// The variable name without `$`.
    pub name: IdentId,
    /// The initializer (PHP 8.3: any expression).
    pub init: Option<Expr>,
    /// Span of `$x = init`.
    pub span: Span,
}

/// One `name=value` directive of a `declare`.
#[derive(Clone, PartialEq, Debug)]
pub struct Directive {
    /// The directive name (`strict_types`, `ticks`, `encoding`), as written.
    pub name: IdentId,
    /// The literal value.
    pub value: Expr,
    /// Span of `name=value`.
    pub span: Span,
}

/// Which symbol table a `use` imports from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UseKind {
    /// `use Foo\Bar;` — classes, interfaces, traits, enums (namespaces too).
    Class,
    /// `use function Foo\bar;`
    Function,
    /// `use const Foo\BAR;`
    Const,
}

impl UseKind {
    /// Stable lowercase name (`class`, `function`, `const`).
    pub fn as_str(self) -> &'static str {
        match self {
            UseKind::Class => "class",
            UseKind::Function => "function",
            UseKind::Const => "const",
        }
    }
}

/// One item of a `use` statement.
#[derive(Clone, PartialEq, Debug)]
pub struct UseItem {
    /// The imported name. Inside a group use (`Stmt::Use::prefix` is `Some`)
    /// it is relative to the prefix; otherwise it is the full name as written
    /// (a leading `\` is permitted and meaningless).
    pub name: Name,
    /// `as Alias`.
    pub alias: Option<IdentId>,
    /// A per-item kind override in a mixed group use
    /// (`use Foo\{function bar, const BAZ, Qux}`); `None` inherits the
    /// statement's kind.
    pub kind: Option<UseKind>,
    /// Span of `name [as alias]` including a per-item kind keyword.
    pub span: Span,
}

/// One `NAME = value` item of a `const` declaration (top-level or member).
#[derive(Clone, PartialEq, Debug)]
pub struct ConstItem {
    /// The constant name, case-sensitive, as written.
    pub name: IdentId,
    /// The value (a constant expression, validated in F7).
    pub value: Expr,
    /// Span of `NAME = value`.
    pub span: Span,
}

/// A statement. Every variant carries a span, reachable via [`Stmt::span`].
#[derive(Clone, PartialEq, Debug)]
pub enum Stmt {
    /// Text outside `<?php ... ?>` tags, emitted verbatim.
    InlineHtml {
        /// The raw bytes.
        text: IdentId,
        /// Span of the text.
        span: Span,
    },
    /// An expression statement `expr;`.
    Expr {
        /// The expression.
        expr: Expr,
        /// Span including the `;` (or the closing tag that ends it).
        span: Span,
    },
    /// `echo a, b;` (also `<?= a, b ?>`).
    Echo {
        /// The echoed expressions, at least one.
        args: Vec<Expr>,
        /// Span from `echo` to `;`.
        span: Span,
    },
    /// `{ ... }`.
    Block {
        /// The statements.
        body: Vec<Stmt>,
        /// Span from `{` to `}`.
        span: Span,
    },
    /// `if (cond) then elseif ... else ...`, in either syntax. Bodies are
    /// always statement lists (a single statement is a one-element list).
    If {
        /// The condition.
        cond: Expr,
        /// The `then` body.
        then: Vec<Stmt>,
        /// The `elseif` clauses in order.
        elseifs: Vec<ElseIf>,
        /// The `else` body, if present.
        else_: Option<Vec<Stmt>>,
        /// Span from `if` to the end of the last branch.
        span: Span,
    },
    /// `while (cond) body`.
    While {
        /// The condition.
        cond: Expr,
        /// The body.
        body: Vec<Stmt>,
        /// Span from `while` to the end of the body.
        span: Span,
    },
    /// `do body while (cond);`.
    DoWhile {
        /// The body.
        body: Vec<Stmt>,
        /// The condition.
        cond: Expr,
        /// Span from `do` to `;`.
        span: Span,
    },
    /// `for (init; cond; step) body`. Each part is a comma list (possibly
    /// empty); the last `cond` decides.
    For {
        /// The initializers.
        init: Vec<Expr>,
        /// The conditions (the last one's value is tested).
        cond: Vec<Expr>,
        /// The step expressions.
        step: Vec<Expr>,
        /// The body.
        body: Vec<Stmt>,
        /// Span from `for` to the end of the body.
        span: Span,
    },
    /// `foreach (subject as [key =>] [&]value) body`. The two targets are
    /// boxed so that this variant does not dominate `size_of::<Stmt>()`.
    Foreach {
        /// The iterated expression.
        subject: Expr,
        /// The key target, if `key =>` was written (lvalue-shaped).
        key: Option<Box<Expr>>,
        /// The value target: lvalue-shaped, or an [`Expr::Array`] destructuring
        /// pattern.
        value: Box<Expr>,
        /// `as &$v`.
        by_ref: bool,
        /// The body.
        body: Vec<Stmt>,
        /// Span from `foreach` to the end of the body.
        span: Span,
    },
    /// `switch (subject) { cases }`.
    Switch {
        /// The switched value.
        subject: Expr,
        /// The cases in source order.
        cases: Vec<Case>,
        /// Span from `switch` to `}`.
        span: Span,
    },
    /// `break;` / `break N;` (`levels` is 1 when omitted).
    Break {
        /// How many enclosing loops/switches to leave (≥ 1).
        levels: u32,
        /// Span from `break` to `;`.
        span: Span,
    },
    /// `continue;` / `continue N;` (`levels` is 1 when omitted).
    Continue {
        /// How many enclosing loops/switches to continue (≥ 1).
        levels: u32,
        /// Span from `continue` to `;`.
        span: Span,
    },
    /// `return;` / `return value;`.
    Return {
        /// The returned value, if any.
        value: Option<Expr>,
        /// Span from `return` to `;`.
        span: Span,
    },
    /// `try { } catch (...) { } finally { }`.
    Try {
        /// The protected body.
        body: Vec<Stmt>,
        /// The catch clauses (may be empty when `finally` is present).
        catches: Vec<Catch>,
        /// The `finally` body.
        finally: Option<Vec<Stmt>>,
        /// Span from `try` to the last `}`.
        span: Span,
    },
    /// `goto label;`.
    Goto {
        /// The target label.
        label: IdentId,
        /// Span from `goto` to `;`.
        span: Span,
    },
    /// `label:`.
    Label {
        /// The label name.
        name: IdentId,
        /// Span of `label:`.
        span: Span,
    },
    /// `global $a, $$b;` — each item is an [`Expr::Var`] or [`Expr::VarVar`].
    Global {
        /// The variables.
        vars: Vec<Expr>,
        /// Span from `global` to `;`.
        span: Span,
    },
    /// `static $a = 1, $b;` (function-static variables).
    StaticVar {
        /// The declared variables.
        vars: Vec<StaticVar>,
        /// Span from `static` to `;`.
        span: Span,
    },
    /// `unset($a, $b[0]);`.
    Unset {
        /// The targets (lvalue-shaped).
        targets: Vec<Expr>,
        /// Span from `unset` to `;`.
        span: Span,
    },
    /// `declare(directives);` or `declare(directives) { body }` / `: ... enddeclare;`.
    Declare {
        /// The directives.
        directives: Vec<Directive>,
        /// The scoped body, if a block form was written.
        body: Option<Vec<Stmt>>,
        /// Span from `declare` to the end.
        span: Span,
    },
    /// `namespace Name;`, `namespace Name { }` or the global `namespace { }`.
    /// For the unbraced form the adapter groups every following statement up
    /// to the next `namespace` (or end of file) into `body`, so both forms
    /// have the same shape.
    Namespace {
        /// The namespace name; `None` for the global `namespace { }`.
        name: Option<Name>,
        /// The statements inside the namespace.
        body: Vec<Stmt>,
        /// Whether the braced form was written.
        braced: bool,
        /// Span from `namespace` to the end (of the last grouped statement for
        /// the unbraced form).
        span: Span,
    },
    /// `use ...;` in any of its forms.
    Use {
        /// The statement-level kind (`use`, `use function`, `use const`).
        kind: UseKind,
        /// The common prefix of a group use `use Prefix\{...}`; `None` for
        /// plain (possibly comma-separated) imports.
        prefix: Option<Name>,
        /// The imported items.
        items: Vec<UseItem>,
        /// Span from `use` to `;`.
        span: Span,
    },
    /// A top-level `const A = 1, B = 2;` (attributes since PHP 8.5).
    ConstDecl {
        /// Attributes before `const`.
        attrs: Vec<AttrGroup>,
        /// The declared constants.
        items: Vec<ConstItem>,
        /// Span from the first attribute or `const` to `;`.
        span: Span,
    },
    /// A function declaration.
    Func(FuncDecl),
    /// A class, interface, trait or enum declaration.
    ClassLike(ClassLike),
    /// `__halt_compiler();` — see [`Program::halt_offset`].
    HaltCompiler {
        /// Span of the call including `;`.
        span: Span,
    },
    /// An empty statement `;`.
    Nop {
        /// Span of the `;`.
        span: Span,
    },
}

impl Stmt {
    /// The span of the statement.
    pub fn span(&self) -> Span {
        match self {
            Stmt::Func(f) => f.span,
            Stmt::ClassLike(c) => c.span,
            Stmt::InlineHtml { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::Echo { span, .. }
            | Stmt::Block { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::DoWhile { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Foreach { span, .. }
            | Stmt::Switch { span, .. }
            | Stmt::Break { span, .. }
            | Stmt::Continue { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Try { span, .. }
            | Stmt::Goto { span, .. }
            | Stmt::Label { span, .. }
            | Stmt::Global { span, .. }
            | Stmt::StaticVar { span, .. }
            | Stmt::Unset { span, .. }
            | Stmt::Declare { span, .. }
            | Stmt::Namespace { span, .. }
            | Stmt::Use { span, .. }
            | Stmt::ConstDecl { span, .. }
            | Stmt::HaltCompiler { span }
            | Stmt::Nop { span } => *span,
        }
    }

    /// `true` for declarations that PHP may hoist (functions and class-likes),
    /// regardless of whether the hoisting conditions hold.
    pub fn is_declaration(&self) -> bool {
        matches!(self, Stmt::Func(_) | Stmt::ClassLike(_))
    }
}
