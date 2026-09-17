//! The core `Throwable` hierarchy (php-src `Zend/zend_exceptions.c`,
//! `zend_exceptions.stub.php`, `zend_interfaces.c` for `Stringable`):
//! `Throwable` (an interface extending `Stringable`), `Exception` and
//! `Error` with php's exact property set, the `Error` subclasses the engine
//! throws (`TypeError`, `ValueError`, `ArithmeticError`,
//! `DivisionByZeroError`, `ArgumentCountError`, `AssertionError`,
//! `UnhandledMatchError`, `CompileError`, `ParseError`), `ErrorException`
//! and the `Exception` subclasses of the core extensions (`JsonException`,
//! `RequestParseBodyException`).
//!
//! `file`/`line`/`trace` are filled at `new` by the engine's `native_init`
//! hook (`Interp::throwable_init`); the trace/`__toString` renderings live in
//! the runtime (`throwable.rs`) so uncaught rendering and user subclasses
//! share them.

use rphp_runtime::{nm, Ctx, Interp, NativeMethod, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Object, Value};

/// A `final public` native method row.
fn final_method(min: u8, max: Option<u8>, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: false,
        is_final: true,
    }
}

/// The receiver of an instance method (never absent for the methods here).
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// `Exception::__construct(string $message = "", int $code = 0, ?Throwable $previous = null)`
fn exception_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let class = ctx.class_name_of(o);
    if let Some(m) = args.first() {
        let m = m.deref().into_owned();
        match &m {
            Value::Array(_) | Value::Object(_) => {
                return Err(Unwind::type_error(format!(
                    "{class}::__construct(): Argument #1 ($message) must be of type string, {} given",
                    rphp_runtime::value_name(&m)
                )))
            }
            _ => o.set(b"message", Value::Str(rphp_value::Str::from_vec(m.to_php_bytes()))),
        }
    }
    if let Some(c) = args.get(1) {
        let c = c.deref().into_owned();
        match &c {
            Value::Int(i) => o.set(b"code", Value::Int(*i)),
            Value::Float(_) | Value::Bool(_) | Value::Null => o.set(b"code", Value::Int(c.to_int())),
            Value::Str(_) if c.is_numeric() => o.set(b"code", Value::Int(c.to_int())),
            _ => {
                return Err(Unwind::type_error(format!(
                    "{class}::__construct(): Argument #2 ($code) must be of type int, {} given",
                    rphp_runtime::value_name(&c)
                )))
            }
        }
    }
    if let Some(p) = args.get(2) {
        match &*p.deref() {
            Value::Null | Value::Uninit => {}
            Value::Object(prev) if ctx.is_throwable(prev) => o.set(b"previous", Value::Object(prev.clone())),
            other => {
                return Err(Unwind::type_error(format!(
                    "{class}::__construct(): Argument #3 ($previous) must be of type ?Throwable, {} given",
                    rphp_runtime::value_name(other)
                )))
            }
        }
    }
    Ok(Value::Null)
}

/// `ErrorException::__construct(string $message = "", int $code = 0, int $severity = E_ERROR, ?string $filename = null, ?int $line = null, ?Throwable $previous = null)`
fn error_exception_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let head_len = args.len().min(2);
    let mut head: Vec<Value> = args[..head_len].to_vec();
    if let Some(p) = args.get(5) {
        while head.len() < 2 {
            head.push(Value::Uninit);
        }
        head.push(p.clone());
    }
    let mut head: Vec<Value> = head
        .into_iter()
        .map(|v| if v.is_uninit() { Value::Null } else { v })
        .collect();
    // A skipped `$code` must not become a null-typed argument.
    if head.len() >= 2 && matches!(head[1], Value::Null) && args.get(1).is_none() {
        head[1] = Value::Int(0);
    }
    exception_construct(ctx, o, &mut head)?;
    if let Some(s) = args.get(2) {
        obj.set(b"severity", Value::Int(s.to_int()));
    }
    if let Some(f) = args.get(3) {
        if !matches!(*f.deref(), Value::Null) {
            obj.set(b"file", Value::Str(rphp_value::Str::from_vec(f.to_php_bytes())));
        }
    }
    if let Some(l) = args.get(4) {
        if !matches!(*l.deref(), Value::Null) {
            obj.set(b"line", Value::Int(l.to_int()));
        }
    }
    Ok(Value::Null)
}

/// `__wakeup(): void` — nothing to restore.
fn exception_wakeup(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// `private __clone()`: exceptions are not cloneable.
fn exception_clone(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Trying to clone an uncloneable object of class Exception"))
}

fn get_message(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"message").unwrap_or_else(|| Value::string(b"")))
}

fn get_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"code").unwrap_or(Value::Int(0)))
}

fn get_file(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"file").unwrap_or_else(|| Value::string(b"")))
}

fn get_line(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"line").unwrap_or(Value::Int(0)))
}

fn get_trace(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"trace").unwrap_or_else(Value::empty_array))
}

fn get_previous(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"previous").unwrap_or(Value::Null))
}

fn get_trace_as_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::string(ctx.throwable_trace_string(o).as_bytes()))
}

fn exception_to_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::string(ctx.throwable_to_string(o).as_bytes()))
}

fn get_severity(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"severity").unwrap_or(Value::Int(1)))
}

/// The `native_init` hook: `file`, `line`, `trace` from the `new` site.
fn throwable_init(it: &mut Interp, o: &Object) -> Result<(), Unwind> {
    it.throwable_init(o)
}

/// Declare `Exception` or `Error` (identical shape, both root classes
/// implementing `Throwable`).
fn declare_root(r: &mut Registry, name: &str) {
    r.class(name)
        .implements(&["Throwable"])
        .prop("message", Visibility::Protected, Value::string(b""))
        .prop("string", Visibility::Private, Value::string(b""))
        .prop("code", Visibility::Protected, Value::Int(0))
        .prop("file", Visibility::Protected, Value::string(b""))
        .prop("line", Visibility::Protected, Value::Int(0))
        .prop("trace", Visibility::Private, Value::empty_array())
        .prop("previous", Visibility::Private, Value::Null)
        .method_vis("__clone", Visibility::Private, nm!(0, Some(0), exception_clone))
        .method("__construct", nm!(0, Some(3), exception_construct))
        .method("__wakeup", nm!(0, Some(0), exception_wakeup))
        .method("getMessage", final_method(0, Some(0), get_message))
        .method("getCode", final_method(0, Some(0), get_code))
        .method("getFile", final_method(0, Some(0), get_file))
        .method("getLine", final_method(0, Some(0), get_line))
        .method("getTrace", final_method(0, Some(0), get_trace))
        .method("getPrevious", final_method(0, Some(0), get_previous))
        .method("getTraceAsString", final_method(0, Some(0), get_trace_as_string))
        .method("__toString", nm!(0, Some(0), exception_to_string))
        .native_init(throwable_init)
        .finish();
}

/// Declare a plain subclass (no members of its own).
fn declare_sub(r: &mut Registry, name: &str, parent: &str) {
    r.class(name).extends(parent).finish();
}

/// Register the hierarchy (call once, before `spl_exceptions`).
pub(crate) fn register_classes(r: &mut Registry) {
    r.interface("Stringable")
        .abstract_method("__toString", nm!(0, Some(0), exception_to_string))
        .finish();
    r.interface("Throwable")
        .implements(&["Stringable"])
        .abstract_method("getMessage", nm!(0, Some(0), get_message))
        .abstract_method("getCode", nm!(0, Some(0), get_code))
        .abstract_method("getFile", nm!(0, Some(0), get_file))
        .abstract_method("getLine", nm!(0, Some(0), get_line))
        .abstract_method("getTrace", nm!(0, Some(0), get_trace))
        .abstract_method("getPrevious", nm!(0, Some(0), get_previous))
        .abstract_method("getTraceAsString", nm!(0, Some(0), get_trace_as_string))
        .finish();
    declare_root(r, "Exception");
    declare_root(r, "Error");
    r.class("ErrorException")
        .extends("Exception")
        .prop("severity", Visibility::Protected, Value::Int(1))
        .method("__construct", nm!(0, Some(6), error_exception_construct))
        .method("getSeverity", final_method(0, Some(0), get_severity))
        .finish();
    declare_sub(r, "CompileError", "Error");
    declare_sub(r, "ParseError", "CompileError");
    declare_sub(r, "TypeError", "Error");
    declare_sub(r, "ArgumentCountError", "TypeError");
    declare_sub(r, "ValueError", "Error");
    declare_sub(r, "ArithmeticError", "Error");
    declare_sub(r, "DivisionByZeroError", "ArithmeticError");
    declare_sub(r, "AssertionError", "Error");
    declare_sub(r, "UnhandledMatchError", "Error");
    declare_sub(r, "RequestParseBodyException", "Exception");
    declare_sub(r, "JsonException", "Exception");
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::interp;

    #[test]
    fn hierarchy_is_registered_with_php_shape() {
        let it = interp();
        let ex = it.class_by_name(b"Exception").unwrap();
        let err = it.class_by_name(b"error").unwrap();
        let throwable = it.class_by_name(b"Throwable").unwrap();
        let dbz = it.class_by_name(b"DivisionByZeroError").unwrap();
        assert!(it.instanceof_class(dbz, err));
        assert!(it.instanceof_class(dbz, throwable));
        assert!(!it.instanceof_class(dbz, ex));
        assert!(it.instanceof_class(ex, it.class_by_name(b"Stringable").unwrap()));
        let c = it.class(ex);
        let names: Vec<&[u8]> = c.props.iter().map(|p| p.name.as_ref()).collect();
        assert_eq!(names, [b"message" as &[u8], b"string", b"code", b"file", b"line", b"trace", b"previous"]);
        assert!(c.method(b"getmessage").unwrap().is_final);
        assert_eq!(it.well_known.type_error, it.class_by_name(b"TypeError"));
    }

    #[test]
    fn engine_faults_materialize_and_render() {
        let mut it = interp();
        let err = it.call_function(b"intdiv", &[Value::Int(1), Value::Int(0)]).unwrap_err();
        let u = it.materialize_unwind(err);
        let rphp_runtime::Unwind::Throw(o) = &u else { panic!("expected an object") };
        assert_eq!(it.class_name_of(o), "DivisionByZeroError");
        assert_eq!(it.throwable_str(o, b"message"), "Division by zero");
        // `call_function` enters the native directly, without the call frame
        // the VM would push, so the trace holds only `{main}`. The *rendering*
        // of a native frame is covered end to end by
        // `examples/tier-a/lang/faults.php`, where the same fault through the
        // interpreter prints php's `#0 Command line code(1): intdiv(1, 0)`.
        assert_eq!(
            it.throwable_to_string(o),
            "DivisionByZeroError: Division by zero in Command line code:0\nStack trace:\n#0 {main}"
        );
    }
}
