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

use crate::native_args::NamedFault;
use crate::class::{MethodBody, MethodDef};
use crate::frame::{FrameExtra, CallTarget, Frame, FrameKind, PendingCall, RetTarget};
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

/// What [`Interp::coerce_object_params`] decides for one object argument.
enum ObjectFit {
    /// The parameter takes the object as it is.
    Accept,
    /// A `string` parameter: the object's `__toString()` stands in.
    ToString(Object),
    /// php's `TypeError`.
    Refuse,
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
        let mut arity_error: Option<Unwind> = None;
        for i in 0..required.min(declared) {
            let missing = i >= argc && (i >= passed_total || self.stack[args_base + i].is_uninit());
            if missing {
                let fname = self.callable_display_name(&func);
                let (caller_file, caller_line) = self.caller_site();
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
        // php 8.4 `#[\Deprecated]`: the notice on every call, raised at
        // the call site before the callee runs (the attribute is read once).
        if func.deprecated.borrow().is_none() {
            let note = crate::deprecation::deprecation_note(self, &func.f.attrs, &func.f.consts, &func.unit);
            *func.deprecated.borrow_mut() = Some(note);
        }
        let note = func.deprecated.borrow().as_ref().and_then(|n| n.clone());
        if let Some(note) = note {
            let what = match func.class {
                Some(_) if !func.f.flags.contains(FnFlags::CLOSURE) => "Method",
                _ => "Function",
            };
            let msg = format!("{what} {}() is deprecated{note}", self.callable_display_name(&func));
            if let Err(u) = self.deprecated(&msg) {
                self.stack.truncate(args_base);
                return Err(u);
            }
        }
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
            Some(f) if f.is_user() => (f.strict, Some(self.frames.len() - 1)),
            _ => (false, None),
        };
        let strict = func.f.flags.contains(FnFlags::STRICT_TYPES);
        // A closure body binds *its own* statics (php gives each closure
        // object a fresh set); everything else uses the function's.
        let statics = closure
            .as_ref()
            .filter(|_| !func.f.statics.is_empty())
            .map(|c| c.statics(func.f.statics.len()));
        let extra = if extra_args.is_empty() && extra_named.is_empty() && symtab.is_none() && statics.is_none() {
            None
        } else {
            Some(Box::new(FrameExtra {
                extra_args,
                extra_named,
                symtab,
                statics,
                ..Default::default()
            }))
        };
        let frame = Frame {
            kind,
            func: Some(func.clone()),
            base: args_base,
            pc: 0,
            argc: argc.max(if used_named { passed_total } else { 0 }),
            this,
            scope,
            static_class,
            ret,
            pending: Vec::new(),
            silence_base: self.silence,
            strict,
            native: None,
            extra,
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
        let mut args = self.take_vec();
        args.extend(self.stack.drain(args_base..args_base + argc));
        let r = match self.bind_native_named(f.name, &mut args, named) {
            Ok(extra) => self.call_native_named(id, &mut args, extra),
            Err(NamedFault::Outside(u)) => Err(u),
            Err(NamedFault::Inside(u, extra)) => {
                let silence = self.silence;
                let mut frame = Frame::native(id, uninit_as_null(&args), silence);
                frame.extra_mut().extra_named = extra;
                self.frames.push(frame);
                let r = self.locate_fault(Err(u));
                self.frames.pop();
                r
            }
        };
        self.give_vec(args);
        r
    }

    /// A cleared argument vector from the pool (or a fresh one).
    #[inline]
    pub(crate) fn take_vec(&mut self) -> Vec<Value> {
        self.vec_pool.pop().unwrap_or_default()
    }

    /// Hand an argument vector back to the pool, cleared.
    #[inline]
    pub(crate) fn give_vec(&mut self, mut v: Vec<Value>) {
        v.clear();
        if self.vec_pool.len() < 64 {
            self.vec_pool.push(v);
        }
    }

    /// Pop a native frame, returning its argument vector to the pool.
    #[inline]
    fn pop_native_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            if let Some((_, args)) = frame.native {
                self.give_vec(args);
            }
        }
    }

    /// Invoke a registered native with already-evaluated arguments: arity is
    /// checked (`ArgumentCountError`), a native frame is pushed for traces,
    /// by-reference positions that hold a `Ref` cell are dereferenced for
    /// the handler and written back through the cell afterwards (the
    /// copy-back shim, so handler bodies never see `Ref`s), staged output
    /// is flushed. `args` is `&mut` so a by-reference native's writes are
    /// visible to the caller when no cell was passed.
    pub fn call_native(&mut self, id: NativeId, args: &mut [Value]) -> NativeResult {
        self.call_native_named(id, args, Vec::new())
    }

    /// [`Interp::call_native`] with the unknown named arguments a
    /// pass-through native forwards (`call_user_func($f, x: 1)`), kept on
    /// its frame for [`Interp::take_extra_named`] and the trace.
    pub(crate) fn call_native_named(
        &mut self,
        id: NativeId,
        args: &mut [Value],
        extra_named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        let f = self.natives[id.0 as usize];
        if !f.accepts(args.len()) {
            return Err(Unwind::argument_count_error(f.arity_message(args.len())));
        }
        self.coerce_object_params(f.name, args)?;
        let cells = Interp::unwrap_native_args(f.name, args, |i| f.is_by_ref(i));
        let silence = self.silence;
        let copy = self.frame_args_for(args, &cells);
        let mut frame = Frame::native(id, copy, silence);
        if !cells.is_empty() {
            frame.extra_mut().ref_cells = cells.clone();
        }
        if !extra_named.is_empty() {
            frame.extra_mut().extra_named = extra_named;
        }
        self.frames.push(frame);
        let r = {
            let mut ctx = Ctx(self);
            (f.handler)(&mut ctx, args)
        };
        // The cells get their arrays back before the fault site is
        // captured, so a trace shows `Array`, not what was taken out.
        Interp::write_back_native_args(args, cells);
        let r = self.locate_fault(r);
        self.pop_native_frame();
        self.silence = silence;
        self.out.flush_pending();
        r
    }

    /// Unwrap the `Ref` cells among a native's arguments: a by-reference
    /// position keeps its cell (for the write-back) and, when the cell
    /// holds an array and the native takes no callback, **takes** the
    /// array out of it for the call — the handler then owns the only
    /// handle and mutates in place, where a clone would have made
    /// `array_pop($a)` copy `$a` every time. A native with a callback
    /// (`usort`, `array_walk`) works on a copy instead, so its callback
    /// sees the variable as php's does; a cell two positions share is
    /// only read, so both see the same value.
    fn unwrap_native_args(name: &str, args: &mut [Value], by_ref: impl Fn(usize) -> bool) -> Vec<(usize, PhpRef)> {
        let mut cells: Vec<(usize, PhpRef)> = Vec::new();
        let shared = |i: usize, r: &PhpRef, args: &[Value]| {
            args.iter().enumerate().any(|(j, a)| j != i && matches!(a, Value::Ref(o) if o.ptr_eq(r)))
        };
        let mut takes_callback: Option<bool> = None;
        for i in 0..args.len() {
            let Value::Ref(r) = &args[i] else { continue };
            let r = r.clone();
            let taken = if by_ref(i) {
                cells.push((i, r.clone()));
                let callback = *takes_callback.get_or_insert_with(|| {
                    crate::native_args::native_arginfo(name).is_some_and(|row| {
                        row.iter().any(|(_, ty, _)| ty.is_some_and(|t| t.contains("callable")))
                    })
                });
                let can_take = !callback && !shared(i, &r, args) && matches!(&*r.borrow(), Value::Array(_));
                if can_take {
                    r.update(|v| std::mem::replace(v, Value::Null))
                } else {
                    r.get()
                }
            } else {
                r.get()
            };
            args[i] = taken;
        }
        cells
    }

    /// The frame's copy of a native's arguments (traces, `func_get_args`
    /// through a callback): a by-reference position holds its cell, so the
    /// trace shows the variable as it is — and no second handle on the
    /// array the handler is working on.
    fn frame_args_for(&mut self, args: &[Value], cells: &[(usize, PhpRef)]) -> Vec<Value> {
        let mut v = self.take_vec();
        for (i, a) in args.iter().enumerate() {
            match cells.iter().find(|(p, _)| *p == i) {
                Some((_, cell)) => v.push(Value::Ref(cell.clone())),
                None => v.push(a.clone()),
            }
        }
        v
    }

    /// Write a native's by-reference results back into their cells.
    fn write_back_native_args(args: &mut [Value], cells: Vec<(usize, PhpRef)>) {
        for (i, cell) in cells {
            cell.set(std::mem::replace(&mut args[i], Value::Null));
        }
    }

    /// php's argument parser on an object argument: a parameter declared
    /// `string` (a union with it included) takes `__toString()` —
    /// `sprintf('%s', $alias)`, `strlen($stringable)` — a class or
    /// interface part takes an instance, `object`/`mixed`/`callable` take
    /// any object, `iterable` a Traversable, and every other declared type
    /// (`int`, `array`, `?bool`, …) refuses the object with php's
    /// `TypeError`. A closure is an object of class `Closure` here. This
    /// runs before the handler, so a handler's own conversions never meet
    /// an object they cannot take; a native the manifest does not describe
    /// is left to its handler.
    ///
    /// `key` is the native's name, or `class::method` for a method; the
    /// lookup only happens when an argument is an object at all.
    pub(crate) fn coerce_object_params(&mut self, key: &str, args: &mut [Value]) -> Result<(), Unwind> {
        if !args.iter().any(|a| matches!(&*a.deref(), Value::Object(_) | Value::Closure(_))) {
            return Ok(());
        }
        let lower = key.to_ascii_lowercase();
        // A handler that parses its object itself, more loosely than its
        // stub declares (php takes any object here and throws its own
        // `InvalidArgumentException` for the wrong kind).
        if matches!(
            lower.as_str(),
            "recursiveiteratoriterator::__construct" | "recursivetreeiterator::__construct"
        ) {
            return Ok(());
        }
        let Some(row) = crate::native_args::params_of(&lower) else {
            return Ok(());
        };
        for (i, a) in args.iter_mut().enumerate() {
            let v = a.deref().into_owned();
            if !matches!(v, Value::Object(_) | Value::Closure(_)) {
                continue;
            }
            // The parameter as declared; a variadic one covers the rest,
            // and php names such a position by number alone.
            let param = row.get(i).or_else(|| row.last().filter(|p| p.2 == Some("...")));
            let Some((name, Some(ty), default)) = param else {
                continue;
            };
            match self.object_param_fit(&v, ty) {
                ObjectFit::Accept => {}
                ObjectFit::ToString(o) => *a = Value::Str(self.object_to_string(&o)?),
                ObjectFit::Refuse => {
                    let name = if *default == Some("...") {
                        String::new()
                    } else {
                        format!(" (${name})")
                    };
                    return Err(Unwind::type_error(format!(
                        "{key}(): Argument #{}{name} must be of type {ty}, {} given",
                        i + 1,
                        crate::ops::value_name(&v)
                    )));
                }
            }
        }
        Ok(())
    }

    /// How an object argument meets a declared parameter type (see
    /// [`Interp::coerce_object_params`]).
    fn object_param_fit(&self, v: &Value, ty: &str) -> ObjectFit {
        let mut string = false;
        for part in ty.split('|') {
            let part = part.trim().trim_start_matches('?');
            match part {
                "mixed" | "object" | "callable" | "self" | "static" => return ObjectFit::Accept,
                "string" => string = true,
                "iterable" => {
                    if let Value::Object(o) = v {
                        if self.is_traversable(o) {
                            return ObjectFit::Accept;
                        }
                    }
                }
                p if p.starts_with(|c: char| c.is_ascii_uppercase()) || p.contains('\\') => {
                    let cid = self.class_of_value(v);
                    let target = self.class_by_name(p.trim_start_matches('\\').as_bytes());
                    if let (Some(cid), Some(target)) = (cid, target) {
                        if self.instanceof_class(cid, target) {
                            return ObjectFit::Accept;
                        }
                    }
                }
                _ => {}
            }
        }
        if string {
            if let Value::Object(o) = v {
                if self.has_to_string(o) {
                    return ObjectFit::ToString(o.clone());
                }
            }
        }
        ObjectFit::Refuse
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
            let mut args: Vec<Value> = self.stack.drain(args_base..args_base + argc).collect();
            let display = format!("Closure::{}", String::from_utf8_lossy(&m.name));
            let lname = m.name.clone();
            // `bindTo` binds its own names; `call` forwards the unknown ones.
            return match self.bind_closure_method_named(&display, &mut args, named) {
                Ok(extra) => self.run_closure_method(&lname, &args, extra),
                Err(NamedFault::Outside(u)) => Err(u),
                Err(NamedFault::Inside(u, extra)) => {
                    let silence = self.silence;
                    let mut frame = Frame::native_method(m, this, uninit_as_null(&args), silence);
                    frame.extra_mut().extra_named = extra;
                    self.frames.push(frame);
                    let r = self.locate_fault(Err(u));
                    self.frames.pop();
                    r
                }
            };
        }
        let MethodBody::Native(_) = &m.body else {
            unreachable!("call_native_method_window on a user method")
        };
        let mut args = self.take_vec();
        args.extend(self.stack.drain(args_base..args_base + argc));
        if named.is_empty() {
            let r = self.call_native_method(m, this, &mut args);
            self.give_vec(args);
            return r;
        }
        let display = format!(
            "{}::{}",
            self.classes[m.decl as usize].name_str(),
            String::from_utf8_lossy(&m.name)
        );
        match self.bind_native_named(&display, &mut args, named) {
            Ok(extra) => self.call_native_method_named(m, this, &mut args, extra),
            Err(NamedFault::Outside(u)) => Err(u),
            Err(NamedFault::Inside(u, extra)) => {
                let silence = self.silence;
                let mut frame = Frame::native_method(m, this, uninit_as_null(&args), silence);
                frame.extra_mut().extra_named = extra;
                self.frames.push(frame);
                let r = self.locate_fault(Err(u));
                self.frames.pop();
                r
            }
        }
    }

    /// Invoke a native method with already-evaluated arguments: arity check,
    /// a native frame (`Class->method` in traces), by-reference write-back,
    /// output flush — the method counterpart of [`Interp::call_native`].
    pub fn call_native_method(&mut self, m: Rc<MethodDef>, this: Option<Object>, args: &mut [Value]) -> NativeResult {
        self.call_native_method_named(m, this, args, Vec::new())
    }

    /// [`Interp::call_native_method`] with the unknown named arguments a
    /// pass-through method forwards (`$closure->call($o, x: 1)`).
    pub(crate) fn call_native_method_named(
        &mut self,
        m: Rc<MethodDef>,
        this: Option<Object>,
        args: &mut [Value],
        extra_named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        // The engine trampolines take the whole window; see
        // [`Interp::call_native_method_window`].
        if Interp::is_magic_trampoline(&m) {
            let args = args.to_vec();
            return self.run_call_trampoline(m, this, args, Vec::new());
        }
        if Interp::is_closure_method(&m) {
            let lname = m.name.clone();
            return self.run_closure_method(&lname, args, extra_named);
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
        if args.iter().any(|a| matches!(&*a.deref(), Value::Object(_) | Value::Closure(_))) {
            let key = format!(
                "{}::{}",
                self.classes[m.decl as usize].name_str(),
                String::from_utf8_lossy(&m.name)
            );
            self.coerce_object_params(&key, args)?;
        }
        // (The name is only needed when a reference is passed.)
        let key = if args.iter().any(|a| matches!(a, Value::Ref(_))) {
            format!("{}::{}", self.classes[m.decl as usize].name_str(), String::from_utf8_lossy(&m.name))
        } else {
            String::new()
        };
        let cells = Interp::unwrap_native_args(&key, args, |i| nm.is_by_ref(i));
        let silence = self.silence;
        let copy = self.frame_args_for(args, &cells);
        let mut frame = Frame::native_method(m, this.clone(), copy, silence);
        if !cells.is_empty() {
            frame.extra_mut().ref_cells = cells.clone();
        }
        if !extra_named.is_empty() {
            frame.extra_mut().extra_named = extra_named;
        }
        self.frames.push(frame);
        let r = {
            let mut ctx = Ctx(self);
            (nm.handler)(&mut ctx, this.as_ref(), args)
        };
        // The cells get their arrays back before the fault site is
        // captured, so a trace shows `Array`, not what was taken out.
        Interp::write_back_native_args(args, cells);
        let r = self.locate_fault(r);
        self.pop_native_frame();
        self.silence = silence;
        self.out.flush_pending();
        r
    }

    /// Call method `name` on `obj` from native code (no visibility check:
    /// the engine's own dispatch for `__toString`, `__destruct`, …). The
    /// method must exist (`Error: Call to undefined method` otherwise).
    pub fn call_method(&mut self, obj: &Object, name: &[u8], args: &[Value]) -> NativeResult {
        self.call_method_raw(obj, name, args).map(Value::unref)
    }

    /// [`Interp::call_method`] keeping a by-reference return (`function
    /// &offsetGet()`) as the cell it returned, for the one caller that binds
    /// to it (a nested write through `ArrayAccess`, `exec.rs`).
    pub(crate) fn call_method_raw(&mut self, obj: &Object, name: &[u8], args: &[Value]) -> NativeResult {
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
        self.autoload_callable(callee)?;
        let callable = self.resolve_callable(callee)?;
        self.call_resolved(callable, args)
    }

    /// [`Interp::call_value`] with named arguments after the positional
    /// ones (a pass-through native forwarding what it was called with).
    pub fn call_value_named(
        &mut self,
        callee: &Value,
        args: &[Value],
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        self.autoload_callable(callee)?;
        let callable = self.resolve_callable(callee)?;
        self.call_resolved_named(callable, args, named)
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
            } => self
                .call_user_func_named(func, this, scope, static_class, closure, args, named)
                // A native sees the value a by-reference return refers to,
                // never the cell (php derefs a by-value use of `&f()`).
                .map(Value::unref),
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

/// A call window as a trace shows it: the positions a call skipped are
/// `NULL`.
fn uninit_as_null(args: &[Value]) -> Vec<Value> {
    args.iter()
        .map(|a| if a.is_uninit() { Value::Null } else { a.clone() })
        .collect()
}
