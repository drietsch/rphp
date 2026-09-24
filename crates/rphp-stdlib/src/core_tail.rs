//! The rest of `Zend/zend_builtin_functions.c` (the `Core` extension):
//! the function forms of `exit`/`die` (php 8.4) and `clone` (php 8.5),
//! `get_included_files`, `get_declared_traits`, `get_mangled_object_vars`
//! and `get_resource_id`.
//!
//! `exit(…)` / `die(…)` with an argument list compile to a call to `\exit`
//! (the parser adapter), so the construct and the callable string share this
//! handler; the `string|int` coercion, the arity check and named arguments
//! are the native call's (`native_zpp.rs`, under the caller's
//! `strict_types`). `clone($o, [...])` likewise compiles to a call to
//! `clone()`, whose frame php shows in stack traces; the bare `clone $o`
//! stays `Op::Clone`.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, Value};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("exit", 0, Some(1), exit),
    nf!("die", 0, Some(1), exit),
    nf!("clone", 1, Some(2), clone),
    nf!("get_included_files", 0, Some(0), get_included_files),
    nf!("get_required_files", 0, Some(0), get_included_files),
    nf!("get_declared_traits", 0, Some(0), get_declared_traits),
    nf!("get_mangled_object_vars", 1, Some(1), get_mangled_object_vars),
    nf!("get_resource_id", 1, Some(1), get_resource_id),
];

#[allow(dead_code)]
pub(crate) fn register_classes(_r: &mut Registry) {}

#[allow(dead_code)]
pub(crate) fn register_constants(_r: &mut Registry) {}

/// `exit(string|int $status = 0): never` — a string is printed and the
/// status is 0; an int is the process status (php keeps its low byte).
fn exit(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let code = match args.first().map(|v| v.deref().into_owned()) {
        None => 0,
        Some(Value::Int(i)) => i as i32,
        // The parameter parsing leaves a resource to the handler.
        Some(v @ Value::Resource(_)) => {
            return Err(Unwind::type_error(format!(
                "exit(): Argument #1 ($status) must be of type string|int, {} given",
                rphp_runtime::value_name(&v)
            )))
        }
        Some(v) => {
            // The parameter parsing left a string (or an int) here.
            ctx.echo(&v.to_php_bytes());
            0
        }
    };
    Err(Unwind::exit(code))
}

/// `clone(object $object, array $withProperties = []): object` — the
/// replacements are ordinary property writes judged from the caller's
/// scope, made inside the copy's clone window (so a `readonly` property
/// may be re-initialized once).
fn clone(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let obj = args[0].deref().into_owned();
    let with = args.get(1).map(|v| v.deref().into_owned());
    ctx.clone_object_with(&obj, with.as_ref())
}

/// `get_included_files(): array` (alias `get_required_files`) — the entry
/// script first, then every included file in first-inclusion order.
fn get_included_files(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for f in ctx.included_files() {
        out.push(Value::string(f.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `get_declared_traits(): array`
fn get_declared_traits(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for c in ctx.declared_classes() {
        if c.kind == rphp_runtime::ClassKind::Trait {
            out.push(Value::string(&c.name));
        }
    }
    Ok(Value::Array(out))
}

/// `get_mangled_object_vars(object $object): array` — the raw property
/// table, whatever the calling scope: private properties as
/// `"\0Class\0name"`, protected as `"\0*\0name"`, and nothing a native class
/// computes (an `ArrayObject`'s storage is not listed).
fn get_mangled_object_vars(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match &*args[0].deref() {
        Value::Object(o) => Ok(Value::Array(ctx.std_property_table(&o.clone()))),
        Value::Closure(_) => Ok(Value::empty_array()),
        other => Err(Unwind::type_error(format!(
            "get_mangled_object_vars(): Argument #1 ($object) must be of type object, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// `get_resource_id(resource $resource): int`
fn get_resource_id(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match &*args[0].deref() {
        Value::Resource(r) => Ok(Value::Int(i64::from(r.id()))),
        other => Err(Unwind::type_error(format!(
            "get_resource_id(): Argument #1 ($resource) must be of type resource, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}
