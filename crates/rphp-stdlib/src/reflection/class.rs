//! `ReflectionClass`, `ReflectionObject` and `ReflectionEnum` (php-src
//! `ext/reflection/php_reflection.c`).
//!
//! **What an instance holds.** `public $name` is a real declared slot — php
//! dumps `object(ReflectionClass)#1 (1) { ["name"]=> string(8) "App\Impl" }`
//! — and the process-wide class id lives in the payload as a
//! [`ClassState`](super::common::ClassState). A `ReflectionObject` keeps the
//! instance as well, because php lists that object's dynamic properties on
//! top of the declared ones.
//!
//! **Member order.** php reports a class's own members first and then its
//! ancestors', which is what [`ordered_props`] and the runtime's
//! `ClassDef::method_order` produce. Two of php's exclusions are reproduced
//! here: a private property or method declared in an *ancestor* is left out
//! of `getProperties()`/`getMethods()` (though `hasMethod`/`getMethod` still
//! find the method), and a private constant of an ancestor is not inherited
//! at all.
//!
//! **Known divergences**, all upstream of this module:
//!
//! * `getEndLine()` is always `false` for the same reason — that declaration
//!   records only the first line.
//! * php interleaves static and instance properties in source order within
//!   one class; the runtime splits them into two lists at declaration time,
//!   so instance properties come first here.

use rphp_runtime::{
    nm, Callable, ClassFlags, ClassKind, Ctx, MethodBody, NativeResult, Registry, Unwind,
    Visibility,
};
use rphp_value::{Array, ArrayKey, Object, Value};

use super::common::{
    class_arg, class_chain, class_modifiers, declared_prop, is_enum, key, list, method_modifiers,
    namespace_name, new_reflector, obj_arg, opt_arg, ordered_methods, ordered_props, refl_error,
    set_static_prop, short_name, state, static_prop_value, store, str_arg, this, ClassState,
    IS_ABSTRACT, IS_CLASS_READONLY, IS_FINAL, IS_IMPLICIT_ABSTRACT, IS_PUBLIC,
};
use super::func;
use super::prop;
use super::types;

/// The reflected class of a reflector.
fn cid_of(o: &Object) -> Result<u32, Unwind> {
    let s: ClassState = state(o)?;
    Ok(s.cid)
}

/// Seed a `ReflectionClass`-shaped reflector: the payload plus the public
/// `$name` slot php dumps.
pub(crate) fn seed_class(ctx: &Ctx, o: &Object, cid: u32, inst: Option<Object>) {
    let name = ctx.class(cid).name.clone();
    o.set(b"name", Value::string(&name));
    store(o, ClassState { cid, obj: inst });
}

/// A new `ReflectionClass` (or the subclass `class`) over `cid`.
pub(crate) fn make_class(ctx: &mut Ctx, class: &str, cid: u32) -> Result<Value, Unwind> {
    let o = new_reflector(ctx, class, ClassState { cid, obj: None })?;
    let name = ctx.class(cid).name.clone();
    o.set(b"name", Value::string(&name));
    Ok(Value::Object(o))
}

// ---- construction ----------------------------------------------------------

/// `ReflectionClass::__construct(object|string $objectOrClass)`
fn class_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cid = class_arg(ctx, &args[0])?;
    // php's `ReflectionClass` reports the *class*: even when it was built
    // from an instance it never lists that instance's dynamic properties.
    // Only `ReflectionObject` keeps the object.
    seed_class(ctx, o, cid, None);
    Ok(Value::Null)
}

/// `ReflectionObject::__construct(object $object)`
fn object_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Some(inst) = obj_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "ReflectionObject::__construct(): Argument #1 ($object) must be of type object",
        ));
    };
    let cid = inst.class_id();
    seed_class(ctx, o, cid, Some(inst));
    Ok(Value::Null)
}

/// `ReflectionEnum::__construct(object|string $objectOrClass)`
fn enum_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let cid = class_arg(ctx, &args[0])?;
    if !is_enum(ctx.class(cid)) {
        return Err(refl_error(format!(
            "Class \"{}\" is not an enum",
            ctx.class(cid).name_str()
        )));
    }
    seed_class(ctx, o, cid, None);
    Ok(Value::Null)
}

// ---- names -----------------------------------------------------------------

/// `ReflectionClass::getName(): string`
fn get_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::string(&ctx.class(cid).name))
}

/// `ReflectionClass::getShortName(): string`
fn get_short_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let n = ctx.class(cid).name.clone();
    Ok(Value::string(short_name(&n)))
}

/// `ReflectionClass::getNamespaceName(): string`
fn get_namespace_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let n = ctx.class(cid).name.clone();
    Ok(Value::string(namespace_name(&n)))
}

/// `ReflectionClass::inNamespace(): bool`
fn in_namespace(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let n = ctx.class(cid).name.clone();
    Ok(Value::Bool(!namespace_name(&n).is_empty()))
}

// ---- predicates ------------------------------------------------------------

/// `ReflectionClass::isInterface(): bool`
fn is_interface(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(ctx.class(cid).kind == ClassKind::Interface))
}

/// `ReflectionClass::isTrait(): bool`
fn is_trait(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(ctx.class(cid).kind == ClassKind::Trait))
}

/// `ReflectionClass::isEnum(): bool`
fn is_enum_m(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(is_enum(ctx.class(cid))))
}

/// `ReflectionClass::isAbstract(): bool` — php reports only *classes* as
/// abstract; an interface or trait carries the engine's ABSTRACT bit too but
/// answers `false`.
fn is_abstract(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid);
    let abstract_class = def.kind == ClassKind::Class && def.flags.contains(ClassFlags::ABSTRACT);
    Ok(Value::Bool(abstract_class))
}

/// `ReflectionClass::isFinal(): bool` — every enum is final.
fn is_final(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid);
    Ok(Value::Bool(
        def.flags.contains(ClassFlags::FINAL) || is_enum(def),
    ))
}

/// `ReflectionClass::isReadOnly(): bool`
fn is_readonly(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(
        ctx.class(cid).flags.contains(ClassFlags::READONLY),
    ))
}

/// `ReflectionClass::isAnonymous(): bool`
fn is_anonymous(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(
        ctx.class(cid).flags.contains(ClassFlags::ANONYMOUS),
    ))
}

/// `ReflectionClass::isInstantiable(): bool`
fn is_instantiable(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(ctx.class(cid).is_instantiable()))
}

/// `ReflectionClass::isCloneable(): bool` — instantiable, not an enum case
/// singleton holder, and `__clone` (if any) reachable from outside.
fn is_cloneable(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid).clone();
    if !def.is_instantiable() || is_enum(&def) {
        return Ok(Value::Bool(false));
    }
    let ok = match def.method(b"__clone") {
        Some(m) => m.vis == Visibility::Public,
        None => true,
    };
    Ok(Value::Bool(ok))
}

/// `ReflectionClass::getDefaultProperties(): array` — every declared
/// property's default, static ones first, in php's order.
fn get_default_properties(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid);
    let statics: Vec<(Vec<u8>, Option<rphp_runtime::PropDefault>)> = def
        .static_props
        .iter()
        .map(|p| (p.name.to_vec(), p.init.clone()))
        .collect();
    let instance: Vec<(Vec<u8>, Option<rphp_runtime::PropDefault>)> = def
        .props
        .iter()
        .map(|p| (p.name.to_vec(), Some(p.default.clone())))
        .collect();
    let mut a = Array::new();
    for (name, d) in statics.into_iter().chain(instance) {
        let v = match d {
            Some(rphp_runtime::PropDefault::Value(v)) if v.is_uninit() => continue,
            Some(rphp_runtime::PropDefault::Value(v)) => v,
            Some(rphp_runtime::PropDefault::Thunk(fid)) => {
                super::common::run_thunk(ctx, fid, Some(cid))?
            }
            None => continue,
        };
        a.set(rphp_value::ArrayKey::str(&name), v);
    }
    Ok(Value::Array(a))
}

/// `ReflectionClass::getTraitAliases(): array` — `as` renamings, which the
/// class model records as ordinary methods, so nothing is left to report.
fn get_trait_aliases(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    cid_of(this(o)?)?;
    let _ = ctx;
    Ok(Value::empty_array())
}

/// `ReflectionClass::getExtensionName(): string|false` — the extension a
/// class comes from, `false` for one the program declares.
fn get_extension_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    if !ctx.class(cid).internal {
        return Ok(Value::Bool(false));
    }
    let name = ctx.class(cid).name.clone();
    Ok(Value::string(
        crate::info::extension_of_class(&name).as_bytes(),
    ))
}

/// `ReflectionClass::getExtension(): ?ReflectionExtension` — the class it
/// would answer with is not implemented, so a class that belongs to no
/// extension is the only case this can be exact about.
fn get_extension(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    if ctx.class(cid).internal {
        return Err(super::common::refl_error(
            "ReflectionClass::getExtension() needs ReflectionExtension, which is not implemented",
        ));
    }
    Ok(Value::Null)
}

/// `ReflectionClass::isUninitializedLazyObject(object $object): bool` —
/// there are no lazy objects in this engine, so no object is one.
fn is_uninitialized_lazy_object(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Bool(false))
}

/// `ReflectionClass::isInternal(): bool`
fn is_internal(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(ctx.class(cid).internal))
}

/// `ReflectionClass::isUserDefined(): bool`
fn is_user_defined(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(!ctx.class(cid).internal))
}

/// `ReflectionClass::isIterable(): bool`
fn is_iterable(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let t = ctx.class_by_name(b"Traversable");
    Ok(Value::Bool(t.is_some_and(|t| ctx.instanceof_class(cid, t))))
}

/// `ReflectionClass::isInstance(object $object): bool`
fn is_instance(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let Some(inst) = obj_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "ReflectionClass::isInstance(): Argument #1 ($object) must be of type object",
        ));
    };
    Ok(Value::Bool(ctx.object_instanceof(&inst, cid)))
}

/// `ReflectionClass::getModifiers(): int`
fn get_modifiers(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Int(class_modifiers(ctx.class(cid))))
}

// ---- the inheritance graph -------------------------------------------------

/// `ReflectionClass::getParentClass(): ReflectionClass|false`
fn get_parent_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let parent = ctx.class(cid).parent;
    match parent {
        Some(p) => make_class(ctx, "ReflectionClass", p),
        None => Ok(Value::Bool(false)),
    }
}

/// `ReflectionClass::getInterfaceNames(): array`
fn get_interface_names(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let ids = ctx.class(cid).interfaces.clone();
    let mut out = Vec::new();
    for i in ids {
        let n = ctx.class(i).name.clone();
        out.push(Value::string(&n));
    }
    Ok(list(out))
}

/// `ReflectionClass::getInterfaces(): array` — keyed by interface name.
fn get_interfaces(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let ids = ctx.class(cid).interfaces.clone();
    let mut a = Array::new();
    for i in ids {
        let name = ctx.class(i).name.clone();
        let r = make_class(ctx, "ReflectionClass", i)?;
        a.set(key(&name), r);
    }
    Ok(Value::Array(a))
}

/// `ReflectionClass::implementsInterface(ReflectionClass|string $interface): bool`
fn implements_interface(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let iid = ref_class_arg(ctx, &args[0], true)?;
    Ok(Value::Bool(ctx.instanceof_class(cid, iid)))
}

/// `ReflectionClass::isSubclassOf(ReflectionClass|string $class): bool` —
/// php answers `false` for the class itself.
fn is_subclass_of(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let target = ref_class_arg(ctx, &args[0], false)?;
    Ok(Value::Bool(
        cid != target && ctx.instanceof_class(cid, target),
    ))
}

/// A `ReflectionClass|string` argument. `want_interface` applies php's two
/// extra checks: the name must resolve to an interface, and php words the
/// missing case as `Interface "X" does not exist`.
fn ref_class_arg(ctx: &mut Ctx, v: &Value, want_interface: bool) -> Result<u32, Unwind> {
    let id = if let Some(r) = obj_arg(v) {
        cid_of(&r)?
    } else {
        let name = str_arg(v);
        match ctx.lookup_class(&name)? {
            Some(id) => id,
            None if want_interface => {
                return Err(refl_error(format!(
                    "Interface \"{}\" does not exist",
                    String::from_utf8_lossy(&name)
                )))
            }
            None => {
                return Err(refl_error(format!(
                    "Class \"{}\" does not exist",
                    String::from_utf8_lossy(&name)
                )))
            }
        }
    };
    if want_interface && ctx.class(id).kind != ClassKind::Interface {
        return Err(refl_error(format!(
            "{} is not an interface",
            ctx.class(id).name_str()
        )));
    }
    Ok(id)
}

/// `ReflectionClass::getTraitNames(): array` — the traits the class itself
/// `use`s, read back from the compiled declaration the runtime kept.
fn get_trait_names(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let ids = trait_ids(ctx, cid_of(this(o)?)?);
    let mut out = Vec::new();
    for t in ids {
        let n = ctx.class(t).name.clone();
        out.push(Value::string(&n));
    }
    Ok(list(out))
}

/// `ReflectionClass::getTraits(): array` — keyed by trait name.
fn get_traits(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let ids = trait_ids(ctx, cid_of(this(o)?)?);
    let mut a = Array::new();
    for tid in ids {
        let display = ctx.class(tid).name.clone();
        let r = make_class(ctx, "ReflectionClass", tid)?;
        a.set(key(&display), r);
    }
    Ok(Value::Array(a))
}

/// The traits a class `use`s itself. The runtime records them on the
/// `ClassDef` (`used_traits`) because linking flattens a trait's members
/// into the class, so the names would otherwise be gone.
fn trait_ids(ctx: &Ctx, cid: u32) -> Vec<u32> {
    ctx.class(cid).used_traits.clone()
}

// ---- instantiation ---------------------------------------------------------

/// `ReflectionClass::newInstanceWithoutConstructor(): object`
fn new_instance_without_ctor(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let obj = ctx.new_object(cid)?;
    Ok(Value::Object(obj))
}

/// `ReflectionClass::newInstance(mixed ...$args): object`
fn new_instance(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    construct(ctx, cid, args, Vec::new())
}

/// `ReflectionClass::newInstanceArgs(array $args = []): object` — string keys
/// are named arguments, as php's `newInstanceArgs` accepts them.
fn new_instance_args(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let mut positional = Vec::new();
    let mut named = Vec::new();
    if let Some(Value::Array(a)) = opt_arg(args, 0) {
        for (k, v) in a.iter() {
            let v = v.deref().into_owned();
            match k {
                ArrayKey::Int(_) => positional.push(v),
                ArrayKey::Str(s) => named.push((Box::from(&s[..]), v)),
            }
        }
    }
    construct(ctx, cid, &positional, named)
}

/// `new C(...)` through Reflection: the visibility of `__construct` is
/// checked against the calling scope (php refuses a private constructor), the
/// instance is built with its defaults and `native_init`, then the
/// constructor runs.
fn construct(
    ctx: &mut Ctx,
    cid: u32,
    args: &[Value],
    named: Vec<(Box<[u8]>, Value)>,
) -> NativeResult {
    let obj = ctx.new_object(cid)?;
    let Some(m) = ctx.resolve_method(cid, b"__construct") else {
        return Ok(Value::Object(obj));
    };
    let scope = ctx.calling_scope();
    if !ctx.access_ok_public(m.vis, m.decl, scope) {
        let vis = match m.vis {
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Public => "public",
        };
        return Err(Unwind::error(format!(
            "Call to {vis} {}::__construct() from {}",
            ctx.class(m.decl).name_str(),
            match scope {
                Some(c) => format!("scope {}", ctx.class(c).name_str()),
                None => "global scope".to_string(),
            }
        )));
    }
    let callable = match &m.body {
        MethodBody::User(f) => Callable::User {
            func: f.clone(),
            this: Some(obj.clone()),
            scope: Some(m.decl),
            static_class: Some(cid),
            closure: None,
        },
        MethodBody::Native(_) => Callable::NativeMethod {
            method: m.clone(),
            this: Some(obj.clone()),
        },
    };
    ctx.call_resolved_named(callable, args, named)?;
    Ok(Value::Object(obj))
}

// ---- methods ---------------------------------------------------------------

/// `ReflectionClass::getConstructor(): ?ReflectionMethod`
fn get_constructor(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    match ctx.resolve_method(cid, b"__construct") {
        Some(m) => func::make_method(ctx, &m),
        None => Ok(Value::Null),
    }
}

/// `ReflectionClass::hasMethod(string $name): bool`
fn has_method(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    Ok(Value::Bool(ctx.resolve_method(cid, &name).is_some()))
}

/// `ReflectionClass::getMethod(string $name): ReflectionMethod`
fn get_method(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    match ctx.resolve_method(cid, &name) {
        Some(m) => func::make_method(ctx, &m),
        None => Err(refl_error(format!(
            "Method {}::{}() does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(&name)
        ))),
    }
}

/// `ReflectionClass::getMethods(?int $filter = null): array`
fn get_methods(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let filter = opt_arg(args, 0).map(|v| v.to_int());
    let ms = ordered_methods(ctx, cid);
    let mut out = Vec::new();
    for m in ms {
        if let Some(f) = filter {
            if method_modifiers(&m) & f == 0 {
                continue;
            }
        }
        out.push(func::make_method(ctx, &m)?);
    }
    Ok(list(out))
}

// ---- properties ------------------------------------------------------------

/// `ReflectionClass::hasProperty(string $name): bool`
fn has_property(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let s: ClassState = state(recv)?;
    let name = str_arg(&args[0]);
    Ok(Value::Bool(find_property(ctx, &s, &name).is_some()))
}

/// `ReflectionClass::getProperty(string $name): ReflectionProperty`
fn get_property(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let s: ClassState = state(recv)?;
    let name = str_arg(&args[0]);
    match find_property(ctx, &s, &name) {
        Some((decl, dynamic)) => prop::make_property(ctx, decl, &name, dynamic),
        None => Err(refl_error(format!(
            "Property {}::${} does not exist",
            ctx.class(s.cid).name_str(),
            String::from_utf8_lossy(&name)
        ))),
    }
}

/// Where a property is declared: `(declaring class, is dynamic)`. A
/// `ReflectionObject` also finds the instance's dynamic properties, which php
/// attributes to the runtime class.
fn find_property(ctx: &Ctx, s: &ClassState, name: &[u8]) -> Option<(u32, bool)> {
    if let Some(decl) = declared_prop(ctx, s.cid, name) {
        return Some((decl, false));
    }
    let inst = s.obj.as_ref()?;
    let present = inst.with_data(|d| d.dyn_props().is_some_and(|p| p.get(name).is_some()));
    present.then_some((s.cid, true))
}

/// `ReflectionClass::getProperties(?int $filter = null): array`
fn get_properties(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let s: ClassState = state(recv)?;
    let filter = opt_arg(args, 0).map(|v| v.to_int());
    let mut out = Vec::new();
    for (name, decl) in ordered_props(ctx, s.cid) {
        if let Some(f) = filter {
            if prop::modifiers(ctx, decl, &name) & f == 0 {
                continue;
            }
        }
        out.push(prop::make_property(ctx, decl, &name, false)?);
    }
    // php appends an instance's dynamic properties, in insertion order.
    if let Some(inst) = &s.obj {
        let dyns: Vec<Box<[u8]>> = inst.with_data(|d| {
            d.dyn_props()
                .map(|p| p.iter().map(|(n, _)| Box::from(n)).collect())
                .unwrap_or_default()
        });
        for n in dyns {
            if let Some(f) = filter {
                if IS_PUBLIC & f == 0 {
                    continue;
                }
            }
            out.push(prop::make_property(ctx, s.cid, &n, true)?);
        }
    }
    Ok(list(out))
}

// ---- constants -------------------------------------------------------------

/// The constants php reports for a class, own first, skipping an ancestor's
/// private ones (php does not inherit those at all).
fn const_names(ctx: &Ctx, cid: u32) -> Vec<Box<[u8]>> {
    let def = ctx.class(cid);
    let mut out = Vec::new();
    // An enum's cases come first and are constants for every Reflection
    // purpose; the runtime keeps them in their own table.
    for c in &def.enum_cases {
        out.push(c.name.clone());
    }
    for name in &def.const_order {
        let Some(k) = def.consts.get(name) else {
            continue;
        };
        if k.vis == Visibility::Private && k.decl != cid {
            continue;
        }
        out.push(name.clone());
    }
    out
}

/// `ReflectionClass::getConstants(?int $filter = null): array`
fn get_constants(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let filter = opt_arg(args, 0).map(|v| v.to_int());
    let mut a = Array::new();
    for name in const_names(ctx, cid) {
        if let Some(f) = filter {
            if prop::const_modifiers(ctx, cid, &name) & f == 0 {
                continue;
            }
        }
        let v = prop::const_value(ctx, cid, &name)?;
        a.set(key(&name), v);
    }
    Ok(Value::Array(a))
}

/// `ReflectionClass::hasConstant(string $name): bool`
fn has_constant(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    Ok(Value::Bool(
        const_names(ctx, cid).contains(&Box::from(&name[..])),
    ))
}

/// `ReflectionClass::getConstant(string $name): mixed` — php deprecated the
/// missing-constant case in 8.3 and still answers `false`.
fn get_constant(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    if !const_names(ctx, cid).contains(&Box::from(&name[..])) {
        ctx.deprecated(
            "ReflectionClass::getConstant() for a non-existent constant is deprecated, use ReflectionClass::hasConstant() to check if the constant exists",
        )?;
        return Ok(Value::Bool(false));
    }
    prop::const_value(ctx, cid, &name)
}

/// `ReflectionClass::getReflectionConstants(?int $filter = null): array`
fn get_reflection_constants(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let filter = opt_arg(args, 0).map(|v| v.to_int());
    let mut out = Vec::new();
    for name in const_names(ctx, cid) {
        if let Some(f) = filter {
            if prop::const_modifiers(ctx, cid, &name) & f == 0 {
                continue;
            }
        }
        out.push(prop::make_class_const(
            ctx,
            cid,
            &name,
            "ReflectionClassConstant",
        )?);
    }
    Ok(list(out))
}

/// `ReflectionClass::getReflectionConstant(string $name): ReflectionClassConstant|false`
fn get_reflection_constant(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    if !const_names(ctx, cid).contains(&Box::from(&name[..])) {
        return Ok(Value::Bool(false));
    }
    prop::make_class_const(ctx, cid, &name, "ReflectionClassConstant")
}

// ---- static properties -----------------------------------------------------

/// `ReflectionClass::getStaticProperties(): ?array`
fn get_static_properties(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid).clone();
    let mut a = Array::new();
    // `static_props` is already flattened parent-first; php reports the
    // reflected class's own properties before its ancestors'.
    for c in class_chain(ctx, cid) {
        for (i, s) in def.static_props.iter().enumerate() {
            if s.decl != c {
                continue;
            }
            let v = static_prop_value(ctx, cid, i)?;
            if v.is_uninit() {
                continue;
            }
            a.set(key(&s.name), v);
        }
    }
    Ok(Value::Array(a))
}

/// `ReflectionClass::getStaticPropertyValue(string $name, mixed $default = ?): mixed`
fn get_static_property_value(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    let slot = ctx.class(cid).static_index.get(name.as_slice()).copied();
    match slot {
        Some(i) => static_prop_value(ctx, cid, i as usize),
        None => match args.get(1) {
            Some(d) => Ok(d.deref().into_owned()),
            None => Err(refl_error(format!(
                "Property {}::${} does not exist",
                ctx.class(cid).name_str(),
                String::from_utf8_lossy(&name)
            ))),
        },
    }
}

/// `ReflectionClass::setStaticPropertyValue(string $name, mixed $value): void`
fn set_static_property_value(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    let slot = ctx.class(cid).static_index.get(name.as_slice()).copied();
    let Some(i) = slot else {
        return Err(refl_error(format!(
            "Property {}::${} does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(&name)
        )));
    };
    let v = args[1].deref().into_owned();
    set_static_prop(ctx, cid, i as usize, v);
    Ok(Value::Null)
}

// ---- source location and metadata ------------------------------------------

/// `ReflectionClass::getFileName(): string|false`
fn get_file_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid);
    match (&def.declared_at, def.internal) {
        (Some((f, _)), false) => Ok(Value::string(f.as_bytes())),
        _ => Ok(Value::Bool(false)),
    }
}

/// `ReflectionClass::getStartLine(): int|false`
fn get_start_line(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let def = ctx.class(cid);
    match (&def.declared_at, def.internal) {
        (Some((_, l)), false) => Ok(Value::Int(i64::from(*l))),
        _ => Ok(Value::Bool(false)),
    }
}

/// `ReflectionClass::getEndLine(): int|false`
///
/// **Engine gap.** The runtime's compiled class declaration records only the
/// first line (`rphp_bytecode::Class::line`); the v2 `ClassDecl::end_line`
/// never reaches it. php's "no line information" answer is `false`, which is
/// what this returns until the declaration carries it.
fn get_end_line(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    cid_of(this(o)?)?;
    Ok(Value::Bool(false))
}

/// `ReflectionClass::getDocComment(): string|false` — the `/** … */` the
/// declaration carries, which is what a container compiler reads its
/// annotations out of.
fn get_doc_comment(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(match &ctx.class(cid).doc {
        Some(d) => Value::string(d),
        None => Value::Bool(false),
    })
}

/// `ReflectionClass::getAttributes(?string $name = null, int $flags = 0): array`
///
/// **Engine gap.** Class attributes never reach the runtime: the compiler
/// drops every `#[...]` group and `rphp_bytecode::Class` has no `attrs`
/// field, so the list is always empty.
fn get_attributes(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let attrs = ctx.class(cid).attrs.clone();
    let infos = super::common::attr_list!(&attrs, super::common::AttrOwner::Class(cid));
    super::attrs::filtered(ctx, infos, args)
}

// ---- ReflectionEnum --------------------------------------------------------

/// `ReflectionEnum::isBacked(): bool`
fn enum_is_backed(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    Ok(Value::Bool(
        ctx.class(cid).enum_backing != rphp_runtime::EnumBacking::None,
    ))
}

/// `ReflectionEnum::getBackingType(): ?ReflectionNamedType`
fn enum_backing_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = match ctx.class(cid).enum_backing {
        rphp_runtime::EnumBacking::Int => "int",
        rphp_runtime::EnumBacking::String => "string",
        rphp_runtime::EnumBacking::None => return Ok(Value::Null),
    };
    types::from_string(ctx, name, Some(cid))
}

/// `ReflectionEnum::getCases(): array`
fn enum_get_cases(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let names: Vec<Box<[u8]>> = ctx
        .class(cid)
        .enum_cases
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let class = enum_case_class(ctx, cid);
    let mut out = Vec::new();
    for n in names {
        out.push(prop::make_class_const(ctx, cid, &n, class)?);
    }
    Ok(list(out))
}

/// `ReflectionEnum::hasCase(string $name): bool`
fn enum_has_case(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    Ok(Value::Bool(
        ctx.class(cid).enum_index.contains_key(name.as_slice()),
    ))
}

/// `ReflectionEnum::getCase(string $name): ReflectionEnumUnitCase`
fn enum_get_case(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = cid_of(this(o)?)?;
    let name = str_arg(&args[0]);
    if !ctx.class(cid).enum_index.contains_key(name.as_slice()) {
        return Err(refl_error(format!(
            "Case {}::{} does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(&name)
        )));
    }
    let class = enum_case_class(ctx, cid);
    prop::make_class_const(ctx, cid, &name, class)
}

/// Which case reflector an enum's cases get: php uses
/// `ReflectionEnumBackedCase` for a backed enum and
/// `ReflectionEnumUnitCase` for a pure one.
fn enum_case_class(ctx: &Ctx, cid: u32) -> &'static str {
    if ctx.class(cid).enum_backing == rphp_runtime::EnumBacking::None {
        "ReflectionEnumUnitCase"
    } else {
        "ReflectionEnumBackedCase"
    }
}

// ---- registration ----------------------------------------------------------

/// Add the methods every `ReflectionClass`-shaped class carries.
fn class_methods(b: rphp_runtime::ClassBuilder<'_>) -> rphp_runtime::ClassBuilder<'_> {
    b.method("getName", nm!(0, Some(0), get_name))
        .method("getShortName", nm!(0, Some(0), get_short_name))
        .method("getNamespaceName", nm!(0, Some(0), get_namespace_name))
        .method("inNamespace", nm!(0, Some(0), in_namespace))
        .method("isInterface", nm!(0, Some(0), is_interface))
        .method("isTrait", nm!(0, Some(0), is_trait))
        .method("isEnum", nm!(0, Some(0), is_enum_m))
        .method("isAbstract", nm!(0, Some(0), is_abstract))
        .method("isFinal", nm!(0, Some(0), is_final))
        .method("isReadOnly", nm!(0, Some(0), is_readonly))
        .method("isAnonymous", nm!(0, Some(0), is_anonymous))
        .method("isInstantiable", nm!(0, Some(0), is_instantiable))
        .method("isCloneable", nm!(0, Some(0), is_cloneable))
        .method("isInternal", nm!(0, Some(0), is_internal))
        .method("getDefaultProperties", nm!(0, Some(0), get_default_properties))
        .method("getTraitAliases", nm!(0, Some(0), get_trait_aliases))
        .method("getExtensionName", nm!(0, Some(0), get_extension_name))
        .method("getExtension", nm!(0, Some(0), get_extension))
        .method(
            "isUninitializedLazyObject",
            nm!(1, Some(1), is_uninitialized_lazy_object),
        )
        .method("isUserDefined", nm!(0, Some(0), is_user_defined))
        .method("isIterable", nm!(0, Some(0), is_iterable))
        .method("isIterateable", nm!(0, Some(0), is_iterable))
        .method("isInstance", nm!(1, Some(1), is_instance))
        .method("getModifiers", nm!(0, Some(0), get_modifiers))
        .method("getParentClass", nm!(0, Some(0), get_parent_class))
        .method("getInterfaceNames", nm!(0, Some(0), get_interface_names))
        .method("getInterfaces", nm!(0, Some(0), get_interfaces))
        .method("implementsInterface", nm!(1, Some(1), implements_interface))
        .method("isSubclassOf", nm!(1, Some(1), is_subclass_of))
        .method("getTraitNames", nm!(0, Some(0), get_trait_names))
        .method("getTraits", nm!(0, Some(0), get_traits))
        .method(
            "newInstanceWithoutConstructor",
            nm!(0, Some(0), new_instance_without_ctor),
        )
        .method("newInstance", nm!(0, None, new_instance))
        .method("newInstanceArgs", nm!(0, Some(1), new_instance_args))
        .method("getConstructor", nm!(0, Some(0), get_constructor))
        .method("hasMethod", nm!(1, Some(1), has_method))
        .method("getMethod", nm!(1, Some(1), get_method))
        .method("getMethods", nm!(0, Some(1), get_methods))
        .method("hasProperty", nm!(1, Some(1), has_property))
        .method("getProperty", nm!(1, Some(1), get_property))
        .method("getProperties", nm!(0, Some(1), get_properties))
        .method("getConstants", nm!(0, Some(1), get_constants))
        .method("hasConstant", nm!(1, Some(1), has_constant))
        .method("getConstant", nm!(1, Some(1), get_constant))
        .method(
            "getReflectionConstants",
            nm!(0, Some(1), get_reflection_constants),
        )
        .method(
            "getReflectionConstant",
            nm!(1, Some(1), get_reflection_constant),
        )
        .method(
            "getStaticProperties",
            nm!(0, Some(0), get_static_properties),
        )
        .method(
            "getStaticPropertyValue",
            nm!(1, Some(2), get_static_property_value),
        )
        .method(
            "setStaticPropertyValue",
            nm!(2, Some(2), set_static_property_value),
        )
        .method("getFileName", nm!(0, Some(0), get_file_name))
        .method("getStartLine", nm!(0, Some(0), get_start_line))
        .method("getEndLine", nm!(0, Some(0), get_end_line))
        .method("getDocComment", nm!(0, Some(0), get_doc_comment))
        .method("getAttributes", nm!(0, Some(2), get_attributes))
}

/// The `ReflectionClass::IS_*` / `SKIP_*` constants php declares.
fn class_constants(b: rphp_runtime::ClassBuilder<'_>) -> rphp_runtime::ClassBuilder<'_> {
    b.class_const("IS_IMPLICIT_ABSTRACT", Value::Int(IS_IMPLICIT_ABSTRACT))
        .class_const("IS_EXPLICIT_ABSTRACT", Value::Int(IS_ABSTRACT))
        .class_const("IS_FINAL", Value::Int(IS_FINAL))
        .class_const("IS_READONLY", Value::Int(IS_CLASS_READONLY))
        .class_const("SKIP_INITIALIZATION_ON_SERIALIZE", Value::Int(8))
        .class_const("SKIP_DESTRUCTOR", Value::Int(16))
}

/// Register `ReflectionClass`, `ReflectionObject` and `ReflectionEnum`.
pub(crate) fn register_classes(r: &mut Registry) {
    let b = r
        .class("ReflectionClass")
        .implements(&["Reflector"])
        .prop("name", Visibility::Public, Value::string(b""))
        .method("__construct", nm!(1, Some(1), class_construct));
    class_constants(class_methods(b)).finish();

    let b = r
        .class("ReflectionObject")
        .extends("ReflectionClass")
        .method("__construct", nm!(1, Some(1), object_construct));
    b.finish();

    let b = r
        .class("ReflectionEnum")
        .extends("ReflectionClass")
        .method("__construct", nm!(1, Some(1), enum_construct))
        .method("isBacked", nm!(0, Some(0), enum_is_backed))
        .method("getBackingType", nm!(0, Some(0), enum_backing_type))
        .method("getCases", nm!(0, Some(0), enum_get_cases))
        .method("hasCase", nm!(1, Some(1), enum_has_case))
        .method("getCase", nm!(1, Some(1), enum_get_case));
    b.finish();
}
