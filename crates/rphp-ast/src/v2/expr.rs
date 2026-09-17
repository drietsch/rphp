//! Expressions.
//!
//! [`Expr`] covers the full PHP 8.4/8.5 expression grammar plus the three
//! HIR-only nodes (`Let`, `Temp`, `Seq`). Write targets (assignment, foreach,
//! `unset`, `global`, by-reference arguments) are ordinary expressions
//! restricted by [`super::lvalue::validate`].
//!
//! Heavy payloads ([`Closure`], [`ArrowFn`]) are boxed so that `Expr` stays
//! around a hundred bytes; the size is pinned by a test in `tests.rs`.

use rphp_intern::IdentId;
use rphp_span::Span;

use super::attr::AttrGroup;
use super::decl::{ClassLike, Param};
use super::name::{ClassRef, MagicKind, MemberName, Name};
use super::stmt::Stmt;
use super::types::Type;

/// Identifier of a compiler-introduced temporary (HIR only). Temporaries are
/// numbered per function body by the desugarer; the compiler maps them to
/// registers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TempId(pub u32);

/// A call argument: positional `f($x)`, named `f(name: $x)` or spread
/// `f(...$xs)`. Named and spread are mutually exclusive in PHP; the tree does
/// not enforce it.
#[derive(Clone, PartialEq, Debug)]
pub struct Arg {
    /// The parameter name for a named argument.
    pub name: Option<IdentId>,
    /// The argument value.
    pub value: Expr,
    /// `...$value` (argument unpacking).
    pub spread: bool,
    /// Span including the name and `...`.
    pub span: Span,
}

/// Which array syntax was written. Only matters for destructuring
/// diagnostics ("Cannot mix [] and list()", "Cannot assign to array()").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ArraySyntax {
    /// `[ ... ]`
    Short,
    /// `array( ... )`
    Long,
    /// `list( ... )` — only valid as a destructuring target.
    List,
}

impl ArraySyntax {
    /// `true` for `list(...)`.
    pub fn is_list(self) -> bool {
        matches!(self, ArraySyntax::List)
    }

    /// Stable lowercase name for dumps.
    pub fn as_str(self) -> &'static str {
        match self {
            ArraySyntax::Short => "short",
            ArraySyntax::Long => "long",
            ArraySyntax::List => "list",
        }
    }
}

/// One element of an array literal or destructuring pattern.
#[derive(Clone, PartialEq, Debug)]
pub struct ArrayItem {
    /// `key => value`; `None` for positional elements.
    pub key: Option<Expr>,
    /// The element value. `None` is a *skipped slot* (`[, $b] = $a`), which is
    /// only legal in destructuring targets.
    pub value: Option<Expr>,
    /// `&$value` (reference element; legal in literals and destructuring).
    pub by_ref: bool,
    /// `...$value` (spread element; legal only in literals).
    pub spread: bool,
    /// Span of the whole element.
    pub span: Span,
}

/// A piece of an interpolated string (`"a $b {$c->d} e"`, heredoc) or of a
/// backtick command.
#[derive(Clone, PartialEq, Debug)]
pub enum InterpPart {
    /// Literal bytes, already escape-decoded.
    Lit(IdentId, Span),
    /// An embedded expression (`$x`, `$x[0]`, `$x->y`, `{$expr}`, `${name}`).
    Expr(Expr),
}

/// One arm of a `match`.
#[derive(Clone, PartialEq, Debug)]
pub struct MatchArm {
    /// The conditions (`a, b => ...`); `None` for `default => ...`.
    pub conds: Option<Vec<Expr>>,
    /// The arm's result expression.
    pub body: Expr,
    /// Span from the first condition (or `default`) to the end of `body`.
    pub span: Span,
}

/// A `use (...)` capture of a closure.
#[derive(Clone, PartialEq, Debug)]
pub struct ClosureUse {
    /// The captured variable name without `$`.
    pub name: IdentId,
    /// `use (&$x)`.
    pub by_ref: bool,
    /// Span of `[&]$name`.
    pub span: Span,
}

/// `function (...) use (...) { ... }`. Boxed inside [`Expr::Closure`].
#[derive(Clone, PartialEq, Debug)]
pub struct Closure {
    /// `static function`: no bound `$this`.
    pub static_: bool,
    /// `function &(...)`: returns by reference.
    pub by_ref: bool,
    /// Attributes before the `function` keyword.
    pub attrs: Vec<AttrGroup>,
    /// Declared parameters.
    pub params: Vec<Param>,
    /// Captured variables from the `use (...)` clause.
    pub uses: Vec<ClosureUse>,
    /// Declared return type.
    pub ret: Option<Type>,
    /// The body.
    pub body: Vec<Stmt>,
    /// The docblock immediately preceding the closure, if any.
    pub doc: Option<IdentId>,
    /// Span from `static`/`function` to the closing `}`.
    pub span: Span,
}

/// `fn (...) => expr`. Boxed inside [`Expr::ArrowFn`]. Free variables are not
/// recorded here; the desugarer computes them (F4 `free_vars`).
#[derive(Clone, PartialEq, Debug)]
pub struct ArrowFn {
    /// `static fn`.
    pub static_: bool,
    /// `fn &(...)`.
    pub by_ref: bool,
    /// Attributes before the `fn` keyword.
    pub attrs: Vec<AttrGroup>,
    /// Declared parameters.
    pub params: Vec<Param>,
    /// Declared return type.
    pub ret: Option<Type>,
    /// The single body expression.
    pub body: Expr,
    /// The docblock immediately preceding the arrow function, if any.
    pub doc: Option<IdentId>,
    /// Span from `static`/`fn` to the end of `body`.
    pub span: Span,
}

/// What follows `::` in a class-constant fetch.
#[derive(Clone, PartialEq, Debug)]
pub enum ConstSel {
    /// `X::NAME`.
    Ident(IdentId, Span),
    /// `X::class` (the keyword; folded by the resolver when `X` is a static
    /// name, otherwise a runtime fetch).
    Class(Span),
    /// `X::{expr}` (PHP 8.3 dynamic class constant fetch).
    Expr(Box<Expr>),
}

impl ConstSel {
    /// The span of the selector as written.
    pub fn span(&self) -> Span {
        match self {
            ConstSel::Ident(_, s) | ConstSel::Class(s) => *s,
            ConstSel::Expr(e) => e.span(),
        }
    }
}

/// The callee of a plain call.
#[derive(Clone, PartialEq, Debug)]
pub enum Callee {
    /// `foo(...)`, `\ns\foo(...)`, `namespace\foo(...)`.
    Name(Name),
    /// `$f(...)`, `$o->p(...)` after grouping, `(expr)(...)`, `f()(...)`.
    Expr(Box<Expr>),
}

impl Callee {
    /// The span of the callee as written.
    pub fn span(&self) -> Span {
        match self {
            Callee::Name(n) => n.span,
            Callee::Expr(e) => e.span(),
        }
    }
}

/// The target of a first-class callable `(...)` expression.
#[derive(Clone, PartialEq, Debug)]
pub enum CallableTarget {
    /// `strlen(...)`, `$f(...)`.
    Func(Callee),
    /// `$obj->method(...)`. A nullsafe form is a PHP compile error and is
    /// rejected by the conformance pass, so there is no `nullsafe` flag.
    Method {
        /// The object expression.
        obj: Box<Expr>,
        /// The method name.
        name: MemberName,
    },
    /// `Foo::method(...)`.
    Static {
        /// The class part.
        class: ClassRef,
        /// The method name.
        name: MemberName,
    },
}

/// What `new` instantiates.
#[derive(Clone, PartialEq, Debug)]
pub enum NewTarget {
    /// `new Foo`, `new static`, `new $cls`, `new (expr)`.
    Ref(ClassRef),
    /// `new class(...) extends X { ... }`; the constructor arguments live on
    /// the enclosing [`Expr::New`]. `ClassLike::name` is `None`.
    Anon(Box<ClassLike>),
}

/// Unary operators, including casts and the PHP 8.5 `(void)` cast.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UnOp {
    /// `-e`
    Neg,
    /// `+e`
    Plus,
    /// `!e`
    Not,
    /// `~e`
    BitNot,
    /// `@e` (error suppression)
    Silence,
    /// `++e`
    PreInc,
    /// `--e`
    PreDec,
    /// `e++`
    PostInc,
    /// `e--`
    PostDec,
    /// `(int)e`, `(string)e`, ... — see [`CastKind`].
    Cast(CastKind),
    /// `(void)e` (PHP 8.5): evaluates and discards.
    Void,
}

/// The target of a cast. Aliases (`(integer)`, `(boolean)`, `(double)`,
/// `(binary)`) map to the canonical kind; `(real)` and `(unset)` are not
/// PHP 8 and are rejected by the conformance pass.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CastKind {
    /// `(int)`, `(integer)`
    Int,
    /// `(float)`, `(double)`
    Float,
    /// `(string)`, `(binary)`
    String,
    /// `(bool)`, `(boolean)`
    Bool,
    /// `(array)`
    Array,
    /// `(object)`
    Object,
}

impl CastKind {
    /// The canonical spelling inside the parentheses, e.g. `"int"`.
    pub fn as_str(self) -> &'static str {
        match self {
            CastKind::Int => "int",
            CastKind::Float => "float",
            CastKind::String => "string",
            CastKind::Bool => "bool",
            CastKind::Array => "array",
            CastKind::Object => "object",
        }
    }
}

impl UnOp {
    /// Stable lowercase name for dumps (`neg`, `cast-int`, `post-inc`, ...).
    pub fn name(self) -> &'static str {
        match self {
            UnOp::Neg => "neg",
            UnOp::Plus => "plus",
            UnOp::Not => "not",
            UnOp::BitNot => "bit-not",
            UnOp::Silence => "silence",
            UnOp::PreInc => "pre-inc",
            UnOp::PreDec => "pre-dec",
            UnOp::PostInc => "post-inc",
            UnOp::PostDec => "post-dec",
            UnOp::Cast(CastKind::Int) => "cast-int",
            UnOp::Cast(CastKind::Float) => "cast-float",
            UnOp::Cast(CastKind::String) => "cast-string",
            UnOp::Cast(CastKind::Bool) => "cast-bool",
            UnOp::Cast(CastKind::Array) => "cast-array",
            UnOp::Cast(CastKind::Object) => "cast-object",
            UnOp::Void => "void",
        }
    }

    /// `true` for `++`/`--` in either position: the operand is a write target.
    pub fn is_inc_dec(self) -> bool {
        matches!(
            self,
            UnOp::PreInc | UnOp::PreDec | UnOp::PostInc | UnOp::PostDec
        )
    }
}

/// Binary operators. `instanceof` is a separate node ([`Expr::InstanceOf`])
/// because its right operand is a [`ClassRef`], not an expression.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `**`
    Pow,
    /// `.`
    Concat,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `==`
    Eq,
    /// `!=`, `<>`
    Ne,
    /// `===`
    Identical,
    /// `!==`
    NotIdentical,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `<=>`
    Spaceship,
    /// `&&`, `and` (the keyword form differs only in precedence)
    And,
    /// `||`, `or`
    Or,
    /// `xor`
    Xor,
    /// `??`
    Coalesce,
    /// `|>` (PHP 8.5 pipe)
    Pipe,
}

impl BinOp {
    /// Stable lowercase name for dumps (`add`, `not-identical`, `coalesce`, ...).
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Mod => "mod",
            BinOp::Pow => "pow",
            BinOp::Concat => "concat",
            BinOp::BitAnd => "bit-and",
            BinOp::BitOr => "bit-or",
            BinOp::BitXor => "bit-xor",
            BinOp::Shl => "shl",
            BinOp::Shr => "shr",
            BinOp::Eq => "eq",
            BinOp::Ne => "ne",
            BinOp::Identical => "identical",
            BinOp::NotIdentical => "not-identical",
            BinOp::Lt => "lt",
            BinOp::Le => "le",
            BinOp::Gt => "gt",
            BinOp::Ge => "ge",
            BinOp::Spaceship => "spaceship",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::Xor => "xor",
            BinOp::Coalesce => "coalesce",
            BinOp::Pipe => "pipe",
        }
    }

    /// `true` if the operator has a compound-assignment form (`+=`, `.=`,
    /// `??=`, `**=`, `<<=`, ...), i.e. may appear as [`Expr::Assign::op`].
    pub fn has_compound_assign(self) -> bool {
        matches!(
            self,
            BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::Pow
                | BinOp::Concat
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
                | BinOp::Shl
                | BinOp::Shr
                | BinOp::Coalesce
        )
    }
}

/// `include`/`require` family.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IncludeKind {
    /// `include`
    Include,
    /// `include_once`
    IncludeOnce,
    /// `require`
    Require,
    /// `require_once`
    RequireOnce,
}

impl IncludeKind {
    /// The keyword spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            IncludeKind::Include => "include",
            IncludeKind::IncludeOnce => "include_once",
            IncludeKind::Require => "require",
            IncludeKind::RequireOnce => "require_once",
        }
    }
}

/// An expression. Every variant carries a span, reachable via [`Expr::span`].
#[derive(Clone, PartialEq, Debug)]
pub enum Expr {
    // ----- literals -------------------------------------------------------
    /// `null` (any case, optionally `\null`).
    Null(Span),
    /// `true` / `false` (any case).
    Bool(bool, Span),
    /// An integer literal. Literals that overflow `i64` are parsed as
    /// [`Expr::Float`], as in PHP.
    Int(i64, Span),
    /// A float literal.
    Float(f64, Span),
    /// A string literal with no interpolation, escape-decoded: single-quoted,
    /// double-quoted without variables, nowdoc, or heredoc without variables.
    Str(IdentId, Span),
    /// A double-quoted string or heredoc with interpolation.
    Interp {
        /// Literal and expression parts in source order.
        parts: Vec<InterpPart>,
        /// Span including the quotes / heredoc delimiters.
        span: Span,
    },
    /// A backtick command `` `ls $dir` ``; parts as for [`Expr::Interp`].
    ShellExec {
        /// Literal and expression parts in source order.
        parts: Vec<InterpPart>,
        /// Span including the backticks.
        span: Span,
    },

    // ----- variables and fetches -----------------------------------------
    /// `$name` (name interned without `$`; `$this` is `Var("this")`).
    Var(IdentId, Span),
    /// `$$name`, `${expr}`, `${'a' . 'b'}`: a variable named by an expression.
    VarVar {
        /// The expression yielding the variable name.
        name: Box<Expr>,
        /// Span from the first `$` to the end.
        span: Span,
    },
    /// `[ ... ]`, `array( ... )` or `list( ... )` — both a literal and a
    /// destructuring pattern, depending on position.
    Array {
        /// The elements; skipped slots have `value: None`.
        items: Vec<ArrayItem>,
        /// Which syntax was written.
        syntax: ArraySyntax,
        /// Span including the brackets / keyword.
        span: Span,
    },
    /// `base[index]` or `base[]` (append; `index: None`). Append is legal only
    /// in write contexts, see [`super::lvalue`].
    Index {
        /// The array/string/object expression.
        base: Box<Expr>,
        /// The offset; `None` for `[]`.
        index: Option<Box<Expr>>,
        /// Span from `base` to `]`.
        span: Span,
    },
    /// `obj->name` / `obj?->name`.
    Prop {
        /// The object expression.
        obj: Box<Expr>,
        /// The property name.
        name: MemberName,
        /// `?->` (whole-chain short circuit on `null`).
        nullsafe: bool,
        /// Span from `obj` to the end of `name`.
        span: Span,
    },
    /// `Class::$name`.
    StaticProp {
        /// The class part.
        class: ClassRef,
        /// The property name (`Ident` for `X::$p`, `Expr` for `X::$$p`).
        name: MemberName,
        /// Span from the class to the end of the name.
        span: Span,
    },
    /// `Class::NAME`, `Class::class`, `Class::{expr}`.
    ClassConst {
        /// The class part.
        class: ClassRef,
        /// The constant selector.
        name: ConstSel,
        /// Span from the class to the end of the selector.
        span: Span,
    },
    /// A constant fetch by name: `FOO`, `\ns\FOO`, `namespace\FOO`. `true`,
    /// `false` and `null` never appear here (they are literals).
    Const(Name),
    /// `__LINE__`, `__CLASS__`, ...
    MagicConst {
        /// Which magic constant.
        kind: MagicKind,
        /// Span of the token.
        span: Span,
    },

    // ----- calls and instantiation ---------------------------------------
    /// `callee(args)`.
    Call {
        /// The callee (a written name or an expression).
        callee: Callee,
        /// The arguments.
        args: Vec<Arg>,
        /// Span from the callee to `)`.
        span: Span,
    },
    /// `obj->name(args)` / `obj?->name(args)`.
    MethodCall {
        /// The object expression.
        obj: Box<Expr>,
        /// The method name.
        name: MemberName,
        /// The arguments.
        args: Vec<Arg>,
        /// `?->`.
        nullsafe: bool,
        /// Span from `obj` to `)`.
        span: Span,
    },
    /// `Class::name(args)`.
    StaticCall {
        /// The class part.
        class: ClassRef,
        /// The method name.
        name: MemberName,
        /// The arguments.
        args: Vec<Arg>,
        /// Span from the class to `)`.
        span: Span,
    },
    /// First-class callable syntax `target(...)`.
    Callable {
        /// What the closure will wrap.
        target: CallableTarget,
        /// Span from the target to `)`.
        span: Span,
    },
    /// `new X(args)`, `new X` (empty `args`), `new class(...) {}`.
    New {
        /// The class or anonymous class.
        class: NewTarget,
        /// Constructor arguments.
        args: Vec<Arg>,
        /// Span from `new` to the end.
        span: Span,
    },
    /// `clone expr` or PHP 8.5 `clone(expr, withProperties)`.
    Clone {
        /// The object to clone.
        expr: Box<Expr>,
        /// The `withProperties` array expression of clone-with, if written.
        with: Option<Box<Expr>>,
        /// Span from `clone` to the end.
        span: Span,
    },

    // ----- operators ------------------------------------------------------
    /// A unary operator application (incl. casts, `@`, `++`/`--`, `(void)`).
    Unary {
        /// The operator.
        op: UnOp,
        /// The operand; a write target for `++`/`--`.
        expr: Box<Expr>,
        /// Span of the whole expression.
        span: Span,
    },
    /// A binary operator application.
    Binary {
        /// The operator.
        op: BinOp,
        /// The left operand.
        lhs: Box<Expr>,
        /// The right operand.
        rhs: Box<Expr>,
        /// Span of the whole expression.
        span: Span,
    },
    /// `target = value`, `target op= value`, `target = &value`.
    Assign {
        /// The write target (an lvalue-shaped expression, possibly an
        /// [`Expr::Array`] destructuring pattern).
        target: Box<Expr>,
        /// The assigned value (for `by_ref`, a referenceable expression).
        value: Box<Expr>,
        /// `Some(op)` for compound assignment (`+=`, `.=`, `??=`, ...); see
        /// [`BinOp::has_compound_assign`].
        op: Option<BinOp>,
        /// `=&`. Never combined with `op`.
        by_ref: bool,
        /// Span of the whole assignment.
        span: Span,
    },
    /// `cond ? then : else` or the short form `cond ?: else` (`then: None`).
    Ternary {
        /// The condition.
        cond: Box<Expr>,
        /// The value if true; `None` for `?:`.
        then: Option<Box<Expr>>,
        /// The value if false.
        else_: Box<Expr>,
        /// Span of the whole expression.
        span: Span,
    },
    /// `isset($a, $b[0], ...)`.
    Isset {
        /// The variables to test (each an lvalue-shaped expression).
        vars: Vec<Expr>,
        /// Span from `isset` to `)`.
        span: Span,
    },
    /// `empty(expr)`.
    Empty {
        /// The expression to test.
        expr: Box<Expr>,
        /// Span from `empty` to `)`.
        span: Span,
    },
    /// `include`/`require`(`_once`) `path`.
    Include {
        /// Which keyword.
        kind: IncludeKind,
        /// The path expression.
        path: Box<Expr>,
        /// Span from the keyword to the end.
        span: Span,
    },
    /// `eval(code)`.
    Eval {
        /// The code string expression.
        code: Box<Expr>,
        /// Span from `eval` to `)`.
        span: Span,
    },
    /// `exit`, `exit(status)`, `die`, `die(msg)`.
    Exit {
        /// The status or message, if given.
        arg: Option<Box<Expr>>,
        /// Span from the keyword to the end.
        span: Span,
    },
    /// `print expr` (an expression that evaluates to `1`).
    Print {
        /// The printed expression.
        expr: Box<Expr>,
        /// Span from `print` to the end.
        span: Span,
    },

    // ----- functions ------------------------------------------------------
    /// An anonymous function.
    Closure(Box<Closure>),
    /// An arrow function.
    ArrowFn(Box<ArrowFn>),

    // ----- control-flow expressions ----------------------------------------
    /// `match (subject) { arms }`.
    Match {
        /// The matched value.
        subject: Box<Expr>,
        /// The arms in source order (at most one `default`).
        arms: Vec<MatchArm>,
        /// Span from `match` to `}`.
        span: Span,
    },
    /// `throw expr` (an expression since PHP 8.0).
    Throw {
        /// The thrown value.
        expr: Box<Expr>,
        /// Span from `throw` to the end.
        span: Span,
    },
    /// `yield`, `yield value`, `yield key => value`.
    Yield {
        /// The key, if `key => value` was written.
        key: Option<Box<Expr>>,
        /// The value, if any.
        value: Option<Box<Expr>>,
        /// Span from `yield` to the end.
        span: Span,
    },
    /// `yield from expr`.
    YieldFrom {
        /// The delegated iterable/generator.
        expr: Box<Expr>,
        /// Span from `yield` to the end.
        span: Span,
    },
    /// `expr instanceof Class`.
    InstanceOf {
        /// The tested value.
        expr: Box<Expr>,
        /// The class (name, `self`/`static`/`parent`, or an expression).
        class: ClassRef,
        /// Span of the whole expression.
        span: Span,
    },

    // ----- HIR-only -------------------------------------------------------
    /// HIR only: bind `init` to a temporary for the extent of `body`, whose
    /// value is the result. Produced by the desugarer's *stabilize* rule so
    /// that `$a[f()] ??= 1` evaluates `f()` once. Never produced by the parser.
    Let {
        /// The temporary being bound.
        temp: TempId,
        /// The value bound to the temporary.
        init: Box<Expr>,
        /// The expression evaluated with the binding in scope.
        body: Box<Expr>,
        /// Synthesized span (usually the desugared construct's).
        span: Span,
    },
    /// HIR only: read a temporary bound by an enclosing [`Expr::Let`]. May
    /// also appear as the base of a write target.
    Temp(TempId, Span),
    /// HIR only: evaluate in order, value of the last. An empty sequence is
    /// `null`.
    Seq(Vec<Expr>, Span),

    // ----- recovery -------------------------------------------------------
    /// A placeholder where the parser recovered from an error. The compiler
    /// must never see one (the front end refuses to compile a program with
    /// diagnostics), but the printer and visitors tolerate it.
    Error(Span),
}

impl Expr {
    /// The span of the expression.
    pub fn span(&self) -> Span {
        match self {
            Expr::Null(s)
            | Expr::Bool(_, s)
            | Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Str(_, s)
            | Expr::Var(_, s)
            | Expr::Temp(_, s)
            | Expr::Seq(_, s)
            | Expr::Error(s) => *s,
            Expr::Const(n) => n.span,
            Expr::Closure(c) => c.span,
            Expr::ArrowFn(f) => f.span,
            Expr::Interp { span, .. }
            | Expr::ShellExec { span, .. }
            | Expr::VarVar { span, .. }
            | Expr::Array { span, .. }
            | Expr::Index { span, .. }
            | Expr::Prop { span, .. }
            | Expr::StaticProp { span, .. }
            | Expr::ClassConst { span, .. }
            | Expr::MagicConst { span, .. }
            | Expr::Call { span, .. }
            | Expr::MethodCall { span, .. }
            | Expr::StaticCall { span, .. }
            | Expr::Callable { span, .. }
            | Expr::New { span, .. }
            | Expr::Clone { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Assign { span, .. }
            | Expr::Ternary { span, .. }
            | Expr::Isset { span, .. }
            | Expr::Empty { span, .. }
            | Expr::Include { span, .. }
            | Expr::Eval { span, .. }
            | Expr::Exit { span, .. }
            | Expr::Print { span, .. }
            | Expr::Match { span, .. }
            | Expr::Throw { span, .. }
            | Expr::Yield { span, .. }
            | Expr::YieldFrom { span, .. }
            | Expr::InstanceOf { span, .. }
            | Expr::Let { span, .. } => *span,
        }
    }

    /// `true` for the scalar literals `null`, bools, ints, floats and
    /// non-interpolated strings.
    pub fn is_literal(&self) -> bool {
        matches!(
            self,
            Expr::Null(_) | Expr::Bool(..) | Expr::Int(..) | Expr::Float(..) | Expr::Str(..)
        )
    }

    /// `true` for the variable-shaped fetches that can be written through:
    /// `$v`, `$$v`, `a[i]`, `a[]`, `o->p`, `o?->p`, `C::$p`, and HIR
    /// temporaries. This is shape only; use [`super::lvalue::validate`] for the
    /// context-dependent rules.
    pub fn is_variable_like(&self) -> bool {
        matches!(
            self,
            Expr::Var(..)
                | Expr::VarVar { .. }
                | Expr::Index { .. }
                | Expr::Prop { .. }
                | Expr::StaticProp { .. }
                | Expr::Temp(..)
        )
    }

    /// `true` for `Call`, `MethodCall`, `StaticCall` and `Callable`, which PHP
    /// treats as "function/method return value" in write contexts.
    pub fn is_call(&self) -> bool {
        matches!(
            self,
            Expr::Call { .. }
                | Expr::MethodCall { .. }
                | Expr::StaticCall { .. }
                | Expr::Callable { .. }
        )
    }

    /// Purely syntactic approximation of "may appear in a constant
    /// expression" (defaults, constants, attribute arguments, enum cases,
    /// static initializers): literals, constants, class constants, magic
    /// constants, arrays of such, unary/binary/ternary combinations, `new`
    /// with such arguments, and static closures (PHP 8.5). Context-specific
    /// restrictions (where `new` and closures are allowed, no `**` on
    /// non-numeric operands, no `??` on undefined, ...) are F7's job.
    pub fn is_constant_shape(&self) -> bool {
        match self {
            Expr::Null(_)
            | Expr::Bool(..)
            | Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::Const(_)
            | Expr::MagicConst { .. } => true,
            Expr::ClassConst { class, name, .. } => {
                let class_ok = match class {
                    ClassRef::Named(_)
                    | ClassRef::SelfKw(_)
                    | ClassRef::Static(_)
                    | ClassRef::Parent(_) => true,
                    ClassRef::Expr(e) => e.is_constant_shape(),
                };
                let name_ok = match name {
                    ConstSel::Ident(..) | ConstSel::Class(_) => true,
                    ConstSel::Expr(e) => e.is_constant_shape(),
                };
                class_ok && name_ok
            }
            Expr::Array { items, syntax, .. } => {
                !syntax.is_list()
                    && items.iter().all(|it| {
                        !it.by_ref
                            && it.key.as_ref().is_none_or(Expr::is_constant_shape)
                            && it.value.as_ref().is_some_and(Expr::is_constant_shape)
                    })
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => base.is_constant_shape() && index.is_constant_shape(),
            Expr::Unary { op, expr, .. } => {
                !op.is_inc_dec()
                    && !matches!(op, UnOp::Silence | UnOp::Void)
                    && expr.is_constant_shape()
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                *op != BinOp::Pipe && lhs.is_constant_shape() && rhs.is_constant_shape()
            }
            Expr::Ternary {
                cond, then, else_, ..
            } => {
                cond.is_constant_shape()
                    && then.as_deref().is_none_or(Expr::is_constant_shape)
                    && else_.is_constant_shape()
            }
            Expr::New {
                class: NewTarget::Ref(ClassRef::Named(_)),
                args,
                ..
            } => args
                .iter()
                .all(|a| !a.spread && a.value.is_constant_shape()),
            Expr::Closure(c) => c.static_ && c.uses.is_empty(),
            Expr::ArrowFn(f) => f.static_,
            Expr::Callable {
                target: CallableTarget::Func(Callee::Name(_)),
                ..
            } => true,
            _ => false,
        }
    }
}
