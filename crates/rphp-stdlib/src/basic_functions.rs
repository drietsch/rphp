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
    nf!("class_uses", 1, Some(2), class_uses),
    nf!("get_resource_type", 1, Some(1), get_resource_type),
    nf!("get_defined_constants", 0, Some(1), get_defined_constants),
    nf!("get_defined_functions", 0, Some(1), get_defined_functions),
    nf!("set_time_limit", 1, Some(1), set_time_limit),
    // --- class introspection over the engine's class table ---
    nf!("class_exists", 1, Some(2), class_exists),
    nf!("interface_exists", 1, Some(2), interface_exists),
    nf!("trait_exists", 1, Some(2), trait_exists),
    nf!("enum_exists", 1, Some(2), enum_exists),
    nf!("get_declared_classes", 0, Some(0), get_declared_classes),
    nf!("get_declared_interfaces", 0, Some(0), get_declared_interfaces),
    nf!("get_class_vars", 1, Some(1), get_class_vars),
    nf!("class_implements", 1, Some(2), class_implements),
    nf!("class_parents", 1, Some(2), class_parents),
    nf!("get_class", 0, Some(1), get_class),
    nf!("get_parent_class", 0, Some(1), get_parent_class),
    nf!("get_called_class", 0, Some(0), get_called_class),
    nf!("method_exists", 2, Some(2), method_exists),
    nf!("property_exists", 2, Some(2), property_exists),
    nf!("get_object_vars", 1, Some(1), get_object_vars),
    nf!("get_class_methods", 1, Some(1), get_class_methods),
    nf!("is_a", 2, Some(3), is_a),
    nf!("is_subclass_of", 2, Some(3), is_subclass_of),
    nf!("spl_object_id", 1, Some(1), spl_object_id),
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

/// `phpversion(?string $extension = null): string|false` — the engine's
/// version, or the version of a bundled extension (`false` when it is not
/// loaded).
pub(crate) fn phpversion(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match args.first() {
        Some(v) if !matches!(*v.deref(), Value::Null) => {
            Ok(crate::info::extension_version(&v.to_php_string()).map_or(Value::Bool(false), |ver| Value::string(ver.as_bytes())))
        }
        _ => Ok(Value::string(crate::info::PHP_VERSION.as_bytes())),
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
    Ok(Value::Bool(ctx.function_exists(name)))
}

/// `set_time_limit(int $seconds): bool` — accepted; the engine has no
/// execution timer yet.
pub(crate) fn set_time_limit(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

/// Whether a declared class-like of `kind` exists under the name in
/// `args[0]` (autoloading arrives with plan E7).
fn class_like_exists(
    ctx: &mut Ctx,
    args: &[Value],
    want: fn(&rphp_runtime::ClassDef) -> bool,
) -> NativeResult {
    let name = args[0].to_php_bytes();
    // `$autoload` defaults to true: an unknown name goes to the autoloader
    // stack before the answer is `false` (E7).
    let autoload = args.get(1).is_none_or(Value::to_bool);
    let id = if autoload {
        ctx.lookup_class(&name)?
    } else {
        ctx.class_by_name(&name)
    };
    Ok(Value::Bool(
        id.map(|id| ctx.class(id)).is_some_and(|c| c.linked && want(c)),
    ))
}

/// `class_exists(string $class, bool $autoload = true): bool` — declared
/// classes (not interfaces, traits or enums).
pub(crate) fn class_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    class_like_exists(ctx, args, |c| c.kind == rphp_runtime::ClassKind::Class)
}

/// `interface_exists(string $interface, bool $autoload = true): bool`
pub(crate) fn interface_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    class_like_exists(ctx, args, |c| c.kind == rphp_runtime::ClassKind::Interface)
}

/// `trait_exists(string $trait, bool $autoload = true): bool`
pub(crate) fn trait_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    class_like_exists(ctx, args, |c| c.kind == rphp_runtime::ClassKind::Trait)
}

/// `enum_exists(string $enum, bool $autoload = true): bool`
pub(crate) fn enum_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    class_like_exists(ctx, args, |c| matches!(c.kind, rphp_runtime::ClassKind::Enum { .. }))
}

/// `get_declared_classes(): array` — declared classes (internal first, then
/// user classes in declaration order), as php lists them.
pub(crate) fn get_declared_classes(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = rphp_value::Array::new();
    for c in ctx.declared_classes() {
        if c.kind == rphp_runtime::ClassKind::Class {
            out.push(Value::string(&c.name));
        }
    }
    Ok(Value::Array(out))
}

/// `get_declared_interfaces(): array`
pub(crate) fn get_declared_interfaces(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = rphp_value::Array::new();
    for c in ctx.declared_classes() {
        if c.kind == rphp_runtime::ClassKind::Interface {
            out.push(Value::string(&c.name));
        }
    }
    Ok(Value::Array(out))
}

/// `get_class_vars(string $class): array` — the default values of the
/// properties visible from the calling scope.
pub(crate) fn get_class_vars(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    let Some(cid) = ctx.class_by_name(&name) else {
        return Err(Unwind::type_error(format!(
            "get_class_vars(): Argument #1 ($class) must be a valid class name, {} given",
            String::from_utf8_lossy(&name)
        )));
    };
    let scope = ctx.current_user_frame().and_then(|f| f.scope);
    let class = ctx.class(cid).clone();
    let mut out = rphp_value::Array::new();
    for p in &class.props {
        if !ctx.access_ok_public(p.vis, p.decl, scope) {
            continue;
        }
        let v = match &p.default {
            rphp_runtime::PropDefault::Value(v) => v.clone(),
            rphp_runtime::PropDefault::Thunk(_) => Value::Null,
        };
        out.set(rphp_value::ArrayKey::str(&p.name), v);
    }
    Ok(Value::Array(out))
}

/// `class_implements(object|string $object_or_class, bool $autoload = true): array|false`
/// — `name => name` for every implemented interface.
pub(crate) fn class_implements(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let class = match &*args[0].deref() {
        Value::Object(o) => Some(o.class_id()),
        Value::Str(s) => ctx.class_by_name(s.as_bytes()),
        other => {
            return Err(Unwind::type_error(format!(
                "class_implements(): Argument #1 ($object_or_class) must be of type object|string, {} given",
                other.type_name()
            )))
        }
    };
    let Some(cid) = class else {
        ctx.warn(&format!(
            "class_implements(): Class {} does not exist and could not be loaded",
            String::from_utf8_lossy(&args[0].to_php_bytes())
        ))?;
        return Ok(Value::Bool(false));
    };
    let mut out = rphp_value::Array::new();
    for &iid in &ctx.class(cid).interfaces {
        let n = ctx.class(iid).name.clone();
        out.set(rphp_value::ArrayKey::str(&n), Value::string(&n));
    }
    Ok(Value::Array(out))
}

/// `class_parents(object|string $object_or_class, bool $autoload = true): array|false`
/// `class_uses(object|string $object_or_class, bool $autoload = true): array|false`
/// — `name => name` for the traits the class uses **itself**; a parent's
/// traits are not reported.
pub(crate) fn class_uses(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let class = match &*args[0].deref() {
        Value::Object(o) => Some(o.class_id()),
        Value::Str(s) => ctx.class_by_name(s.as_bytes()),
        other => {
            return Err(Unwind::type_error(format!(
                "class_uses(): Argument #1 ($object_or_class) must be of type object|string, {} given",
                other.type_name()
            )))
        }
    };
    let Some(cid) = class else {
        ctx.warn(&format!(
            "class_uses(): Class {} does not exist and could not be loaded",
            String::from_utf8_lossy(&args[0].to_php_bytes())
        ))?;
        return Ok(Value::Bool(false));
    };
    let mut out = rphp_value::Array::new();
    for &tid in &ctx.class(cid).used_traits.clone() {
        let n = ctx.class(tid).name.clone();
        out.set(rphp_value::ArrayKey::str(&n), Value::string(&n));
    }
    Ok(Value::Array(out))
}

/// `get_resource_type(resource $resource): string` — the kind the engine
/// registered the resource under (`stream`, …), or `Unknown` once closed.
pub(crate) fn get_resource_type(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match &*args[0].deref() {
        Value::Resource(r) => Ok(Value::string(r.kind().as_bytes())),
        other => Err(Unwind::type_error(format!(
            "get_resource_type(): Argument #1 ($resource) must be of type resource, {} given",
            other.type_name()
        ))),
    }
}

/// `get_defined_constants(bool $categorize = false): array`. The categorized
/// form needs each constant's extension, which rphp does not record: the
/// engine's own land under `Core` and everything `define()` added under
/// `user`, which is php's split for those two groups.
pub(crate) fn get_defined_constants(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let categorize = args.first().is_some_and(|v| v.deref().to_bool());
    let mut all: Vec<(Box<[u8]>, Value)> = ctx
        .constants()
        .map(|(k, v)| (Box::from(k), v.clone()))
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    if !categorize {
        let mut out = rphp_value::Array::new();
        for (k, v) in all {
            out.set(rphp_value::ArrayKey::str(&k), v);
        }
        return Ok(Value::Array(out));
    }
    let user: std::collections::HashSet<Box<[u8]>> =
        ctx.user_constant_names().iter().cloned().collect();
    let mut core = rphp_value::Array::new();
    let mut theirs = rphp_value::Array::new();
    for (k, v) in all {
        if user.contains(&k) {
            theirs.set(rphp_value::ArrayKey::str(&k), v);
        } else {
            core.set(rphp_value::ArrayKey::str(&k), v);
        }
    }
    let mut out = rphp_value::Array::new();
    out.set(rphp_value::ArrayKey::str(b"Core"), Value::Array(core));
    if !theirs.is_empty() {
        out.set(rphp_value::ArrayKey::str(b"user"), Value::Array(theirs));
    }
    Ok(Value::Array(out))
}

/// `get_defined_functions(bool $exclude_disabled = true): array` — php's
/// `['internal' => …, 'user' => …]`, both lower-cased, user functions in
/// declaration order.
pub(crate) fn get_defined_functions(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut internal = rphp_value::Array::new();
    for f in ctx.natives() {
        internal.push(Value::string(f.name.to_ascii_lowercase().as_bytes()));
    }
    let mut user = rphp_value::Array::new();
    for name in ctx.user_function_names() {
        user.push(Value::string(&name));
    }
    let mut out = rphp_value::Array::new();
    out.set(rphp_value::ArrayKey::str(b"internal"), Value::Array(internal));
    out.set(rphp_value::ArrayKey::str(b"user"), Value::Array(user));
    Ok(Value::Array(out))
}

pub(crate) fn class_parents(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let class = match &*args[0].deref() {
        Value::Object(o) => Some(o.class_id()),
        Value::Str(s) => ctx.class_by_name(s.as_bytes()),
        other => {
            return Err(Unwind::type_error(format!(
                "class_parents(): Argument #1 ($object_or_class) must be of type object|string, {} given",
                other.type_name()
            )))
        }
    };
    let Some(cid) = class else {
        ctx.warn(&format!(
            "class_parents(): Class {} does not exist and could not be loaded",
            String::from_utf8_lossy(&args[0].to_php_bytes())
        ))?;
        return Ok(Value::Bool(false));
    };
    let mut out = rphp_value::Array::new();
    let mut cur = ctx.class(cid).parent;
    while let Some(p) = cur {
        let n = ctx.class(p).name.clone();
        out.set(rphp_value::ArrayKey::str(&n), Value::string(&n));
        cur = ctx.class(p).parent;
    }
    Ok(Value::Array(out))
}

/// The class id a `$object_or_class` argument denotes, with php's
/// `TypeError` text for `func`.
fn class_arg(ctx: &Ctx, func: &str, pos: usize, v: &Value, allow_string: bool) -> Result<Option<u32>, Unwind> {
    match &*v.deref() {
        Value::Object(o) => Ok(Some(o.class_id())),
        Value::Str(s) if allow_string => Ok(ctx.class_by_name(s.as_bytes())),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #{pos} ($object_or_class) must be of type {}, {} given",
            if allow_string { "object|string" } else { "object" },
            other.type_name()
        ))),
    }
}

/// `get_class(object $object = ?): string`
pub(crate) fn get_class(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match args.first() {
        Some(v) => match &*v.deref() {
            // `get_class()` is one of the few places php answers with the
            // string's real length, so an anonymous class's whole name.
            Value::Object(o) => Ok(Value::string(&ctx.class_full_name_of(o))),
            Value::Closure(_) => Ok(Value::string(b"Closure")),
            other => Err(Unwind::type_error(format!(
                "get_class(): Argument #1 ($object) must be of type object, {} given",
                other.type_name()
            ))),
        },
        None => {
            ctx.deprecated("Calling get_class() without arguments is deprecated")?;
            match ctx.current_user_frame().and_then(|f| f.scope) {
                Some(c) => Ok(Value::string(&ctx.class(c).name)),
                None => Err(Unwind::error(
                    "get_class() without arguments must be called from within a class",
                )),
            }
        }
    }
}

/// `get_parent_class(object|string $object_or_class = ?): string|false`
pub(crate) fn get_parent_class(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let class = match args.first() {
        Some(v) => class_arg(ctx, "get_parent_class", 1, v, true)?,
        None => {
            ctx.deprecated("Calling get_parent_class() without arguments is deprecated")?;
            ctx.current_user_frame().and_then(|f| f.scope)
        }
    };
    Ok(match class.and_then(|c| ctx.class(c).parent) {
        Some(p) => Value::string(&ctx.class(p).name),
        None => Value::Bool(false),
    })
}

/// `get_called_class(): string` — the late-static-bound class of the caller.
pub(crate) fn get_called_class(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    match ctx.current_user_frame().and_then(|f| f.static_class.or(f.scope)) {
        Some(c) => Ok(Value::string(&ctx.class(c).name)),
        None => Err(Unwind::error("get_called_class() must be called from within a class")),
    }
}

/// `method_exists(object|string $object_or_class, string $method): bool`
pub(crate) fn method_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let class = class_arg(ctx, "method_exists", 1, &args[0], true)?;
    let name = args[1].to_php_bytes();
    Ok(Value::Bool(class.is_some_and(|c| ctx.resolve_method(c, &name).is_some())))
}

/// `property_exists(object|string $object_or_class, string $property): bool`
/// — declared (any visibility) or, for an object, dynamic properties.
pub(crate) fn property_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[1].to_php_bytes();
    if let Value::Object(o) = &*args[0].deref() {
        if o.get(&name).is_some() {
            return Ok(Value::Bool(true));
        }
    }
    let class = class_arg(ctx, "property_exists", 1, &args[0], true)?;
    Ok(Value::Bool(class.is_some_and(|c| ctx.resolve_prop(c, &name).is_some())))
}

/// `get_object_vars(object $object): array` — the properties visible from
/// the calling scope, in declaration order.
pub(crate) fn get_object_vars(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Object(o) = &*args[0].deref() else {
        return Err(Unwind::type_error(format!(
            "get_object_vars(): Argument #1 ($object) must be of type object, {} given",
            args[0].type_name()
        )));
    };
    let scope = ctx.current_user_frame().and_then(|f| f.scope);
    let mut out = rphp_value::Array::new();
    for (name, value, _) in o.props_snapshot() {
        if let Some((vis, decl)) = ctx.resolve_prop(o.class_id(), &name) {
            if !ctx.access_ok_public(vis, decl, scope) {
                continue;
            }
        }
        out.set(rphp_value::ArrayKey::str(&name), value.deref().into_owned());
    }
    Ok(Value::Array(out))
}

/// `get_class_methods(object|string $object_or_class): array` — the methods
/// visible from the calling scope, own class first then parents.
pub(crate) fn get_class_methods(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(mut cur) = class_arg(ctx, "get_class_methods", 1, &args[0], true)? else {
        return Err(Unwind::type_error(
            "get_class_methods(): Argument #1 ($object_or_class) must be an object or a valid class name, string given",
        ));
    };
    let scope = ctx.current_user_frame().and_then(|f| f.scope);
    let mut out = rphp_value::Array::new();
    let c = ctx.class(cur).clone();
    for key in &c.method_order {
        let Some(m) = c.methods.get(key) else { continue };
        if !ctx.access_ok_public(m.vis, m.decl, scope) {
            continue;
        }
        out.push(Value::string(&m.name));
    }
    cur = c.id;
    let _ = cur;
    Ok(Value::Array(out))
}

/// `is_a(mixed $object_or_class, string $class, bool $allow_string = false): bool`
pub(crate) fn is_a(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let allow_string = args.get(2).is_some_and(Value::to_bool);
    // php autoloads the *subject* when it is a name (that is what makes
    // `is_a(A::class, $type, true)` work before anything touched `A`), and
    // then matches the target by name up the chain without loading it.
    let class = match &*args[0].deref() {
        Value::Object(o) => Some(o.class_id()),
        Value::Str(s) if allow_string => ctx.lookup_class(s.as_bytes())?,
        _ => None,
    };
    let target = ctx.class_by_name(&args[1].to_php_bytes());
    Ok(Value::Bool(match (class, target) {
        (Some(c), Some(t)) => ctx.is_subclass_or_eq(c, t),
        _ => false,
    }))
}

/// `is_subclass_of(mixed $object_or_class, string $class, bool $allow_string = true): bool`
pub(crate) fn is_subclass_of(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let allow_string = args.get(2).is_none_or(Value::to_bool);
    let class = match &*args[0].deref() {
        Value::Object(o) => Some(o.class_id()),
        Value::Str(s) if allow_string => ctx.lookup_class(s.as_bytes())?,
        _ => None,
    };
    let target = ctx.class_by_name(&args[1].to_php_bytes());
    Ok(Value::Bool(match (class, target) {
        (Some(c), Some(t)) => c != t && ctx.is_subclass_or_eq(c, t),
        _ => false,
    }))
}

/// `spl_object_id(object $object): int`
pub(crate) fn spl_object_id(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match &*args[0].deref() {
        Value::Object(o) => Ok(Value::Int(i64::from(o.id()))),
        other => Err(Unwind::type_error(format!(
            "spl_object_id(): Argument #1 ($object) must be of type object, {} given",
            other.type_name()
        ))),
    }
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
