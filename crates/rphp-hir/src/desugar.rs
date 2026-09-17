//! Desugaring into the canonical subset (ADR-026), in place.
//!
//! The table, as implemented. `t` is a fresh [`TempId`] per function body;
//! every rule below was checked against php 8.5.0 with side-effecting
//! calls, and `tests/desugar.rs` snapshots each shape.
//!
//! | sugar | canonical form |
//! |-------|----------------|
//! | `if (a) A elseif (b) B else C` | `if (a) A else { if (b) B else C }` (`If.elseifs` is always empty) |
//! | `a ?: b` | `Let(t, a, Ternary(Temp t, Temp t, b))` |
//! | `target ??= v` | *stabilize*(target), then `Coalesce(target, Assign(target, v))` — the fetch runs twice (php re-fetches `$o->a` too) but every side-effecting sub-expression once |
//! | `$o?->a->b(x)[i]` | `Let(t, $o, Ternary(Identical(Temp t, null), null, chain))` with the whole chain (`->`, `?->`, `[]`, `::$`, `::m()` links) short-circuited; inner `?->` links nest further `Let`s in evaluation order |
//! | `isset($o?->a)` / `empty($o?->a)` | same, yielding `false` / `true` on the null branch, the `Isset`/`Empty` node inside the other branch |
//! | `$o?->a ?? d` | `Let(t, $o, Ternary(…, d, Coalesce(chain, d)))` — the default is duplicated into the null branch so `??` keeps its quiet fetch on a plain chain |
//! | `isset(a, b)` | `And(Isset([a]), Isset([b]))` |
//! | `[$a, [, $b], 'k' => $c] = e` / `list(...)` | `Let(t, e, Seq[Assign($a, t[0]), Let(t2, t[1], Seq[Assign($b, t2[1]), Temp t2]), Assign($c, t['k']), Temp t])` — keys evaluate in item order at their assignment, the value of the whole is the source |
//! | `[$a, &$b] = $arr` (any by-ref item) | the source is *stabilized* instead of copied and each item reads it: `Seq[Assign($a, $arr[0]), Assign($b, &$arr[1]), $arr]` |
//! | `foreach (e as [$a, $b])` | `Foreach{value: Temp t}` with `Seq[...]` (as above, reading `Temp t`) prepended to the body; `by_ref` is set when the pattern has a by-ref item so `t` aliases the element |
//! | `x \|> f(...)` / `\|> $o->m(...)` / `\|> C::m(...)` | `Let(t, x, Call/MethodCall/StaticCall(…, [Temp t]))` — `x` first, then the callee, as php |
//! | `x \|> callable` | `Let(t, x, Call{callee: Expr(callable), args: [Temp t]})` |
//! | `get => e;` | `get { return e; }` |
//! | `set => e;` | `set { $this->prop = e; }` (php's definition of the shorthand); a `set` hook without a parameter list gets the implicit `$value` parameter typed like the property |
//! | `readonly class` | `readonly` set on every property member and promoted parameter |
//!
//! **Stabilize** binds, from left to right, every sub-expression of a write
//! target that php memoizes between its read and its write: dimension
//! expressions, computed member names and class expressions, `$$name`
//! names, and a non-variable chain base (`f()->x`), *except* literals and
//! plain variables, which php re-reads (`$a[$i] ??= ($i = 5)` writes
//! `$a[5]`, verified). The fetch chain itself is left in place.
//!
//! Kept as nodes (the compiler lowers them natively): `for`, `do`/`while`,
//! `switch`, `match`, full `?:`, `??` on plain operands, `op=`, `++`/`--`,
//! single-operand `isset`/`empty`, `clone`, `print`/`exit`, `include`/
//! `eval`, `yield`, `global`/`static`, `unset`, `Interp`, casts, `@`,
//! `(void)`, `instanceof`, first-class callables, and `ArrowFn` (see
//! [`crate::free_vars`]). Legacy spellings (`and`/`or`/`xor`, `<>`,
//! `(integer)`, `(boolean)`, `(double)`, `(binary)`) are already canonical
//! `BinOp`/`CastKind` values in the tree.

use rphp_ast::v2::visit::mutable::{walk_expr, walk_stmt};
use rphp_ast::v2::{
    Arg, ArrayItem, ArrowFn, BinOp, CallableTarget, Callee, ClassLike, ClassRef, Closure, Expr,
    Hook, HookBody, HookKind, Member, MemberName, Modifiers, Param, Program, Stmt, TempId, Type,
    VisitorMut,
};
use rphp_diagnostics::Diagnostic;
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;

/// Run the desugaring pass.
pub(crate) fn run(program: &mut Program, interner: &mut Interner, diags: &mut Vec<Diagnostic>) {
    let mut d = Desugar {
        it: interner,
        _diags: diags,
        temps: vec![0],
        cur_prop: None,
    };
    d.visit_block(&mut program.items);
}

struct Desugar<'a> {
    it: &'a mut Interner,
    _diags: &'a mut Vec<Diagnostic>,
    /// One counter per function body (innermost last).
    temps: Vec<u32>,
    /// The property (name, type) whose hooks are being walked.
    cur_prop: Option<(IdentId, Option<Type>)>,
}

/// A pending `Let` binding produced by *stabilize*.
struct Bind {
    temp: TempId,
    init: Expr,
}

impl Desugar<'_> {
    fn new_temp(&mut self) -> TempId {
        let c = self.temps.last_mut().expect("temp counter stack");
        let t = TempId(*c);
        *c += 1;
        t
    }

    fn in_body<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.temps.push(0);
        let r = f(self);
        self.temps.pop();
        r
    }

    // ----- helpers to build nodes ------------------------------------------

    fn temp(t: TempId, span: Span) -> Expr {
        Expr::Temp(t, span)
    }

    fn let_(temp: TempId, init: Expr, body: Expr, span: Span) -> Expr {
        Expr::Let {
            temp,
            init: Box::new(init),
            body: Box::new(body),
            span,
        }
    }

    fn wrap_lets(binds: Vec<Bind>, body: Expr, span: Span) -> Expr {
        binds
            .into_iter()
            .rev()
            .fold(body, |acc, b| Self::let_(b.temp, b.init, acc, span))
    }

    fn is_null_check(t: TempId, span: Span) -> Expr {
        Expr::Binary {
            op: BinOp::Identical,
            lhs: Box::new(Expr::Temp(t, span)),
            rhs: Box::new(Expr::Null(span)),
            span,
        }
    }

    fn ternary(cond: Expr, then: Expr, else_: Expr, span: Span) -> Expr {
        Expr::Ternary {
            cond: Box::new(cond),
            then: Some(Box::new(then)),
            else_: Box::new(else_),
            span,
        }
    }

    fn assign(target: Expr, value: Expr, by_ref: bool, span: Span) -> Expr {
        Expr::Assign {
            target: Box::new(target),
            value: Box::new(value),
            op: None,
            by_ref,
            span,
        }
    }

    fn index(base: Expr, index: Expr, span: Span) -> Expr {
        Expr::Index {
            base: Box::new(base),
            index: Some(Box::new(index)),
            span,
        }
    }

    // ----- stabilize -----------------------------------------------------------

    /// `true` for sub-expressions php does not memoize (re-reads) between the
    /// read and the write of a target: literals and plain variables.
    fn trivial(e: &Expr) -> bool {
        e.is_literal() || matches!(e, Expr::Var(..) | Expr::Temp(..))
    }

    fn bind(&mut self, e: &mut Expr, binds: &mut Vec<Bind>) {
        if Self::trivial(e) {
            return;
        }
        let span = e.span();
        let t = self.new_temp();
        let init = std::mem::replace(e, Expr::Temp(t, span));
        binds.push(Bind { temp: t, init });
    }

    fn bind_member(&mut self, m: &mut MemberName, binds: &mut Vec<Bind>) {
        if let MemberName::Expr(e) = m {
            self.bind(e, binds);
        }
    }

    /// Bind the side-effecting sub-expressions of a write target, left to
    /// right; the fetch chain stays.
    fn stabilize(&mut self, target: &mut Expr, binds: &mut Vec<Bind>) {
        match target {
            Expr::Var(..) | Expr::Temp(..) => {}
            Expr::VarVar { name, .. } => self.bind(name, binds),
            Expr::Index { base, index, .. } => {
                self.stabilize_base(base, binds);
                if let Some(i) = index {
                    self.bind(i, binds);
                }
            }
            Expr::Prop { obj, name, .. } => {
                self.stabilize_base(obj, binds);
                self.bind_member(name, binds);
            }
            Expr::StaticProp { class, name, .. } => {
                if let ClassRef::Expr(e) = class {
                    self.bind(e, binds);
                }
                self.bind_member(name, binds);
            }
            other => self.bind(other, binds),
        }
    }

    /// The base of a fetch: a variable-like chain is stabilized in turn, any
    /// other expression (a call, `new`, ...) is bound whole.
    fn stabilize_base(&mut self, base: &mut Expr, binds: &mut Vec<Bind>) {
        if base.is_variable_like() {
            self.stabilize(base, binds);
        } else {
            self.bind(base, binds);
        }
    }

    // ----- nullsafe chains -------------------------------------------------------

    /// A link of a short-circuiting chain (Zend's
    /// `zend_ast_kind_is_short_circuited`): its base is an expression.
    fn is_chain_link(e: &Expr) -> bool {
        matches!(
            e,
            Expr::Prop { .. }
                | Expr::Index { .. }
                | Expr::MethodCall { .. }
                | Expr::StaticProp {
                    class: ClassRef::Expr(_),
                    ..
                }
                | Expr::StaticCall {
                    class: ClassRef::Expr(_),
                    ..
                }
        )
    }

    fn base_mut(e: &mut Expr) -> &mut Expr {
        match e {
            Expr::Prop { obj, .. } | Expr::MethodCall { obj, .. } => obj,
            Expr::Index { base, .. } => base,
            Expr::StaticProp {
                class: ClassRef::Expr(c),
                ..
            }
            | Expr::StaticCall {
                class: ClassRef::Expr(c),
                ..
            } => c,
            _ => unreachable!("not a chain link"),
        }
    }

    fn nullsafe_mut(e: &mut Expr) -> Option<&mut bool> {
        match e {
            Expr::Prop { nullsafe, .. } | Expr::MethodCall { nullsafe, .. } => Some(nullsafe),
            _ => None,
        }
    }

    /// Visit the non-base children of every link and the innermost base.
    fn visit_chain_children(&mut self, e: &mut Expr) {
        let mut cur = e;
        loop {
            match cur {
                Expr::Prop { obj, name, .. } => {
                    if let MemberName::Expr(n) = name {
                        self.visit_expr(n);
                    }
                    cur = obj;
                }
                Expr::Index { base, index, .. } => {
                    if let Some(i) = index {
                        self.visit_expr(i);
                    }
                    cur = base;
                }
                Expr::MethodCall {
                    obj, name, args, ..
                } => {
                    if let MemberName::Expr(n) = name {
                        self.visit_expr(n);
                    }
                    for a in args {
                        self.visit_arg(a);
                    }
                    cur = obj;
                }
                Expr::StaticProp {
                    class: ClassRef::Expr(c),
                    name,
                    ..
                } => {
                    if let MemberName::Expr(n) = name {
                        self.visit_expr(n);
                    }
                    cur = c;
                }
                Expr::StaticCall {
                    class: ClassRef::Expr(c),
                    name,
                    args,
                    ..
                } => {
                    if let MemberName::Expr(n) = name {
                        self.visit_expr(n);
                    }
                    for a in args {
                        self.visit_arg(a);
                    }
                    cur = c;
                }
                other => {
                    self.visit_expr(other);
                    return;
                }
            }
        }
    }

    /// Depth (0 = root) of the innermost `?->` link of a chain, if any.
    fn innermost_nullsafe(e: &Expr) -> Option<usize> {
        let mut cur = e;
        let mut depth = 0;
        let mut found = None;
        while Self::is_chain_link(cur) {
            let ns = match cur {
                Expr::Prop { nullsafe, .. } | Expr::MethodCall { nullsafe, .. } => *nullsafe,
                _ => false,
            };
            if ns {
                found = Some(depth);
            }
            cur = match cur {
                Expr::Prop { obj, .. } | Expr::MethodCall { obj, .. } => obj,
                Expr::Index { base, .. } => base,
                Expr::StaticProp {
                    class: ClassRef::Expr(c),
                    ..
                }
                | Expr::StaticCall {
                    class: ClassRef::Expr(c),
                    ..
                } => c,
                _ => break,
            };
            depth += 1;
        }
        found
    }

    /// Rewrite the `?->` links of `chain` from the innermost outwards;
    /// `on_null` is the value of the whole expression when a link's base is
    /// `null`, `wrap` builds the node around the (now plain) chain.
    fn short_circuit(
        &mut self,
        mut chain: Expr,
        on_null: &Expr,
        wrap: &dyn Fn(Expr) -> Expr,
    ) -> Expr {
        let Some(depth) = Self::innermost_nullsafe(&chain) else {
            return wrap(chain);
        };
        let span = chain.span();
        let t = self.new_temp();
        let mut node = &mut chain;
        for _ in 0..depth {
            node = Self::base_mut(node);
        }
        if let Some(flag) = Self::nullsafe_mut(node) {
            *flag = false;
        }
        let base_slot = Self::base_mut(node);
        let base_span = base_slot.span();
        let base = std::mem::replace(base_slot, Expr::Temp(t, base_span));
        let rest = self.short_circuit(chain, on_null, wrap);
        Self::let_(
            t,
            base,
            Self::ternary(Self::is_null_check(t, span), on_null.clone(), rest, span),
            span,
        )
    }

    /// `true` if `e` has the shape [`short_circuit`](Self::short_circuit)
    /// produces: `Let(t, _, Ternary(Identical(Temp t, null), null, _))`.
    fn is_short_circuit_shape(e: &Expr) -> bool {
        let Expr::Let { temp, body, .. } = e else {
            return false;
        };
        let Expr::Ternary {
            cond,
            then: Some(then),
            ..
        } = &**body
        else {
            return false;
        };
        let Expr::Binary {
            op: BinOp::Identical,
            lhs,
            rhs,
            ..
        } = &**cond
        else {
            return false;
        };
        matches!(**lhs, Expr::Temp(t, _) if t == *temp)
            && matches!(**rhs, Expr::Null(_))
            && matches!(**then, Expr::Null(_))
    }

    /// `lhs ?? rhs` where `lhs` is a short-circuited chain: push the coalesce
    /// into the non-null branch and duplicate `rhs` into the null branch.
    fn push_coalesce(lhs: Expr, rhs: Expr, span: Span) -> Expr {
        if !Self::is_short_circuit_shape(&lhs) {
            return Expr::Binary {
                op: BinOp::Coalesce,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        let Expr::Let {
            temp,
            init,
            body,
            span: lspan,
        } = lhs
        else {
            unreachable!()
        };
        let Expr::Ternary {
            cond,
            else_,
            span: tspan,
            ..
        } = *body
        else {
            unreachable!()
        };
        let inner = Self::push_coalesce(*else_, rhs.clone(), span);
        Expr::Let {
            temp,
            init,
            body: Box::new(Expr::Ternary {
                cond,
                then: Some(Box::new(rhs)),
                else_: Box::new(inner),
                span: tspan,
            }),
            span: lspan,
        }
    }

    // ----- destructuring ---------------------------------------------------------

    fn pattern_has_ref(items: &[ArrayItem]) -> bool {
        items.iter().any(|it| {
            it.by_ref
                || matches!(&it.value, Some(Expr::Array { items, .. }) if Self::pattern_has_ref(items))
        })
    }

    /// Expand a pattern reading from `source` (an expression that is safe to
    /// duplicate: a `Temp` or a stabilized lvalue). Returns the assignments
    /// in order.
    fn destructure(&mut self, items: Vec<ArrayItem>, source: &Expr, out: &mut Vec<Expr>) {
        let mut pos: i64 = 0;
        for it in items {
            let span = it.span;
            let key = match it.key {
                Some(k) => k,
                None => {
                    let k = Expr::Int(pos, span);
                    pos += 1;
                    k
                }
            };
            let Some(value) = it.value else {
                // Skipped slot `[, $b]`.
                continue;
            };
            let elem = Self::index(source.clone(), key, span);
            match value {
                Expr::Array {
                    items: inner,
                    span: ispan,
                    ..
                } => {
                    if Self::pattern_has_ref(&inner) {
                        // Reference items need the element itself as their
                        // source: `elem` is stable (built from stable parts).
                        let mut seq = Vec::new();
                        self.destructure(inner, &elem, &mut seq);
                        seq.push(elem);
                        out.push(Expr::Seq(seq, ispan));
                    } else {
                        let t = self.new_temp();
                        let mut seq = Vec::new();
                        self.destructure(inner, &Self::temp(t, ispan), &mut seq);
                        seq.push(Self::temp(t, ispan));
                        out.push(Self::let_(t, elem, Expr::Seq(seq, ispan), ispan));
                    }
                }
                target => out.push(Self::assign(target, elem, it.by_ref, span)),
            }
        }
    }

    /// `[pattern] = value` as an expression.
    fn destructure_assign(&mut self, items: Vec<ArrayItem>, mut value: Expr, span: Span) -> Expr {
        if Self::pattern_has_ref(&items) {
            let mut binds = Vec::new();
            self.stabilize(&mut value, &mut binds);
            let mut seq = Vec::new();
            self.destructure(items, &value, &mut seq);
            seq.push(value);
            Self::wrap_lets(binds, Expr::Seq(seq, span), span)
        } else {
            let t = self.new_temp();
            let mut seq = Vec::new();
            self.destructure(items, &Self::temp(t, span), &mut seq);
            seq.push(Self::temp(t, span));
            Self::let_(t, value, Expr::Seq(seq, span), span)
        }
    }

    // ----- statements -------------------------------------------------------------

    fn nest_elseifs(s: &mut Stmt) {
        let Stmt::If { elseifs, else_, .. } = s else {
            return;
        };
        if elseifs.is_empty() {
            return;
        }
        let mut tail = else_.take();
        while let Some(ei) = elseifs.pop() {
            let inner = Stmt::If {
                cond: ei.cond,
                then: ei.body,
                elseifs: Vec::new(),
                else_: tail,
                span: ei.span,
            };
            tail = Some(vec![inner]);
        }
        *else_ = tail;
    }

    // ----- hooks -----------------------------------------------------------------------

    fn hook(&mut self, h: &mut Hook) {
        let (prop, ty) = match &self.cur_prop {
            Some((p, t)) => (*p, t.clone()),
            None => return,
        };
        if h.kind == HookKind::Set && h.params.is_none() {
            let value = self.it.intern(b"value");
            h.params = Some(vec![Param {
                attrs: Vec::new(),
                name: value,
                ty,
                default: None,
                by_ref: false,
                variadic: false,
                promote: None,
                hooks: Vec::new(),
                span: h.span,
            }]);
        }
        if let HookBody::Expr(_) = &h.body {
            let HookBody::Expr(e) = std::mem::replace(&mut h.body, HookBody::Abstract) else {
                unreachable!()
            };
            let span = e.span();
            let stmt = match h.kind {
                HookKind::Get => Stmt::Return {
                    value: Some(e),
                    span,
                },
                HookKind::Set => {
                    let this = self.it.intern(b"this");
                    let target = Expr::Prop {
                        obj: Box::new(Expr::Var(this, span)),
                        name: MemberName::Ident(prop, span),
                        nullsafe: false,
                        span,
                    };
                    Stmt::Expr {
                        expr: Self::assign(target, e, false, span),
                        span,
                    }
                }
            };
            h.body = HookBody::Block(vec![stmt]);
        }
    }

    fn call_with_temp(callee: Expr, t: TempId, span: Span) -> Expr {
        let arg = Arg {
            name: None,
            value: Expr::Temp(t, span),
            spread: false,
            span,
        };
        match callee {
            Expr::Callable { target, .. } => match target {
                CallableTarget::Func(c) => Expr::Call {
                    callee: c,
                    args: vec![arg],
                    span,
                },
                CallableTarget::Method { obj, name } => Expr::MethodCall {
                    obj,
                    name,
                    args: vec![arg],
                    nullsafe: false,
                    span,
                },
                CallableTarget::Static { class, name } => Expr::StaticCall {
                    class,
                    name,
                    args: vec![arg],
                    span,
                },
            },
            other => Expr::Call {
                callee: Callee::Expr(Box::new(other)),
                args: vec![arg],
                span,
            },
        }
    }
}

impl VisitorMut for Desugar<'_> {
    fn visit_stmt(&mut self, s: &mut Stmt) {
        Self::nest_elseifs(s);
        match s {
            Stmt::Foreach {
                subject,
                key,
                value,
                by_ref,
                body,
                span,
            } => {
                self.visit_expr(subject);
                if let Some(k) = key {
                    self.visit_expr(k);
                }
                if let Expr::Array { .. } = &**value {
                    let Expr::Array {
                        items, span: pspan, ..
                    } = std::mem::replace(&mut **value, Expr::Error(*span))
                    else {
                        unreachable!()
                    };
                    let t = self.new_temp();
                    if Self::pattern_has_ref(&items) {
                        *by_ref = true;
                    }
                    let mut seq = Vec::new();
                    self.destructure(items, &Self::temp(t, pspan), &mut seq);
                    seq.push(Self::temp(t, pspan));
                    **value = Expr::Temp(t, pspan);
                    body.insert(
                        0,
                        Stmt::Expr {
                            expr: Expr::Seq(seq, pspan),
                            span: pspan,
                        },
                    );
                } else {
                    self.visit_expr(value);
                }
                self.visit_block(body);
            }
            Stmt::Func(f) => {
                self.in_body(|d| {
                    for g in &mut f.attrs {
                        d.visit_attr_group(g);
                    }
                    for p in &mut f.params {
                        d.visit_param(p);
                    }
                    d.visit_block(&mut f.body);
                });
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &mut Expr) {
        if Self::is_chain_link(e) {
            self.visit_chain_children(e);
            let span = e.span();
            let chain = std::mem::replace(e, Expr::Error(span));
            *e = self.short_circuit(chain, &Expr::Null(span), &|c| c);
            return;
        }
        match e {
            Expr::Isset { vars, span } if vars.len() > 1 => {
                let span = *span;
                let mut parts = std::mem::take(vars).into_iter();
                let first = parts.next().expect("non-empty isset");
                let mut acc = Expr::Isset {
                    vars: vec![first],
                    span,
                };
                for v in parts {
                    acc = Expr::Binary {
                        op: BinOp::And,
                        lhs: Box::new(acc),
                        rhs: Box::new(Expr::Isset {
                            vars: vec![v],
                            span,
                        }),
                        span,
                    };
                }
                *e = acc;
                self.visit_expr(e);
                return;
            }
            Expr::Isset { vars, span } => {
                let span = *span;
                let v = &mut vars[0];
                if Self::is_chain_link(v) {
                    self.visit_chain_children(v);
                    let chain = std::mem::replace(v, Expr::Error(span));
                    *e = self.short_circuit(chain, &Expr::Bool(false, span), &|c| Expr::Isset {
                        vars: vec![c],
                        span,
                    });
                } else {
                    self.visit_expr(v);
                }
                return;
            }
            Expr::Empty { expr, span } => {
                let span = *span;
                if Self::is_chain_link(expr) {
                    self.visit_chain_children(expr);
                    let chain = std::mem::replace(&mut **expr, Expr::Error(span));
                    *e = self.short_circuit(chain, &Expr::Bool(true, span), &|c| Expr::Empty {
                        expr: Box::new(c),
                        span,
                    });
                } else {
                    self.visit_expr(expr);
                }
                return;
            }
            Expr::Closure(c) => {
                self.in_body(|d| d.visit_closure(c));
                return;
            }
            Expr::ArrowFn(f) => {
                self.in_body(|d| d.visit_arrow_fn(f));
                return;
            }
            _ => {}
        }
        walk_expr(self, e);
        let span = e.span();
        match e {
            Expr::Binary {
                op: BinOp::Coalesce,
                lhs,
                ..
            } if Self::is_short_circuit_shape(lhs) => {
                let Expr::Binary { lhs, rhs, .. } = std::mem::replace(e, Expr::Error(span)) else {
                    unreachable!()
                };
                *e = Self::push_coalesce(*lhs, *rhs, span);
            }
            Expr::Assign {
                op: Some(BinOp::Coalesce),
                ..
            } => {
                let Expr::Assign { target, value, .. } = std::mem::replace(e, Expr::Error(span))
                else {
                    unreachable!()
                };
                let mut target = *target;
                let mut binds = Vec::new();
                self.stabilize(&mut target, &mut binds);
                let body = Expr::Binary {
                    op: BinOp::Coalesce,
                    lhs: Box::new(target.clone()),
                    rhs: Box::new(Self::assign(target, *value, false, span)),
                    span,
                };
                *e = Self::wrap_lets(binds, body, span);
            }
            Expr::Assign {
                target,
                op: None,
                by_ref: false,
                ..
            } if matches!(**target, Expr::Array { .. }) => {
                let Expr::Assign { target, value, .. } = std::mem::replace(e, Expr::Error(span))
                else {
                    unreachable!()
                };
                let Expr::Array { items, .. } = *target else {
                    unreachable!()
                };
                *e = self.destructure_assign(items, *value, span);
            }
            Expr::Ternary { then: None, .. } => {
                let Expr::Ternary { cond, else_, .. } = std::mem::replace(e, Expr::Error(span))
                else {
                    unreachable!()
                };
                let t = self.new_temp();
                *e = Self::let_(
                    t,
                    *cond,
                    Self::ternary(Self::temp(t, span), Self::temp(t, span), *else_, span),
                    span,
                );
            }
            Expr::Binary {
                op: BinOp::Pipe, ..
            } => {
                let Expr::Binary { lhs, rhs, .. } = std::mem::replace(e, Expr::Error(span)) else {
                    unreachable!()
                };
                let t = self.new_temp();
                *e = Self::let_(t, *lhs, Self::call_with_temp(*rhs, t, span), span);
            }
            _ => {}
        }
    }

    fn visit_closure(&mut self, c: &mut Closure) {
        for g in &mut c.attrs {
            self.visit_attr_group(g);
        }
        for p in &mut c.params {
            self.visit_param(p);
        }
        self.visit_block(&mut c.body);
    }

    fn visit_arrow_fn(&mut self, f: &mut ArrowFn) {
        for g in &mut f.attrs {
            self.visit_attr_group(g);
        }
        for p in &mut f.params {
            self.visit_param(p);
        }
        self.visit_expr(&mut f.body);
    }

    fn visit_class_like(&mut self, c: &mut ClassLike) {
        let readonly = c.modifiers.readonly;
        for g in &mut c.attrs {
            self.visit_attr_group(g);
        }
        for m in &mut c.members {
            match m {
                Member::Prop(p) => {
                    if readonly {
                        p.modifiers.readonly = true;
                    }
                    let saved = self.cur_prop.take();
                    self.cur_prop = p.items.first().map(|i| (i.name, p.ty.clone()));
                    for g in &mut p.attrs {
                        self.visit_attr_group(g);
                    }
                    for it in &mut p.items {
                        if let Some(d) = &mut it.default {
                            self.visit_expr(d);
                        }
                    }
                    for h in &mut p.hooks {
                        self.visit_hook(h);
                    }
                    self.cur_prop = saved;
                }
                Member::Method(md) => {
                    self.in_body(|d| {
                        for g in &mut md.attrs {
                            d.visit_attr_group(g);
                        }
                        for p in &mut md.params {
                            if readonly {
                                if let Some(m) = &mut p.promote {
                                    m.readonly = true;
                                }
                            }
                            d.visit_param(p);
                        }
                        if let Some(b) = &mut md.body {
                            d.visit_block(b);
                        }
                    });
                }
                Member::Const(cm) => {
                    for g in &mut cm.attrs {
                        self.visit_attr_group(g);
                    }
                    for it in &mut cm.items {
                        self.visit_expr(&mut it.value);
                    }
                }
                Member::EnumCase(ec) => {
                    for g in &mut ec.attrs {
                        self.visit_attr_group(g);
                    }
                    if let Some(v) = &mut ec.value {
                        self.visit_expr(v);
                    }
                }
                Member::TraitUse(_) => {}
            }
        }
    }

    fn visit_param(&mut self, p: &mut Param) {
        for g in &mut p.attrs {
            self.visit_attr_group(g);
        }
        if let Some(d) = &mut p.default {
            self.visit_expr(d);
        }
        if !p.hooks.is_empty() {
            let saved = self.cur_prop.take();
            self.cur_prop = Some((p.name, p.ty.clone()));
            for h in &mut p.hooks {
                self.visit_hook(h);
            }
            self.cur_prop = saved;
        }
    }

    fn visit_hook(&mut self, h: &mut Hook) {
        self.hook(h);
        self.in_body(|d| {
            for g in &mut h.attrs {
                d.visit_attr_group(g);
            }
            if let Some(ps) = &mut h.params {
                for p in ps {
                    d.visit_param(p);
                }
            }
            match &mut h.body {
                HookBody::Expr(e) => d.visit_expr(e),
                HookBody::Block(b) => d.visit_block(b),
                HookBody::Abstract => {}
            }
        });
    }

    fn visit_modifiers(&mut self, _m: &mut Modifiers) {}
}
