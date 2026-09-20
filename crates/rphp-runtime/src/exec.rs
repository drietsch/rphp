//! The dispatch loop (ADR-018): [`Interp::run_until`] runs the frame on top
//! of the explicit frame stack until the stack shrinks back to `stop_depth`,
//! switching frames in place on calls, returns and includes — PHP→PHP calls
//! never recurse on the Rust stack. Every fault is an [`Unwind`]
//! (ADR-022): the frames above `stop_depth` are popped (registers released,
//! `@` depth restored) and the error returned to the caller — a native's
//! `call_value`, or the SAPI for the entry `{main}`.

use rphp_bytecode::{
    AssignOpKind, ClassRef, ClassRefKind, Const, FinallyState, FnFlags, IncludeKind, InitRef,
    NameRef, NameRefKind, Op, Visibility,
};
use rphp_value::{array_key, Closure, Object, PhpRef, Value};

use crate::class::MethodBody;
use crate::frame::{CallTarget, FrameKind, IterState, PendingCall, RetTarget};
use crate::ops::value_name;
use crate::registry::Unwind;
use crate::symtab::Symtab;
use crate::Interp;

/// The keyword of a visibility, for messages.
pub(crate) fn vis_word(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

/// What a frame did when its op stream was left.
enum Switch {
    /// A call, include or return replaced the top frame; keep looping.
    Continue,
    /// The frame at `stop_depth` returned this value.
    Done(Value),
}

impl Interp {
    /// Run until the frame stack is back to `stop_depth` frames; the value
    /// of the frame that was on top of `stop_depth` is returned (the
    /// re-entry boundary's result).
    pub fn run_until(&mut self, stop_depth: usize) -> Result<Value, Unwind> {
        loop {
            if self.frames.len() <= stop_depth {
                // A generator built at a re-entry boundary left its value
                // here, because its frame never returned (`generator.rs`).
                return Ok(self.boundary_value.take().unwrap_or(Value::Null));
            }
            match self.run_frame(stop_depth) {
                Ok(Switch::Continue) => continue,
                Ok(Switch::Done(v)) => return Ok(v),
                Err(u) => {
                    let u = self.locate_fault(Err::<(), _>(u)).unwrap_err();
                    // Search the frames above `stop_depth` for a handler
                    // (`unwind.rs`); when none applies they are popped and
                    // the unwind returns to the caller of this loop.
                    self.dispatch_unwind(u, stop_depth)?;
                }
            }
        }
    }

    /// Pop every frame above `depth`, releasing its registers and restoring
    /// the `@` depth it was entered with.
    pub(crate) fn unwind_to(&mut self, depth: usize) {
        while self.frames.len() > depth {
            let f = self.frames.pop().expect("frame");
            if f.is_user() {
                self.stack.truncate(f.base);
            }
            self.silence = f.silence_base;
        }
    }

    // ---- register access ----------------------------------------------------

    #[inline]
    fn rd(&self, base: usize, r: u16) -> Value {
        self.stack[base + r as usize].deref().into_owned()
    }

    /// php's `Warning: Undefined variable $x` for a register that holds a
    /// variable which was never assigned. The name comes from the function's
    /// `var_names` table, so the op itself does not have to carry it.
    fn warn_if_undefined(
        &mut self,
        func: &crate::unit::FuncRt,
        base: usize,
        reg: u16,
    ) -> Result<(), Unwind> {
        if !self.raw(base, reg).deref().is_uninit() {
            return Ok(());
        }
        let name = func
            .f
            .var_names
            .iter()
            .find(|(_, r)| *r == reg)
            .map(|(n, _)| n.clone());
        if let Some(name) = name {
            self.warn(&format!(
                "Undefined variable ${}",
                String::from_utf8_lossy(&name)
            ))?;
        }
        Ok(())
    }

    #[inline]
    fn raw(&self, base: usize, r: u16) -> &Value {
        &self.stack[base + r as usize]
    }

    #[inline]
    fn set(&mut self, base: usize, r: u16, v: Value) {
        self.stack[base + r as usize] = v;
    }

    /// Run `f` on the storage slot register `r` denotes: the cell behind a
    /// `Ref`, else the register itself. `f` must not touch the interpreter.
    fn with_slot<R>(&mut self, base: usize, r: u16, f: impl FnOnce(&mut Value) -> R) -> R {
        let slot = &mut self.stack[base + r as usize];
        match slot {
            Value::Ref(cell) => {
                let cell = cell.clone();
                cell.update(f)
            }
            _ => f(slot),
        }
    }

    /// Bind register `r` to `cell` and, in a symbol-table frame, the named
    /// variable it holds too.
    fn rebind(&mut self, base: usize, r: u16, cell: PhpRef) {
        self.stack[base + r as usize] = Value::Ref(cell.clone());
        let fi = self.frames.len() - 1;
        let f = &self.frames[fi];
        if let (Some(symtab), Some(func)) = (&f.symtab, &f.func) {
            if let Some(name) = func.reg_name(r) {
                symtab.insert(name, cell);
            }
        }
    }

    /// The `Ref` cell of register `r`, making it one in place.
    /// Bind register `r` to a reference cell.
    ///
    /// Taking a reference to a variable that was never assigned *defines* it,
    /// as `null` — which is why php says nothing about `$u` being undefined
    /// after `f($u)` with a by-reference parameter. Binding a *symbol table*
    /// entry is not that (`Op::BindSymtab` goes straight to
    /// [`Value::make_ref`]), so an untouched variable stays uninitialized and
    /// `get_defined_vars()` does not list it.
    fn make_ref(&mut self, base: usize, r: u16) -> PhpRef {
        let slot = &mut self.stack[base + r as usize];
        match slot {
            // Already a cell (a symbol-table binding, an earlier `&`): define
            // what it holds rather than rebinding it.
            Value::Ref(cell) => {
                if cell.get().is_uninit() {
                    cell.set(Value::Null);
                }
                cell.clone()
            }
            _ => {
                if slot.is_uninit() {
                    *slot = Value::Null;
                }
                Value::make_ref(slot)
            }
        }
    }

    fn const_value(&self, func: &crate::unit::FuncRt, k: u32) -> Value {
        func.f.consts[k as usize].to_value()
    }

    fn name_bytes(&self, func: &crate::unit::FuncRt, k: u32) -> Box<[u8]> {
        match &func.f.consts[k as usize] {
            Const::Name(n) => n.orig.clone(),
            other => other.to_value().to_php_bytes().into_boxed_slice(),
        }
    }

    /// A member name operand as bytes.
    fn member_name(&self, func: &crate::unit::FuncRt, base: usize, name: NameRef) -> Result<Box<[u8]>, Unwind> {
        Ok(match name.kind() {
            NameRefKind::Const(k) => self.name_bytes(func, k),
            NameRefKind::Reg(r) => {
                let v = self.rd(base, r);
                if matches!(v, Value::Array(_) | Value::Object(_) | Value::Closure(_)) {
                    return Err(Unwind::error("Illegal member name"));
                }
                v.to_php_bytes().into_boxed_slice()
            }
        })
    }

    /// Resolve a `ClassRef` operand to a declared class (no autoload yet).
    fn resolve_class_ref(
        &mut self,
        func: &crate::unit::FuncRt,
        base: usize,
        class: ClassRef,
    ) -> Result<u32, Unwind> {
        let fi = self.frames.len() - 1;
        match class.kind() {
            ClassRefKind::Named(k) => {
                // `new A`, `A::m()`, `A::$p`, `A::C` all autoload (E7).
                let name = self.name_bytes(func, k);
                self.lookup_class_or_error(&name)
            }
            ClassRefKind::SelfKw => self.frames[fi]
                .scope
                .ok_or_else(|| Unwind::error("Cannot use \"self\" when no class scope is active")),
            ClassRefKind::Parent => {
                let scope = self.frames[fi]
                    .scope
                    .ok_or_else(|| Unwind::error("Cannot use \"parent\" when no class scope is active"))?;
                self.classes[scope as usize].parent.ok_or_else(|| {
                    Unwind::error("Cannot use \"parent\" when current class scope has no parent")
                })
            }
            ClassRefKind::Static => self.frames[fi]
                .static_class
                .or(self.frames[fi].scope)
                .ok_or_else(|| Unwind::error("Cannot use \"static\" when no class scope is active")),
            ClassRefKind::Reg(r) => match self.rd(base, r) {
                Value::Object(o) => Ok(o.class_id()),
                // `new $cls` autoloads like `new A` does. php looks the name
                // up with a leading `\` stripped — the loader is called with
                // `Foo`, never `\Foo` — but keeps the caller's spelling in
                // the error.
                Value::Str(s) => {
                    let raw = s.as_bytes().to_vec();
                    let name = raw.strip_prefix(b"\\").unwrap_or(&raw).to_vec();
                    match self.lookup_class(&name)? {
                        Some(id) => Ok(id),
                        None => Err(Unwind::error(format!(
                            "Class \"{}\" not found",
                            String::from_utf8_lossy(&raw)
                        ))),
                    }
                }
                other => Err(Unwind::error(format!(
                    "Cannot use value of type {} as class name",
                    value_name(&other)
                ))),
            },
        }
    }

    /// How the source spelled a class reference (`self`, `parent`, `static`,
    /// or the name as written); a dynamic one falls back to the class it
    /// resolved to. php quotes this spelling in
    /// `Cannot declare self-referencing constant self::X`.
    fn class_ref_spelling(&self, func: &crate::unit::FuncRt, cid: u32, class: ClassRef) -> String {
        match class.kind() {
            ClassRefKind::SelfKw => "self".to_string(),
            ClassRefKind::Parent => "parent".to_string(),
            ClassRefKind::Static => "static".to_string(),
            ClassRefKind::Named(k) => {
                String::from_utf8_lossy(&self.name_bytes(func, k)).into_owned()
            }
            ClassRefKind::Reg(_) => self.classes[cid as usize].name_str(),
        }
    }

    /// Like [`Interp::resolve_class_ref`] but never errors on an unknown
    /// name (`instanceof`).
    fn resolve_class_ref_quiet(&mut self, func: &crate::unit::FuncRt, base: usize, class: ClassRef) -> Result<Option<u32>, Unwind> {
        match class.kind() {
            ClassRefKind::Named(k) => {
                let name = self.name_bytes(func, k);
                Ok(self.class_by_name(&name))
            }
            ClassRefKind::Reg(r) => match self.rd(base, r) {
                Value::Object(o) => Ok(Some(o.class_id())),
                // `$x instanceof $name` never autoloads: an unknown class
                // simply does not match.
                Value::Str(s) => {
                    let raw = s.as_bytes();
                    Ok(self.class_by_name(raw.strip_prefix(b"\\").unwrap_or(raw)))
                }
                _ => Err(Unwind::error("Class name must be a valid object or a string")),
            },
            _ => self.resolve_class_ref(func, base, class).map(Some),
        }
    }

    // ---- properties ---------------------------------------------------------

    /// Enforce property visibility. An undeclared (dynamic) property is public.
    pub(crate) fn check_prop_access(&self, class: u32, name: &[u8]) -> Result<(), Unwind> {
        if let Some((vis, decl)) = self.resolve_prop(class, name) {
            let scope = self.current_user_frame().and_then(|f| f.scope);
            if !self.access_ok(vis, decl, scope) {
                return Err(Unwind::error(format!(
                    "Cannot access {} property {}::${}",
                    vis_word(vis),
                    String::from_utf8_lossy(&self.classes[decl as usize].name),
                    String::from_utf8_lossy(name),
                )));
            }
        }
        Ok(())
    }

    // ---- iteration ----------------------------------------------------------

    /// The `foreach` state for an object: php's `Iterator` protocol when the
    /// object — or the `IteratorAggregate` behind it — implements it, else
    /// the properties visible from the calling scope.
    ///
    /// An `Iterator` is carried in the `ByRef` cell of an
    /// [`IterState`](crate::IterState) (an object there means "drive the
    /// protocol"); `IterState` has no object variant yet because `frame.rs`
    /// is not this wave's file to change.
    ///
    /// TODO(E8): a `Generator` is a `Traversable` whose iteration resumes a
    /// suspended frame instead of calling methods. Generators are plan E8;
    /// there is no `Generator` class yet, so nothing can reach this path as
    /// one — when E8 lands, it branches here before `resolve_iterator`.
    fn object_iter_state(&mut self, o: &Object, by_ref: bool) -> Result<IterState, Unwind> {
        // A lazy object initializes before it is walked; a proxy walks its
        // real instance.
        let o = &self.lazy_resolve(o)?;
        if let Some(iter) = self.resolve_iterator(o)? {
            if by_ref {
                return Err(Unwind::error(
                    "An iterator cannot be used with foreach by reference",
                ));
            }
            self.call_method(&iter, b"rewind", &[])?;
            return Ok(IterState::ByRef {
                cell: PhpRef::new(Value::Object(iter)),
                pos: 0,
            });
        }
        // A plain object iterates the properties visible from the calling
        // scope; an uninitialized typed property is skipped.
        let scope = self.current_user_frame().and_then(|f| f.scope);
        let mut arr = rphp_value::Array::new();
        for (name, value, _) in o.props_snapshot() {
            if let Some((vis, decl)) = self.resolve_prop(o.class_id(), &name) {
                if !self.access_ok(vis, decl, scope) {
                    continue;
                }
            }
            let key = rphp_value::ArrayKey::str(&name);
            if by_ref {
                arr.set(key, Value::Ref(o.prop_ref(&name)));
            } else {
                arr.set(key, value);
            }
        }
        Ok(if by_ref {
            IterState::ByRef {
                cell: PhpRef::new(Value::Array(arr)),
                pos: 0,
            }
        } else {
            IterState::Array { arr, pos: 0 }
        })
    }

    /// The `Iterator` an object iterates through: itself, or what its
    /// `getIterator()` chain yields. `None` for a plain object.
    fn resolve_iterator(&mut self, o: &Object) -> Result<Option<Object>, Unwind> {
        // php follows `getIterator()` as deep as it goes; the engine stops
        // before the Rust stack does.
        const MAX_AGGREGATE_DEPTH: usize = 64;
        if self.is_iterator(o) {
            return Ok(Some(o.clone()));
        }
        let mut cur = o.clone();
        for _ in 0..MAX_AGGREGATE_DEPTH {
            if !self.is_iterator_aggregate(&cur) {
                return Ok(None);
            }
            let got = self.call_method(&cur, b"getIterator", &[])?.unref();
            match got {
                Value::Object(next) if next.ptr_eq(&cur) => return Err(self.aggregate_error(&cur)),
                Value::Object(next) if self.is_iterator(&next) => return Ok(Some(next)),
                Value::Object(next) if self.is_iterator_aggregate(&next) => cur = next,
                _ => return Err(self.aggregate_error(&cur)),
            }
        }
        Err(self.aggregate_error(&cur))
    }

    /// One step of php's `Iterator` protocol: `next()` from the second step
    /// on, then `valid()`, `current()` and — only when the loop binds one —
    /// `key()`, which is the order php calls them in.
    #[allow(clippy::type_complexity)]
    fn iterator_step(
        &mut self,
        o: &Object,
        pos: usize,
        want_key: bool,
    ) -> Result<Option<(Value, Value, Option<PhpRef>)>, Unwind> {
        if pos > 0 {
            self.call_method(o, b"next", &[])?;
        }
        let valid = self.call_method(o, b"valid", &[])?.unref();
        if !valid.to_bool() {
            return Ok(None);
        }
        let v = self.call_method(o, b"current", &[])?.unref();
        let k = if want_key {
            self.call_method(o, b"key", &[])?.unref()
        } else {
            Value::Null
        };
        Ok(Some((k, v, None)))
    }

    /// Whether the object implements `Iterator`.
    fn is_iterator(&self, o: &Object) -> bool {
        self.well_known
            .iterator
            .or_else(|| self.class_by_name(b"Iterator"))
            .is_some_and(|id| self.object_instanceof(o, id))
    }

    /// Whether the object implements `IteratorAggregate`.
    fn is_iterator_aggregate(&self, o: &Object) -> bool {
        self.well_known
            .iterator_aggregate
            .or_else(|| self.class_by_name(b"IteratorAggregate"))
            .is_some_and(|id| self.object_instanceof(o, id))
    }

    /// php's `Exception` for a `getIterator()` that did not yield something
    /// iterable.
    fn aggregate_error(&self, o: &Object) -> Unwind {
        Unwind::exception(
            "Exception",
            format!(
                "Objects returned by {}::getIterator() must be traversable or implement interface Iterator",
                self.class_name_of(o)
            ),
        )
    }

    // ---- the loop -------------------------------------------------------------

    /// Execute the top frame's ops until it switches frames.
    fn run_frame(&mut self, stop_depth: usize) -> Result<Switch, Unwind> {
        let fi = self.frames.len() - 1;
        let func = self.frames[fi]
            .func
            .clone()
            .expect("the top frame runs bytecode");
        let base = self.frames[fi].base;
        let mut pc = self.frames[fi].pc;
        let code = &func.f.code;
        loop {
            if rphp_value::has_pending_destructors() {
                // ADR-017: objects whose last handle dropped during the
                // previous op get their `__destruct` now.
                self.frames[fi].pc = pc;
                self.run_pending_destructors()?;
            }
            let Some(&op) = code.get(pc) else {
                // Falling off the end is an implicit `return null`.
                self.frames[fi].pc = pc;
                let strict = self.frames[fi].strict;
                let v = self.verify_return(&func, None, strict)?;
                return self.do_return(v, stop_depth);
            };
            self.frames[fi].pc = pc;
            match op {
                // --- moves / constants ---
                Op::LoadConst { dst, k } => {
                    let v = self.const_value(&func, k);
                    self.set(base, dst, v);
                }
                Op::LoadNull { dst } => self.set(base, dst, Value::Null),
                Op::LoadBool { dst, val } => self.set(base, dst, Value::Bool(val)),
                Op::Move { dst, src } => {
                    let v = self.raw(base, src).clone();
                    self.set(base, dst, v);
                }
                Op::Deref { dst, src } => {
                    let v = self.rd(base, src);
                    self.set(base, dst, v);
                }
                Op::AssignThroughRef { dst, src } => {
                    let v = self.rd(base, src);
                    Value::assign(&mut self.stack[base + dst as usize], v);
                }
                Op::MakeRef { var } => {
                    self.make_ref(base, var);
                }
                Op::AssignRef { dst, src } => {
                    let cell = self.make_ref(base, src);
                    self.rebind(base, dst, cell);
                }

                // --- arithmetic ---
                Op::Add { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Add)?,
                Op::Sub { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Sub)?,
                Op::Mul { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Mul)?,
                Op::Div { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Div)?,
                Op::Mod { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Mod)?,
                Op::Pow { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Pow)?,
                Op::Concat { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Concat)?,
                Op::BitAnd { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::BitAnd)?,
                Op::BitOr { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::BitOr)?,
                Op::BitXor { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::BitXor)?,
                Op::Shl { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Shl)?,
                Op::Shr { dst, a, b } => self.arith(base, dst, a, b, AssignOpKind::Shr)?,
                Op::Neg { dst, src } => {
                    let v = self.rd(base, src);
                    let r = self.unary_neg(&v)?;
                    self.set(base, dst, r);
                }
                Op::Plus { dst, src } => {
                    let v = self.rd(base, src);
                    let r = self.unary_plus(&v)?;
                    self.set(base, dst, r);
                }
                Op::BitNot { dst, src } => {
                    let v = self.rd(base, src);
                    let r = self.bit_not(&v)?;
                    self.set(base, dst, r);
                }
                Op::Not { dst, src } => {
                    let v = self.rd(base, src);
                    self.set(base, dst, v.not());
                }
                Op::ConcatN { dst, base: b0, n } => {
                    let mut out = Vec::new();
                    for i in 0..n {
                        let v = self.rd(base, b0 + i);
                        match &v {
                            Value::Object(_) | Value::Array(_) => {
                                out.extend_from_slice(self.to_string(&v)?.as_bytes());
                            }
                            _ => v.append_php_bytes(&mut out),
                        }
                    }
                    self.set(base, dst, Value::Str(rphp_value::Str::from_vec(out)));
                }
                Op::Cast { dst, src, kind } => {
                    let v = self.rd(base, src);
                    let r = self.cast(&v, kind)?;
                    self.set(base, dst, r);
                }
                Op::AssignOp { op, var, src } => {
                    let cur = self.rd(base, var);
                    let rhs = self.rd(base, src);
                    let r = self.binary_op(op, &cur, &rhs)?;
                    Value::assign(&mut self.stack[base + var as usize], r);
                }
                Op::AssignOpElem { op, arr, key, src } => {
                    let k = self.rd(base, key);
                    let rhs = self.rd(base, src);
                    // Read the element without keeping a handle on the
                    // container: a live clone would force a copy-on-write of
                    // the whole array in the store below.
                    let container = self.rd(base, arr);
                    // `$o[$k] .= $v` on `ArrayAccess` is php's
                    // `offsetGet()`, the operation, then `offsetSet()`.
                    if let Value::Object(o) = &*container.deref() {
                        if self.is_array_access(o) {
                            let o = o.clone();
                            let cur = self.offset_get(&o, &k)?;
                            let r = self.binary_op(op, &cur, &rhs)?;
                            self.offset_set(&o, Some(&k), r)?;
                            pc += 1;
                            continue;
                        }
                    }
                    let cur = match &container {
                        Value::Null | Value::Uninit => Value::Null,
                        _ => self.array_get(&container, &k)?,
                    };
                    drop(container);
                    let r = self.binary_op(op, &cur, &rhs)?;
                    self.array_set(base, arr, Some(key), r)?;
                }
                Op::AssignOpProp { op, obj, name, src } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    let cur = self.fetch_prop(&o, &name)?;
                    let rhs = self.rd(base, src);
                    let r = self.binary_op(op, &cur, &rhs)?;
                    self.assign_prop(&o, &name, r)?;
                }
                Op::IncDec { var, dst, pre, inc } => {
                    let old = self.rd(base, var);
                    let new = self.inc_dec(&old, inc)?;
                    Value::assign(&mut self.stack[base + var as usize], new.clone());
                    if let Some(d) = dst {
                        self.set(base, d, if pre { new } else { old });
                    }
                }

                // --- arrays ---
                Op::NewArray { dst } => self.set(base, dst, Value::empty_array()),
                Op::ArrayGet { dst, base: b, key } => {
                    let container = self.rd(base, b);
                    let k = self.rd(base, key);
                    // E6: `ArrayAccess` takes over `$o[$k]` (`objects.rs`).
                    let v = match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            self.offset_get(&o, &k)?
                        }
                        _ => self.array_get(&container, &k)?,
                    };
                    self.set(base, dst, v);
                }
                Op::FetchElemRW { dst, base: b, key } => {
                    let container = self.rd(base, b);
                    let k = self.rd(base, key);
                    let v = match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            self.fetch_dim_w_object(&o, k, false)?
                        }
                        _ => self.array_get(&container, &k)?,
                    };
                    self.set(base, dst, v);
                }
                Op::ListGet { dst, base: b, key } => {
                    let container = self.rd(base, b);
                    let k = self.rd(base, key);
                    let v = match &container {
                        Value::Array(_) => self.array_get(&container, &k)?,
                        Value::Null | Value::Uninit => Value::Null,
                        Value::Object(o) => {
                            return Err(Unwind::error(format!(
                                "Cannot use object of type {} as array",
                                self.class_name_of(o)
                            )))
                        }
                        other => {
                            self.warn(&format!("Cannot use {} as array", value_name(other)))?;
                            Value::Null
                        }
                    };
                    self.set(base, dst, v);
                }
                Op::ArrayGetQuiet { dst, base: b, key } => {
                    let container = self.rd(base, b);
                    let k = self.rd(base, key);
                    // `$o[$k] ?? d`: php asks `offsetExists` before reading.
                    let v = match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            if self.offset_exists(&o, &k)? {
                                self.offset_get(&o, &k)?
                            } else {
                                Value::Null
                            }
                        }
                        _ => self.array_get_quiet(&container, &k),
                    };
                    self.set(base, dst, v);
                }
                Op::ArraySet { arr, key, value } => {
                    let v = self.rd(base, value);
                    let container = self.rd(base, arr);
                    match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            let k = self.rd(base, key);
                            self.offset_set(&o, Some(&k), v)?;
                        }
                        _ => self.array_set(base, arr, Some(key), v)?,
                    }
                }
                Op::ArrayPush { arr, value } => {
                    let v = self.rd(base, value);
                    let container = self.rd(base, arr);
                    match &*container.deref() {
                        // `$o[] = $v` is `offsetSet(null, $v)`.
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            self.offset_set(&o, None, v)?;
                        }
                        _ => self.array_set(base, arr, None, v)?,
                    }
                }
                Op::WriteBackElem { arr, key, val } => {
                    let v = self.rd(base, val);
                    let container = self.rd(base, arr);
                    match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            if self.has_dim_storage(o) {
                                let o = o.clone();
                                let k = key.map(|k| self.rd(base, k));
                                self.offset_set(&o, k.as_ref(), v)?;
                            }
                            // The fetched temporary (or the cell a
                            // by-reference `offsetGet()` handed out) is done
                            // with: release it, as php does at the end of
                            // the statement.
                            self.set(base, val, Value::Null);
                        }
                        _ => self.array_set(base, arr, key, v)?,
                    }
                }
                Op::FetchElemW { dst, arr, key } => {
                    let k = key.map(|k| self.rd(base, k));
                    let container = self.rd(base, arr);
                    let taken = match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            self.fetch_dim_w_object(&o, k.unwrap_or(Value::Null), true)?
                        }
                        _ => self.fetch_elem_w(base, arr, k.as_ref())?,
                    };
                    self.set(base, dst, taken);
                }
                Op::FetchPropW { dst, obj, name } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    let holder = self.prop_holder(&o, &name)?;
                    let name = self.prop_storage_key(&holder, &name);
                    self.check_prop_access(holder.class_id(), &name)?;
                    self.check_indirect_modify(&holder, &name)?;
                    // Take the property value out (or share its cell) so the
                    // nested write mutates in place; the write-back restores it.
                    let taken = holder.with_data_mut(|d| match d.get_mut(&name) {
                        Some(slot) => match slot {
                            Value::Ref(_) => slot.clone(),
                            _ => std::mem::replace(slot, Value::Null),
                        },
                        None => Value::Null,
                    });
                    self.set(base, dst, taken);
                }
                Op::RefElem { dst, arr, key } => {
                    let k = key.map(|k| self.rd(base, k));
                    let cell = self.elem_ref(base, arr, k.as_ref())?;
                    self.rebind(base, dst, cell);
                }
                Op::RefProp { dst, obj, name } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    let holder = self.prop_holder(&o, &name)?;
                    let name = self.prop_storage_key(&holder, &name);
                    self.check_prop_access(holder.class_id(), &name)?;
                    self.check_indirect_modify(&holder, &name)?;
                    if holder.get(&name).is_none() {
                        self.dynamic_prop_notice(&holder, &name)?;
                    }
                    let cell = holder.prop_ref(&name);
                    self.rebind(base, dst, cell);
                }
                Op::AssignRefElem { arr, key, src } => {
                    let cell = self.make_ref(base, src);
                    let k = key.map(|k| self.rd(base, k));
                    self.with_slot(base, arr, |slot| {
                        if matches!(slot, Value::Null | Value::Uninit) {
                            *slot = Value::empty_array();
                        }
                        match slot {
                            Value::Array(a) => match &k {
                                Some(k) => match array_key(k) {
                                    Some(k) => {
                                        a.set_ref(k, cell);
                                        Ok(())
                                    }
                                    None => Err(Unwind::type_error(format!(
                                        "Cannot access offset of type {} on array",
                                        value_name(k)
                                    ))),
                                },
                                None => {
                                    a.push_ref(cell);
                                    Ok(())
                                }
                            },
                            _ => Err(Unwind::error("Cannot use a scalar value as an array")),
                        }
                    })?;
                }
                Op::AssignRefProp { obj, name, src } => {
                    let cell = self.make_ref(base, src);
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    let holder = self.prop_holder(&o, &name)?;
                    self.check_prop_access(holder.class_id(), &name)?;
                    holder.with_data_mut(|d| match d.layout().slot_of(&name) {
                        Some(i) => d.slots_mut()[usize::from(i)] = Value::Ref(cell),
                        None => d.dyn_props_mut().set_ref(&name, cell),
                    });
                }
                Op::UnsetVar { var } => {
                    self.set(base, var, Value::Uninit);
                    let f = &self.frames[fi];
                    if let Some(symtab) = f.symtab.clone() {
                        if let Some(name) = func.reg_name(var) {
                            symtab.with_mut(|t| t.remove(name));
                        }
                    }
                }
                Op::UnsetElem { arr, key } => {
                    let k = self.rd(base, key);
                    // E6: `unset($o[$k])` is `offsetUnset($k)`.
                    let container = self.rd(base, arr);
                    if let Value::Object(o) = &*container.deref() {
                        if self.is_array_access(o) {
                            let o = o.clone();
                            self.offset_unset(&o, &k)?;
                            pc += 1;
                            continue;
                        }
                    }
                    self.with_slot(base, arr, |slot| match slot {
                        Value::Array(a) => {
                            if let Some(k) = array_key(&k) {
                                a.unset(&k);
                            }
                            Ok(())
                        }
                        Value::Null | Value::Uninit => Ok(()),
                        Value::Str(_) => Err(Unwind::error("Cannot unset string offsets")),
                        Value::Object(_) | Value::Closure(_) => Err(Unwind::error(
                            "Cannot use object as array",
                        )),
                        _ => Err(Unwind::error("Cannot unset offset in a non-array variable")),
                    })?;
                }
                Op::UnsetProp { obj, name } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    self.unset_prop(&o, &name)?;
                }
                Op::IssetVar { dst, var } => {
                    let set = !matches!(self.rd(base, var), Value::Null | Value::Uninit);
                    self.set(base, dst, Value::Bool(set));
                }
                Op::IssetElem { dst, arr, key } => {
                    let container = self.rd(base, arr);
                    let k = self.rd(base, key);
                    let set = match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            self.offset_exists(&o, &k)?
                        }
                        _ => self.isset_elem(&container, &k),
                    };
                    self.set(base, dst, Value::Bool(set));
                }
                Op::IssetProp { dst, obj, name } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    let set = self.isset_prop(&o, &name)?;
                    self.set(base, dst, Value::Bool(set));
                }
                Op::IssetStaticProp { dst, class, name } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let n = self.member_name(&func, base, name)?;
                    let scope = self.frames[fi].scope;
                    let set = self.isset_static_prop(cid, &n, scope);
                    self.set(base, dst, Value::Bool(set));
                }
                Op::EmptyVar { dst, var } => {
                    let v = self.rd(base, var);
                    self.set(base, dst, Value::Bool(!v.to_bool()));
                }
                Op::EmptyElem { dst, arr, key } => {
                    let container = self.rd(base, arr);
                    let k = self.rd(base, key);
                    let empty = match &*container.deref() {
                        Value::Object(o) if self.is_array_access(o) => {
                            let o = o.clone();
                            self.offset_empty(&o, &k)?
                        }
                        _ => !self.array_get_quiet(&container, &k).to_bool(),
                    };
                    self.set(base, dst, Value::Bool(empty));
                }
                Op::EmptyProp { dst, obj, name } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    // `empty_prop` already returns the *empty* answer.
                    let empty = self.empty_prop(&o, &name)?;
                    self.set(base, dst, Value::Bool(empty));
                }

                // --- iteration ---
                Op::IterInit { it, src, by_ref } => {
                    let state = if by_ref {
                        match self.rd(base, src) {
                            Value::Array(_) | Value::Null | Value::Uninit => {
                                let cell = self.make_ref(base, src);
                                cell.update(|v| {
                                    if matches!(v, Value::Null | Value::Uninit) {
                                        *v = Value::empty_array();
                                    }
                                });
                                IterState::ByRef { cell, pos: 0 }
                            }
                            Value::Object(o) => self.object_iter_state(&o, true)?,
                            v => {
                                self.warn(&format!(
                                    "foreach() argument must be of type array|object, {} given",
                                    value_name(&v)
                                ))?;
                                IterState::Empty
                            }
                        }
                    } else {
                        match self.rd(base, src) {
                            Value::Array(arr) => IterState::Array { arr, pos: 0 },
                            Value::Object(o) => self.object_iter_state(&o, false)?,
                            other => {
                                self.warn(&format!(
                                    "foreach() argument must be of type array|object, {} given",
                                    value_name(&other)
                                ))?;
                                IterState::Empty
                            }
                        }
                    };
                    let f = &mut self.frames[fi];
                    f.iters.retain(|(r, _)| *r != it);
                    f.iters.push((it, state));
                }
                Op::IterNext { it, key, val, target } => {
                    let Some(idx) = self.frames[fi].iters.iter().position(|(r, _)| *r == it) else {
                        pc = target as usize;
                        continue;
                    };
                    // An `Iterator` object drives user code, which needs the
                    // interpreter, so it is stepped outside the frame borrow.
                    let iter_obj = match &self.frames[fi].iters[idx].1 {
                        IterState::ByRef { cell, pos } => {
                            let o = match &*cell.borrow() {
                                Value::Object(o) => Some(o.clone()),
                                _ => None,
                            };
                            o.map(|o| (o, *pos))
                        }
                        _ => None,
                    };
                    let step = if let Some((o, pos)) = iter_obj {
                        let step = self.iterator_step(&o, pos, key.is_some())?;
                        if let IterState::ByRef { pos, .. } = &mut self.frames[fi].iters[idx].1 {
                            *pos += 1;
                        }
                        step
                    } else {
                        let f = &mut self.frames[fi];
                        match &mut f.iters[idx].1 {
                            IterState::Empty => None,
                            IterState::Array { arr, pos } => match arr.next_live_from(*pos) {
                                Some((raw, k, v)) => {
                                    *pos = raw + 1;
                                    Some((k.to_value(), v.deref().into_owned(), None))
                                }
                                None => None,
                            },
                            IterState::ByRef { cell, pos } => {
                                let p = *pos;
                                let r = cell.update(|v| match v {
                                    Value::Array(a) => match a.next_live_from(p) {
                                        Some((raw, k, _)) => {
                                            let k = k.clone();
                                            let elem = a.get_ref(k.clone());
                                            Some((raw, k.to_value(), elem))
                                        }
                                        None => None,
                                    },
                                    _ => None,
                                });
                                match r {
                                    Some((raw, k, elem)) => {
                                        *pos = raw + 1;
                                        Some((k, Value::Null, Some(elem)))
                                    }
                                    None => None,
                                }
                            }
                        }
                    };
                    match step {
                        None => {
                            pc = target as usize;
                            continue;
                        }
                        Some((k, v, elem)) => {
                            match elem {
                                Some(cell) => self.rebind(base, val, cell),
                                None => Value::assign(&mut self.stack[base + val as usize], v),
                            }
                            if let Some(kr) = key {
                                Value::assign(&mut self.stack[base + kr as usize], k);
                            }
                        }
                    }
                }
                Op::IterFree { it } => {
                    self.frames[fi].iters.retain(|(r, _)| *r != it);
                }

                // --- comparison ---
                Op::CmpEq { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Bool(l.loose_eq(&r)));
                }
                Op::CmpNe { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Bool(!l.loose_eq(&r)));
                }
                Op::CmpIdentical { dst, a, b } => {
                    let r = self.rd(base, a).identical(&self.rd(base, b));
                    self.set(base, dst, Value::Bool(r));
                }
                Op::CmpNotIdentical { dst, a, b } => {
                    let r = !self.rd(base, a).identical(&self.rd(base, b));
                    self.set(base, dst, Value::Bool(r));
                }
                Op::CmpLt { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Bool(l.lt(&r)));
                }
                Op::CmpLe { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Bool(l.le(&r)));
                }
                Op::CmpGt { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Bool(l.gt(&r)));
                }
                Op::CmpGe { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Bool(l.ge(&r)));
                }
                Op::Spaceship { dst, a, b } => {
                    let (l, r) = self.cmp_operands(self.rd(base, a), self.rd(base, b))?;
                    self.set(base, dst, Value::Int(l.spaceship(&r)));
                }

                // --- control flow ---
                Op::Jmp { target } => {
                    pc = target as usize;
                    continue;
                }
                Op::JmpIfTrue { cond, target } => {
                    if self.raw(base, cond).to_bool() {
                        pc = target as usize;
                        continue;
                    }
                }
                Op::JmpIfFalse { cond, target } => {
                    if !self.raw(base, cond).to_bool() {
                        pc = target as usize;
                        continue;
                    }
                }
                Op::JmpUnlessArgByRef { pos, target } => {
                    if !self.pending_by_ref(fi, pos as usize) {
                        pc = target as usize;
                        continue;
                    }
                }
                Op::Switch {
                    src,
                    table,
                    default,
                    strict,
                } => {
                    let v = self.rd(base, src);
                    let rows = func.f.consts[table as usize]
                        .as_jump_table()
                        .expect("Switch table");
                    let hit = rows.iter().find(|(k, _)| {
                        if strict {
                            v.identical(k)
                        } else {
                            v.loose_eq(k)
                        }
                    });
                    pc = hit.map_or(default, |(_, t)| *t) as usize;
                    continue;
                }
                Op::MatchError { src } => {
                    let v = self.rd(base, src);
                    return Err(Unwind::unhandled_match_error(self.match_error_message(&v)));
                }
                Op::Silence { begin } => {
                    if begin {
                        self.silence += 1;
                    } else {
                        self.silence = self.silence.saturating_sub(1);
                    }
                }
                Op::Exit { src } => {
                    let code = match src {
                        None => 0,
                        Some(r) => match self.rd(base, r) {
                            Value::Int(i) => i as i32,
                            other => {
                                self.check_array_to_string(&other)?;
                                self.echo_value(&other);
                                0
                            }
                        },
                    };
                    return Err(Unwind::Exit(code));
                }
                Op::Throw { src } => {
                    return match self.rd(base, src) {
                        Value::Object(o) => {
                            if self.well_known.throwable.is_some() && !self.is_throwable(&o) {
                                Err(Unwind::error(
                                    "Cannot throw objects that do not implement Throwable",
                                ))
                            } else {
                                Err(Unwind::Throw(o))
                            }
                        }
                        _ => Err(Unwind::error("Can only throw objects")),
                    };
                }

                // --- calls ---
                Op::InitFCall { name, ns_fallback, ic } => {
                    let target = self.resolve_fcall(&func, name, ns_fallback, ic)?;
                    let name = self.name_bytes(&func, name);
                    let args_base = self.stack.len();
                    self.frames[fi].pending.push(PendingCall {
                        target,
                        this: None,
                        scope: None,
                        static_class: None,
                        args_base,
                        argc: 0,
                        named: Vec::new(),
                        new_obj: None,
                        name,
                    });
                }
                Op::InitDynCall { callee, .. } => {
                    let v = self.rd(base, callee);
                    self.autoload_callable(&v)?;
                    let c = self.resolve_callable(&v)?;
                    let (target, this, scope, static_class, name) = match c {
                        crate::call::Callable::Native(id) => (
                            CallTarget::Native(id),
                            None,
                            None,
                            None,
                            Box::from(self.natives[id.0 as usize].name.as_bytes()),
                        ),
                        crate::call::Callable::NativeMethod { method, this } => {
                            let name = method.name.clone();
                            let decl = method.decl;
                            let static_class = this.as_ref().map(|o| o.class_id()).or(Some(decl));
                            (CallTarget::NativeMethod(method), this, Some(decl), static_class, name)
                        }
                        crate::call::Callable::User {
                            func: f,
                            this,
                            scope,
                            static_class,
                            closure,
                        } => {
                            let name = f.f.name_bytes.clone();
                            (CallTarget::User { func: f, closure }, this, scope, static_class, name)
                        }
                    };
                    let args_base = self.stack.len();
                    self.frames[fi].pending.push(PendingCall {
                        target,
                        this,
                        scope,
                        static_class,
                        args_base,
                        argc: 0,
                        named: Vec::new(),
                        new_obj: None,
                        name,
                    });
                }
                Op::InitMethodCall { obj, name, .. } => {
                    let o = self.rd(base, obj);
                    let mname = self.member_name(&func, base, name)?;
                    // `methods.rs` owns dispatch: virtual lookup, visibility,
                    // `__call`, the private-shadowing retry and the `Closure`
                    // receiver (`bindTo`/`call`/`__invoke`).
                    self.init_method_call(fi, o, mname)?;
                }
                Op::InitStaticCall { class, name, .. } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let mname = self.member_name(&func, base, name)?;
                    // php's forwarding-call rule: `self::`/`parent::`/`static::`
                    // keep the *called* scope (late static binding), a named
                    // class does not.
                    let forwarding = matches!(
                        class.kind(),
                        ClassRefKind::SelfKw | ClassRefKind::Parent | ClassRefKind::Static
                    );
                    self.init_static_call(fi, cid, mname, forwarding)?;
                }
                Op::InitNew { class, .. } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let obj = self.new_object(cid)?;
                    let (target, scope) = match self.resolve_method(cid, b"__construct") {
                        Some(m) => {
                            let scope = self.frames[fi].scope;
                            if !self.access_ok(m.vis, m.decl, scope) {
                                return Err(Unwind::error(format!(
                                    "Call to {} {}::__construct() from {}",
                                    vis_word(m.vis),
                                    String::from_utf8_lossy(&self.classes[m.decl as usize].name),
                                    match scope {
                                        Some(c) => format!("scope {}", String::from_utf8_lossy(&self.classes[c as usize].name)),
                                        None => "global scope".to_string(),
                                    }
                                )));
                            }
                            let target = match &m.body {
                                MethodBody::User(f) => CallTarget::User {
                                    func: f.clone(),
                                    closure: None,
                                },
                                MethodBody::Native(_) => CallTarget::NativeMethod(m.clone()),
                            };
                            (target, Some(m.decl))
                        }
                        None => (CallTarget::NoCtor, None),
                    };
                    let args_base = self.stack.len();
                    let name = self.classes[cid as usize].name.clone();
                    self.frames[fi].pending.push(PendingCall {
                        target,
                        this: Some(obj.clone()),
                        scope,
                        static_class: Some(cid),
                        args_base,
                        argc: 0,
                        named: Vec::new(),
                        new_obj: Some(obj),
                        name,
                    });
                }
                Op::SendVal { pos, src } => {
                    let v = self.rd(base, src);
                    let by_ref = self.pending_by_ref(fi, pos as usize);
                    if by_ref {
                        let msg = self.by_ref_message(fi, pos as usize);
                        return Err(Unwind::error(msg));
                    }
                    self.send(fi, v);
                }
                Op::SendVar { pos, var } => {
                    let v = if self.pending_by_ref(fi, pos as usize) {
                        // A by-reference parameter *creates* the variable, so
                        // php says nothing about it being undefined.
                        Value::Ref(self.make_ref(base, var))
                    } else {
                        self.warn_if_undefined(&func, base, var)?;
                        self.rd(base, var)
                    };
                    self.send(fi, v);
                }
                Op::SendFuncResult { pos, src } => {
                    let v = if self.pending_by_ref(fi, pos as usize) {
                        self.notice("Only variables should be passed by reference")?;
                        Value::Ref(PhpRef::new(self.rd(base, src)))
                    } else {
                        self.rd(base, src)
                    };
                    self.send(fi, v);
                }
                Op::SendRefElem { pos, arr, key } => {
                    if !self.pending_by_ref(fi, pos as usize) {
                        self.warn_if_undefined(&func, base, arr)?;
                    }
                    let k = self.rd(base, key);
                    let v = if self.pending_by_ref(fi, pos as usize) {
                        Value::Ref(self.elem_ref(base, arr, Some(&k))?)
                    } else {
                        let container = self.rd(base, arr);
                        // `f($o[$k])` reads through `ArrayAccess` like any
                        // other element read (`Op::ArrayGet`).
                        match &*container.deref() {
                            Value::Object(o) if self.is_array_access(o) => {
                                let o = o.clone();
                                self.offset_get(&o, &k)?
                            }
                            _ => self.array_get(&container, &k)?,
                        }
                    };
                    self.send(fi, v);
                }
                Op::SendRefProp { pos, obj, name } => {
                    let o = self.rd(base, obj);
                    let name = self.member_name(&func, base, name)?;
                    let v = if self.pending_by_ref(fi, pos as usize) {
                        let holder = self.prop_holder(&o, &name)?;
                        let name = self.prop_storage_key(&holder, &name);
                        self.check_prop_access(holder.class_id(), &name)?;
                        self.check_indirect_modify(&holder, &name)?;
                        if holder.get(&name).is_none() {
                            self.dynamic_prop_notice(&holder, &name)?;
                        }
                        Value::Ref(holder.prop_ref(&name))
                    } else {
                        self.fetch_prop(&o, &name)?
                    };
                    self.send(fi, v);
                }
                Op::SendUnpack { src } => {
                    let v = self.rd(base, src);
                    let Value::Array(a) = v else {
                        return Err(Unwind::error("Only arrays and Traversables can be unpacked"));
                    };
                    for (k, val) in a.iter() {
                        match k {
                            rphp_value::ArrayKey::Int(_) => {
                                if !self.frames[fi].pending.last().expect("pending").named.is_empty() {
                                    return Err(Unwind::error(
                                        "Cannot use positional argument after named argument during unpacking",
                                    ));
                                }
                                let pos = self.frames[fi].pending.last().expect("pending").argc;
                                let v = if self.pending_by_ref(fi, pos) {
                                    val.clone()
                                } else {
                                    val.deref().into_owned()
                                };
                                self.send(fi, v);
                            }
                            rphp_value::ArrayKey::Str(s) => {
                                let p = self.frames[fi].pending.last_mut().expect("pending");
                                p.named.push((s.clone(), val.deref().into_owned()));
                            }
                        }
                    }
                }
                Op::SendNamed { name, src } => {
                    let nm = self.name_bytes(&func, name);
                    // A by-reference parameter shares the register's cell.
                    let by_ref = {
                        let p = self.frames[fi].pending.last().expect("pending");
                        match &p.target {
                            CallTarget::User { func: f, .. } => f
                                .f
                                .params
                                .iter()
                                .find(|p| p.name.as_ref() == nm.as_ref())
                                .is_some_and(|p| p.by_ref),
                            // A native's names come from the arginfo table,
                            // its by-reference mask from its row.
                            CallTarget::Native(id) => {
                                let f = self.natives[id.0 as usize];
                                crate::native_args::params_of(&f.name.to_ascii_lowercase())
                                    .and_then(|ps| ps.iter().position(|(n, _, _)| n.as_bytes() == nm.as_ref()))
                                    .is_some_and(|i| f.is_by_ref(i))
                            }
                            CallTarget::NativeMethod(m) => match &m.body {
                                MethodBody::Native(nm_def) => {
                                    let key = format!(
                                        "{}::{}",
                                        self.classes[m.decl as usize].name_str().to_ascii_lowercase(),
                                        String::from_utf8_lossy(&m.name).to_ascii_lowercase()
                                    );
                                    crate::native_args::params_of(&key)
                                        .and_then(|ps| ps.iter().position(|(n, _, _)| n.as_bytes() == nm.as_ref()))
                                        .is_some_and(|i| nm_def.is_by_ref(i))
                                }
                                MethodBody::User(f) => f
                                    .f
                                    .params
                                    .iter()
                                    .find(|p| p.name.as_ref() == nm.as_ref())
                                    .is_some_and(|p| p.by_ref),
                            },
                            _ => false,
                        }
                    };
                    let v = if by_ref {
                        Value::Ref(self.make_ref(base, src))
                    } else {
                        self.rd(base, src)
                    };
                    self.frames[fi].pending.last_mut().expect("pending").named.push((nm, v));
                }
                Op::DoCall { dst } => {
                    let pending = self.frames[fi].pending.pop().expect("DoCall without Init");
                    let abs = base + dst as usize;
                    match pending.target.clone() {
                        CallTarget::Native(id) => {
                            let PendingCall {
                                args_base,
                                argc,
                                named,
                                ..
                            } = pending;
                            let r = self.call_native_window(id, args_base, argc, named)?;
                            self.stack[abs] = r;
                        }
                        CallTarget::NativeMethod(m) => {
                            let PendingCall {
                                args_base,
                                argc,
                                named,
                                this,
                                new_obj,
                                ..
                            } = pending;
                            let r = self.call_native_method_window(m, this, args_base, argc, named)?;
                            // `Fiber::suspend()`: the chain from the fiber's
                            // boundary up to this frame leaves the stacks,
                            // and the loop that `start()`/`resume()` entered
                            // ends here (`fiber.rs`). The resumed value lands
                            // in `dst` when the chain comes back.
                            if let Some(id) = self.fiber_suspending.take() {
                                self.park_fiber(id, dst);
                                return Ok(Switch::Done(Value::Null));
                            }
                            self.stack[abs] = match new_obj {
                                Some(obj) => Value::Object(obj),
                                None => r,
                            };
                        }
                        CallTarget::NoCtor => {
                            self.stack.truncate(pending.args_base);
                            let obj = pending.new_obj.expect("NoCtor carries the object");
                            self.stack[abs] = Value::Object(obj);
                        }
                        CallTarget::User { func: callee, closure } => {
                            let ret = match pending.new_obj.clone() {
                                Some(obj) => RetTarget::New { reg: abs, obj },
                                None => RetTarget::Reg(abs),
                            };
                            let symtab = if callee.f.flags.contains(FnFlags::NEEDS_SYMTAB) {
                                Some(Symtab::new())
                            } else {
                                None
                            };
                            // The frame keeps the call op as its pc (traces
                            // and region lookups use it); `do_return`
                            // advances it when the callee returns.
                            self.activate(pending, FrameKind::Normal, ret, symtab, closure)?;
                            return Ok(Switch::Continue);
                        }
                    }
                }
                Op::MakeCallableClosure { dst, .. } => {
                    let v = self.make_callable_closure(&func, base, &op)?;
                    self.set(base, dst, v);
                }
                Op::MakeClosure { dst, proto } => {
                    let fid = func.unit.func_id(proto);
                    let cf = self.funcs[fid as usize].clone();
                    let mut captures = Vec::with_capacity(cf.f.captures.len() + 3);
                    for d in &cf.f.captures {
                        if d.by_ref {
                            captures.push(Value::Ref(self.make_ref(base, d.src)));
                        } else {
                            captures.push(self.rd(base, d.src));
                        }
                    }
                    let f = &self.frames[fi];
                    let this = if cf.f.flags.contains(FnFlags::STATIC) {
                        None
                    } else {
                        f.this.clone()
                    };
                    let called = f.static_class.or(f.scope);
                    captures.push(this.map_or(Value::Null, Value::Object));
                    captures.push(f.scope.map_or(Value::Null, |c| Value::Int(i64::from(c))));
                    // Third tail slot: the late-static-bound class, so
                    // `static::` inside a closure declared in a static method
                    // still means the called class (`methods.rs`).
                    captures.push(called.map_or(Value::Null, |c| Value::Int(i64::from(c))));
                    self.set(base, dst, Value::Closure(Closure::new(fid, captures)));
                }
                Op::Ret { src } => {
                    // A generator body that falls off the end finishes the
                    // generator; it does not return to a caller.
                    if let Some(gid) = self.frames[fi].generator {
                        self.finish_generator(gid, Value::Null);
                        return Ok(Switch::Done(Value::Null));
                    }
                    let v = src.map(|r| self.rd(base, r));
                    let v = if func.f.ret_ty.is_some() {
                        let strict = self.frames[fi].strict;
                        self.verify_return(&func, v, strict)?
                    } else {
                        v.unwrap_or(Value::Null)
                    };
                    return self.do_return(v, stop_depth);
                }
                Op::RetRef { var } => {
                    // `function &f() { return $place; }`: the caller receives
                    // the cell itself, which `$x = &f()` binds to and a
                    // by-value use derefs. The declared type is checked on
                    // what the cell holds; a coercion never lands in it.
                    let cell = self.make_ref(base, var);
                    if func.f.ret_ty.is_some() {
                        let strict = self.frames[fi].strict;
                        self.verify_return(&func, Some(cell.get()), strict)?;
                    }
                    return self.do_return(Value::Ref(cell), stop_depth);
                }
                Op::RetRefTemp { src } => {
                    self.notice("Only variable references should be returned by reference")?;
                    let v = src.map_or(Value::Null, |r| self.rd(base, r));
                    let v = if func.f.ret_ty.is_some() {
                        let strict = self.frames[fi].strict;
                        self.verify_return(&func, Some(v), strict)?
                    } else {
                        v
                    };
                    return self.do_return(Value::Ref(PhpRef::new(v)), stop_depth);
                }

                // --- prologue ---
                Op::RecvInit { param, init } => {
                    let f = &self.frames[fi];
                    let slot = &self.stack[base + param as usize];
                    let missing = f.argc <= param as usize || slot.is_uninit();
                    if missing {
                        let v = match init {
                            InitRef::Const(k) => self.const_value(&func, k),
                            InitRef::Thunk(t) => {
                                let fid = func.unit.func_id(t);
                                let (this, scope) = {
                                    let f = &self.frames[fi];
                                    (f.this.clone(), f.scope)
                                };
                                self.run_thunk(fid, this, scope)?
                            }
                        };
                        let slot = &mut self.stack[base + param as usize];
                        if slot.is_uninit() {
                            *slot = v;
                        } else {
                            Value::assign(slot, v);
                        }
                    }
                }
                Op::RecvVariadic { reg } => {
                    let f = &mut self.frames[fi];
                    let mut arr = rphp_value::Array::new();
                    // The extras stay on the frame for `func_get_args()`; a
                    // by-reference variadic collects the cells themselves.
                    for v in &f.extra_args {
                        match v {
                            Value::Ref(r) => arr.push_ref(r.clone()),
                            other => arr.push(other.clone()),
                        }
                    }
                    for (k, v) in &f.extra_named {
                        arr.set(rphp_value::ArrayKey::str(k), v.clone());
                    }
                    Value::assign(&mut self.stack[base + reg as usize], Value::Array(arr));
                }
                Op::BindStatic { reg, idx } => {
                    // A closure frame keeps its own table (`frame.statics`).
                    let table = self.frames[fi].statics.clone();
                    let existing = match &table {
                        Some(t) => t.borrow()[idx as usize].clone(),
                        None => func.statics.borrow()[idx as usize].clone(),
                    };
                    let cell = match existing {
                        Some(c) => c,
                        None => {
                            let sv = &func.f.statics[idx as usize];
                            let v = match sv.init {
                                None => Value::Null,
                                Some(InitRef::Const(k)) => self.const_value(&func, k),
                                Some(InitRef::Thunk(t)) => {
                                    let fid = func.unit.func_id(t);
                                    let (this, scope) = {
                                        let f = &self.frames[fi];
                                        (f.this.clone(), f.scope)
                                    };
                                    self.run_thunk(fid, this, scope)?
                                }
                            };
                            // A recursive call inside the initializer may
                            // have created the cell meanwhile: php keeps
                            // that one (BIND_INIT_STATIC_OR_JMP).
                            let existing = match &table {
                                Some(t) => t.borrow()[idx as usize].clone(),
                                None => func.statics.borrow()[idx as usize].clone(),
                            };
                            match existing {
                                Some(c) => c,
                                None => {
                                    let c = PhpRef::new(v);
                                    match &table {
                                        Some(t) => t.borrow_mut()[idx as usize] = Some(c.clone()),
                                        None => {
                                            func.statics.borrow_mut()[idx as usize] = Some(c.clone())
                                        }
                                    }
                                    c
                                }
                            }
                        }
                    };
                    self.rebind(base, reg, cell);
                }
                Op::BindStaticOrJmp { reg, idx, target } => {
                    let table = self.frames[fi].statics.clone();
                    let existing = match &table {
                        Some(t) => t.borrow()[idx as usize].clone(),
                        None => func.statics.borrow()[idx as usize].clone(),
                    };
                    if let Some(cell) = existing {
                        self.rebind(base, reg, cell);
                        pc = target as usize;
                        continue;
                    }
                }
                Op::CheckVar { reg, name } => {
                    if self.raw(base, reg).deref().is_uninit() {
                        let name = self.name_bytes(&func, name);
                        self.warn(&format!(
                            "Undefined variable ${}",
                            String::from_utf8_lossy(&name)
                        ))?;
                    }
                }
                Op::BindGlobal { reg, name } => {
                    let name = self.name_bytes(&func, name);
                    let cell = self.auto_global_cell(&name);
                    self.rebind(base, reg, cell);
                }
                Op::BindSymtab => {
                    let symtab = match self.frames[fi].symtab.clone() {
                        Some(t) => t,
                        None => {
                            let t = Symtab::new();
                            self.frames[fi].symtab = Some(t.clone());
                            t
                        }
                    };
                    for (name, reg) in &func.f.var_names {
                        let slot = &mut self.stack[base + *reg as usize];
                        match symtab.get(name) {
                            Some(cell) => *slot = Value::Ref(cell),
                            None => {
                                // A variable that has not been assigned stays
                                // *uninitialized*: php's symbol table has no
                                // entry for it at all, so `$GLOBALS`,
                                // `get_defined_vars()` and `isset()` must not
                                // see it. The cell is bound so a later write
                                // lands in the table.
                                let cell = Value::make_ref(slot);
                                symtab.insert(name, cell);
                            }
                        }
                    }
                }
                Op::FetchDynVar { dst, name, global } => {
                    let n = self.rd(base, name).to_php_bytes();
                    let table = if global {
                        Some(self.globals.clone())
                    } else {
                        self.frames[fi].symtab.clone()
                    };
                    let v = match table.and_then(|t| t.get(&n)) {
                        Some(cell) => cell.get(),
                        None => {
                            self.warn(&format!(
                                "Undefined {}variable ${}",
                                if global { "global " } else { "" },
                                String::from_utf8_lossy(&n)
                            ))?;
                            Value::Null
                        }
                    };
                    self.set(base, dst, v);
                }
                Op::BindDynVar { reg, name, global } => {
                    let n = self.rd(base, name).to_php_bytes();
                    let table = if global {
                        self.globals.clone()
                    } else {
                        match self.frames[fi].symtab.clone() {
                            Some(t) => t,
                            None => {
                                let t = Symtab::new();
                                self.frames[fi].symtab = Some(t.clone());
                                t
                            }
                        }
                    };
                    let cell = table.get_or_create(&n);
                    self.set(base, reg, Value::Ref(cell));
                }

                // --- names ---
                Op::FetchConst { dst, name, ns_fallback } => {
                    let n = self.name_bytes(&func, name);
                    // Like an unqualified call, an unqualified *constant*
                    // inside a namespace falls back to the global one, which
                    // is how `DIRECTORY_SEPARATOR` resolves inside
                    // `namespace Composer\Autoload`.
                    let (v, found) = match self.constants.get(&n).cloned() {
                        Some(v) => (v, n),
                        None => {
                            let g = ns_fallback.map(|k| self.name_bytes(&func, k));
                            match g.as_ref().and_then(|g| self.constants.get(g).cloned()) {
                                Some(v) => (v, g.expect("looked up through it")),
                                None => {
                                    return Err(Unwind::error(format!(
                                        "Undefined constant \"{}\"",
                                        String::from_utf8_lossy(&n)
                                    )))
                                }
                            }
                        }
                    };
                    if !self.deprecated_constants.is_empty() {
                        if let Some(note) = self.deprecated_constants.get(&found).copied() {
                            self.deprecated(&format!(
                                "Constant {} is deprecated{note}",
                                String::from_utf8_lossy(&found)
                            ))?;
                        }
                    }
                    self.set(base, dst, v);
                }
                Op::DeclareConst { name, src } => {
                    let n = self.name_bytes(&func, name);
                    let v = self.rd(base, src);
                    if !self.define(&n, v) {
                        self.warn(&format!(
                            "Constant {} already defined, this will be an error in PHP 9",
                            String::from_utf8_lossy(&n)
                        ))?;
                    }
                }
                Op::FetchClassConst { dst, class, name, .. } => {
                    let n = self.member_name(&func, base, name)?;
                    if n.as_ref() == b"class" {
                        // `X::class` never looks the class up: a name yields
                        // itself (`Nope::class` is "Nope"), a register must
                        // hold an object.
                        let v = match class.kind() {
                            ClassRefKind::Named(k) => Value::string(&self.name_bytes(&func, k)),
                            ClassRefKind::Reg(r) => match self.rd(base, r) {
                                Value::Object(o) => {
                                    Value::string(&self.classes[o.class_id() as usize].name)
                                }
                                other => {
                                    return Err(Unwind::type_error(format!(
                                        "Cannot use \"::class\" on {}",
                                        value_name(&other)
                                    )))
                                }
                            },
                            _ => {
                                let cid = self.resolve_class_ref(&func, base, class)?;
                                Value::string(&self.classes[cid as usize].name)
                            }
                        };
                        self.set(base, dst, v);
                    } else {
                        let cid = self.resolve_class_ref(&func, base, class)?;
                        // `statics.rs` owns constants, `enums.rs` owns cases.
                        let scope = self.frames[fi].scope;
                        let v = if self.classes[cid as usize].enum_index.contains_key(&n[..]) {
                            self.enum_case(cid, &n)?
                        } else {
                            // php's self-referencing-constant error quotes the
                            // reference as written (`self::X`), so the
                            // spelling travels with the lookup.
                            let spelling = self.class_ref_spelling(&func, cid, class);
                            self.class_const_spelled(cid, &n, scope, &spelling)?
                        };
                        self.set(base, dst, v);
                    }
                }
                Op::FetchStaticProp { dst, class, name, .. } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let n = self.member_name(&func, base, name)?;
                    let scope = self.frames[fi].scope;
                    let v = self.fetch_static_prop(cid, &n, scope)?;
                    self.set(base, dst, v);
                }
                Op::AssignStaticProp { class, name, src } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let n = self.member_name(&func, base, name)?;
                    let scope = self.frames[fi].scope;
                    let strict = self.frames[fi].strict;
                    let v = self.rd(base, src);
                    self.assign_static_prop(cid, &n, scope, v, strict)?;
                }
                Op::RefStaticProp { dst, class, name } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let n = self.member_name(&func, base, name)?;
                    let scope = self.frames[fi].scope;
                    // The shared cell itself becomes the reference, so
                    // `$r = &A::$p; $r = 5;` is visible as `A::$p`.
                    let cell = self.ref_static_prop(cid, &n, scope)?;
                    self.set(base, dst, Value::Ref(cell));
                }
                Op::ArrayUnpack { arr, src } => {
                    let v = self.rd(base, src);
                    // Integer keys are renumbered, string keys preserved.
                    let items: Vec<(Value, Value)> = match &*v.deref() {
                        Value::Array(a) => a
                            .iter()
                            .map(|(k, val)| (k.to_value(), val.deref().into_owned()))
                            .collect(),
                        // Only a `Traversable` unpacks; a plain object is a
                        // TypeError naming its class, as php has it.
                        Value::Object(o)
                            if self.generator_id(o).is_some()
                                || self
                                    .well_known
                                    .traversable
                                    .is_some_and(|t| self.object_instanceof(o, t)) =>
                        {
                            let o = o.clone();
                            self.iterate_traversable(&o)?
                        }
                        Value::Object(o) => {
                            let name = self.class_name_of(o);
                            return Err(Unwind::type_error(format!(
                                "Only arrays and Traversables can be unpacked, {name} given"
                            )));
                        }
                        other => {
                            return Err(Unwind::type_error(format!(
                                "Only arrays and Traversables can be unpacked, {} given",
                                other.type_name()
                            )))
                        }
                    };
                    self.with_slot(base, arr, |slot| {
                        let Value::Array(a) = slot else {
                            return Err(Unwind::error("internal error: unpack into a non-array"));
                        };
                        for (k, val) in items {
                            match k {
                                Value::Str(_) => match rphp_value::array_key(&k) {
                                    Some(key) => a.set(key, val),
                                    None => a.push(val),
                                },
                                _ => a.push(val),
                            }
                        }
                        Ok(())
                    })?;
                }
                Op::AssignRefStaticProp { class, name, src } => {
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let n = self.member_name(&func, base, name)?;
                    let scope = self.frames[fi].scope;
                    // The property's slot takes the *caller's* reference cell,
                    // so both names share one value from here on.
                    let r = self.make_ref(base, src);
                    self.bind_static_prop_ref(cid, &n, scope, r)?;
                }
                Op::UnsetStaticProp { class, name } => {
                    // php never removes a static property; it throws, naming
                    // the resolved class and the property as written.
                    let cid = self.resolve_class_ref(&func, base, class)?;
                    let n = self.member_name(&func, base, name)?;
                    return Err(Unwind::error(format!(
                        "Attempt to unset static property {}::${}",
                        self.classes[cid as usize].name_str(),
                        String::from_utf8_lossy(&n)
                    )));
                }
                Op::FetchProp { dst, obj, name, .. } => {
                    let o = self.rd(base, obj);
                    let n = self.member_name(&func, base, name)?;
                    let v = self.fetch_prop(&o, &n)?;
                    self.set(base, dst, v);
                }
                Op::AssignProp { obj, name, src, .. } => {
                    let o = self.rd(base, obj);
                    let n = self.member_name(&func, base, name)?;
                    let v = self.rd(base, src);
                    self.assign_prop(&o, &n, v)?;
                }
                Op::FetchClass { dst, name, .. } => {
                    let n = self.name_bytes(&func, name);
                    let cid = self.lookup_class_or_error(&n)?;
                    let v = Value::string(&self.classes[cid as usize].name);
                    self.set(base, dst, v);
                }
                Op::FetchGlobals { dst } => {
                    let arr = self.globals.with(|t| t.to_array());
                    self.set(base, dst, Value::Array(arr));
                }
                Op::AssignGlobal { key, src } => {
                    let k = self.rd(base, key).to_php_bytes();
                    let v = self.rd(base, src);
                    let cell = self.globals.get_or_create(&k);
                    cell.set(v);
                }
                Op::LoadThis { dst } => {
                    let this = self.frames[fi].this.clone().ok_or_else(|| {
                        Unwind::error("Using $this when not in object context")
                    })?;
                    self.set(base, dst, Value::Object(this));
                }
                Op::InstanceOfRef { dst, obj, class } => {
                    let o = self.rd(base, obj);
                    let r = if let Value::Closure(_) = &o {
                        // Closures are not class instances yet (plan E6):
                        // only `instanceof Closure` holds.
                        match class.kind() {
                            ClassRefKind::Named(k) => {
                                self.name_bytes(&func, k).eq_ignore_ascii_case(b"closure")
                            }
                            ClassRefKind::Reg(r) => matches!(
                                self.rd(base, r),
                                Value::Str(s) if s.as_bytes().eq_ignore_ascii_case(b"closure")
                            ),
                            _ => false,
                        }
                    } else {
                        let cid = self.resolve_class_ref_quiet(&func, base, class)?;
                        match (o, cid) {
                            (Value::Object(o), Some(cid)) => self.instanceof_class(o.class_id(), cid),
                            _ => false,
                        }
                    };
                    self.set(base, dst, Value::Bool(r));
                }
                Op::Clone { dst, src, with } => {
                    let v = self.rd(base, src);
                    // php 8.5 `clone($o, ['p' => $v])`; `None` is plain `clone`.
                    let with = with.map(|r| self.rd(base, r));
                    let copy = self.clone_object_with(&v, with.as_ref())?;
                    self.set(base, dst, copy);
                }

                // --- units ---
                Op::DeclareFunction { idx } => {
                    let fid = func.unit.func_id(idx);
                    self.declare_function(fid)?;
                }
                Op::DeclareClass { idx } => {
                    let cid = func.unit.class_id(idx);
                    self.declare_class(cid)?;
                }
                Op::Include { dst, path, kind } => {
                    let p = self.rd(base, path).to_php_bytes();
                    if self.include_file(&p, kind, base + dst as usize)? {
                        return Ok(Switch::Continue);
                    }
                }
                Op::Eval { dst, src } => {
                    let code = self.rd(base, src).to_php_bytes();
                    // `eval.rs` compiles the string and pushes its frame.
                    self.stack[base + dst as usize] = Value::Null;
                    if self.eval_code(&code, base + dst as usize)? {
                        return Ok(Switch::Continue);
                    }
                }
                Op::Yield { dst, key, val } => {
                    let Some(gid) = self.frames[fi].generator else {
                        return Err(Unwind::error("Cannot yield outside a generator"));
                    };
                    let v = val.map(|r| self.rd(base, r)).unwrap_or(Value::Null);
                    let k = match key {
                        Some(r) => {
                            let k = self.rd(base, r);
                            self.generator_note_key(gid, &k);
                            k
                        }
                        None => self.generator_auto_key(gid),
                    };
                    self.park_generator(gid, k, v, dst)?;
                    return Ok(Switch::Done(Value::Null));
                }
                Op::YieldFrom { dst, src } => {
                    let Some(gid) = self.frames[fi].generator else {
                        return Err(Unwind::error("Cannot yield outside a generator"));
                    };
                    let v = self.rd(base, src);
                    self.begin_yield_from(gid, v, dst)?;
                    return Ok(Switch::Done(Value::Null));
                }
                Op::GenReturn { src } => {
                    let Some(gid) = self.frames[fi].generator else {
                        return Err(Unwind::error("Cannot return from outside a generator"));
                    };
                    let v = src.map(|r| self.rd(base, r)).unwrap_or(Value::Null);
                    self.finish_generator(gid, v);
                    return Ok(Switch::Done(Value::Null));
                }
                Op::FinallyEnd { state, payload, targets } => {
                    let st = self.rd(base, state);
                    match FinallyState::from_code(st.to_int()) {
                        Some(FinallyState::None) | None => {}
                        Some(FinallyState::Throw) => {
                            let v = self.rd(base, payload);
                            self.set(base, state, Value::Int(0));
                            return match v {
                                Value::Object(o) => Err(Unwind::Throw(o)),
                                _ => Err(Unwind::error("internal error: FinallyEnd without a pending exception")),
                            };
                        }
                        Some(FinallyState::Return) => {
                            let v = self.rd(base, payload);
                            self.set(base, state, Value::Int(0));
                            let v = if func.f.ret_ty.is_some() {
                                let strict = self.frames[fi].strict;
                                self.verify_return(&func, Some(v), strict)?
                            } else {
                                v
                            };
                            return self.do_return(v, stop_depth);
                        }
                        Some(FinallyState::Jump) => {
                            let k = self.rd(base, payload).to_int();
                            self.set(base, state, Value::Int(0));
                            let rows = func.f.consts[targets as usize]
                                .as_jump_table()
                                .expect("FinallyEnd targets");
                            let Some((_, target)) = rows.get(k as usize) else {
                                return Err(Unwind::error("internal error: FinallyEnd jump target out of range"));
                            };
                            pc = *target as usize;
                            continue;
                        }
                    }
                }

                // --- io ---
                Op::Echo { src } => {
                    let v = self.rd(base, src);
                    match &v {
                        Value::Object(_) | Value::Array(_) => {
                            let s = self.to_string(&v)?;
                            self.echo(s.as_bytes());
                            self.out.flush_pending();
                        }
                        _ => self.echo_value(&v),
                    }
                }

                // The M0 ops are no longer produced by the compiler.
                Op::Call { .. }
                | Op::CallNative { .. }
                | Op::CallDynamic { .. }
                | Op::New { .. }
                | Op::PropGet { .. }
                | Op::PropSet { .. }
                | Op::MethodCall { .. }
                | Op::StaticCall { .. }
                | Op::InstanceOf { .. }
                | Op::ForeachNext { .. } => {
                    return Err(Unwind::error(format!(
                        "internal error: M0 opcode not executed by the v2 interpreter: {op:?}"
                    )));
                }
            }
            pc += 1;
        }
    }

    /// `dst = a OP b` for the binary operators.
    fn arith(&mut self, base: usize, dst: u16, a: u16, b: u16, op: AssignOpKind) -> Result<(), Unwind> {
        let x = self.rd(base, a);
        let y = self.rd(base, b);
        let r = self.binary_op(op, &x, &y)?;
        self.set(base, dst, r);
        Ok(())
    }

    /// Return `v` from the top frame: pop it, release its registers, deliver
    /// the value to its target. A re-entry boundary hands the value back to
    /// `run_until`.
    fn do_return(&mut self, v: Value, stop_depth: usize) -> Result<Switch, Unwind> {
        let f = self.frames.pop().expect("frame");
        self.stack.truncate(f.base);
        self.silence = f.silence_base;
        // A frame pushed by bytecode (`DoCall`, `Include`) left its caller
        // parked on the call op: resume after it. A boundary frame's caller
        // is a native still inside its own op.
        if matches!(f.kind, FrameKind::Normal | FrameKind::Include) {
            if let Some(top) = self.frames.last_mut() {
                top.pc += 1;
            }
        }
        match f.ret {
            RetTarget::Discard => {}
            RetTarget::Reg(abs) => {
                if abs < self.stack.len() {
                    self.stack[abs] = v.clone();
                }
            }
            RetTarget::New { reg, obj } => {
                if reg < self.stack.len() {
                    self.stack[reg] = Value::Object(obj);
                }
            }
        }
        if f.kind == FrameKind::ReentryBoundary || self.frames.len() <= stop_depth {
            return Ok(Switch::Done(v));
        }
        Ok(Switch::Continue)
    }

    /// Whether the innermost pending call's parameter `pos` is by-reference.
    fn pending_by_ref(&self, fi: usize, pos: usize) -> bool {
        let Some(p) = self.frames[fi].pending.last() else {
            return false;
        };
        match &p.target {
            CallTarget::User { func, .. } => {
                let params = &func.f.params;
                match params.get(pos) {
                    Some(pd) => pd.by_ref,
                    // Past the declared parameters: a by-ref variadic takes
                    // the rest by reference.
                    None => params.last().is_some_and(|l| l.variadic && l.by_ref),
                }
            }
            CallTarget::Native(id) => self.natives[id.0 as usize].is_by_ref(pos),
            CallTarget::NativeMethod(m) => match &m.body {
                MethodBody::Native(nm) => nm.is_by_ref(pos),
                MethodBody::User(_) => false,
            },
            CallTarget::NoCtor => false,
        }
    }

    /// `f(): Argument #n ($x) could not be passed by reference`.
    fn by_ref_message(&self, fi: usize, pos: usize) -> String {
        let p = self.frames[fi].pending.last().expect("pending");
        let param = match &p.target {
            CallTarget::User { func, .. } => func
                .f
                .params
                .get(pos)
                .map(|pd| format!(" (${})", String::from_utf8_lossy(&pd.name)))
                .unwrap_or_default(),
            CallTarget::Native(id) => self.natives[id.0 as usize]
                .params
                .get(pos)
                .map(|n| format!(" (${n})"))
                .unwrap_or_default(),
            CallTarget::NativeMethod(m) => match &m.body {
                MethodBody::Native(nm) => nm
                    .params
                    .get(pos)
                    .map(|n| format!(" (${n})"))
                    .unwrap_or_default(),
                MethodBody::User(_) => String::new(),
            },
            CallTarget::NoCtor => String::new(),
        };
        let name = match &p.target {
            CallTarget::User { func, .. } => self.callable_display_name(func),
            _ => String::from_utf8_lossy(&p.name).into_owned(),
        };
        format!(
            "{name}(): Argument #{}{param} could not be passed by reference",
            pos + 1
        )
    }

    /// Stage one positional argument of the innermost pending call.
    fn send(&mut self, fi: usize, v: Value) {
        let p = self.frames[fi].pending.last_mut().expect("Send without Init");
        debug_assert_eq!(p.args_base + p.argc, self.stack.len());
        p.argc += 1;
        self.stack.push(v);
    }

    /// Resolve the target of an `InitFCall` through the inline cache.
    fn resolve_fcall(
        &mut self,
        func: &crate::unit::FuncRt,
        name: u32,
        ns_fallback: Option<u32>,
        ic: u16,
    ) -> Result<CallTarget, Unwind> {
        use crate::unit::IcSlot;
        let gen = self.func_gen;
        if let Some(slot) = func.ics.borrow().get(ic as usize) {
            match slot {
                IcSlot::Func { gen: g, id } if *g == gen => {
                    return Ok(CallTarget::User {
                        func: self.funcs[*id as usize].clone(),
                        closure: None,
                    })
                }
                IcSlot::Native { gen: g, id } if *g == gen => return Ok(CallTarget::Native(*id)),
                _ => {}
            }
        }
        let n = match &func.f.consts[name as usize] {
            Const::Name(n) => n,
            _ => return Err(Unwind::error("internal error: InitFCall without a name")),
        };
        let target = if let Some(&id) = self.func_index.get(&n.lower) {
            if let Some(slot) = func.ics.borrow_mut().get_mut(ic as usize) {
                *slot = IcSlot::Func { gen, id };
            }
            CallTarget::User {
                func: self.funcs[id as usize].clone(),
                closure: None,
            }
        } else if let Some(&id) = self.native_index.get(&n.lower) {
            if let Some(slot) = func.ics.borrow_mut().get_mut(ic as usize) {
                *slot = IcSlot::Native { gen, id };
            }
            CallTarget::Native(id)
        } else if let Some(fb) = ns_fallback {
            // Inside a namespace an unqualified call falls back to the global
            // function when the namespaced one does not exist — and that
            // fallback reaches **natives** too, which is how `strlen()` works
            // inside `namespace Composer\Autoload`.
            let g = match &func.f.consts[fb as usize] {
                Const::Name(g) => g,
                _ => return Err(Unwind::error("internal error: InitFCall fallback without a name")),
            };
            if let Some(&id) = self.func_index.get(&g.lower) {
                if let Some(slot) = func.ics.borrow_mut().get_mut(ic as usize) {
                    *slot = IcSlot::Func { gen, id };
                }
                CallTarget::User {
                    func: self.funcs[id as usize].clone(),
                    closure: None,
                }
            } else if let Some(&id) = self.native_index.get(&g.lower) {
                if let Some(slot) = func.ics.borrow_mut().get_mut(ic as usize) {
                    *slot = IcSlot::Native { gen, id };
                }
                CallTarget::Native(id)
            } else {
                // php names the *unqualified* candidate in the error.
                return Err(Unwind::error(format!(
                    "Call to undefined function {}()",
                    String::from_utf8_lossy(&n.orig)
                )));
            }
        } else {
            return Err(Unwind::error(format!(
                "Call to undefined function {}()",
                String::from_utf8_lossy(&n.orig)
            )));
        };
        Ok(target)
    }

    /// `arr[key] = v` / `arr[] = v` on the container in register `arr`
    /// (through a reference binding), with php's autovivification rules.
    fn array_set(&mut self, base: usize, arr: u16, key: Option<u16>, v: Value) -> Result<(), Unwind> {
        let k = key.map(|k| self.rd(base, k));
        let classes = self.classes.clone();
        let class_name = move |o: &Object| classes[o.class_id() as usize].name_str();
        let notice =
            self.with_slot(base, arr, |slot| Interp::array_set_in(slot, k.as_ref(), v, &class_name))?;
        match notice {
            crate::ops::SetNotice::None => {}
            crate::ops::SetNotice::FalseToArray => {
                self.deprecated("Automatic conversion of false to array is deprecated")?;
            }
            crate::ops::SetNotice::FirstByteOnly => {
                self.warn("Only the first byte will be assigned to the string offset")?;
            }
        }
        Ok(())
    }

    /// Fetch-for-write of `arr[key]`: the element is moved out of the
    /// container (or its reference cell shared) so the following nested
    /// write mutates it in place; the compiler writes it back afterwards.
    fn fetch_elem_w(&mut self, base: usize, arr: u16, key: Option<&Value>) -> Result<Value, Unwind> {
        let classes = self.classes.clone();
        let key = key.cloned();
        let r = self.with_slot(base, arr, |slot| {
            let mut deprecated_false = false;
            if matches!(slot, Value::Bool(false)) {
                deprecated_false = true;
                *slot = Value::empty_array();
            }
            if matches!(slot, Value::Null | Value::Uninit) {
                *slot = Value::empty_array();
            }
            match slot {
                Value::Array(a) => {
                    let Some(k) = &key else {
                        return Ok((Value::Null, deprecated_false));
                    };
                    let Some(k) = array_key(k) else {
                        return Err(Unwind::type_error(format!(
                            "Cannot access offset of type {} on array",
                            value_name(k)
                        )));
                    };
                    let taken = match a.get_mut(&k) {
                        Some(elem) => match elem {
                            Value::Ref(_) => elem.clone(),
                            _ => std::mem::replace(elem, Value::Null),
                        },
                        None => Value::Null,
                    };
                    Ok((taken, deprecated_false))
                }
                Value::Str(_) => Err(Unwind::error("Cannot use string offset as an array")),
                Value::Object(o) => Err(Unwind::error(format!(
                    "Cannot use object of type {} as array",
                    String::from_utf8_lossy(&classes[o.class_id() as usize].name)
                ))),
                Value::Closure(_) => Err(Unwind::error("Cannot use object of type Closure as array")),
                _ => Err(Unwind::error("Cannot use a scalar value as an array")),
            }
        })?;
        if r.1 {
            self.deprecated("Automatic conversion of false to array is deprecated")?;
        }
        Ok(r.0)
    }

    /// Whether an `ArrayAccess` object's elements are real storage a nested
    /// write reaches (php's `spl_array` / `WeakMap` dimension handlers),
    /// as against an `offsetGet()` that hands out a temporary: the
    /// resolved `offsetGet` is the engine's own, on one of those classes.
    pub(crate) fn has_dim_storage(&self, o: &Object) -> bool {
        let Some(m) = self.resolve_method(o.class_id(), b"offsetget") else {
            return false;
        };
        if !matches!(m.body, MethodBody::Native(_)) {
            return false;
        }
        matches!(
            self.classes[m.decl as usize].lname.as_ref(),
            b"arrayobject" | b"arrayiterator" | b"weakmap"
        )
    }

    /// `$o[$k]` fetched for a nested write: the stored element when the
    /// object has real dimension storage (the write-back stores it again;
    /// a `W` fetch is `quiet` — php neither warns nor notices on a missing
    /// key there — an `RW` one warns as a read would), otherwise
    /// `offsetGet()`'s temporary with php's notice that writing into it
    /// changes nothing.
    fn fetch_dim_w_object(&mut self, o: &Object, key: Value, quiet: bool) -> Result<Value, Unwind> {
        if self.has_dim_storage(o) {
            if !quiet {
                return self.offset_get(o, &key);
            }
            self.silence += 1;
            let r = self.offset_get(o, &key);
            self.silence -= 1;
            return r;
        }
        let o = o.clone();
        let v = self.call_method_raw(&o, b"offsetGet", std::slice::from_ref(&key))?;
        if !matches!(v, Value::Ref(_)) {
            self.notice(&format!(
                "Indirect modification of overloaded element of {} has no effect",
                self.class_of(&o).name_str()
            ))?;
        }
        Ok(v)
    }

    /// `&$arr[$key]`: the element's reference cell (autovivified).
    fn elem_ref(&mut self, base: usize, arr: u16, key: Option<&Value>) -> Result<PhpRef, Unwind> {
        let key = key.cloned();
        self.with_slot(base, arr, |slot| {
            if matches!(slot, Value::Null | Value::Uninit) {
                *slot = Value::empty_array();
            }
            match slot {
                Value::Array(a) => match key {
                    Some(k) => match array_key(&k) {
                        Some(k) => Ok(a.get_ref(k)),
                        None => Err(Unwind::type_error(format!(
                            "Cannot access offset of type {} on array",
                            value_name(&k)
                        ))),
                    },
                    None => {
                        let k = rphp_value::ArrayKey::Int(a.next_free_index());
                        Ok(a.get_ref(k))
                    }
                },
                Value::Str(_) => Err(Unwind::error("Cannot create references to/from string offsets")),
                Value::Object(_) | Value::Closure(_) => Err(Unwind::error("Cannot use object as array")),
                _ => Err(Unwind::error("Cannot use a scalar value as an array")),
            }
        })
    }

    /// `include`/`require`: resolve, compile (through the hook), load and
    /// push the file's `{main}` as an `Include` frame sharing the current
    /// symbol table. Returns `Ok(true)` when a frame was pushed (the loop
    /// must switch), `Ok(false)` when the value was delivered directly
    /// (`false` for a missing include, `true` for a repeated `_once`).
    fn include_file(&mut self, path: &[u8], kind: IncludeKind, dst_abs: usize) -> Result<bool, Unwind> {
        let keyword = match kind {
            IncludeKind::Include => "include",
            IncludeKind::IncludeOnce => "include_once",
            IncludeKind::Require => "require",
            IncludeKind::RequireOnce => "require_once",
        };
        let is_require = matches!(kind, IncludeKind::Require | IncludeKind::RequireOnce);
        let once = matches!(kind, IncludeKind::IncludeOnce | IncludeKind::RequireOnce);
        let path_str = String::from_utf8_lossy(path).into_owned();
        let include_path = self.ini_get("include_path").unwrap_or(".").to_string();
        let resolved = self.resolve_include_path(path);
        let Some(resolved) = resolved else {
            self.warn(&format!(
                "{keyword}({path_str}): Failed to open stream: No such file or directory"
            ))?;
            if is_require {
                return Err(Unwind::error(format!(
                    "Failed opening required '{path_str}' (include_path='{include_path}')"
                )));
            }
            self.warn(&format!(
                "{keyword}(): Failed opening '{path_str}' for inclusion (include_path='{include_path}')"
            ))?;
            self.stack[dst_abs] = Value::Bool(false);
            return Ok(false);
        };
        let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
        if once && self.included.contains(&canonical) {
            self.stack[dst_abs] = Value::Bool(true);
            return Ok(false);
        }
        let bytes = match std::fs::read(&resolved) {
            Ok(b) => b,
            Err(_) => {
                self.warn(&format!(
                    "{keyword}({path_str}): Failed to open stream: No such file or directory"
                ))?;
                if is_require {
                    return Err(Unwind::error(format!(
                        "Failed opening required '{path_str}' (include_path='{include_path}')"
                    )));
                }
                self.stack[dst_abs] = Value::Bool(false);
                return Ok(false);
            }
        };
        let name = canonical.to_string_lossy().into_owned();
        let Some(hook) = self.compile_hook.take() else {
            return Err(Unwind::error("include is not available: no compile hook installed"));
        };
        let compiled = hook(self, &bytes, &name);
        self.compile_hook = Some(hook);
        let module = match compiled {
            Ok(m) => m,
            Err(crate::interp::CompileFailure::Parse { message, line }) => {
                return Err(self.parse_error(&message, &name, line));
            }
            Err(crate::interp::CompileFailure::Rejected(lines)) => {
                return Err(Unwind::error(format!(
                    "{name}: the engine cannot lower this file yet:\n{}",
                    lines.join("\n")
                )));
            }
        };
        self.included.insert(canonical);
        let main = self.load_unit(module)?;
        let func = self.funcs[main as usize].clone();
        let fi = self.frames.len() - 1;
        let (this, scope, static_class, symtab) = {
            let f = &self.frames[fi];
            (f.this.clone(), f.scope, f.static_class, f.symtab.clone())
        };
        let symtab = symtab.unwrap_or_else(|| self.globals.clone());
        self.push_user_frame(
            func,
            FrameKind::Include,
            &[],
            this,
            scope,
            static_class,
            RetTarget::Reg(dst_abs),
            Some(symtab),
        )?;
        let top = self.frames.len() - 1;
        self.frames[top].include_kind = Some(kind);
        Ok(true)
    }

    /// Resolve an include path: absolute as is; otherwise the `include_path`
    /// entries, then the including file's directory, then the cwd.
    fn resolve_include_path(&self, path: &[u8]) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(String::from_utf8_lossy(path).into_owned());
        if p.is_absolute() || path.starts_with(b"./") || path.starts_with(b"../") {
            let full = if p.is_absolute() { p } else { self.cwd.join(p) };
            return full.is_file().then_some(full);
        }
        let include_path = self.ini_get("include_path").unwrap_or(".").to_string();
        let mut candidates: Vec<std::path::PathBuf> = include_path
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|dir| {
                if dir == "." {
                    self.cwd.join(&p)
                } else {
                    std::path::Path::new(dir).join(&p)
                }
            })
            .collect();
        if let Some(f) = self.current_user_frame() {
            let file = self.frame_file(f);
            if let Some(dir) = std::path::Path::new(&file).parent() {
                candidates.push(dir.join(&p));
            }
        }
        candidates.push(self.cwd.join(&p));
        candidates.into_iter().find(|c| c.is_file())
    }
}
