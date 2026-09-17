//! Exception dispatch (plan E5, CONTRACT.md §5): when an op returns
//! `Err(Unwind)`, [`Interp::dispatch_unwind`] walks the frames above the
//! dispatch loop's `stop_depth` looking for the innermost applicable
//! [`ExRegion`] — a matching `catch` (type test by `instanceof` **without**
//! autoload; an undeclared class never matches), else a pending `finally`
//! (state = `Throw`, payload = the object, continue at the finally entry) —
//! popping frames without one. Popping a re-entry boundary hands the
//! exception to the native that entered the VM, which propagates it with
//! `?`, so exceptions cross native frames. `Unwind::Exit` never runs
//! handlers (php 8: `exit` is not an exception; `finally` blocks do not
//! run). Engine faults (`Unwind::Pending`) become instances of their class
//! here, once a frame may observe them.

use rphp_bytecode::{ExRegion, FinallyState};
use rphp_value::{Object, Value};

use crate::frame::FrameKind;
use crate::registry::Unwind;
use crate::unit::FuncRt;
use crate::Interp;

/// What the region search decided for a frame.
enum Handler {
    /// Continue at `handler` with the exception in `dst`.
    Catch { handler: u32, dst: Option<u16> },
    /// Run the finally body with the exception pending.
    Finally { entry: u32, state: u16, payload: u16 },
}

impl Interp {
    /// Find a handler for the exception thrown at `pc` of `func` (frame
    /// registers at `base`): the first region, innermost-first, whose try
    /// range covers `pc` and has a matching catch, else whose protected
    /// range (try + catches) covers `pc` and has a finally. A throw from
    /// inside a finally body with a pending exception chains the pending
    /// one as `previous`.
    fn find_handler(&mut self, func: &FuncRt, base: usize, pc: u32, obj: &Object) -> Option<Handler> {
        for region in &func.f.ex_regions {
            if region.try_lo <= pc && pc < region.try_hi {
                for clause in &region.catches {
                    for &ty in &clause.types {
                        let name = match &func.f.consts[ty as usize] {
                            rphp_bytecode::Const::Name(n) => n.orig.clone(),
                            other => other.to_value().to_php_bytes().into_boxed_slice(),
                        };
                        let Some(cid) = self.class_by_name(&name) else {
                            continue;
                        };
                        if self.object_instanceof(obj, cid) {
                            return Some(Handler::Catch {
                                handler: clause.handler,
                                dst: clause.dst,
                            });
                        }
                    }
                }
            }
            if let Some(fin) = &region.finally {
                if region.try_lo <= pc && pc < fin.entry {
                    return Some(Handler::Finally {
                        entry: fin.entry,
                        state: fin.state,
                        payload: fin.payload,
                    });
                }
                if fin.lo <= pc && pc < fin.hi {
                    self.chain_pending(base, region, obj);
                }
            }
        }
        None
    }

    /// A throw inside a finally body whose region has `state == Throw`: the
    /// pending exception becomes the new one's `previous`.
    fn chain_pending(&self, base: usize, region: &ExRegion, obj: &Object) {
        let Some(fin) = &region.finally else { return };
        let state = self.stack[base + fin.state as usize].deref().into_owned();
        if FinallyState::from_code(state.to_int()) != Some(FinallyState::Throw) {
            return;
        }
        if let Value::Object(pending) = self.stack[base + fin.payload as usize].deref().into_owned() {
            self.throwable_chain_previous(obj, pending);
        }
    }

    /// Dispatch an unwind raised by the top frame: `Ok(())` when a handler
    /// in some frame above `stop_depth` took it (that frame is on top with
    /// its `pc` set to the handler), `Err` when the frames above
    /// `stop_depth` were popped without one (the caller of the dispatch
    /// loop receives the unwind).
    pub(crate) fn dispatch_unwind(&mut self, u: Unwind, stop_depth: usize) -> Result<(), Unwind> {
        let obj = match u {
            Unwind::Exit(code) => {
                self.unwind_to(stop_depth);
                return Err(Unwind::Exit(code));
            }
            Unwind::Pending(p) => match self.materialize(p) {
                Ok(o) => o,
                Err(p) => {
                    self.unwind_to(stop_depth);
                    return Err(Unwind::Pending(p));
                }
            },
            Unwind::Throw(o) => o,
        };
        loop {
            if self.frames.len() <= stop_depth {
                return Err(Unwind::Throw(obj));
            }
            let fi = self.frames.len() - 1;
            let (func, base, pc, kind) = {
                let f = &self.frames[fi];
                (f.func.clone(), f.base, f.pc as u32, f.kind)
            };
            if let Some(func) = func {
                if let Some(h) = self.find_handler(&func, base, pc, &obj) {
                    // Discard calls being set up inside the protected range
                    // and their staged arguments; restore the `@` depth.
                    let f = &mut self.frames[fi];
                    f.pending.clear();
                    let top = base + func.f.num_regs as usize;
                    self.stack.truncate(top);
                    self.silence = f.silence_base;
                    match h {
                        Handler::Catch { handler, dst } => {
                            if let Some(r) = dst {
                                Value::assign(&mut self.stack[base + r as usize], Value::Object(obj));
                            }
                            self.frames[fi].pc = handler as usize;
                        }
                        Handler::Finally { entry, state, payload } => {
                            self.stack[base + state as usize] = Value::Int(FinallyState::Throw.code());
                            self.stack[base + payload as usize] = Value::Object(obj);
                            self.frames[fi].pc = entry as usize;
                        }
                    }
                    return Ok(());
                }
            }
            // No handler in this frame: pop it.
            let f = self.frames.pop().expect("frame");
            if f.is_user() {
                self.stack.truncate(f.base);
            }
            self.silence = f.silence_base;
            if kind == FrameKind::ReentryBoundary || self.frames.len() <= stop_depth {
                return Err(Unwind::Throw(obj));
            }
        }
    }

    /// Run `__destruct` on every object queued by the last drops
    /// (ADR-017): FIFO, each once; an exception from a destructor
    /// propagates from here.
    pub fn run_pending_destructors(&mut self) -> Result<(), Unwind> {
        let queued = rphp_value::take_pending_destructors();
        for obj in queued {
            self.call_destructor(&obj)?;
        }
        Ok(())
    }

    /// Call `__destruct` on `obj` (its class has one) unless it already
    /// ran; marks the object destructed either way.
    pub fn call_destructor(&mut self, obj: &Object) -> Result<(), Unwind> {
        obj.add_flags(rphp_value::ObjFlags::DESTRUCTED);
        if !self
            .class_of(obj)
            .magic
            .contains(crate::class::MagicFlags::DESTRUCT)
        {
            return Ok(());
        }
        self.call_method(obj, b"__destruct", &[])?;
        Ok(())
    }

    /// End-of-request destructor pass (php's `shutdown_destructors`): the
    /// global symbol table is walked in reverse declaration order and every
    /// object held only by its global is released (repeated until stable),
    /// then every still-live object with a destructor is destructed in
    /// creation order. `Err` when a destructor throws / exits (rendered by
    /// the caller; the remaining destructors still run).
    pub fn shutdown_destructors(&mut self) -> Result<(), Unwind> {
        self.in_shutdown = true;
        let mut first_err: Option<Unwind> = None;
        // Phase 1: globals, reverse order, refcount-1 objects only.
        loop {
            let names: Vec<Box<[u8]>> = self.globals.with(|t| t.iter().map(|(n, _)| Box::from(n)).collect());
            let before = names.len();
            let mut removed = 0;
            for name in names.iter().rev() {
                let Some(cell) = self.globals.get(name) else { continue };
                let is_sole_object = matches!(&*cell.borrow(), Value::Object(o) if o.strong_count() == 1);
                // `cell` is our handle plus the table's: nobody else holds
                // the variable (a `global $x` binding or a reference would).
                if is_sole_object && cell.strong_count() == 2 {
                    self.globals.with_mut(|t| {
                        t.remove(name);
                    });
                    drop(cell);
                    removed += 1;
                    if let Err(u) = self.run_pending_destructors() {
                        first_err.get_or_insert(u);
                    }
                }
            }
            let after = self.globals.with(|t| t.len());
            if removed == 0 || after == before {
                break;
            }
        }
        // Phase 2: everything else with a destructor, in creation order.
        let live: Vec<Object> = std::mem::take(&mut self.destructibles)
            .iter()
            .filter_map(|w| w.upgrade())
            .collect();
        for obj in live {
            if obj.flags().contains(rphp_value::ObjFlags::DESTRUCTED) {
                continue;
            }
            if let Err(u) = self.call_destructor(&obj) {
                first_err.get_or_insert(u);
            }
        }
        if let Err(u) = self.run_pending_destructors() {
            first_err.get_or_insert(u);
        }
        self.in_shutdown = false;
        match first_err {
            Some(u) => Err(u),
            None => Ok(()),
        }
    }
}
