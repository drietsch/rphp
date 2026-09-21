//! Function-handling builtins (php-src `Zend/zend_builtin_functions.c`,
//! `ext/standard/array.c` for the symbol-table ones): invoking callables
//! through the interpreter (`ctx.call_value`), the argument accessors of the
//! calling frame (`func_get_args` & co.), the symbol-table functions of a
//! `NEEDS_SYMTAB` frame (`compact`, `extract`, `get_defined_vars`) and the
//! by-reference walker `array_walk`.
use rphp_value::{Array, ArrayKey, Value};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Unwind};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("call_user_func", 1, None, call_user_func),
    nf!("call_user_func_array", 2, Some(2), call_user_func_array),
    nf!("func_get_args", 0, Some(0), func_get_args),
    nf!("func_num_args", 0, Some(0), func_num_args),
    nf!("func_get_arg", 1, Some(1), func_get_arg),
    nf!("compact", 1, None, compact),
    // php declares `$array` prefer-ref (literals are accepted); without
    // `EXTR_REFS` the array is never written, so it is by value here.
    nf!("extract", 1, Some(3), extract),
    nf!("get_defined_vars", 0, Some(0), get_defined_vars),
    nf_ref!("array_walk", 2, Some(3), 0b1, array_walk),
];

/// `call_user_func($callable, ...$args)`: invoke `$callable` with the remaining
/// arguments and return its result.
pub(crate) fn call_user_func(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let named = ctx.take_extra_named();
    ctx.call_value_named(&args[0], &args[1..], named)
}

/// `call_user_func_array($callable, $args)`: invoke `$callable` with the
/// values of the `$args` array; string keys are passed as named arguments
/// (PHP 8.0), which must follow every positional one.
pub(crate) fn call_user_func_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = match &args[1] {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "call_user_func_array(): Argument #2 ($args) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let mut positional: Vec<Value> = Vec::new();
    let mut named: Vec<(Box<[u8]>, Value)> = Vec::new();
    for (k, v) in arr.iter() {
        match k {
            ArrayKey::Int(_) => {
                if !named.is_empty() {
                    return Err(Unwind::error(
                        "Cannot use positional argument after named argument during unpacking",
                    ));
                }
                positional.push(v.clone());
            }
            ArrayKey::Str(s) => named.push((s.clone(), v.clone())),
        }
    }
    if named.is_empty() {
        return ctx.call_value(&args[0], &positional);
    }
    ctx.autoload_callable(&args[0])?;
    let callable = ctx.resolve_callable(&args[0])?;
    ctx.call_resolved_named(callable, &positional, named)
}

/// The innermost user frame — the function that called this native — or
/// php's error for a call from the top level.
fn calling_frame<'a>(ctx: &'a Ctx, what: &str) -> Result<&'a rphp_runtime::Frame, Unwind> {
    match ctx.current_user_frame() {
        Some(f) if f.func.as_ref().is_some_and(|func| !func.is_main()) => Ok(f),
        _ => Err(Unwind::error(format!(
            "{what}() cannot be called from the global scope"
        ))),
    }
}

/// `func_get_args(): array` — the current values of the passed parameters
/// followed by the extra arguments.
pub(crate) fn func_get_args(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let frame = calling_frame(ctx, "func_get_args")?;
    let args = ctx.frame_args(frame);
    let mut out = Array::new();
    for a in args {
        out.push(a);
    }
    Ok(Value::Array(out))
}

/// `func_num_args(): int`
pub(crate) fn func_num_args(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let frame = calling_frame(ctx, "func_num_args")?;
    Ok(Value::Int(ctx.frame_args(frame).len() as i64))
}

/// `func_get_arg(int $position): mixed`
pub(crate) fn func_get_arg(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let frame = calling_frame(ctx, "func_get_arg")?;
    let all = ctx.frame_args(frame);
    let pos = args[0].to_int();
    if pos < 0 {
        return Err(Unwind::value_error(
            "func_get_arg(): Argument #1 ($position) must be greater than or equal to 0",
        ));
    }
    match all.get(pos as usize) {
        Some(v) => Ok(v.clone()),
        None => Err(Unwind::value_error(
            "func_get_arg(): Argument #1 ($position) must be less than the number of the arguments passed to the currently executed function",
        )),
    }
}

/// `compact(array|string ...$var_names): array` — the named variables of the
/// calling frame that are set; nested arrays of names are flattened, an
/// undefined name warns.
pub(crate) fn compact(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let symtab = ctx.current_symtab();
    let mut out = Array::new();
    fn walk(ctx: &mut Ctx, symtab: &Option<rphp_runtime::Symtab>, v: &Value, out: &mut Array) -> Result<(), Unwind> {
        match &*v.deref() {
            Value::Array(a) => {
                for (_, item) in a.iter() {
                    walk(ctx, symtab, item, out)?;
                }
                Ok(())
            }
            other => {
                let name = other.to_php_bytes();
                let cell = symtab.as_ref().and_then(|t| t.get(&name));
                match cell.map(|c| c.get()) {
                    Some(val) if !val.is_uninit() => {
                        out.set(ArrayKey::str(&name), val);
                    }
                    _ => ctx.warn(&format!(
                        "compact(): Undefined variable ${}",
                        String::from_utf8_lossy(&name)
                    ))?,
                }
                Ok(())
            }
        }
    }
    for a in args.iter() {
        walk(ctx, &symtab, a, &mut out)?;
    }
    Ok(Value::Array(out))
}

/// `extract(array &$array, int $flags = EXTR_OVERWRITE, string $prefix = ""): int`
/// — import the string-keyed entries into the calling frame's symbol table
/// (`EXTR_OVERWRITE` semantics only; invalid names and `this` are skipped).
pub(crate) fn extract(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "extract(): Argument #1 ($array) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let Some(symtab) = ctx.current_symtab() else {
        return Ok(Value::Int(0));
    };
    let mut n = 0;
    for (k, v) in arr.iter() {
        let ArrayKey::Str(name) = k else { continue };
        if name.is_empty()
            || name.as_ref() == b"this"
            || !(name[0].is_ascii_alphabetic() || name[0] == b'_' || name[0] >= 0x80)
            || !name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b >= 0x80)
        {
            continue;
        }
        let cell = symtab.get_or_create(name);
        cell.set(v.deref().into_owned());
        n += 1;
    }
    Ok(Value::Int(n))
}

/// `get_defined_vars(): array` — the calling frame's variables in
/// declaration order.
pub(crate) fn get_defined_vars(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Array(match ctx.current_symtab() {
        Some(t) => t.with(|d| d.to_array()),
        None => Array::new(),
    }))
}

/// `array_walk(array|object &$array, callable $callback, mixed $arg = null): true`
/// — the callback receives each element by reference (writes land in the
/// array), the key, and `$arg` when given.
pub(crate) fn array_walk(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut arr = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "array_walk(): Argument #1 ($array) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let cb = args[1].clone();
    let extra = args.get(2).cloned();
    let keys: Vec<ArrayKey> = arr.keys().cloned().collect();
    for k in keys {
        let cell = arr.get_ref(k.clone());
        let mut call_args = vec![Value::Ref(cell), k.to_value()];
        if let Some(e) = &extra {
            call_args.push(e.clone());
        }
        ctx.call_value(&cb, &call_args)?;
    }
    // Elements the callback did not bind elsewhere go back to plain values.
    let mut out = Array::new();
    for (k, v) in arr.iter() {
        match v {
            Value::Ref(r) if r.strong_count() > 1 => out.set_ref(k.clone(), r.clone()),
            other => out.set(k.clone(), other.deref().into_owned()),
        }
    }
    args[0] = Value::Array(out);
    Ok(Value::Bool(true))
}
