//! Generators (plan E8).
//!
//! A function whose body contains `yield` is compiled with
//! [`FnFlags::GENERATOR`]; **calling it never runs the body**. The frame is
//! built exactly as for an ordinary call — so arguments are evaluated at the
//! call site, as php does — and then immediately *parked*: its register
//! window moves off the register stack into a [`GeneratorState`], and the
//! caller receives a `Generator` object.
//!
//! Resuming copies the window back onto the stack, pushes the frame at the
//! saved `pc` and runs the dispatch loop until the body yields, returns or
//! throws. `Op::Yield` parks it again. Everything the body sees — `$this`,
//! scope, the symbol table, live `foreach` iterators — travels with the
//! parked frame, so a generator can suspend anywhere, including inside a
//! `foreach` or a `try`.
//!
//! `yield from` delegates: an array or `Traversable` is iterated to
//! exhaustion, and an inner *generator* is driven directly so that `send()`
//! reaches it and its `getReturn()` becomes the value of the `yield from`
//! expression.

use rphp_value::{Object, Value};

use crate::frame::{Frame, FrameKind, RetTarget};
use crate::registry::Unwind;
use crate::Interp;

/// Where a generator is in its life cycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GenStatus {
    /// Built but never resumed: the body has not started.
    NotStarted,
    /// Parked at a `yield`.
    Suspended,
    /// The body is on the frame stack right now.
    Running,
    /// The body returned or threw; nothing more will come out of it.
    Finished,
}

/// What `yield from` is currently delegating to.
enum Delegate {
    /// An array or `Traversable` snapshot, by position.
    Values { items: Vec<(Value, Value)>, pos: usize },
    /// Another generator, by object.
    Gen(Object),
}

/// A parked generator body.
pub struct GeneratorState {
    /// The frame, minus its registers, as it was when parked.
    frame: Option<Frame>,
    /// The register window (`frame.base` is meaningless while parked).
    regs: Vec<Value>,
    /// Life cycle.
    pub status: GenStatus,
    /// The pair most recently yielded.
    pub current_key: Value,
    pub current_val: Value,
    /// The register the resumed `Op::Yield` must write the sent value into.
    resume_dst: Option<u16>,
    /// `return` value, for `getReturn()`.
    pub return_val: Value,
    /// The next auto-key for a `yield $v` with no key.
    auto_key: i64,
    /// The `yield from` in progress, if any.
    delegate: Option<Delegate>,
    /// The register the `yield from` result goes into.
    delegate_dst: Option<u16>,
    /// An exception to throw at the resumption point (`Generator::throw()`).
    pending_throw: Option<Value>,
}

impl GeneratorState {
    fn new(frame: Frame, regs: Vec<Value>) -> GeneratorState {
        GeneratorState {
            frame: Some(frame),
            regs,
            status: GenStatus::NotStarted,
            current_key: Value::Null,
            current_val: Value::Null,
            resume_dst: None,
            return_val: Value::Null,
            auto_key: 0,
            delegate: None,
            delegate_dst: None,
            pending_throw: None,
        }
    }
}

impl Interp {
    /// Deliver a value to a frame's return target (the tail of a call).
    fn deliver(&mut self, ret: RetTarget, v: Value) {
        match ret {
            RetTarget::Discard => {}
            RetTarget::Reg(abs) => {
                if abs < self.stack.len() {
                    self.stack[abs] = v;
                }
            }
            RetTarget::New { reg, obj } => {
                if reg < self.stack.len() {
                    self.stack[reg] = Value::Object(obj);
                }
            }
        }
    }

    /// Drain any `Traversable` object — `Iterator`, `IteratorAggregate` or
    /// `Generator` — into `(key, value)` pairs. The php-facing entry point
    /// behind `iterator_to_array()` and friends.
    pub fn iterate_traversable(&mut self, o: &Object) -> Result<Vec<(Value, Value)>, Unwind> {
        self.iterate_object_pairs(o)
    }

    /// Snapshot a `Traversable` object as `(key, value)` pairs, for
    /// `yield from`. A `Generator` is driven directly instead (see
    /// [`Delegate`]), so this only ever sees `Iterator` /
    /// `IteratorAggregate`.
    fn iterate_object_pairs(&mut self, o: &Object) -> Result<Vec<(Value, Value)>, Unwind> {
        let o = o.clone();
        // A `Generator` drains directly — it has no `rewind`/`valid` frame
        // protocol of its own to re-enter.
        if let Some(iid) = self.generator_id(&o) {
            let mut out = Vec::new();
            self.ensure_started(iid)?;
            while self.generators[iid as usize].status != GenStatus::Finished {
                let (k, v) = {
                    let g = &self.generators[iid as usize];
                    (g.current_key.clone(), g.current_val.clone())
                };
                out.push((k, v));
                self.resume(iid, Value::Null)?;
            }
            return Ok(out);
        }
        // `IteratorAggregate` hands over the real iterator first.
        let inner = if self
            .well_known
            .iterator_aggregate
            .is_some_and(|c| self.object_instanceof(&o, c))
        {
            match self.call_method(&o, b"getIterator", &[])? {
                Value::Object(i) => i,
                other => {
                    return Err(Unwind::error(format!(
                        "Objects returned by {}::getIterator() must be traversable or implement interface Iterator, {} given",
                        self.class_name_of(&o),
                        crate::ops::value_name(&other)
                    )))
                }
            }
        } else {
            o.clone()
        };
        if let Some(iid) = self.generator_id(&inner) {
            // A generator behind an aggregate: drain it.
            let mut out = Vec::new();
            self.ensure_started(iid)?;
            while self.generators[iid as usize].status != GenStatus::Finished {
                let (k, v) = {
                    let g = &self.generators[iid as usize];
                    (g.current_key.clone(), g.current_val.clone())
                };
                out.push((k, v));
                self.resume(iid, Value::Null)?;
            }
            return Ok(out);
        }
        let mut out = Vec::new();
        self.call_method(&inner, b"rewind", &[])?;
        while self.call_method(&inner, b"valid", &[])?.to_bool() {
            let v = self.call_method(&inner, b"current", &[])?;
            let k = self.call_method(&inner, b"key", &[])?;
            out.push((k, v));
            self.call_method(&inner, b"next", &[])?;
        }
        Ok(out)
    }

    /// The generator index carried by a `Generator` object.
    pub(crate) fn generator_id(&self, o: &Object) -> Option<u32> {
        o.with_payload(|id: &mut u32| *id)
    }

    /// Park the frame just pushed for a generator function and hand the
    /// caller a `Generator` object instead of running the body.
    pub(crate) fn park_new_generator(&mut self) -> Result<(), Unwind> {
        let mut frame = self.frames.pop().expect("generator frame");
        let regs = self.stack.split_off(frame.base);
        let ret = std::mem::replace(&mut frame.ret, RetTarget::Discard);
        let was = frame.kind;
        frame.kind = FrameKind::Generator;

        // A frame pushed by bytecode left its caller parked on the call op;
        // since this call produces a value without ever running a frame to
        // completion, advance it here exactly as `do_return` would. A
        // re-entry boundary's caller is a native still inside its own op, so
        // it must NOT be advanced.
        if matches!(was, FrameKind::Normal | FrameKind::Include) {
            if let Some(top) = self.frames.last_mut() {
                top.pc += 1;
            }
        }

        let idx = self.generators.len() as u32;
        self.generators.push(GeneratorState::new(frame, regs));

        let Some(cid) = self.well_known.generator else {
            return Err(Unwind::error("Generator class is not registered"));
        };
        let obj = self.instantiate(cid);
        obj.set_payload(rphp_value::Payload::Native(Box::new(idx)));
        self.generators[idx as usize].frame.as_mut().expect("frame").generator = Some(idx);
        let v = Value::Object(obj);
        if was == FrameKind::ReentryBoundary {
            // `run_until` hands this back to the native that re-entered.
            self.boundary_value = Some(v.clone());
        }
        self.deliver(ret, v);
        Ok(())
    }

    /// Park the running generator frame at a `yield`, publishing the pair.
    pub(crate) fn park_generator(
        &mut self,
        idx: u32,
        key: Value,
        val: Value,
        dst: u16,
    ) -> Result<(), Unwind> {
        let mut frame = self.frames.pop().expect("generator frame");
        // Resume after the `Yield` op, with the sent value in `dst`.
        frame.pc += 1;
        let regs = self.stack.split_off(frame.base);
        let g = &mut self.generators[idx as usize];
        g.frame = Some(frame);
        g.regs = regs;
        g.status = GenStatus::Suspended;
        g.resume_dst = Some(dst);
        g.current_key = key;
        g.current_val = val;
        Ok(())
    }

    /// The next auto-key (`yield $v` with no key), which php keeps counting
    /// past explicit integer keys.
    pub(crate) fn generator_auto_key(&mut self, idx: u32) -> Value {
        let g = &mut self.generators[idx as usize];
        let k = g.auto_key;
        g.auto_key += 1;
        Value::Int(k)
    }

    /// Record an explicit key, keeping the auto-key counter above it as php
    /// does for integer keys.
    pub(crate) fn generator_note_key(&mut self, idx: u32, key: &Value) {
        if let Value::Int(i) = key {
            let g = &mut self.generators[idx as usize];
            if *i >= g.auto_key {
                g.auto_key = *i + 1;
            }
        }
    }

    /// Finish a generator: record its return value and drop the frame.
    pub(crate) fn finish_generator(&mut self, idx: u32, value: Value) {
        let frame = self.frames.pop().expect("generator frame");
        self.stack.truncate(frame.base);
        let g = &mut self.generators[idx as usize];
        g.frame = None;
        g.regs = Vec::new();
        g.status = GenStatus::Finished;
        g.return_val = value;
        g.current_key = Value::Null;
        g.current_val = Value::Null;
    }

    /// Run the generator until it yields, returns or throws.
    ///
    /// `sent` is the value the parked `yield` expression evaluates to.
    fn resume(&mut self, idx: u32, sent: Value) -> Result<(), Unwind> {
        match self.generators[idx as usize].status {
            GenStatus::Finished => return Ok(()),
            GenStatus::Running => {
                return Err(Unwind::error("Cannot resume an already running generator"))
            }
            _ => {}
        }
        // Drive a `yield from` delegate first: while it has values, the outer
        // body stays parked.
        if self.step_delegate(idx, &sent)? {
            return Ok(());
        }

        let (mut frame, regs, dst, throw) = {
            let g = &mut self.generators[idx as usize];
            let frame = g.frame.take().expect("parked frame");
            let regs = std::mem::take(&mut g.regs);
            g.status = GenStatus::Running;
            (frame, regs, g.resume_dst.take(), g.pending_throw.take())
        };
        let base = self.stack.len();
        frame.base = base;
        self.stack.extend(regs);
        if let Some(d) = dst {
            let abs = base + d as usize;
            if abs < self.stack.len() {
                self.stack[abs] = sent;
            }
        }
        let depth = self.frames.len();
        if throw.is_some() {
            // `Generator::throw()`: the exception surfaces *at* the yield,
            // inside a `try` that ends with it.
            frame.pc = frame.pc.saturating_sub(1);
        }
        self.frames.push(frame);
        if let Some(e) = throw {
            let u = Unwind::Throw(match e {
                Value::Object(o) => o,
                other => return Err(Unwind::error(format!("Cannot throw {other:?}"))),
            });
            match self.dispatch_unwind(u, depth) {
                Ok(()) => {}
                Err(u) => {
                    self.force_finish(idx);
                    return Err(u);
                }
            }
        }
        match self.run_until(depth) {
            Ok(_) => Ok(()),
            Err(u) => {
                self.force_finish(idx);
                Err(u)
            }
        }
    }

    /// Mark a generator finished after its body escaped with an exception.
    fn force_finish(&mut self, idx: u32) {
        let g = &mut self.generators[idx as usize];
        g.frame = None;
        g.regs = Vec::new();
        g.status = GenStatus::Finished;
        g.current_key = Value::Null;
        g.current_val = Value::Null;
    }

    /// Advance a `yield from` delegate. Returns `true` when the delegate
    /// produced a value, leaving the outer body parked.
    fn step_delegate(&mut self, idx: u32, sent: &Value) -> Result<bool, Unwind> {
        let has = self.generators[idx as usize].delegate.is_some();
        if !has {
            return Ok(false);
        }
        // Advance, then publish whatever the delegate is now on.
        let started = self.generators[idx as usize].status != GenStatus::NotStarted;
        let done = match self.generators[idx as usize].delegate.as_mut() {
            Some(Delegate::Values { items, pos }) => {
                if started {
                    *pos += 1;
                }
                let (p, n) = (*pos, items.len());
                if p < n {
                    let (k, v) = items[p].clone();
                    let g = &mut self.generators[idx as usize];
                    g.current_key = k;
                    g.current_val = v;
                    g.status = GenStatus::Suspended;
                    return Ok(true);
                }
                true
            }
            Some(Delegate::Gen(inner)) => {
                let inner = inner.clone();
                let Some(iid) = self.generator_id(&inner) else {
                    return Err(Unwind::error("yield from: not a generator"));
                };
                // The outer body is unparked while the inner runs: a trace
                // shows `gen()` beneath `inner()`, and an exception out of
                // the inner surfaces at the `yield from`, where the outer's
                // own `try` may catch it.
                let unparked = {
                    let g = &mut self.generators[idx as usize];
                    g.frame.take().map(|mut frame| {
                        let regs = std::mem::take(&mut g.regs);
                        g.status = GenStatus::Running;
                        let base = self.stack.len();
                        frame.base = base;
                        // Back on the `yield from` op itself, so a `try`
                        // around it covers what the delegate throws.
                        frame.pc -= 1;
                        self.stack.extend(regs);
                        let depth = self.frames.len();
                        self.frames.push(frame);
                        (base, depth)
                    })
                };
                // `Generator::throw()` lands in the innermost delegate.
                if let Some(t) = self.generators[idx as usize].pending_throw.take() {
                    self.generators[iid as usize].pending_throw = Some(t);
                }
                let stepped = if started {
                    self.resume(iid, sent.clone())
                } else {
                    // First touch of the delegate: run the inner generator to
                    // its first `yield` before reading `current`.
                    self.ensure_started(iid)
                };
                let repark = |it: &mut Interp| {
                    if let Some((base, _)) = unparked {
                        let mut frame = it.frames.pop().expect("outer generator frame");
                        frame.pc += 1;
                        let regs = it.stack.split_off(base);
                        let g = &mut it.generators[idx as usize];
                        g.frame = Some(frame);
                        g.regs = regs;
                    }
                };
                if let Err(u) = stepped {
                    let Some((base, depth)) = unparked else {
                        return Err(u);
                    };
                    {
                        let g = &mut self.generators[idx as usize];
                        g.delegate = None;
                        g.delegate_dst = None;
                    }
                    return match self.dispatch_unwind(u, depth) {
                        // Caught inside the outer body: it runs on to its
                        // next `yield` or its end.
                        Ok(()) => match self.run_until(depth) {
                            Ok(_) => Ok(true),
                            Err(u) => {
                                self.force_finish(idx);
                                Err(u)
                            }
                        },
                        // Not caught: the dispatch popped the outer frame.
                        Err(u) => {
                            let _ = base;
                            self.force_finish(idx);
                            Err(u)
                        }
                    };
                }
                repark(self);
                if self.generators[iid as usize].status == GenStatus::Finished {
                    self.generators[idx as usize].status = GenStatus::Suspended;
                    true
                } else {
                    let (k, v) = {
                        let ig = &self.generators[iid as usize];
                        (ig.current_key.clone(), ig.current_val.clone())
                    };
                    let g = &mut self.generators[idx as usize];
                    g.current_key = k;
                    g.current_val = v;
                    g.status = GenStatus::Suspended;
                    return Ok(true);
                }
            }
            None => true,
        };
        if done {
            // The delegate is exhausted: its result becomes the value of the
            // `yield from` expression and the outer body continues.
            let result = match self.generators[idx as usize].delegate.take() {
                Some(Delegate::Gen(inner)) => match self.generator_id(&inner) {
                    Some(iid) => self.generators[iid as usize].return_val.clone(),
                    None => Value::Null,
                },
                _ => Value::Null,
            };
            let dst = self.generators[idx as usize].delegate_dst.take();
            self.generators[idx as usize].resume_dst = dst;
            // Feed the delegate's result in as the value of `yield from` and
            // let the outer body run on.
            self.resume_with(idx, result)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// `resume` without the delegate check, used once a `yield from` ends.
    fn resume_with(&mut self, idx: u32, value: Value) -> Result<(), Unwind> {
        let (mut frame, regs, dst) = {
            let g = &mut self.generators[idx as usize];
            let Some(frame) = g.frame.take() else {
                return Ok(());
            };
            let regs = std::mem::take(&mut g.regs);
            g.status = GenStatus::Running;
            (frame, regs, g.resume_dst.take())
        };
        let base = self.stack.len();
        frame.base = base;
        self.stack.extend(regs);
        if let Some(d) = dst {
            let abs = base + d as usize;
            if abs < self.stack.len() {
                self.stack[abs] = value;
            }
        }
        let depth = self.frames.len();
        self.frames.push(frame);
        match self.run_until(depth) {
            Ok(_) => Ok(()),
            Err(u) => {
                self.force_finish(idx);
                Err(u)
            }
        }
    }

    /// Begin a `yield from` over `src`, parking the outer body.
    pub(crate) fn begin_yield_from(
        &mut self,
        idx: u32,
        src: Value,
        dst: u16,
    ) -> Result<(), Unwind> {
        let delegate = match &*src.deref() {
            Value::Array(a) => Delegate::Values {
                items: a.iter().map(|(k, v)| (k.to_value(), v.deref().into_owned())).collect(),
                pos: 0,
            },
            Value::Object(o) if self.generator_id(o).is_some() => Delegate::Gen(o.clone()),
            Value::Object(o) => {
                // Any other Traversable: snapshot it through the ordinary
                // iteration protocol.
                let items = self.iterate_object_pairs(o)?;
                Delegate::Values { items, pos: 0 }
            }
            other => {
                return Err(Unwind::error(format!(
                    "Can use \"yield from\" only with arrays and Traversables, {} given",
                    crate::ops::value_name(other)
                )))
            }
        };
        // Park the outer body; the delegate drives the next `current()`.
        let mut frame = self.frames.pop().expect("generator frame");
        frame.pc += 1;
        let regs = self.stack.split_off(frame.base);
        {
            let g = &mut self.generators[idx as usize];
            g.frame = Some(frame);
            g.regs = regs;
            g.delegate = Some(delegate);
            g.delegate_dst = Some(dst);
            g.resume_dst = None;
            g.status = GenStatus::NotStarted;
        }
        // Publish the delegate's first value (or fall straight through when
        // it is empty).
        if !self.step_delegate(idx, &Value::Null)? {
            self.generators[idx as usize].status = GenStatus::Suspended;
        }
        Ok(())
    }

    // ---- the `Generator` class's methods ---------------------------------

    /// Run the body up to its first `yield`, if it has not started.
    fn ensure_started(&mut self, idx: u32) -> Result<(), Unwind> {
        if self.generators[idx as usize].status == GenStatus::NotStarted {
            self.resume(idx, Value::Null)?;
        }
        Ok(())
    }

    /// `Generator::current()`
    pub fn generator_current(&mut self, o: &Object) -> Result<Value, Unwind> {
        let idx = self.gen_idx(o)?;
        self.ensure_started(idx)?;
        Ok(self.generators[idx as usize].current_val.clone())
    }

    /// `Generator::key()`
    pub fn generator_key(&mut self, o: &Object) -> Result<Value, Unwind> {
        let idx = self.gen_idx(o)?;
        self.ensure_started(idx)?;
        Ok(self.generators[idx as usize].current_key.clone())
    }

    /// `Generator::next()`
    pub fn generator_next(&mut self, o: &Object) -> Result<(), Unwind> {
        let idx = self.gen_idx(o)?;
        self.ensure_started(idx)?;
        self.resume(idx, Value::Null)
    }

    /// `Generator::send($v)` — the value the parked `yield` evaluates to.
    pub fn generator_send(&mut self, o: &Object, v: Value) -> Result<Value, Unwind> {
        let idx = self.gen_idx(o)?;
        // php runs a not-yet-started generator to its first yield, and *that*
        // yield is the one that receives the sent value.
        self.ensure_started(idx)?;
        self.resume(idx, v)?;
        Ok(self.generators[idx as usize].current_val.clone())
    }

    /// `Generator::valid()`
    pub fn generator_valid(&mut self, o: &Object) -> Result<bool, Unwind> {
        let idx = self.gen_idx(o)?;
        self.ensure_started(idx)?;
        Ok(self.generators[idx as usize].status != GenStatus::Finished)
    }

    /// `Generator::rewind()` — only legal before the body has advanced past
    /// its first yield.
    pub fn generator_rewind(&mut self, o: &Object) -> Result<(), Unwind> {
        let idx = self.gen_idx(o)?;
        self.ensure_started(idx)?;
        Ok(())
    }

    /// `Generator::getReturn()`
    pub fn generator_get_return(&mut self, o: &Object) -> Result<Value, Unwind> {
        let idx = self.gen_idx(o)?;
        if self.generators[idx as usize].status != GenStatus::Finished {
            return Err(Unwind::error(
                "Cannot get return value of a generator that hasn't returned",
            ));
        }
        Ok(self.generators[idx as usize].return_val.clone())
    }

    /// `Generator::throw($e)` — the exception surfaces at the parked yield.
    pub fn generator_throw(&mut self, o: &Object, e: Value) -> Result<Value, Unwind> {
        let idx = self.gen_idx(o)?;
        self.ensure_started(idx)?;
        if self.generators[idx as usize].status == GenStatus::Finished {
            // Nothing is parked: the exception is simply thrown here.
            return Err(match e {
                Value::Object(obj) => Unwind::Throw(obj),
                other => Unwind::error(format!("Cannot throw {other:?}")),
            });
        }
        self.generators[idx as usize].pending_throw = Some(e);
        self.resume(idx, Value::Null)?;
        Ok(self.generators[idx as usize].current_val.clone())
    }

    fn gen_idx(&mut self, o: &Object) -> Result<u32, Unwind> {
        self.generator_id(o)
            .ok_or_else(|| Unwind::error("not a Generator"))
    }
}

// ---- the `Generator` class ------------------------------------------------
//
// Registered by the engine rather than an extension, because the state it
// wraps lives in `Interp::generators` and the class must exist before any
// user code runs. `Generator` is final, cannot be constructed from php, and
// implements `Iterator` so `foreach` drives it through the ordinary protocol.

use crate::registry::{Ctx, NativeResult, Registry};
use crate::{nm, ClassFlags};

/// Register `Generator`. Called once, after the SPL interfaces exist.
pub fn register_generator_class(r: &mut Registry) {
    if r.0.class_by_name(b"Generator").is_some() {
        return;
    }
    // `Iterator` is registered by the stdlib; when it is present `foreach`
    // drives a generator through the ordinary protocol.
    let ifaces: &[&str] = if r.0.class_by_name(b"Iterator").is_some() {
        &["Iterator"]
    } else {
        &[]
    };
    r.class("Generator")
        .flags(ClassFlags::FINAL)
        .implements(ifaces)
        .method("current", nm!(0, Some(0), gen_current))
        .method("key", nm!(0, Some(0), gen_key))
        .method("next", nm!(0, Some(0), gen_next))
        .method("send", nm!(1, Some(1), gen_send))
        .method("throw", nm!(1, Some(1), gen_throw))
        .method("valid", nm!(0, Some(0), gen_valid))
        .method("rewind", nm!(0, Some(0), gen_rewind))
        .method("getReturn", nm!(0, Some(0), gen_get_return))
        .finish();
}

/// The receiver of a `Generator` method.
fn this(o: Option<&Object>) -> Result<Object, Unwind> {
    o.cloned()
        .ok_or_else(|| Unwind::error("Generator method called without an instance"))
}

fn gen_current(ctx: &mut Ctx, o: Option<&Object>, _a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    ctx.generator_current(&o)
}

fn gen_key(ctx: &mut Ctx, o: Option<&Object>, _a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    ctx.generator_key(&o)
}

fn gen_next(ctx: &mut Ctx, o: Option<&Object>, _a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    ctx.generator_next(&o)?;
    Ok(Value::Null)
}

fn gen_send(ctx: &mut Ctx, o: Option<&Object>, a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let v = a.first().cloned().unwrap_or(Value::Null);
    ctx.generator_send(&o, v)
}

fn gen_throw(ctx: &mut Ctx, o: Option<&Object>, a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let v = a.first().cloned().unwrap_or(Value::Null);
    ctx.generator_throw(&o, v)
}

fn gen_valid(ctx: &mut Ctx, o: Option<&Object>, _a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(ctx.generator_valid(&o)?))
}

fn gen_rewind(ctx: &mut Ctx, o: Option<&Object>, _a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    ctx.generator_rewind(&o)?;
    Ok(Value::Null)
}

fn gen_get_return(ctx: &mut Ctx, o: Option<&Object>, _a: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    ctx.generator_get_return(&o)
}
