//! Expression lowering.
//!
//! [`FnCompiler::compile_expr`] returns the register holding the value.
//! Invariant: the call leaves exactly one extra live temporary (the result)
//! when the result is a fresh temp, or zero when it is an existing variable
//! register. Every construct outside the lowered slice is reported as
//! `RPHP_E0300` and yields a `null` temporary so lowering can continue.

use rphp_ast::v2::{
    Arg, ArrayItem, BinOp, Callee, ClassRef, Expr, InterpPart, MemberName, Name, NameKind,
    NewTarget, UnOp,
};
use rphp_bytecode::{ClassId, Const, FuncId, Op, Reg};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::IdentId;
use rphp_span::Span;
use rphp_value::Str;

use crate::func::{CallTarget, FnCompiler};
use crate::{
    unsupported, BY_REF_NOT_VARIABLE, INVALID_APPEND_READ, INVALID_SCOPE, NESTED_ARRAY_WRITE,
    UNDEFINED_CLASS, UNDEFINED_METHOD,
};

impl FnCompiler<'_> {
    /// Compile `e`, returning the register that holds its value.
    pub(crate) fn compile_expr(&mut self, e: &Expr) -> Reg {
        self.mark_line(e.span());
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
            Expr::Var(id, _) => self.var_reg(*id),
            Expr::Assign {
                target,
                value,
                op,
                by_ref,
                span,
            } => {
                if let Some(op) = op {
                    return self
                        .unsupported_expr(*span, &format!("compound assignment `{}=`", op.name()));
                }
                if *by_ref {
                    return self.unsupported_expr(*span, "assignment by reference");
                }
                self.compile_assign(target, value, *span)
            }
            Expr::Unary { op, expr, span } => {
                if !matches!(op, UnOp::Neg | UnOp::Not) {
                    return self
                        .unsupported_expr(*span, &format!("unary operator `{}`", op.name()));
                }
                let mark = self.temp_top;
                let r = self.compile_expr(expr);
                self.free_to(mark);
                let dst = self.alloc_temp();
                self.emit(match op {
                    UnOp::Neg => Op::Neg { dst, src: r },
                    _ => Op::Not { dst, src: r },
                });
                dst
            }
            Expr::Binary { op, lhs, rhs, span } => match op {
                BinOp::And => self.compile_and(lhs, rhs),
                BinOp::Or => self.compile_or(lhs, rhs),
                _ => {
                    let Some(make) = binary_op(*op) else {
                        return self
                            .unsupported_expr(*span, &format!("binary operator `{}`", op.name()));
                    };
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
                NewTarget::Ref(ClassRef::Named(name)) => self.compile_new(name, args, *span),
                NewTarget::Ref(ClassRef::SelfKw(_)) => self.unsupported_expr(*span, "new self"),
                NewTarget::Ref(ClassRef::Static(_)) => self.unsupported_expr(*span, "new static"),
                NewTarget::Ref(ClassRef::Parent(_)) => self.unsupported_expr(*span, "new parent"),
                NewTarget::Ref(ClassRef::Expr(_)) => {
                    self.unsupported_expr(*span, "dynamic class instantiation")
                }
                NewTarget::Anon(_) => self.unsupported_expr(*span, "anonymous class"),
            },
            Expr::Prop {
                obj,
                name,
                nullsafe,
                span,
            } => {
                if *nullsafe {
                    return self.unsupported_expr(*span, "nullsafe property fetch");
                }
                let Some(name) = self.member_ident(name, "dynamic property name") else {
                    return self.null_temp();
                };
                self.compile_prop_get(obj, name)
            }
            Expr::MethodCall {
                obj,
                name,
                args,
                nullsafe,
                span,
            } => {
                if *nullsafe {
                    return self.unsupported_expr(*span, "nullsafe method call");
                }
                let Some(name) = self.member_ident(name, "dynamic method name") else {
                    return self.null_temp();
                };
                self.compile_method_call(obj, name, args)
            }
            Expr::StaticCall {
                class,
                name,
                args,
                span,
            } => {
                let Some(method) = self.member_ident(name, "dynamic static method name") else {
                    return self.null_temp();
                };
                self.compile_static_call(class, method, args, *span)
            }
            Expr::InstanceOf { expr, class, span } => self.compile_instance_of(expr, class, *span),

            // ----- everything else is not lowered yet ---------------------------
            Expr::ShellExec { span, .. } => self.unsupported_expr(*span, "shell execution"),
            Expr::VarVar { span, .. } => self.unsupported_expr(*span, "variable variable"),
            Expr::StaticProp { span, .. } => self.unsupported_expr(*span, "static property"),
            Expr::ClassConst { span, .. } => self.unsupported_expr(*span, "class constant"),
            Expr::Const(name) => self.unsupported_expr(name.span, "constant"),
            Expr::MagicConst { kind, span } => {
                self.unsupported_expr(*span, &format!("magic constant {}", kind.as_str()))
            }
            Expr::Callable { span, .. } => self.unsupported_expr(*span, "first-class callable"),
            Expr::Clone { span, .. } => self.unsupported_expr(*span, "clone"),
            Expr::Ternary { span, .. } => self.unsupported_expr(*span, "ternary"),
            Expr::Isset { span, .. } => self.unsupported_expr(*span, "isset"),
            Expr::Empty { span, .. } => self.unsupported_expr(*span, "empty"),
            Expr::Include { kind, span, .. } => self.unsupported_expr(*span, kind.as_str()),
            Expr::Eval { span, .. } => self.unsupported_expr(*span, "eval"),
            Expr::Exit { span, .. } => self.unsupported_expr(*span, "exit"),
            Expr::Print { span, .. } => self.unsupported_expr(*span, "print"),
            Expr::Match { span, .. } => self.unsupported_expr(*span, "match"),
            Expr::Throw { span, .. } => self.unsupported_expr(*span, "throw"),
            Expr::Yield { span, .. } => self.unsupported_expr(*span, "yield"),
            Expr::YieldFrom { span, .. } => self.unsupported_expr(*span, "yield from"),
            Expr::Let { span, .. } => self.unsupported_expr(*span, "HIR let"),
            Expr::Temp(_, span) => self.unsupported_expr(*span, "HIR temporary"),
            Expr::Seq(_, span) => self.unsupported_expr(*span, "HIR sequence"),
            Expr::Error(span) => self.unsupported_expr(*span, "parse-error placeholder"),
        }
    }

    // ---- literals -----------------------------------------------------------

    fn load_const(&mut self, c: Const) -> Reg {
        let k = self.push_const(c);
        let dst = self.alloc_temp();
        self.emit(Op::LoadConst { dst, k });
        dst
    }

    fn load_str(&mut self, id: IdentId) -> Reg {
        let bytes = self.interner().resolve(id);
        self.load_const(Const::Str(Str::new(bytes)))
    }

    /// `"a $b {$c->d}"`: a left-associative concatenation seeded with an empty
    /// string, so the result is always a string (a lone `"$x"` is `(string)$x`,
    /// not the raw value of `$x`).
    fn compile_interp(&mut self, parts: &[InterpPart]) -> Reg {
        let mark = self.temp_top;
        let mut acc = self.load_const(Const::Str(Str::new(b"")));
        for part in parts {
            let r = match part {
                InterpPart::Lit(id, _) => self.load_str(*id),
                InterpPart::Expr(e) => self.compile_expr(e),
            };
            // Release the part's temporaries and the previous accumulator;
            // the new accumulator takes the accumulator's register (`dst ==
            // acc`, which the runtime handles like any `a == dst` operand).
            self.free_to(mark);
            let dst = self.alloc_temp();
            self.emit(Op::Concat { dst, a: acc, b: r });
            acc = dst;
        }
        acc
    }

    // ---- assignment ---------------------------------------------------------

    /// `target = value` for the lowered target shapes: a variable, `$var[i]`
    /// / `$var[]`, or `obj->name`.
    fn compile_assign(&mut self, target: &Expr, value: &Expr, span: Span) -> Reg {
        match target {
            Expr::Var(id, _) => {
                let dst = self.var_reg(*id);
                let mark = self.temp_top;
                let r = self.compile_expr(value);
                if r != dst {
                    self.emit(Op::Move { dst, src: r });
                }
                self.free_to(mark);
                // The assignment expression evaluates to the assigned register.
                dst
            }
            Expr::Index { base, index, .. } => {
                self.compile_index_assign(base, index.as_deref(), value, span)
            }
            Expr::Prop {
                obj,
                name,
                nullsafe,
                span: pspan,
            } => {
                if *nullsafe {
                    return self.unsupported_expr(*pspan, "nullsafe property write");
                }
                let Some(name) = self.member_ident(name, "dynamic property name") else {
                    return self.null_temp();
                };
                self.compile_prop_set(obj, name, value)
            }
            Expr::Array { .. } => self.unsupported_expr(span, "destructuring assignment"),
            Expr::VarVar { .. } => self.unsupported_expr(span, "variable-variable assignment"),
            Expr::StaticProp { .. } => self.unsupported_expr(span, "static property assignment"),
            other => self.unsupported_expr(other.span(), "assignment target"),
        }
    }

    /// A literal member name, or `None` after reporting the dynamic form.
    fn member_ident(&mut self, name: &MemberName, what: &str) -> Option<IdentId> {
        match name {
            MemberName::Ident(id, _) => Some(*id),
            MemberName::Expr(e) => {
                unsupported(self.diags, e.span(), what);
                None
            }
        }
    }

    /// The positional argument expressions of a call; named arguments and
    /// unpacking are reported.
    fn plain_args<'e>(&mut self, args: &'e [Arg]) -> Vec<&'e Expr> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            if a.name.is_some() {
                unsupported(self.diags, a.span, "named argument");
            }
            if a.spread {
                unsupported(self.diags, a.span, "argument unpacking");
            }
            out.push(&a.value);
        }
        out
    }

    // ---- arrays ---------------------------------------------------------------

    /// `[ ... ]` / `array( ... )`: build a fresh array, then fill it element by
    /// element preserving source order.
    fn compile_array(&mut self, items: &[ArrayItem]) -> Reg {
        let dst = self.alloc_temp();
        self.emit(Op::NewArray { dst });
        let mark = self.temp_top;
        for item in items {
            if item.by_ref {
                unsupported(self.diags, item.span, "by-reference array element");
            }
            if item.spread {
                unsupported(self.diags, item.span, "array spread");
            }
            let Some(value) = &item.value else {
                unsupported(self.diags, item.span, "skipped array element");
                continue;
            };
            match &item.key {
                Some(key) => {
                    let kr = self.compile_expr(key);
                    let vr = self.compile_expr(value);
                    self.emit(Op::ArraySet {
                        arr: dst,
                        key: kr,
                        value: vr,
                    });
                }
                None => {
                    let vr = self.compile_expr(value);
                    self.emit(Op::ArrayPush {
                        arr: dst,
                        value: vr,
                    });
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
        let br = self.compile_expr(base);
        let kr = self.compile_expr(index);
        self.free_to(mark);
        let dst = self.alloc_temp();
        self.emit(Op::ArrayGet {
            dst,
            base: br,
            key: kr,
        });
        dst
    }

    /// `base[index] = value` / `base[] = value`. Only a plain `$var` base is
    /// supported so far (nested-subscript writes need an lvalue chain). The
    /// expression evaluates to the assigned value.
    fn compile_index_assign(
        &mut self,
        base: &Expr,
        index: Option<&Expr>,
        value: &Expr,
        span: Span,
    ) -> Reg {
        let Expr::Var(id, _) = base else {
            self.diags.push(
                Diagnostic::error(
                    NESTED_ARRAY_WRITE,
                    "nested array assignment is not supported yet",
                )
                .with_primary(span, "write through a single `$var[...]` for now"),
            );
            return self.null_temp();
        };
        let arr = self.var_reg(*id);
        // The assigned value is the result of the expression, so keep it live
        // while the (freed) index temp sits above it.
        let vr = self.compile_expr(value);
        let key_mark = self.temp_top;
        match index {
            Some(index) => {
                let kr = self.compile_expr(index);
                self.emit(Op::ArraySet {
                    arr,
                    key: kr,
                    value: vr,
                });
            }
            None => {
                self.emit(Op::ArrayPush { arr, value: vr });
            }
        }
        self.free_to(key_mark); // release the index temp, keep `vr`
        vr
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
        let jend = self.emit(Op::Jmp { target: 0 });
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
        let jend = self.emit(Op::Jmp { target: 0 });
        // True path: lhs was truthy -> result is `true`.
        let ltrue = self.here();
        self.patch(jt, ltrue);
        self.emit(Op::LoadBool { dst, val: true });
        let lend = self.here();
        self.patch(jend, lend);
        dst
    }

    // ---- calls ------------------------------------------------------------------

    /// Stage `args` into the contiguous window `base ..= base+argc-1` (the
    /// current temp top), leaving the window allocated.
    fn stage_args(&mut self, args: &[&Expr]) -> (Reg, u16) {
        let argc = args.len() as u16;
        let base = self.temp_top;
        self.set_top(base + argc);
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        (base, argc)
    }

    /// `name(args...)`: a user function takes precedence over a builtin of
    /// the same name; a builtin is matched case-insensitively by its bytes.
    /// Only unqualified and fully qualified names resolve (namespaces are
    /// resolved in F4).
    fn compile_call(&mut self, name: &Name, args: &[Arg], span: Span) -> Reg {
        if !matches!(name.kind, NameKind::Unqualified | NameKind::FullyQualified) {
            return self.unsupported_expr(name.span, "namespaced function call");
        }
        let args = self.plain_args(args);
        let argc = args.len() as u16;
        let name = name.text;

        let target = if let Some(&id) = self.mx.func_map.get(&name) {
            self.check_user_arity(name, id, argc, span);
            Some(CallTarget::User(id))
        } else if let Some(sig) = self.mx.natives.native(self.interner().resolve(name)) {
            self.check_native_arity(name, &sig, argc, span);
            Some(CallTarget::Native {
                id: sig.id,
                by_ref: sig.by_ref,
            })
        } else {
            self.diags.push(
                Diagnostic::error(
                    codes::UNDEFINED_FUNCTION,
                    format!(
                        "call to undefined function {}()",
                        self.interner().resolve_lossy(name)
                    ),
                )
                .with_primary(span, "not defined"),
            );
            None
        };
        let Some(target) = target else {
            return self.null_temp();
        };

        // Builtins may declare by-reference parameters (user by-ref is not
        // modelled yet). A call that actually passes an argument into a by-ref
        // slot needs a write-back, handled on a separate path.
        let by_ref = match &target {
            CallTarget::Native { by_ref, .. } => *by_ref,
            CallTarget::User(_) => 0,
        };
        if let (true, CallTarget::Native { id, .. }) =
            ((0..argc).any(|i| is_by_ref(by_ref, i)), &target)
        {
            return self.compile_native_by_ref(name, *id, by_ref, &args);
        }

        let (base, argc) = self.stage_args(&args);
        // Free the window; the result lands in `dst == base` (the runtime copies
        // the args into the callee frame before writing the return value).
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        let op = match target {
            CallTarget::User(func) => Op::Call {
                dst,
                func,
                base,
                argc,
            },
            CallTarget::Native { id, .. } => Op::CallNative {
                dst,
                native: id,
                base,
                argc,
            },
        };
        self.emit(op);
        dst
    }

    /// Lower a builtin call that passes one or more arguments **by reference**
    /// (`sort($a)`, `array_push($a, …)`, `preg_match($p, $s, $m)`). A by-ref
    /// argument must be a plain variable; its value is copied into the call
    /// window, and after the call the (mutated) window slot is copied back into
    /// that variable. The result is brought down to a single temporary so the
    /// usual "the result is the top live temp" invariant still holds.
    fn compile_native_by_ref(
        &mut self,
        name: IdentId,
        native: u32,
        by_ref: u32,
        args: &[&Expr],
    ) -> Reg {
        let argc = args.len() as u16;
        let base = self.temp_top;
        self.set_top(base + argc);
        // (variable register, window slot) pairs to copy back after the call.
        let mut write_backs: Vec<(Reg, Reg)> = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            if is_by_ref(by_ref, i as u16) {
                if let Expr::Var(id, _) = arg {
                    let vr = self.var_reg(*id);
                    self.emit(Op::Move { dst: slot, src: vr });
                    write_backs.push((vr, slot));
                    continue;
                }
                self.diags.push(
                    Diagnostic::error(
                        BY_REF_NOT_VARIABLE,
                        format!(
                            "{}(): only a variable can be passed by reference",
                            self.interner().resolve_lossy(name)
                        ),
                    )
                    .with_primary(arg.span(), "not a variable"),
                );
            }
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        // The result goes into a temp ABOVE the window, so it cannot alias a
        // by-ref slot the runtime writes back into the window.
        let dst_high = self.alloc_temp();
        debug_assert_eq!(dst_high, base + argc);
        self.emit(Op::CallNative {
            dst: dst_high,
            native,
            base,
            argc,
        });
        // Copy each mutated by-ref slot back into its variable.
        for (vr, slot) in &write_backs {
            self.emit(Op::Move {
                dst: *vr,
                src: *slot,
            });
        }
        // Bring the result down to `base`, releasing the window and the high temp.
        self.emit(Op::Move {
            dst: base,
            src: dst_high,
        });
        self.free_to(base + 1);
        base
    }

    /// Lower `callee(args...)` where the callee is a runtime value (a closure or
    /// callable string). The callee is evaluated first and kept live below the
    /// argument window; the runtime resolves and invokes it.
    fn compile_dynamic_call(&mut self, callee: &Expr, args: &[Arg]) -> Reg {
        let args = self.plain_args(args);
        let callee_reg = self.compile_expr(callee);
        // Stage args into a fresh window above the (still-live) callee register.
        let (base, argc) = self.stage_args(&args);
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        self.emit(Op::CallDynamic {
            dst,
            callee: callee_reg,
            base,
            argc,
        });
        dst
    }

    // ---- objects ------------------------------------------------------------------

    /// Resolve a written class name to a declared class; `None` for a
    /// namespaced spelling (F4) or an undeclared class (reported when
    /// `report_undefined`).
    fn class_by_name(
        &mut self,
        name: &Name,
        span: Span,
        report_undefined: bool,
    ) -> Option<ClassId> {
        if !matches!(name.kind, NameKind::Unqualified | NameKind::FullyQualified) {
            unsupported(self.diags, name.span, "namespaced class name");
            return None;
        }
        let t = self.mx.class_ctx.map.get(&name.text).copied();
        if t.is_none() && report_undefined {
            self.diags.push(
                Diagnostic::error(
                    UNDEFINED_CLASS,
                    format!(
                        "class \"{}\" not found",
                        self.interner().resolve_lossy(name.text)
                    ),
                )
                .with_primary(span, "not defined"),
            );
        }
        t
    }

    /// `new Class(args...)`: allocate the instance with its default properties,
    /// then — if the class declares a constructor — invoke `__construct` with the
    /// arguments, discarding its result. Evaluates to the new object.
    fn compile_new(&mut self, class: &Name, args: &[Arg], span: Span) -> Reg {
        let args = self.plain_args(args);
        let Some(cid) = self.class_by_name(class, span, true) else {
            return self.null_temp();
        };
        let dst = self.alloc_temp();
        self.emit(Op::New { dst, class: cid });
        if self.mx.class_ctx.has_ctor[cid as usize] {
            // Stage constructor args in a window *above* the object register, so
            // the object (the result) is never clobbered.
            let (base, argc) = self.stage_args(&args);
            self.free_to(base);
            let ret = self.alloc_temp(); // constructor result, discarded
            let method = self.push_const(Const::Str(Str::new(b"__construct")));
            self.emit(Op::MethodCall {
                dst: ret,
                obj: dst,
                method,
                base,
                argc,
            });
            self.free_to(dst + 1); // release the window and discarded result
        }
        dst
    }

    /// `obj->name` property read.
    fn compile_prop_get(&mut self, obj: &Expr, name: IdentId) -> Reg {
        let mark = self.temp_top;
        let obj_reg = self.compile_expr(obj);
        self.free_to(mark);
        let dst = self.alloc_temp();
        let name = self.name_const(name);
        self.emit(Op::PropGet {
            dst,
            obj: obj_reg,
            name,
        });
        dst
    }

    /// `obj->name = value`. The value is the expression's result; the write goes
    /// through the object's shared cell (objects are reference handles), so `obj`
    /// may be any expression, not just a variable.
    fn compile_prop_set(&mut self, obj: &Expr, name: IdentId, value: &Expr) -> Reg {
        let vr = self.compile_expr(value);
        let obj_mark = self.temp_top;
        let obj_reg = self.compile_expr(obj);
        let name = self.name_const(name);
        self.emit(Op::PropSet {
            obj: obj_reg,
            name,
            value: vr,
        });
        self.free_to(obj_mark); // drop the object temp, keep the value
        vr
    }

    /// `obj->method(args...)`. The object is evaluated and kept live below the
    /// argument window; the runtime binds it to the callee's `$this`.
    fn compile_method_call(&mut self, obj: &Expr, method: IdentId, args: &[Arg]) -> Reg {
        let args = self.plain_args(args);
        let obj_reg = self.compile_expr(obj);
        let (base, argc) = self.stage_args(&args);
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        let method = self.name_const(method);
        self.emit(Op::MethodCall {
            dst,
            obj: obj_reg,
            method,
            base,
            argc,
        });
        dst
    }

    /// The class a `self`/`parent`/name reference denotes at compile time,
    /// with the scope errors of a scoped call. `None` after a diagnostic (or,
    /// for `instanceof`, for an unknown class name).
    fn scoped_class(&mut self, class: &ClassRef, span: Span, what: &str) -> Option<ClassId> {
        match class {
            ClassRef::SelfKw(_) => {
                if self.cur_class.is_none() {
                    self.scope_error(&format!("cannot use \"self{what}\" outside a class"), span);
                }
                self.cur_class
            }
            ClassRef::Parent(_) => match self.cur_class {
                None => {
                    self.scope_error(
                        &format!("cannot use \"parent{what}\" outside a class"),
                        span,
                    );
                    None
                }
                Some(c) => {
                    let p = self.mx.class_ctx.parent[c as usize];
                    if p.is_none() && what == "::" {
                        self.scope_error("current class has no parent", span);
                    }
                    p
                }
            },
            ClassRef::Named(name) => self.class_by_name(name, span, what == "::"),
            ClassRef::Static(s) => {
                unsupported(self.diags, *s, "late static binding (static::)");
                None
            }
            ClassRef::Expr(e) => {
                unsupported(self.diags, e.span(), "dynamic class reference");
                None
            }
        }
    }

    /// `class::method(args...)` — a scoped (non-virtual) call. Resolves the
    /// target class (`self`/`parent`/name) and the method (compile time, walking
    /// the chain), then forwards the current `$this` (register 0 in a method) and
    /// the arguments to the resolved function.
    fn compile_static_call(
        &mut self,
        class: &ClassRef,
        method: IdentId,
        args: &[Arg],
        span: Span,
    ) -> Reg {
        let args = self.plain_args(args);
        let target = self.scoped_class(class, span, "::");
        let func: Option<FuncId> = target.and_then(|c| {
            self.mx
                .class_ctx
                .resolve_method(c, self.interner().resolve(method))
        });
        let Some(func) = func else {
            if target.is_some() {
                self.diags.push(
                    Diagnostic::error(
                        UNDEFINED_METHOD,
                        format!(
                            "call to undefined method {}()",
                            self.interner().resolve_lossy(method)
                        ),
                    )
                    .with_primary(span, "no such method"),
                );
            }
            return self.null_temp();
        };

        // `$this` to forward: register 0 inside a method, otherwise a fresh null.
        let this_reg = if self.cur_class.is_some() {
            0
        } else {
            self.null_temp()
        };
        let (base, argc) = self.stage_args(&args);
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        self.emit(Op::StaticCall {
            dst,
            this: this_reg,
            func,
            base,
            argc,
        });
        dst
    }

    /// `expr instanceof Class`. The class name resolves at compile time; an
    /// unknown name yields a constant `false` (PHP does not error there), while
    /// `self`/`parent` outside a class is a hard error.
    fn compile_instance_of(&mut self, expr: &Expr, class: &ClassRef, span: Span) -> Reg {
        let target = self.scoped_class(class, span, "");
        let mark = self.temp_top;
        let obj_reg = self.compile_expr(expr);
        self.free_to(mark);
        let dst = self.alloc_temp();
        match target {
            Some(cid) => {
                self.emit(Op::InstanceOf {
                    dst,
                    obj: obj_reg,
                    class: cid,
                });
            }
            None => {
                self.emit(Op::LoadBool { dst, val: false });
            }
        }
        dst
    }

    /// Push a `self::`/`parent::` scope-misuse diagnostic.
    fn scope_error(&mut self, msg: &str, span: Span) {
        self.diags
            .push(Diagnostic::error(INVALID_SCOPE, msg).with_primary(span, "invalid scope"));
    }
}

/// Whether argument position `i` is a by-reference slot of `mask` (positions
/// beyond the 32-bit mask are by value: the mask cannot describe them).
fn is_by_ref(mask: u32, i: u16) -> bool {
    i < 32 && mask & (1 << i) != 0
}

/// The three-address op for a plain binary operator, or `None` for the
/// operators the slice does not lower (`&&`/`||` are lowered to branches by
/// the caller).
fn binary_op(op: BinOp) -> Option<fn(Reg, Reg, Reg) -> Op> {
    Some(match op {
        BinOp::Add => |dst, a, b| Op::Add { dst, a, b },
        BinOp::Sub => |dst, a, b| Op::Sub { dst, a, b },
        BinOp::Mul => |dst, a, b| Op::Mul { dst, a, b },
        BinOp::Div => |dst, a, b| Op::Div { dst, a, b },
        BinOp::Mod => |dst, a, b| Op::Mod { dst, a, b },
        BinOp::Pow => |dst, a, b| Op::Pow { dst, a, b },
        BinOp::Concat => |dst, a, b| Op::Concat { dst, a, b },
        BinOp::Eq => |dst, a, b| Op::CmpEq { dst, a, b },
        BinOp::Ne => |dst, a, b| Op::CmpNe { dst, a, b },
        BinOp::Identical => |dst, a, b| Op::CmpIdentical { dst, a, b },
        BinOp::NotIdentical => |dst, a, b| Op::CmpNotIdentical { dst, a, b },
        BinOp::Lt => |dst, a, b| Op::CmpLt { dst, a, b },
        BinOp::Le => |dst, a, b| Op::CmpLe { dst, a, b },
        BinOp::Gt => |dst, a, b| Op::CmpGt { dst, a, b },
        BinOp::Ge => |dst, a, b| Op::CmpGe { dst, a, b },
        BinOp::Spaceship => |dst, a, b| Op::Spaceship { dst, a, b },
        BinOp::And
        | BinOp::Or
        | BinOp::Xor
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr
        | BinOp::Coalesce
        | BinOp::Pipe => return None,
    })
}
