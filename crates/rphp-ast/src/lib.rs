//! Typed AST for the M0 scalar subset.
//!
//! This is the owned AST contract (`rphp-ast`, per ADR-007): the parser
//! produces it and the compiler consumes it. The full PHP 8.5 surface (strings,
//! arrays, objects, classes, closures, namespaces …) is added incrementally;
//! the lossless CST is deferred. Every node carries a [`Span`].
#![forbid(unsafe_code)]

use rphp_intern::IdentId;
use rphp_span::Span;

#[derive(Clone, Debug)]
pub struct Program {
    pub items: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `echo e1, e2, ...;`
    Echo { args: Vec<Expr>, span: Span },
    /// A bare expression statement, e.g. `$x = 1;`
    Expr(Expr),
    If {
        cond: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Vec<Stmt>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `foreach ($subject as [$key =>] $value) { body }`
    Foreach {
        subject: Expr,
        key_var: Option<IdentId>,
        value_var: IdentId,
        body: Vec<Stmt>,
        span: Span,
    },
    Return { value: Option<Expr>, span: Span },
    Func(Func),
    Class(Class),
}

#[derive(Clone, Debug)]
pub struct Class {
    pub name: IdentId,
    pub props: Vec<PropDecl>,
    pub methods: Vec<Method>,
    pub span: Span,
}

/// A declared property `[visibility] $name [= default];`. Visibility is parsed
/// but not yet enforced (everything is effectively public).
#[derive(Clone, Debug)]
pub struct PropDecl {
    pub name: IdentId,
    pub default: Option<Expr>,
    pub span: Span,
}

/// A method `[visibility] function name(params) { body }`. The body sees an
/// implicit `$this`; `params` lists only the declared parameters.
#[derive(Clone, Debug)]
pub struct Method {
    pub name: IdentId,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: IdentId,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: IdentId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Null(Span),
    Bool(bool, Span),
    Int(i64, Span),
    Float(f64, Span),
    /// A string literal; `id` interns the final (escape-decoded) bytes.
    Str(IdentId, Span),
    /// `$name`
    Var(IdentId, Span),
    /// `$name = value`
    Assign { target: IdentId, value: Box<Expr>, span: Span },
    /// `base[index] = value`, or `base[] = value` (append) when `index` is None.
    IndexAssign {
        base: Box<Expr>,
        index: Option<Box<Expr>>,
        value: Box<Expr>,
        span: Span,
    },
    Unary { op: UnOp, expr: Box<Expr>, span: Span },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `name(args...)`
    Call { name: IdentId, args: Vec<Expr>, span: Span },
    /// `callee(args...)` where the callee is itself an expression — a closure or
    /// callable held in a variable (`$f(1, 2)`), or the result of another call.
    CallDynamic { callee: Box<Expr>, args: Vec<Expr>, span: Span },
    /// `function (params) use ($a, $b) { body }`, or an arrow `fn (params) => e`
    /// desugared to the same node (its free variables become the `uses` list and
    /// the body a single `return e;`). Captures are by value.
    Closure {
        params: Vec<Param>,
        uses: Vec<IdentId>,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `[ item, key => item, ... ]` or `array( ... )`
    Array { items: Vec<ArrayItem>, span: Span },
    /// `base[index]` read. A read with no index (`$a[]`) is invalid and rejected
    /// by the compiler; the node exists so `$a[] = v` can be recognized.
    Index { base: Box<Expr>, index: Option<Box<Expr>>, span: Span },
    /// `new Class(args...)`
    New { class: IdentId, args: Vec<Expr>, span: Span },
    /// `obj->name` property read.
    PropGet { obj: Box<Expr>, name: IdentId, span: Span },
    /// `obj->name = value` property write.
    PropSet { obj: Box<Expr>, name: IdentId, value: Box<Expr>, span: Span },
    /// `obj->method(args...)`
    MethodCall { obj: Box<Expr>, method: IdentId, args: Vec<Expr>, span: Span },
}

/// One element of an array literal: `value`, or `key => value`.
#[derive(Clone, Debug)]
pub struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Null(s)
            | Expr::Bool(_, s)
            | Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Str(_, s)
            | Expr::Var(_, s)
            | Expr::Assign { span: s, .. }
            | Expr::IndexAssign { span: s, .. }
            | Expr::Unary { span: s, .. }
            | Expr::Binary { span: s, .. }
            | Expr::Call { span: s, .. }
            | Expr::CallDynamic { span: s, .. }
            | Expr::Closure { span: s, .. }
            | Expr::Array { span: s, .. }
            | Expr::Index { span: s, .. }
            | Expr::New { span: s, .. }
            | Expr::PropGet { span: s, .. }
            | Expr::PropSet { span: s, .. }
            | Expr::MethodCall { span: s, .. } => *s,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,    // .
    Eq,        // ==
    Ne,        // !=
    Identical, // ===
    NotIdentical, // !==
    Lt,
    Le,
    Gt,
    Ge,
    Spaceship, // <=>
    And,       // &&
    Or,        // ||
}
