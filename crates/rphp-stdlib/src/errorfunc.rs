//! Error-handling builtins (php-src `Zend/zend_builtin_functions.c`,
//! `ext/standard/basic_functions.c: error_log`): the user handler stacks,
//! `error_reporting`, `trigger_error`, `error_get_last`, `error_log`.

use rphp_runtime::{
    nf, Ctx, ErrLevel, NativeFn, NativeResult, Unwind, E_ALL, E_USER_DEPRECATED, E_USER_ERROR,
    E_USER_NOTICE, E_USER_WARNING,
};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("set_error_handler", 1, Some(2), set_error_handler),
    nf!("restore_error_handler", 0, Some(0), restore_error_handler),
    nf!("set_exception_handler", 1, Some(1), set_exception_handler),
    nf!("restore_exception_handler", 0, Some(0), restore_exception_handler),
    nf!("error_reporting", 0, Some(1), error_reporting),
    nf!("trigger_error", 1, Some(2), trigger_error),
    nf!("user_error", 1, Some(2), trigger_error),
    nf!("error_get_last", 0, Some(0), error_get_last),
    nf!("error_clear_last", 0, Some(0), error_clear_last),
    nf!("error_log", 1, Some(4), error_log),
];

/// `set_error_handler(?callable $callback, int $error_levels = E_ALL): ?callable`
/// — returns the previously active handler (`null` when none).
pub(crate) fn set_error_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let prev = ctx.current_error_handler().map_or(Value::Null, |(h, _)| h);
    let mask = args.get(1).map_or(E_ALL, Value::to_int);
    let cb = args[0].deref().into_owned();
    ctx.error_handler.push((cb, mask));
    Ok(prev)
}

/// `restore_error_handler(): true`
pub(crate) fn restore_error_handler(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.error_handler.pop();
    Ok(Value::Bool(true))
}

/// `set_exception_handler(?callable $callback): ?callable` — stored; invoked
/// for uncaught exceptions once they are objects (E5).
pub(crate) fn set_exception_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let prev = ctx.exception_handler.last().cloned().unwrap_or(Value::Null);
    ctx.exception_handler.push(args[0].deref().into_owned());
    Ok(prev)
}

/// `restore_exception_handler(): true`
pub(crate) fn restore_exception_handler(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.exception_handler.pop();
    Ok(Value::Bool(true))
}

/// `error_reporting(?int $error_level = null): int` — the previous mask.
pub(crate) fn error_reporting(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let old = ctx.effective_error_reporting();
    if let Some(v) = args.first() {
        if !matches!(*v.deref(), Value::Null) {
            ctx.set_error_reporting(v.to_int());
        }
    }
    Ok(Value::Int(old))
}

/// `trigger_error(string $message, int $error_level = E_USER_NOTICE): true`
pub(crate) fn trigger_error(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let message = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let level = match args.get(1).map_or(E_USER_NOTICE, Value::to_int) {
        E_USER_NOTICE => ErrLevel::UserNotice,
        E_USER_WARNING => ErrLevel::UserWarning,
        E_USER_DEPRECATED => ErrLevel::UserDeprecated,
        E_USER_ERROR => {
            ctx.deprecated(
                "Passing E_USER_ERROR to trigger_error() is deprecated since 8.4, throw an exception or call exit with a string message instead",
            )?;
            ErrLevel::UserError
        }
        _ => {
            return Err(Unwind::value_error(
                "trigger_error(): Argument #2 ($error_level) must be one of E_USER_ERROR, E_USER_WARNING, E_USER_NOTICE, or E_USER_DEPRECATED",
            ))
        }
    };
    ctx.emit_error(level, &message)?;
    Ok(Value::Bool(true))
}

/// `error_get_last(): ?array`
pub(crate) fn error_get_last(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(match &ctx.last_error {
        Some(e) => {
            let mut a = Array::new();
            a.set(ArrayKey::str(b"type"), Value::Int(e.kind));
            a.set(ArrayKey::str(b"message"), Value::string(e.message.as_bytes()));
            a.set(ArrayKey::str(b"file"), Value::string(e.file.as_bytes()));
            a.set(ArrayKey::str(b"line"), Value::Int(e.line as i64));
            Value::Array(a)
        }
        None => Value::Null,
    })
}

/// `error_clear_last(): void`
pub(crate) fn error_clear_last(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.last_error = None;
    Ok(Value::Null)
}

/// `error_log(string $message, int $message_type = 0, ?string $destination = null, ?string $additional_headers = null): bool`
/// — type 0/4 write the line to stderr (the CLI's log), type 3 appends to
/// `$destination`; mail (1) and the removed type 2 are `false`.
pub(crate) fn error_log(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let message = args[0].to_php_bytes();
    let kind = args.get(1).map_or(0, Value::to_int);
    match kind {
        0 | 4 => {
            use std::io::Write;
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(&message);
            let _ = err.write_all(b"\n");
            let _ = err.flush();
            Ok(Value::Bool(true))
        }
        3 => {
            let dest = args.get(2).map(Value::to_php_bytes).unwrap_or_default();
            let path = String::from_utf8_lossy(&dest).into_owned();
            let mut f = match std::fs::OpenOptions::new().create(true).append(true).open(path) {
                Ok(f) => f,
                Err(_) => return Ok(Value::Bool(false)),
            };
            use std::io::Write;
            Ok(Value::Bool(f.write_all(&message).is_ok()))
        }
        _ => Ok(Value::Bool(false)),
    }
}

#[cfg(test)]
mod tests {
    use rphp_runtime::{ErrorKind, E_ALL, E_WARNING};
    use rphp_value::{ArrayKey, Value};

    use crate::tests::interp;

    #[test]
    fn handler_stack_and_error_reporting() {
        let mut it = interp();
        let h = Value::string(b"strlen");
        assert_eq!(it.call_function(b"set_error_handler", std::slice::from_ref(&h)).unwrap(), Value::Null);
        assert_eq!(it.call_function(b"set_error_handler", &[Value::Null]).unwrap(), h);
        assert_eq!(it.call_function(b"restore_error_handler", &[]).unwrap(), Value::Bool(true));
        assert_eq!(it.current_error_handler().map(|(v, _)| v), Some(h));
        assert_eq!(it.call_function(b"error_reporting", &[]).unwrap(), Value::Int(E_ALL));
        assert_eq!(it.call_function(b"error_reporting", &[Value::Int(E_WARNING)]).unwrap(), Value::Int(E_ALL));
        assert_eq!(it.call_function(b"error_reporting", &[Value::Null]).unwrap(), Value::Int(E_WARNING));
        assert_eq!(it.ini_get("error_reporting"), Some("2"));
    }

    #[test]
    fn trigger_error_levels_and_last_error() {
        let mut it = interp();
        it.call_function(b"trigger_error", &[Value::string(b"n")]).unwrap();
        it.call_function(b"trigger_error", &[Value::string(b"w"), Value::Int(512)]).unwrap();
        it.call_function(b"user_error", &[Value::string(b"d"), Value::Int(16384)]).unwrap();
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "\nNotice: n in Command line code on line 0\n\
             \nWarning: w in Command line code on line 0\n\
             \nDeprecated: d in Command line code on line 0\n"
        );
        let Value::Array(last) = it.call_function(b"error_get_last", &[]).unwrap() else { panic!() };
        assert_eq!(last.get(&ArrayKey::str(b"type")).unwrap(), &Value::Int(16384));
        assert_eq!(last.get(&ArrayKey::str(b"message")).unwrap(), &Value::string(b"d"));
        it.call_function(b"error_clear_last", &[]).unwrap();
        assert_eq!(it.call_function(b"error_get_last", &[]).unwrap(), Value::Null);
        let err = it.call_function(b"trigger_error", &[Value::string(b"x"), Value::Int(1)]).unwrap_err();
        assert_eq!(err.kind(), Some(ErrorKind::ValueError));
        // E_USER_ERROR: deprecation, then a fatal that ends the request.
        let err = it.call_function(b"trigger_error", &[Value::string(b"fatal!"), Value::Int(256)]).unwrap_err();
        assert!(matches!(err, rphp_runtime::Unwind::Exit(255)));
        let out = String::from_utf8(it.take_test_output()).unwrap();
        assert!(out.starts_with("\nDeprecated: Passing E_USER_ERROR to trigger_error() is deprecated since 8.4"), "{out}");
        // A bare test interpreter has no `{main}` frame, so the backtrace is
        // the bottom line only; through the engine it reads
        // `#0 file(line): trigger_error('fatal!', 256)` first.
        assert!(out.contains("\nFatal error: fatal! in Command line code on line 0\nStack trace:\n#0 {main}\n"), "{out}");
    }
}
