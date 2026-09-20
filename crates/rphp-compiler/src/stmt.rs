//! Statement lowering.
//!
//! Lowered: `echo`, expression statements, `if`/`elseif`/`else`, `while`,
//! `do`/`while`, `for`, `foreach` (by value and by reference, with
//! destructuring targets — after the HIR pass a destructuring `foreach`
//! binds an HIR temporary and the pattern statement prepended to the body
//! reads it), `switch` (jump table or compare chain), `break N`/`continue
//! N`, `goto`/labels, `return`, `global`, `static`, `unset`, top-level
//! `const`, `declare(strict_types)`, blocks, inline HTML, and namespaces:
//! `namespace X;` / `namespace X { }` are transparent statement lists
//! (resolution already happened in `rphp-hir`, and declared names are
//! FQNs) and `use` imports are inert. Function and class declarations are
//! hoisted when PHP hoists them and lowered to `DeclareFunction` /
//! `DeclareClass` in place otherwise. `try`/`catch`/`finally` (E5) and
//! `declare(ticks)` are reported as `RPHP_E0300`, as is `unset(A::$p)`
//! (php raises a runtime `Error` there and the ISA has no op for it).

use rphp_ast::v2::{Case, Expr, Stmt};
use rphp_bytecode::{ClassId, Const, FuncId, InitRef, Op, Reg, StaticVar};
use rphp_diagnostics::Diagnostic;
use rphp_value::{Str, Value};

use crate::func::{FnCompiler, LoopCtx};
use crate::{unsupported, INVALID_BREAK};

impl FnCompiler<'_> {
    pub(crate) fn compile_stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.compile_stmt(s);
        }
    }

    /// Compile the body of a compound statement. Declarations inside it are
    /// conditional (PHP only hoists from the top-level statement list, plain
    /// blocks and global namespace bodies).
    pub(crate) fn compile_nested(&mut self, stmts: &[Stmt]) {
        let saved = self.at_top_level;
        self.at_top_level = false;
        self.compile_stmts(stmts);
        self.at_top_level = saved;
    }

    /// The ids of the loops enclosing the current point (outermost first).
    fn loop_ids(&self) -> Vec<u32> {
        self.loops.iter().map(|l| l.id).collect()
    }

    fn push_loop(&mut self, is_switch: bool) {
        let id = self.next_loop_id;
        self.next_loop_id += 1;
        let finally_depth = self.finallys.len();
        self.loops.push(LoopCtx {
            id,
            breaks: Vec::new(),
            continues: Vec::new(),
            is_switch,
            finally_depth,
        });
    }

    /// Close the innermost loop: patch its `break`s to `break_to` and its
    /// `continue`s to `continue_to`.
    fn pop_loop(&mut self, break_to: u32, continue_to: u32) {
        let l = self.loops.pop().expect("loop stack underflow");
        for b in l.breaks {
            self.patch(b, break_to);
        }
        for c in l.continues {
            self.patch(c, continue_to);
        }
    }

    pub(crate) fn compile_stmt(&mut self, s: &Stmt) {
        self.mark_line(s.span());
        // A statement that branches makes the linear walk say nothing about
        // what ran, so what is known to be assigned is forgotten around it.
        // Erring this way only costs a comparison at run time.
        if matches!(
            s,
            Stmt::If { .. }
                | Stmt::While { .. }
                | Stmt::DoWhile { .. }
                | Stmt::For { .. }
                | Stmt::Foreach { .. }
                | Stmt::Switch { .. }
                | Stmt::Try { .. }
                | Stmt::Goto { .. }
                | Stmt::Label { .. }
        ) {
            self.forget_assigned();
        }
        let branching = matches!(
            s,
            Stmt::If { .. }
                | Stmt::While { .. }
                | Stmt::DoWhile { .. }
                | Stmt::For { .. }
                | Stmt::Foreach { .. }
                | Stmt::Switch { .. }
                | Stmt::Try { .. }
        );
        match s {
            Stmt::Echo { args, .. } => {
                for a in args {
                    let mark = self.temp_top;
                    let r = self.compile_expr(a);
                    self.emit(Op::Echo { src: r });
                    self.free_to(mark);
                }
            }
            Stmt::InlineHtml { text, .. } => {
                // Text outside the PHP tags is echoed verbatim.
                let k = self.push_const(Const::Str(Str::new(self.interner().resolve(*text))));
                let mark = self.temp_top;
                let dst = self.alloc_temp();
                self.emit(Op::LoadConst { dst, k });
                self.emit(Op::Echo { src: dst });
                self.free_to(mark);
            }
            Stmt::Expr { expr, .. } => {
                let mark = self.temp_top;
                self.compile_expr_discard(expr);
                self.free_to(mark);
            }
            Stmt::If {
                cond,
                then,
                elseifs,
                else_,
                ..
            } => self.compile_if(cond, then, elseifs, else_.as_deref()),
            Stmt::While { cond, body, .. } => {
                let ltop = self.here();
                let mark = self.temp_top;
                let rc = self.compile_expr(cond);
                let jf = self.emit(Op::JmpIfFalse {
                    cond: rc,
                    target: 0,
                });
                self.free_to(mark);
                self.push_loop(false);
                self.compile_nested(body);
                self.emit(Op::Jmp { target: ltop });
                let lend = self.here();
                self.patch(jf, lend);
                self.pop_loop(lend, ltop);
            }
            Stmt::DoWhile { body, cond, .. } => {
                let ltop = self.here();
                self.push_loop(false);
                self.compile_nested(body);
                let lcond = self.here();
                self.mark_line(cond.span());
                let mark = self.temp_top;
                let rc = self.compile_expr(cond);
                self.emit(Op::JmpIfTrue {
                    cond: rc,
                    target: ltop,
                });
                self.free_to(mark);
                let lend = self.here();
                self.pop_loop(lend, lcond);
            }
            Stmt::For {
                init,
                cond,
                step,
                body,
                ..
            } => {
                for e in init {
                    let mark = self.temp_top;
                    self.compile_expr_discard(e);
                    self.free_to(mark);
                }
                let ltop = self.here();
                let mut jf = None;
                if let Some((last, rest)) = cond.split_last() {
                    let mark = self.temp_top;
                    for e in rest {
                        self.compile_expr_discard(e);
                        self.free_to(mark);
                    }
                    let rc = self.compile_expr(last);
                    jf = Some(self.emit(Op::JmpIfFalse {
                        cond: rc,
                        target: 0,
                    }));
                    self.free_to(mark);
                }
                self.push_loop(false);
                self.compile_nested(body);
                let lstep = self.here();
                for e in step {
                    self.mark_line(e.span());
                    let mark = self.temp_top;
                    self.compile_expr_discard(e);
                    self.free_to(mark);
                }
                self.emit(Op::Jmp { target: ltop });
                let lend = self.here();
                if let Some(jf) = jf {
                    self.patch(jf, lend);
                }
                self.pop_loop(lend, lstep);
            }
            Stmt::Foreach {
                subject,
                key,
                value,
                by_ref,
                body,
                ..
            } => self.compile_foreach(subject, key.as_deref(), value, *by_ref, body),
            Stmt::Switch { subject, cases, .. } => self.compile_switch(subject, cases),
            Stmt::Break { levels, span } | Stmt::Continue { levels, span } => {
                let is_break = matches!(s, Stmt::Break { .. });
                let n = (*levels).max(1) as usize;
                if n > self.loops.len() {
                    let what = if is_break { "break" } else { "continue" };
                    let msg = if self.loops.is_empty() {
                        format!("'{what}' not in the 'loop' or 'switch' context")
                    } else {
                        format!(
                            "Cannot '{what}' {n} level{} (only {} level{} available)",
                            if n == 1 { "" } else { "s" },
                            self.loops.len(),
                            if self.loops.len() == 1 { "" } else { "s" }
                        )
                    };
                    self.diags.push(
                        Diagnostic::error(INVALID_BREAK, msg).with_primary(*span, "invalid level"),
                    );
                    return;
                }
                let idx = self.loops.len() - n;
                if self.exit_loop_via_finally(idx, is_break) {
                    return;
                }
                let j = self.jmp_fwd();
                let target = &mut self.loops[idx];
                // `continue` targeting a `switch` behaves like `break` (PHP
                // warns at compile time; the behaviour is the same).
                if is_break || target.is_switch {
                    target.breaks.push(j);
                } else {
                    target.continues.push(j);
                }
            }
            Stmt::Return { value, .. } => self.compile_return(value.as_ref()),
            Stmt::Block { body, .. } => self.compile_stmts(body),
            Stmt::Nop { .. } => {}
            Stmt::Goto { label, span } => {
                if !self.check_goto_finally(*span) {
                    return;
                }
                let j = self.jmp_fwd();
                let loops = self.loop_ids();
                self.gotos.push((j, *label, loops, *span));
            }
            Stmt::Label { name, span } => {
                let addr = self.here();
                let loops = self.loop_ids();
                if self.labels.insert(*name, (addr, loops)).is_some() {
                    self.diags.push(
                        Diagnostic::error(
                            crate::UNDEFINED_LABEL,
                            format!(
                                "Label '{}' already defined",
                                self.interner().resolve_lossy(*name)
                            ),
                        )
                        .with_primary(*span, "duplicate label"),
                    );
                }
            }
            // Top-level declarations were hoisted by the driver and emit no
            // code; a declaration anywhere else is conditional and is declared
            // when the statement executes.
            Stmt::Func(f) => {
                if !self.at_top_level {
                    let idx = crate::compile_nested_function(self.mx, self.diags, f);
                    self.emit(Op::DeclareFunction { idx });
                }
            }
            Stmt::ClassLike(c) => {
                // A top-level class php cannot bind early (`implements`,
                // traits, enums) is declared in statement order too.
                if !self.at_top_level || !crate::class_is_hoisted(self.mx, c) {
                    let idx = crate::class::compile_class(self.mx, self.diags, c);
                    if let Some(idx) = idx {
                        self.emit(Op::DeclareClass { idx });
                    }
                }
            }
            Stmt::Namespace { body, .. } => {
                // `namespace X;` / `namespace X { }` / the global wrapper:
                // a transparent statement list at the same level (php hoists
                // from namespace bodies), names already resolved.
                self.compile_stmts(body);
            }
            // Imports were consumed by the resolver.
            Stmt::Use { .. } => {}
            Stmt::Global { vars, .. } => {
                for v in vars {
                    match v {
                        Expr::Var(id, _) => {
                            let reg = self.var_reg(*id);
                            let name = self.name_const(*id);
                            self.emit(Op::BindGlobal { reg, name });
                            self.mark_assigned(*id);
                        }
                        other => unsupported(self.diags, other.span(), "global with a variable variable"),
                    }
                }
            }
            Stmt::StaticVar { vars, .. } => {
                for v in vars {
                    let reg = self.var_reg(v.name);
                    let idx = self.statics.len() as u16;
                    let name: Box<[u8]> = self.interner().resolve(v.name).into();
                    // A literal initializer is a pool constant bound in one
                    // op; anything else is evaluated inline in this scope
                    // (php 8.3), guarded so it runs once.
                    let literal = v
                        .init
                        .as_ref()
                        .and_then(|e| crate::class::const_default(e, self.interner()))
                        .filter(|val| !matches!(val, Value::Array(_)));
                    match (&v.init, literal) {
                        (None, _) => {
                            self.statics.push(StaticVar { name, reg, init: None });
                            self.emit(Op::BindStatic { reg, idx });
                        }
                        (Some(_), Some(val)) => {
                            let k = self.push_const(match val {
                                Value::Null => Const::Null,
                                Value::Bool(b) => Const::Bool(b),
                                Value::Int(i) => Const::Int(i),
                                Value::Float(f) => Const::Float(f),
                                Value::Str(s) => Const::Str(s),
                                _ => unreachable!("filtered above"),
                            });
                            self.statics.push(StaticVar {
                                name,
                                reg,
                                init: Some(InitRef::Const(k)),
                            });
                            self.emit(Op::BindStatic { reg, idx });
                        }
                        (Some(e), None) => {
                            self.statics.push(StaticVar { name, reg, init: None });
                            let j1 = self.emit(Op::BindStaticOrJmp { reg, idx, target: 0 });
                            let mark = self.temp_top;
                            let val = self.compile_expr(e);
                            let j2 = self.emit(Op::BindStaticOrJmp { reg, idx, target: 0 });
                            self.emit(Op::BindStatic { reg, idx });
                            self.emit(Op::AssignThroughRef { dst: reg, src: val });
                            self.free_to(mark);
                            let lend = self.here();
                            self.patch(j1, lend);
                            self.patch(j2, lend);
                        }
                    }
                }
            }
            Stmt::Unset { targets, .. } => {
                for t in targets {
                    let mark = self.temp_top;
                    self.compile_unset(t);
                    self.free_to(mark);
                }
            }
            Stmt::ConstDecl { items, .. } => {
                for item in items {
                    let mark = self.temp_top;
                    let src = self.compile_expr(&item.value);
                    let name = self.sym_const(self.interner().resolve(item.name));
                    self.emit(Op::DeclareConst { name, src });
                    self.free_to(mark);
                }
            }
            Stmt::Declare {
                directives,
                body,
                span,
            } => {
                for d in directives {
                    let name = self.interner().resolve(d.name);
                    if !name.eq_ignore_ascii_case(b"strict_types") {
                        unsupported(self.diags, *span, "declare directive other than strict_types");
                    }
                }
                if let Some(body) = body {
                    self.compile_nested(body);
                }
            }
            Stmt::Try {
                body,
                catches,
                finally,
                ..
            } => self.compile_try(body, catches, finally.as_deref()),
            Stmt::HaltCompiler { span } => unsupported(self.diags, *span, "__halt_compiler"),
        }
        // Whatever the body assigned was assigned *conditionally*.
        if branching {
            self.forget_assigned();
        }
    }

    /// `if (cond) then [elseif (c) body]* [else body]`. Each `elseif` is
    /// lowered as an `if` nested in the previous branch's `else`.
    fn compile_if(
        &mut self,
        cond: &Expr,
        then: &[Stmt],
        elseifs: &[rphp_ast::v2::ElseIf],
        else_: Option<&[Stmt]>,
    ) {
        let mark = self.temp_top;
        let rc = self.compile_expr(cond);
        let jf = self.emit(Op::JmpIfFalse {
            cond: rc,
            target: 0,
        });
        self.free_to(mark);
        self.compile_nested(then);
        let has_else = !elseifs.is_empty() || else_.is_some();
        if !has_else {
            let lend = self.here();
            self.patch(jf, lend);
            return;
        }
        let jend = self.jmp_fwd();
        let lelse = self.here();
        self.patch(jf, lelse);
        match elseifs.split_first() {
            Some((first, rest)) => {
                self.mark_line(first.span);
                self.compile_if(&first.cond, &first.body, rest, else_);
            }
            None => self.compile_nested(else_.unwrap_or(&[])),
        }
        let lend = self.here();
        self.patch(jend, lend);
    }

    /// `foreach (subject as [key =>] [&]value) body`.
    fn compile_foreach(
        &mut self,
        subject: &Expr,
        key: Option<&Expr>,
        value: &Expr,
        by_ref: bool,
        body: &[Stmt],
    ) {
        let mark = self.temp_top;
        // The iterated container: for a by-reference loop over a variable,
        // element or property the register must alias the container so the
        // loop mutates it in place.
        let src = if by_ref {
            match subject {
                Expr::Var(id, _) if !self.is_this(*id) => self.var_reg(*id),
                Expr::Index {
                    base,
                    index: Some(index),
                    ..
                } => {
                    let arr = self.compile_expr(base);
                    let key = self.compile_expr(index);
                    let dst = self.alloc_temp();
                    self.emit(Op::RefElem { dst, arr, key: Some(key) });
                    dst
                }
                Expr::Prop {
                    obj,
                    name,
                    nullsafe: false,
                    ..
                } => {
                    let obj = self.compile_expr(obj);
                    let name = self.member_name_ref(name);
                    let dst = self.alloc_temp();
                    self.emit(Op::RefProp { dst, obj, name });
                    dst
                }
                Expr::StaticProp { class, name, span } => {
                    match self.static_prop_ref(class, name, *span) {
                        Some((class, name)) => {
                            let dst = self.alloc_temp();
                            self.emit(Op::RefStaticProp { dst, class, name });
                            dst
                        }
                        None => self.null_temp(),
                    }
                }
                other => self.compile_expr(other),
            }
        } else {
            self.compile_expr(subject)
        };
        let it = self.alloc_temp();
        self.emit(Op::IterInit { it, src, by_ref });
        // Simple variable targets receive the element directly; an HIR
        // temporary (a desugared destructuring pattern) is *bound* to a fresh
        // register the pattern statement in the body reads; anything else
        // goes through a temp and an assignment.
        let mut bound_temp = None;
        let val_direct = match value {
            Expr::Var(id, _) if !self.is_this(*id) => Some(self.var_reg(*id)),
            Expr::Temp(t, _) => {
                let r = self.alloc_temp();
                self.temps.push((*t, r));
                bound_temp = Some(*t);
                Some(r)
            }
            _ => None,
        };
        let key_direct = match key {
            None => None,
            Some(Expr::Var(id, _)) if !self.is_this(*id) => Some(Some(self.var_reg(*id))),
            Some(_) => Some(None),
        };
        let val_reg = val_direct.unwrap_or_else(|| self.alloc_temp());
        let key_reg = match key_direct {
            None => None,
            Some(Some(r)) => Some(r),
            Some(None) => Some(self.alloc_temp()),
        };
        let ltop = self.here();
        let next = self.emit(Op::IterNext {
            it,
            key: key_reg,
            val: val_reg,
            target: 0,
        });
        let body_mark = self.temp_top;
        if val_direct.is_none() {
            if by_ref {
                unsupported(self.diags, value.span(), "foreach by reference into a non-variable");
            }
            self.assign_to_target(value, val_reg);
            self.free_to(body_mark);
        }
        if let (Some(k), Some(None)) = (key, key_direct) {
            let kr = key_reg.expect("key register");
            self.assign_to_target(k, kr);
            self.free_to(body_mark);
        }
        self.push_loop(false);
        self.compile_nested(body);
        self.emit(Op::Jmp { target: ltop });
        let lend = self.here();
        self.patch(next, lend);
        self.emit(Op::IterFree { it });
        self.pop_loop(lend, ltop);
        if let Some(t) = bound_temp {
            self.unbind_temp(t);
        }
        self.free_to(mark);
    }

    /// `switch (subject) { case ...: ... default: ... }`: a `Switch` jump
    /// table when every case value is a literal, else a compare chain; bodies
    /// fall through in source order.
    fn compile_switch(&mut self, subject: &Expr, cases: &[Case]) {
        let mark = self.temp_top;
        let subj = self.compile_expr(subject);
        let all_literal = cases
            .iter()
            .all(|c| c.cond.as_ref().is_none_or(|e| literal_value(e, self.interner()).is_some()));
        // (case index → jump to patch) for the compare chain, or the jump table.
        let mut chain: Vec<(usize, usize)> = Vec::new();
        let mut table: Option<(u32, usize)> = None;
        let default_idx = cases.iter().position(|c| c.cond.is_none());
        if all_literal {
            let rows: Vec<(Value, u32)> = cases
                .iter()
                .filter_map(|c| c.cond.as_ref())
                .map(|e| (literal_value(e, self.interner()).expect("literal"), 0))
                .collect();
            let k = self.push_const(Const::JumpTable(rows));
            let op = self.emit(Op::Switch {
                src: subj,
                table: k,
                default: 0,
                strict: false,
            });
            table = Some((k, op));
        } else {
            for (i, c) in cases.iter().enumerate() {
                let Some(cond) = &c.cond else { continue };
                let m = self.temp_top;
                let cr = self.compile_expr(cond);
                let t = self.alloc_temp();
                self.emit(Op::CmpEq {
                    dst: t,
                    a: subj,
                    b: cr,
                });
                let j = self.emit(Op::JmpIfTrue { cond: t, target: 0 });
                self.free_to(m);
                chain.push((i, j));
            }
        }
        let jdefault = self.jmp_fwd();
        self.push_loop(true);
        let mut starts = Vec::with_capacity(cases.len());
        for c in cases {
            starts.push(self.here());
            self.compile_nested(&c.body);
        }
        let lend = self.here();
        // Resolve the dispatch targets.
        match table {
            Some((k, op)) => {
                let mut row = 0;
                if let Const::JumpTable(rows) = &mut self.consts[k as usize] {
                    for (i, c) in cases.iter().enumerate() {
                        if c.cond.is_some() {
                            rows[row].1 = starts[i];
                            row += 1;
                        }
                    }
                }
                let _ = op;
            }
            None => {
                for (i, j) in chain {
                    self.patch(j, starts[i]);
                }
            }
        }
        let default_target = default_idx.map_or(lend, |i| starts[i]);
        self.patch(jdefault, default_target);
        if let Some((_, op)) = table {
            self.patch(op, default_target);
        }
        self.pop_loop(lend, lend);
        self.free_to(mark);
    }

    /// `unset(target)`.
    fn compile_unset(&mut self, target: &Expr) {
        match target {
            Expr::Var(id, _) if !self.is_this(*id) => {
                let var = self.var_reg(*id);
                self.emit(Op::UnsetVar { var });
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                if let Expr::Var(id, _) = &**base {
                    if self.is_globals(*id) {
                        unsupported(self.diags, target.span(), "unset of a $GLOBALS entry");
                        return;
                    }
                    if !self.is_this(*id) {
                        let arr = self.var_reg(*id);
                        let key = self.compile_expr(index);
                        self.emit(Op::UnsetElem { arr, key });
                        return;
                    }
                }
                // Nested: only descend when the container exists, so the
                // unset never autovivifies the path.
                let plan = self.plan_chain(base);
                let key = self.compile_expr(index);
                let Some(plan) = plan else { return };
                let c = self.alloc_temp();
                let probe = self.plan_read_quiet(&plan);
                self.emit(Op::IssetVar { dst: c, var: probe });
                let jf = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
                let (handle, wbs) = self.plan_fetch_w(&plan);
                self.emit(Op::UnsetElem { arr: handle, key });
                self.emit_writebacks(wbs);
                let lend = self.here();
                self.patch(jf, lend);
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let obj = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                self.emit(Op::UnsetProp { obj, name });
            }
            // php raises `Error: Attempt to unset static property C::$p` at
            // run time, naming the *resolved* class (`unset(B::$p)` on a
            // property declared in `A` says `B`), so the resolution has to
            // happen there rather than here.
            Expr::StaticProp { class, name, span } => {
                let mark = self.temp_top;
                if let Some((class, name)) = self.static_prop_ref(class, name, *span) {
                    self.emit(Op::UnsetStaticProp { class, name });
                }
                self.free_to(mark);
            }
            other => unsupported(self.diags, other.span(), "unset target"),
        }
    }
}

/// The value of a scalar literal (`switch`/`match` jump tables).
pub(crate) fn literal_value(e: &Expr, interner: &rphp_intern::Interner) -> Option<Value> {
    Some(match e {
        Expr::Null(_) => Value::Null,
        Expr::Bool(b, _) => Value::Bool(*b),
        Expr::Int(i, _) => Value::Int(*i),
        Expr::Float(f, _) => Value::Float(*f),
        Expr::Str(id, _) => Value::Str(Str::new(interner.resolve(*id))),
        Expr::Unary {
            op: rphp_ast::v2::UnOp::Neg,
            expr,
            ..
        } => match literal_value(expr, interner)? {
            Value::Int(i) => Value::Int(i.checked_neg()?),
            Value::Float(f) => Value::Float(-f),
            _ => return None,
        },
        _ => return None,
    })
}

// Silence unused-import lints for items only some cfgs use.
#[allow(dead_code)]
fn _unused(_: ClassId, _: FuncId, _: InitRef, _: Reg) {}
