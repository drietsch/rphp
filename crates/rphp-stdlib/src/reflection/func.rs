//! `ReflectionFunctionAbstract` and its two concrete forms, plus
//! `ReflectionParameter` (php-src `ext/reflection/php_reflection.c`).
//!
//! **What an instance holds.** `ReflectionFunction` and `ReflectionMethod`
//! declare the public `$name` slot php dumps (and `ReflectionMethod` the
//! `$class` slot as well); everything else is an [`FnTarget`] in the payload.
//! The target is stored as a *reference* — a function id, a native id, a
//! `(class, method name)` pair or a closure — and resolved on every call, so
//! a reflector survives a class table that keeps growing under it.
//!
//! **Known divergences**, all upstream of this module:
//!
//! * `getAttributes()` is always empty: the compiler drops every `#[...]`
//!   group, so `Function::attrs` and `ParamDef::attrs` never fill. The code
//!   below reads those fields, so it starts answering as soon as they do.
//! * `getDocComment()` reads `Function::doc`, which the compiler fills from
//!   the declaration. php's scanner keeps the last docblock it saw and
//!   hands it to the next declaration it opens, so `/** … */ $f = function
//!   () {};` documents the *closure* there; the parser here asks for the
//!   docblock directly before the `function` keyword, and that spelling
//!   finds none.
//! * A native function carries no arginfo beyond its arity, its
//!   by-reference mask and (sometimes) its parameter *names*: there are no
//!   declared types and no defaults. `getReturnType()` on an internal
//!   function is therefore `null` and `getParameters()` returns only the
//!   parameters php-src's stub named here.
//! * php 8.4 renamed a closure to `{closure:file:line}`; the compiler still
//!   names one `{closure}`, which is what `getName()` reports.

use std::rc::Rc;

use rphp_runtime::{
    nm, Callable, Ctx, FuncRt, MethodBody, MethodDef, NativeFn, NativeResult, Registry, Unwind,
    Visibility,
};
use rphp_value::{Array, Closure, Object, Value};

use super::attrs;
use super::class;
use super::common::{
    attr_list, list, method_modifiers, namespace_name, new_reflector, obj_arg, opt_arg,
    parse_init_ref, refl_error, run_thunk, short_name, state, store, str_arg, this, AttrInfo,
    FnState, FnTarget, InitKey, ParamState, IS_ABSTRACT, IS_FINAL, IS_PRIVATE, IS_PROTECTED,
    IS_PUBLIC, IS_STATIC,
};
use super::types;

/// The engine's sentinel `Closure::func()` for a closure that wraps a
/// callable which is not a compiled function (`strlen(...)`, `$o->m(...)`
/// over a native method): capture 0 holds the underlying callable.
const ENGINE_CLOSURE: u32 = u32::MAX;

/// Everything the reflectors need about a target, resolved on demand.
pub(crate) struct Info {
    /// The method descriptor, when the target is a method.
    pub method: Option<Rc<MethodDef>>,
    /// The compiled body, when there is one.
    pub func: Option<Rc<FuncRt>>,
    /// The native descriptor, when the target is a plain native.
    pub native: Option<NativeFn>,
    /// The name php reports.
    pub name: Box<[u8]>,
    /// The declaring class, for a method.
    pub class: Option<u32>,
    /// The closure, when the reflector was built over one.
    pub closure: Option<Closure>,
}

impl Info {
    /// The class scope `self`/`parent` in a signature resolve against.
    fn scope(&self) -> Option<u32> {
        self.class
    }
}

/// Resolve a target against the current class and function tables.
pub(crate) fn info(ctx: &Ctx, t: &FnTarget) -> Result<Info, Unwind> {
    let empty = Info {
        method: None,
        func: None,
        native: None,
        name: Box::from(&b""[..]),
        class: None,
        closure: None,
    };
    match t {
        FnTarget::User(id) => {
            let f = ctx.func(*id).clone();
            Ok(Info {
                name: f.f.name_bytes.clone(),
                func: Some(f),
                ..empty
            })
        }
        FnTarget::Native(id) => {
            let n = *ctx.native(*id);
            Ok(Info {
                name: Box::from(n.name.as_bytes()),
                native: Some(n),
                ..empty
            })
        }
        FnTarget::Method { cid, name } => {
            let m = ctx.resolve_method(*cid, name).ok_or_else(|| {
                refl_error(format!(
                    "Method {}::{}() does not exist",
                    ctx.class(*cid).name_str(),
                    String::from_utf8_lossy(name)
                ))
            })?;
            let f = m.user_func().cloned();
            Ok(Info {
                name: m.name.clone(),
                class: Some(m.decl),
                func: f,
                method: Some(m),
                ..empty
            })
        }
        FnTarget::Closure(c) if c.func() != ENGINE_CLOSURE => {
            let f = ctx.func(c.func()).clone();
            Ok(Info {
                name: f.f.name_bytes.clone(),
                class: f.class,
                func: Some(f),
                closure: Some(c.clone()),
                ..empty
            })
        }
        // An engine closure re-resolves the callable it wraps on every use;
        // Reflection asks it the same question.
        FnTarget::Closure(c) => {
            let inner = c.captures().first().cloned().unwrap_or(Value::Null);
            match ctx.resolve_callable(&inner)? {
                Callable::User { func, .. } => Ok(Info {
                    name: func.f.name_bytes.clone(),
                    class: func.class,
                    func: Some(func),
                    closure: Some(c.clone()),
                    ..empty
                }),
                Callable::Native(id) => {
                    let n = *ctx.native(id);
                    Ok(Info {
                        name: Box::from(n.name.as_bytes()),
                        native: Some(n),
                        closure: Some(c.clone()),
                        ..empty
                    })
                }
                Callable::NativeMethod { method, .. } => Ok(Info {
                    name: method.name.clone(),
                    class: Some(method.decl),
                    method: Some(method),
                    closure: Some(c.clone()),
                    ..empty
                }),
            }
        }
    }
}

/// The target of the reflector `o`.
fn target(o: &Object) -> Result<FnTarget, Unwind> {
    let s: FnState = state(o)?;
    Ok(s.target)
}

/// The resolved target of the reflector `o`.
fn recv(ctx: &Ctx, o: Option<&Object>) -> Result<Info, Unwind> {
    info(ctx, &target(this(o)?)?)
}

// ---- building reflectors ---------------------------------------------------

/// A `ReflectionMethod` over `m`, with php's `$name` / `$class` slots.
pub(crate) fn make_method(ctx: &mut Ctx, m: &Rc<MethodDef>) -> Result<Value, Unwind> {
    let st = FnState {
        target: FnTarget::Method {
            cid: m.decl,
            name: m.name.clone(),
        },
        this: None,
    };
    let o = new_reflector(ctx, "ReflectionMethod", st)?;
    o.set(b"name", Value::string(&m.name));
    let class = ctx.class(m.decl).name.clone();
    o.set(b"class", Value::string(&class));
    Ok(Value::Object(o))
}

/// A `ReflectionFunction` over a target.
fn make_function(ctx: &mut Ctx, t: FnTarget) -> Result<Value, Unwind> {
    let name = info(ctx, &t)?.name;
    let o = new_reflector(
        ctx,
        "ReflectionFunction",
        FnState {
            target: t,
            this: None,
        },
    )?;
    o.set(b"name", Value::string(&name));
    Ok(Value::Object(o))
}

/// A `ReflectionParameter` for position `index` of `owner`.
fn make_parameter(
    ctx: &mut Ctx,
    owner: &FnTarget,
    index: usize,
    name: &[u8],
) -> Result<Value, Unwind> {
    let st = ParamState {
        owner: owner.clone(),
        index,
    };
    let o = new_reflector(ctx, "ReflectionParameter", st)?;
    o.set(b"name", Value::string(name));
    Ok(Value::Object(o))
}

// ---- constructors ----------------------------------------------------------

/// `ReflectionFunction::__construct(Closure|string $function)`
fn function_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let t = function_target(ctx, &args[0])?;
    let name = info(ctx, &t)?.name;
    recv.set(b"name", Value::string(&name));
    store(
        recv,
        FnState {
            target: t,
            this: None,
        },
    );
    Ok(Value::Null)
}

/// Resolve a `Closure|string` into a target, with php's missing-function
/// wording.
fn function_target(ctx: &mut Ctx, v: &Value) -> Result<FnTarget, Unwind> {
    if let Value::Closure(c) = &*v.deref() {
        return Ok(FnTarget::Closure(c.clone()));
    }
    let name = str_arg(v);
    let bare = name.strip_prefix(b"\\").unwrap_or(&name);
    if let Some(id) = ctx.user_function(bare) {
        return Ok(FnTarget::User(id));
    }
    if let Some(id) = ctx.native_by_name(bare) {
        return Ok(FnTarget::Native(id));
    }
    Err(refl_error(format!(
        "Function {}() does not exist",
        String::from_utf8_lossy(bare)
    )))
}

/// `ReflectionMethod::__construct(object|string $objectOrClass, ?string $method = null)`
///
/// php 8.5 deprecated the one-argument `"Class::method"` spelling in favour
/// of `ReflectionMethod::createFromMethodName()`.
fn method_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let (cid, mname) = match opt_arg(args, 1) {
        Some(m) => (super::common::class_arg(ctx, &args[0])?, str_arg(&m)),
        None => {
            ctx.deprecated(
                "Calling ReflectionMethod::__construct() with 1 argument is deprecated, use ReflectionMethod::createFromMethodName() instead",
            )?;
            split_method_name(ctx, &args[0])?
        }
    };
    seed_method(ctx, recv, cid, &mname)
}

/// `ReflectionMethod::createFromMethodName(string $method): static`
fn method_from_name(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let (cid, mname) = split_method_name(ctx, &args[0])?;
    let m = ctx.resolve_method(cid, &mname).ok_or_else(|| {
        refl_error(format!(
            "Method {}::{}() does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(&mname)
        ))
    })?;
    make_method(ctx, &m)
}

/// `"Class::method"` split into its two halves.
fn split_method_name(ctx: &mut Ctx, v: &Value) -> Result<(u32, Vec<u8>), Unwind> {
    let s = str_arg(v);
    let Some(pos) = s.windows(2).position(|w| w == b"::") else {
        return Err(refl_error(format!(
            "Invalid method name {}",
            String::from_utf8_lossy(&s)
        )));
    };
    let cid = super::common::class_by_name_or_error(ctx, &s[..pos])?;
    Ok((cid, s[pos + 2..].to_vec()))
}

/// Fill a `ReflectionMethod` reflector, raising php's error when the method
/// is missing.
fn seed_method(ctx: &mut Ctx, recv: &Object, cid: u32, mname: &[u8]) -> NativeResult {
    let m = ctx.resolve_method(cid, mname).ok_or_else(|| {
        refl_error(format!(
            "Method {}::{}() does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(mname)
        ))
    })?;
    recv.set(b"name", Value::string(&m.name));
    let class = ctx.class(m.decl).name.clone();
    recv.set(b"class", Value::string(&class));
    store(
        recv,
        FnState {
            target: FnTarget::Method {
                cid: m.decl,
                name: m.name.clone(),
            },
            this: None,
        },
    );
    Ok(Value::Null)
}

// ---- the shared surface ----------------------------------------------------

/// `ReflectionFunctionAbstract::getName(): string`
fn get_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(&recv(ctx, o)?.name))
}

/// `ReflectionFunctionAbstract::getShortName(): string`
fn get_short_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let n = recv(ctx, o)?.name;
    Ok(Value::string(short_name(&n)))
}

/// `ReflectionFunctionAbstract::getNamespaceName(): string`
fn get_namespace_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let n = recv(ctx, o)?.name;
    Ok(Value::string(namespace_name(&n)))
}

/// `ReflectionFunctionAbstract::inNamespace(): bool`
fn in_namespace(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let n = recv(ctx, o)?.name;
    Ok(Value::Bool(!namespace_name(&n).is_empty()))
}

/// `ReflectionFunctionAbstract::isInternal(): bool`
fn is_internal(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(i.func.is_none()))
}

/// `ReflectionFunctionAbstract::isUserDefined(): bool`
fn is_user_defined(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(i.func.is_some()))
}

/// `ReflectionFunctionAbstract::getFileName(): string|false`
fn get_file_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    match recv(ctx, o)?.func {
        Some(f) => Ok(Value::string(f.unit.file.as_bytes())),
        None => Ok(Value::Bool(false)),
    }
}

/// `ReflectionFunctionAbstract::getStartLine(): int|false`
fn get_start_line(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    match recv(ctx, o)?.func {
        Some(f) => Ok(Value::Int(i64::from(f.f.decl_line))),
        None => Ok(Value::Bool(false)),
    }
}

/// `ReflectionFunctionAbstract::getEndLine(): int|false`
fn get_end_line(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    match recv(ctx, o)?.func {
        Some(f) => Ok(Value::Int(i64::from(f.f.end_line))),
        None => Ok(Value::Bool(false)),
    }
}

/// `ReflectionFunctionAbstract::getDocComment(): string|false`
///
/// **Known divergence.** php's scanner keeps the last docblock it saw and
/// gives it to the next declaration it opens, so `/** … */ $f = function
/// () {};` documents the *closure*. The parser here asks for the docblock
/// directly before the `function` keyword, so that spelling finds none.
fn get_doc_comment(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    match recv(ctx, o)?.func.as_ref().and_then(|f| f.f.doc.clone()) {
        Some(d) => Ok(Value::string(&d)),
        None => Ok(Value::Bool(false)),
    }
}

/// `ReflectionFunctionAbstract::getNumberOfParameters(): int`
fn num_parameters(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let n = match (&i.func, &i.native) {
        (Some(f), _) => f.f.params.len(),
        (None, Some(n)) => n.max_args.unwrap_or(n.min_args) as usize,
        (None, None) => native_method_arity(&i).map_or(0, |(min, max)| max.unwrap_or(min) as usize),
    };
    Ok(Value::Int(n as i64))
}

/// `ReflectionFunctionAbstract::getNumberOfRequiredParameters(): int`
fn num_required_parameters(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let n = match (&i.func, &i.native) {
        (Some(f), _) => f.f.required_params(),
        (None, Some(n)) => n.min_args as usize,
        (None, None) => native_method_arity(&i).map_or(0, |(min, _)| min as usize),
    };
    Ok(Value::Int(n as i64))
}

/// The declared arity of a native *method*, when that is what the target is.
fn native_method_arity(i: &Info) -> Option<(u8, Option<u8>)> {
    match i.method.as_ref().map(|m| &m.body) {
        Some(MethodBody::Native(d)) => Some((d.min_args, d.max_args)),
        _ => None,
    }
}

/// The parameter *names* a native declares, if php-src's stub named them.
fn native_param_names(i: &Info) -> &'static [&'static str] {
    if let Some(n) = &i.native {
        return n.params;
    }
    match i.method.as_ref().map(|m| &m.body) {
        Some(MethodBody::Native(d)) => d.params,
        _ => &[],
    }
}

/// `ReflectionFunctionAbstract::getParameters(): array`
fn get_parameters(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t = target(this(o)?)?;
    let i = info(ctx, &t)?;
    let mut out = Vec::new();
    if let Some(f) = &i.func {
        let names: Vec<Box<[u8]>> = f.f.params.iter().map(|p| p.name.clone()).collect();
        for (idx, name) in names.iter().enumerate() {
            out.push(make_parameter(ctx, &t, idx, name)?);
        }
        return Ok(list(out));
    }
    for (idx, name) in native_param_names(&i).iter().enumerate() {
        out.push(make_parameter(ctx, &t, idx, name.as_bytes())?);
    }
    Ok(list(out))
}

/// `ReflectionFunctionAbstract::hasReturnType(): bool`
fn has_return_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let has = i.func.as_ref().is_some_and(|f| f.f.ret_ty.is_some());
    Ok(Value::Bool(has))
}

/// `ReflectionFunctionAbstract::getReturnType(): ?ReflectionType`
fn get_return_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let scope = i.scope();
    let Some(s) = i
        .func
        .as_ref()
        .and_then(|f| f.f.ret_ty.as_ref().map(ToString::to_string))
    else {
        return Ok(Value::Null);
    };
    types::from_string(ctx, &s, scope)
}

/// `ReflectionFunctionAbstract::isVariadic(): bool`
fn is_variadic(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let v = match (&i.func, &i.native) {
        (Some(f), _) => f.f.params.last().is_some_and(|p| p.variadic),
        (None, Some(n)) => n.max_args.is_none(),
        (None, None) => native_method_arity(&i).is_some_and(|(_, max)| max.is_none()),
    };
    Ok(Value::Bool(v))
}

/// `ReflectionFunctionAbstract::isGenerator(): bool`
fn is_generator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(
        i.func.as_ref().is_some_and(|f| f.f.is_generator()),
    ))
}

/// `ReflectionFunctionAbstract::isStatic(): bool` — a method's `static`
/// modifier, or whether a closure was declared `static function`.
fn is_static(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    if let Some(m) = &i.method {
        return Ok(Value::Bool(m.is_static));
    }
    if let Some(c) = &i.closure {
        return Ok(Value::Bool(ctx.closure_is_static(c)));
    }
    Ok(Value::Bool(false))
}

/// `ReflectionFunctionAbstract::getAttributes(?string $name = null, int $flags = 0): array`
fn get_attributes(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let t = target(this(o)?)?;
    let i = info(ctx, &t)?;
    let infos: Vec<AttrInfo> = match &i.func {
        Some(f) => attr_list!(&f.f.attrs, t.clone()),
        None => Vec::new(),
    };
    attrs::filtered(ctx, infos, args)
}

/// `ReflectionFunctionAbstract::getClosureThis(): ?object`
fn get_closure_this(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let bound = i.closure.as_ref().and_then(|c| ctx.closure_this(c));
    match bound {
        Some(t) => Ok(Value::Object(t)),
        None => Ok(Value::Null),
    }
}

/// `ReflectionFunctionAbstract::getClosureScopeClass(): ?ReflectionClass`
fn get_closure_scope_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let scope = i.closure.as_ref().and_then(|c| ctx.closure_scope(c));
    match scope {
        Some(cid) => class::make_class(ctx, "ReflectionClass", cid),
        None => Ok(Value::Null),
    }
}

/// `ReflectionFunctionAbstract::getClosureCalledClass(): ?ReflectionClass`
/// — the `static::` class a bound closure carries, which is the one php
/// answers `get_called_class()` with inside it.
fn get_closure_called_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let called = i.closure.as_ref().and_then(|c| ctx.closure_called_class(c));
    match called {
        Some(cid) => class::make_class(ctx, "ReflectionClass", cid),
        None => Ok(Value::Null),
    }
}

/// `ReflectionFunctionAbstract::getClosureUsedVariables(): array` — what a
/// closure captured, by name. The compiled body records a capture by the
/// register it lands in, and the same body's variable table gives that
/// register its name.
fn get_closure_used_variables(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let (Some(c), Some(f)) = (i.closure.clone(), i.func.clone()) else {
        return Ok(Value::empty_array());
    };
    let mut a = rphp_value::Array::new();
    for (idx, cap) in f.f.captures.iter().enumerate() {
        let Some(value) = c.captures().get(idx) else {
            break;
        };
        let Some((name, _)) = f.f.var_names.iter().find(|(_, reg)| *reg == cap.dst) else {
            continue;
        };
        // **Known divergence.** php marks a `use (&$x)` capture as the
        // reference it is (`&int(1)` in a dump); the value that comes back
        // here is the one behind the cell.
        a.set(rphp_value::ArrayKey::str(name), value.clone());
    }
    Ok(Value::Array(a))
}

/// `ReflectionFunctionAbstract::getExtensionName(): string|false` — the
/// extension a function comes from, `false` for one the program declares.
fn get_extension_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    if i.func.is_some() {
        return Ok(Value::Bool(false));
    }
    let name = String::from_utf8_lossy(&i.name).into_owned();
    Ok(Value::string(crate::info::extension_of(&name).as_bytes()))
}

/// `ReflectionFunctionAbstract::getExtension(): ?ReflectionExtension` —
/// `ReflectionExtension` is not implemented, so only the answer for a
/// function that belongs to no extension is exact.
fn get_extension(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    if i.func.is_none() {
        return Err(super::common::refl_error(
            "ReflectionFunctionAbstract::getExtension() needs ReflectionExtension, which is not implemented",
        ));
    }
    Ok(Value::Null)
}

/// `ReflectionFunctionAbstract::isDeprecated(): bool` — php marks an
/// internal function deprecated in its own table; nothing here is.
fn is_deprecated(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    recv(ctx, o)?;
    Ok(Value::Bool(false))
}

/// `ReflectionFunctionAbstract::hasTentativeReturnType(): bool` /
/// `getTentativeReturnType(): ?ReflectionType` — a tentative return type is
/// an internal-function annotation this engine does not carry.
fn has_tentative_return_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    recv(ctx, o)?;
    Ok(Value::Bool(false))
}

fn get_tentative_return_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    recv(ctx, o)?;
    Ok(Value::Null)
}

/// `ReflectionFunction::isAnonymous(): bool` — true for a closure, which is
/// the only function php gives no name of its own.
fn is_anonymous(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(i.closure.is_some()))
}

/// `ReflectionFunction::isDisabled(): bool` — `disable_functions` is not
/// implemented, so nothing is disabled.
fn is_disabled(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    recv(ctx, o)?;
    ctx.deprecated(
        "Method ReflectionFunction::isDisabled() is deprecated since 8.0, as ReflectionFunction can no longer be constructed for disabled functions",
    )?;
    Ok(Value::Bool(false))
}

/// The method of the same name an ancestor declares — php's *prototype*.
/// An interface's method wins over a parent class's, which is the order php
/// walks.
fn prototype_of(ctx: &Ctx, cid: u32, name: &[u8]) -> Option<Rc<MethodDef>> {
    let def = ctx.class(cid);
    let lower = name.to_ascii_lowercase();
    for iid in def.interfaces.clone() {
        if let Some(m) = ctx.class(iid).methods.get(lower.as_slice()) {
            return Some(m.clone());
        }
    }
    let mut cur = def.parent;
    while let Some(pid) = cur {
        let p = ctx.class(pid);
        if let Some(m) = p.methods.get(lower.as_slice()) {
            return Some(m.clone());
        }
        cur = p.parent;
    }
    None
}

/// `ReflectionMethod::hasPrototype(): bool`
fn has_prototype(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let (Some(cid), name) = (i.class, i.name.clone()) else {
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(prototype_of(ctx, cid, &name).is_some()))
}

/// `ReflectionMethod::getPrototype(): ReflectionMethod`
fn get_prototype(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let proto = i
        .class
        .and_then(|cid| prototype_of(ctx, cid, &i.name));
    match proto {
        Some(m) => make_method(ctx, &m),
        None => Err(super::common::refl_error(format!(
            "Method {}::{} does not have a prototype",
            i.class
                .map(|cid| ctx.class(cid).name_str().to_string())
                .unwrap_or_default(),
            String::from_utf8_lossy(&i.name)
        ))),
    }
}

/// `ReflectionMethod::isClosure(): bool` — a method is never one.
fn method_is_closure(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    recv(ctx, o)?;
    Ok(Value::Bool(false))
}

// ---- ReflectionMethod ------------------------------------------------------

/// `ReflectionMethod::isPublic(): bool`
fn is_public(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(
        i.method
            .as_ref()
            .is_some_and(|m| m.vis == Visibility::Public),
    ))
}

/// `ReflectionMethod::isProtected(): bool`
fn is_protected(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(
        i.method
            .as_ref()
            .is_some_and(|m| m.vis == Visibility::Protected),
    ))
}

/// `ReflectionMethod::isPrivate(): bool`
fn is_private(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(
        i.method
            .as_ref()
            .is_some_and(|m| m.vis == Visibility::Private),
    ))
}

/// `ReflectionMethod::isAbstract(): bool`
fn is_abstract(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(
        i.method.as_ref().is_some_and(|m| m.is_abstract),
    ))
}

/// `ReflectionMethod::isFinal(): bool`
fn is_final(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(i.method.as_ref().is_some_and(|m| m.is_final)))
}

/// `ReflectionMethod::isConstructor(): bool`
fn is_constructor(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(i.name.eq_ignore_ascii_case(b"__construct")))
}

/// `ReflectionMethod::isDestructor(): bool`
fn is_destructor(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Bool(i.name.eq_ignore_ascii_case(b"__destruct")))
}

/// `ReflectionMethod::getModifiers(): int`
fn get_modifiers(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    Ok(Value::Int(
        i.method.as_ref().map_or(0, |m| method_modifiers(m)),
    ))
}

/// `ReflectionMethod::getDeclaringClass(): ReflectionClass`
fn get_declaring_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    match i.class {
        Some(cid) => class::make_class(ctx, "ReflectionClass", cid),
        None => Ok(Value::Null),
    }
}

/// `ReflectionMethod::setAccessible(bool $accessible): void`
///
/// A no-op since 8.1 (Reflection ignores visibility), and deprecated in 8.5.
fn set_accessible(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    ctx.deprecated(
        "Method ReflectionMethod::setAccessible() is deprecated since 8.5, as it has no effect",
    )?;
    Ok(Value::Null)
}

/// `ReflectionMethod::invoke(?object $object = null, mixed ...$args): mixed`
fn method_invoke(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let inst = args.first().and_then(obj_arg);
    let rest: Vec<Value> = args
        .iter()
        .skip(1)
        .map(|v| v.deref().into_owned())
        .collect();
    invoke_method(ctx, this(o)?, inst, &rest)
}

/// `ReflectionMethod::invokeArgs(?object $object, array $args): mixed`
fn method_invoke_args(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let inst = args.first().and_then(obj_arg);
    let rest = match opt_arg(args, 1) {
        Some(Value::Array(a)) => a.values().map(|v| v.deref().into_owned()).collect(),
        _ => Vec::new(),
    };
    invoke_method(ctx, this(o)?, inst, &rest)
}

/// The body behind both, with php's three `ReflectionException`s: no object
/// for an instance method, the wrong object, and an abstract method.
fn invoke_method(
    ctx: &mut Ctx,
    recv_obj: &Object,
    inst: Option<Object>,
    args: &[Value],
) -> NativeResult {
    let t = target(recv_obj)?;
    let i = info(ctx, &t)?;
    let Some(m) = i.method.clone() else {
        return Err(refl_error(
            "Internal error: Failed to retrieve the reflection object",
        ));
    };
    let cname = ctx.class(m.decl).name_str();
    if m.is_abstract {
        return Err(refl_error(format!(
            "Trying to invoke abstract method {cname}::{}()",
            String::from_utf8_lossy(&m.name)
        )));
    }
    if !m.is_static {
        let Some(obj) = inst else {
            return Err(refl_error(format!(
                "Trying to invoke non static method {cname}::{}() without an object",
                String::from_utf8_lossy(&m.name)
            )));
        };
        if !ctx.object_instanceof(&obj, m.decl) {
            return Err(refl_error(
                "Given object is not an instance of the class this method was declared in",
            ));
        }
        return dispatch(ctx, &m, Some(obj), args);
    }
    dispatch(ctx, &m, None, args)
}

/// Call a resolved method, bypassing visibility the way Reflection does.
fn dispatch(
    ctx: &mut Ctx,
    m: &Rc<MethodDef>,
    this_obj: Option<Object>,
    args: &[Value],
) -> NativeResult {
    let static_class = this_obj.as_ref().map_or(m.decl, |o| o.class_id());
    let callable = match &m.body {
        MethodBody::User(f) => Callable::User {
            func: f.clone(),
            this: this_obj,
            scope: Some(m.decl),
            static_class: Some(static_class),
            closure: None,
        },
        MethodBody::Native(_) => Callable::NativeMethod {
            method: m.clone(),
            this: this_obj,
        },
    };
    ctx.call_resolved(callable, args)
}

/// `ReflectionMethod::getClosure(?object $object = null): Closure`
fn method_get_closure(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let i = recv(ctx, o)?;
    let Some(m) = i.method.clone() else {
        return Err(refl_error(
            "Internal error: Failed to retrieve the reflection object",
        ));
    };
    let inst = args.first().and_then(obj_arg);
    if !m.is_static && inst.is_none() {
        return Err(Unwind::value_error(
            "ReflectionMethod::getClosure(): Argument #1 ($object) cannot be null for non-static methods",
        ));
    }
    match &m.body {
        // A compiled body becomes a closure directly: the engine reads
        // `[$this, scope, called class]` off the end of a closure's captures,
        // so binding it here bypasses visibility exactly as php does.
        MethodBody::User(f) => {
            let this_v = inst
                .as_ref()
                .map_or(Value::Null, |o| Value::Object(o.clone()));
            let called = inst.as_ref().map_or(m.decl, |o| o.class_id());
            let caps = vec![
                this_v,
                Value::Int(i64::from(m.decl)),
                Value::Int(i64::from(called)),
            ];
            Ok(Value::Closure(Closure::new(f.id, caps)))
        }
        // A native method has no function id, so the engine's own
        // `Closure::fromCallable` path builds the wrapper.
        MethodBody::Native(_) => {
            let callee = match inst {
                Some(obj) => {
                    let mut a = Array::new();
                    a.push(Value::Object(obj));
                    a.push(Value::string(&m.name));
                    Value::Array(a)
                }
                None => {
                    let mut s = ctx.class(m.decl).name.to_vec();
                    s.extend_from_slice(b"::");
                    s.extend_from_slice(&m.name);
                    Value::string(&s)
                }
            };
            ctx.closure_from_callable(&callee)
        }
    }
}

// ---- ReflectionFunction ----------------------------------------------------

/// `ReflectionFunction::isClosure(): bool`
fn is_closure(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(
        target(this(o)?)?,
        FnTarget::Closure(_)
    )))
}

/// `ReflectionFunction::invoke(mixed ...$args): mixed`
fn function_invoke(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let vals: Vec<Value> = args.iter().map(|v| v.deref().into_owned()).collect();
    invoke_function(ctx, this(o)?, &vals)
}

/// `ReflectionFunction::invokeArgs(array $args): mixed`
fn function_invoke_args(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let vals: Vec<Value> = match opt_arg(args, 0) {
        Some(Value::Array(a)) => a.values().map(|v| v.deref().into_owned()).collect(),
        _ => Vec::new(),
    };
    invoke_function(ctx, this(o)?, &vals)
}

/// Call the reflected function.
fn invoke_function(ctx: &mut Ctx, recv_obj: &Object, args: &[Value]) -> NativeResult {
    let t = target(recv_obj)?;
    let callee = callable_value(ctx, &t)?;
    ctx.call_value(&callee, args)
}

/// `ReflectionFunction::getClosure(): Closure`
fn function_get_closure(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t = target(this(o)?)?;
    if let FnTarget::Closure(c) = &t {
        return Ok(Value::Closure(c.clone()));
    }
    let callee = callable_value(ctx, &t)?;
    ctx.closure_from_callable(&callee)
}

/// The php callable value a function target names.
fn callable_value(ctx: &Ctx, t: &FnTarget) -> Result<Value, Unwind> {
    Ok(match t {
        FnTarget::Closure(c) => Value::Closure(c.clone()),
        FnTarget::User(id) => Value::string(&ctx.func(*id).f.name_bytes),
        FnTarget::Native(id) => Value::string(ctx.native(*id).name.as_bytes()),
        FnTarget::Method { cid, name } => {
            let mut s = ctx.class(*cid).name.to_vec();
            s.extend_from_slice(b"::");
            s.extend_from_slice(name);
            Value::string(&s)
        }
    })
}

// ---- ReflectionParameter ---------------------------------------------------

/// The parameter a `ReflectionParameter` names, with its owner resolved.
fn param_of(ctx: &Ctx, o: &Object) -> Result<(ParamState, Info), Unwind> {
    let s: ParamState = state(o)?;
    let i = info(ctx, &s.owner)?;
    Ok((s, i))
}

/// `ReflectionParameter::__construct(callable $function, int|string $param)`
fn parameter_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv_obj = this(o)?;
    let owner = callable_target(ctx, &args[0])?;
    let i = info(ctx, &owner)?;
    let names: Vec<Box<[u8]>> = match &i.func {
        Some(f) => f.f.params.iter().map(|p| p.name.clone()).collect(),
        None => native_param_names(&i)
            .iter()
            .map(|n| Box::from(n.as_bytes()))
            .collect(),
    };
    let wanted = args[1].deref().into_owned();
    let index = match &wanted {
        Value::Str(_) => {
            let n = str_arg(&wanted);
            names.iter().position(|p| p.as_ref() == n.as_slice())
        }
        other => {
            let idx = other.to_int();
            (idx >= 0 && (idx as usize) < names.len()).then_some(idx as usize)
        }
    };
    // A negative offset is php's `ValueError` before it is a lookup at all.
    if let Value::Int(n) = &wanted {
        if *n < 0 {
            return Err(Unwind::value_error(
                "ReflectionParameter::__construct(): Argument #2 ($param) must be greater than or equal to 0",
            ));
        }
    }
    let Some(index) = index else {
        return Err(refl_error(format!(
            "The parameter specified by its {} could not be found",
            if matches!(wanted, Value::Str(_)) {
                "name"
            } else {
                "offset"
            }
        )));
    };
    recv_obj.set(b"name", Value::string(&names[index]));
    store(recv_obj, ParamState { owner, index });
    Ok(Value::Null)
}

/// php's rendering of a default value inside `__toString()`: `NULL`,
/// `true`, `'text'`, `[0 => 1, 'k' => 2]`, a number as written.
fn export_default(v: &Value) -> String {
    match &*v.deref() {
        Value::Null | Value::Uninit => "NULL".to_string(),
        Value::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
            let s = f.to_string();
            if s.contains(['.', 'e', 'E', 'n', 'i']) {
                s
            } else {
                format!("{s}.0")
            }
        }
        Value::Str(st) => format!("'{}'", String::from_utf8_lossy(st.as_bytes())),
        Value::Array(a) => {
            let parts: Vec<String> = a
                .iter()
                .map(|(k, v)| {
                    let key = match &k {
                        rphp_value::ArrayKey::Int(n) => n.to_string(),
                        rphp_value::ArrayKey::Str(s) => {
                            format!("'{}'", String::from_utf8_lossy(s))
                        }
                    };
                    format!("{key} => {}", export_default(&v))
                })
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(o) => format!(
            "new \\{}()",
            String::from_utf8_lossy(o.layout().class_name())
        ),
        other => other.to_php_string(),
    }
}

/// `ReflectionParameter::__toString(): string` — php's one-line
/// description, which code in the wild parses:
///
/// ```text
/// Parameter #1 [ <optional> ?string $b = NULL ]
/// ```
///
/// **Known divergence.** php keeps the default *expression* and prints a
/// constant default by name (`$c = MYC`); the compiled parameter here keeps
/// the value, so the value is what is printed.
fn param_to_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let recv_obj = this(o)?;
    let index = param_get_position(ctx, o, &mut [])?.to_int();
    let name = recv_obj
        .get_deref(b"name")
        .map(|v| v.to_php_bytes().to_vec())
        .unwrap_or_default();
    let mut out = format!("Parameter #{index} [ ");
    let optional = param_is_optional(ctx, o, &mut [])?.to_bool();
    out.push_str(if optional { "<optional> " } else { "<required> " });
    let ty = param_get_type(ctx, o, &mut [])?;
    if let Value::Object(t) = &ty {
        let text = ctx.call_method(t, b"__toString", &[])?;
        out.push_str(&String::from_utf8_lossy(&text.to_php_bytes()));
        out.push(' ');
    }
    if param_by_ref(ctx, o, &mut [])?.to_bool() {
        out.push('&');
    }
    if param_is_variadic(ctx, o, &mut [])?.to_bool() {
        out.push_str("...");
    }
    out.push('$');
    out.push_str(&String::from_utf8_lossy(&name));
    if param_has_default(ctx, o, &mut [])?.to_bool() {
        let d = param_get_default(ctx, o, &mut [])?;
        out.push_str(" = ");
        out.push_str(&export_default(&d));
    }
    out.push_str(" ]");
    Ok(Value::string(out.as_bytes()))
}

/// A `callable` argument as a target: a plain name, `[$obj, 'm']`,
/// `['C', 'm']`, `'C::m'` or a closure.
fn callable_target(ctx: &mut Ctx, v: &Value) -> Result<FnTarget, Unwind> {
    let v = v.deref().into_owned();
    if let Value::Closure(c) = &v {
        return Ok(FnTarget::Closure(c.clone()));
    }
    if let Value::Array(a) = &v {
        let mut it = a.values();
        let (Some(first), Some(second)) = (it.next(), it.next()) else {
            return Err(refl_error(
                "The parameter specified by its offset is invalid",
            ));
        };
        let cid = super::common::class_arg(ctx, &first.deref())?;
        let name = str_arg(&second.deref());
        let m = ctx.resolve_method(cid, &name).ok_or_else(|| {
            refl_error(format!(
                "Method {}::{}() does not exist",
                ctx.class(cid).name_str(),
                String::from_utf8_lossy(&name)
            ))
        })?;
        return Ok(FnTarget::Method {
            cid: m.decl,
            name: m.name.clone(),
        });
    }
    let s = str_arg(&v);
    if s.windows(2).any(|w| w == b"::") {
        let (cid, name) = split_method_name(ctx, &v)?;
        let m = ctx.resolve_method(cid, &name).ok_or_else(|| {
            refl_error(format!(
                "Method {}::{}() does not exist",
                ctx.class(cid).name_str(),
                String::from_utf8_lossy(&name)
            ))
        })?;
        return Ok(FnTarget::Method {
            cid: m.decl,
            name: m.name.clone(),
        });
    }
    function_target(ctx, &v)
}

/// `ReflectionParameter::getName(): string`
fn param_get_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    Ok(Value::string(&param_name(&i, s.index)))
}

/// The declared name of parameter `index`.
fn param_name(i: &Info, index: usize) -> Box<[u8]> {
    if let Some(f) = &i.func {
        return f
            .f
            .params
            .get(index)
            .map_or_else(|| Box::from(&b""[..]), |p| p.name.clone());
    }
    native_param_names(i)
        .get(index)
        .map_or_else(|| Box::from(&b""[..]), |n| Box::from(n.as_bytes()))
}

/// `ReflectionParameter::getPosition(): int`
fn param_get_position(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, _) = param_of(ctx, this(o)?)?;
    Ok(Value::Int(s.index as i64))
}

/// The canonical spelling of a parameter's declared type, if it has one.
fn param_type_string(i: &Info, index: usize) -> Option<String> {
    let p = i.func.as_ref().and_then(|f| f.f.params.get(index))?;
    let ty = p.ty.as_ref().map(ToString::to_string)?;
    // php's *implicitly nullable* parameter: a single declared type with a
    // literal `null` default accepts null as well, and Reflection shows the
    // `?`. A composite type already spells its own null, so it is left as
    // written.
    let composite = ty.contains('|') || ty.contains('&');
    if !composite
        && !ty.starts_with('?')
        && !ty.eq_ignore_ascii_case("mixed")
        && !ty.eq_ignore_ascii_case("null")
        && default_is_null(i, index)
    {
        return Some(format!("?{ty}"));
    }
    Some(ty)
}

/// Whether the parameter's default is the literal `null` — the only shape
/// php's implicit nullability keys on.
fn default_is_null(i: &Info, index: usize) -> bool {
    let Some(f) = i.func.as_ref() else {
        return false;
    };
    match param_default(i, index) {
        Some(InitKey::Const(idx)) => f
            .f
            .consts
            .get(idx as usize)
            .and_then(|c| c.try_to_value())
            .is_some_and(|v| matches!(v, Value::Null)),
        _ => false,
    }
}

/// `ReflectionParameter::hasType(): bool`
fn param_has_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    Ok(Value::Bool(param_type_string(&i, s.index).is_some()))
}

/// `ReflectionParameter::getType(): ?ReflectionType`
fn param_get_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let scope = i.scope();
    match param_type_string(&i, s.index) {
        Some(t) => types::from_string(ctx, &t, scope),
        None => Ok(Value::Null),
    }
}

/// `ReflectionParameter::allowsNull(): bool` — an untyped parameter accepts
/// anything.
fn param_allows_null(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let scope = i.scope();
    let allows = match param_type_string(&i, s.index) {
        Some(t) => types::parse(ctx, &t, scope).nullable,
        None => true,
    };
    Ok(Value::Bool(allows))
}

/// `ReflectionParameter::isOptional(): bool`
fn param_is_optional(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let required = match (&i.func, &i.native) {
        (Some(f), _) => f.f.required_params(),
        (None, Some(n)) => n.min_args as usize,
        (None, None) => native_method_arity(&i).map_or(0, |(min, _)| min as usize),
    };
    Ok(Value::Bool(s.index >= required))
}

/// `ReflectionParameter::isVariadic(): bool`
fn param_is_variadic(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let v = match &i.func {
        Some(f) => f.f.params.get(s.index).is_some_and(|p| p.variadic),
        None => {
            let names = native_param_names(&i);
            let last = !names.is_empty() && s.index + 1 == names.len();
            last && match (&i.native, native_method_arity(&i)) {
                (Some(n), _) => n.max_args.is_none(),
                (None, Some((_, max))) => max.is_none(),
                (None, None) => false,
            }
        }
    };
    Ok(Value::Bool(v))
}

/// `ReflectionParameter::isPassedByReference(): bool`
fn param_by_ref(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let by_ref = match (&i.func, &i.native) {
        (Some(f), _) => f.f.params.get(s.index).is_some_and(|p| p.by_ref),
        (None, Some(n)) => n.is_by_ref(s.index),
        (None, None) => match i.method.as_ref().map(|m| &m.body) {
            Some(MethodBody::Native(d)) => d.is_by_ref(s.index),
            _ => false,
        },
    };
    Ok(Value::Bool(by_ref))
}

/// `ReflectionParameter::canBePassedByValue(): bool`
fn param_by_value(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = param_by_ref(ctx, o, args)?;
    Ok(Value::Bool(!r.to_bool()))
}

/// `ReflectionParameter::isPromoted(): bool`
fn param_is_promoted(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let p = i
        .func
        .as_ref()
        .and_then(|f| f.f.params.get(s.index))
        .is_some_and(|p| p.promoted.is_some());
    Ok(Value::Bool(p))
}

/// The compiled default of parameter `index`, when it has one.
fn param_default(i: &Info, index: usize) -> Option<InitKey> {
    let p = i.func.as_ref()?.f.params.get(index)?;
    let d = p.default.as_ref()?;
    parse_init_ref(&format!("{d:?}"))
}

/// `ReflectionParameter::isDefaultValueAvailable(): bool`
fn param_has_default(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    Ok(Value::Bool(param_default(&i, s.index).is_some()))
}

/// `ReflectionParameter::getDefaultValue(): mixed`
fn param_get_default(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let failed = || refl_error("Internal error: Failed to retrieve the default value");
    let Some(k) = param_default(&i, s.index) else {
        return Err(failed());
    };
    let f = i.func.clone().ok_or_else(failed)?;
    match k {
        InitKey::Const(idx) => {
            f.f.consts
                .get(idx as usize)
                .and_then(|c| c.try_to_value())
                .ok_or_else(failed)
        }
        // A non-foldable default is a zero-argument thunk, re-run on every
        // read exactly as php re-evaluates a parameter default per call.
        InitKey::Thunk(local) => {
            let fid = f.unit.func_base + local;
            run_thunk(ctx, fid, i.class)
        }
    }
}

/// `ReflectionParameter::getDeclaringFunction(): ReflectionFunctionAbstract`
fn param_declaring_function(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ParamState = state(this(o)?)?;
    match &s.owner {
        FnTarget::Method { cid, name } => {
            let m = ctx.resolve_method(*cid, name).ok_or_else(|| {
                refl_error("Internal error: Failed to retrieve the reflection object")
            })?;
            make_method(ctx, &m)
        }
        other => make_function(ctx, other.clone()),
    }
}

/// `ReflectionParameter::getDeclaringClass(): ?ReflectionClass`
fn param_declaring_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let (_, i) = param_of(ctx, this(o)?)?;
    match i.class {
        Some(cid) => class::make_class(ctx, "ReflectionClass", cid),
        None => Ok(Value::Null),
    }
}

/// `ReflectionParameter::getAttributes(?string $name = null, int $flags = 0): array`
fn param_get_attributes(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let (s, i) = param_of(ctx, this(o)?)?;
    let owner = s.owner.clone();
    let infos: Vec<AttrInfo> = match i.func.as_ref().and_then(|f| f.f.params.get(s.index)) {
        Some(p) => attr_list!(&p.attrs, owner.clone()),
        None => Vec::new(),
    };
    attrs::filtered(ctx, infos, args)
}

// ---- registration ----------------------------------------------------------

/// Register `ReflectionFunctionAbstract` and the four classes over it.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ReflectionFunctionAbstract")
        .flags(rphp_runtime::ClassFlags::ABSTRACT)
        .implements(&["Reflector"])
        .prop("name", Visibility::Public, Value::string(b""))
        .method("getName", nm!(0, Some(0), get_name))
        .method("getShortName", nm!(0, Some(0), get_short_name))
        .method("getNamespaceName", nm!(0, Some(0), get_namespace_name))
        .method("inNamespace", nm!(0, Some(0), in_namespace))
        .method("isInternal", nm!(0, Some(0), is_internal))
        .method("isUserDefined", nm!(0, Some(0), is_user_defined))
        .method("getFileName", nm!(0, Some(0), get_file_name))
        .method("getStartLine", nm!(0, Some(0), get_start_line))
        .method("getEndLine", nm!(0, Some(0), get_end_line))
        .method("getDocComment", nm!(0, Some(0), get_doc_comment))
        .method("getParameters", nm!(0, Some(0), get_parameters))
        .method("getNumberOfParameters", nm!(0, Some(0), num_parameters))
        .method(
            "getNumberOfRequiredParameters",
            nm!(0, Some(0), num_required_parameters),
        )
        .method("hasReturnType", nm!(0, Some(0), has_return_type))
        .method("getReturnType", nm!(0, Some(0), get_return_type))
        .method("isVariadic", nm!(0, Some(0), is_variadic))
        .method("isGenerator", nm!(0, Some(0), is_generator))
        .method("isStatic", nm!(0, Some(0), is_static))
        .method("getAttributes", nm!(0, Some(2), get_attributes))
        .method("getClosureThis", nm!(0, Some(0), get_closure_this))
        .method(
            "getClosureCalledClass",
            nm!(0, Some(0), get_closure_called_class),
        )
        .method(
            "getClosureUsedVariables",
            nm!(0, Some(0), get_closure_used_variables),
        )
        .method("isDeprecated", nm!(0, Some(0), is_deprecated))
        .method("getExtensionName", nm!(0, Some(0), get_extension_name))
        .method("getExtension", nm!(0, Some(0), get_extension))
        .method(
            "hasTentativeReturnType",
            nm!(0, Some(0), has_tentative_return_type),
        )
        .method(
            "getTentativeReturnType",
            nm!(0, Some(0), get_tentative_return_type),
        )
        .method(
            "getClosureScopeClass",
            nm!(0, Some(0), get_closure_scope_class),
        )
        .finish();

    r.class("ReflectionFunction")
        .extends("ReflectionFunctionAbstract")
        .class_const("IS_DEPRECATED", Value::Int(2048))
        .method("__construct", nm!(1, Some(1), function_construct))
        .method("isClosure", nm!(0, Some(0), is_closure))
        .method("isAnonymous", nm!(0, Some(0), is_anonymous))
        .method("isDisabled", nm!(0, Some(0), is_disabled))
        .method("invoke", nm!(0, None, function_invoke))
        .method("invokeArgs", nm!(1, Some(1), function_invoke_args))
        .method("getClosure", nm!(0, Some(0), function_get_closure))
        .finish();

    r.class("ReflectionMethod")
        .extends("ReflectionFunctionAbstract")
        .prop("class", Visibility::Public, Value::string(b""))
        .class_const("IS_STATIC", Value::Int(IS_STATIC))
        .class_const("IS_PUBLIC", Value::Int(IS_PUBLIC))
        .class_const("IS_PROTECTED", Value::Int(IS_PROTECTED))
        .class_const("IS_PRIVATE", Value::Int(IS_PRIVATE))
        .class_const("IS_ABSTRACT", Value::Int(IS_ABSTRACT))
        .class_const("IS_FINAL", Value::Int(IS_FINAL))
        .method("__construct", nm!(1, Some(2), method_construct))
        .method(
            "createFromMethodName",
            super::common::snm(1, Some(1), method_from_name),
        )
        .method("isPublic", nm!(0, Some(0), is_public))
        .method("isProtected", nm!(0, Some(0), is_protected))
        .method("isPrivate", nm!(0, Some(0), is_private))
        .method("isAbstract", nm!(0, Some(0), is_abstract))
        .method("isFinal", nm!(0, Some(0), is_final))
        .method("isClosure", nm!(0, Some(0), method_is_closure))
        .method("hasPrototype", nm!(0, Some(0), has_prototype))
        .method("getPrototype", nm!(0, Some(0), get_prototype))
        .method("isConstructor", nm!(0, Some(0), is_constructor))
        .method("isDestructor", nm!(0, Some(0), is_destructor))
        .method("getModifiers", nm!(0, Some(0), get_modifiers))
        .method("getDeclaringClass", nm!(0, Some(0), get_declaring_class))
        .method("setAccessible", nm!(1, Some(1), set_accessible))
        .method("invoke", nm!(0, None, method_invoke))
        .method("invokeArgs", nm!(2, Some(2), method_invoke_args))
        .method("getClosure", nm!(0, Some(1), method_get_closure))
        .finish();

    r.class("ReflectionParameter")
        .implements(&["Reflector"])
        .prop("name", Visibility::Public, Value::string(b""))
        .method("__construct", nm!(2, Some(2), parameter_construct))
        .method("getName", nm!(0, Some(0), param_get_name))
        .method("getPosition", nm!(0, Some(0), param_get_position))
        .method("hasType", nm!(0, Some(0), param_has_type))
        .method("getType", nm!(0, Some(0), param_get_type))
        .method("allowsNull", nm!(0, Some(0), param_allows_null))
        .method("isOptional", nm!(0, Some(0), param_is_optional))
        .method("isVariadic", nm!(0, Some(0), param_is_variadic))
        .method("isPassedByReference", nm!(0, Some(0), param_by_ref))
        .method("canBePassedByValue", nm!(0, Some(0), param_by_value))
        .method("isPromoted", nm!(0, Some(0), param_is_promoted))
        .method(
            "isDefaultValueAvailable",
            nm!(0, Some(0), param_has_default),
        )
        .method("getDefaultValue", nm!(0, Some(0), param_get_default))
        .method(
            "getDeclaringFunction",
            nm!(0, Some(0), param_declaring_function),
        )
        .method("getDeclaringClass", nm!(0, Some(0), param_declaring_class))
        .method("getAttributes", nm!(0, Some(2), param_get_attributes))
        .method("__toString", nm!(0, Some(0), param_to_string))
        .finish();
}
