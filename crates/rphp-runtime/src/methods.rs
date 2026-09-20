//! Method dispatch and callables (plan E6).
//!
//! **This module owns how a method call and a callable value are resolved.**
//!
//! What lives here:
//!
//! * [`Interp::lookup_method`] / [`Interp::lookup_static_method`] — php's
//!   method lookup, judged against the *calling* scope, including the
//!   private-shadowing retry (a private method of the same name declared in
//!   the calling scope wins when the receiver is an instance of that scope);
//! * `__call` / `__callStatic`, consulted when a method is missing or not
//!   visible, and `__invoke`, which makes an object callable. Both magic
//!   forms are dispatched through a **trampoline** [`MethodDef`] so that
//!   every call path — `$o->m()`, `A::m()`, `$f()`, `call_user_func`,
//!   `array_map`, a first-class callable — packs the arguments into the
//!   `$args` array at exactly one place ([`Interp::run_call_trampoline`]);
//! * the static/non-static and visibility errors, with php's wording:
//!   `Call to undefined method C::m()` (the *receiver's* class),
//!   `Call to private method A::m() from global scope` and
//!   `Non-static method A::m() cannot be called statically` (the
//!   *declaring* class);
//! * [`Interp::resolve_callable`] — the one shared path for every callable
//!   spelling: `'f'`, `'A::m'`, `[$obj, 'm']`, `['A', 'm']`,
//!   `[$obj, 'parent::m']`, a `Closure`, an object with `__invoke`, and a
//!   first-class callable — so `array_map`, `call_user_func`, `usort`,
//!   `is_callable` and a direct `$f()` all agree;
//! * first-class callable syntax (`strlen(...)`, `$o->m(...)`, `A::m(...)`),
//!   which turns the pending-call record into a `Closure`
//!   ([`Interp::make_callable_closure`]);
//! * the `Closure` semantics behind the builtin class:
//!   [`Interp::closure_bind`], [`Interp::closure_call`],
//!   [`Interp::closure_from_callable`], static closures and what `$this`
//!   means inside a closure declared in a class body.
//!
//! ## The closure representation
//!
//! A closure value is `Value::Closure(Closure)`; `Closure::func()` is a
//! process-wide function id and `Closure::captures()` is
//! `[explicit captures…, $this, scope]` — the bound receiver (an object or
//! null) and the bound scope class id (an int or null), appended by
//! `Op::MakeClosure`. [`Interp::closure_binding`] reads that tail, so every
//! engine-built closure keeps the same two trailing entries.
//!
//! A callable that is **not** a compiled function — a native function, a
//! native method, a `__call` trampoline — cannot be named by a function id,
//! so the engine builds an *engine closure*: `func()` is the [`ENGINE_CLOSURE`]
//! sentinel and capture 0 holds the underlying callable value (`'strlen'`,
//! `[$obj, 'format']`, `'C::zz'`), re-resolved on every call in the captured
//! scope. Everything else (binding, `bindTo`, `is_callable`, `$f()`) then
//! works on both kinds without a special case.

use std::rc::Rc;

use rphp_bytecode::{FnFlags, Op, Visibility};
use rphp_value::{Array, ArrayKey, Closure, Object, Value};

use crate::call::Callable;
use crate::class::{MethodBody, MethodDef, NativeMethod};
use crate::frame::{CallTarget, PendingCall};
use crate::registry::{Ctx, NativeResult, Unwind};
use crate::unit::FuncRt;
use crate::Interp;

/// The `Closure::func()` of an **engine closure** (see the module header):
/// capture 0 is the underlying callable value, re-resolved on each call.
pub(crate) const ENGINE_CLOSURE: u32 = u32::MAX;

/// How many trailing capture slots the engine appends to every closure:
/// `[$this, scope, called class]`. php keeps the scope (what the body may
/// reach) and the called class (what `static::` resolves to) apart — see
/// [`Interp::closure_called_class`].
pub(crate) const CLOSURE_TAIL: usize = 3;

/// The tag in `NativeMethod::params[0]` that marks a `__call` /
/// `__callStatic` trampoline, and the one that marks a `Closure`
/// instance-method trampoline. A parameter name can never collide with
/// either: php identifiers hold no NUL, and every real native names its
/// parameters after the php stub.
const MAGIC_TAG: &str = "\0rphp:__call";
const CLOSURE_TAG: &str = "\0rphp:closure";

/// The handler slot of a trampoline. It is never entered: the call is taken
/// off the native path in `call.rs` and run by
/// [`Interp::run_call_trampoline`] / [`Interp::run_closure_method`], which
/// need the whole argument window. Reaching it means a call path bypassed
/// that interception.
fn trampoline_unreachable(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "internal error: an engine trampoline was invoked directly",
    ))
}

/// The descriptor of a `__call` / `__callStatic` trampoline: variadic, with
/// no real parameter names (named arguments become string keys in `$args`).
const MAGIC_TRAMPOLINE: NativeMethod = NativeMethod {
    handler: trampoline_unreachable,
    min_args: 0,
    max_args: None,
    params: &[MAGIC_TAG],
    by_ref: 0,
    is_static: false,
    is_final: false,
};

/// The descriptor of a `Closure` instance-method trampoline. The receiver is
/// staged as argument 0, so the arity is unconstrained here and checked in
/// [`Interp::run_closure_method`] with php's wording.
const CLOSURE_METHOD: NativeMethod = NativeMethod {
    handler: trampoline_unreachable,
    min_args: 0,
    max_args: None,
    params: &[CLOSURE_TAG],
    by_ref: 0,
    is_static: false,
    is_final: false,
};

/// Whether a native descriptor carries `tag` as its marker parameter.
fn tagged(m: &MethodDef, tag: &str) -> bool {
    match &m.body {
        MethodBody::Native(nm) => nm.params.first().is_some_and(|p| *p == tag),
        MethodBody::User(_) => false,
    }
}

/// Why a method could not be dispatched.
pub(crate) enum MethodMiss {
    /// No method of that name exists on the class.
    Undefined,
    /// One exists but is not visible from the calling scope.
    Inaccessible {
        vis: Visibility,
        /// The declaring class — the one php names in the message.
        decl: u32,
    },
}

impl Interp {
    // ---- the method table --------------------------------------------------

    /// php's instance-method lookup: the method `name` of `class` as seen
    /// from `scope`.
    ///
    /// A method that is not visible from `scope` is retried against a
    /// **private** method of the same name declared *in* `scope`, which php
    /// prefers when the receiver is an instance of that scope — that is how
    /// `A::go()` calling `$this->pn()` reaches `A::pn` even though the `B`
    /// instance overrides `pn` with a private of its own.
    pub(crate) fn lookup_method(
        &self,
        class: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Result<Rc<MethodDef>, MethodMiss> {
        let Some(m) = self.resolve_method(class, name) else {
            return Err(MethodMiss::Undefined);
        };
        if self.access_ok(m.vis, m.decl, scope) {
            return Ok(m);
        }
        if let Some(s) = scope {
            if let Some(shadow) = self.resolve_method(s, name) {
                if shadow.vis == Visibility::Private
                    && shadow.decl == s
                    && self.is_subclass_or_eq(class, s)
                {
                    return Ok(shadow);
                }
            }
        }
        Err(MethodMiss::Inaccessible {
            vis: m.vis,
            decl: m.decl,
        })
    }

    /// php's static-method lookup (`A::m()`, `'A::m'`, `['A', 'm']`): like
    /// [`Interp::lookup_method`] but without the private-shadowing retry,
    /// which php only applies to a call through an object.
    pub(crate) fn lookup_static_method(
        &self,
        class: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Result<Rc<MethodDef>, MethodMiss> {
        let Some(m) = self.resolve_method(class, name) else {
            return Err(MethodMiss::Undefined);
        };
        if self.access_ok(m.vis, m.decl, scope) {
            return Ok(m);
        }
        Err(MethodMiss::Inaccessible {
            vis: m.vis,
            decl: m.decl,
        })
    }

    /// php's `Error` for a lookup that missed. `class` is the class the call
    /// was dispatched on — php names it in the undefined case and names the
    /// *declaring* class in the visibility case.
    pub(crate) fn method_miss_error(
        &self,
        miss: MethodMiss,
        class: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Unwind {
        match miss {
            MethodMiss::Undefined => Unwind::error(format!(
                "Call to undefined method {}::{}()",
                self.classes[class as usize].name_str(),
                String::from_utf8_lossy(name)
            )),
            MethodMiss::Inaccessible { vis, decl } => Unwind::error(format!(
                "Call to {} method {}::{}() from {}",
                crate::exec::vis_word(vis),
                self.classes[decl as usize].name_str(),
                String::from_utf8_lossy(name),
                self.scope_word(scope)
            )),
        }
    }

    /// `global scope` / `scope Foo`, as php renders the calling context in a
    /// visibility error.
    pub(crate) fn scope_word(&self, scope: Option<u32>) -> String {
        match scope {
            Some(c) => format!("scope {}", self.classes[c as usize].name_str()),
            None => "global scope".to_string(),
        }
    }

    /// Enforce method visibility against the current frame's scope (the
    /// pre-E6 entry point, kept for the call sites that only need the check).
    pub(crate) fn check_method_access(
        &self,
        vis: Visibility,
        decl: u32,
        name: &[u8],
    ) -> Result<(), Unwind> {
        let scope = self.current_user_frame().and_then(|f| f.scope);
        if self.access_ok(vis, decl, scope) {
            return Ok(());
        }
        Err(Unwind::error(format!(
            "Call to {} method {}::{}() from {}",
            crate::exec::vis_word(vis),
            self.classes[decl as usize].name_str(),
            String::from_utf8_lossy(name),
            self.scope_word(scope)
        )))
    }

    /// Whether a member with the given visibility, declared in `decl`, is
    /// reachable from code executing in `scope`.
    pub(crate) fn access_ok(&self, vis: Visibility, decl: u32, scope: Option<u32>) -> bool {
        match vis {
            Visibility::Public => true,
            Visibility::Private => scope == Some(decl),
            Visibility::Protected => match scope {
                Some(cc) => self.is_subclass_or_eq(cc, decl) || self.is_subclass_or_eq(decl, cc),
                None => false,
            },
        }
    }

    /// [`Interp::access_ok`] for natives (`get_object_vars`,
    /// `get_class_methods`).
    pub fn access_ok_public(&self, vis: Visibility, decl: u32, scope: Option<u32>) -> bool {
        self.access_ok(vis, decl, scope)
    }

    /// The scope the running code is in (the innermost *user* frame's class),
    /// which is what every visibility decision is judged against. A native on
    /// the stack is transparent, so `call_user_func([$this, 'priv'])` called
    /// from inside the class still sees that class.
    pub fn calling_scope(&self) -> Option<u32> {
        self.current_user_frame().and_then(|f| f.scope)
    }

    // ---- __call / __callStatic ---------------------------------------------

    /// A trampoline descriptor standing in for `__call` (`is_static` false,
    /// with the receiver as `$this`) or `__callStatic` (`is_static` true).
    ///
    /// The fields are repurposed: `name` is the method name **as written at
    /// the call site** (php passes that spelling to the magic method) and
    /// `decl` is the class the magic method is *dispatched on*, not a
    /// declaring class. Nothing outside [`Interp::run_call_trampoline`] inspects a
    /// trampoline.
    fn call_trampoline(&self, dispatch: u32, name: &[u8], is_static: bool) -> Rc<MethodDef> {
        Rc::new(MethodDef {
            name: Box::from(name),
            body: MethodBody::Native(MAGIC_TRAMPOLINE),
            vis: Visibility::Public,
            is_static,
            is_abstract: false,
            is_final: false,
            decl: dispatch,
        })
    }

    /// Whether a method descriptor is a `__call` / `__callStatic` trampoline.
    pub(crate) fn is_magic_trampoline(m: &MethodDef) -> bool {
        tagged(m, MAGIC_TAG)
    }

    /// Whether class `class` has `name` (`__call` / `__callStatic`) available.
    fn defines_method(&self, class: u32, name: &[u8]) -> bool {
        self.resolve_method(class, name).is_some()
    }

    /// Run a `__call` / `__callStatic` trampoline: pack `args` (positional)
    /// and `named` (string keys, php 8's named-argument behaviour) into the
    /// `$args` array and invoke the magic method.
    pub(crate) fn run_call_trampoline(
        &mut self,
        m: Rc<MethodDef>,
        this: Option<Object>,
        args: Vec<Value>,
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        let magic_name: &[u8] = if m.is_static { b"__callstatic" } else { b"__call" };
        let scope = self.calling_scope();
        let Some(magic) = self.resolve_method(m.decl, magic_name) else {
            return Err(self.method_miss_error(MethodMiss::Undefined, m.decl, &m.name, scope));
        };
        let mut packed = Array::new();
        for a in args {
            packed.push(a.deref().into_owned());
        }
        for (k, v) in named {
            packed.set(ArrayKey::Str(k), v.deref().into_owned());
        }
        let call_args = [Value::string(&m.name), Value::Array(packed)];
        let static_class = match &this {
            Some(o) => Some(o.class_id()),
            None => Some(m.decl),
        };
        match &magic.body {
            MethodBody::Native(_) => {
                let mut a = call_args.to_vec();
                self.call_native_method(magic.clone(), this, &mut a)
            }
            MethodBody::User(f) => {
                let f = f.clone();
                self.call_user_func(f, this, Some(magic.decl), static_class, None, &call_args)
            }
        }
    }

    // ---- callable resolution -----------------------------------------------

    /// Resolve a callable value against the **calling** scope — the one
    /// shared path behind `Op::InitDynCall`, `call_value`, `is_callable`,
    /// `array_map`, `usort` and the first-class callable syntax.
    pub fn resolve_callable(&self, callee: &Value) -> Result<Callable, Unwind> {
        self.resolve_callable_in_scope(callee, self.calling_scope())
    }

    /// [`Interp::resolve_callable`] against an explicit scope. An engine
    /// closure re-resolves its target this way, so a closure taken over a
    /// private method keeps reaching it after it escapes the class.
    pub fn resolve_callable_in_scope(
        &self,
        callee: &Value,
        scope: Option<u32>,
    ) -> Result<Callable, Unwind> {
        let callee = callee.deref();
        match &*callee {
            Value::Closure(c) if c.func() == ENGINE_CLOSURE => self.resolve_engine_closure(c),
            Value::Closure(c) => {
                let func = self
                    .funcs
                    .get(c.func() as usize)
                    .cloned()
                    .ok_or_else(|| Unwind::error("Closure refers to an unloaded function"))?;
                let (this, cscope, called) = self.closure_binding(c);
                let static_class = this.as_ref().map(|o| o.class_id()).or(called).or(cscope);
                Ok(Callable::User {
                    func,
                    this,
                    scope: cscope,
                    static_class,
                    closure: Some(c.clone()),
                })
            }
            Value::Str(s) => {
                let name = s.as_bytes();
                let name = name.strip_prefix(b"\\").unwrap_or(name);
                if let Some(pos) = name.windows(2).position(|w| w == b"::") {
                    let (class, method) = (&name[..pos], &name[pos + 2..]);
                    return self.resolve_static_callable(class, method, None, scope);
                }
                if let Some(id) = self.user_function(name) {
                    return Ok(Callable::User {
                        func: self.funcs[id as usize].clone(),
                        this: None,
                        scope: None,
                        static_class: None,
                        closure: None,
                    });
                }
                if let Some(id) = self.native_by_name(name) {
                    return Ok(Callable::Native(id));
                }
                Err(Unwind::error(format!(
                    "Call to undefined function {}()",
                    String::from_utf8_lossy(name)
                )))
            }
            Value::Array(a) => {
                if a.len() != 2 {
                    return Err(Unwind::error("Array callback must have exactly two elements"));
                }
                let first = a
                    .get_deref(&ArrayKey::Int(0))
                    .ok_or_else(|| Unwind::error("Array callback has to contain indices 0 and 1"))?;
                let second = a
                    .get_deref(&ArrayKey::Int(1))
                    .ok_or_else(|| Unwind::error("Array callback has to contain indices 0 and 1"))?;
                if !matches!(first, Value::Object(_) | Value::Str(_) | Value::Closure(_)) {
                    return Err(Unwind::error(
                        "First array member is not a valid class name or object",
                    ));
                }
                let Value::Str(method) = &second else {
                    return Err(Unwind::error("Second array member is not a valid method"));
                };
                // `[$closure, '__invoke']` is a valid callback in php. The
                // other `Closure` methods take the receiver as `$this`, which
                // a `Value::Closure` cannot be — see the module report.
                if matches!(first, Value::Closure(_)) {
                    if method.as_bytes().eq_ignore_ascii_case(b"__invoke") {
                        return self.resolve_callable_in_scope(&first, scope);
                    }
                    return Err(Unwind::error(format!(
                        "Call to undefined method Closure::{}()",
                        String::from_utf8_lossy(method.as_bytes())
                    )));
                }
                match &first {
                    Value::Object(o) => self.resolve_object_callable(o, method.as_bytes(), scope),
                    Value::Str(class) => {
                        self.resolve_static_callable(class.as_bytes(), method.as_bytes(), None, scope)
                    }
                    _ => Err(Unwind::error(
                        "First array member is not a valid class name or object",
                    )),
                }
            }
            // An object with `__invoke` is callable; anything else is not.
            Value::Object(o) => match self.lookup_method(o.class_id(), b"__invoke", scope) {
                Ok(m) => Ok(self.callable_for(m, Some(o.clone()), o.class_id())),
                Err(_) => Err(Unwind::error(format!(
                    "Object of type {} is not callable",
                    self.class_name_of(o)
                ))),
            },
            other => Err(Unwind::error(format!(
                "Value of type {} is not callable",
                crate::ops::value_name(other)
            ))),
        }
    }

    /// `[$obj, 'm']` — and php's qualified spellings `[$obj, 'A::m']`,
    /// `[$obj, 'parent::m']`, `[$obj, 'self::m']`, `[$obj, 'static::m']`,
    /// which call the named class's implementation non-virtually.
    ///
    /// php additionally emits `Deprecated: Callables of the form
    /// ["B", "parent::m"] are deprecated` for the qualified forms; this path
    /// is `&self` (it is shared with `is_callable`, which the stdlib calls
    /// through a shared borrow), so the deprecation is not emitted here.
    fn resolve_object_callable(
        &self,
        o: &Object,
        method: &[u8],
        scope: Option<u32>,
    ) -> Result<Callable, Unwind> {
        if let Some(pos) = method.windows(2).position(|w| w == b"::") {
            let (class, name) = (&method[..pos], &method[pos + 2..]);
            let cid = if class.eq_ignore_ascii_case(b"parent") {
                scope.and_then(|s| self.classes[s as usize].parent)
            } else if class.eq_ignore_ascii_case(b"self") {
                scope
            } else if class.eq_ignore_ascii_case(b"static") {
                Some(o.class_id())
            } else {
                self.class_by_name(class)
            };
            let cid = cid.ok_or_else(|| {
                Unwind::error(format!(
                    "Class \"{}\" not found",
                    String::from_utf8_lossy(class)
                ))
            })?;
            let m = match self.lookup_static_method(cid, name, scope) {
                Ok(m) => m,
                Err(miss) => return Err(self.method_miss_error(miss, cid, name, scope)),
            };
            let this = if m.is_static { None } else { Some(o.clone()) };
            let static_class = this.as_ref().map(|t| t.class_id()).unwrap_or(cid);
            return Ok(self.callable_for(m, this, static_class));
        }
        match self.lookup_method(o.class_id(), method, scope) {
            Ok(m) => {
                let this = if m.is_static { None } else { Some(o.clone()) };
                Ok(self.callable_for(m, this, o.class_id()))
            }
            Err(miss) => {
                if self.defines_method(o.class_id(), b"__call") {
                    let t = self.call_trampoline(o.class_id(), method, false);
                    return Ok(Callable::NativeMethod {
                        method: t,
                        this: Some(o.clone()),
                    });
                }
                Err(self.method_miss_error(miss, o.class_id(), method, scope))
            }
        }
    }

    /// `'A::m'` / `['A', 'm']`: a method reached through its class. Without a
    /// compatible `$this` in the current frame a non-static method cannot be
    /// called statically; a missing or inaccessible method falls back to
    /// `__call` (with a compatible `$this`) or `__callStatic`.
    fn resolve_static_callable(
        &self,
        class: &[u8],
        method: &[u8],
        this: Option<Object>,
        scope: Option<u32>,
    ) -> Result<Callable, Unwind> {
        let cid = self.class_by_name(class).ok_or_else(|| {
            Unwind::error(format!(
                "Class \"{}\" not found",
                String::from_utf8_lossy(class)
            ))
        })?;
        let forwarded = this.or_else(|| {
            self.current_user_frame()
                .and_then(|f| f.this.clone())
                .filter(|o| self.is_subclass_or_eq(o.class_id(), cid))
        });
        match self.lookup_static_method(cid, method, scope) {
            Ok(m) => {
                let this = if m.is_static { None } else { forwarded };
                if this.is_none() && !m.is_static {
                    return Err(Unwind::error(format!(
                        "Non-static method {}::{}() cannot be called statically",
                        self.classes[m.decl as usize].name_str(),
                        String::from_utf8_lossy(&m.name)
                    )));
                }
                let static_class = this.as_ref().map(|o| o.class_id()).unwrap_or(cid);
                Ok(self.callable_for(m, this, static_class))
            }
            Err(miss) => self.magic_static_fallback(cid, method, forwarded, miss, scope),
        }
    }

    /// The `__call` / `__callStatic` fallback of a static call. php prefers
    /// `__call` — with the current `$this` — when the method is *missing* and
    /// the frame holds a compatible receiver; an **inaccessible** method only
    /// ever falls back to `__callStatic`.
    fn magic_static_fallback(
        &self,
        cid: u32,
        method: &[u8],
        this: Option<Object>,
        miss: MethodMiss,
        scope: Option<u32>,
    ) -> Result<Callable, Unwind> {
        if matches!(miss, MethodMiss::Undefined) {
            if let Some(o) = this {
                if self.defines_method(cid, b"__call") {
                    let t = self.call_trampoline(o.class_id(), method, false);
                    return Ok(Callable::NativeMethod {
                        method: t,
                        this: Some(o),
                    });
                }
            }
        }
        if self.defines_method(cid, b"__callstatic") {
            let t = self.call_trampoline(cid, method, true);
            return Ok(Callable::NativeMethod { method: t, this: None });
        }
        Err(self.method_miss_error(miss, cid, method, scope))
    }

    /// Wrap a resolved method in a [`Callable`].
    fn callable_for(&self, m: Rc<MethodDef>, this: Option<Object>, static_class: u32) -> Callable {
        match &m.body {
            MethodBody::Native(_) => Callable::NativeMethod {
                method: m.clone(),
                this,
            },
            MethodBody::User(func) => Callable::User {
                func: func.clone(),
                this,
                scope: Some(m.decl),
                static_class: Some(static_class),
                closure: None,
            },
        }
    }

    /// Whether a value is callable (`is_callable`). `__call` / `__callStatic`
    /// make an otherwise missing method callable, exactly as php reports.
    pub fn is_callable(&self, v: &Value) -> bool {
        self.resolve_callable(v).is_ok()
    }

    // ---- the `Init*` entry points ------------------------------------------

    /// `$obj->m(...)` — `Op::InitMethodCall`. Pushes the pending-call record
    /// on frame `fi`: virtual dispatch on the runtime class of `obj`,
    /// visibility judged against the calling scope, `__call` when the method
    /// is missing or not visible. A `Closure` receiver dispatches to the
    /// engine-side closure methods ([`Interp::init_closure_method_call`]).
    pub(crate) fn init_method_call(
        &mut self,
        fi: usize,
        obj: Value,
        mname: Box<[u8]>,
    ) -> Result<(), Unwind> {
        let obj = obj.deref().into_owned();
        if let Value::Closure(c) = &obj {
            return self.init_closure_method_call(fi, c.clone(), mname);
        }
        let Value::Object(o) = obj else {
            return Err(Unwind::error(format!(
                "Call to a member function {}() on {}",
                String::from_utf8_lossy(&mname),
                crate::ops::value_name(&obj),
            )));
        };
        let scope = self.frames[fi].scope;
        let (target, this, call_scope, static_class) =
            match self.lookup_method(o.class_id(), &mname, scope) {
                Ok(m) => {
                    if m.is_abstract {
                        return Err(Unwind::error(format!(
                            "Cannot call abstract method {}::{}()",
                            self.classes[m.decl as usize].name_str(),
                            String::from_utf8_lossy(&m.name)
                        )));
                    }
                    let this = if m.is_static { None } else { Some(o.clone()) };
                    let target = match &m.body {
                        MethodBody::User(f) => CallTarget::User {
                            func: f.clone(),
                            closure: None,
                        },
                        MethodBody::Native(_) => CallTarget::NativeMethod(m.clone()),
                    };
                    (target, this, Some(m.decl), Some(o.class_id()))
                }
                Err(miss) => {
                    if !self.defines_method(o.class_id(), b"__call") {
                        return Err(self.method_miss_error(miss, o.class_id(), &mname, scope));
                    }
                    let t = self.call_trampoline(o.class_id(), &mname, false);
                    (
                        CallTarget::NativeMethod(t),
                        Some(o.clone()),
                        None,
                        Some(o.class_id()),
                    )
                }
            };
        let args_base = self.stack.len();
        self.frames[fi].pending.push(PendingCall {
            target,
            this,
            scope: call_scope,
            static_class,
            args_base,
            argc: 0,
            named: Vec::new(),
            new_obj: None,
            name: mname,
        });
        Ok(())
    }

    /// `Class::m(...)` — `Op::InitStaticCall`. `forwarding` is true for
    /// `self::`/`parent::`/`static::`, which keep the caller's late static
    /// binding; a named class rebinds it.
    pub(crate) fn init_static_call(
        &mut self,
        fi: usize,
        cid: u32,
        mname: Box<[u8]>,
        forwarding: bool,
    ) -> Result<(), Unwind> {
        let scope = self.frames[fi].scope;
        // A forwarding instance call keeps `$this` when it is an instance of
        // the named class.
        let receiver = self.frames[fi]
            .this
            .clone()
            .filter(|o| self.is_subclass_or_eq(o.class_id(), cid));
        let (target, this, call_scope, bound) = match self.lookup_static_method(cid, &mname, scope) {
            Ok(m) => {
                if m.is_abstract {
                    return Err(Unwind::error(format!(
                        "Cannot call abstract method {}::{}()",
                        self.classes[m.decl as usize].name_str(),
                        String::from_utf8_lossy(&m.name)
                    )));
                }
                let this = if m.is_static { None } else { receiver };
                if this.is_none() && !m.is_static {
                    return Err(Unwind::error(format!(
                        "Non-static method {}::{}() cannot be called statically",
                        self.classes[m.decl as usize].name_str(),
                        String::from_utf8_lossy(&m.name)
                    )));
                }
                let target = match &m.body {
                    MethodBody::User(f) => CallTarget::User {
                        func: f.clone(),
                        closure: None,
                    },
                    MethodBody::Native(_) => CallTarget::NativeMethod(m.clone()),
                };
                (target, this, Some(m.decl), None)
            }
            Err(miss) => match self.magic_static_fallback(cid, &mname, receiver, miss, scope)? {
                Callable::NativeMethod { method, this } => {
                    let bound = this.as_ref().map(|o| o.class_id()).unwrap_or(cid);
                    (CallTarget::NativeMethod(method), this, None, Some(bound))
                }
                // `magic_static_fallback` only ever produces a trampoline.
                _ => unreachable!("the magic fallback is always a trampoline"),
            },
        };
        let static_class = match bound {
            Some(b) => Some(b),
            None if forwarding => self.frames[fi].static_class.or(Some(cid)),
            None => this.as_ref().map(|o| o.class_id()).or(Some(cid)),
        };
        let args_base = self.stack.len();
        self.frames[fi].pending.push(PendingCall {
            target,
            this,
            scope: call_scope,
            static_class,
            args_base,
            argc: 0,
            named: Vec::new(),
            new_obj: None,
            name: mname,
        });
        Ok(())
    }

    // ---- closures ----------------------------------------------------------

    /// The closure binding convention: `Closure::captures()` holds the
    /// explicit captures (`Function::captures` order), then the bound `$this`
    /// (or null) and the scope class id (or null).
    pub(crate) fn closure_binding(&self, c: &Closure) -> (Option<Object>, Option<u32>, Option<u32>) {
        let caps = c.captures();
        let n = caps.len();
        if n < CLOSURE_TAIL {
            return (None, None, None);
        }
        let this = match &caps[n - 3] {
            Value::Object(o) => Some(o.clone()),
            _ => None,
        };
        let scope = match &caps[n - 2] {
            Value::Int(i) => Some(*i as u32),
            _ => None,
        };
        let called = match &caps[n - 1] {
            Value::Int(i) => Some(*i as u32),
            _ => None,
        };
        (this, scope, called)
    }

    /// The late-static-bound class of a closure
    /// (`ReflectionFunction::getClosureCalledClass`). php keeps this separate
    /// from the scope: `B::m(...)` over a method declared in `A` has scope
    /// `A` (what it may reach) and called class `B` (what `static::` means).
    pub fn closure_called_class(&self, c: &Closure) -> Option<u32> {
        self.closure_binding(c).2
    }

    /// The `$this` a closure is bound to (`ReflectionFunction::getClosureThis`).
    pub fn closure_this(&self, c: &Closure) -> Option<Object> {
        self.closure_binding(c).0
    }

    /// The class a closure is scoped to (its visibility context).
    pub fn closure_scope(&self, c: &Closure) -> Option<u32> {
        self.closure_binding(c).1
    }

    /// Whether the closure is a `static function () {}` — no `$this` may ever
    /// be bound to it.
    ///
    /// An engine closure is never static: php's "fake closures" over
    /// internal functions are not either (`strlen(...)->bindTo(new stdClass)`
    /// returns a closure rather than warning).
    pub fn closure_is_static(&self, c: &Closure) -> bool {
        match self.funcs.get(c.func() as usize) {
            Some(f) => f.f.flags.contains(FnFlags::STATIC),
            None => false,
        }
    }

    /// Whether the closure body mentions `$this` (php refuses to unbind such
    /// a closure once it is bound).
    fn closure_uses_this(&self, c: &Closure) -> bool {
        match self.funcs.get(c.func() as usize) {
            Some(f) => f.f.flags.contains(FnFlags::USES_THIS),
            None => false,
        }
    }

    /// Resolve an engine closure (see the module header): capture 0 is the
    /// underlying callable, re-resolved in the captured scope so a closure
    /// taken over a private method still reaches it.
    fn resolve_engine_closure(&self, c: &Closure) -> Result<Callable, Unwind> {
        let caps = c.captures();
        let target = caps.first().cloned().unwrap_or(Value::Null);
        let (this, scope, _called) = self.closure_binding(c);
        let resolved = self.resolve_callable_in_scope(&target, scope)?;
        // A rebound engine closure overrides the receiver of a method target.
        Ok(match (resolved, this) {
            (Callable::NativeMethod { method, this: t }, bound) => Callable::NativeMethod {
                method,
                this: bound.or(t),
            },
            (other, _) => other,
        })
    }

    /// Build a closure value over an already-resolved callable, bound to
    /// `this` and `scope`. A compiled function becomes an ordinary closure
    /// over its function id; anything else becomes an *engine closure* over
    /// `target`, the callable value that produced it.
    fn closure_over(
        &self,
        callable: &Callable,
        target: Value,
        this: Option<Object>,
        scope: Option<u32>,
        called: Option<u32>,
    ) -> Value {
        let tail = [
            this.as_ref().map_or(Value::Null, |o| Value::Object(o.clone())),
            scope.map_or(Value::Null, |c| Value::Int(i64::from(c))),
            called
                .or_else(|| this.as_ref().map(|o| o.class_id()))
                .or(scope)
                .map_or(Value::Null, |c| Value::Int(i64::from(c))),
        ];
        match callable {
            Callable::User { func, closure: None, .. } => {
                Value::Closure(Closure::new(func.id, tail.to_vec()))
            }
            // Already a closure value: keep its captures, replace the binding.
            Callable::User { closure: Some(c), .. } => {
                let caps = c.captures();
                let mut new: Vec<Value> = caps[..caps.len().saturating_sub(CLOSURE_TAIL)].to_vec();
                new.extend_from_slice(&tail);
                Value::Closure(Closure::new(c.func(), new))
            }
            Callable::Native(_) | Callable::NativeMethod { .. } => {
                let mut caps = vec![target];
                caps.extend_from_slice(&tail);
                Value::Closure(Closure::new(ENGINE_CLOSURE, caps))
            }
        }
    }

    /// `Closure::fromCallable($callable)` — and the engine side of every
    /// place php turns a callable into a closure.
    ///
    /// A `Closure` is returned unchanged. Anything else is resolved through
    /// [`Interp::resolve_callable`] (so visibility is judged where
    /// `fromCallable` was called) and wrapped. An unresolvable callable
    /// raises php's `TypeError: Failed to create closure from callable: …`.
    pub fn closure_from_callable(&mut self, v: &Value) -> Result<Value, Unwind> {
        let v = v.deref().into_owned();
        if matches!(v, Value::Closure(_)) {
            return Ok(v);
        }
        let scope = self.calling_scope();
        let callable = match self.resolve_callable_in_scope(&v, scope) {
            Ok(c) => c,
            Err(_) => return Err(Unwind::type_error(self.from_callable_error(&v))),
        };
        // A user target carries the declaring class as its scope, so the
        // closure keeps reaching that class's private members. An engine
        // closure re-resolves its target instead, so it must remember the
        // scope the resolution was made in — otherwise a callable that only
        // resolved through `__call` here would resolve straight to the
        // private method later.
        let (this, cscope, called) = match &callable {
            // `Closure::fromCallable('B::m')` keeps php's split: scope is the
            // declaring class, the called class is the one named.
            Callable::User { this, scope: s, static_class, .. } => {
                (this.clone(), *s, *static_class)
            }
            Callable::NativeMethod { this, .. } => (this.clone(), scope, None),
            Callable::Native(_) => (None, scope, None),
        };
        Ok(self.closure_over(&callable, v, this, cscope, called))
    }

    /// php's `Failed to create closure from callable: …` detail for a
    /// callable that did not resolve.
    fn from_callable_error(&self, v: &Value) -> String {
        let detail = match v {
            Value::Str(s) => {
                let name = s.as_bytes();
                let bare = name.strip_prefix(b"\\").unwrap_or(name);
                match bare.windows(2).position(|w| w == b"::") {
                    Some(pos) => {
                        let (class, method) = (&bare[..pos], &bare[pos + 2..]);
                        match self.class_by_name(class) {
                            None => format!("class \"{}\" not found", String::from_utf8_lossy(class)),
                            Some(_) => format!(
                                "class {} does not have a method \"{}\"",
                                String::from_utf8_lossy(class),
                                String::from_utf8_lossy(method)
                            ),
                        }
                    }
                    None => format!(
                        "function \"{}\" not found or invalid function name",
                        String::from_utf8_lossy(name)
                    ),
                }
            }
            Value::Array(a) => {
                let target = a.get_deref(&ArrayKey::Int(0));
                let method = a.get_deref(&ArrayKey::Int(1));
                match (target, method) {
                    (Some(t), Some(Value::Str(m))) => {
                        let class = match &t {
                            Value::Object(o) => Some(self.class_name_of(o)),
                            Value::Str(s) => {
                                let n = s.as_bytes();
                                self.class_by_name(n).map(|_| String::from_utf8_lossy(n).into_owned())
                            }
                            _ => None,
                        };
                        match class {
                            Some(c) => format!(
                                "class {c} does not have a method \"{}\"",
                                String::from_utf8_lossy(m.as_bytes())
                            ),
                            None => "array callback has to contain indices 0 and 1".to_string(),
                        }
                    }
                    _ => "array callback has to contain indices 0 and 1".to_string(),
                }
            }
            other => format!("no array or string given ({})", crate::ops::value_name(other)),
        };
        format!("Failed to create closure from callable: {detail}")
    }

    /// `Closure::bind($closure, $newThis, $newScope)` and
    /// `$closure->bindTo($newThis, $newScope)`.
    ///
    /// `new_this` is the `?object` argument as passed. `new_scope` is the
    /// `object|string|null` argument as passed, or `None` when it was
    /// **omitted** (php's `"static"` default: keep the closure's scope).
    /// Returns the new `Value::Closure`, or `Value::Null` after emitting
    /// php's warning for a binding it refuses.
    pub fn closure_bind(
        &mut self,
        closure: &Closure,
        new_this: &Value,
        new_scope: Option<&Value>,
    ) -> Result<Value, Unwind> {
        let this = match &*new_this.deref() {
            Value::Object(o) => Some(o.clone()),
            _ => None,
        };
        let (cur_this, cur_scope, _cur_called) = self.closure_binding(closure);
        if this.is_some() && self.closure_is_static(closure) {
            self.warn("Cannot bind an instance to a static closure, this will be an error in PHP 9")?;
            return Ok(Value::Null);
        }
        if this.is_none() && cur_this.is_some() && self.closure_uses_this(closure) {
            self.warn("Cannot unbind $this of closure using $this, this will be an error in PHP 9")?;
            return Ok(Value::Null);
        }
        let scope = match new_scope.map(|v| v.deref().into_owned()) {
            // Omitted: php's `"static"` default keeps the current scope.
            None => cur_scope,
            Some(Value::Null) | Some(Value::Uninit) => None,
            Some(Value::Object(o)) => Some(o.class_id()),
            Some(Value::Str(s)) => {
                let name = s.as_bytes();
                // php spells the default scope `"static"`.
                if name.eq_ignore_ascii_case(b"static") {
                    cur_scope
                } else {
                    match self.class_by_name(name) {
                        Some(cid) => Some(cid),
                        None => {
                            let msg = format!(
                                "Class \"{}\" not found",
                                String::from_utf8_lossy(name)
                            );
                            self.warn(&msg)?;
                            return Ok(Value::Null);
                        }
                    }
                }
            }
            Some(other) => {
                return Err(Unwind::type_error(format!(
                    "Closure::bind(): Argument #3 ($newScope) must be of type object|string|null, {} given",
                    crate::ops::value_name(&other)
                )))
            }
        };
        // php refuses to scope a closure to an *internal* class (binding
        // `$this` to an instance of one is fine — only an explicitly given
        // scope is checked).
        if let Some(cid) = scope {
            if scope != cur_scope && self.classes[cid as usize].internal {
                let msg = format!(
                    "Cannot bind closure to scope of internal class {}, this will be an error in PHP 9",
                    self.classes[cid as usize].name_str()
                );
                self.warn(&msg)?;
                return Ok(Value::Null);
            }
        }
        Ok(self.rebind_closure(closure, this, scope))
    }

    /// Replace a closure's trailing `[$this, scope]` pair, keeping its
    /// function and every explicit capture (php's closures are immutable, so
    /// binding always produces a new one).
    fn rebind_closure(&self, c: &Closure, this: Option<Object>, scope: Option<u32>) -> Value {
        let caps = c.captures();
        let keep = caps.len().saturating_sub(CLOSURE_TAIL);
        let mut new: Vec<Value> = caps[..keep].to_vec();
        // An engine closure over `[$obj, 'm']` carries the receiver twice;
        // keep the target in step with the new binding.
        if c.func() == ENGINE_CLOSURE {
            if let Some(o) = &this {
                let retargeted = match new.first() {
                    Some(Value::Array(a)) => match a.get_deref(&ArrayKey::Int(1)) {
                        Some(Value::Str(m)) => {
                            let mut t = Array::new();
                            t.push(Value::Object(o.clone()));
                            t.push(Value::Str(m));
                            Some(Value::Array(t))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(v) = retargeted {
                    new[0] = v;
                }
            }
        }
        let called = this.as_ref().map(|o| o.class_id()).or(scope);
        new.push(this.map_or(Value::Null, Value::Object));
        new.push(scope.map_or(Value::Null, |c| Value::Int(i64::from(c))));
        new.push(called.map_or(Value::Null, |c| Value::Int(i64::from(c))));
        Value::Closure(Closure::new(c.func(), new))
    }

    /// `$closure->call($newThis, ...$args)` — bind `$this` *and* the scope to
    /// `new_this`'s class for one call. A static closure cannot be bound:
    /// php warns and yields null.
    pub fn closure_call(
        &mut self,
        closure: &Closure,
        new_this: &Object,
        args: &[Value],
    ) -> NativeResult {
        self.closure_call_named(closure, new_this, args, Vec::new())
    }

    /// [`Interp::closure_call`] with named arguments for the closure.
    pub(crate) fn closure_call_named(
        &mut self,
        closure: &Closure,
        new_this: &Object,
        args: &[Value],
        named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        if self.closure_is_static(closure) {
            self.warn("Cannot bind an instance to a static closure, this will be an error in PHP 9")?;
            return Ok(Value::Null);
        }
        let bound = self.rebind_closure(closure, Some(new_this.clone()), Some(new_this.class_id()));
        self.autoload_callable(&bound)?;
        let callable = self.resolve_callable(&bound)?;
        self.call_resolved_named(callable, args, named)
    }

    /// `$closure->bindTo(…)` / `->call(…)` / `->__invoke(…)` reached through
    /// `Op::InitMethodCall`: a closure value is not an `Object`, so the
    /// registered `Closure` class's instance methods cannot receive it as
    /// `$this`. This is the engine-side dispatch for that receiver.
    ///
    /// `__invoke` becomes a plain call of the closure. `bindTo` and `call`
    /// need their arguments, which are not staged yet at `Init` time, so they
    /// go through a trampoline of their own: the closure is staged as
    /// argument 0 and [`Interp::run_closure_method`] runs the engine-side
    /// implementation once `DoCall` has the whole window. Everything else is
    /// `Call to undefined method Closure::x()`.
    fn init_closure_method_call(
        &mut self,
        fi: usize,
        c: Closure,
        mname: Box<[u8]>,
    ) -> Result<(), Unwind> {
        let lname = mname.to_ascii_lowercase();
        if matches!(lname.as_slice(), b"__invoke") {
            // `$c->__invoke(...)` is `$c(...)`: resolve the closure itself.
            let callable = self.resolve_callable(&Value::Closure(c))?;
            let (target, this, scope, static_class) = match callable {
                Callable::Native(id) => (CallTarget::Native(id), None, None, None),
                Callable::NativeMethod { method, this } => {
                    let decl = method.decl;
                    let sc = this.as_ref().map(|o| o.class_id()).or(Some(decl));
                    (CallTarget::NativeMethod(method), this, Some(decl), sc)
                }
                Callable::User {
                    func,
                    this,
                    scope,
                    static_class,
                    closure,
                } => (CallTarget::User { func, closure }, this, scope, static_class),
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
                name: mname,
            });
            return Ok(());
        }
        if !matches!(lname.as_slice(), b"bindto" | b"call") {
            return Err(Unwind::error(format!(
                "Call to undefined method Closure::{}()",
                String::from_utf8_lossy(&mname)
            )));
        }
        let t = self.closure_method_trampoline(&lname);
        // The receiver is staged as argument 0; the `Send*` ops that follow
        // fill positions 1.., which is what `run_closure_method` expects.
        let args_base = self.stack.len();
        self.stack.push(Value::Closure(c));
        self.frames[fi].pending.push(PendingCall {
            target: CallTarget::NativeMethod(t),
            this: None,
            scope: None,
            static_class: self.well_known.closure,
            args_base,
            argc: 1,
            named: Vec::new(),
            new_obj: None,
            name: mname,
        });
        Ok(())
    }

    /// A trampoline descriptor for `$closure->bindTo(…)` / `->call(…)`.
    /// `name` is the lowercased method name; `decl` is only used for
    /// diagnostics.
    fn closure_method_trampoline(&self, lname: &[u8]) -> Rc<MethodDef> {
        Rc::new(MethodDef {
            name: Box::from(lname),
            body: MethodBody::Native(CLOSURE_METHOD),
            vis: Visibility::Public,
            is_static: false,
            is_abstract: false,
            is_final: false,
            decl: self.well_known.closure.unwrap_or(0),
        })
    }

    /// Whether a method descriptor is a `Closure` instance-method trampoline.
    pub(crate) fn is_closure_method(m: &MethodDef) -> bool {
        tagged(m, CLOSURE_TAG)
    }

    /// Run `$closure->bindTo(…)` / `$closure->call(…)`: `args[0]` is the
    /// receiver staged by [`Interp::init_closure_method_call`], the rest are
    /// the sent arguments.
    pub(crate) fn run_closure_method(
        &mut self,
        lname: &[u8],
        args: &[Value],
        extra_named: Vec<(Box<[u8]>, Value)>,
    ) -> NativeResult {
        let Some(Value::Closure(c)) = args.first().map(|v| v.deref().into_owned()) else {
            return Err(Unwind::error(
                "internal error: Closure method trampoline without a receiver",
            ));
        };
        let rest = &args[1..];
        match lname {
            b"bindto" => {
                if rest.is_empty() {
                    return Err(Unwind::argument_count_error(
                        "Closure::bindTo() expects at least 1 argument, 0 given",
                    ));
                }
                if rest.len() > 2 {
                    return Err(Unwind::argument_count_error(format!(
                        "Closure::bindTo() expects at most 2 arguments, {} given",
                        rest.len()
                    )));
                }
                self.closure_bind(&c, &rest[0], rest.get(1))
            }
            b"call" => {
                let Some(first) = rest.first() else {
                    return Err(Unwind::argument_count_error(
                        "Closure::call() expects at least 1 argument, 0 given",
                    ));
                };
                let o = match first.deref().into_owned() {
                    Value::Object(o) => o,
                    other => {
                        return Err(Unwind::type_error(format!(
                            "Closure::call(): Argument #1 ($newThis) must be of type object, {} given",
                            crate::ops::value_name(&other)
                        )))
                    }
                };
                self.closure_call_named(&c, &o, &rest[1..], extra_named)
            }
            other => Err(Unwind::error(format!(
                "Call to undefined method Closure::{}()",
                String::from_utf8_lossy(other)
            ))),
        }
    }

    /// `f(...)` / `$o->m(...)` / `A::m(...)` — `Op::MakeCallableClosure`.
    ///
    /// The operand encoding is the pending-call record the preceding
    /// `InitFCall` / `InitMethodCall` / `InitStaticCall` pushed (the op
    /// itself carries only `dst`, and no `Send*` may have run): the target,
    /// `$this`, the scope and the late-static-bound class are already
    /// resolved there, so this only has to turn them into a `Closure`.
    pub(crate) fn make_callable_closure(
        &mut self,
        _func: &Rc<FuncRt>,
        _base: usize,
        _op: &Op,
    ) -> Result<Value, Unwind> {
        let fi = self.frames.len() - 1;
        // An engine closure re-resolves its target on every call, so it must
        // capture the scope the `Init*` resolution was made in — not the
        // callee's own scope, which would widen what it may reach.
        let caller_scope = self.frames[fi].scope;
        let pending = self.frames[fi]
            .pending
            .pop()
            .ok_or_else(|| Unwind::error("internal error: MakeCallableClosure without an Init"))?;
        self.stack.truncate(pending.args_base);
        let PendingCall {
            target,
            this,
            scope,
            static_class,
            name,
            ..
        } = pending;
        Ok(match target {
            CallTarget::User { func, closure: None } => {
                // `B::m(...)` over a method declared in `A`: scope stays `A`
                // (visibility), the called class is `B` (late static binding).
                let tail = vec![
                    this.as_ref().map_or(Value::Null, |o| Value::Object(o.clone())),
                    scope.map_or(Value::Null, |c| Value::Int(i64::from(c))),
                    static_class
                        .or_else(|| this.as_ref().map(|o| o.class_id()))
                        .or(scope)
                        .map_or(Value::Null, |c| Value::Int(i64::from(c))),
                ];
                Value::Closure(Closure::new(func.id, tail))
            }
            CallTarget::User { closure: Some(c), .. } => self.rebind_closure(&c, this, scope),
            CallTarget::Native(id) => {
                let target = Value::string(self.natives[id.0 as usize].name.as_bytes());
                let caps = vec![target, Value::Null, Value::Null];
                Value::Closure(Closure::new(ENGINE_CLOSURE, caps))
            }
            CallTarget::NativeMethod(m) => {
                // Re-resolvable spelling of the method (a `__call` trampoline
                // included: `[$obj, 'zz']` re-enters the same fallback, and
                // `'C::zz'` the `__callStatic` one).
                let target = match &this {
                    Some(o) => {
                        let mut arr = Array::new();
                        arr.push(Value::Object(o.clone()));
                        arr.push(Value::string(&name));
                        Value::Array(arr)
                    }
                    None => {
                        let mut s = self.classes[m.decl as usize].name.to_vec();
                        s.extend_from_slice(b"::");
                        s.extend_from_slice(&name);
                        Value::string(&s)
                    }
                };
                let caps = vec![
                    target,
                    this.as_ref().map_or(Value::Null, |o| Value::Object(o.clone())),
                    caller_scope.map_or(Value::Null, |c| Value::Int(i64::from(c))),
                    static_class
                        .or_else(|| this.as_ref().map(|o| o.class_id()))
                        .or(caller_scope)
                        .map_or(Value::Null, |c| Value::Int(i64::from(c))),
                ];
                Value::Closure(Closure::new(ENGINE_CLOSURE, caps))
            }
            CallTarget::NoCtor => {
                return Err(Unwind::error(
                    "internal error: `new` is not a first-class callable",
                ))
            }
        })
    }
}
