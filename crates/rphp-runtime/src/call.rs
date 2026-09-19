//! The call ABI (ADR-016): pending-call records built by `Init*`, arguments
//! staged by `Send*` into the register stack, frames activated by `DoCall`;
//! native invocation over the same window (by-reference positions are
//! dereferenced before the handler runs and written back afterwards); and the
//! native→PHP re-entry (`call_value`, ADR-018).
//!
//! **Resolution** — which function or method a call names, whether it is
//! visible from here, and every callable spelling — lives in `methods.rs`;
//! this module only *runs* what that one resolved ([`Callable`] is the
//! hand-over).

use std::rc::Rc;

use rphp_bytecode::FnFlags;
use rphp_value::{Closure, Object, PhpRef, Value};

use crate::class::{MethodBody, MethodDef};
use crate::frame::{CallTarget, Frame, FrameKind, PendingCall, RetTarget};
use crate::registry::{Ctx, NativeId, NativeResult, Unwind};
use crate::symtab::Symtab;
use crate::unit::FuncRt;
use crate::interp::{MAX_FRAMES, MAX_REENTRY_DEPTH};
use crate::Interp;

/// A resolved callable value.
#[derive(Clone)]
pub enum Callable {
    /// A user function / method / closure with its binding.
    User {
        func: Rc<FuncRt>,
        this: Option<Object>,
        scope: Option<u32>,
        static_class: Option<u32>,
        closure: Option<Closure>,
    },
    /// A native.
    Native(NativeId),
    /// A native method with its receiver.
    NativeMethod {
        method: Rc<MethodDef>,
        this: Option<Object>,
    },
}

impl Interp {
    /// Bind a closure's captures and `$this` into a freshly pushed frame.
    fn bind_closure(&mut self, func: &FuncRt, base: usize, c: &Closure) {
        let caps = c.captures();
        for (i, d) in func.f.captures.iter().enumerate() {
            let Some(v) = caps.get(i) else { break };
            let slot = &mut self.stack[base + d.dst as usize];
            if d.by_ref {
                *slot = v.clone();
            } else {
                *slot = v.deref().into_owned();
            }
        }
    }

    /// Push a user frame whose argument window is `args` (copied to the top
    /// of the register stack). Used by re-entry (`call_value`), thunks and
    /// `run_main`; the bytecode call path stages the window itself and
    /// calls [`Interp::activate`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_user_frame(
        &mut self,
        func: Rc<FuncRt>,
        kind: FrameKind,
        args: &[Value],
        this: Option<Object>,
        scope: Option<u32>,
        static_class: Option<u32>,
        ret: RetTarget,
        symtab: Option<Symtab>,
    ) -> Result<(), Unwind> {
        let args_base = self.stack.len();
        self.push_args(&func, args);
        let pending = PendingCall {
            target: CallTarget::User {
                func: func.clone(),
                closure: None,
            },
            this,
            scope,
            static_class,
            args_base,
            argc: args.len(),
            named: Vec::new(),
            new_obj: None,
            name: func.f.name_bytes.clone(),
        };
        self.activate(pending, kind, ret, symtab, None)
    }

    /// Stage already-evaluated arguments as `func`'s window: a by-reference
    /// parameter shares a passed `Ref` cell, a by-value one gets a copy.
    fn push_args(&mut self, func: &FuncRt, args: &[Value]) {
        for (i, a) in args.iter().enumerate() {
            let by_ref = func.f.params.get(i).is_some_and(|p| p.by_ref);
            if by_ref {
                self.stack.push(a.clone());
            } else {
                self.stack.push(a.deref().into_owned());
            }
        }
    }

    /// Activate the callee of a pending call over its staged window: place
    /// named arguments, check arity, move extra arguments out, size the
    /// register window, bind closure captures and push the frame. The
    /// prologue ops (`RecvInit`, `RecvVariadic`, `BindSymtab`) then run as
    /// part of the callee's code.
    pub(crate) fn activate(
        &mut self,
        pending: PendingCall,
        kind: FrameKind,
        ret: RetTarget,
        symtab: Option<Symtab>,
        closure: Option<Closure>,
    ) -> Result<(), Unwind> {
        let PendingCall {
            target,
            this,
            scope,
            static_class,
            args_base,
            argc,
            named,
            ..
        } = pending;
        let func = match target {
            CallTarget::User { func, .. } => func,
            _ => unreachable!("activate on a non-user target"),
        };
        if self.frames.len() >= MAX_FRAMES {
            return Err(Unwind::error(format!(
                "Maximum function nesting level of '{MAX_FRAMES}' reached, aborting!"
            )));
        }
        let np = func.f.num_params as usize;
        let variadic = func.f.flags.contains(FnFlags::VARIADIC);
        let declared = if variadic { np.saturating_sub(1) } else { np };
        let mut extra_named = Vec::new();
        let mut used_named = false;
        // Named arguments land in their parameter's slot; gaps stay `Uninit`
        // so `RecvInit` fills their defaults.
        for (name, v) in named {
            used_named = true;
            let pos = func
                .f
                .params
                .iter()
                .take(declared)
                .position(|p| p.name.as_ref() == name.as_ref());
            match pos {
                Some(p) => {
                    if p < argc {
                        self.stack.truncate(args_base);
                        return Err(Unwind::error(format!(
                            "Named parameter ${} overwrites previous argument",
                            String::from_utf8_lossy(&name)
                        )));
                    }
                    while self.stack.len() < args_base + p + 1 {
                        self.stack.push(Value::Uninit);
                    }
                    let slot = &mut self.stack[args_base + p];
                    if !slot.is_uninit() {
                        self.stack.truncate(args_base);
                        return Err(Unwind::error(format!(
                            "Named parameter ${} overwrites previous argument",
                            String::from_utf8_lossy(&name)
                        )));
                    }
                    *slot = v;
                }
                None if variadic => extra_named.push((name, v)),
                None => {
                    self.stack.truncate(args_base);
                    return Err(Unwind::error(format!(
                        "Unknown named parameter ${}",
                        String::from_utf8_lossy(&name)
                    )));
                }
            }
        }
        // Arity: every required parameter must have been passed. The check
        // is decided here but raised once the frame exists (php raises it
        // from the callee's RECV, so the callee shows in the trace).
        let required = func.f.required_params();
        let passed_total = self.stack.len() - args_base;
        let (caller_file, caller_line) = self.caller_site();
        let mut arity_error: Option<Unwind> = None;
        for i in 0..required.min(declared) {
            let missing = i >= argc && (i >= passed_total || self.stack[args_base + i].is_uninit());
            if missing {
                let fname = self.callable_display_name(&func);
                arity_error = Some(if used_named {
                    Unwind::argument_count_error(format!(
                        "{fname}(): Argument #{} (${}) not passed",
                        i + 1,
                        String::from_utf8_lossy(&func.f.params[i].name)
                    ))
                } else {
                    let kind = if declared > required || variadic {
                        "at least"
                    } else {
                        "exactly"
                    };
                    Unwind::argument_count_error(format!(
                        "Too few arguments to function {fname}(), {argc} passed in {caller_file} on line {caller_line} and {kind} {required} expected"
                    ))
                });
                break;
            }
        }
        // Extra positional arguments beyond the declared parameters (a
        // by-reference variadic keeps the sent cells).
        let ref_variadic = variadic && func.f.params.last().is_some_and(|p| p.by_ref);
        let extra_args: Vec<Value> = if argc > declared {
            let extras: Vec<Value> = self.stack[args_base + declared..args_base + argc]
                .iter()
                .map(|v| if ref_variadic { v.clone() } else { v.deref().into_owned() })
                .collect();
            self.stack.truncate(args_base + declared);
            extras
        } else {
            Vec::new()
        };
        let num_regs = func.f.num_regs as usize;
        // A register starts *uninitialized*, not null: php's symbol table has
        // no entry for a variable that was never assigned, so
        // `get_defined_vars()` and `$GLOBALS` must not show one either.
        self.stack.resize(args_base + num_regs, Value::Uninit);
        if let Some(c) = &closure {
            self.bind_closure(&func, args_base, c);
        }
        // Parameter types are checked under the *caller's* strict_types; a
        // call from a native (re-entry) is coercive and has no `called in`.
        let (caller_strict, caller_site) = match self.frames.last() {
            Some(f) if f.is_user() => (f.strict, Some((self.frame_file(f), self.frame_line(f)))),
            _ => (false, None),
        };
        let strict = func.f.flags.contains(FnFlags::STRICT_TYPES);
        let frame = Frame {
            kind,
            func: Some(func.clone()),
            base: args_base,
            pc: 0,
            argc: argc.max(if used_named { passed_total } else { 0 }),
            extra_args,
            extra_named,
            this,
            scope,
            static_class,
            ret,
            symtab,
            pending: Vec::new(),
            silence_base: self.silence,
            strict,
            native: None,
            include_kind: None,
            iters: Vec::new(),
            generator: None,
            // A closure body binds *its own* statics (php gives each closure
            // object a fresh set); everything else uses the function's.
            statics: closure
                .as_ref()
                .filter(|_| !func.f.statics.is_empty())
                .map(|c| c.statics(func.f.statics.len())),
        };
        self.frames.push(frame);
        // E8: calling a generator function evaluates its arguments and then
        // parks the body — the caller gets a `Generator`, not a result.
        if func.f.is_generator() && arity_error.is_none() {
            return self.park_new_generator();
        }
        if let Some(u) = arity_error {
            // php raises this from the callee's RECV: the callee shows in the
            // trace and the fault line is the declaration line. Locate it
            // with the callee on top, then drop the frame again so the caller
            // is left exactly as before the call.
            let u = match u {
                Unwind::Pending(mut p) => {
                    let f = self.frames.last().expect("frame");
                    let func = f.func.clone().expect("user frame");
                    p.site = Some(Box::new(crate::FaultSite {
                        file: func.unit.file.to_string(),
                        line: func.f.decl_line,
                        trace: self.exception_trace(0),
                    }));
                    Unwind::Pending(p)
                }
                other => other,
            };
            let f = self.frames.pop().expect("frame");
            self.stack.truncate(f.base);
            return Err(u);
        }
        let has_types = func.f.params.iter().any(|p| p.ty.is_some());
        if has_types {
            let argc = self.frames.last().expect("frame").argc;
            if let Err(u) = self.verify_params(&func, args_base, argc, caller_strict, caller_site) {
                // Raised from the callee's RECV like the arity error.
                let u = match u {
                    Unwind::Pending(mut p) => {
                        p.site = Some(Box::new(crate::FaultSite {
                            file: func.unit.file.to_string(),
                            line: func.f.decl_line,
                            trace: self.exception_trace(0),
                        }));
                        Unwind::Pending(p)
                    }
                    other => other,
                };
                let f = self.frames.pop().expect("frame");
                self.stack.truncate(f.base);
                return Err(u);
            }
        }
        Ok(())
    }

    /// The file and line of the innermost user frame (the caller of a call
    /// being set up), for `Too few arguments … passed in %s on line %d`.
    pub(crate) fn caller_site(&self) -> (String, u32) {
        match self.current_user_frame() {
            Some(f) => (self.frame_file(f), self.frame_line(f)),
            None => (self.script_name.clone(), 0),
        }
    }

    /// `f` / `A::m` for messages.
    pub(crate) fn callable_display_name(&self, func: &FuncRt) -> String {
        let name = String::from_utf8_lossy(&func.f.name_bytes).into_owned();
        match func.class {
            Some(cid) if !func.f.flags.contains(FnFlags::CLOSURE) => {
                format!("{}::{name}", String::from_utf8_lossy(&self.classes[cid as usize].name))
            }
            _ => name,
        }
    }

    // ---- natives ---------------------------------------------------------

    /// Invoke a native over a staged window `[args_base, args_base+argc)`
    /// plus named arguments: by-reference positions holding a `Ref` are
    /// dereferenced for the handler and written back through the cell
    /// afterwards. The window is consumed.
    pub(crate) fn call_native_window(
        &mut self,
        id: NativeId,
        args_base: usize,
        argc: usize,
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        let f = self.natives[id.0 as usize];
        let mut args: Vec<Value> = self.stack.drain(args_base..args_base + argc).collect();
        for (name, v) in named {
            match f.params.iter().position(|p| p.as_bytes() == name.as_ref()) {
                Some(p) => {
                    if p < args.len() {
                        if !args[p].is_uninit() {
                            return Err(Unwind::error(format!(
                                "Named parameter ${} overwrites previous argument",
                                String::from_utf8_lossy(&name)
                            )));
                        }
                        args[p] = v;
                    } else {
                        while args.len() < p {
                            args.push(Value::Uninit);
                        }
                        args.push(v);
                    }
                }
                None => {
                    return Err(Unwind::error(format!(
                        "Unknown named parameter ${}",
                        String::from_utf8_lossy(&name)
                    )))
                }
            }
        }
        for a in &mut args {
            if a.is_uninit() {
                *a = Value::Null;
            }
        }
        self.call_native(id, &mut args)
    }

    /// Invoke a registered native with already-evaluated arguments: arity is
    /// checked (`ArgumentCountError`), a native frame is pushed for traces,
    /// by-reference positions that hold a `Ref` cell are dereferenced for
    /// the handler and written back through the cell afterwards (the
    /// copy-back shim, so handler bodies never see `Ref`s), staged output
    /// is flushed. `args` is `&mut` so a by-reference native's writes are
    /// visible to the caller when no cell was passed.
    pub fn call_native(&mut self, id: NativeId, args: &mut [Value]) -> NativeResult {
        let f = self.natives[id.0 as usize];
        if !f.accepts(args.len()) {
            return Err(Unwind::argument_count_error(f.arity_message(args.len())));
        }
        let mut cells: Vec<(usize, PhpRef)> = Vec::new();
        for (i, a) in args.iter_mut().enumerate() {
            if let Value::Ref(r) = a {
                if f.is_by_ref(i) {
                    cells.push((i, r.clone()));
                }
                *a = r.get();
            }
        }
        let silence = self.silence;
        self.frames.push(Frame::native(id, args.to_vec(), silence));
        let r = {
            let mut ctx = Ctx(self);
            (f.handler)(&mut ctx, args)
        };
        let r = self.locate_fault(r);
        self.frames.pop();
        self.silence = silence;
        for (i, cell) in cells {
            cell.set(args[i].clone());
        }
        self.out.flush_pending();
        r
    }

    /// Invoke a native method over a staged window (`DoCall` on a
    /// `CallTarget::NativeMethod`): named arguments are placed by the
    /// method's parameter names, then [`Interp::call_native_method`] runs.
    ///
    /// Two engine trampolines (`methods.rs`) share this entry point and are
    /// taken off it before the arity/named-argument machinery, because they
    /// take the whole window: a `__call` / `__callStatic` trampoline, whose
    /// arguments (named ones included, as string keys) become the magic
    /// method's `$args` array, and a `Closure` instance-method trampoline,
    /// whose argument 0 is the closure receiver.
    pub(crate) fn call_native_method_window(
        &mut self,
        m: Rc<MethodDef>,
        this: Option<Object>,
        args_base: usize,
        argc: usize,
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        if Interp::is_magic_trampoline(&m) {
            let args: Vec<Value> = self.stack.drain(args_base..args_base + argc).collect();
            return self.run_call_trampoline(m, this, args, named);
        }
        if Interp::is_closure_method(&m) {
            let args: Vec<Value> = self.stack.drain(args_base..args_base + argc).collect();
            if !named.is_empty() {
                return Err(Unwind::error(format!(
                    "Unknown named parameter ${}",
                    String::from_utf8_lossy(&named[0].0)
                )));
            }
            let lname = m.name.clone();
            return self.run_closure_method(&lname, &args);
        }
        let MethodBody::Native(nm) = &m.body else {
            unreachable!("call_native_method_window on a user method")
        };
        let mut args: Vec<Value> = self.stack.drain(args_base..args_base + argc).collect();
        for (name, v) in named {
            match nm.params.iter().position(|p| p.as_bytes() == name.as_ref()) {
                Some(p) => {
                    if p < args.len() {
                        if !args[p].is_uninit() {
                            return Err(Unwind::error(format!(
                                "Named parameter ${} overwrites previous argument",
                                String::from_utf8_lossy(&name)
                            )));
                        }
                        args[p] = v;
                    } else {
                        while args.len() < p {
                            args.push(Value::Uninit);
                        }
                        args.push(v);
                    }
                }
                None => {
                    return Err(Unwind::error(format!(
                        "Unknown named parameter ${}",
                        String::from_utf8_lossy(&name)
                    )))
                }
            }
        }
        for a in &mut args {
            if a.is_uninit() {
                *a = Value::Null;
            }
        }
        self.call_native_method(m, this, &mut args)
    }

    /// Invoke a native method with already-evaluated arguments: arity check,
    /// a native frame (`Class->method` in traces), by-reference write-back,
    /// output flush — the method counterpart of [`Interp::call_native`].
    pub fn call_native_method(&mut self, m: Rc<MethodDef>, this: Option<Object>, args: &mut [Value]) -> NativeResult {
        // The engine trampolines take the whole window; see
        // [`Interp::call_native_method_window`].
        if Interp::is_magic_trampoline(&m) {
            let args = args.to_vec();
            return self.run_call_trampoline(m, this, args, Vec::new());
        }
        if Interp::is_closure_method(&m) {
            let lname = m.name.clone();
            return self.run_closure_method(&lname, args);
        }
        let MethodBody::Native(nm) = &m.body else {
            return Err(Unwind::error("internal error: call_native_method on a user method"));
        };
        let nm = *nm;
        if !nm.accepts(args.len()) {
            let display = format!(
                "{}::{}",
                self.classes[m.decl as usize].name_str(),
                String::from_utf8_lossy(&m.name)
            );
            return Err(Unwind::argument_count_error(nm.arity_message(&display, args.len())));
        }
        let mut cells: Vec<(usize, PhpRef)> = Vec::new();
        for (i, a) in args.iter_mut().enumerate() {
            if let Value::Ref(r) = a {
                if nm.is_by_ref(i) {
                    cells.push((i, r.clone()));
                }
                *a = r.get();
            }
        }
        let silence = self.silence;
        self.frames.push(Frame::native_method(m, this.clone(), args.to_vec(), silence));
        let r = {
            let mut ctx = Ctx(self);
            (nm.handler)(&mut ctx, this.as_ref(), args)
        };
        let r = self.locate_fault(r);
        self.frames.pop();
        self.silence = silence;
        for (i, cell) in cells {
            cell.set(args[i].clone());
        }
        self.out.flush_pending();
        r
    }

    /// Call method `name` on `obj` from native code (no visibility check:
    /// the engine's own dispatch for `__toString`, `__destruct`, …). The
    /// method must exist (`Error: Call to undefined method` otherwise).
    pub fn call_method(&mut self, obj: &Object, name: &[u8], args: &[Value]) -> NativeResult {
        let Some(m) = self.resolve_method(obj.class_id(), name) else {
            return Err(Unwind::error(format!(
                "Call to undefined method {}::{}()",
                self.class_name_of(obj),
                String::from_utf8_lossy(name)
            )));
        };
        match &m.body {
            MethodBody::Native(_) => {
                let mut args = args.to_vec();
                self.call_native_method(m, Some(obj.clone()), &mut args)
            }
            MethodBody::User(func) => {
                let func = func.clone();
                self.call_user_func(func, Some(obj.clone()), Some(m.decl), Some(obj.class_id()), None, args)
            }
        }
    }

    /// Call static method `name` of `class` from native code.
    pub fn call_static_method(&mut self, class: u32, name: &[u8], args: &[Value]) -> NativeResult {
        let Some(m) = self.resolve_method(class, name) else {
            return Err(Unwind::error(format!(
                "Call to undefined method {}::{}()",
                self.classes[class as usize].name_str(),
                String::from_utf8_lossy(name)
            )));
        };
        match &m.body {
            MethodBody::Native(_) => {
                let mut args = args.to_vec();
                self.call_native_method(m, None, &mut args)
            }
            MethodBody::User(func) => {
                let func = func.clone();
                self.call_user_func(func, None, Some(m.decl), Some(class), None, args)
            }
        }
    }

    // ---- re-entry ---------------------------------------------------------

    /// Invoke a callable value with `args` from native code: pushes a
    /// re-entry boundary frame and runs the VM to it. The single re-entry
    /// path for higher-order natives (`array_map`, `usort`, `ob_start`
    /// handlers, `set_error_handler` callbacks, shutdown functions).
    pub fn call_value(&mut self, callee: &Value, args: &[Value]) -> NativeResult {
        let callable = self.resolve_callable(callee)?;
        self.call_resolved(callable, args)
    }

    /// Invoke an already-resolved callable.
    pub fn call_resolved(&mut self, callable: Callable, args: &[Value]) -> NativeResult {
        self.call_resolved_named(callable, args, Vec::new())
    }

    /// Invoke an already-resolved callable with positional and named
    /// arguments (`call_user_func_array` with string keys).
    pub fn call_resolved_named(
        &mut self,
        callable: Callable,
        args: &[Value],
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        match callable {
            Callable::Native(id) => {
                let args_base = self.stack.len();
                for a in args {
                    self.stack.push(a.clone());
                }
                self.call_native_window(id, args_base, args.len(), named)
            }
            Callable::NativeMethod { method, this } => {
                let args_base = self.stack.len();
                for a in args {
                    self.stack.push(a.clone());
                }
                self.call_native_method_window(method, this, args_base, args.len(), named)
            }
            Callable::User {
                func,
                this,
                scope,
                static_class,
                closure,
            } => self.call_user_func_named(func, this, scope, static_class, closure, args, named),
        }
    }

    /// Run user function `func` to completion from native code (a boundary
    /// frame; the only Rust recursion into the dispatch loop).
    pub(crate) fn call_user_func(
        &mut self,
        func: Rc<FuncRt>,
        this: Option<Object>,
        scope: Option<u32>,
        static_class: Option<u32>,
        closure: Option<Closure>,
        args: &[Value],
    ) -> NativeResult {
        self.call_user_func_named(func, this, scope, static_class, closure, args, Vec::new())
    }

    /// [`Interp::call_user_func`] with named arguments.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call_user_func_named(
        &mut self,
        func: Rc<FuncRt>,
        this: Option<Object>,
        scope: Option<u32>,
        static_class: Option<u32>,
        closure: Option<Closure>,
        args: &[Value],
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        if self.reentry_depth >= MAX_REENTRY_DEPTH {
            return Err(Unwind::error(format!(
                "Maximum function nesting level of '{MAX_REENTRY_DEPTH}' reached, aborting!"
            )));
        }
        let depth = self.frames.len();
        let args_base = self.stack.len();
        self.push_args(&func, args);
        let pending = PendingCall {
            target: CallTarget::User {
                func: func.clone(),
                closure: None,
            },
            this,
            scope,
            static_class,
            args_base,
            argc: args.len(),
            named,
            new_obj: None,
            name: func.f.name_bytes.clone(),
        };
        let symtab = if func.f.flags.contains(FnFlags::NEEDS_SYMTAB) {
            Some(Symtab::new())
        } else {
            None
        };
        if let Err(u) = self.activate(pending, FrameKind::ReentryBoundary, RetTarget::Discard, symtab, closure) {
            let u = self.locate_fault(Err::<(), _>(u)).unwrap_err();
            return Err(u);
        }
        self.reentry_depth += 1;
        let r = self.run_until(depth);
        self.reentry_depth -= 1;
        self.out.flush_pending();
        r
    }

    /// Call a function by name (user or native).
    pub fn call_function(&mut self, name: &[u8], args: &[Value]) -> NativeResult {
        self.call_value(&Value::string(name), args)
    }

    /// Run a zero-argument thunk (a default-value / static initializer) in
    /// the given class scope and return its value.
    pub(crate) fn run_thunk(&mut self, fid: u32, this: Option<Object>, scope: Option<u32>) -> NativeResult {
        let func = self.funcs[fid as usize].clone();
        self.call_user_func(func, this, scope, scope, None, &[])
    }
}
