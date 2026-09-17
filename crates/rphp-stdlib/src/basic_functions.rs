//! Engine-facing builtins from php-src `ext/standard/basic_functions.c` and
//! `Zend/zend_builtin_functions.c`: ini access, constants, shutdown
//! functions, SAPI/version queries.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::Value;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("ini_get", 1, Some(1), ini_get),
    nf!("ini_set", 2, Some(2), ini_set),
    nf!("ini_alter", 2, Some(2), ini_set),
    nf!("ini_restore", 1, Some(1), ini_restore),
    nf!("define", 2, Some(3), define),
    nf!("defined", 1, Some(1), defined),
    nf!("constant", 1, Some(1), constant),
    nf!("register_shutdown_function", 1, None, register_shutdown_function),
    nf!("php_sapi_name", 0, Some(0), php_sapi_name),
    nf!("phpversion", 0, Some(1), phpversion),
    nf!("function_exists", 1, Some(1), function_exists),
    nf!("set_time_limit", 1, Some(1), set_time_limit),
];

fn name_of(v: &Value) -> String {
    String::from_utf8_lossy(&v.to_php_bytes()).into_owned()
}

/// `ini_get(string $option): string|false`
pub(crate) fn ini_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(match ctx.ini_get(&name_of(&args[0])) {
        Some(v) => Value::string(v.as_bytes()),
        None => Value::Bool(false),
    })
}

/// php's string form of an `ini_set` value argument (`true` ⇒ `"1"`,
/// `false`/`null` ⇒ `""`).
fn ini_value(v: &Value) -> Vec<u8> {
    match &*v.deref() {
        Value::Null | Value::Bool(false) => Vec::new(),
        Value::Bool(true) => b"1".to_vec(),
        other => other.to_php_bytes(),
    }
}

/// `ini_set(string $option, string|int|float|bool|null $value): string|false`
pub(crate) fn ini_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let value = String::from_utf8_lossy(&ini_value(&args[1])).into_owned();
    Ok(match ctx.ini_set(&name_of(&args[0]), &value) {
        Some(old) => Value::string(old.as_bytes()),
        None => Value::Bool(false),
    })
}

/// `ini_restore(string $option): void`
pub(crate) fn ini_restore(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = name_of(&args[0]);
    ctx.ini.restore(&name);
    if name == "error_reporting" {
        let v = ctx.ini.get("error_reporting").unwrap_or("").to_string();
        ctx.error_reporting = rphp_runtime::parse_error_reporting(&v).unwrap_or(rphp_runtime::E_ALL);
    }
    Ok(Value::Null)
}

/// `define(string $constant_name, mixed $value, bool $case_insensitive = false): bool`
pub(crate) fn define(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if args.get(2).is_some_and(Value::to_bool) {
        ctx.warn("define(): Argument #3 ($case_insensitive) is ignored since declaration of case-insensitive constants is no longer supported")?;
    }
    let name = args[0].to_php_bytes();
    let value = args[1].deref().into_owned();
    if !ctx.define(&name, value) {
        ctx.warn(&format!(
            "Constant {} already defined, this will be an error in PHP 9",
            String::from_utf8_lossy(&name)
        ))?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

/// `defined(string $constant_name): bool`
pub(crate) fn defined(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(ctx.defined(&args[0].to_php_bytes())))
}

/// `constant(string $name): mixed` — `Error: Undefined constant "X"` when
/// unknown (class constants `A::B` arrive with the class model).
pub(crate) fn constant(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    let name = name.strip_prefix(b"\\").unwrap_or(&name);
    ctx.constant(name).ok_or_else(|| {
        Unwind::error(format!("Undefined constant \"{}\"", String::from_utf8_lossy(name)))
    })
}

/// `register_shutdown_function(callable $callback, mixed ...$args): void`
pub(crate) fn register_shutdown_function(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let cb = args[0].deref().into_owned();
    let rest: Vec<Value> = args[1..].iter().map(|v| v.deref().into_owned()).collect();
    ctx.shutdown.push((cb, rest));
    Ok(Value::Null)
}

/// `php_sapi_name(): string`
pub(crate) fn php_sapi_name(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(ctx.sapi.name().as_bytes()))
}

/// `phpversion(?string $extension = null): string|false`
pub(crate) fn phpversion(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match args.first() {
        Some(v) if !matches!(*v.deref(), Value::Null) => Ok(Value::Bool(false)),
        _ => Ok(Value::string(b"8.5.0")),
    }
}

/// `function_exists(string $function): bool` — user functions of the loaded
/// program and registered natives.
pub(crate) fn function_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    let name = name.strip_prefix(b"\\").unwrap_or(&name);
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    let user = ctx.module().is_some_and(|m| m.func_by_name(name).is_some());
    Ok(Value::Bool(user || ctx.native_by_name(name).is_some()))
}

/// `set_time_limit(int $seconds): bool` — accepted; the engine has no
/// execution timer yet.
pub(crate) fn set_time_limit(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use rphp_runtime::{ErrorKind, Registry};
    use rphp_value::Value;

    use crate::tests::interp;

    #[test]
    fn define_constant_defined() {
        let mut it = interp();
        Registry(&mut it).constant("PHP_EOL", Value::string(b"\n"));
        assert_eq!(it.call_function(b"defined", &[Value::string(b"PHP_EOL")]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_function(b"constant", &[Value::string(b"PHP_EOL")]).unwrap(), Value::string(b"\n"));
        assert_eq!(it.call_function(b"define", &[Value::string(b"X"), Value::Int(1)]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_function(b"define", &[Value::string(b"X"), Value::Int(2)]).unwrap(), Value::Bool(false));
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "\nWarning: Constant X already defined, this will be an error in PHP 9 in Command line code on line 0\n"
        );
        assert_eq!(it.call_function(b"constant", &[Value::string(b"X")]).unwrap(), Value::Int(1));
        let err = it.call_function(b"constant", &[Value::string(b"NOPE")]).unwrap_err();
        assert_eq!(err.kind(), Some(ErrorKind::Error));
        assert_eq!(err.message(), Some("Undefined constant \"NOPE\""));
    }

    #[test]
    fn ini_get_set_and_friends() {
        let mut it = interp();
        assert_eq!(it.call_function(b"ini_get", &[Value::string(b"precision")]).unwrap(), Value::string(b"14"));
        assert_eq!(
            it.call_function(b"ini_set", &[Value::string(b"precision"), Value::Int(10)]).unwrap(),
            Value::string(b"14")
        );
        assert_eq!(it.call_function(b"ini_get", &[Value::string(b"precision")]).unwrap(), Value::string(b"10"));
        assert_eq!(it.call_function(b"ini_set", &[Value::string(b"nope.x"), Value::Int(1)]).unwrap(), Value::Bool(false));
        assert_eq!(it.call_function(b"ini_get", &[Value::string(b"nope.x")]).unwrap(), Value::Bool(false));
        assert_eq!(
            it.call_function(b"ini_set", &[Value::string(b"display_errors"), Value::Bool(false)]).unwrap(),
            Value::string(b"1")
        );
        assert_eq!(it.call_function(b"ini_get", &[Value::string(b"display_errors")]).unwrap(), Value::string(b""));
        assert_eq!(it.call_function(b"php_sapi_name", &[]).unwrap(), Value::string(b"embed"));
        assert_eq!(it.call_function(b"phpversion", &[]).unwrap(), Value::string(b"8.5.0"));
        assert_eq!(it.call_function(b"function_exists", &[Value::string(b"StrLen")]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_function(b"function_exists", &[Value::string(b"nope")]).unwrap(), Value::Bool(false));
        it.call_function(b"register_shutdown_function", &[Value::string(b"strlen"), Value::string(b"x")]).unwrap();
        assert_eq!(it.shutdown.len(), 1);
    }
}
