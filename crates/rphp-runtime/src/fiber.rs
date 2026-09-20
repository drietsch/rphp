//! Fibers (php 8.1): `Fiber`, `FiberError`.
//!
//! php switches C stacks for a fiber, so `Fiber::suspend()` can happen
//! anywhere. This engine has no such switch, but its frames are explicit
//! (ADR-016) and its registers live on one contiguous stack, so a fiber is
//! a *parked call chain*: `Fiber::start()` runs the callable through the
//! ordinary re-entry path, and a `Fiber::suspend()` reached from bytecode
//! moves every frame above the fiber's boundary — with its register
//! windows, pending calls and `foreach` iterators — into the
//! [`FiberState`], and hands the transfer value back to whoever started or
//! resumed it. `resume()` puts the chain back at the current stack top
//! (rebasing the absolute indices) and continues the dispatch loop after
//! the suspending call; `throw()` does the same and raises at that point.
//!
//! The one thing the parked chain cannot hold is a Rust stack: a suspend
//! reached *through a native* — from a callback inside `array_map()`, a
//! generator body driven by `Generator::send()`, `__toString()` — has that
//! native's Rust frame between the boundary and the suspension point, and
//! is refused with a `FiberError` (cataloged in `COVERAGE.md`). The
//! generator's parking (`generator.rs`) is the single-frame version of the
//! same idea.

use rphp_value::{Object, Value};

use crate::frame::{Frame, FrameKind, RetTarget};
use crate::registry::{Ctx, NativeResult, Registry, Unwind};
use crate::{nm, ClassFlags, Interp, NativeMethod};

/// Where a fiber is in its life cycle (php's `zend_fiber_status`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FiberStatus {
    Init,
    Running,
    Suspended,
    Terminated,
}

/// A fiber: its callable and, while suspended, its parked call chain.
pub struct FiberState {
    pub status: FiberStatus,
    callable: Value,
    /// The `Fiber` object, for `Fiber::getCurrent()`.
    object: Object,
    /// The parked frames, innermost last; empty unless suspended.
    frames: Vec<Frame>,
    /// The parked register stack from the first frame's base to the top.
    regs: Vec<Value>,
    /// The register (relative to the innermost frame) the suspending call
    /// writes the resumed value into.
    resume_dst: u16,
    /// The value `Fiber::suspend()` handed out.
    transfer: Value,
    /// The callable's return value.
    return_val: Value,
    /// The fiber ended with an exception.
    threw: bool,
    /// While running: the frame index the chain starts at, and the
    /// re-entry depth it runs at (a suspend from deeper is through a native).
    boundary: usize,
    reentry_depth: usize,
}

impl Interp {
    /// The fiber index a `Fiber` object carries.
    fn fiber_id(&self, o: &Object) -> Result<u32, Unwind> {
        o.with_payload(|id: &mut u32| *id)
            .ok_or_else(|| Unwind::error("Fiber object is not initialized"))
    }

    /// `Fiber::getCurrent()`: the innermost running fiber's object.
    pub fn current_fiber(&self) -> Option<Object> {
        self.fiber_stack
            .last()
            .map(|&id| self.fibers[id as usize].object.clone())
    }

    /// `Fiber::suspend($value)`, called from the native: validate, mark the
    /// fiber suspending and let the dispatch loop park the chain (it knows
    /// the destination register; see [`Interp::park_fiber`]).
    fn fiber_suspend(&mut self, value: Value) -> Result<(), Unwind> {
        let Some(&id) = self.fiber_stack.last() else {
            return Err(fiber_error("Cannot suspend outside of a fiber"));
        };
        let f = &self.fibers[id as usize];
        // The native frame of `Fiber::suspend` itself is on top; everything
        // between it and the boundary must be bytecode driven by the loop
        // that `start()`/`resume()` entered.
        let through_native = self.reentry_depth != f.reentry_depth
            || self.frames[f.boundary..self.frames.len() - 1]
                .iter()
                .skip(1)
                .any(|fr| !matches!(fr.kind, FrameKind::Normal | FrameKind::Include));
        if through_native {
            return Err(fiber_error(
                "Cannot suspend a fiber from within a native call (rphp: a suspend through a native frame is not supported)",
            ));
        }
        self.fibers[id as usize].transfer = value;
        self.fiber_suspending = Some(id);
        Ok(())
    }

    /// Park the running fiber's chain after the `Fiber::suspend()` call at
    /// the top frame's pc returned; `dst` is that call's result register.
    pub(crate) fn park_fiber(&mut self, id: u32, dst: u16) {
        let boundary = self.fibers[id as usize].boundary;
        let frames: Vec<Frame> = self.frames.drain(boundary..).collect();
        let first_base = frames[0].base;
        let regs = self.stack.split_off(first_base);
        let f = &mut self.fibers[id as usize];
        f.frames = frames;
        f.regs = regs;
        f.resume_dst = dst;
        f.status = FiberStatus::Suspended;
        self.fiber_stack.pop();
    }

    /// Put a suspended fiber's chain back on the stacks, at the current
    /// top, and run it until it suspends again or ends. `with` is what the
    /// suspending call receives: a value, or an exception to raise there.
    fn fiber_continue(&mut self, id: u32, with: Result<Value, Object>) -> NativeResult {
        let boundary = self.frames.len();
        let (frames, regs, dst) = {
            let f = &mut self.fibers[id as usize];
            (std::mem::take(&mut f.frames), std::mem::take(&mut f.regs), f.resume_dst)
        };
        let old_base = frames[0].base;
        let new_base = self.stack.len();
        self.stack.extend(regs);
        for mut fr in frames {
            fr.base = fr.base - old_base + new_base;
            for p in &mut fr.pending {
                p.args_base = p.args_base - old_base + new_base;
            }
            match &mut fr.ret {
                RetTarget::Reg(r) => *r = *r - old_base + new_base,
                RetTarget::New { reg, .. } => *reg = *reg - old_base + new_base,
                RetTarget::Discard => {}
            }
            self.frames.push(fr);
        }
        let depth = self.reentry_depth + 1;
        {
            let f = &mut self.fibers[id as usize];
            f.status = FiberStatus::Running;
            f.boundary = boundary;
            f.reentry_depth = depth;
        }
        self.fiber_stack.push(id);
        self.reentry_depth += 1;
        let r = match with {
            Ok(v) => {
                // The suspending call completes with `v`; continue after it.
                let top = self.frames.len() - 1;
                let base = self.frames[top].base;
                self.stack[base + dst as usize] = v;
                self.frames[top].pc += 1;
                self.run_until(boundary)
            }
            Err(e) => match self.dispatch_unwind(Unwind::Throw(e), boundary) {
                Ok(()) => self.run_until(boundary),
                Err(u) => Err(u),
            },
        };
        self.reentry_depth -= 1;
        self.out.flush_pending();
        self.fiber_finish(id, r)
    }

    /// What `start()`/`resume()`/`throw()` return once the chain stopped
    /// running: the transfer value of a suspend, null at the end.
    fn fiber_finish(&mut self, id: u32, r: Result<Value, Unwind>) -> NativeResult {
        let f = &mut self.fibers[id as usize];
        match r {
            Ok(v) => {
                if f.status == FiberStatus::Suspended {
                    return Ok(std::mem::replace(&mut f.transfer, Value::Null));
                }
                f.status = FiberStatus::Terminated;
                f.return_val = v;
                self.fiber_stack.retain(|&x| x != id);
                Ok(Value::Null)
            }
            Err(u) => {
                f.status = FiberStatus::Terminated;
                f.threw = true;
                self.fiber_stack.retain(|&x| x != id);
                Err(u)
            }
        }
    }
}

fn fiber_error(msg: &str) -> Unwind {
    Unwind::exception("FiberError", msg)
}

/// The receiver of a `Fiber` method.
fn this(o: Option<&Object>) -> Result<Object, Unwind> {
    o.cloned()
        .ok_or_else(|| Unwind::error("Fiber method called without an instance"))
}

/// `Fiber::__construct(callable $callback)`
fn fiber_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cb = args[0].deref().into_owned();
    ctx.autoload_callable(&cb)?;
    if !ctx.is_callable(&cb) {
        return Err(Unwind::type_error(format!(
            "Fiber::__construct(): Argument #1 ($callback) must be a valid callback, {}",
            match &cb {
                Value::Str(s) => format!(
                    "function \"{}\" not found or invalid function name",
                    s.to_string_lossy()
                ),
                Value::Array(_) => "array callback must have exactly two members".to_string(),
                _ => "no array or string given".to_string(),
            }
        )));
    }
    let id = ctx.fibers.len() as u32;
    ctx.fibers.push(FiberState {
        status: FiberStatus::Init,
        callable: cb,
        object: o.clone(),
        frames: Vec::new(),
        regs: Vec::new(),
        resume_dst: 0,
        transfer: Value::Null,
        return_val: Value::Null,
        threw: false,
        boundary: 0,
        reentry_depth: 0,
    });
    o.set_payload(rphp_value::Payload::Native(Box::new(id)));
    Ok(Value::Null)
}

/// `Fiber::start(mixed ...$args): mixed`
fn fiber_start(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let id = ctx.fiber_id(&this(o)?)?;
    if ctx.fibers[id as usize].status != FiberStatus::Init {
        return Err(fiber_error("Cannot start a fiber that has already been started"));
    }
    let named = ctx.take_extra_named();
    let callable = ctx.fibers[id as usize].callable.clone();
    let (boundary, depth) = (ctx.frames.len(), ctx.reentry_depth + 1);
    {
        let f = &mut ctx.fibers[id as usize];
        f.status = FiberStatus::Running;
        f.boundary = boundary;
        f.reentry_depth = depth;
    }
    ctx.fiber_stack.push(id);
    let r = ctx.call_value_named(&callable, args, named);
    ctx.fiber_finish(id, r)
}

/// `Fiber::resume(mixed $value = null): mixed`
fn fiber_resume(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let id = ctx.fiber_id(&this(o)?)?;
    if ctx.fibers[id as usize].status != FiberStatus::Suspended {
        return Err(fiber_error("Cannot resume a fiber that is not suspended"));
    }
    let v = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    ctx.fiber_continue(id, Ok(v))
}

/// `Fiber::throw(Throwable $exception): mixed`
fn fiber_throw(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let id = ctx.fiber_id(&this(o)?)?;
    let e = match &*args[0].deref() {
        Value::Object(e) if ctx.is_throwable(e) => e.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "Fiber::throw(): Argument #1 ($exception) must be of type Throwable, {} given",
                crate::ops::value_name(other)
            )))
        }
    };
    if ctx.fibers[id as usize].status != FiberStatus::Suspended {
        return Err(fiber_error("Cannot resume a fiber that is not suspended"));
    }
    ctx.fiber_continue(id, Err(e))
}

/// `Fiber::getReturn(): mixed`
fn fiber_get_return(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let id = ctx.fiber_id(&this(o)?)?;
    let f = &ctx.fibers[id as usize];
    match f.status {
        FiberStatus::Terminated if f.threw => Err(fiber_error(
            "Cannot get fiber return value: The fiber threw an exception",
        )),
        FiberStatus::Terminated => Ok(f.return_val.clone()),
        FiberStatus::Init => Err(fiber_error(
            "Cannot get fiber return value: The fiber has not been started",
        )),
        _ => Err(fiber_error("Cannot get fiber return value: The fiber has not returned")),
    }
}

fn status_of(ctx: &mut Ctx, o: Option<&Object>) -> Result<FiberStatus, Unwind> {
    let id = ctx.fiber_id(&this(o)?)?;
    Ok(ctx.fibers[id as usize].status)
}

fn fiber_is_started(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(status_of(ctx, o)? != FiberStatus::Init))
}

fn fiber_is_suspended(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(status_of(ctx, o)? == FiberStatus::Suspended))
}

fn fiber_is_running(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(status_of(ctx, o)? == FiberStatus::Running))
}

fn fiber_is_terminated(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(status_of(ctx, o)? == FiberStatus::Terminated))
}

/// `Fiber::suspend(mixed $value = null): mixed` — the value comes back
/// from the dispatch loop once the fiber is resumed.
fn fiber_suspend_native(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let v = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    ctx.fiber_suspend(v)?;
    Ok(Value::Null)
}

/// `Fiber::getCurrent(): ?Fiber`
fn fiber_get_current(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(ctx.current_fiber().map_or(Value::Null, Value::Object))
}

/// `new FiberError` is refused, as php refuses it.
fn fiber_error_construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "The \"FiberError\" class is reserved for internal use and cannot be manually instantiated",
    ))
}

fn static_method(min: u8, max: Option<u8>, f: crate::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

/// Register `Fiber` and `FiberError`. Called after the stdlib's `Error`
/// exists.
pub fn register_fiber_classes(r: &mut Registry) {
    if r.0.class_by_name(b"Fiber").is_some() {
        return;
    }
    r.class("Fiber")
        .flags(ClassFlags::FINAL)
        .method("__construct", nm!(1, Some(1), fiber_construct))
        .method("start", nm!(0, None, fiber_start))
        .method("resume", nm!(0, Some(1), fiber_resume))
        .method("throw", nm!(1, Some(1), fiber_throw))
        .method("getReturn", nm!(0, Some(0), fiber_get_return))
        .method("isStarted", nm!(0, Some(0), fiber_is_started))
        .method("isSuspended", nm!(0, Some(0), fiber_is_suspended))
        .method("isRunning", nm!(0, Some(0), fiber_is_running))
        .method("isTerminated", nm!(0, Some(0), fiber_is_terminated))
        .method("suspend", static_method(0, Some(1), fiber_suspend_native))
        .method("getCurrent", static_method(0, Some(0), fiber_get_current))
        .finish();
    if r.0.class_by_name(b"Error").is_some() {
        r.class("FiberError")
            .extends("Error")
            .flags(ClassFlags::FINAL)
            .method("__construct", nm!(0, None, fiber_error_construct))
            .finish();
    }
}
