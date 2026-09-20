//! Output-control builtins (php-src `main/output.c`): the `ob_*` family and
//! `flush()`. The levels live in the interpreter's output stack; user
//! handlers are invoked through `Interp::ob_*` with php's phase flags
//! (`PHP_OUTPUT_HANDLER_START|CLEAN|FLUSH|FINAL`).

use rphp_runtime::{
    nf, Ctx, NativeFn, NativeResult, Registry, PHP_OUTPUT_HANDLER_CLEAN,
    PHP_OUTPUT_HANDLER_CLEANABLE, PHP_OUTPUT_HANDLER_DISABLED, PHP_OUTPUT_HANDLER_FINAL,
    PHP_OUTPUT_HANDLER_FLUSH, PHP_OUTPUT_HANDLER_FLUSHABLE, PHP_OUTPUT_HANDLER_PROCESSED,
    PHP_OUTPUT_HANDLER_REMOVABLE, PHP_OUTPUT_HANDLER_START, PHP_OUTPUT_HANDLER_STARTED,
    PHP_OUTPUT_HANDLER_STDFLAGS, PHP_OUTPUT_HANDLER_USER,
};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("ob_start", 0, Some(3), ob_start),
    nf!("ob_get_clean", 0, Some(0), ob_get_clean),
    nf!("ob_get_contents", 0, Some(0), ob_get_contents),
    nf!("ob_end_clean", 0, Some(0), ob_end_clean),
    nf!("ob_end_flush", 0, Some(0), ob_end_flush),
    nf!("ob_get_flush", 0, Some(0), ob_get_flush),
    nf!("ob_get_level", 0, Some(0), ob_get_level),
    nf!("ob_get_length", 0, Some(0), ob_get_length),
    nf!("ob_flush", 0, Some(0), ob_flush),
    nf!("ob_clean", 0, Some(0), ob_clean),
    nf!("flush", 0, Some(0), flush),
    nf!("ob_get_status", 0, Some(1), ob_get_status),
    nf!("ob_implicit_flush", 0, Some(1), ob_implicit_flush),
    nf!("ob_list_handlers", 0, Some(0), ob_list_handlers),
];

/// The `PHP_OUTPUT_HANDLER_*` constants.
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("PHP_OUTPUT_HANDLER_START", PHP_OUTPUT_HANDLER_START),
        ("PHP_OUTPUT_HANDLER_WRITE", 0),
        ("PHP_OUTPUT_HANDLER_FLUSH", PHP_OUTPUT_HANDLER_FLUSH),
        ("PHP_OUTPUT_HANDLER_CLEAN", PHP_OUTPUT_HANDLER_CLEAN),
        ("PHP_OUTPUT_HANDLER_FINAL", PHP_OUTPUT_HANDLER_FINAL),
        ("PHP_OUTPUT_HANDLER_CONT", 0),
        ("PHP_OUTPUT_HANDLER_END", PHP_OUTPUT_HANDLER_FINAL),
        ("PHP_OUTPUT_HANDLER_CLEANABLE", PHP_OUTPUT_HANDLER_CLEANABLE),
        ("PHP_OUTPUT_HANDLER_FLUSHABLE", PHP_OUTPUT_HANDLER_FLUSHABLE),
        ("PHP_OUTPUT_HANDLER_REMOVABLE", PHP_OUTPUT_HANDLER_REMOVABLE),
        ("PHP_OUTPUT_HANDLER_STDFLAGS", PHP_OUTPUT_HANDLER_STDFLAGS),
        ("PHP_OUTPUT_HANDLER_STARTED", PHP_OUTPUT_HANDLER_STARTED),
        ("PHP_OUTPUT_HANDLER_DISABLED", PHP_OUTPUT_HANDLER_DISABLED),
        ("PHP_OUTPUT_HANDLER_PROCESSED", PHP_OUTPUT_HANDLER_PROCESSED),
    ] {
        r.constant(name, Value::Int(v));
    }
}

/// `ob_start(?callable $callback = null, int $chunk_size = 0, int $flags = PHP_OUTPUT_HANDLER_STDFLAGS): bool`
pub(crate) fn ob_start(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let callback = args.first().filter(|v| !matches!(v.deref().as_ref(), Value::Null)).cloned();
    let chunk = args.get(1).map_or(0, |v| v.to_int().max(0)) as usize;
    let flags = args.get(2).map_or(PHP_OUTPUT_HANDLER_STDFLAGS, Value::to_int);
    ctx.ob_start(callback, chunk, flags);
    Ok(Value::Bool(true))
}

/// `ob_get_clean(): string|false` — the contents, then the level is
/// discarded; `false` when no level is active (no notice, as php).
pub(crate) fn ob_get_clean(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(match ctx.ob_discard_top()? {
        Some(raw) => Value::string(&raw),
        None => Value::Bool(false),
    })
}

/// `ob_get_contents(): string|false`
pub(crate) fn ob_get_contents(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(match ctx.out.top_contents() {
        Some(b) => Value::string(&b),
        None => Value::Bool(false),
    })
}

/// `ob_end_clean(): bool`
pub(crate) fn ob_end_clean(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    if ctx.out.level() == 0 {
        ctx.notice("ob_end_clean(): Failed to delete buffer. No buffer to delete")?;
        return Ok(Value::Bool(false));
    }
    ctx.ob_discard_top()?;
    Ok(Value::Bool(true))
}

/// `ob_end_flush(): bool`
pub(crate) fn ob_end_flush(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    if ctx.out.level() == 0 {
        ctx.notice("ob_end_flush(): Failed to delete and flush buffer. No buffer to delete or flush")?;
        return Ok(Value::Bool(false));
    }
    ctx.ob_flush_top(true)?;
    Ok(Value::Bool(true))
}

/// `ob_get_flush(): string|false`
pub(crate) fn ob_get_flush(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    if ctx.out.level() == 0 {
        ctx.notice("ob_get_flush(): Failed to delete and flush buffer. No buffer to delete or flush")?;
        return Ok(Value::Bool(false));
    }
    Ok(match ctx.ob_flush_top(true)? {
        Some(raw) => Value::string(&raw),
        None => Value::Bool(false),
    })
}

/// `ob_get_level(): int`
pub(crate) fn ob_get_level(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(ctx.out.level() as i64))
}

/// `ob_get_length(): int|false`
pub(crate) fn ob_get_length(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(match ctx.out.top_len() {
        Some(n) => Value::Int(n as i64),
        None => Value::Bool(false),
    })
}

/// `ob_flush(): bool`
pub(crate) fn ob_flush(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    if ctx.out.level() == 0 {
        ctx.notice("ob_flush(): Failed to flush buffer. No buffer to flush")?;
        return Ok(Value::Bool(false));
    }
    ctx.ob_flush_top(false)?;
    Ok(Value::Bool(true))
}

/// `ob_clean(): bool`
pub(crate) fn ob_clean(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    if ctx.out.level() == 0 {
        ctx.notice("ob_clean(): Failed to delete buffer. No buffer to delete")?;
        return Ok(Value::Bool(false));
    }
    ctx.ob_clean_top()?;
    Ok(Value::Bool(true))
}

/// `flush(): void` — push the SAPI layer (the sink); `ob_*` levels stay.
pub(crate) fn flush(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.out.flush_sink();
    // A web SAPI sends the head on `flush()` even while the buffers hold
    // every byte; php then never records where the output started, so a
    // later `header()` complains without a location.
    if ctx.head.lock().map(|h| h.sent).unwrap_or(false) {
        ctx.out.forget_first_send();
    }
    Ok(Value::Null)
}

fn status_of(level: &rphp_runtime::ObLevel, index: usize) -> Value {
    let mut a = Array::new();
    let (name, ty, flags) = match &level.callback {
        Some(cb) => {
            let name = match cb {
                Value::Str(s) => s.as_bytes().to_vec(),
                _ => b"Closure::__invoke".to_vec(),
            };
            (name, PHP_OUTPUT_HANDLER_USER, level.flags | PHP_OUTPUT_HANDLER_USER)
        }
        None => (b"default output handler".to_vec(), 0, level.flags),
    };
    let flags = if level.started { flags | PHP_OUTPUT_HANDLER_STARTED } else { flags };
    a.set(ArrayKey::str(b"name"), Value::string(&name));
    a.set(ArrayKey::str(b"type"), Value::Int(ty));
    a.set(ArrayKey::str(b"flags"), Value::Int(flags));
    a.set(ArrayKey::str(b"level"), Value::Int(index as i64));
    a.set(ArrayKey::str(b"chunk_size"), Value::Int(level.chunk_size as i64));
    // php sizes the buffer to the chunk size when there is one, else 16K.
    let size = if level.chunk_size > 0 { level.chunk_size } else { 16384 };
    a.set(ArrayKey::str(b"buffer_size"), Value::Int(size as i64));
    a.set(ArrayKey::str(b"buffer_used"), Value::Int(level.buf.len() as i64));
    Value::Array(a)
}

/// `ob_get_status(bool $full_status = false): array`
pub(crate) fn ob_get_status(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let full = args.first().is_some_and(Value::to_bool);
    let levels = ctx.out.levels();
    if full {
        let mut a = Array::new();
        for (i, l) in levels.iter().enumerate() {
            a.push(status_of(l, i));
        }
        return Ok(Value::Array(a));
    }
    Ok(match levels.last() {
        Some(l) => status_of(l, levels.len() - 1),
        None => Value::empty_array(),
    })
}

/// `ob_implicit_flush(bool $enable = true): void` — the CLI already streams
/// (`implicit_flush=1`); a no-op.
pub(crate) fn ob_implicit_flush(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// `ob_list_handlers(): array`
pub(crate) fn ob_list_handlers(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    for l in ctx.out.levels() {
        let name = match &l.callback {
            Some(Value::Str(s)) => s.as_bytes().to_vec(),
            Some(_) => b"Closure::__invoke".to_vec(),
            None => l.handler_name().as_bytes().to_vec(),
        };
        a.push(Value::string(&name));
    }
    Ok(Value::Array(a))
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::interp;

    #[test]
    fn nested_levels_and_edge_cases_match_php() {
        let mut it = interp();
        assert_eq!(it.call_function(b"ob_get_clean", &[]).unwrap(), Value::Bool(false));
        assert_eq!(it.call_function(b"ob_end_clean", &[]).unwrap(), Value::Bool(false));
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "\nNotice: ob_end_clean(): Failed to delete buffer. No buffer to delete in Command line code on line 0\n"
        );
        assert_eq!(it.call_function(b"ob_get_level", &[]).unwrap(), Value::Int(0));
        it.call_function(b"ob_start", &[]).unwrap();
        it.echo(b"in");
        assert_eq!(it.call_function(b"ob_get_length", &[]).unwrap(), Value::Int(2));
        let status = it.call_function(b"ob_get_status", &[]).unwrap();
        let Value::Array(a) = status else { panic!() };
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"name")).unwrap(), &Value::string(b"default output handler"));
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"flags")).unwrap(), &Value::Int(112));
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"buffer_used")).unwrap(), &Value::Int(2));
        assert_eq!(it.call_function(b"ob_get_clean", &[]).unwrap(), Value::string(b"in"));
        it.call_function(b"ob_start", &[]).unwrap();
        it.echo(b"x");
        it.call_function(b"ob_start", &[]).unwrap();
        it.echo(b"y");
        assert_eq!(it.call_function(b"ob_get_level", &[]).unwrap(), Value::Int(2));
        assert_eq!(it.call_function(b"ob_end_flush", &[]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_function(b"ob_get_flush", &[]).unwrap(), Value::string(b"xy"));
        assert_eq!(it.take_test_output(), b"xy");
        assert_eq!(it.call_function(b"ob_get_contents", &[]).unwrap(), Value::Bool(false));
        assert_eq!(it.call_function(b"flush", &[]).unwrap(), Value::Null);
    }

    #[test]
    fn user_handler_sees_php_phase_flags() {
        let mut it = interp();
        // strtoupper as a handler: called with (buffer, phase) — the extra arg
        // trips its arity check exactly like php ("expects exactly 1 argument").
        it.call_function(b"ob_start", &[Value::string(b"strtoupper")]).unwrap();
        it.echo(b"abc");
        let err = it.call_function(b"ob_end_flush", &[]).unwrap_err();
        assert_eq!(err.message(), Some("strtoupper() expects exactly 1 argument, 2 given"));
    }
}
