//! Statements: `mago_syntax::cst::Statement` → [`Stmt`], plus the
//! file-level rules (namespace ordering, `strict_types` placement,
//! `__halt_compiler`, inline HTML after `?>`).

use mago_span::HasSpan;
use mago_syntax::cst::{
    Block, Break, Constant, Continue, Declare, DeclareBody, Expression, ForBody, ForeachBody,
    ForeachTarget, If, IfBody, Inline, InlineKind, Literal, Namespace, NamespaceBody,
    Program as MagoProgram, Sequence, Statement, StaticItem, Switch, SwitchBody, SwitchCase,
    Terminator, Try, UnaryPrefix, UnaryPrefixOperator, Use, UseItem as MagoUseItem, UseItems,
    UseType, WhileBody,
};
use rphp_ast::v2::lvalue::LvalueCtx;
use rphp_ast::v2::{
    Case, Catch, ConstItem, Directive, ElseIf, Expr, Name, StaticVar, Stmt, TypeKind, UseItem,
    UseKind,
};
use rphp_diagnostics::codes;
use rphp_span::Span;

use super::conformance::{
    check_const_expr, check_paren_target, unexpected_token, validate_target, ConstCtx,
};
use super::ctx::{eq_ci, Ctx, FnKind, ReturnValue};
use super::expr::{self, Pos};
use super::literals::{parse_int, IntValue};
use super::{class, func, names, types};

/// Where a statement list sits; decides which statements are legal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Level {
    /// The file's top level.
    File,
    /// Directly inside `namespace X;` (unbraced).
    Namespace,
    /// Directly inside `namespace X { }`.
    BracedNamespace,
    /// Anywhere else (blocks, bodies).
    Nested,
}

/// Convert the whole program.
pub(crate) fn program(ctx: &mut Ctx<'_, '_>, p: &MagoProgram<'_>) -> Vec<Stmt> {
    stmts(ctx, &p.statements, Level::File)
}

/// Convert a statement sequence at the given level.
pub(crate) fn stmts(ctx: &mut Ctx<'_, '_>, seq: &Sequence<'_, Statement<'_>>, level: Level) -> Vec<Stmt> {
    let mut out = Vec::with_capacity(seq.len());
    for s in seq.iter() {
        if ctx.halted {
            break;
        }
        if let Some(st) = stmt(ctx, s, level) {
            out.push(st);
        }
    }
    out
}

/// The statements of a `{ }` block.
pub(crate) fn block_stmts(ctx: &mut Ctx<'_, '_>, b: &Block<'_>) -> Vec<Stmt> {
    stmts(ctx, &b.statements, Level::Nested)
}

/// A single-statement body, unwrapping a direct block.
pub(crate) fn body(ctx: &mut Ctx<'_, '_>, s: &Statement<'_>) -> Vec<Stmt> {
    match s {
        Statement::Block(b) => block_stmts(ctx, b),
        other => stmt(ctx, other, Level::Nested).into_iter().collect(),
    }
}

/// The end offset of a terminator, recording a `?>` for the inline-HTML rule.
fn terminator_end(ctx: &mut Ctx<'_, '_>, t: &Terminator<'_>) -> u32 {
    match t {
        Terminator::Semicolon(s) => s.end.offset,
        Terminator::ClosingTag(ct) | Terminator::TagPair(ct, _) => {
            ctx.last_close_tag_end = Some(ct.span.end.offset);
            ct.span.end.offset
        }
        Terminator::Missing(s) => s.end.offset,
    }
}

/// Whether a top-level statement counts as "code" for the namespace and
/// `strict_types` ordering rules.
fn is_top_code(s: &Statement<'_>) -> bool {
    !matches!(
        s,
        Statement::OpeningTag(_)
            | Statement::ClosingTag(_)
            | Statement::Declare(_)
            | Statement::Noop(_)
            | Statement::Inline(Inline {
                kind: InlineKind::Shebang,
                ..
            })
    )
}

/// Convert one statement; `None` for tags and dropped inline text.
pub(crate) fn stmt(ctx: &mut Ctx<'_, '_>, s: &Statement<'_>, level: Level) -> Option<Stmt> {
    let span = ctx.sp(s.span());
    let at_top = level == Level::File;
    let outside_ns_code = !matches!(
        s,
        Statement::OpeningTag(_) | Statement::ClosingTag(_) | Statement::Noop(_) | Statement::Namespace(_)
    );
    if at_top && ctx.seen_braced_namespace && outside_ns_code {
        if let Statement::Inline(i) = s {
            inline_text(ctx, i)?;
        }
        if !matches!(s, Statement::HaltCompiler(_)) {
            ctx.reject("No code may exist outside of namespace {}", span);
        }
    }
    let result = match s {
        Statement::OpeningTag(_) => None,
        Statement::ClosingTag(ct) => {
            ctx.last_close_tag_end = Some(ct.span.end.offset);
            None
        }
        Statement::Inline(i) => inline(ctx, i),
        Statement::Namespace(ns) => Some(namespace(ctx, ns, level, span)),
        Statement::Use(u) => {
            if !matches!(level, Level::File | Level::Namespace | Level::BracedNamespace) {
                ctx.reject(unexpected_token("use"), ctx.sp(u.r#use.span));
            }
            Some(use_stmt(ctx, u, span))
        }
        Statement::Class(c) => Some(Stmt::ClassLike(class::class(ctx, c))),
        Statement::Interface(i) => Some(Stmt::ClassLike(class::interface(ctx, i))),
        Statement::Trait(t) => Some(Stmt::ClassLike(class::trait_(ctx, t))),
        Statement::Enum(e) => Some(Stmt::ClassLike(class::enum_(ctx, e))),
        Statement::Block(b) => Some(Stmt::Block {
            body: block_stmts(ctx, b),
            span,
        }),
        Statement::Constant(c) => {
            if !matches!(level, Level::File | Level::Namespace | Level::BracedNamespace) {
                ctx.reject(unexpected_token("const"), ctx.sp(c.r#const.span));
            }
            Some(const_decl(ctx, c, span))
        }
        Statement::Function(f) => Some(Stmt::Func(func::function(ctx, f))),
        Statement::Declare(d) => Some(declare(ctx, d, level, span)),
        Statement::Goto(g) => {
            names::check_reserved(ctx, &g.label, "identifier");
            let label = names::local_recorded(ctx, &g.label);
            terminator_end(ctx, &g.terminator);
            Some(Stmt::Goto { label, span })
        }
        Statement::Label(l) => {
            names::check_reserved(ctx, &l.name, ":");
            let name = names::local_recorded(ctx, &l.name);
            Some(Stmt::Label { name, span })
        }
        Statement::Try(t) => Some(try_stmt(ctx, t, span)),
        Statement::Foreach(f) => {
            let subject = expr::expr(ctx, f.expression);
            let (key, value, by_ref) = foreach_target(ctx, &f.target);
            let body = ctx.in_loop(|ctx| match &f.body {
                ForeachBody::Statement(s) => self::body(ctx, s),
                ForeachBody::ColonDelimited(c) => {
                    let b = stmts(ctx, &c.statements, Level::Nested);
                    terminator_end(ctx, &c.terminator);
                    b
                }
            });
            Some(Stmt::Foreach {
                subject,
                key: key.map(Box::new),
                value: Box::new(value),
                by_ref,
                body,
                span,
            })
        }
        Statement::For(f) => {
            // `(void)` is legal at the top of every init/step expression and
            // of every condition but the last (the one whose value is tested).
            let init = f.initializations.iter().map(|e| expr::stmt_expr(ctx, e)).collect();
            let n = f.conditions.len();
            let cond = f
                .conditions
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    if i + 1 < n {
                        expr::stmt_expr(ctx, e)
                    } else {
                        expr::expr(ctx, e)
                    }
                })
                .collect();
            let step = f.increments.iter().map(|e| expr::stmt_expr(ctx, e)).collect();
            let body = ctx.in_loop(|ctx| match &f.body {
                ForBody::Statement(s) => self::body(ctx, s),
                ForBody::ColonDelimited(c) => {
                    let b = stmts(ctx, &c.statements, Level::Nested);
                    terminator_end(ctx, &c.terminator);
                    b
                }
            });
            Some(Stmt::For {
                init,
                cond,
                step,
                body,
                span,
            })
        }
        Statement::While(w) => {
            let cond = expr::expr(ctx, w.condition);
            let body = ctx.in_loop(|ctx| match &w.body {
                WhileBody::Statement(s) => self::body(ctx, s),
                WhileBody::ColonDelimited(c) => {
                    let b = stmts(ctx, &c.statements, Level::Nested);
                    terminator_end(ctx, &c.terminator);
                    b
                }
            });
            Some(Stmt::While { cond, body, span })
        }
        Statement::DoWhile(d) => {
            let body = ctx.in_loop(|ctx| self::body(ctx, d.statement));
            let cond = expr::expr(ctx, d.condition);
            terminator_end(ctx, &d.terminator);
            Some(Stmt::DoWhile { body, cond, span })
        }
        Statement::Continue(c) => Some(continue_stmt(ctx, c, span)),
        Statement::Break(b) => Some(break_stmt(ctx, b, span)),
        Statement::Switch(sw) => Some(switch(ctx, sw, span)),
        Statement::If(i) => Some(if_stmt(ctx, i, span)),
        Statement::Return(r) => {
            // `function &f() { return $a[]; }` fetches the element for writing.
            let pos = if ctx.fn_scope().by_ref { Pos::Write } else { Pos::Read };
            let value = r.value.map(|v| expr::expr_in(ctx, v, pos));
            terminator_end(ctx, &r.terminator);
            let kind = match &value {
                None => ReturnValue::None,
                Some(Expr::Null(_)) => ReturnValue::Null,
                Some(_) => ReturnValue::Value,
            };
            ctx.fn_scope().returns.push((span, kind));
            Some(Stmt::Return { value, span })
        }
        Statement::Expression(es) => {
            let e = expr::stmt_expr(ctx, es.expression);
            terminator_end(ctx, &es.terminator);
            Some(Stmt::Expr { expr: e, span })
        }
        Statement::Echo(e) => {
            let args = e.values.iter().map(|v| expr::expr(ctx, v)).collect();
            terminator_end(ctx, &e.terminator);
            Some(Stmt::Echo { args, span })
        }
        Statement::EchoTag(e) => {
            let args = e.values.iter().map(|v| expr::expr(ctx, v)).collect();
            terminator_end(ctx, &e.terminator);
            Some(Stmt::Echo { args, span })
        }
        Statement::Global(g) => {
            let vars: Vec<Expr> = g.variables.iter().map(|v| expr::variable(ctx, v)).collect();
            for v in &vars {
                validate_target(ctx, v, LvalueCtx::Global);
            }
            terminator_end(ctx, &g.terminator);
            Some(Stmt::Global { vars, span })
        }
        Statement::Static(st) => Some(static_stmt(ctx, st, span)),
        Statement::HaltCompiler(h) => {
            let allowed = matches!(level, Level::File | Level::Namespace)
                && ctx.fn_kind() == FnKind::Main
                && ctx.classes.is_empty();
            if !allowed {
                ctx.reject("__HALT_COMPILER() can only be used from the outermost scope", span);
            }
            let mut end = terminator_end(ctx, &h.terminator);
            if matches!(h.terminator, Terminator::ClosingTag(_)) {
                // PHP's `?>` token swallows one newline; the offset is past it.
                match ctx.src.get(end as usize) {
                    Some(b'\n') => end += 1,
                    Some(b'\r') => {
                        end += 1;
                        if ctx.src.get(end as usize) == Some(&b'\n') {
                            end += 1;
                        }
                    }
                    _ => {}
                }
            }
            if allowed && ctx.halt_offset.is_none() {
                ctx.halt_offset = Some(end);
                ctx.halted = true;
            }
            Some(Stmt::HaltCompiler { span })
        }
        Statement::Unset(u) => {
            let mut targets = Vec::with_capacity(u.values.len());
            for v in u.values.iter() {
                check_paren_target(ctx, v, ")");
                let t = expr::expr_in(ctx, v, Pos::Write);
                validate_target(ctx, &t, LvalueCtx::Unset);
                targets.push(t);
            }
            terminator_end(ctx, &u.terminator);
            Some(Stmt::Unset { targets, span })
        }
        Statement::Noop(_) => Some(Stmt::Nop { span }),
        other => {
            let kind = mago_syntax::cst::Node::Statement(other).kind();
            ctx.unsupported(kind, span);
            Some(Stmt::Nop { span })
        }
    };
    if at_top && is_top_code(s) {
        // Inline text that is only the newline after `?>` is not code.
        let empty_inline = matches!(s, Statement::Inline(_)) && result.is_none();
        if !empty_inline {
            ctx.seen_top_code = true;
        }
    }
    result
}

/// The bytes of an inline segment after PHP's newline swallowing; `None`
/// when nothing is left (or for a shebang line, which php-cli skips).
fn inline_text<'s>(ctx: &Ctx<'_, 's>, i: &Inline<'s>) -> Option<&'s [u8]> {
    if i.kind == InlineKind::Shebang {
        return None;
    }
    let mut text = i.value;
    if ctx.last_close_tag_end == Some(i.span.start.offset) {
        text = match text {
            [b'\r', b'\n', rest @ ..] => rest,
            [b'\n' | b'\r', rest @ ..] => rest,
            other => other,
        };
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn inline(ctx: &mut Ctx<'_, '_>, i: &Inline<'_>) -> Option<Stmt> {
    let text = inline_text(ctx, i)?;
    let stripped = i.value.len() - text.len();
    let span = ctx.span(i.span.start.offset + stripped as u32, i.span.end.offset);
    let id = ctx.intern(text);
    Some(Stmt::InlineHtml { text: id, span })
}

fn namespace(ctx: &mut Ctx<'_, '_>, ns: &Namespace<'_>, level: Level, span: Span) -> Stmt {
    let braced = matches!(ns.body, NamespaceBody::BraceDelimited(_));
    if level != Level::File {
        ctx.reject(unexpected_token("namespace"), ctx.sp(ns.namespace.span));
    } else {
        let first = !ctx.seen_braced_namespace && !ctx.seen_unbraced_namespace;
        if (braced && ctx.seen_unbraced_namespace) || (!braced && ctx.seen_braced_namespace) {
            ctx.reject(
                "Cannot mix bracketed namespace declarations with unbracketed namespace declarations",
                span,
            );
        } else if first && ctx.seen_top_code {
            ctx.reject(
                "Namespace declaration statement has to be the very first statement or after any declare call in the script",
                span,
            );
        }
        if braced {
            ctx.seen_braced_namespace = true;
        } else {
            ctx.seen_unbraced_namespace = true;
        }
    }
    let name = ns.name.as_ref().map(|n| {
        let (kind, _) = names::name_parts(n);
        if kind != rphp_ast::v2::NameKind::Unqualified && kind != rphp_ast::v2::NameKind::Qualified {
            let s = ctx.sp(n.span());
            ctx.reject("syntax error, unexpected namespaced name, expecting \"{\"", s);
        }
        names::name(ctx, n)
    });
    let (body, end) = match &ns.body {
        NamespaceBody::Implicit(i) => {
            terminator_end(ctx, &i.terminator);
            let body = stmts(ctx, &i.statements, Level::Namespace);
            let end = body.last().map_or(span.hi, |s| s.span().hi.max(span.hi));
            (body, end)
        }
        NamespaceBody::BraceDelimited(b) => (stmts(ctx, &b.statements, Level::BracedNamespace), span.hi),
    };
    Stmt::Namespace {
        name,
        body,
        braced,
        span: Span::new(span.file, span.lo, end),
    }
}

fn use_kind(t: &UseType<'_>) -> UseKind {
    match t {
        UseType::Function(_) => UseKind::Function,
        UseType::Const(_) => UseKind::Const,
    }
}

fn use_item(ctx: &mut Ctx<'_, '_>, it: &MagoUseItem<'_>, kind: Option<UseKind>) -> UseItem {
    let name = names::name(ctx, &it.name);
    let alias = it.alias.as_ref().map(|a| {
        names::check_reserved(ctx, &a.identifier, "identifier");
        names::local(ctx, &a.identifier)
    });
    UseItem {
        name,
        alias,
        kind,
        span: ctx.sp(it.span()),
    }
}

fn use_stmt(ctx: &mut Ctx<'_, '_>, u: &Use<'_>, span: Span) -> Stmt {
    let (kind, prefix, items): (UseKind, Option<Name>, Vec<UseItem>) = match &u.items {
        UseItems::Sequence(s) => (
            UseKind::Class,
            None,
            s.items.iter().map(|it| use_item(ctx, it, None)).collect(),
        ),
        UseItems::TypedSequence(t) => (
            use_kind(&t.r#type),
            None,
            t.items.iter().map(|it| use_item(ctx, it, None)).collect(),
        ),
        UseItems::TypedList(t) => (
            use_kind(&t.r#type),
            Some(names::name(ctx, &t.namespace)),
            {
                check_group_use(ctx, t.items.len(), t.right_brace, t.items.iter().map(|it| &it.name));
                t.items.iter().map(|it| use_item(ctx, it, None)).collect()
            },
        ),
        UseItems::MixedList(m) => (
            UseKind::Class,
            Some(names::name(ctx, &m.namespace)),
            {
                check_group_use(ctx, m.items.len(), m.right_brace, m.items.iter().map(|it| &it.item.name));
                m.items
                .iter()
                .map(|it| {
                    let kind = it.r#type.as_ref().map(use_kind);
                    let mut item = use_item(ctx, &it.item, kind);
                    item.span = ctx.sp(it.span());
                    item
                })
                .collect()
            },
        ),
    };
    terminator_end(ctx, &u.terminator);
    Stmt::Use {
        kind,
        prefix,
        items,
        span,
    }
}

/// `use A\{}` is empty and `use A\{\B}` has a leading `\`: both parse
/// errors in PHP.
fn check_group_use<'x>(
    ctx: &mut Ctx<'_, '_>,
    len: usize,
    right_brace: mago_span::Span,
    items: impl Iterator<Item = &'x mago_syntax::cst::Identifier<'x>>,
) {
    if len == 0 {
        let s = ctx.sp(right_brace);
        ctx.reject(
            "syntax error, unexpected token \"}\", expecting identifier or namespaced name or \"function\" or \"const\"",
            s,
        );
    }
    for id in items {
        if let mago_syntax::cst::Identifier::FullyQualified(f) = id {
            let s = ctx.sp(f.span);
            ctx.reject(
                format!(
                    "syntax error, unexpected fully qualified name \"{}\", expecting identifier or namespaced name or \"function\" or \"const\"",
                    String::from_utf8_lossy(f.value)
                ),
                s,
            );
        }
    }
}

fn const_decl(ctx: &mut Ctx<'_, '_>, c: &Constant<'_>, span: Span) -> Stmt {
    let attrs = super::attrs::groups(ctx, &c.attribute_lists);
    let items = c
        .items
        .iter()
        .map(|it| {
            names::check_reserved(ctx, &it.name, "identifier");
            let name = names::local_recorded(ctx, &it.name);
            let value = expr::const_expr(ctx, it.value);
            check_const_expr(ctx, &value, ConstCtx::GlobalConst);
            ConstItem {
                name,
                value,
                span: ctx.sp(it.span()),
            }
        })
        .collect();
    terminator_end(ctx, &c.terminator);
    Stmt::ConstDecl { attrs, items, span }
}

fn declare(ctx: &mut Ctx<'_, '_>, d: &Declare<'_>, level: Level, span: Span) -> Stmt {
    let mut directives = Vec::with_capacity(d.items.len());
    for it in d.items.iter() {
        let name = names::local(ctx, &it.name);
        let value = expr::expr(ctx, it.value);
        let dir_span = ctx.sp(it.span());
        if eq_ci(it.name.value, "strict_types") {
            let block = !matches!(d.body, DeclareBody::Statement(Statement::Noop(_)));
            match &value {
                Expr::Int(0 | 1, _) => {}
                Expr::Int(..) => ctx.error(
                    codes::STRICT_TYPES,
                    "strict_types declaration must have 0 or 1 as its value",
                    dir_span,
                ),
                _ => ctx.error(
                    codes::STRICT_TYPES,
                    "declare(strict_types) value must be a literal",
                    dir_span,
                ),
            }
            if level != Level::File || ctx.seen_top_code {
                ctx.error(
                    codes::STRICT_TYPES,
                    "strict_types declaration must be the very first statement in the script",
                    span,
                );
            } else if block {
                ctx.error(
                    codes::STRICT_TYPES,
                    "strict_types declaration must not use block mode",
                    span,
                );
            } else if matches!(value, Expr::Int(1, _)) {
                ctx.strict_types = true;
            }
        } else if eq_ci(it.name.value, "encoding") && !matches!(value, Expr::Str(..)) {
            ctx.reject("Encoding must be a literal", dir_span);
        }
        directives.push(Directive {
            name,
            value,
            span: dir_span,
        });
    }
    let body = match &d.body {
        DeclareBody::Statement(Statement::Noop(n)) => {
            let _ = n;
            None
        }
        DeclareBody::Statement(s) => Some(self::body(ctx, s)),
        DeclareBody::ColonDelimited(c) => {
            let b = stmts(ctx, &c.statements, Level::Nested);
            terminator_end(ctx, &c.terminator);
            Some(b)
        }
    };
    Stmt::Declare {
        directives,
        body,
        span,
    }
}

fn try_stmt(ctx: &mut Ctx<'_, '_>, t: &Try<'_>, span: Span) -> Stmt {
    let body = block_stmts(ctx, &t.block);
    let mut catches = Vec::with_capacity(t.catch_clauses.len());
    for c in t.catch_clauses.iter() {
        let c_span = ctx.sp(c.span());
        let ty = types::hint(ctx, &c.hint);
        let mut names_out = Vec::new();
        collect_catch_names(ctx, &ty, &mut names_out);
        let var = c.variable.as_ref().map(|v| {
            let id = ctx.intern_var(v.name);
            if Some(id) == ctx.sv.this {
                ctx.reject("Cannot re-assign $this", ctx.sp(v.span));
            }
            id
        });
        let body = block_stmts(ctx, &c.block);
        catches.push(Catch {
            types: names_out,
            var,
            body,
            span: c_span,
        });
    }
    let finally = t.finally_clause.as_ref().map(|f| block_stmts(ctx, &f.block));
    if catches.is_empty() && finally.is_none() {
        ctx.reject("Cannot use try without catch or finally", span);
    }
    Stmt::Try {
        body,
        catches,
        finally,
        span,
    }
}

/// The class names of a `catch (A | B $e)` type (anything else is a parse
/// error in PHP).
fn collect_catch_names(ctx: &mut Ctx<'_, '_>, t: &rphp_ast::v2::Type, out: &mut Vec<Name>) {
    match &t.kind {
        TypeKind::Named(n) => out.push(n.clone()),
        TypeKind::Union(ms) => {
            for m in ms {
                collect_catch_names(ctx, m, out);
            }
        }
        TypeKind::Builtin(rphp_ast::v2::Builtin::SelfTy | rphp_ast::v2::Builtin::StaticTy | rphp_ast::v2::Builtin::ParentTy) => {
            ctx.reject("Bad class name in the catch statement", t.span);
        }
        TypeKind::Nullable(_) => ctx.reject(unexpected_token("?"), t.span),
        // `catch (int $e)` parses (a class named `int`); the rest are keywords.
        TypeKind::Builtin(b) => {
            let id = ctx.intern(b.as_str().as_bytes());
            out.push(Name::new(id, rphp_ast::v2::NameKind::Unqualified, t.span));
        }
        TypeKind::Intersection(_) => ctx.reject(unexpected_token("&"), t.span),
    }
}

fn foreach_target(ctx: &mut Ctx<'_, '_>, t: &ForeachTarget<'_>) -> (Option<Expr>, Expr, bool) {
    let (key, value) = match t {
        ForeachTarget::Value(v) => (None, v.value),
        ForeachTarget::KeyValue(kv) => (Some(kv.key), kv.value),
    };
    let key = key.map(|k| {
        if let Expression::UnaryPrefix(UnaryPrefix {
            operator: UnaryPrefixOperator::Reference(_),
            operand,
        }) = k
        {
            ctx.reject("Key element cannot be a reference", ctx.sp(k.span()));
            let e = expr::expr_in(ctx, operand, Pos::Write);
            validate_target(ctx, &e, LvalueCtx::ForeachKey);
            return e;
        }
        check_paren_target(ctx, k, ")");
        let e = expr::expr_in(ctx, k, Pos::Write);
        validate_target(ctx, &e, LvalueCtx::ForeachKey);
        e
    });
    let (value, by_ref) = if let Expression::UnaryPrefix(UnaryPrefix {
        operator: UnaryPrefixOperator::Reference(_),
        operand,
    }) = value
    {
        check_paren_target(ctx, operand, ")");
        let e = expr::expr_in(ctx, operand, Pos::Write);
        validate_target(ctx, &e, LvalueCtx::AssignRef);
        (e, true)
    } else {
        check_paren_target(ctx, value, ")");
        let e = expr::expr_in(ctx, value, Pos::Write);
        validate_target(ctx, &e, LvalueCtx::Foreach);
        (e, false)
    };
    (key, value, by_ref)
}

/// The level operand of `break` / `continue`, with PHP's rules.
fn loop_levels(ctx: &mut Ctx<'_, '_>, kw: &str, level: Option<&Expression<'_>>, span: Span) -> u32 {
    let n: u32 = match level {
        None => 1,
        Some(e) => {
            let mut e = e;
            while let Expression::Parenthesized(p) = e {
                e = p.expression;
            }
            match e {
                Expression::Literal(Literal::Integer(i)) => match parse_int(i.raw) {
                    IntValue::Int(v) if v >= 1 => u32::try_from(v).unwrap_or(u32::MAX),
                    _ => {
                        ctx.reject(format!("'{kw}' operator accepts only positive integers"), span);
                        1
                    }
                },
                Expression::Literal(_) => {
                    ctx.reject(format!("'{kw}' operator accepts only positive integers"), span);
                    1
                }
                other => {
                    let _ = expr::expr(ctx, other);
                    ctx.reject(
                        format!("'{kw}' operator with non-integer operand is no longer supported"),
                        span,
                    );
                    1
                }
            }
        }
    };
    let depth = ctx.fn_scope().loop_depth;
    if depth == 0 {
        ctx.reject(format!("'{kw}' not in the 'loop' or 'switch' context"), span);
    } else if n > depth {
        ctx.reject(format!("Cannot '{kw}' {n} levels"), span);
    }
    n
}

fn break_stmt(ctx: &mut Ctx<'_, '_>, b: &Break<'_>, span: Span) -> Stmt {
    let levels = loop_levels(ctx, "break", b.level, span);
    terminator_end(ctx, &b.terminator);
    Stmt::Break { levels, span }
}

fn continue_stmt(ctx: &mut Ctx<'_, '_>, c: &Continue<'_>, span: Span) -> Stmt {
    let levels = loop_levels(ctx, "continue", c.level, span);
    terminator_end(ctx, &c.terminator);
    Stmt::Continue { levels, span }
}

fn switch(ctx: &mut Ctx<'_, '_>, sw: &Switch<'_>, span: Span) -> Stmt {
    let subject = expr::expr(ctx, sw.expression);
    let cases_seq = match &sw.body {
        SwitchBody::BraceDelimited(b) => &b.cases,
        SwitchBody::ColonDelimited(c) => &c.cases,
    };
    let mut defaults = 0;
    let cases = ctx.in_loop(|ctx| {
        let mut out = Vec::with_capacity(cases_seq.len());
        for c in cases_seq.iter() {
            let c_span = ctx.sp(c.span());
            match c {
                SwitchCase::Expression(e) => {
                    let cond = expr::expr(ctx, e.expression);
                    let body = stmts(ctx, &e.statements, Level::Nested);
                    out.push(Case {
                        cond: Some(cond),
                        body,
                        span: c_span,
                    });
                }
                SwitchCase::Default(d) => {
                    defaults += 1;
                    if defaults > 1 {
                        ctx.reject(
                            "Switch statements may only contain one default clause",
                            ctx.sp(d.default.span),
                        );
                    }
                    let body = stmts(ctx, &d.statements, Level::Nested);
                    out.push(Case {
                        cond: None,
                        body,
                        span: c_span,
                    });
                }
            }
        }
        out
    });
    if let SwitchBody::ColonDelimited(c) = &sw.body {
        terminator_end(ctx, &c.terminator);
    }
    Stmt::Switch {
        subject,
        cases,
        span,
    }
}

fn if_stmt(ctx: &mut Ctx<'_, '_>, i: &If<'_>, span: Span) -> Stmt {
    let cond = expr::expr(ctx, i.condition);
    match &i.body {
        IfBody::Statement(b) => {
            let then = body(ctx, b.statement);
            let elseifs = b
                .else_if_clauses
                .iter()
                .map(|c| {
                    let cond = expr::expr(ctx, c.condition);
                    let body = body(ctx, c.statement);
                    ElseIf {
                        cond,
                        body,
                        span: ctx.sp(c.span()),
                    }
                })
                .collect();
            let else_ = b.else_clause.as_ref().map(|e| body(ctx, e.statement));
            Stmt::If {
                cond,
                then,
                elseifs,
                else_,
                span,
            }
        }
        IfBody::ColonDelimited(b) => {
            let then = stmts(ctx, &b.statements, Level::Nested);
            let elseifs = b
                .else_if_clauses
                .iter()
                .map(|c| {
                    let cond = expr::expr(ctx, c.condition);
                    let body = stmts(ctx, &c.statements, Level::Nested);
                    ElseIf {
                        cond,
                        body,
                        span: ctx.sp(c.span()),
                    }
                })
                .collect();
            let else_ = b
                .else_clause
                .as_ref()
                .map(|e| stmts(ctx, &e.statements, Level::Nested));
            terminator_end(ctx, &b.terminator);
            Stmt::If {
                cond,
                then,
                elseifs,
                else_,
                span,
            }
        }
    }
}

fn static_stmt(ctx: &mut Ctx<'_, '_>, st: &mago_syntax::cst::Static<'_>, span: Span) -> Stmt {
    let mut vars: Vec<StaticVar> = Vec::with_capacity(st.items.len());
    for it in st.items.iter() {
        let it_span = ctx.sp(it.span());
        let (var, init) = match it {
            StaticItem::Abstract(a) => (&a.variable, None),
            StaticItem::Concrete(c) => (&c.variable, Some(c.value)),
        };
        let name = ctx.intern_var(var.name);
        if Some(name) == ctx.sv.this {
            ctx.reject("Cannot use $this as static variable", it_span);
        } else if vars.iter().any(|v| v.name == name) {
            let text = ctx.interner.resolve_lossy(name).into_owned();
            ctx.reject(format!("Duplicate declaration of static variable ${text}"), it_span);
        }
        let init = init.map(|e| expr::expr(ctx, e));
        vars.push(StaticVar {
            name,
            init,
            span: it_span,
        });
    }
    terminator_end(ctx, &st.terminator);
    Stmt::StaticVar { vars, span }
}
