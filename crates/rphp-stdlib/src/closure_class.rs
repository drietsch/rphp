//! The `Closure` class (php-src `Zend/zend_closures.c`): `final`, not
//! instantiable, with `bind` / `bindTo` / `call` / `fromCallable` /
//! `getCurrent` / `__invoke`.
//!
//! **How the work is split.** A closure in this runtime is a
//! [`Value::Closure`](rphp_value::Value::Closure), not an
//! [`Object`](rphp_value::Object), so the native-method ABI
//! (`fn(&mut Ctx, Option<&Object>, &mut [Value])`) cannot deliver `$this` to
//! an *instance* method of this class. The engine therefore intercepts a
//! closure receiver before the class table is consulted
//! (`Interp::init_closure_method_call`, `rphp-runtime/src/methods.rs`) and
//! runs `bindTo` / `call` / `__invoke` itself; the bodies here are
//! unreachable and say so.
//!
//! The two **static** methods do arrive through this class, because
//! `Op::InitStaticCall` resolves `Closure::bind` / `Closure::fromCallable`
//! against the registered method table. They validate their arguments with
//! php's exact texts and then hand off to the engine
//! ([`Interp::closure_bind`], [`Interp::closure_from_callable`]), which owns
//! the capture/scope convention.
//!
//! Registering the class is also what fills `WellKnown::closure`, which the
//! engine's closure dispatch reads, and what makes `get_class($fn)`,
//! `method_exists('Closure', …)` and `new Closure` behave like php.

use rphp_runtime::{nm, ClassFlags, Ctx, Interp, NativeMethod, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

/// A `static` native method row.
fn static_method(
    min: u8,
    max: Option<u8>,
    params: &'static [&'static str],
    f: rphp_runtime::NativeMethodHandler,
) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params,
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

/// An instance native method row with parameter names.
fn method(
    min: u8,
    max: Option<u8>,
    params: &'static [&'static str],
    f: rphp_runtime::NativeMethodHandler,
) -> NativeMethod {
    NativeMethod {
        is_static: false,
        ..static_method(min, max, params, f)
    }
}

/// The failure an instance method raises if dispatch ever reaches it: the
/// engine takes a closure receiver off this path, so getting here means a
/// call route bypassed `Interp::init_closure_method_call`. Deliberately not
/// a php message — it must never be mistaken for one.
fn unreachable_instance_method(name: &str) -> Unwind {
    Unwind::error(format!(
        "internal error: Closure::{name}() was dispatched through the class table; \
         a closure receiver is handled by the engine (methods.rs)"
    ))
}

/// The `native_init` hook: `new Closure` is refused with php's text. It runs
/// on `new` only — [`Interp::instantiate`] skips it — so nothing that builds
/// a `Closure` instance internally is affected.
fn not_instantiable(_: &mut Interp, _: &Object) -> Result<(), Unwind> {
    Err(Unwind::error("Instantiation of class Closure is not allowed"))
}

/// `Closure::__construct()` — php refuses in its create handler; this is the
/// same message for any other route in.
fn closure_construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Instantiation of class Closure is not allowed"))
}

/// `Closure::bind(Closure $closure, ?object $newThis, object|string|null $newScope = "static"): ?Closure`
///
/// Arguments #1 and #2 are type-checked here (the engine entry point takes
/// them already narrowed); #3's `TypeError`, the `Class "X" not found` and
/// static-closure warnings, and the rebinding itself are
/// [`Interp::closure_bind`]. An omitted `$newScope` is php's `"static"`
/// default — *keep the current scope* — which is why it is passed as `None`
/// rather than as a null value.
fn closure_bind(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let closure = match args[0].deref().into_owned() {
        Value::Closure(c) => c,
        other => {
            return Err(Unwind::type_error(format!(
                "Closure::bind(): Argument #1 ($closure) must be of type Closure, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let new_this = args[1].deref().into_owned();
    match &new_this {
        Value::Null | Value::Uninit | Value::Object(_) => {}
        other => {
            return Err(Unwind::type_error(format!(
                "Closure::bind(): Argument #2 ($newThis) must be of type ?object, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    }
    let new_scope = args.get(2).cloned();
    ctx.closure_bind(&closure, &new_this, new_scope.as_ref())
}

/// `Closure::fromCallable(callable $callback): Closure`
///
/// [`Interp::closure_from_callable`] resolves the callable in the *calling*
/// scope (a native frame is transparent to `Interp::calling_scope`), so a
/// `'C::privateMethod'` spelling keeps php's visibility rules, and raises
/// php's `TypeError: Failed to create closure from callable: …`.
fn closure_from_callable(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let callback = args[0].clone();
    ctx.closure_from_callable(&callback)
}

/// `Closure::getCurrent(): Closure` (php 8.5) — the closure whose body is
/// running. Not implemented: it needs the executing frame's closure, which
/// only the engine can hand out, and no entry point exists for it yet.
fn closure_get_current(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "rphp: Closure::getCurrent() is not implemented yet (needs the running frame's closure from the engine)",
    ))
}

/// `Closure::bindTo(?object $newThis, object|string|null $newScope = "static"): ?Closure`
fn closure_bind_to(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(unreachable_instance_method("bindTo"))
}

/// `Closure::call(object $newThis, mixed ...$args): mixed`
fn closure_call(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(unreachable_instance_method("call"))
}

/// `Closure::__invoke(mixed ...$args): mixed`
fn closure_invoke(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(unreachable_instance_method("__invoke"))
}

/// Register `Closure` (php's method order: `__construct`, `bind`, `bindTo`,
/// `call`, `fromCallable`, `getCurrent`, `__invoke`).
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("Closure")
        .flags(ClassFlags::FINAL)
        .method("__construct", nm!(0, Some(0), closure_construct))
        .method(
            "bind",
            static_method(2, Some(3), &["closure", "newThis", "newScope"], closure_bind),
        )
        .method(
            "bindTo",
            method(1, Some(2), &["newThis", "newScope"], closure_bind_to),
        )
        .method("call", method(1, None, &["newThis"], closure_call))
        .method(
            "fromCallable",
            static_method(1, Some(1), &["callback"], closure_from_callable),
        )
        .method("getCurrent", static_method(0, Some(0), &[], closure_get_current))
        .method("__invoke", method(0, None, &[], closure_invoke))
        .native_init(not_instantiable)
        .finish();
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::interp;

    #[test]
    fn closure_has_phps_shape() {
        let mut it = interp();
        let cid = it.class_by_name(b"Closure").unwrap();
        // The engine's closure dispatch reads this slot.
        assert_eq!(it.well_known.closure, Some(cid));
        {
            let c = it.class(cid);
            assert!(c.flags.contains(rphp_runtime::ClassFlags::FINAL));
            assert!(c.method(b"bind").unwrap().is_static);
            assert!(c.method(b"fromcallable").unwrap().is_static);
            assert!(c.method(b"getcurrent").unwrap().is_static);
            assert!(!c.method(b"bindto").unwrap().is_static);
            assert!(!c.method(b"__invoke").unwrap().is_static);
        }
        // `new Closure` is php's `Error`, not "cannot instantiate abstract".
        let err = it.new_object(cid).unwrap_err();
        assert_eq!(
            err.message(),
            Some("Instantiation of class Closure is not allowed")
        );
    }

    #[test]
    fn bind_validates_its_arguments_with_phps_texts() {
        let mut it = interp();
        let cid = it.class_by_name(b"Closure").unwrap();
        let err = it
            .call_static_method(cid, b"bind", &[Value::Int(1), Value::Null])
            .unwrap_err();
        assert_eq!(
            err.message(),
            Some("Closure::bind(): Argument #1 ($closure) must be of type Closure, int given")
        );
        let err = it
            .call_static_method(cid, b"bind", &[Value::Int(1)])
            .unwrap_err();
        assert_eq!(err.kind(), Some(rphp_runtime::ErrorKind::ArgumentCountError));
    }

    #[test]
    fn from_callable_wraps_a_builtin() {
        let mut it = interp();
        let cid = it.class_by_name(b"Closure").unwrap();
        let c = it
            .call_static_method(cid, b"fromCallable", &[Value::string(b"strtoupper")])
            .unwrap();
        assert!(matches!(c, Value::Closure(_)));
        assert_eq!(
            it.call_value(&c, &[Value::string(b"hi")]).unwrap(),
            Value::string(b"HI")
        );
        let err = it
            .call_static_method(cid, b"fromCallable", &[Value::string(b"no_such_function")])
            .unwrap_err();
        assert_eq!(err.kind(), Some(rphp_runtime::ErrorKind::TypeError));
    }
}
