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
    Return { value: Option<Expr>, span: Span },
    Func(Func),
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
    Unary { op: UnOp, expr: Box<Expr>, span: Span },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `name(args...)`
    Call { name: IdentId, args: Vec<Expr>, span: Span },
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
            | Expr::Unary { span: s, .. }
            | Expr::Binary { span: s, .. }
            | Expr::Call { span: s, .. } => *s,
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
