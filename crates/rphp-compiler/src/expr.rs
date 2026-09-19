//! Expression lowering.
//!
//! [`FnCompiler::compile_expr`] returns the register holding the value: a
//! fresh temporary or, for a plain variable read, the variable's own
//! register. Callers bracket an expression with `let mark = self.temp_top;
//! … self.free_to(mark)` once they have consumed the result. Every
//! construct outside the lowered slice is reported as `RPHP_E0300` and yields
//! a `null` temporary so lowering can continue.
//!
//! Write targets go through one path ([`FnCompiler::compile_write`]): the
//! container chain is *planned* first (root register and every key/object
//! evaluated in source order, [`Plan`]), then the value, then the
//! fetch-for-write chain (`FetchElemW`/`FetchPropW`, which move the element
//! out), the innermost store, and the write-backs in reverse
//! (`ArraySet`/`ArrayPush`/`AssignProp`) — see CONTRACT.md §8.
//!
//! HIR temporaries (`rphp-hir`'s `Let`/`Temp`/`Seq`, the only nodes the
//! parser never produces): `Let` evaluates its initializer into a fresh
//! register that stays allocated while the body compiles (the temporary
//! stack discipline keeps every temp of the body above it), `Temp` reads
//! that register — as a value, as a write-target root (`Temp(t)->p = v`,
//! `&Temp(t)[k]`) or as a destructuring source, where `Temp(t)[k]` is a
//! `ListGet` — and `Seq` evaluates in order and yields the last value.
//!
//! Names are emitted from the resolver's [`Resolved`] entries: class
//! positions carry the static FQN; an unqualified function/constant name
//! inside a namespace carries both candidates of php's two-step lookup
//! (`InitFCall{name: N\f, ns_fallback: f}` — the runtime tries `name`
//! first).
//!
//! Class members (E6) are all late-bound: the class part of `A::$p`,
//! `A::C`, `A::m()`, `new A`, `$x instanceof A` becomes a
//! [`ClassRef`] operand, never a resolved [`ClassId`](rphp_bytecode::ClassId),
//! and `static::` is emitted as [`ClassRef::STATIC`] so the runtime resolves
//! it against the frame's called scope — folding it here would break late
//! static binding. A static property is a shared cell, so every position
//! it can appear in is covered by four ops: `FetchStaticProp` (read),
//! `AssignStaticProp` (write), `RefStaticProp` (bind the cell — used for
//! `&A::$p`, for `A::$a['k'] = 1`, for a by-reference argument and for a
//! by-reference `foreach`) and `IssetStaticProp`. There is no
//! `AssignOpStaticProp`, `EmptyStaticProp` or `UnsetStaticProp`: compound
//! assignment and `++`/`--` are lowered read-modify-write, `empty()` as
//! `!isset || !value`, and `unset(A::$p)` — always an `Error` in php — is
//! reported as `RPHP_E0300`.

use rphp_ast::v2::{
    Arg, ArrayItem, BinOp, CallableTarget, Callee, CastKind as AstCast, ClassRef as AstClassRef,
    ConstSel, Expr, InterpPart, MagicKind, MatchArm, MemberName, Name, NewTarget, Resolved,
    TempId, UnOp,
};
use rphp_bytecode::{
    AssignOpKind, CastKind, ClassRef, Const, IncludeKind as BcInclude, NameRef, Op, Reg,
};
use rphp_diagnostics::Diagnostic;
use rphp_intern::IdentId;
use rphp_span::Span;
use rphp_value::{Str, Value};

use crate::func::{FnCompiler, NullsafeCtx};
use crate::stmt::literal_value;
use crate::{unsupported, INVALID_APPEND_READ, INVALID_SCOPE, INVALID_WRITE_TARGET};

/// One step of a write-target chain below its root.
pub(crate) enum Step {
    /// `[key]` (`None` = `[]`).
    Elem(Option<Reg>),
    /// `->name`.
    Prop(NameRef),
}

/// A planned write-target container: the root register and the steps to
/// the container being written, with every key / object already evaluated.
pub(crate) struct Plan {
    pub(crate) root: Reg,
    pub(crate) steps: Vec<Step>,
}

/// A pending write-back after a nested store.
pub(crate) enum Wb {
    Elem {
        arr: Reg,
        key: Option<Reg>,
        val: Reg,
    },
    Prop {
        obj: Reg,
        name: NameRef,
        val: Reg,
    },
}

/// Where an assignment's value comes from: an expression still to be
/// evaluated (at the point PHP evaluates it) or an already-computed register.
#[derive(Clone, Copy)]
pub(crate) enum ValueSrc<'e> {
    Expr(&'e Expr),
    Reg(Reg),
}

impl FnCompiler<'_> {
    /// Compile `e`, returning the register that holds its value.
    pub(crate) fn compile_expr(&mut self, e: &Expr) -> Reg {
        self.mark_line(e.span());
        // A nullsafe chain is bracketed once at its outermost link.
        if !self.in_nullsafe && chain_has_nullsafe(e) {
            return self.compile_nullsafe_chain(e);
        }
        match e {
            Expr::Null(_) => self.null_temp(),
            Expr::Bool(b, _) => {
                let dst = self.alloc_temp();
                self.emit(Op::LoadBool { dst, val: *b });
                dst
            }
            Expr::Int(i, _) => self.load_const(Const::Int(*i)),
            Expr::Float(f, _) => self.load_const(Const::Float(*f)),
            Expr::Str(id, _) => self.load_str(*id),
            Expr::Interp { parts, .. } => self.compile_interp(parts),
            Expr::Var(id, _) => {
                if self.is_this(*id) {
                    self.load_this()
                } else if self.is_globals(*id) {
                    let dst = self.alloc_temp();
                    self.emit(Op::FetchGlobals { dst });
                    dst
                } else {
                    self.read_var(*id)
                }
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let dst = self.alloc_temp();
                self.emit(Op::FetchDynVar {
                    dst,
                    name: n,
                    global: false,
                });
                dst
            }
            Expr::Assign {
                target,
                value,
                op,
                by_ref,
                span,
            } => {
                if *by_ref {
                    return self.compile_assign_ref(target, value, *span);
                }
                match op {
                    Some(BinOp::Coalesce) => self.compile_coalesce_assign(target, value, true),
                    Some(op) => self.compile_compound(target, *op, value, true),
                    None => self.compile_write(target, ValueSrc::Expr(value), true),
                }
            }
            Expr::Unary { op, expr, span } => self.compile_unary(*op, expr, *span, true),
            Expr::Binary { op, lhs, rhs, span } => match op {
                BinOp::And => self.compile_and(lhs, rhs),
                BinOp::Or => self.compile_or(lhs, rhs),
                BinOp::Xor => {
                    let mark = self.temp_top;
                    let a = self.compile_expr(lhs);
                    let b = self.compile_expr(rhs);
                    let ta = self.alloc_temp();
                    let tb = self.alloc_temp();
                    self.emit(Op::Not { dst: ta, src: a });
                    self.emit(Op::Not { dst: tb, src: b });
                    self.free_to(mark);
                    let dst = self.alloc_temp();
                    self.emit(Op::CmpNotIdentical { dst, a: ta, b: tb });
                    dst
                }
                BinOp::Coalesce => self.compile_coalesce(lhs, rhs),
                BinOp::Pipe => self.unsupported_expr(*span, "pipe operator"),
                _ => {
                    let make = binary_op(*op).expect("every remaining operator lowers");
                    let mark = self.temp_top;
                    let a = self.compile_expr(lhs);
                    let b = self.compile_expr(rhs);
                    self.free_to(mark);
                    let dst = self.alloc_temp();
                    self.emit(make(dst, a, b));
                    dst
                }
            },
            Expr::Call { callee, args, span } => match callee {
                Callee::Name(name) => self.compile_call(name, args, *span),
                Callee::Expr(callee) => self.compile_dynamic_call(callee, args),
            },
            Expr::Closure(c) => self.compile_closure_expr(c),
            Expr::ArrowFn(f) => self.compile_arrow_fn(f),
            Expr::Array {
                items,
                syntax,
                span,
            } => {
                if syntax.is_list() {
                    return self.unsupported_expr(*span, "list() outside a write context");
                }
                self.compile_array(items)
            }
            Expr::Index { base, index, span } => {
                self.compile_index_read(base, index.as_deref(), *span)
            }
            Expr::New { class, args, span } => match class {
                NewTarget::Ref(class) => {
                    let line = self.cur_line;
                    let Some(class) = self.class_ref(class, *span) else {
                        return self.null_temp();
                    };
                    let ic = self.ic();
                    self.emit(Op::InitNew { class, ic });
                    self.compile_sends(args);
                    self.emit_do_call(line)
                }
                NewTarget::Anon(c) => {
                    // php declares the class when the expression runs, and
                    // reuses that one entry every time the site runs again.
                    let Some(idx) = crate::class::compile_class(self.mx, self.diags, c) else {
                        return self.null_temp();
                    };
                    self.emit(Op::DeclareClass { idx });
                    let name = *self
                        .mx
                        .anon_names
                        .get(&(c.as_ref() as *const rphp_ast::v2::ClassLike))
                        .expect("anonymous class named by the pre-pass");
                    let k = self.sym_const(self.interner().resolve(name));
                    let ic = self.ic();
                    let line = self.cur_line;
                    self.emit(Op::InitNew {
                        class: ClassRef::named(k),
                        ic,
                    });
                    self.compile_sends(args);
                    self.emit_do_call(line)
                }
            },
            Expr::Prop {
                obj,
                name,
                nullsafe,
                ..
            } => {
                let mark = self.temp_top;
                let obj_reg = self.compile_chain_obj(obj);
                if *nullsafe {
                    self.nullsafe_check(obj_reg);
                }
                let name = self.member_name_ref(name);
                self.free_to(mark);
                let dst = self.alloc_temp();
                let ic = self.ic();
                self.emit(Op::FetchProp {
                    dst,
                    obj: obj_reg,
                    name,
                    ic,
                });
                dst
            }
            Expr::StaticProp { class, name, span } => {
                let mark = self.temp_top;
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return self.null_temp();
                };
                self.free_to(mark);
                let dst = self.alloc_temp();
                let ic = self.ic();
                self.emit(Op::FetchStaticProp {
                    dst,
                    class,
                    name,
                    ic,
                });
                dst
            }
            Expr::MethodCall {
                obj,
                name,
                args,
                nullsafe,
                ..
            } => {
                let mark = self.temp_top;
                let obj_reg = self.compile_chain_obj(obj);
                if *nullsafe {
                    self.nullsafe_check(obj_reg);
                }
                let name = self.member_name_ref(name);
                let ic = self.ic();
                let line = self.cur_line;
                self.emit(Op::InitMethodCall {
                    obj: obj_reg,
                    name,
                    ic,
                });
                self.free_to(mark);
                self.compile_sends(args);
                self.emit_do_call(line)
            }
            Expr::StaticCall {
                class,
                name,
                args,
                span,
            } => {
                let mark = self.temp_top;
                let Some(class) = self.class_ref(class, *span) else {
                    return self.null_temp();
                };
                let name = self.member_name_ref(name);
                let ic = self.ic();
                let line = self.cur_line;
                self.emit(Op::InitStaticCall { class, name, ic });
                self.free_to(mark);
                self.compile_sends(args);
                self.emit_do_call(line)
            }
            Expr::InstanceOf { expr, class, span } => {
                let mark = self.temp_top;
                let obj = self.compile_expr(expr);
                let Some(class) = self.class_ref(class, *span) else {
                    return self.null_temp();
                };
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::InstanceOfRef { dst, obj, class });
                dst
            }
            Expr::Const(name) => {
                let (k, ns_fallback) = match name.resolved {
                    Some(Resolved::Const { ns_key, global_key }) => {
                        self.two_step_consts(ns_key, global_key)
                    }
                    // Not visited by the resolver: as spelled.
                    _ => (self.sym_const(self.interner().resolve(name.text)), None),
                };
                let dst = self.alloc_temp();
                self.emit(Op::FetchConst {
                    dst,
                    name: k,
                    ns_fallback,
                });
                dst
            }
            Expr::MagicConst { kind, span } => self.compile_magic_const(*kind, *span),
            Expr::ClassConst { class, name, span } => match name {
                ConstSel::Class(_) => self.compile_class_name(class, *span),
                // `A::C`, `self::C`, `static::C`, `$cls::C`, `$obj::C` and
                // `A::{$expr}` (8.3) — and enum case access (`Suit::Hearts`),
                // which is spelled exactly like a constant: whether a name is
                // a constant or a case is the runtime's decision.
                ConstSel::Ident(..) | ConstSel::Expr(_) => {
                    self.compile_class_const(class, name, *span)
                }
            },
            Expr::Ternary {
                cond, then, else_, ..
            } => self.compile_ternary(cond, then.as_deref(), else_),
            Expr::Isset { vars, .. } => self.compile_isset(vars),
            Expr::Empty { expr, .. } => self.compile_empty(expr),
            Expr::Exit { arg, .. } => {
                let mark = self.temp_top;
                let src = arg.as_ref().map(|a| self.compile_expr(a));
                self.emit(Op::Exit { src });
                self.free_to(mark);
                self.null_temp()
            }
            Expr::Print { expr, .. } => {
                let mark = self.temp_top;
                let r = self.compile_expr(expr);
                self.emit(Op::Echo { src: r });
                self.free_to(mark);
                self.load_const(Const::Int(1))
            }
            Expr::Include { kind, path, .. } => {
                let mark = self.temp_top;
                let p = self.compile_expr(path);
                self.free_to(mark);
                let dst = self.alloc_temp();
                let kind = match kind {
                    rphp_ast::v2::IncludeKind::Include => BcInclude::Include,
                    rphp_ast::v2::IncludeKind::IncludeOnce => BcInclude::IncludeOnce,
                    rphp_ast::v2::IncludeKind::Require => BcInclude::Require,
                    rphp_ast::v2::IncludeKind::RequireOnce => BcInclude::RequireOnce,
                };
                self.emit(Op::Include { dst, path: p, kind });
                dst
            }
            Expr::Match {
                subject,
                arms,
                span,
            } => self.compile_match(subject, arms, *span),
            Expr::Throw { expr, .. } => {
                let mark = self.temp_top;
                let src = self.compile_expr(expr);
                self.emit(Op::Throw { src });
                self.free_to(mark);
                self.null_temp()
            }
            Expr::Clone { expr, with, span } => {
                if with.is_some() {
                    return self.unsupported_expr(*span, "clone with properties");
                }
                let mark = self.temp_top;
                let src = self.compile_expr(expr);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::Clone {
                    dst,
                    src,
                    with: None,
                });
                dst
            }

            // ----- everything else is not lowered yet ---------------------------
            Expr::ShellExec { span, .. } => self.unsupported_expr(*span, "shell execution"),
            Expr::Callable { span, target } => self.compile_callable(target, *span),
            // `eval($code)` compiles and runs in the runtime (`eval.rs`); like
            // `include`, the unit is not known here. `regs.rs` already marks
            // the enclosing function `NEEDS_SYMTAB`, because eval'd code
            // shares the caller's variables.
            Expr::Eval { code, .. } => {
                let mark = self.temp_top;
                let src = self.compile_expr(code);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::Eval { dst, src });
                dst
            }
            // `yield` / `yield from` park the frame; the value the
            // generator is resumed with lands in `dst` (E8, `generator.rs`).
            Expr::Yield { key, value, .. } => {
                let mark = self.temp_top;
                let k = key.as_ref().map(|k| self.compile_expr(k));
                let v = value.as_ref().map(|v| self.compile_expr(v));
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::Yield { dst, key: k, val: v });
                dst
            }
            Expr::YieldFrom { expr, .. } => {
                let mark = self.temp_top;
                let src = self.compile_expr(expr);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::YieldFrom { dst, src });
                dst
            }
            Expr::Let {
                temp, init, body, ..
            } => self.compile_let(*temp, init, body),
            Expr::Temp(t, span) => self.temp_reg(*t, *span),
            Expr::Seq(exprs, _) => {
                let Some((last, rest)) = exprs.split_last() else {
                    return self.null_temp();
                };
                for e in rest {
                    let mark = self.temp_top;
                    self.compile_expr_discard(e);
                    self.free_to(mark);
                }
                self.compile_expr(last)
            }
            Expr::Error(span) => self.unsupported_expr(*span, "parse-error placeholder"),
        }
    }

    /// Finish a call: the arguments have each marked their own line, so the
    /// line of the call itself goes back before the op that runs it. php
    /// reports a call at the line its *name* is on — a call spread over
    /// several lines is not reported at its closing paren.
    fn emit_do_call(&mut self, line: u32) -> Reg {
        self.cur_line = line;
        let dst = self.alloc_temp();
        self.emit(Op::DoCall { dst });
        dst
    }

    /// Compile an expression whose value is not needed (an expression
    /// statement, a `for` clause): assignment forms skip materializing their
    /// result.
    pub(crate) fn compile_expr_discard(&mut self, e: &Expr) {
        self.mark_line(e.span());
        match e {
            Expr::Assign {
                target,
                value,
                op,
                by_ref: false,
                ..
            } if !chain_has_nullsafe(e) => {
                match op {
                    Some(BinOp::Coalesce) => self.compile_coalesce_assign(target, value, false),
                    Some(op) => self.compile_compound(target, *op, value, false),
                    None => self.compile_write(target, ValueSrc::Expr(value), false),
                };
            }
            Expr::Unary { op, expr, span } if op.is_inc_dec() && !chain_has_nullsafe(e) => {
                self.compile_unary(*op, expr, *span, false);
            }
            _ => {
                self.compile_expr(e);
            }
        }
    }

    // ---- literals -----------------------------------------------------------

    pub(crate) fn load_const(&mut self, c: Const) -> Reg {
        let k = self.push_const(c);
        let dst = self.alloc_temp();
        self.emit(Op::LoadConst { dst, k });
        dst
    }

    fn load_str(&mut self, id: IdentId) -> Reg {
        let bytes = self.interner().resolve(id);
        self.load_const(Const::Str(Str::new(bytes)))
    }

    fn load_bytes(&mut self, bytes: &[u8]) -> Reg {
        self.load_const(Const::Str(Str::new(bytes)))
    }

    /// `"a $b {$c->d}"`: every part staged into consecutive temporaries, then
    /// one `ConcatN` (a lone `"$x"` is still `(string)$x`).
    fn compile_interp(&mut self, parts: &[InterpPart]) -> Reg {
        let n = parts.len() as u16;
        if n == 0 {
            return self.load_bytes(b"");
        }
        let base = self.temp_top;
        self.set_top(base + n);
        for (i, part) in parts.iter().enumerate() {
            let slot = base + i as Reg;
            let mark = self.temp_top;
            let r = match part {
                InterpPart::Lit(id, _) => self.load_str(*id),
                InterpPart::Expr(e) => self.compile_expr(e),
            };
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        self.free_to(base);
        let dst = self.alloc_temp();
        self.emit(Op::ConcatN { dst, base, n });
        dst
    }

    fn compile_magic_const(&mut self, kind: MagicKind, span: Span) -> Reg {
        match kind {
            MagicKind::Line => {
                let line = self.mx.line(span.lo);
                self.load_const(Const::Int(i64::from(line)))
            }
            MagicKind::File => {
                let f = self.mx.file.clone();
                self.load_bytes(&f)
            }
            MagicKind::Dir => {
                let d = self.mx.dir.clone();
                self.load_bytes(&d)
            }
            MagicKind::Function => {
                let n = self.func_name.clone();
                self.load_bytes(&n)
            }
            MagicKind::Class => match self.cur_class {
                Some((_, name)) => {
                    let n = self.interner().resolve(name).to_vec();
                    self.load_bytes(&n)
                }
                None => self.load_bytes(b""),
            },
            MagicKind::Method => {
                let mut n = Vec::new();
                if let Some((_, class)) = self.cur_class {
                    if !self.flags.contains(rphp_bytecode::FnFlags::CLOSURE) {
                        n.extend_from_slice(self.interner().resolve(class));
                        n.extend_from_slice(b"::");
                    }
                }
                n.extend_from_slice(&self.func_name);
                self.load_bytes(&n)
            }
            MagicKind::Namespace => self.load_bytes(b""),
            MagicKind::Trait | MagicKind::Property => self.load_bytes(b""),
        }
    }

    // ---- HIR temporaries ----------------------------------------------------------

    /// `Let(t, init, body)`: bind `t` to a register holding `init`'s value
    /// (a dereferenced copy, so a variable initializer is snapshotted as
    /// php's temporary would be) for the duration of `body`.
    fn compile_let(&mut self, temp: TempId, init: &Expr, body: &Expr) -> Reg {
        let reg = self.alloc_temp();
        let mark = self.temp_top;
        let r = self.compile_expr(init);
        self.emit(Op::Deref { dst: reg, src: r });
        self.free_to(mark);
        self.temps.push((temp, reg));
        let out = self.compile_expr(body);
        self.unbind_temp(temp);
        out
    }

    /// The register bound to an HIR temporary.
    pub(crate) fn temp_reg(&mut self, t: TempId, span: Span) -> Reg {
        match self.temps.iter().rev().find(|(id, _)| *id == t) {
            Some(&(_, r)) => r,
            None => {
                // An HIR invariant violation (a `Temp` outside its `Let`);
                // report rather than mis-compile.
                unsupported(self.diags, span, "HIR temporary outside its binding");
                self.null_temp()
            }
        }
    }

    /// Drop the innermost binding of `t`.
    pub(crate) fn unbind_temp(&mut self, t: TempId) {
        if let Some(i) = self.temps.iter().rposition(|(id, _)| *id == t) {
            self.temps.remove(i);
        }
    }

    /// The pool constants of a two-step function/constant lookup: the
    /// namespaced candidate as `name` and the global one as `ns_fallback`
    /// (`None` when the resolver found a single candidate).
    fn two_step_consts(
        &mut self,
        ns_key: Option<IdentId>,
        global_key: IdentId,
    ) -> (u32, Option<u32>) {
        match ns_key {
            Some(ns) => {
                let k = self.sym_const(self.interner().resolve(ns));
                let g = self.sym_const(self.interner().resolve(global_key));
                (k, Some(g))
            }
            None => (self.sym_const(self.interner().resolve(global_key)), None),
        }
    }

    /// The pool constant of a class-position name: its resolved FQN.
    fn class_name_const(&mut self, name: &Name) -> u32 {
        let id = match name.resolved {
            Some(Resolved::Class { fqn, .. }) => fqn,
            _ => name.text,
        };
        self.sym_const(self.interner().resolve(id))
    }

    // ---- names --------------------------------------------------------------

    /// Lower a member name to a [`NameRef`]: a `Const::Str` for a literal
    /// identifier, a register for a computed name.
    pub(crate) fn member_name_ref(&mut self, name: &MemberName) -> NameRef {
        match name {
            MemberName::Ident(id, _) => NameRef::constant(self.name_const(*id)),
            MemberName::Expr(e) => NameRef::reg(self.compile_expr(e)),
        }
    }

    /// Lower a class reference operand. `self`/`parent` outside a class are
    /// compile errors (as in PHP); a dynamic reference evaluates to a
    /// register.
    pub(crate) fn class_ref(&mut self, class: &AstClassRef, span: Span) -> Option<ClassRef> {
        Some(match class {
            AstClassRef::Named(name) => {
                let k = self.class_name_const(name);
                ClassRef::named(k)
            }
            AstClassRef::SelfKw(_) => {
                if self.cur_class.is_none() {
                    self.scope_error("Cannot use \"self\" when no class scope is active", span);
                    return None;
                }
                ClassRef::SELF_KW
            }
            AstClassRef::Parent(_) => {
                if self.cur_class.is_none() {
                    self.scope_error("Cannot use \"parent\" when no class scope is active", span);
                    return None;
                }
                ClassRef::PARENT
            }
            AstClassRef::Static(_) => {
                if self.cur_class.is_none() {
                    self.scope_error("Cannot use \"static\" when no class scope is active", span);
                    return None;
                }
                ClassRef::STATIC
            }
            AstClassRef::Expr(e) => ClassRef::reg(self.compile_expr(e)),
        })
    }

    /// `X::class`.
    fn compile_class_name(&mut self, class: &AstClassRef, span: Span) -> Reg {
        match class {
            AstClassRef::Named(name) => {
                // The resolver folds `Name::class` to a string; this is the
                // fallback for a tree that did not go through it.
                let id = match name.resolved {
                    Some(Resolved::Class { fqn, .. }) => fqn,
                    _ => name.text,
                };
                let n = self.interner().resolve(id).to_vec();
                self.load_bytes(&n)
            }
            // `self::class` cannot be folded to `cur_class`: inside a *trait*
            // method php reports the **using** class, and traits are flattened
            // by copying the body, so the lexical name is the trait's. Like
            // `static::class`, `parent::class` and `$obj::class` (8.0), it
            // resolves against the frame at runtime.
            other => self.compile_class_const(other, &ConstSel::Class(span), span),
        }
    }

    /// `Class::NAME`, `Class::{expr}` and the dynamic half of
    /// `Class::class`: one [`Op::FetchClassConst`]. Enum case access reads
    /// the same way — `Suit::Hearts` is spelled like a constant and the
    /// runtime resolves it against the class's cases.
    fn compile_class_const(&mut self, class: &AstClassRef, sel: &ConstSel, span: Span) -> Reg {
        let mark = self.temp_top;
        let Some(class) = self.class_ref(class, span) else {
            return self.null_temp();
        };
        // Class-constant names are case-sensitive, so they are `Const::Str`
        // pool entries (`name_const`), never the prelowercased `Const::Name`.
        let name = match sel {
            ConstSel::Ident(id, _) => NameRef::constant(self.name_const(*id)),
            ConstSel::Class(_) => NameRef::constant(self.str_const(b"class")),
            ConstSel::Expr(e) => NameRef::reg(self.compile_expr(e)),
        };
        self.free_to(mark);
        let dst = self.alloc_temp();
        let ic = self.ic();
        self.emit(Op::FetchClassConst {
            dst,
            class,
            name,
            ic,
        });
        dst
    }

    /// The `(class, name)` operand pair of a static-property access —
    /// `A::$p`, `self::$p`, `static::$p`, `parent::$p`, `$cls::$p`,
    /// `$obj::$p`, `A::$$n` — evaluated in php's order: the class reference
    /// first, then a computed name. `None` after a scope diagnostic.
    ///
    /// `static::$p` must stay [`ClassRef::STATIC`] here: the late-static-bound
    /// class is the frame's, not the lexical scope's, so it cannot be folded
    /// at compile time.
    pub(crate) fn static_prop_ref(
        &mut self,
        class: &AstClassRef,
        name: &MemberName,
        span: Span,
    ) -> Option<(ClassRef, NameRef)> {
        let class = self.class_ref(class, span)?;
        // Static-property names are case-sensitive: `Const::Str` entries.
        let name = self.member_name_ref(name);
        Some((class, name))
    }

    /// `dst = Class::$name` without the "Access to undeclared static
    /// property" error — the read half of `??` and of `empty()`.
    fn quiet_static_prop_into(&mut self, dst: Reg, class: ClassRef, name: NameRef) {
        let c = self.alloc_temp();
        self.emit(Op::IssetStaticProp { dst: c, class, name });
        let jnull = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
        let ic = self.ic();
        self.emit(Op::FetchStaticProp {
            dst,
            class,
            name,
            ic,
        });
        let jend = self.jmp_fwd();
        let lnull = self.here();
        self.patch(jnull, lnull);
        self.emit(Op::LoadNull { dst });
        let lend = self.here();
        self.patch(jend, lend);
    }

    /// First-class callable syntax (php 8.1): `strlen(...)`, `$f(...)`,
    /// `$obj->m(...)`, `$obj->$n(...)`, `A::m(...)`, `$cls::m(...)`,
    /// `A::$m(...)`.
    ///
    /// **Encoding.** [`Op::MakeCallableClosure`] carries no operand but its
    /// destination: it turns the frame's pending-call record into a
    /// `Closure`. So the lowering is the ordinary call sequence with the
    /// argument sends left out — the `Init*` op that names the target is
    /// emitted exactly as for a real call and is always the instruction
    /// immediately before `MakeCallableClosure`, with never a `Send*`
    /// between them:
    ///
    /// | source          | emitted                                          |
    /// |-----------------|--------------------------------------------------|
    /// | `f(...)`        | `InitFCall{name, ns_fallback}` + `MakeCallableClosure` |
    /// | `$f(...)`       | `InitDynCall{callee}` + `MakeCallableClosure`     |
    /// | `$o->m(...)`    | `InitMethodCall{obj, name}` + `MakeCallableClosure` |
    /// | `A::m(...)`     | `InitStaticCall{class, name}` + `MakeCallableClosure` |
    ///
    /// Every piece of the target — the class reference, the (possibly
    /// computed) member name, the object — therefore lives on that `Init*`
    /// op, which is what the runtime's `make_callable_closure` reads when it
    /// binds `$this` and the scope.
    fn compile_callable(&mut self, target: &CallableTarget, span: Span) -> Reg {
        let mark = self.temp_top;
        match target {
            CallableTarget::Func(Callee::Name(name)) => self.emit_init_fcall(name),
            CallableTarget::Func(Callee::Expr(callee)) => {
                let callee = self.compile_expr(callee);
                let ic = self.ic();
                self.emit(Op::InitDynCall { callee, ic });
            }
            CallableTarget::Method { obj, name } => {
                let obj = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                let ic = self.ic();
                self.emit(Op::InitMethodCall { obj, name, ic });
            }
            CallableTarget::Static { class, name } => {
                let Some(class) = self.class_ref(class, span) else {
                    return self.null_temp();
                };
                let name = self.member_name_ref(name);
                let ic = self.ic();
                self.emit(Op::InitStaticCall { class, name, ic });
            }
        }
        self.free_to(mark);
        let dst = self.alloc_temp();
        self.emit(Op::MakeCallableClosure { dst });
        dst
    }

    /// Push a `self::`/`parent::` scope-misuse diagnostic.
    fn scope_error(&mut self, msg: &str, span: Span) {
        self.diags
            .push(Diagnostic::error(INVALID_SCOPE, msg).with_primary(span, "invalid scope"));
    }

    // ---- nullsafe chains ---------------------------------------------------------

    /// The object/base part of a member chain, compiled inside the chain (so
    /// a nested nullsafe link short-circuits the whole chain).
    fn compile_chain_obj(&mut self, e: &Expr) -> Reg {
        if self.nullsafe.is_empty() {
            return self.compile_expr(e);
        }
        let saved = self.in_nullsafe;
        self.in_nullsafe = true;
        let r = self.compile_expr(e);
        self.in_nullsafe = saved;
        r
    }

    /// Bracket a chain containing `?->`: the result register is `null` when
    /// any nullsafe link sees `null`.
    fn compile_nullsafe_chain(&mut self, e: &Expr) -> Reg {
        let res = self.alloc_temp();
        let mark = self.temp_top;
        self.nullsafe.push(NullsafeCtx { jumps: Vec::new() });
        let saved = self.in_nullsafe;
        self.in_nullsafe = true;
        let r = self.compile_expr(e);
        self.in_nullsafe = saved;
        let ctx = self.nullsafe.pop().expect("nullsafe ctx");
        self.emit(Op::Move { dst: res, src: r });
        let jend = self.jmp_fwd();
        let lnull = self.here();
        for j in ctx.jumps {
            self.patch(j, lnull);
        }
        self.emit(Op::LoadNull { dst: res });
        let lend = self.here();
        self.patch(jend, lend);
        self.free_to(mark);
        res
    }

    /// `?->`: jump to the chain's null exit when `obj` is null.
    fn nullsafe_check(&mut self, obj: Reg) {
        let c = self.alloc_temp();
        self.emit(Op::IssetVar { dst: c, var: obj });
        let j = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
        match self.nullsafe.last_mut() {
            Some(ctx) => ctx.jumps.push(j),
            None => {
                // Not inside a bracketed chain (cannot happen: the outermost
                // link brackets); degrade to a plain read.
                let here = self.here();
                self.patch(j, here);
            }
        }
    }

    // ---- write targets ------------------------------------------------------------

    /// Plan the container chain of a write target (everything but the last
    /// `[key]` / `->name`), evaluating the root and every key/object in
    /// source order. `None` after a diagnostic.
    pub(crate) fn plan_chain(&mut self, e: &Expr) -> Option<Plan> {
        match e {
            Expr::Var(id, _) => {
                if self.is_this(*id) {
                    let r = self.load_this();
                    return Some(Plan {
                        root: r,
                        steps: Vec::new(),
                    });
                }
                if self.is_globals(*id) {
                    self.diags.push(
                        Diagnostic::error(INVALID_WRITE_TARGET, "Cannot re-assign $GLOBALS")
                            .with_primary(e.span(), "read-only"),
                    );
                    return None;
                }
                Some(Plan {
                    root: self.var_reg(*id),
                    steps: Vec::new(),
                })
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let reg = self.alloc_temp();
                self.emit(Op::BindDynVar {
                    reg,
                    name: n,
                    global: false,
                });
                Some(Plan {
                    root: reg,
                    steps: Vec::new(),
                })
            }
            Expr::Index { base, index, .. } => {
                // `$GLOBALS[key]` as a container: bind a temp to the global cell.
                if let Expr::Var(id, _) = &**base {
                    if self.is_globals(*id) {
                        let Some(index) = index else {
                            unsupported(self.diags, e.span(), "append to $GLOBALS");
                            return None;
                        };
                        let reg = self.alloc_temp();
                        if let Expr::Str(s, _) = &**index {
                            let name = self.name_const(*s);
                            self.emit(Op::BindGlobal { reg, name });
                        } else {
                            let n = self.compile_expr(index);
                            self.emit(Op::BindDynVar {
                                reg,
                                name: n,
                                global: true,
                            });
                        }
                        return Some(Plan {
                            root: reg,
                            steps: Vec::new(),
                        });
                    }
                }
                let mut plan = self.plan_chain(base)?;
                let key = index.as_ref().map(|i| self.compile_expr(i));
                plan.steps.push(Step::Elem(key));
                Some(plan)
            }
            Expr::Prop {
                obj,
                name,
                nullsafe,
                span,
            } => {
                if *nullsafe {
                    self.diags.push(
                        Diagnostic::error(INVALID_WRITE_TARGET, "Can't use nullsafe operator in write context")
                            .with_primary(*span, "nullsafe in write context"),
                    );
                    return None;
                }
                // Objects are handles: the holder is read (quietly — a
                // missing holder is the `Attempt to assign property on null`
                // error, not an undefined-key/property warning), the property
                // is the step.
                let root = self.compile_quiet(obj);
                let name = self.member_name_ref(name);
                Some(Plan {
                    root,
                    steps: vec![Step::Prop(name)],
                })
            }
            Expr::StaticProp { class, name, span } => {
                let (class, name) = self.static_prop_ref(class, name, *span)?;
                // A static property is a shared cell, so the cell itself is
                // the container handle: `A::$a['k'] = 1` mutates it in place
                // and needs no write-back step.
                let root = self.alloc_temp();
                self.emit(Op::RefStaticProp {
                    dst: root,
                    class,
                    name,
                });
                Some(Plan {
                    root,
                    steps: Vec::new(),
                })
            }
            // A stabilized chain base (`Let(t, f(), Temp(t)->p = v)`) or a
            // by-reference destructuring source.
            Expr::Temp(t, span) => Some(Plan {
                root: self.temp_reg(*t, *span),
                steps: Vec::new(),
            }),
            other => {
                self.diags.push(
                    Diagnostic::error(INVALID_WRITE_TARGET, "Cannot use temporary expression in write context")
                        .with_primary(other.span(), "not a variable"),
                );
                None
            }
        }
    }

    /// Emit the fetch-for-write chain of a plan; returns the handle register
    /// of the container to write and the write-backs to emit afterwards.
    pub(crate) fn plan_fetch_w(&mut self, plan: &Plan) -> (Reg, Vec<Wb>) {
        let mut cur = plan.root;
        let mut wbs = Vec::with_capacity(plan.steps.len());
        for step in &plan.steps {
            let t = self.alloc_temp();
            match step {
                Step::Elem(key) => {
                    self.emit(Op::FetchElemW {
                        dst: t,
                        arr: cur,
                        key: *key,
                    });
                    wbs.push(Wb::Elem {
                        arr: cur,
                        key: *key,
                        val: t,
                    });
                }
                Step::Prop(name) => {
                    self.emit(Op::FetchPropW {
                        dst: t,
                        obj: cur,
                        name: *name,
                    });
                    wbs.push(Wb::Prop {
                        obj: cur,
                        name: *name,
                        val: t,
                    });
                }
            }
            cur = t;
        }
        (cur, wbs)
    }

    /// Read the container a plan denotes without warnings (for `??=`,
    /// nested `unset`), into a register.
    pub(crate) fn plan_read_quiet(&mut self, plan: &Plan) -> Reg {
        let mut cur = plan.root;
        for step in &plan.steps {
            let t = self.alloc_temp();
            match step {
                Step::Elem(Some(key)) => {
                    self.emit(Op::ArrayGetQuiet {
                        dst: t,
                        base: cur,
                        key: *key,
                    });
                }
                Step::Elem(None) => {
                    self.emit(Op::LoadNull { dst: t });
                }
                Step::Prop(name) => self.quiet_prop_into(t, cur, *name),
            }
            cur = t;
        }
        cur
    }

    /// `dst = obj->name` without an "undefined property" warning (`dst` may
    /// alias `obj`, so the object is read before anything lands in `dst`).
    fn quiet_prop_into(&mut self, dst: Reg, obj: Reg, name: NameRef) {
        let c = self.alloc_temp();
        self.emit(Op::IssetProp { dst: c, obj, name });
        let jnull = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
        let ic = self.ic();
        self.emit(Op::FetchProp { dst, obj, name, ic });
        let jend = self.jmp_fwd();
        let lnull = self.here();
        self.patch(jnull, lnull);
        self.emit(Op::LoadNull { dst });
        let lend = self.here();
        self.patch(jend, lend);
    }

    /// Emit the write-backs of a nested store, innermost first.
    pub(crate) fn emit_writebacks(&mut self, wbs: Vec<Wb>) {
        for wb in wbs.into_iter().rev() {
            match wb {
                Wb::Elem {
                    arr,
                    key: Some(key),
                    val,
                } => {
                    self.emit(Op::ArraySet {
                        arr,
                        key,
                        value: val,
                    });
                }
                Wb::Elem {
                    arr,
                    key: None,
                    val,
                } => {
                    self.emit(Op::ArrayPush { arr, value: val });
                }
                Wb::Prop { obj, name, val } => {
                    let ic = self.ic();
                    self.emit(Op::AssignProp {
                        obj,
                        name,
                        src: val,
                        ic,
                    });
                }
            }
        }
    }

    /// Materialize a [`ValueSrc`].
    fn value_reg(&mut self, v: ValueSrc<'_>) -> Reg {
        match v {
            ValueSrc::Expr(e) => self.compile_expr(e),
            ValueSrc::Reg(r) => r,
        }
    }

    /// Assign an already-computed register to a write target.
    pub(crate) fn assign_to_target(&mut self, target: &Expr, val: Reg) {
        if let Expr::Var(id, _) = target {
            self.mark_assigned(*id);
        }
        self.compile_write(target, ValueSrc::Reg(val), false);
    }

    /// `target = value` for every target shape. Returns the register holding
    /// the assigned value (the expression's result).
    pub(crate) fn compile_write(&mut self, target: &Expr, value: ValueSrc<'_>, want: bool) -> Reg {
        let _ = want;
        match target {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let dst = self.var_reg(*id);
                let mark = self.temp_top;
                let r = self.value_reg(value);
                self.store_var(dst, r);
                self.free_to(mark);
                dst
            }
            Expr::Var(id, span) => {
                let what = if self.is_this(*id) {
                    "Cannot re-assign $this"
                } else {
                    "Cannot re-assign $GLOBALS"
                };
                self.diags.push(
                    Diagnostic::error(INVALID_WRITE_TARGET, what).with_primary(*span, "read-only"),
                );
                self.null_temp()
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } if matches!(&**base, Expr::Var(id, _) if self.is_globals(*id)) => {
                let key = self.compile_expr(index);
                let src = self.value_reg(value);
                self.emit(Op::AssignGlobal { key, src });
                src
            }
            Expr::Index { base, index, .. } => {
                let plan = self.plan_chain(base);
                let key = index.as_ref().map(|i| self.compile_expr(i));
                let v = self.value_reg(value);
                let Some(plan) = plan else {
                    return v;
                };
                let (handle, wbs) = self.plan_fetch_w(&plan);
                match key {
                    Some(key) => {
                        self.emit(Op::ArraySet {
                            arr: handle,
                            key,
                            value: v,
                        });
                    }
                    None => {
                        self.emit(Op::ArrayPush {
                            arr: handle,
                            value: v,
                        });
                    }
                }
                self.emit_writebacks(wbs);
                v
            }
            Expr::Prop {
                obj,
                name,
                nullsafe,
                span,
            } => {
                if *nullsafe {
                    self.diags.push(
                        Diagnostic::error(INVALID_WRITE_TARGET, "Can't use nullsafe operator in write context")
                            .with_primary(*span, "nullsafe in write context"),
                    );
                    return self.null_temp();
                }
                let o = self.compile_quiet(obj);
                let name = self.member_name_ref(name);
                let v = self.value_reg(value);
                let ic = self.ic();
                self.emit(Op::AssignProp {
                    obj: o,
                    name,
                    src: v,
                    ic,
                });
                v
            }
            Expr::Array { items, .. } => {
                let v = self.value_reg(value);
                // A pattern with `&$x` items binds into the source variable
                // itself; otherwise the pattern reads from a snapshot of the
                // source so that targets inside the source array do not
                // disturb it (`[$a[1], $a[0]] = $a`).
                let src = if pattern_has_ref(items) {
                    v
                } else {
                    let src = self.alloc_temp();
                    self.emit(Op::Deref { dst: src, src: v });
                    src
                };
                self.destructure(items, src);
                v
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let reg = self.alloc_temp();
                self.emit(Op::BindDynVar {
                    reg,
                    name: n,
                    global: false,
                });
                let v = self.value_reg(value);
                self.emit(Op::AssignThroughRef { dst: reg, src: v });
                v
            }
            Expr::StaticProp { class, name, span } => {
                // php evaluates the class reference (and a computed name)
                // before the value: `cls()::$p = val()` calls `cls` first.
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return self.null_temp();
                };
                let v = self.value_reg(value);
                self.emit(Op::AssignStaticProp { class, name, src: v });
                v
            }
            other => {
                self.diags.push(
                    Diagnostic::error(INVALID_WRITE_TARGET, "Cannot use temporary expression in write context")
                        .with_primary(other.span(), "not a variable"),
                );
                self.null_temp()
            }
        }
    }

    /// `[$a, 'k' => $b, [$c, $d]] = src`: each element is read from `src`
    /// (with PHP's undefined-key warning) and assigned to its target.
    fn destructure(&mut self, items: &[ArrayItem], src: Reg) {
        let mut pos: i64 = 0;
        for item in items {
            let Some(value) = &item.value else {
                pos += 1;
                continue;
            };
            if item.spread {
                unsupported(self.diags, item.span, "spread in a destructuring pattern");
                continue;
            }
            let mark = self.temp_top;
            let key = match &item.key {
                Some(k) => self.compile_expr(k),
                None => {
                    let k = self.load_const(Const::Int(pos));
                    pos += 1;
                    k
                }
            };
            if item.by_ref {
                match value {
                    Expr::Var(id, _) if !self.is_this(*id) => {
                        let dst = self.var_reg(*id);
                        self.emit(Op::RefElem { dst, arr: src, key });
                    }
                    other => unsupported(self.diags, other.span(), "by-reference destructuring into a non-variable"),
                }
            } else {
                let t = self.alloc_temp();
                self.emit(Op::ListGet {
                    dst: t,
                    base: src,
                    key,
                });
                self.assign_to_target(value, t);
            }
            self.free_to(mark);
        }
    }

    /// `$a = &$b`, `$a = &$b[k]`, `$a = &$o->p`, `$a[k] = &$b`, `$o->p = &$b`.
    fn compile_assign_ref(&mut self, target: &Expr, value: &Expr, span: Span) -> Reg {
        // The source must be a referenceable place; the register that will
        // hold (or be bound to) the shared cell.
        let mark = self.temp_top;
        let Some(src) = self.compile_ref_source(value) else {
            return self.null_temp();
        };
        self.bind_ref_target(target, src, span, mark)
    }

    /// The register holding the cell `&expr` names — a variable, an array
    /// element, a property, a static property, a variable variable or a
    /// `$GLOBALS` entry. `None` after reporting an expression php will not
    /// take a reference to.
    ///
    /// Every place that writes a reference shares this: `$x = &…`, a
    /// by-reference array element (`[&$a[$k], &$v]`) and the loops.
    fn compile_ref_source(&mut self, value: &Expr) -> Option<Reg> {
        let src: Reg = match value {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => self.var_reg(*id),
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                match &**base {
                    Expr::Var(id, _) if self.is_globals(*id) => {
                        let reg = self.alloc_temp();
                        if let Expr::Str(s, _) = &**index {
                            let name = self.name_const(*s);
                            self.emit(Op::BindGlobal { reg, name });
                        } else {
                            let n = self.compile_expr(index);
                            self.emit(Op::BindDynVar {
                                reg,
                                name: n,
                                global: true,
                            });
                        }
                        // `&$GLOBALS['x']` is the global cell itself.
                        return Some(reg);
                    }
                    _ => {
                        let plan = self.plan_chain(base)?;
                        let key = self.compile_expr(index);
                        let (handle, wbs) = self.plan_fetch_w(&plan);
                        let dst = self.alloc_temp();
                        self.emit(Op::RefElem {
                            dst,
                            arr: handle,
                            key,
                        });
                        self.emit_writebacks(wbs);
                        dst
                    }
                }
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let o = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                let dst = self.alloc_temp();
                self.emit(Op::RefProp { dst, obj: o, name });
                dst
            }
            Expr::StaticProp { class, name, span } => {
                let (class, name) = self.static_prop_ref(class, name, *span)?;
                let dst = self.alloc_temp();
                self.emit(Op::RefStaticProp { dst, class, name });
                dst
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let reg = self.alloc_temp();
                self.emit(Op::BindDynVar {
                    reg,
                    name: n,
                    global: false,
                });
                reg
            }
            Expr::Closure(_) | Expr::ArrowFn(_) | Expr::New { .. } => {
                // `$x = &new Foo` is gone since 7.0; `= &function` binds the value.
                self.compile_expr(value)
            }
            other if other.is_call() => {
                // `$x = &f()`: no reference return yet — bind to a fresh cell.
                self.compile_expr(value)
            }
            other => {
                unsupported(self.diags, other.span(), "reference to a non-variable");
                return None;
            }
        };
        Some(src)
    }

    /// Bind `target` to the cell of `src` (making `src` a reference).
    fn bind_ref_target(&mut self, target: &Expr, src: Reg, span: Span, mark: Reg) -> Reg {
        match target {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let dst = self.var_reg(*id);
                self.emit(Op::AssignRef { dst, src });
                self.free_to(mark);
                dst
            }
            Expr::Index { base, index, .. } => {
                if let Expr::Var(id, _) = &**base {
                    if self.is_globals(*id) {
                        // `$GLOBALS['x'] = &$y`: bind the global cell.
                        let Some(index) = index else {
                            return self.unsupported_expr(span, "append to $GLOBALS");
                        };
                        let reg = self.alloc_temp();
                        if let Expr::Str(s, _) = &**index {
                            let name = self.name_const(*s);
                            self.emit(Op::BindGlobal { reg, name });
                        } else {
                            let n = self.compile_expr(index);
                            self.emit(Op::BindDynVar {
                                reg,
                                name: n,
                                global: true,
                            });
                        }
                        // Rebinding a global entry needs the table itself:
                        // not expressible through the cell — assign by value.
                        self.emit(Op::AssignThroughRef { dst: reg, src });
                        self.free_to(mark);
                        return src;
                    }
                }
                let Some(plan) = self.plan_chain(base) else {
                    return self.null_temp();
                };
                let key = index.as_ref().map(|i| self.compile_expr(i));
                let (handle, wbs) = self.plan_fetch_w(&plan);
                self.emit(Op::AssignRefElem {
                    arr: handle,
                    key,
                    src,
                });
                self.emit_writebacks(wbs);
                src
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let o = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                self.emit(Op::AssignRefProp {
                    obj: o,
                    name,
                    src,
                });
                src
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let reg = self.alloc_temp();
                self.emit(Op::BindDynVar {
                    reg,
                    name: n,
                    global: false,
                });
                // Rebinding a symbol-table entry: assign by value instead.
                self.emit(Op::AssignThroughRef { dst: reg, src });
                src
            }
            // `A::$p = &$x`: bind the shared cell to the reference.
            Expr::StaticProp { class, name, span } => {
                if let Some((class, name)) = self.static_prop_ref(class, name, *span) {
                    self.emit(Op::AssignRefStaticProp { class, name, src });
                }
                src
            }
            other => self.unsupported_expr(other.span(), "reference assignment target"),
        }
    }

    /// `target OP= value`.
    fn compile_compound(&mut self, target: &Expr, op: BinOp, value: &Expr, want: bool) -> Reg {
        let Some(kind) = assign_op_kind(op) else {
            return self.unsupported_expr(target.span(), &format!("compound assignment `{}=`", op.name()));
        };
        match target {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let var = self.var_reg(*id);
                let mark = self.temp_top;
                let src = self.compile_expr(value);
                self.emit(Op::AssignOp { op: kind, var, src });
                self.free_to(mark);
                var
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } if matches!(&**base, Expr::Var(id, _) if self.is_globals(*id)) => {
                let reg = self.alloc_temp();
                if let Expr::Str(s, _) = &**index {
                    let name = self.name_const(*s);
                    self.emit(Op::BindGlobal { reg, name });
                } else {
                    let n = self.compile_expr(index);
                    self.emit(Op::BindDynVar {
                        reg,
                        name: n,
                        global: true,
                    });
                }
                let src = self.compile_expr(value);
                self.emit(Op::AssignOp {
                    op: kind,
                    var: reg,
                    src,
                });
                reg
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                let plan = self.plan_chain(base);
                let key = self.compile_expr(index);
                let src = self.compile_expr(value);
                let Some(plan) = plan else {
                    return src;
                };
                let (handle, wbs) = self.plan_fetch_w(&plan);
                self.emit(Op::AssignOpElem {
                    op: kind,
                    arr: handle,
                    key,
                    src,
                });
                let res = if want {
                    let res = self.alloc_temp();
                    self.emit(Op::ArrayGetQuiet {
                        dst: res,
                        base: handle,
                        key,
                    });
                    res
                } else {
                    src
                };
                self.emit_writebacks(wbs);
                res
            }
            Expr::Index { span, .. } => {
                self.diags.push(
                    Diagnostic::error(INVALID_APPEND_READ, "cannot use `[]` for reading")
                        .with_primary(*span, "expected an index"),
                );
                self.null_temp()
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let o = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                let src = self.compile_expr(value);
                self.emit(Op::AssignOpProp {
                    op: kind,
                    obj: o,
                    name,
                    src,
                });
                if want {
                    let res = self.alloc_temp();
                    let ic = self.ic();
                    self.emit(Op::FetchProp {
                        dst: res,
                        obj: o,
                        name,
                        ic,
                    });
                    res
                } else {
                    src
                }
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let reg = self.alloc_temp();
                self.emit(Op::BindDynVar {
                    reg,
                    name: n,
                    global: false,
                });
                let src = self.compile_expr(value);
                self.emit(Op::AssignOp {
                    op: kind,
                    var: reg,
                    src,
                });
                reg
            }
            Expr::StaticProp { class, name, span } => {
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return self.null_temp();
                };
                // There is no `AssignOpStaticProp`; read-modify-write it.
                // php evaluates the right-hand side *before* reading the
                // property (`A::$p += g()` sees g()'s write to `A::$p`).
                let src = self.compile_expr(value);
                let cur = self.alloc_temp();
                let ic = self.ic();
                self.emit(Op::FetchStaticProp {
                    dst: cur,
                    class,
                    name,
                    ic,
                });
                self.emit(Op::AssignOp {
                    op: kind,
                    var: cur,
                    src,
                });
                self.emit(Op::AssignStaticProp {
                    class,
                    name,
                    src: cur,
                });
                if want {
                    // A typed static property coerces on the way in, so the
                    // expression's value is what landed in the cell.
                    let res = self.alloc_temp();
                    let ic = self.ic();
                    self.emit(Op::FetchStaticProp {
                        dst: res,
                        class,
                        name,
                        ic,
                    });
                    res
                } else {
                    cur
                }
            }
            other => {
                self.diags.push(
                    Diagnostic::error(INVALID_WRITE_TARGET, "Cannot use temporary expression in write context")
                        .with_primary(other.span(), "not a variable"),
                );
                self.null_temp()
            }
        }
    }

    /// `target ??= value`: assign only when the target is unset or null.
    fn compile_coalesce_assign(&mut self, target: &Expr, value: &Expr, want: bool) -> Reg {
        let _ = want;
        let res = self.alloc_temp();
        let mark = self.temp_top;
        match target {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let var = self.var_reg(*id);
                let c = self.alloc_temp();
                self.emit(Op::IssetVar { dst: c, var });
                let jset = self.emit(Op::JmpIfTrue { cond: c, target: 0 });
                let v = self.compile_expr(value);
                self.store_var(var, v);
                let lend = self.here();
                self.patch(jset, lend);
                self.free_to(mark);
                var
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                let (plan, probe) = if matches!(&**base, Expr::Var(id, _) if self.is_globals(*id)) {
                    let g = self.alloc_temp();
                    self.emit(Op::FetchGlobals { dst: g });
                    (None, g)
                } else {
                    let plan = self.plan_chain(base);
                    let probe = match &plan {
                        Some(p) => self.plan_read_quiet(p),
                        None => self.null_temp(),
                    };
                    (plan, probe)
                };
                let key = self.compile_expr(index);
                let t = self.alloc_temp();
                self.emit(Op::ArrayGetQuiet {
                    dst: t,
                    base: probe,
                    key,
                });
                let c = self.alloc_temp();
                self.emit(Op::IssetVar { dst: c, var: t });
                let jassign = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
                self.emit(Op::Move { dst: res, src: t });
                let jend = self.jmp_fwd();
                let lassign = self.here();
                self.patch(jassign, lassign);
                let v = self.compile_expr(value);
                match plan {
                    Some(plan) => {
                        let (handle, wbs) = self.plan_fetch_w(&plan);
                        self.emit(Op::ArraySet {
                            arr: handle,
                            key,
                            value: v,
                        });
                        self.emit_writebacks(wbs);
                    }
                    None if matches!(&**base, Expr::Var(id, _) if self.is_globals(*id)) => {
                        self.emit(Op::AssignGlobal { key, src: v });
                    }
                    None => {}
                }
                self.emit(Op::Move { dst: res, src: v });
                let lend = self.here();
                self.patch(jend, lend);
                self.free_to(mark);
                res
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let o = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                let c = self.alloc_temp();
                self.emit(Op::IssetProp {
                    dst: c,
                    obj: o,
                    name,
                });
                let jassign = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
                let ic = self.ic();
                self.emit(Op::FetchProp {
                    dst: res,
                    obj: o,
                    name,
                    ic,
                });
                let jend = self.jmp_fwd();
                let lassign = self.here();
                self.patch(jassign, lassign);
                let v = self.compile_expr(value);
                let ic = self.ic();
                self.emit(Op::AssignProp {
                    obj: o,
                    name,
                    src: v,
                    ic,
                });
                self.emit(Op::Move { dst: res, src: v });
                let lend = self.here();
                self.patch(jend, lend);
                self.free_to(mark);
                res
            }
            Expr::StaticProp { class, name, span } => {
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    self.free_to(mark);
                    self.emit(Op::LoadNull { dst: res });
                    return res;
                };
                let c = self.alloc_temp();
                self.emit(Op::IssetStaticProp { dst: c, class, name });
                let jassign = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
                let ic = self.ic();
                self.emit(Op::FetchStaticProp {
                    dst: res,
                    class,
                    name,
                    ic,
                });
                let jend = self.jmp_fwd();
                let lassign = self.here();
                self.patch(jassign, lassign);
                let v = self.compile_expr(value);
                self.emit(Op::AssignStaticProp { class, name, src: v });
                self.emit(Op::Move { dst: res, src: v });
                let lend = self.here();
                self.patch(jend, lend);
                self.free_to(mark);
                res
            }
            other => {
                self.free_to(mark);
                self.unsupported_expr(other.span(), "??= target")
            }
        }
    }

    // ---- unary ------------------------------------------------------------------

    fn compile_unary(&mut self, op: UnOp, expr: &Expr, span: Span, want: bool) -> Reg {
        match op {
            UnOp::PreInc | UnOp::PreDec | UnOp::PostInc | UnOp::PostDec => {
                let pre = matches!(op, UnOp::PreInc | UnOp::PreDec);
                let inc = matches!(op, UnOp::PreInc | UnOp::PostInc);
                self.compile_incdec(expr, pre, inc, want)
            }
            UnOp::Silence => {
                self.emit(Op::Silence { begin: true });
                let r = self.compile_expr(expr);
                self.emit(Op::Silence { begin: false });
                r
            }
            UnOp::Void => {
                let mark = self.temp_top;
                self.compile_expr_discard(expr);
                self.free_to(mark);
                self.null_temp()
            }
            UnOp::Cast(kind) => {
                let mark = self.temp_top;
                let r = self.compile_expr(expr);
                self.free_to(mark);
                let dst = self.alloc_temp();
                let kind = match kind {
                    AstCast::Int => CastKind::Int,
                    AstCast::Float => CastKind::Float,
                    AstCast::String => CastKind::String,
                    AstCast::Bool => CastKind::Bool,
                    AstCast::Array => CastKind::Array,
                    AstCast::Object => CastKind::Object,
                };
                self.emit(Op::Cast { dst, src: r, kind });
                dst
            }
            UnOp::Neg | UnOp::Plus | UnOp::Not | UnOp::BitNot => {
                let mark = self.temp_top;
                let r = self.compile_expr(expr);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(match op {
                    UnOp::Neg => Op::Neg { dst, src: r },
                    UnOp::Plus => Op::Plus { dst, src: r },
                    UnOp::BitNot => Op::BitNot { dst, src: r },
                    _ => Op::Not { dst, src: r },
                });
                let _ = span;
                dst
            }
        }
    }

    /// `++$x` / `$x++` / `--` on a variable, element or property.
    fn compile_incdec(&mut self, target: &Expr, pre: bool, inc: bool, want: bool) -> Reg {
        match target {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let var = self.var_reg(*id);
                let dst = if want { Some(self.alloc_temp()) } else { None };
                self.emit(Op::IncDec { var, dst, pre, inc });
                dst.unwrap_or(var)
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } if matches!(&**base, Expr::Var(id, _) if self.is_globals(*id)) => {
                let reg = self.alloc_temp();
                if let Expr::Str(s, _) = &**index {
                    let name = self.name_const(*s);
                    self.emit(Op::BindGlobal { reg, name });
                } else {
                    let n = self.compile_expr(index);
                    self.emit(Op::BindDynVar {
                        reg,
                        name: n,
                        global: true,
                    });
                }
                let dst = if want { Some(self.alloc_temp()) } else { None };
                self.emit(Op::IncDec {
                    var: reg,
                    dst,
                    pre,
                    inc,
                });
                dst.unwrap_or(reg)
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                let res = if want { Some(self.alloc_temp()) } else { None };
                let Some(plan) = self.plan_chain(base) else {
                    return self.null_temp();
                };
                let key = self.compile_expr(index);
                let (handle, wbs) = self.plan_fetch_w(&plan);
                let cur = self.alloc_temp();
                self.emit(Op::ArrayGet {
                    dst: cur,
                    base: handle,
                    key,
                });
                self.emit(Op::IncDec {
                    var: cur,
                    dst: res,
                    pre,
                    inc,
                });
                self.emit(Op::ArraySet {
                    arr: handle,
                    key,
                    value: cur,
                });
                self.emit_writebacks(wbs);
                res.unwrap_or(cur)
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let res = if want { Some(self.alloc_temp()) } else { None };
                let o = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                let cur = self.alloc_temp();
                let ic = self.ic();
                self.emit(Op::FetchProp {
                    dst: cur,
                    obj: o,
                    name,
                    ic,
                });
                self.emit(Op::IncDec {
                    var: cur,
                    dst: res,
                    pre,
                    inc,
                });
                let ic = self.ic();
                self.emit(Op::AssignProp {
                    obj: o,
                    name,
                    src: cur,
                    ic,
                });
                res.unwrap_or(cur)
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let reg = self.alloc_temp();
                self.emit(Op::BindDynVar {
                    reg,
                    name: n,
                    global: false,
                });
                let dst = if want { Some(self.alloc_temp()) } else { None };
                self.emit(Op::IncDec {
                    var: reg,
                    dst,
                    pre,
                    inc,
                });
                dst.unwrap_or(reg)
            }
            Expr::StaticProp { class, name, span } => {
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return self.null_temp();
                };
                let res = if want { Some(self.alloc_temp()) } else { None };
                let cur = self.alloc_temp();
                let ic = self.ic();
                self.emit(Op::FetchStaticProp {
                    dst: cur,
                    class,
                    name,
                    ic,
                });
                self.emit(Op::IncDec {
                    var: cur,
                    dst: res,
                    pre,
                    inc,
                });
                self.emit(Op::AssignStaticProp {
                    class,
                    name,
                    src: cur,
                });
                res.unwrap_or(cur)
            }
            other => self.unsupported_expr(other.span(), "increment/decrement target"),
        }
    }

    // ---- arrays ---------------------------------------------------------------

    /// `[ ... ]` / `array( ... )`: build a fresh array, then fill it element by
    /// element preserving source order.
    fn compile_array(&mut self, items: &[ArrayItem]) -> Reg {
        let dst = self.alloc_temp();
        self.emit(Op::NewArray { dst });
        let mark = self.temp_top;
        for item in items {
            if item.spread {
                // `...$x` merges at this position; the runtime does the
                // key renumbering (`Op::ArrayUnpack`).
                if let Some(value) = &item.value {
                    let src = self.compile_expr(value);
                    self.emit(Op::ArrayUnpack { arr: dst, src });
                    self.free_to(mark);
                }
                continue;
            }
            let Some(value) = &item.value else {
                unsupported(self.diags, item.span, "skipped array element");
                continue;
            };
            let key = item.key.as_ref().map(|k| self.compile_expr(k));
            if item.by_ref {
                // `[&$a[$k], &$o->p, &$v]`: the element is bound to the
                // cell, not to a copy, so the same places `$x = &…` accepts
                // are the places an element accepts.
                let Some(src) = self.compile_ref_source(value) else {
                    self.free_to(mark);
                    continue;
                };
                self.emit(Op::AssignRefElem { arr: dst, key, src });
            } else {
                let vr = self.compile_expr(value);
                match key {
                    Some(key) => {
                        self.emit(Op::ArraySet {
                            arr: dst,
                            key,
                            value: vr,
                        });
                    }
                    None => {
                        self.emit(Op::ArrayPush {
                            arr: dst,
                            value: vr,
                        });
                    }
                }
            }
            self.free_to(mark);
        }
        dst
    }

    /// `base[index]` read. `$a[]` (no index) is not a readable expression.
    fn compile_index_read(&mut self, base: &Expr, index: Option<&Expr>, span: Span) -> Reg {
        let Some(index) = index else {
            self.diags.push(
                Diagnostic::error(INVALID_APPEND_READ, "cannot use `[]` for reading")
                    .with_primary(span, "expected an index"),
            );
            return self.null_temp();
        };
        let mark = self.temp_top;
        let br = self.compile_chain_obj(base);
        let kr = self.compile_expr(index);
        self.free_to(mark);
        let dst = self.alloc_temp();
        // `Temp(t)[k]` is how a desugared destructuring pattern reads its
        // source: php's list-read semantics (silently null on a null source).
        if matches!(base, Expr::Temp(..)) {
            self.emit(Op::ListGet {
                dst,
                base: br,
                key: kr,
            });
        } else {
            self.emit(Op::ArrayGet {
                dst,
                base: br,
                key: kr,
            });
        }
        dst
    }

    /// Read an lvalue-shaped expression without "undefined" warnings (the
    /// left side of `??`, the operands of `isset`/`empty`).
    fn compile_quiet(&mut self, e: &Expr) -> Reg {
        match e {
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                let mark = self.temp_top;
                let b = self.compile_quiet(base);
                let k = self.compile_expr(index);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::ArrayGetQuiet {
                    dst,
                    base: b,
                    key: k,
                });
                dst
            }
            Expr::Prop { obj, name, .. } => {
                let mark = self.temp_top;
                let o = self.compile_quiet(obj);
                let name = self.member_name_ref(name);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.quiet_prop_into(dst, o, name);
                dst
            }
            Expr::StaticProp { class, name, span } => {
                let mark = self.temp_top;
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return self.null_temp();
                };
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.quiet_static_prop_into(dst, class, name);
                dst
            }
            // A quiet read is exactly the place php does *not* warn about an
            // undefined variable (`$x ?? 'd'`, `isset($x->p)`).
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => self.var_reg(*id),
            _ => self.compile_expr(e),
        }
    }

    // ---- short-circuit logic ----------------------------------------------------

    /// `a && b` with short-circuit; result register holds a real bool.
    fn compile_and(&mut self, lhs: &Expr, rhs: &Expr) -> Reg {
        let dst = self.alloc_temp();
        let mark = self.temp_top;
        let ra = self.compile_expr(lhs);
        let jf = self.emit(Op::JmpIfFalse {
            cond: ra,
            target: 0,
        });
        self.free_to(mark);
        // True path: dst = (bool) b, via double logical-negation.
        let rb = self.compile_expr(rhs);
        self.emit(Op::Not { dst, src: rb });
        self.emit(Op::Not { dst, src: dst });
        self.free_to(mark);
        let jend = self.jmp_fwd();
        // False path: lhs was falsy -> result is `false`.
        let lfalse = self.here();
        self.patch(jf, lfalse);
        self.emit(Op::LoadBool { dst, val: false });
        let lend = self.here();
        self.patch(jend, lend);
        dst
    }

    /// `a || b` with short-circuit; result register holds a real bool.
    fn compile_or(&mut self, lhs: &Expr, rhs: &Expr) -> Reg {
        let dst = self.alloc_temp();
        let mark = self.temp_top;
        let ra = self.compile_expr(lhs);
        let jt = self.emit(Op::JmpIfTrue {
            cond: ra,
            target: 0,
        });
        self.free_to(mark);
        // Fall-through path: lhs was falsy -> result = (bool) b.
        let rb = self.compile_expr(rhs);
        self.emit(Op::Not { dst, src: rb });
        self.emit(Op::Not { dst, src: dst });
        self.free_to(mark);
        let jend = self.jmp_fwd();
        // True path: lhs was truthy -> result is `true`.
        let ltrue = self.here();
        self.patch(jt, ltrue);
        self.emit(Op::LoadBool { dst, val: true });
        let lend = self.here();
        self.patch(jend, lend);
        dst
    }

    /// `a ?? b`: `a` read quietly; `b` only when `a` is unset/null.
    fn compile_coalesce(&mut self, lhs: &Expr, rhs: &Expr) -> Reg {
        let res = self.alloc_temp();
        let mark = self.temp_top;
        let l = self.compile_quiet(lhs);
        let c = self.alloc_temp();
        self.emit(Op::IssetVar { dst: c, var: l });
        let jelse = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
        self.emit(Op::Move { dst: res, src: l });
        let jend = self.jmp_fwd();
        self.free_to(mark);
        let lelse = self.here();
        self.patch(jelse, lelse);
        let r = self.compile_expr(rhs);
        self.emit(Op::Move { dst: res, src: r });
        self.free_to(mark);
        let lend = self.here();
        self.patch(jend, lend);
        res
    }

    /// `c ? a : b` and `c ?: b`.
    fn compile_ternary(&mut self, cond: &Expr, then: Option<&Expr>, else_: &Expr) -> Reg {
        let res = self.alloc_temp();
        let mark = self.temp_top;
        let c = self.compile_expr(cond);
        let jelse = self.emit(Op::JmpIfFalse { cond: c, target: 0 });
        match then {
            Some(then) => {
                let t = self.compile_expr(then);
                self.emit(Op::Move { dst: res, src: t });
            }
            None => {
                self.emit(Op::Move { dst: res, src: c });
            }
        }
        let jend = self.jmp_fwd();
        self.free_to(mark);
        let lelse = self.here();
        self.patch(jelse, lelse);
        let e = self.compile_expr(else_);
        self.emit(Op::Move { dst: res, src: e });
        self.free_to(mark);
        let lend = self.here();
        self.patch(jend, lend);
        res
    }

    /// `isset($a, $b[0], $c->p)`: every operand must be set (short-circuit).
    fn compile_isset(&mut self, vars: &[Expr]) -> Reg {
        let res = self.alloc_temp();
        let mark = self.temp_top;
        let mut false_jumps = Vec::new();
        for v in vars {
            let r = self.compile_isset_one(v);
            false_jumps.push(self.emit(Op::JmpIfFalse { cond: r, target: 0 }));
            self.free_to(mark);
        }
        self.emit(Op::LoadBool { dst: res, val: true });
        let jend = self.jmp_fwd();
        let lfalse = self.here();
        for j in false_jumps {
            self.patch(j, lfalse);
        }
        self.emit(Op::LoadBool {
            dst: res,
            val: false,
        });
        let lend = self.here();
        self.patch(jend, lend);
        res
    }

    fn compile_isset_one(&mut self, e: &Expr) -> Reg {
        match e {
            Expr::Var(id, _) if self.is_this(*id) => {
                let dst = self.alloc_temp();
                self.emit(Op::LoadBool {
                    dst,
                    val: self.cur_class.is_some(),
                });
                dst
            }
            Expr::Var(id, _) if self.is_globals(*id) => {
                let dst = self.alloc_temp();
                self.emit(Op::LoadBool { dst, val: true });
                dst
            }
            Expr::Var(id, _) => {
                let var = self.var_reg(*id);
                let dst = self.alloc_temp();
                self.emit(Op::IssetVar { dst, var });
                dst
            }
            Expr::Temp(t, span) => {
                let var = self.temp_reg(*t, *span);
                let dst = self.alloc_temp();
                self.emit(Op::IssetVar { dst, var });
                dst
            }
            Expr::VarVar { name, .. } => {
                let n = self.compile_expr(name);
                let t = self.alloc_temp();
                self.emit(Op::FetchDynVar {
                    dst: t,
                    name: n,
                    global: false,
                });
                let dst = self.alloc_temp();
                self.emit(Op::IssetVar { dst, var: t });
                dst
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                let b = self.compile_quiet(base);
                let k = self.compile_expr(index);
                let dst = self.alloc_temp();
                self.emit(Op::IssetElem {
                    dst,
                    arr: b,
                    key: k,
                });
                dst
            }
            Expr::Prop { obj, name, .. } => {
                let o = self.compile_quiet(obj);
                let name = self.member_name_ref(name);
                let dst = self.alloc_temp();
                self.emit(Op::IssetProp { dst, obj: o, name });
                dst
            }
            Expr::StaticProp { class, name, span } => {
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return self.null_temp();
                };
                let dst = self.alloc_temp();
                self.emit(Op::IssetStaticProp { dst, class, name });
                dst
            }
            other => {
                self.diags.push(
                    Diagnostic::error(INVALID_WRITE_TARGET, "Cannot use isset() on the result of an expression (you can use \"null !== expression\" instead)")
                        .with_primary(other.span(), "not a variable"),
                );
                self.null_temp()
            }
        }
    }

    /// `empty(e)`: `!isset(e) || !e`, never warns.
    fn compile_empty(&mut self, e: &Expr) -> Reg {
        match e {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let var = self.var_reg(*id);
                let dst = self.alloc_temp();
                self.emit(Op::EmptyVar { dst, var });
                dst
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } => {
                let mark = self.temp_top;
                let b = self.compile_quiet(base);
                let k = self.compile_expr(index);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::EmptyElem {
                    dst,
                    arr: b,
                    key: k,
                });
                dst
            }
            Expr::Prop { obj, name, .. } => {
                let mark = self.temp_top;
                let o = self.compile_quiet(obj);
                let name = self.member_name_ref(name);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::EmptyProp { dst, obj: o, name });
                dst
            }
            // No `EmptyStaticProp` op: `!isset(x) || !x`, spelled out.
            Expr::StaticProp { class, name, span } => {
                let dst = self.alloc_temp();
                let mark = self.temp_top;
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    self.free_to(mark);
                    self.emit(Op::LoadNull { dst });
                    return dst;
                };
                let c = self.alloc_temp();
                self.emit(Op::IssetStaticProp { dst: c, class, name });
                let jset = self.emit(Op::JmpIfTrue { cond: c, target: 0 });
                self.emit(Op::LoadBool { dst, val: true });
                let jend = self.jmp_fwd();
                let lset = self.here();
                self.patch(jset, lset);
                let v = self.alloc_temp();
                let ic = self.ic();
                self.emit(Op::FetchStaticProp {
                    dst: v,
                    class,
                    name,
                    ic,
                });
                self.emit(Op::Not { dst, src: v });
                let lend = self.here();
                self.patch(jend, lend);
                self.free_to(mark);
                dst
            }
            other => {
                let mark = self.temp_top;
                let r = self.compile_quiet(other);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(Op::Not { dst, src: r });
                dst
            }
        }
    }

    /// `match (subject) { conds => body, default => body }`.
    fn compile_match(&mut self, subject: &Expr, arms: &[MatchArm], span: Span) -> Reg {
        let _ = span;
        let res = self.alloc_temp();
        let mark = self.temp_top;
        let subj = self.compile_expr(subject);
        let all_literal = arms.iter().all(|a| {
            a.conds
                .as_ref()
                .is_none_or(|cs| cs.iter().all(|c| literal_value(c, self.interner()).is_some()))
        });
        let default_idx = arms.iter().position(|a| a.conds.is_none());
        // (arm index, jump) for the compare chain / the jump table.
        let mut chain: Vec<(usize, usize)> = Vec::new();
        let mut table: Option<(u32, usize)> = None;
        if all_literal {
            let mut rows: Vec<(Value, u32)> = Vec::new();
            for a in arms {
                if let Some(cs) = &a.conds {
                    for c in cs {
                        rows.push((literal_value(c, self.interner()).expect("literal"), 0));
                    }
                }
            }
            let k = self.push_const(Const::JumpTable(rows));
            let op = self.emit(Op::Switch {
                src: subj,
                table: k,
                default: 0,
                strict: true,
            });
            table = Some((k, op));
        } else {
            for (i, a) in arms.iter().enumerate() {
                let Some(cs) = &a.conds else { continue };
                for c in cs {
                    let m = self.temp_top;
                    let cr = self.compile_expr(c);
                    let t = self.alloc_temp();
                    self.emit(Op::CmpIdentical {
                        dst: t,
                        a: subj,
                        b: cr,
                    });
                    let j = self.emit(Op::JmpIfTrue { cond: t, target: 0 });
                    self.free_to(m);
                    chain.push((i, j));
                }
            }
        }
        let jdefault = self.jmp_fwd();
        let mut starts = Vec::with_capacity(arms.len());
        let mut ends = Vec::with_capacity(arms.len());
        for a in arms {
            starts.push(self.here());
            self.mark_line(a.span);
            let m = self.temp_top;
            let r = self.compile_expr(&a.body);
            self.emit(Op::Move { dst: res, src: r });
            self.free_to(m);
            ends.push(self.jmp_fwd());
        }
        // No default arm: the fall-through raises UnhandledMatchError.
        let default_target = match default_idx {
            Some(i) => starts[i],
            None => {
                let here = self.here();
                self.emit(Op::MatchError { src: subj });
                here
            }
        };
        let lend = self.here();
        match table {
            Some((k, op)) => {
                let mut row = 0;
                if let Const::JumpTable(rows) = &mut self.consts[k as usize] {
                    for (i, a) in arms.iter().enumerate() {
                        if let Some(cs) = &a.conds {
                            for _ in cs {
                                rows[row].1 = starts[i];
                                row += 1;
                            }
                        }
                    }
                }
                self.patch(op, default_target);
            }
            None => {
                for (i, j) in chain {
                    self.patch(j, starts[i]);
                }
            }
        }
        self.patch(jdefault, default_target);
        for j in ends {
            self.patch(j, lend);
        }
        self.free_to(mark);
        res
    }

    // ---- calls ------------------------------------------------------------------

    /// Emit the `Send*` sequence for a call's arguments.
    pub(crate) fn compile_sends(&mut self, args: &[Arg]) {
        let mut pos: u16 = 0;
        for a in args {
            let mark = self.temp_top;
            if a.spread {
                let src = self.compile_expr(&a.value);
                self.emit(Op::SendUnpack { src });
            } else if let Some(name) = a.name {
                let src = match &a.value {
                    Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => self.var_reg(*id),
                    other => self.compile_expr(other),
                };
                let name = self.name_const(name);
                self.emit(Op::SendNamed { name, src });
            } else {
                self.compile_send_positional(pos, &a.value);
                pos += 1;
            }
            self.free_to(mark);
        }
    }

    /// One positional argument: a variable, element or property is sent so
    /// the callee can take it by reference; a call result or nested place
    /// goes through a temporary (silently by value if the parameter is
    /// by-reference); anything else is a value.
    fn compile_send_positional(&mut self, pos: u16, value: &Expr) {
        match value {
            Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id) => {
                let var = self.var_reg(*id);
                self.emit(Op::SendVar { pos, var });
            }
            Expr::Index {
                base,
                index: Some(index),
                ..
            } if matches!(&**base, Expr::Var(id, _) if !self.is_this(*id) && !self.is_globals(*id)) => {
                let Expr::Var(id, _) = &**base else { unreachable!() };
                let arr = self.var_reg(*id);
                let key = self.compile_expr(index);
                self.emit(Op::SendRefElem { pos, arr, key });
            }
            Expr::Prop {
                obj,
                name,
                nullsafe: false,
                ..
            } => {
                let obj = self.compile_expr(obj);
                let name = self.member_name_ref(name);
                self.emit(Op::SendRefProp { pos, obj, name });
            }
            // No `SendRefStaticProp` op — but a static property *is* a shared
            // cell, so binding a register to it and sending that register
            // covers both directions: `SendVar` passes the cell itself to a
            // by-reference parameter and a dereferenced copy otherwise.
            Expr::StaticProp { class, name, span } => {
                let Some((class, name)) = self.static_prop_ref(class, name, *span) else {
                    return;
                };
                let var = self.alloc_temp();
                self.emit(Op::RefStaticProp { dst: var, class, name });
                self.emit(Op::SendVar { pos, var });
            }
            Expr::Index {
                index: Some(_), ..
            }
            | Expr::VarVar { .. } => {
                let var = self.compile_expr(value);
                self.emit(Op::SendVar { pos, var });
            }
            Expr::Call { .. }
            | Expr::MethodCall { .. }
            | Expr::StaticCall { .. }
            | Expr::New { .. } => {
                // A call result: by value, with php's notice when the
                // parameter turns out to be by-reference.
                let src = self.compile_expr(value);
                self.emit(Op::SendFuncResult { pos, src });
            }
            Expr::Index { index: None, span, .. } => {
                unsupported(self.diags, *span, "`[]` append as an argument");
            }
            other => {
                let src = self.compile_expr(other);
                self.emit(Op::SendVal { pos, src });
            }
        }
    }

    /// `name(args...)`: late-bound through the runtime's function table,
    /// with php's two-step lookup for an unqualified name in a namespace.
    fn compile_call(&mut self, name: &Name, args: &[Arg], span: Span) -> Reg {
        let line = self.cur_line;
        // `assert()` is the one call php decides about at compile time: with
        // `zend.assertions=-1` it is not compiled at all, so its argument is
        // never even evaluated.
        // `0` (compile but jump over it) and `-1` (do not compile) differ
        // only in whether the code exists; neither evaluates the argument.
        if self.mx.assertions <= 0 && self.is_global_assert(name) {
            let dst = self.alloc_temp();
            self.emit(Op::LoadBool { dst, val: true });
            return dst;
        }
        self.emit_init_fcall(name);
        self.compile_sends(args);
        // php hands `assert()` the source of the call as a second argument,
        // which is the text an `AssertionError` carries when the caller gave
        // no description of its own.
        if self.mx.assertions > 0 && self.is_global_assert(name) && args.len() == 1 {
            if let Some(text) = self.assert_text(span) {
                let k = self.str_const(&text);
                let reg = self.alloc_temp();
                self.emit(Op::LoadConst { dst: reg, k });
                self.emit(Op::SendVal { pos: 1, src: reg });
            }
        }
        self.emit_do_call(line)
    }

    /// The call as written, for `assert()`'s message. php prints its own
    /// decompilation of the expression; this is the source slice, which is
    /// the same text whenever the source is already spaced php's way.
    fn assert_text(&self, span: Span) -> Option<Vec<u8>> {
        let src = self.mx.source?;
        let (lo, hi) = (span.lo as usize, span.hi as usize);
        if hi > src.len() || lo >= hi {
            return None;
        }
        Some(src[lo..hi].to_vec())
    }

    /// Whether this call names the global `assert()` — the resolver marks an
    /// unqualified name in a namespace with both keys, and only the global
    /// one is php's builtin.
    fn is_global_assert(&self, name: &Name) -> bool {
        let text = match name.resolved {
            Some(Resolved::Func { global_key, .. }) => global_key,
            _ => name.text,
        };
        self.interner().resolve(text).eq_ignore_ascii_case(b"assert")
    }

    /// Begin a call to a written function name, with php's two-step lookup
    /// for an unqualified name inside a namespace.
    fn emit_init_fcall(&mut self, name: &Name) {
        let (k, ns_fallback) = match name.resolved {
            Some(Resolved::Func { ns_key, global_key }) => self.two_step_consts(ns_key, global_key),
            // Not visited by the resolver: as spelled.
            _ => (self.sym_const(self.interner().resolve(name.text)), None),
        };
        let ic = self.ic();
        self.emit(Op::InitFCall {
            name: k,
            ns_fallback,
            ic,
        });
    }

    /// `callee(args...)` where the callee is a runtime value.
    fn compile_dynamic_call(&mut self, callee: &Expr, args: &[Arg]) -> Reg {
        let line = self.cur_line;
        let mark = self.temp_top;
        let callee_reg = self.compile_chain_obj(callee);
        let ic = self.ic();
        self.emit(Op::InitDynCall {
            callee: callee_reg,
            ic,
        });
        self.free_to(mark);
        self.compile_sends(args);
        self.emit_do_call(line)
    }
}

/// Whether a destructuring pattern binds any target by reference (at any depth).
fn pattern_has_ref(items: &[ArrayItem]) -> bool {
    items.iter().any(|it| {
        it.by_ref
            || matches!(&it.value, Some(Expr::Array { items, .. }) if pattern_has_ref(items))
    })
}

/// Whether a member chain (`a->b?->c[0]->d()`) contains a nullsafe link.
fn chain_has_nullsafe(e: &Expr) -> bool {
    match e {
        Expr::Prop { obj, nullsafe, .. } | Expr::MethodCall { obj, nullsafe, .. } => {
            *nullsafe || chain_has_nullsafe(obj)
        }
        Expr::Index { base, .. } => chain_has_nullsafe(base),
        Expr::Call {
            callee: Callee::Expr(c),
            ..
        } => chain_has_nullsafe(c),
        _ => false,
    }
}

/// The compound-assignment kind of a binary operator, if it has one.
fn assign_op_kind(op: BinOp) -> Option<AssignOpKind> {
    Some(match op {
        BinOp::Add => AssignOpKind::Add,
        BinOp::Sub => AssignOpKind::Sub,
        BinOp::Mul => AssignOpKind::Mul,
        BinOp::Div => AssignOpKind::Div,
        BinOp::Mod => AssignOpKind::Mod,
        BinOp::Pow => AssignOpKind::Pow,
        BinOp::Concat => AssignOpKind::Concat,
        BinOp::BitAnd => AssignOpKind::BitAnd,
        BinOp::BitOr => AssignOpKind::BitOr,
        BinOp::BitXor => AssignOpKind::BitXor,
        BinOp::Shl => AssignOpKind::Shl,
        BinOp::Shr => AssignOpKind::Shr,
        BinOp::Coalesce => AssignOpKind::Coalesce,
        _ => return None,
    })
}

/// The three-address op for a plain binary operator, or `None` for the
/// operators lowered to branches by the caller (`&&`, `||`, `xor`, `??`,
/// `|>`).
fn binary_op(op: BinOp) -> Option<fn(Reg, Reg, Reg) -> Op> {
    Some(match op {
        BinOp::Add => |dst, a, b| Op::Add { dst, a, b },
        BinOp::Sub => |dst, a, b| Op::Sub { dst, a, b },
        BinOp::Mul => |dst, a, b| Op::Mul { dst, a, b },
        BinOp::Div => |dst, a, b| Op::Div { dst, a, b },
        BinOp::Mod => |dst, a, b| Op::Mod { dst, a, b },
        BinOp::Pow => |dst, a, b| Op::Pow { dst, a, b },
        BinOp::Concat => |dst, a, b| Op::Concat { dst, a, b },
        BinOp::BitAnd => |dst, a, b| Op::BitAnd { dst, a, b },
        BinOp::BitOr => |dst, a, b| Op::BitOr { dst, a, b },
        BinOp::BitXor => |dst, a, b| Op::BitXor { dst, a, b },
        BinOp::Shl => |dst, a, b| Op::Shl { dst, a, b },
        BinOp::Shr => |dst, a, b| Op::Shr { dst, a, b },
        BinOp::Eq => |dst, a, b| Op::CmpEq { dst, a, b },
        BinOp::Ne => |dst, a, b| Op::CmpNe { dst, a, b },
        BinOp::Identical => |dst, a, b| Op::CmpIdentical { dst, a, b },
        BinOp::NotIdentical => |dst, a, b| Op::CmpNotIdentical { dst, a, b },
        BinOp::Lt => |dst, a, b| Op::CmpLt { dst, a, b },
        BinOp::Le => |dst, a, b| Op::CmpLe { dst, a, b },
        BinOp::Gt => |dst, a, b| Op::CmpGt { dst, a, b },
        BinOp::Ge => |dst, a, b| Op::CmpGe { dst, a, b },
        BinOp::Spaceship => |dst, a, b| Op::Spaceship { dst, a, b },
        BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Coalesce | BinOp::Pipe => return None,
    })
}
