//! Expressions: `mago_syntax::cst::Expression` → [`Expr`].
//!
//! Conversion is position-aware only where PHP's grammar is: array
//! literals become destructuring patterns in write position
//! ([`Pos::Write`]), where skipped slots and `&` elements are legal.

use mago_span::HasSpan;
use mago_syntax::cst::{
    Access, Argument, ArgumentList, ArrayElement, Assignment, AssignmentOperator, Binary,
    BinaryOperator, Call, Clone as MagoClone, Conditional, Construct, Expression, Identifier,
    Instantiation, Literal, MagicConstant, Match, MatchArm as MagoMatchArm, PartialApplication,
    PartialArgument, PartialArgumentList, Pipe, TokenSeparatedSequence, UnaryPostfix,
    UnaryPostfixOperator, UnaryPrefix, UnaryPrefixOperator, Variable, Yield,
};
use rphp_ast::v2::lvalue::LvalueCtx;
use rphp_ast::v2::{
    Arg, ArrayItem, ArraySyntax, BinOp, CallableTarget, Callee, CastKind, ClassRef, Expr,
    IncludeKind, MagicKind, MatchArm, Name, NameKind, NewTarget, UnOp,
};
use rphp_span::Span;

use super::conformance::{
    check_args_order, check_paren_target, describe_unexpected, is_class_name_reference,
    is_clone_callee, unexpected_token, validate_rvalue, validate_target,
};
use super::ctx::{eq_ci, Ctx, FnKind};
use super::names::{self, LiteralKw};
use super::{class, func, literals, strings};

/// Whether an expression is being converted as a value or as a write target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pos {
    /// An rvalue.
    Read,
    /// A write target: array literals are destructuring patterns.
    Write,
    /// A call argument: may be passed by reference, so `$a[]` is legal
    /// (PHP defers the check to runtime), but arrays are literals.
    Arg,
}

/// Convert an expression in rvalue position.
pub(crate) fn expr(ctx: &mut Ctx<'_, '_>, e: &Expression<'_>) -> Expr {
    expr_in(ctx, e, Pos::Read)
}

/// Convert an expression in a constant-expression position (`self` /
/// `parent` are resolved at runtime there, not scope-checked).
pub(crate) fn const_expr(ctx: &mut Ctx<'_, '_>, e: &Expression<'_>) -> Expr {
    ctx.const_depth += 1;
    let out = expr_in(ctx, e, Pos::Read);
    ctx.const_depth -= 1;
    out
}

/// Convert a statement-level expression: the only position (besides the
/// `for` init/step lists) where `(void)` is legal.
pub(crate) fn stmt_expr(ctx: &mut Ctx<'_, '_>, e: &Expression<'_>) -> Expr {
    ctx.void_cast_ok = matches!(
        e,
        Expression::UnaryPrefix(UnaryPrefix {
            operator: UnaryPrefixOperator::VoidCast(..),
            ..
        })
    );
    let out = expr(ctx, e);
    ctx.void_cast_ok = false;
    out
}

/// Whether a literal is falsy / truthy at compile time (Zend folds
/// `lit && rhs` / `lit || rhs` and never compiles the dead side).
fn literal_truth(e: &Expression<'_>) -> Option<bool> {
    let mut e = e;
    while let Expression::Parenthesized(p) = e {
        e = p.expression;
    }
    match e {
        Expression::Literal(Literal::True(_)) => Some(true),
        Expression::Literal(Literal::False(_)) | Expression::Literal(Literal::Null(_)) => {
            Some(false)
        }
        Expression::Literal(Literal::Integer(i)) => match literals::parse_int(i.raw) {
            literals::IntValue::Int(v) => Some(v != 0),
            literals::IntValue::Float(f) => Some(f != 0.0),
            literals::IntValue::Invalid => None,
        },
        Expression::Literal(Literal::Float(f)) => Some(f.value.0 != 0.0),
        Expression::Literal(Literal::String(s)) => {
            s.value.map(|v| !(v.is_empty() || v == b"0"))
        }
        Expression::Array(a) => Some(!a.elements.is_empty()),
        Expression::LegacyArray(a) => Some(!a.elements.is_empty()),
        _ => None,
    }
}

/// Convert an expression in the given position.
pub(crate) fn expr_in(ctx: &mut Ctx<'_, '_>, e: &Expression<'_>, pos: Pos) -> Expr {
    let span = ctx.sp(e.span());
    match e {
        Expression::Binary(b) => binary(ctx, b, span),
        Expression::UnaryPrefix(u) => unary_prefix(ctx, u, span, pos),
        Expression::UnaryPostfix(u) => unary_postfix(ctx, u, span),
        Expression::Parenthesized(p) => expr_in(ctx, p.expression, pos),
        Expression::Literal(l) => literal(ctx, l),
        Expression::CompositeString(s) => strings::composite(ctx, s),
        Expression::Assignment(a) => assignment(ctx, a, span),
        Expression::Conditional(c) => conditional(ctx, c, span),
        Expression::Array(a) => array(ctx, &a.elements, ArraySyntax::Short, span, pos),
        Expression::LegacyArray(a) => array(ctx, &a.elements, ArraySyntax::Long, span, pos),
        Expression::List(l) => array(ctx, &l.elements, ArraySyntax::List, span, pos),
        Expression::Access(Access::Property(p)) => {
            // `$a[]->b = 1` is a write chain too.
            let obj = expr_in(ctx, p.object, pos);
            let name = names::member(ctx, &p.property);
            Expr::Prop {
                obj: Box::new(obj),
                name,
                nullsafe: false,
                span,
            }
        }
        Expression::ArrayAccess(a) => {
            // Write chains propagate: `$a[][0] = 1` appends, then indexes.
            let base = expr_in(ctx, a.array, pos);
            let index = index_expr(ctx, a.index);
            Expr::Index {
                base: Box::new(base),
                index: Some(Box::new(index)),
                span,
            }
        }
        Expression::ArrayAppend(a) => {
            let base = expr_in(ctx, a.array, pos);
            let node = Expr::Index {
                base: Box::new(base),
                index: None,
                span,
            };
            if pos == Pos::Read {
                validate_rvalue(ctx, &node);
            }
            node
        }
        Expression::AnonymousClass(a) => {
            let cls = class::anonymous(ctx, a);
            let args = match &a.argument_list {
                Some(al) => partial_args(ctx, al, "Cannot create Closure for new expression"),
                None => Vec::new(),
            };
            Expr::New {
                class: NewTarget::Anon(Box::new(cls)),
                args,
                span,
            }
        }
        Expression::Closure(c) => func::closure(ctx, c),
        Expression::ArrowFunction(f) => func::arrow_fn(ctx, f),
        Expression::Variable(v) => variable(ctx, v),
        Expression::ConstantAccess(c) => constant(ctx, &c.name),
        Expression::Identifier(id) => constant(ctx, id),
        Expression::Match(m) => match_expr(ctx, m, span),
        Expression::Yield(y) => yield_expr(ctx, y, span),
        Expression::Construct(c) => construct(ctx, c, span),
        Expression::Throw(t) => Expr::Throw {
            expr: Box::new(expr(ctx, t.exception)),
            span,
        },
        Expression::Clone(c) => clone_kw(ctx, c, span),
        Expression::Call(c) => call(ctx, c, span),
        Expression::PartialApplication(p) => partial_application(ctx, p, span),
        Expression::Access(a) => access(ctx, a, span),
        Expression::Parent(k) => keyword_const(ctx, k.value, span),
        Expression::Self_(k) => keyword_const(ctx, k.value, span),
        Expression::Static(_) => {
            ctx.reject(unexpected_token("static"), span);
            Expr::Error(span)
        }
        Expression::Instantiation(i) => new_expr(ctx, i, span),
        Expression::MagicConstant(m) => magic(ctx, m, span),
        Expression::Pipe(p) => pipe(ctx, p, span),
        Expression::Error(_) => Expr::Error(span),
        other => {
            ctx.unsupported(other.node_kind(), span);
            Expr::Error(span)
        }
    }
}

/// `self` / `parent` used as a value: a constant fetch, like PHP.
fn keyword_const(ctx: &mut Ctx<'_, '_>, text: &[u8], span: Span) -> Expr {
    let id = ctx.intern(text);
    Expr::Const(Name::new(id, NameKind::Unqualified, span))
}

/// A literal.
fn literal(ctx: &mut Ctx<'_, '_>, l: &Literal<'_>) -> Expr {
    match l {
        Literal::String(s) => strings::literal(ctx, s),
        Literal::Integer(i) => literals::int(ctx, i),
        Literal::Float(f) => literals::float(ctx, f),
        Literal::True(k) => Expr::Bool(true, ctx.sp(k.span)),
        Literal::False(k) => Expr::Bool(false, ctx.sp(k.span)),
        Literal::Null(k) => Expr::Null(ctx.sp(k.span)),
    }
}

/// A bare name in value position: `FOO`, `\ns\FOO`, `namespace\FOO`; the
/// fully qualified literal keywords (`\true`, `\null`) stay literals.
fn constant(ctx: &mut Ctx<'_, '_>, id: &Identifier<'_>) -> Expr {
    let (kind, text) = names::name_parts(id);
    let span = ctx.sp(id.span());
    if kind != NameKind::Relative {
        if let Some(kw) = names::literal_keyword(text) {
            return match kw {
                LiteralKw::True => Expr::Bool(true, span),
                LiteralKw::False => Expr::Bool(false, span),
                LiteralKw::Null => Expr::Null(span),
            };
        }
    }
    Expr::Const(names::name(ctx, id))
}

/// An array index. Inside string interpolation mago represents `"$a[key]"`
/// with a bare identifier index, which PHP treats as the string `'key'`;
/// that is the only way an identifier reaches this position.
fn index_expr(ctx: &mut Ctx<'_, '_>, index: &Expression<'_>) -> Expr {
    if let Expression::Identifier(Identifier::Local(l)) = index {
        let id = ctx.intern(l.value);
        return Expr::Str(id, ctx.sp(l.span));
    }
    expr(ctx, index)
}

/// `$x`, `$$x`, `${expr}` — and the in-string `${name}` / `${name[i]}`.
pub(crate) fn variable(ctx: &mut Ctx<'_, '_>, v: &Variable<'_>) -> Expr {
    let span = ctx.sp(v.span());
    match v {
        Variable::Direct(d) => {
            let id = ctx.intern_var(d.name);
            Expr::Var(id, span)
        }
        Variable::Indirect(i) => match i.expression {
            Expression::Identifier(Identifier::Local(l)) => {
                let id = ctx.intern(l.value);
                Expr::Var(id, span)
            }
            Expression::ArrayAccess(a)
                if matches!(a.array, Expression::Identifier(Identifier::Local(_))) =>
            {
                let name = match a.array {
                    Expression::Identifier(Identifier::Local(l)) => ctx.intern(l.value),
                    _ => unreachable!(),
                };
                let base = Expr::Var(name, ctx.sp(a.array.span()));
                let index = index_expr(ctx, a.index);
                Expr::Index {
                    base: Box::new(base),
                    index: Some(Box::new(index)),
                    span,
                }
            }
            other => Expr::VarVar {
                name: Box::new(expr(ctx, other)),
                span,
            },
        },
        Variable::Nested(n) => Expr::VarVar {
            name: Box::new(variable(ctx, n.variable)),
            span,
        },
    }
}

fn magic(ctx: &mut Ctx<'_, '_>, m: &MagicConstant<'_>, span: Span) -> Expr {
    let _ = ctx;
    let kind = match m {
        MagicConstant::Line(_) => MagicKind::Line,
        MagicConstant::File(_) => MagicKind::File,
        MagicConstant::Directory(_) => MagicKind::Dir,
        MagicConstant::Trait(_) => MagicKind::Trait,
        MagicConstant::Method(_) => MagicKind::Method,
        MagicConstant::Function(_) => MagicKind::Function,
        MagicConstant::Property(_) => MagicKind::Property,
        MagicConstant::Namespace(_) => MagicKind::Namespace,
        MagicConstant::Class(_) => MagicKind::Class,
    };
    Expr::MagicConst { kind, span }
}

/// A binary expression. Left-associative chains (`$a . $b . $c ...`, long
/// `+` sums in generated code) are thousands of nodes deep on the left, so
/// the left spine is collected iteratively and folded from the innermost
/// operand instead of recursing once per operator.
fn binary(ctx: &mut Ctx<'_, '_>, b: &Binary<'_>, span: Span) -> Expr {
    let mut spine: Vec<(&Binary<'_>, Span)> = vec![(b, span)];
    let mut cur = b;
    while let Expression::Binary(inner) = cur.lhs {
        spine.push((inner, ctx.sp(inner.span())));
        cur = inner;
    }
    let mut acc = expr(ctx, cur.lhs);
    for (node, sp) in spine.into_iter().rev() {
        acc = binary_with_lhs(ctx, node, acc, sp);
    }
    acc
}

/// One binary node whose left operand is already converted.
fn binary_with_lhs(ctx: &mut Ctx<'_, '_>, b: &Binary<'_>, lhs: Expr, span: Span) -> Expr {
    let op = match &b.operator {
        BinaryOperator::Addition(_) => BinOp::Add,
        BinaryOperator::Subtraction(_) => BinOp::Sub,
        BinaryOperator::Multiplication(_) => BinOp::Mul,
        BinaryOperator::Division(_) => BinOp::Div,
        BinaryOperator::Modulo(_) => BinOp::Mod,
        BinaryOperator::Exponentiation(_) => BinOp::Pow,
        BinaryOperator::BitwiseAnd(_) => BinOp::BitAnd,
        BinaryOperator::BitwiseOr(_) => BinOp::BitOr,
        BinaryOperator::BitwiseXor(_) => BinOp::BitXor,
        BinaryOperator::LeftShift(_) => BinOp::Shl,
        BinaryOperator::RightShift(_) => BinOp::Shr,
        BinaryOperator::NullCoalesce(_) => BinOp::Coalesce,
        BinaryOperator::Equal(_) => BinOp::Eq,
        BinaryOperator::NotEqual(_) | BinaryOperator::AngledNotEqual(_) => BinOp::Ne,
        BinaryOperator::Identical(_) => BinOp::Identical,
        BinaryOperator::NotIdentical(_) => BinOp::NotIdentical,
        BinaryOperator::LessThan(_) => BinOp::Lt,
        BinaryOperator::LessThanOrEqual(_) => BinOp::Le,
        BinaryOperator::GreaterThan(_) => BinOp::Gt,
        BinaryOperator::GreaterThanOrEqual(_) => BinOp::Ge,
        BinaryOperator::Spaceship(_) => BinOp::Spaceship,
        BinaryOperator::StringConcat(dot) => {
            check_float_concat(ctx, b.lhs, *dot, b.rhs);
            BinOp::Concat
        }
        BinaryOperator::And(_) | BinaryOperator::LowAnd(_) => {
            if literal_truth(b.lhs) == Some(false) {
                return dead_rhs(ctx, BinOp::And, b, lhs, span);
            }
            BinOp::And
        }
        BinaryOperator::Or(_) | BinaryOperator::LowOr(_) => {
            if literal_truth(b.lhs) == Some(true) {
                return dead_rhs(ctx, BinOp::Or, b, lhs, span);
            }
            BinOp::Or
        }
        BinaryOperator::LowXor(_) => BinOp::Xor,
        BinaryOperator::Instanceof(_) => {
            if !is_class_name_reference(b.rhs) {
                let msg = describe_unexpected(b.rhs);
                let s = ctx.sp(b.rhs.span());
                ctx.reject(msg, s);
            }
            let class = names::class_ref(ctx, b.rhs);
            check_class_scope(ctx, &class);
            return Expr::InstanceOf {
                expr: Box::new(lhs),
                class,
                span,
            };
        }
    };
    let rhs = expr(ctx, b.rhs);
    Expr::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
        span,
    }
}

/// `false && rhs` / `true || rhs`: PHP folds the operator at compile time
/// and never compiles `rhs`, so its compile-time rejections do not apply.
fn dead_rhs(ctx: &mut Ctx<'_, '_>, op: BinOp, b: &Binary<'_>, lhs: Expr, span: Span) -> Expr {
    let mark = ctx.diags.len();
    let rhs = expr(ctx, b.rhs);
    ctx.drop_conformance_since(mark);
    Expr::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
        span,
    }
}

/// `100._0`: PHP's lexer takes `100.` as a float literal, so an integer
/// literal immediately followed by `.` and an identifier is a parse error
/// where mago sees a concatenation.
fn check_float_concat(ctx: &mut Ctx<'_, '_>, lhs: &Expression<'_>, dot: mago_span::Span, rhs: &Expression<'_>) {
    let Expression::Literal(Literal::Integer(i)) = lhs else { return };
    if i.span.end.offset != dot.start.offset || dot.end.offset != rhs.span().start.offset {
        return;
    }
    let raw = i.raw;
    if raw.len() > 1 && raw[0] == b'0' && matches!(raw[1], b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
        return;
    }
    if matches!(rhs, Expression::ConstantAccess(_) | Expression::Identifier(_) | Expression::Call(_)) {
        let s = ctx.sp(rhs.span());
        ctx.reject("syntax error, unexpected identifier", s);
    }
}

/// `self`/`static`/`parent` in a class-reference position need a class
/// scope when the scope is known at compile time.
pub(crate) fn check_class_scope(ctx: &mut Ctx<'_, '_>, class: &ClassRef) {
    let b = match class {
        ClassRef::SelfKw(_) => rphp_ast::v2::Builtin::SelfTy,
        ClassRef::Static(_) => rphp_ast::v2::Builtin::StaticTy,
        ClassRef::Parent(_) => rphp_ast::v2::Builtin::ParentTy,
        _ => return,
    };
    super::types::check_scope(ctx, b, class.span());
}

fn cast(ctx: &mut Ctx<'_, '_>, kind: CastKind, u: &UnaryPrefix<'_>, span: Span) -> Expr {
    Expr::Unary {
        op: UnOp::Cast(kind),
        expr: Box::new(expr(ctx, u.operand)),
        span,
    }
}

fn unary(ctx: &mut Ctx<'_, '_>, op: UnOp, u: &UnaryPrefix<'_>, span: Span) -> Expr {
    Expr::Unary {
        op,
        expr: Box::new(expr(ctx, u.operand)),
        span,
    }
}

fn unary_prefix(ctx: &mut Ctx<'_, '_>, u: &UnaryPrefix<'_>, span: Span, pos: Pos) -> Expr {
    match &u.operator {
        UnaryPrefixOperator::ErrorControl(_) => unary(ctx, UnOp::Silence, u, span),
        UnaryPrefixOperator::Reference(r) => {
            // `&` is only legal in the positions that handle it themselves
            // (assignment value, array element, foreach target, closure use).
            let s = ctx.sp(*r);
            ctx.reject(unexpected_token("&"), s);
            expr_in(ctx, u.operand, pos)
        }
        UnaryPrefixOperator::ArrayCast(..) => cast(ctx, CastKind::Array, u, span),
        UnaryPrefixOperator::BoolCast(..) | UnaryPrefixOperator::BooleanCast(..) => {
            cast(ctx, CastKind::Bool, u, span)
        }
        UnaryPrefixOperator::DoubleCast(..) | UnaryPrefixOperator::FloatCast(..) => {
            cast(ctx, CastKind::Float, u, span)
        }
        UnaryPrefixOperator::RealCast(s, _) => {
            let s = ctx.sp(*s);
            ctx.reject("The (real) cast has been removed, use (float) instead", s);
            cast(ctx, CastKind::Float, u, span)
        }
        UnaryPrefixOperator::IntCast(..) | UnaryPrefixOperator::IntegerCast(..) => {
            cast(ctx, CastKind::Int, u, span)
        }
        UnaryPrefixOperator::ObjectCast(..) => cast(ctx, CastKind::Object, u, span),
        UnaryPrefixOperator::UnsetCast(s, _) => {
            let s = ctx.sp(*s);
            ctx.reject("The (unset) cast is no longer supported", s);
            let _ = expr(ctx, u.operand);
            Expr::Error(span)
        }
        UnaryPrefixOperator::StringCast(..) | UnaryPrefixOperator::BinaryCast(..) => {
            cast(ctx, CastKind::String, u, span)
        }
        UnaryPrefixOperator::VoidCast(s, _) => {
            if !std::mem::take(&mut ctx.void_cast_ok) {
                let s = ctx.sp(*s);
                ctx.reject(unexpected_token("(void)"), s);
            }
            unary(ctx, UnOp::Void, u, span)
        }
        UnaryPrefixOperator::BitwiseNot(_) => unary(ctx, UnOp::BitNot, u, span),
        UnaryPrefixOperator::Not(_) => unary(ctx, UnOp::Not, u, span),
        UnaryPrefixOperator::PreIncrement(_) => inc_dec(ctx, UnOp::PreInc, u.operand, span, "++"),
        UnaryPrefixOperator::PreDecrement(_) => inc_dec(ctx, UnOp::PreDec, u.operand, span, "--"),
        UnaryPrefixOperator::Plus(_) => unary(ctx, UnOp::Plus, u, span),
        UnaryPrefixOperator::Negation(_) => unary(ctx, UnOp::Neg, u, span),
    }
}

fn inc_dec(ctx: &mut Ctx<'_, '_>, op: UnOp, operand: &Expression<'_>, span: Span, tok: &str) -> Expr {
    check_paren_target(ctx, operand, tok);
    let target = expr_in(ctx, operand, Pos::Write);
    validate_target(ctx, &target, LvalueCtx::CompoundAssign);
    Expr::Unary {
        op,
        expr: Box::new(target),
        span,
    }
}

fn unary_postfix(ctx: &mut Ctx<'_, '_>, u: &UnaryPostfix<'_>, span: Span) -> Expr {
    match &u.operator {
        UnaryPostfixOperator::PostIncrement(_) => inc_dec(ctx, UnOp::PostInc, u.operand, span, "++"),
        UnaryPostfixOperator::PostDecrement(_) => inc_dec(ctx, UnOp::PostDec, u.operand, span, "--"),
    }
}

fn assignment(ctx: &mut Ctx<'_, '_>, a: &Assignment<'_>, span: Span) -> Expr {
    let (op, tok) = match &a.operator {
        AssignmentOperator::Assign(_) => (None, "="),
        AssignmentOperator::Addition(_) => (Some(BinOp::Add), "+="),
        AssignmentOperator::Subtraction(_) => (Some(BinOp::Sub), "-="),
        AssignmentOperator::Multiplication(_) => (Some(BinOp::Mul), "*="),
        AssignmentOperator::Division(_) => (Some(BinOp::Div), "/="),
        AssignmentOperator::Modulo(_) => (Some(BinOp::Mod), "%="),
        AssignmentOperator::Exponentiation(_) => (Some(BinOp::Pow), "**="),
        AssignmentOperator::Concat(_) => (Some(BinOp::Concat), ".="),
        AssignmentOperator::BitwiseAnd(_) => (Some(BinOp::BitAnd), "&="),
        AssignmentOperator::BitwiseOr(_) => (Some(BinOp::BitOr), "|="),
        AssignmentOperator::BitwiseXor(_) => (Some(BinOp::BitXor), "^="),
        AssignmentOperator::LeftShift(_) => (Some(BinOp::Shl), "<<="),
        AssignmentOperator::RightShift(_) => (Some(BinOp::Shr), ">>="),
        AssignmentOperator::Coalesce(_) => (Some(BinOp::Coalesce), "??="),
    };
    check_paren_target(ctx, a.lhs, tok);
    // `$a = &expr`
    if op.is_none() {
        if let Expression::UnaryPrefix(UnaryPrefix {
            operator: UnaryPrefixOperator::Reference(_),
            operand,
        }) = a.rhs
        {
            let target = expr_in(ctx, a.lhs, Pos::Write);
            validate_target(ctx, &target, LvalueCtx::AssignRef);
            // The referenced value may be a new element (`=& $a[]`).
            let value = expr_in(ctx, operand, Pos::Write);
            validate_target(ctx, &value, LvalueCtx::ByRefArg);
            return Expr::Assign {
                target: Box::new(target),
                value: Box::new(value),
                op: None,
                by_ref: true,
                span,
            };
        }
    }
    let target = expr_in(ctx, a.lhs, Pos::Write);
    let lctx = match op {
        None => LvalueCtx::Assign,
        Some(BinOp::Coalesce) => LvalueCtx::CoalesceAssign,
        Some(_) => LvalueCtx::CompoundAssign,
    };
    validate_target(ctx, &target, lctx);
    let value = expr(ctx, a.rhs);
    Expr::Assign {
        target: Box::new(target),
        value: Box::new(value),
        op,
        by_ref: false,
        span,
    }
}

fn conditional(ctx: &mut Ctx<'_, '_>, c: &Conditional<'_>, span: Span) -> Expr {
    let cond = expr(ctx, c.condition);
    let then = c.then.map(|t| Box::new(expr(ctx, t)));
    let else_ = expr(ctx, c.r#else);
    Expr::Ternary {
        cond: Box::new(cond),
        then,
        else_: Box::new(else_),
        span,
    }
}

/// An array literal or destructuring pattern.
fn array(
    ctx: &mut Ctx<'_, '_>,
    elements: &TokenSeparatedSequence<'_, ArrayElement<'_>>,
    syntax: ArraySyntax,
    span: Span,
    pos: Pos,
) -> Expr {
    // Only a write target is a pattern; an argument is a literal.
    let pos = if pos == Pos::Arg { Pos::Read } else { pos };
    let mut items = Vec::with_capacity(elements.len());
    for el in elements.iter() {
        let item_span = ctx.sp(el.span());
        let item = match el {
            ArrayElement::KeyValue(kv) => {
                let key = expr(ctx, kv.key);
                let (value, by_ref) = item_value(ctx, kv.value, pos);
                ArrayItem {
                    key: Some(key),
                    value: Some(value),
                    by_ref,
                    spread: false,
                    span: item_span,
                }
            }
            ArrayElement::Value(v) => {
                let (value, by_ref) = item_value(ctx, v.value, pos);
                ArrayItem {
                    key: None,
                    value: Some(value),
                    by_ref,
                    spread: false,
                    span: item_span,
                }
            }
            ArrayElement::Variadic(v) => ArrayItem {
                key: None,
                value: Some(expr(ctx, v.value)),
                by_ref: false,
                spread: true,
                span: item_span,
            },
            ArrayElement::Missing(_) => {
                if pos == Pos::Read {
                    ctx.reject("Cannot use empty array elements in arrays", item_span);
                }
                ArrayItem {
                    key: None,
                    value: None,
                    by_ref: false,
                    spread: false,
                    span: item_span,
                }
            }
        };
        items.push(item);
    }
    let node = Expr::Array {
        items,
        syntax,
        span,
    };
    if pos == Pos::Read {
        validate_rvalue(ctx, &node);
    }
    node
}

/// An element value, unwrapping a leading `&`.
fn item_value(ctx: &mut Ctx<'_, '_>, v: &Expression<'_>, pos: Pos) -> (Expr, bool) {
    if let Expression::UnaryPrefix(UnaryPrefix {
        operator: UnaryPrefixOperator::Reference(_),
        operand,
    }) = v
    {
        // In a pattern the operand may be a new element (`[&$a[]] = ...`);
        // in a literal PHP treats `&$a[]` as a read ("Cannot use [] for reading").
        let value = expr_in(ctx, operand, pos);
        // `[&$this]` is legal: only explicit rebinding of `$this` is not.
        let is_this = matches!(&value, Expr::Var(id, _) if Some(*id) == ctx.sv.this);
        if pos == Pos::Read && !is_this {
            validate_target(ctx, &value, LvalueCtx::AssignRef);
        }
        return (value, true);
    }
    (expr_in(ctx, v, pos), false)
}

/// An argument list.
pub(crate) fn args(ctx: &mut Ctx<'_, '_>, list: &ArgumentList<'_>) -> Vec<Arg> {
    let out: Vec<Arg> = list.arguments.iter().map(|a| arg(ctx, a)).collect();
    check_args_order(ctx, &out);
    out
}

fn arg(ctx: &mut Ctx<'_, '_>, a: &Argument<'_>) -> Arg {
    let span = ctx.sp(a.span());
    match a {
        Argument::Positional(p) => Arg {
            name: None,
            value: expr_in(ctx, p.value, Pos::Arg),
            spread: p.ellipsis.is_some(),
            span,
        },
        Argument::Named(n) => {
            let name = names::local_recorded(ctx, &n.name);
            Arg {
                name: Some(name),
                value: expr_in(ctx, n.value, Pos::Arg),
                spread: false,
                span,
            }
        }
    }
}

/// An argument list that mago parsed with placeholder support (anonymous
/// class constructor arguments, attribute arguments). Placeholders are
/// partial function application, which PHP 8.5 does not have; a lone `...`
/// is first-class-callable syntax, which PHP parses here and rejects with
/// `fcc_msg`.
pub(crate) fn partial_args(ctx: &mut Ctx<'_, '_>, list: &PartialArgumentList<'_>, fcc_msg: &str) -> Vec<Arg> {
    let mut out = Vec::with_capacity(list.arguments.len());
    for a in list.arguments.iter() {
        let span = ctx.sp(a.span());
        match a {
            PartialArgument::Positional(p) => out.push(Arg {
                name: None,
                value: expr_in(ctx, p.value, Pos::Arg),
                spread: p.ellipsis.is_some(),
                span,
            }),
            PartialArgument::Named(n) => {
                let name = names::local_recorded(ctx, &n.name);
                out.push(Arg {
                    name: Some(name),
                    value: expr_in(ctx, n.value, Pos::Arg),
                    spread: false,
                    span,
                });
            }
            PartialArgument::NamedPlaceholder(_) | PartialArgument::Placeholder(_) => {
                ctx.reject(unexpected_token("?"), span);
            }
            PartialArgument::VariadicPlaceholder(_) => {
                ctx.reject(fcc_msg, span);
            }
        }
    }
    check_args_order(ctx, &out);
    out
}

/// The callee of a plain call.
fn callee(ctx: &mut Ctx<'_, '_>, f: &Expression<'_>) -> Callee {
    match f {
        Expression::Identifier(id) => Callee::Name(names::name(ctx, id)),
        Expression::Parenthesized(p) => Callee::Expr(Box::new(expr(ctx, p.expression))),
        Expression::Static(k) | Expression::Self_(k) | Expression::Parent(k) => {
            let s = ctx.sp(k.span);
            ctx.reject(unexpected_token("("), s);
            Callee::Expr(Box::new(Expr::Error(s)))
        }
        other => Callee::Expr(Box::new(expr(ctx, other))),
    }
}

fn call(ctx: &mut Ctx<'_, '_>, c: &Call<'_>, span: Span) -> Expr {
    match c {
        Call::Function(f) => {
            if is_clone_callee(f.function) {
                if let Some(e) = clone_call(ctx, &f.argument_list, span) {
                    return e;
                }
            }
            let callee = callee(ctx, f.function);
            let args = args(ctx, &f.argument_list);
            Expr::Call { callee, args, span }
        }
        Call::Method(m) => {
            let obj = expr(ctx, m.object);
            let name = names::member(ctx, &m.method);
            let args = args(ctx, &m.argument_list);
            Expr::MethodCall {
                obj: Box::new(obj),
                name,
                args,
                nullsafe: false,
                span,
            }
        }
        Call::NullSafeMethod(m) => {
            let obj = expr(ctx, m.object);
            let name = names::member(ctx, &m.method);
            let args = args(ctx, &m.argument_list);
            Expr::MethodCall {
                obj: Box::new(obj),
                name,
                args,
                nullsafe: true,
                span,
            }
        }
        Call::StaticMethod(s) => {
            let class = match parent_hook_call(ctx, s.class, &s.method) {
                Some(class) => class,
                None => static_class(ctx, s.class),
            };
            let name = names::member(ctx, &s.method);
            let args = args(ctx, &s.argument_list);
            Expr::StaticCall {
                class,
                name,
                args,
                span,
            }
        }
    }
}

/// `parent::$prop::get()` / `parent::$prop::set(...)`: legal only inside
/// the same hook of the same property (PHP checks the class's parent at
/// runtime there). Returns the converted class part when the shape matches.
fn parent_hook_call(
    ctx: &mut Ctx<'_, '_>,
    class: &Expression<'_>,
    method: &mago_syntax::cst::ClassLikeMemberSelector<'_>,
) -> Option<ClassRef> {
    use mago_syntax::cst::ClassLikeMemberSelector;
    let Expression::Access(Access::StaticProperty(sp)) = class else { return None };
    if !matches!(sp.class, Expression::Parent(_)) {
        return None;
    }
    let Variable::Direct(prop) = &sp.property else { return None };
    let ClassLikeMemberSelector::Identifier(m) = method else { return None };
    let hook_kind = if eq_ci(m.value, "get") {
        rphp_ast::v2::HookKind::Get
    } else if eq_ci(m.value, "set") {
        rphp_ast::v2::HookKind::Set
    } else {
        return None;
    };
    let prop_name = prop.name.strip_prefix(b"$").unwrap_or(prop.name);
    let shown = format!(
        "parent::${}::{}()",
        String::from_utf8_lossy(prop_name),
        hook_kind.as_str()
    );
    let span = ctx.sp(class.span());
    match ctx.fn_scope().hook.clone() {
        None if ctx.classes.is_empty() => {
            ctx.reject("Cannot use \"parent\" when no class scope is active", span)
        }
        None => ctx.reject(format!("Must not use {shown} outside a property hook"), span),
        Some((name, _)) if name != prop_name => ctx.reject(
            format!(
                "Must not use {shown} in a different property (${})",
                String::from_utf8_lossy(&name)
            ),
            span,
        ),
        Some((_, kind)) if kind != hook_kind => ctx.reject(
            format!(
                "Must not use {shown} in a different property hook ({})",
                kind.as_str()
            ),
            span,
        ),
        Some(_) => {}
    }
    let name = names::static_prop(ctx, &sp.property);
    Some(ClassRef::Expr(Box::new(Expr::StaticProp {
        class: ClassRef::Parent(ctx.sp(sp.class.span())),
        name,
        span,
    })))
}

/// The class part of a `::` access. Any dereferencable expression is legal
/// except an array literal (`[1]::m()` — "Illegal class name").
fn static_class(ctx: &mut Ctx<'_, '_>, class: &Expression<'_>) -> ClassRef {
    if matches!(class, Expression::Array(_) | Expression::LegacyArray(_)) {
        let s = ctx.sp(class.span());
        ctx.reject("Illegal class name", s);
    }
    let r = names::class_ref(ctx, class);
    check_class_scope(ctx, &r);
    r
}

/// PHP 8.5's `clone($object [, $withProperties])` function form, when the
/// argument shape matches; `None` keeps it a plain call.
fn clone_call(ctx: &mut Ctx<'_, '_>, list: &ArgumentList<'_>, span: Span) -> Option<Expr> {
    let items: Vec<&Argument<'_>> = list.arguments.iter().collect();
    fn positional<'a>(a: &Argument<'a>) -> Option<&'a Expression<'a>> {
        match a {
            Argument::Positional(p) if p.ellipsis.is_none() => Some(p.value),
            _ => None,
        }
    }
    fn named<'a>(a: &Argument<'a>, n: &str) -> Option<&'a Expression<'a>> {
        match a {
            Argument::Named(na) if eq_ci(na.name.value, n) => Some(na.value),
            _ => None,
        }
    }
    let object = match items.as_slice() {
        [a] => positional(a).or_else(|| named(a, "object"))?,
        // With `$withProperties` php makes a real call to `clone()` (its
        // frame shows in stack traces and the writes are judged from the
        // caller's scope), so that form stays a plain call to the native.
        _ => return None,
    };
    let object = expr(ctx, object);
    Some(Expr::Clone {
        expr: Box::new(object),
        with: None,
        span,
    })
}

fn clone_kw(ctx: &mut Ctx<'_, '_>, c: &MagoClone<'_>, span: Span) -> Expr {
    Expr::Clone {
        expr: Box::new(expr(ctx, c.object)),
        with: None,
        span,
    }
}

/// `f(...)` first-class callable syntax; every other placeholder form is
/// partial function application, which PHP 8.5 rejects.
fn partial_application(ctx: &mut Ctx<'_, '_>, p: &PartialApplication<'_>, span: Span) -> Expr {
    let list = match p {
        PartialApplication::Function(f) => &f.argument_list,
        PartialApplication::Method(m) => &m.argument_list,
        PartialApplication::StaticMethod(s) => &s.argument_list,
    };
    let is_fcc = list.arguments.len() == 1
        && matches!(
            list.arguments.first(),
            Some(PartialArgument::VariadicPlaceholder(_))
        );
    if is_fcc {
        let target = match p {
            PartialApplication::Function(f) => CallableTarget::Func(callee(ctx, f.function)),
            PartialApplication::Method(m) => CallableTarget::Method {
                obj: Box::new(expr(ctx, m.object)),
                name: names::member(ctx, &m.method),
            },
            PartialApplication::StaticMethod(s) => CallableTarget::Static {
                class: static_class(ctx, s.class),
                name: names::member(ctx, &s.method),
            },
        };
        return Expr::Callable { target, span };
    }
    // Partial function application: convert what can be converted and
    // reject the placeholders with PHP's parse-error wording.
    let args = partial_args(ctx, list, &unexpected_token("..."));
    match p {
        PartialApplication::Function(f) => {
            let callee = callee(ctx, f.function);
            Expr::Call { callee, args, span }
        }
        PartialApplication::Method(m) => {
            let obj = expr(ctx, m.object);
            let name = names::member(ctx, &m.method);
            Expr::MethodCall {
                obj: Box::new(obj),
                name,
                args,
                nullsafe: false,
                span,
            }
        }
        PartialApplication::StaticMethod(s) => {
            let class = static_class(ctx, s.class);
            let name = names::member(ctx, &s.method);
            Expr::StaticCall {
                class,
                name,
                args,
                span,
            }
        }
    }
}

fn access(ctx: &mut Ctx<'_, '_>, a: &Access<'_>, span: Span) -> Expr {
    match a {
        Access::Property(p) => {
            let obj = expr(ctx, p.object);
            let name = names::member(ctx, &p.property);
            Expr::Prop {
                obj: Box::new(obj),
                name,
                nullsafe: false,
                span,
            }
        }
        Access::NullSafeProperty(p) => {
            let obj = expr(ctx, p.object);
            let name = names::member(ctx, &p.property);
            Expr::Prop {
                obj: Box::new(obj),
                name,
                nullsafe: true,
                span,
            }
        }
        Access::StaticProperty(s) => {
            let class = static_class(ctx, s.class);
            let name = names::static_prop(ctx, &s.property);
            Expr::StaticProp { class, name, span }
        }
        Access::ClassConstant(c) => {
            let class = static_class(ctx, c.class);
            let name = names::const_sel(ctx, &c.constant);
            Expr::ClassConst { class, name, span }
        }
    }
}

fn new_expr(ctx: &mut Ctx<'_, '_>, i: &Instantiation<'_>, span: Span) -> Expr {
    if !is_class_name_reference(i.class) {
        let msg = describe_unexpected(i.class);
        let s = ctx.sp(i.class.span());
        ctx.reject(msg, s);
    }
    let class = names::class_ref(ctx, i.class);
    check_class_scope(ctx, &class);
    let args = match &i.argument_list {
        Some(al) => args(ctx, al),
        None => Vec::new(),
    };
    Expr::New {
        class: NewTarget::Ref(class),
        args,
        span,
    }
}

fn match_expr(ctx: &mut Ctx<'_, '_>, m: &Match<'_>, span: Span) -> Expr {
    let subject = expr(ctx, m.expression);
    let mut arms = Vec::with_capacity(m.arms.len());
    let mut defaults = 0;
    for arm in m.arms.iter() {
        let arm_span = ctx.sp(arm.span());
        match arm {
            MagoMatchArm::Expression(a) => {
                let conds = a.conditions.iter().map(|c| expr(ctx, c)).collect();
                let body = expr(ctx, a.expression);
                arms.push(MatchArm {
                    conds: Some(conds),
                    body,
                    span: arm_span,
                });
            }
            MagoMatchArm::Default(d) => {
                defaults += 1;
                if defaults > 1 {
                    ctx.reject(
                        "Match expressions may only contain one default arm",
                        ctx.sp(d.default.span),
                    );
                }
                let body = expr(ctx, d.expression);
                arms.push(MatchArm {
                    conds: None,
                    body,
                    span: arm_span,
                });
            }
        }
    }
    Expr::Match {
        subject: Box::new(subject),
        arms,
        span,
    }
}

fn yield_expr(ctx: &mut Ctx<'_, '_>, y: &Yield<'_>, span: Span) -> Expr {
    if ctx.fn_kind() == FnKind::Main {
        ctx.reject(
            "The \"yield\" expression can only be used inside a function",
            span,
        );
    }
    ctx.fn_scope().has_yield = true;
    match y {
        Yield::Value(v) => {
            let value = v.value.map(|e| Box::new(expr(ctx, e)));
            Expr::Yield {
                key: None,
                value,
                span,
            }
        }
        Yield::Pair(p) => {
            let key = expr(ctx, p.key);
            let value = expr(ctx, p.value);
            Expr::Yield {
                key: Some(Box::new(key)),
                value: Some(Box::new(value)),
                span,
            }
        }
        Yield::From(f) => Expr::YieldFrom {
            expr: Box::new(expr(ctx, f.iterator)),
            span,
        },
    }
}

fn construct(ctx: &mut Ctx<'_, '_>, c: &Construct<'_>, span: Span) -> Expr {
    match c {
        Construct::Isset(i) => {
            let vars: Vec<Expr> = i.values.iter().map(|v| expr(ctx, v)).collect();
            for v in &vars {
                if !v.is_variable_like() && !matches!(v, Expr::Error(_)) {
                    ctx.reject(
                        "Cannot use isset() on the result of an expression (you can use \"null !== expression\" instead)",
                        v.span(),
                    );
                }
            }
            Expr::Isset { vars, span }
        }
        Construct::Empty(e) => Expr::Empty {
            expr: Box::new(expr(ctx, e.value)),
            span,
        },
        Construct::Eval(e) => Expr::Eval {
            code: Box::new(expr(ctx, e.value)),
            span,
        },
        Construct::Include(i) => include(ctx, IncludeKind::Include, i.value, span),
        Construct::IncludeOnce(i) => include(ctx, IncludeKind::IncludeOnce, i.value, span),
        Construct::Require(r) => include(ctx, IncludeKind::Require, r.value, span),
        Construct::RequireOnce(r) => include(ctx, IncludeKind::RequireOnce, r.value, span),
        Construct::Print(p) => Expr::Print {
            expr: Box::new(expr(ctx, p.value)),
            span,
        },
        Construct::Exit(e) => exit(ctx, e.arguments.as_ref(), span),
        Construct::Die(d) => exit(ctx, d.arguments.as_ref(), span),
    }
}

fn include(ctx: &mut Ctx<'_, '_>, kind: IncludeKind, path: &Expression<'_>, span: Span) -> Expr {
    Expr::Include {
        kind,
        path: Box::new(expr(ctx, path)),
        span,
    }
}

/// `exit` / `die`: PHP 8.4 made them functions. The bare keyword stays the
/// construct; with an argument list it is an ordinary call to `\exit` (php
/// compiles both spellings to `exit`, which is how its stack traces and
/// messages name them), so named arguments, the arity check and the
/// `string|int` coercion under the file's `strict_types` are the native's.
fn exit(ctx: &mut Ctx<'_, '_>, list: Option<&ArgumentList<'_>>, span: Span) -> Expr {
    match list {
        Some(l) => {
            let args = args(ctx, l);
            let text = ctx.intern(b"exit");
            let name = Name::new(text, NameKind::FullyQualified, span);
            Expr::Call { callee: Callee::Name(name), args, span }
        }
        None => Expr::Exit { arg: None, span },
    }
}

fn pipe(ctx: &mut Ctx<'_, '_>, p: &Pipe<'_>, span: Span) -> Expr {
    let input = expr(ctx, p.input);
    if matches!(p.callable, Expression::ArrowFunction(_)) {
        let s = ctx.sp(p.callable.span());
        ctx.reject(
            "Arrow functions on the right hand side of |> must be parenthesized",
            s,
        );
    }
    let callable = expr(ctx, p.callable);
    Expr::Binary {
        op: BinOp::Pipe,
        lhs: Box::new(input),
        rhs: Box::new(callable),
        span,
    }
}
