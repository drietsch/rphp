//! `try`/`catch`/`finally` lowering (plan E5, CONTRACT.md §5) and the
//! control-flow exits that cross a `finally` body.
//!
//! Layout of one `try` statement:
//!
//! ```text
//! try_lo:   <try body>
//! try_hi:   [state = None;] Jmp entry|end
//! handler_i: <catch body i>; [state = None;] Jmp entry|end
//!           <trampolines for exits recorded while compiling the bodies>
//! entry:    <finally body>
//!           FinallyEnd {state, payload, targets}
//! end:
//! ```
//!
//! A `return`, `break` or `continue` inside the protected range of a region
//! with a `finally` cannot leave directly: it stores its action in the
//! region's `state`/`payload` registers and jumps to the finally entry;
//! `FinallyEnd` performs it. Crossing several regions is chained with
//! trampolines (`FinallyState::Jump` + a `JumpTable` index), innermost
//! first, and only the outermost crossing performs the real action — the
//! runtime never chains finally bodies itself. A `return` value travels in
//! the *outermost* crossed region's payload register, which every inner
//! trampoline leaves alone.
//!
//! Catch types are the resolved class names; matching happens at run time
//! by `instanceof` without autoload.

use rphp_ast::v2::{Builtin, Catch, Expr, Name, Resolved, Stmt, Type, TypeKind};
use rphp_bytecode::{
    BuiltinType, CatchClause, Const, ExRegion, Finally, FinallyState, FnFlags, Op, Reg, TypeDecl,
};
use rphp_value::Value;

use crate::func::FnCompiler;
use crate::unsupported;

/// An open `try` region with a `finally`, while its protected bodies are
/// being compiled.
pub(crate) struct FinallyCtx {
    /// The `FinallyState` register.
    pub(crate) state: Reg,
    /// The payload register (pending exception / return value / jump index).
    pub(crate) payload: Reg,
    /// Jumps to the finally entry to patch once it is known.
    pub(crate) entry_jumps: Vec<usize>,
    /// Exits recorded inside the protected range, by `Jump` index.
    pub(crate) exits: Vec<Exit>,
}

/// A control-flow exit that crosses this region's `finally`.
#[derive(Clone, Copy)]
pub(crate) struct Exit {
    /// The outermost region the exit crosses (index into the finally stack).
    pub(crate) regions_from: usize,
    /// What happens once every crossed `finally` has run.
    pub(crate) action: ExitAction,
}

/// The action performed after the crossed finally bodies.
#[derive(Clone, Copy)]
pub(crate) enum ExitAction {
    /// Return the value held in the outermost crossed region's payload.
    Return,
    /// `break` (true) / `continue` (false) to the loop at this index of the
    /// loop stack.
    Jump { loop_idx: usize, is_break: bool },
}

impl FnCompiler<'_> {
    /// The FQN bytes of a class-position name (the resolver's `fqn`, else
    /// the spelling).
    pub(crate) fn class_fqn(&self, name: &Name) -> Vec<u8> {
        let id = match name.resolved {
            Some(Resolved::Class { fqn, .. }) => fqn,
            _ => name.text,
        };
        self.interner().resolve(id).to_vec()
    }

    /// Lower a declared type to its bytecode form (class names as FQNs).
    pub(crate) fn lower_type(&self, t: &Type) -> TypeDecl {
        match &t.kind {
            TypeKind::Named(name) => TypeDecl::Named(self.class_fqn(name).into_boxed_slice()),
            TypeKind::Builtin(b) => TypeDecl::Builtin(match b {
                Builtin::Int => BuiltinType::Int,
                Builtin::Float => BuiltinType::Float,
                Builtin::String => BuiltinType::String,
                Builtin::Bool => BuiltinType::Bool,
                Builtin::Array => BuiltinType::Array,
                Builtin::Object => BuiltinType::Object,
                Builtin::Mixed => BuiltinType::Mixed,
                Builtin::Void => BuiltinType::Void,
                Builtin::Never => BuiltinType::Never,
                Builtin::Null => BuiltinType::Null,
                Builtin::True => BuiltinType::True,
                Builtin::False => BuiltinType::False,
                Builtin::Callable => BuiltinType::Callable,
                Builtin::Iterable => BuiltinType::Iterable,
                Builtin::SelfTy => BuiltinType::SelfTy,
                Builtin::StaticTy => BuiltinType::StaticTy,
                Builtin::ParentTy => BuiltinType::ParentTy,
            }),
            TypeKind::Nullable(inner) => TypeDecl::Nullable(Box::new(self.lower_type(inner))),
            TypeKind::Union(parts) => TypeDecl::Union(parts.iter().map(|p| self.lower_type(p)).collect()),
            TypeKind::Intersection(parts) => {
                TypeDecl::Intersection(parts.iter().map(|p| self.lower_type(p)).collect())
            }
        }
    }

    /// `try { body } catch (…) { … } finally { … }`.
    pub(crate) fn compile_try(&mut self, body: &[Stmt], catches: &[Catch], finally: Option<&[Stmt]>) {
        let mark = self.temp_top;
        let has_finally = finally.is_some();
        let (state, payload) = if has_finally {
            let s = self.alloc_temp();
            let p = self.alloc_temp();
            self.flags |= FnFlags::HAS_FINALLY;
            self.finallys.push(FinallyCtx {
                state: s,
                payload: p,
                entry_jumps: Vec::new(),
                exits: Vec::new(),
            });
            (s, p)
        } else {
            (0, 0)
        };
        let mut end_jumps: Vec<usize> = Vec::new();
        let try_lo = self.here();
        self.compile_nested(body);
        let try_hi = self.here();
        self.emit_normal_exit(has_finally, &mut end_jumps);
        let mut clauses = Vec::with_capacity(catches.len());
        for c in catches {
            let handler = self.here();
            let types: Vec<u32> = c
                .types
                .iter()
                .map(|n| {
                    let fqn = self.class_fqn(n);
                    self.sym_const(&fqn)
                })
                .collect();
            let dst = c.var.map(|v| self.var_reg(v));
            self.mark_line(c.span);
            self.compile_nested(&c.body);
            self.emit_normal_exit(has_finally, &mut end_jumps);
            clauses.push(CatchClause { types, handler, dst });
        }
        let mut fin = None;
        if let Some(fbody) = finally {
            let ctx = self.finallys.pop().expect("finally stack");
            // Trampolines for the exits recorded in the bodies: each one
            // continues the exit past this region.
            let mut targets: Vec<(Value, u32)> = Vec::with_capacity(ctx.exits.len());
            for (k, exit) in ctx.exits.iter().enumerate() {
                targets.push((Value::Int(k as i64), self.here()));
                self.exit_via(exit.regions_from, exit.action, None);
            }
            let entry = self.here();
            for j in ctx.entry_jumps {
                self.patch(j, entry);
            }
            self.compile_nested(fbody);
            let k = self.push_const(Const::JumpTable(targets));
            self.emit(Op::FinallyEnd {
                state,
                payload,
                targets: k,
            });
            let hi = self.here();
            fin = Some(Finally {
                lo: entry,
                hi,
                entry,
                end: hi,
                state,
                payload,
            });
        }
        let end = self.here();
        for j in end_jumps {
            self.patch(j, end);
        }
        // Appended when the statement closes: nested regions precede their
        // enclosing ones (innermost-first, as the unwinder expects).
        self.ex_regions.push(ExRegion {
            try_lo,
            try_hi,
            catches: clauses,
            finally: fin,
        });
        self.free_to(mark);
    }

    /// The end of a try body / catch handler: run the finally body with
    /// nothing pending, or skip to the end.
    fn emit_normal_exit(&mut self, has_finally: bool, end_jumps: &mut Vec<usize>) {
        if has_finally {
            let ctx = self.finallys.last().expect("finally stack");
            let (state, _) = (ctx.state, ctx.payload);
            let k = self.push_const(Const::Int(FinallyState::None.code()));
            self.emit(Op::LoadConst { dst: state, k });
            let j = self.jmp_fwd();
            self.finallys.last_mut().expect("finally stack").entry_jumps.push(j);
        } else {
            let j = self.jmp_fwd();
            end_jumps.push(j);
        }
    }

    /// Perform `action` from the current point, running the finally bodies
    /// of `self.finallys[regions_from..]` (innermost first) on the way.
    /// `value` is the returned register when no region is crossed at all
    /// (the direct `Ret`); otherwise the value already sits in the
    /// outermost crossed region's payload.
    fn exit_via(&mut self, regions_from: usize, action: ExitAction, value: Option<Reg>) {
        let n = self.finallys.len();
        if regions_from >= n {
            match action {
                ExitAction::Return => {
                    self.emit(Op::Ret { src: value });
                }
                ExitAction::Jump { loop_idx, is_break } => {
                    let j = self.jmp_fwd();
                    let target = &mut self.loops[loop_idx];
                    if is_break || target.is_switch {
                        target.breaks.push(j);
                    } else {
                        target.continues.push(j);
                    }
                }
            }
            return;
        }
        let inner = n - 1;
        let (state, payload) = {
            let c = &self.finallys[inner];
            (c.state, c.payload)
        };
        if inner == regions_from && matches!(action, ExitAction::Return) {
            // The outermost crossing: the value is (or is put) in this
            // region's payload; `FinallyEnd` returns it.
            if let Some(v) = value {
                self.emit(Op::Move { dst: payload, src: v });
            }
            let k = self.push_const(Const::Int(FinallyState::Return.code()));
            self.emit(Op::LoadConst { dst: state, k });
        } else {
            let idx = self.finallys[inner].exits.len();
            self.finallys[inner].exits.push(Exit { regions_from, action });
            let ks = self.push_const(Const::Int(FinallyState::Jump.code()));
            self.emit(Op::LoadConst { dst: state, k: ks });
            let kp = self.push_const(Const::Int(idx as i64));
            self.emit(Op::LoadConst { dst: payload, k: kp });
        }
        let j = self.jmp_fwd();
        self.finallys[inner].entry_jumps.push(j);
    }

    /// `return [expr];` honouring enclosing `finally` bodies.
    pub(crate) fn compile_return(&mut self, value: Option<&Expr>) {
        let mark = self.temp_top;
        let src = value.map(|e| self.compile_expr(e));
        if self.finallys.is_empty() {
            self.emit(Op::Ret { src });
            self.free_to(mark);
            return;
        }
        // The value travels in the outermost region's payload.
        let outer_payload = self.finallys[0].payload;
        match src {
            Some(r) => {
                self.emit(Op::Move {
                    dst: outer_payload,
                    src: r,
                });
            }
            None => {
                self.emit(Op::LoadNull { dst: outer_payload });
            }
        }
        self.exit_via(0, ExitAction::Return, None);
        self.free_to(mark);
    }

    /// `break`/`continue` to `self.loops[loop_idx]`, honouring the `finally`
    /// bodies opened inside that loop. Returns `false` when no finally is
    /// crossed (the caller emits the plain jump).
    pub(crate) fn exit_loop_via_finally(&mut self, loop_idx: usize, is_break: bool) -> bool {
        let from = self.loops[loop_idx].finally_depth;
        if from >= self.finallys.len() {
            return false;
        }
        self.exit_via(from, ExitAction::Jump { loop_idx, is_break }, None);
        true
    }

    /// `goto` out of a protected range with a `finally` is not lowered.
    pub(crate) fn check_goto_finally(&mut self, span: rphp_span::Span) -> bool {
        if self.finallys.is_empty() {
            return true;
        }
        unsupported(self.diags, span, "goto inside a try with finally");
        false
    }
}
