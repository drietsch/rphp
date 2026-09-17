//! The autoloader stack (php-src `ext/spl/php_spl.c`), plan E7.
//!
//! The engine owns the stack and the lookup path (`rphp-runtime`'s
//! `autoload.rs`); this module is the php-facing surface. Registering the
//! same callable twice is a no-op, which is what lets a package's bootstrap
//! run more than once safely — Composer's generated `autoload_real.php`
//! depends on it.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("spl_autoload_register", 0, Some(3), spl_autoload_register),
    nf!("spl_autoload_unregister", 1, Some(1), spl_autoload_unregister),
    nf!("spl_autoload_functions", 0, Some(0), spl_autoload_functions),
    nf!("spl_autoload_call", 1, Some(1), spl_autoload_call),
    nf!("class_alias", 2, Some(3), class_alias),
];

/// `spl_autoload_register(?callable $callback = null, bool $throw = true, bool $prepend = false): bool`
fn spl_autoload_register(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let callback = args.first().cloned().unwrap_or(Value::Null);
    if matches!(callback, Value::Null) {
        // php registers its own `spl_autoload` default here; rphp has no
        // include-path-scanning default loader, so say so rather than
        // pretending to register one.
        return Err(Unwind::error(
            "spl_autoload_register(): Argument #1 ($callback) must be a valid callback, the default autoloader is not implemented",
        ));
    }
    if !ctx.is_callable(&callback) {
        return Err(Unwind::type_error(
            "spl_autoload_register(): Argument #1 ($callback) must be a valid callback",
        ));
    }
    let prepend = args.get(2).is_some_and(Value::to_bool);
    if !ctx.autoloader_registered(&callback) {
        ctx.autoloader_add(callback, prepend);
    }
    Ok(Value::Bool(true))
}

/// `spl_autoload_unregister(callable $callback): bool`
fn spl_autoload_unregister(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(ctx.autoloader_remove(&args[0])))
}

/// `spl_autoload_functions(): array` — the stack in call order.
fn spl_autoload_functions(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for f in ctx.autoloaders() {
        out.push(f);
    }
    Ok(Value::Array(out))
}

/// `spl_autoload_call(string $class): void` — run the stack for `$class`,
/// whether or not it is already declared.
fn spl_autoload_call(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    ctx.autoload_call(&name)?;
    Ok(Value::Null)
}

/// `class_alias(string $class, string $alias, bool $autoload = true): bool`
fn class_alias(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let original = args[0].to_php_bytes();
    let alias = args[1].to_php_bytes();
    let autoload = args.get(2).map_or(true, Value::to_bool);
    let id = if autoload {
        ctx.lookup_class(&original)?
    } else {
        ctx.class_by_name(&original)
    };
    let Some(id) = id else {
        return Err(Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(&original)
        )));
    };
    if ctx.class_by_name(&alias).is_some() {
        ctx.warn(&format!(
            "Cannot declare class {}, because the name is already in use",
            String::from_utf8_lossy(&alias)
        ))?;
        return Ok(Value::Bool(false));
    }
    ctx.alias_class(&alias, id);
    Ok(Value::Bool(true))
}
