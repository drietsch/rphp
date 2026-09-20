//! `ReflectionProperty`, `ReflectionClassConstant` and the two enum-case
//! reflectors (php-src `ext/reflection/php_reflection.c`).
//!
//! **What an instance holds.** Both families declare the public `$name` and
//! `$class` slots php dumps; the payload keeps the declaring class id and the
//! member name, so the reflector re-resolves the member on every call rather
//! than caching a descriptor that inheritance could invalidate.
//!
//! **Visibility is not enforced.** `getValue`/`setValue` read and write the
//! instance slot (or the static cell) directly, which is what makes
//! Reflection the escape hatch php documents.
//!
//! **Known divergences**, all upstream of this module:
//!
//! * `ReflectionClassConstant::getAttributes()` is still empty: the
//!   compiler lowers a constant's attributes, but the runtime's class
//!   constant carries neither them nor a docblock yet.
//! * php refuses `setValue()` on an initialized `readonly` property with
//!   `Cannot modify readonly property C::$p`; the write path used here goes
//!   straight to the slot and does not raise it.

use rphp_runtime::{nm, Ctx, NativeResult, PropDefault, Registry, Unwind, Visibility};
use rphp_value::{Object, Value};

use super::class;
use super::common::{
    class_arg, declared_prop, list, obj_arg, refl_error, state, static_prop_value, store, str_arg,
    this, vis_bit, ConstState, PropState, IS_FINAL, IS_PRIVATE, IS_PRIVATE_SET, IS_PROTECTED,
    IS_PROTECTED_SET, IS_PUBLIC, IS_READONLY, IS_STATIC,
};
use super::types;

// ---- ReflectionProperty ----------------------------------------------------

/// Where a property is declared and what shape it has. The two kinds are kept
/// apart because the runtime stores instance and static properties in
/// separate tables.
enum Decl {
    /// An instance property: its index in the class's `props`.
    Instance(usize),
    /// A static property: its index in the class's `static_props`.
    Static(usize),
    /// A property the instance grew at run time.
    Dynamic,
}

/// Locate a property on its declaring class.
fn locate(ctx: &Ctx, s: &PropState) -> Decl {
    if s.dynamic {
        return Decl::Dynamic;
    }
    let def = ctx.class(s.cid);
    if let Some(&i) = def.prop_index.get(&s.name) {
        return Decl::Instance(i as usize);
    }
    if let Some(&i) = def.static_index.get(&s.name) {
        return Decl::Static(i as usize);
    }
    Decl::Dynamic
}

/// A `ReflectionProperty` over `name` as declared in `decl`.
pub(crate) fn make_property(
    ctx: &mut Ctx,
    decl: u32,
    name: &[u8],
    dynamic: bool,
) -> Result<Value, Unwind> {
    let st = PropState {
        cid: decl,
        name: Box::from(name),
        dynamic,
    };
    let o = super::common::new_reflector(ctx, "ReflectionProperty", st)?;
    o.set(b"name", Value::string(name));
    let class = ctx.class(decl).name.clone();
    o.set(b"class", Value::string(&class));
    Ok(Value::Object(o))
}

/// `ReflectionProperty::getModifiers()` for a property of `cid`, as php's
/// `getProperties($filter)` needs it before the reflector exists.
pub(crate) fn modifiers(ctx: &Ctx, cid: u32, name: &[u8]) -> i64 {
    let s = PropState {
        cid,
        name: Box::from(name),
        dynamic: false,
    };
    prop_modifiers(ctx, &s)
}

/// The modifier bits of a located property.
fn prop_modifiers(ctx: &Ctx, s: &PropState) -> i64 {
    let def = ctx.class(s.cid);
    match locate(ctx, s) {
        Decl::Instance(i) => {
            let p = &def.props[i];
            let mut bits = vis_bit(p.vis);
            // `readonly` *is* `protected(set)`, and php reports that bit when
            // the read visibility is wider than it — so `public readonly` is
            // 2177 while `protected readonly` is 130.
            if p.readonly {
                bits |= IS_READONLY;
                if p.vis == Visibility::Public {
                    bits |= IS_PROTECTED_SET;
                }
            }
            match p.set_vis {
                Some(Visibility::Protected) => bits |= IS_PROTECTED_SET,
                // A `private(set)` property cannot be redeclared, so php
                // marks it final as well.
                Some(Visibility::Private) => bits |= IS_PRIVATE_SET | IS_FINAL,
                _ => {}
            }
            bits
        }
        Decl::Static(i) => vis_bit(def.static_props[i].vis) | IS_STATIC,
        // A dynamic property is public and nothing else.
        Decl::Dynamic => IS_PUBLIC,
    }
}

/// `ReflectionProperty::__construct(object|string $class, string $property)`
fn prop_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let cid = class_arg(ctx, &args[0])?;
    let name = str_arg(&args[1]);
    // Reflecting an *instance* also finds a property it grew at run time,
    // which is the only way to reach one — a class knows nothing about it.
    let dynamic = match &*args[0].deref() {
        Value::Object(obj) => obj.get_deref(&name).is_some(),
        _ => false,
    };
    let decl = match declared_prop(ctx, cid, &name) {
        Some(decl) => decl,
        None if dynamic => cid,
        None => {
            return Err(refl_error(format!(
                "Property {}::${} does not exist",
                ctx.class(cid).name_str(),
                String::from_utf8_lossy(&name)
            )))
        }
    };
    let dynamic = dynamic && declared_prop(ctx, cid, &name).is_none();
    recv.set(b"name", Value::string(&name));
    let class = ctx.class(decl).name.clone();
    recv.set(b"class", Value::string(&class));
    store(
        recv,
        PropState {
            cid: decl,
            name: Box::from(&name[..]),
            dynamic,
        },
    );
    Ok(Value::Null)
}

/// `ReflectionProperty::getName(): string`
fn prop_get_name(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::string(&s.name))
}

/// `ReflectionProperty::getDeclaringClass(): ReflectionClass`
fn prop_declaring_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    class::make_class(ctx, "ReflectionClass", s.cid)
}

/// `ReflectionProperty::isPrivateSet(): bool` — php 8.4's asymmetric
/// visibility, which `readonly` also implies (as `protected(set)`).
fn prop_is_private_set(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_PRIVATE_SET)
}

/// `ReflectionProperty::isProtectedSet(): bool`
fn prop_is_protected_set(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_PROTECTED_SET)
}

/// `ReflectionProperty::isFinal(): bool` — a `private(set)` property is
/// final as well, since nothing may redeclare it.
fn prop_is_final(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_FINAL)
}

/// `ReflectionProperty::isAbstract(): bool` — only a hooked property can be
/// abstract, and hooks are not lowered, so this is always false.
fn prop_is_abstract(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, super::common::IS_ABSTRACT)
}

/// `ReflectionProperty::isDynamic(): bool` — grown at run time rather than
/// declared, the inverse of `isDefault()`.
fn prop_is_dynamic(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::Bool(s.dynamic))
}

/// `ReflectionProperty::getMangledName(): string` — the key the property
/// really lives under: `"\0Class\0name"` for a private one, `"\0*\0name"`
/// for a protected one, the plain name for everything else.
fn prop_mangled_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let bits = prop_modifiers(ctx, &s);
    let mut out = Vec::new();
    if bits & IS_PRIVATE != 0 {
        out.push(0);
        out.extend_from_slice(&ctx.class(s.cid).name);
        out.push(0);
    } else if bits & IS_PROTECTED != 0 {
        out.extend_from_slice(b"\0*\0");
    }
    out.extend_from_slice(&s.name);
    Ok(Value::string(&out))
}

/// `ReflectionProperty::hasHooks(): bool` / `getHooks(): array` /
/// `hasHook(PropertyHookType $type): bool` / `getHook(…): ?ReflectionMethod`
///
/// Property hooks are not lowered by the compiler (see `props.rs`), so no
/// property this engine can run has one and the four answers are the empty
/// ones php gives for a plain property.
fn prop_has_hooks(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Bool(false))
}

fn prop_get_hooks(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::empty_array())
}

fn prop_has_hook(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Bool(false))
}

fn prop_get_hook(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Null)
}

/// `ReflectionProperty::getSettableType(): ?ReflectionType` — the type a
/// write must satisfy. Without hooks that is the declared type, and a
/// property with no type accepts anything, which php spells `null`.
fn prop_settable_type(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    prop_get_type(ctx, o, args)
}

/// `ReflectionProperty::getRawValue(object $object): mixed` — the value
/// behind the hooks, which is the value itself while there are none.
fn prop_get_raw_value(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    prop_get_value(ctx, o, args)
}

/// `ReflectionProperty::setRawValue(object $object, mixed $value): void`
fn prop_set_raw_value(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    prop_set_value(ctx, o, args)
}

/// `ReflectionProperty::setRawValueWithoutLazyInitialization(object $object, mixed $value): void`
/// — a write that leaves a lazy object lazy: the slot is set and marked
/// initialized ahead of the initializer.
fn prop_set_raw_value_no_lazy(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let obj = instance_arg(ctx, &s, args, "setRawValueWithoutLazyInitialization")?;
    let v = args.get(1).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if obj.is_uninitialized_lazy() {
        skip_lazy(&obj, &s.name, Some(v));
    } else {
        obj.set(&s.name, v);
    }
    Ok(Value::Null)
}

/// Initialize `name` ahead of the initializer, to `value` or its default.
/// Once every declared property is, php treats the object as initialized —
/// an ordinary object again, with no initializer left to run.
fn skip_lazy(obj: &Object, name: &[u8], value: Option<Value>) {
    if obj.lazy_skip(name, value) {
        obj.clear_lazy();
    }
}

/// `ReflectionProperty::isLazy(object $object): bool` — whether reading the
/// property would run the object's initializer.
fn prop_is_lazy(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let obj = instance_arg(ctx, &s, args, "isLazy")?;
    Ok(Value::Bool(obj.lazy_needs_init(Some(&s.name))))
}

/// `ReflectionProperty::skipLazyInitialization(object $object): void` — the
/// property keeps its default and no longer triggers the initializer.
fn prop_skip_lazy_init(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let obj = instance_arg(ctx, &s, args, "skipLazyInitialization")?;
    if obj.is_uninitialized_lazy() {
        skip_lazy(&obj, &s.name, None);
    }
    Ok(Value::Null)
}

/// `ReflectionProperty::getModifiers(): int`
fn prop_get_modifiers(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::Int(prop_modifiers(ctx, &s)))
}

/// One modifier predicate.
fn prop_flag(ctx: &mut Ctx, o: Option<&Object>, bit: i64) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::Bool(prop_modifiers(ctx, &s) & bit != 0))
}

/// `ReflectionProperty::isPublic(): bool`
fn prop_is_public(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_PUBLIC)
}

/// `ReflectionProperty::isProtected(): bool`
fn prop_is_protected(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_PROTECTED)
}

/// `ReflectionProperty::isPrivate(): bool`
fn prop_is_private(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_PRIVATE)
}

/// `ReflectionProperty::isStatic(): bool`
fn prop_is_static(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_STATIC)
}

/// `ReflectionProperty::isReadOnly(): bool`
fn prop_is_readonly(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    prop_flag(ctx, o, IS_READONLY)
}

/// `ReflectionProperty::isDefault(): bool` — declared, as opposed to grown at
/// run time.
fn prop_is_default(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::Bool(!s.dynamic))
}

/// `ReflectionProperty::isPromoted(): bool`
///
/// The runtime's `PropInfo` has no promotion flag — the compiler emits a
/// promoted parameter as an ordinary property — so this asks the declaring
/// class's constructor whether a parameter of the same name promotes.
/// `ReflectionProperty::isVirtual(): bool` (8.4) — true for a hooked property
/// whose hooks never touch the backing store, so php gives it no storage.
///
/// **Divergence:** rphp lays out a slot for every declared property and does
/// not record whether a hook reads or writes it, so this is always `false`.
/// The answer is right for a property without hooks, which is nearly all of
/// them; a genuinely virtual one is reported as backed.
fn prop_is_virtual(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _: PropState = state(this(o)?)?;
    Ok(Value::Bool(false))
}

fn prop_is_promoted(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let Some(m) = ctx.resolve_method(s.cid, b"__construct") else {
        return Ok(Value::Bool(false));
    };
    if m.decl != s.cid {
        return Ok(Value::Bool(false));
    }
    let Some(f) = m.user_func() else {
        return Ok(Value::Bool(false));
    };
    let promoted =
        f.f.params
            .iter()
            .any(|p| p.promoted.is_some() && p.name == s.name);
    Ok(Value::Bool(promoted))
}

/// `ReflectionProperty::hasType(): bool`
fn prop_has_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::Bool(prop_type_string(ctx, &s).is_some()))
}

/// `ReflectionProperty::getType(): ?ReflectionType`
fn prop_get_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let ty = prop_type_string(ctx, &s);
    match ty {
        Some(t) => types::from_string(ctx, &t, Some(s.cid)),
        None => Ok(Value::Null),
    }
}

/// The canonical spelling of a property's declared type.
fn prop_type_string(ctx: &Ctx, s: &PropState) -> Option<String> {
    let def = ctx.class(s.cid);
    match locate(ctx, s) {
        Decl::Instance(i) => def.props[i].ty.as_ref().map(ToString::to_string),
        Decl::Static(i) => def.static_props[i].ty.as_ref().map(ToString::to_string),
        Decl::Dynamic => None,
    }
}

/// `ReflectionProperty::hasDefaultValue(): bool` — a typed property without
/// an initializer starts `Uninit` and has none.
fn prop_has_default(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    Ok(Value::Bool(prop_default(ctx, &s).is_some()))
}

/// The declared default of a property, when it has one.
fn prop_default(ctx: &Ctx, s: &PropState) -> Option<PropDefault> {
    let def = ctx.class(s.cid);
    let d = match locate(ctx, s) {
        Decl::Instance(i) => def.props[i].default.clone(),
        Decl::Static(i) => def.static_props[i].init.clone()?,
        Decl::Dynamic => return None,
    };
    match &d {
        PropDefault::Value(v) if v.is_uninit() => None,
        _ => Some(d),
    }
}

/// `ReflectionProperty::getDefaultValue(): mixed`
fn prop_get_default(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let d = prop_default(ctx, &s);
    match d {
        Some(PropDefault::Value(v)) => Ok(v),
        Some(PropDefault::Thunk(fid)) => super::common::run_thunk(ctx, fid, Some(s.cid)),
        None => {
            ctx.deprecated(
                "ReflectionProperty::getDefaultValue() for a property without a default value is deprecated, use ReflectionProperty::hasDefaultValue() to check if the default value exists",
            )?;
            Ok(Value::Null)
        }
    }
}

/// `ReflectionProperty::getValue(?object $object = null): mixed`
fn prop_get_value(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let where_ = locate(ctx, &s);
    if let Decl::Static(i) = where_ {
        let v = static_prop_value(ctx, s.cid, i)?;
        if v.is_uninit() {
            return Err(uninit_error(ctx, &s));
        }
        return Ok(v);
    }
    let obj = instance_arg(ctx, &s, args, "getValue")?;
    match obj.get_deref(&s.name) {
        Some(v) if !v.is_uninit() => Ok(v),
        Some(_) => Err(uninit_error(ctx, &s)),
        None => Ok(Value::Null),
    }
}

/// `ReflectionProperty::setValue(mixed $objectOrValue, mixed $value = ?): void`
fn prop_set_value(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let where_ = locate(ctx, &s);
    if let Decl::Static(i) = where_ {
        // php accepts both `setValue($value)` and `setValue(null, $value)`
        // for a static property.
        let v = match args.len() {
            0 => Value::Null,
            1 => args[0].deref().into_owned(),
            _ => args[1].deref().into_owned(),
        };
        super::common::set_static_prop(ctx, s.cid, i, v);
        return Ok(Value::Null);
    }
    let obj = instance_arg(ctx, &s, args, "setValue")?;
    let v = match args.get(1) {
        Some(v) => v.deref().into_owned(),
        None => Value::Null,
    };
    obj.set(&s.name, v);
    Ok(Value::Null)
}

/// `ReflectionProperty::isInitialized(?object $object = null): bool`
fn prop_is_initialized(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let where_ = locate(ctx, &s);
    if let Decl::Static(i) = where_ {
        let v = static_prop_value(ctx, s.cid, i)?;
        return Ok(Value::Bool(!v.is_uninit()));
    }
    let obj = instance_arg(ctx, &s, args, "isInitialized")?;
    let set = obj.get(&s.name).is_some_and(|v| !v.is_uninit());
    Ok(Value::Bool(set))
}

/// The `$object` argument of an instance-property operation, with php's two
/// errors.
fn instance_arg(ctx: &Ctx, s: &PropState, args: &[Value], method: &str) -> Result<Object, Unwind> {
    let Some(obj) = args.first().and_then(obj_arg) else {
        return Err(Unwind::type_error(format!(
            "ReflectionProperty::{method}(): Argument #1 ($object) must be provided for instance properties"
        )));
    };
    if !ctx.object_instanceof(&obj, s.cid) {
        return Err(refl_error(
            "Given object is not an instance of the class this property was declared in",
        ));
    }
    Ok(obj)
}

/// php's error for reading a typed property that was never written.
fn uninit_error(ctx: &Ctx, s: &PropState) -> Unwind {
    Unwind::error(format!(
        "Typed property {}::${} must not be accessed before initialization",
        ctx.class(s.cid).name_str(),
        String::from_utf8_lossy(&s.name)
    ))
}

/// `ReflectionProperty::setAccessible(bool $accessible): void` — a no-op
/// since 8.1 and deprecated in 8.5.
fn prop_set_accessible(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    ctx.deprecated(
        "Method ReflectionProperty::setAccessible() is deprecated since 8.5, as it has no effect",
    )?;
    Ok(Value::Null)
}

/// `ReflectionProperty::getDocComment(): string|false` — the `/** … */`
/// before the declaration. A declaration that names several properties
/// gives each of them the same one, as php does.
fn prop_doc_comment(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let doc = match locate(ctx, &s) {
        Decl::Instance(i) => ctx.class(s.cid).props[i].doc.clone(),
        Decl::Static(_) | Decl::Dynamic => None,
    };
    Ok(match doc {
        Some(d) => Value::string(&d),
        None => Value::Bool(false),
    })
}

/// `ReflectionProperty::getAttributes(?string $name = null, int $flags = 0): array`
fn prop_get_attributes(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let attrs = match locate(ctx, &s) {
        Decl::Instance(i) => ctx.class(s.cid).props[i].attrs.clone(),
        Decl::Static(_) | Decl::Dynamic => Vec::new(),
    };
    let infos = super::common::attr_list!(&attrs, super::common::AttrOwner::Class(s.cid));
    super::attrs::filtered(ctx, infos, args)
}

/// The text of one `ReflectionType`, or `None` for an untyped declaration.
fn type_text(ctx: &mut Ctx, t: &Value) -> Result<Option<String>, Unwind> {
    match t {
        Value::Object(o) => {
            let s = ctx.call_method(o, b"__toString", &[])?;
            Ok(Some(String::from_utf8_lossy(&s.to_php_bytes()).into_owned()))
        }
        _ => Ok(None),
    }
}

/// `ReflectionProperty::__toString(): string` —
/// `Property [ <dynamic> final public private(set) readonly static int $p = 1 ]`,
/// with each part only when it applies. A typed property with no default
/// shows none; an untyped one without a default shows `= NULL`, which is
/// what it holds.
fn prop_to_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: PropState = state(this(o)?)?;
    let bits = prop_modifiers(ctx, &s);
    let mut out = String::from("Property [ ");
    if s.dynamic {
        out.push_str("<dynamic> ");
    }
    if bits & IS_FINAL != 0 {
        out.push_str("final ");
    }
    out.push_str(if bits & IS_PRIVATE != 0 {
        "private "
    } else if bits & IS_PROTECTED != 0 {
        "protected "
    } else {
        "public "
    });
    if bits & IS_PRIVATE_SET != 0 {
        out.push_str("private(set) ");
    } else if bits & IS_PROTECTED_SET != 0 && bits & IS_PRIVATE == 0 {
        out.push_str("protected(set) ");
    }
    if bits & IS_READONLY != 0 {
        out.push_str("readonly ");
    }
    if bits & IS_STATIC != 0 {
        out.push_str("static ");
    }
    let ty = prop_get_type(ctx, o, &mut [])?;
    let typed = if let Some(t) = type_text(ctx, &ty)? {
        out.push_str(&t);
        out.push(' ');
        true
    } else {
        false
    };
    out.push('$');
    out.push_str(&String::from_utf8_lossy(&s.name));
    if !s.dynamic {
        match prop_default(ctx, &s) {
            Some(d) => {
                let v = match d {
                    PropDefault::Value(v) => v,
                    PropDefault::Thunk(fid) => super::common::run_thunk(ctx, fid, Some(s.cid))?,
                };
                out.push_str(" = ");
                out.push_str(&super::func::export_default(&v));
            }
            None if !typed && bits & IS_STATIC == 0 => out.push_str(" = NULL"),
            None => {}
        }
    }
    out.push_str(" ]\n");
    Ok(Value::string(out.as_bytes()))
}

/// `ReflectionClassConstant::__toString(): string` —
/// `Constant [ final protected int D ] { 2 }`. The type shown is the
/// declared one, else the value's own; an object value prints as `Object`.
fn const_to_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    let bits = const_modifiers(ctx, s.cid, &s.name);
    let value = const_value(ctx, s.cid, &s.name)?;
    let mut out = String::from("Constant [ ");
    if bits & IS_FINAL != 0 {
        out.push_str("final ");
    }
    out.push_str(if bits & IS_PRIVATE != 0 {
        "private "
    } else if bits & IS_PROTECTED != 0 {
        "protected "
    } else {
        "public "
    });
    let declared = const_get_type(ctx, o, &mut [])?;
    let ty = match type_text(ctx, &declared)? {
        Some(t) => t,
        None => match &value {
            Value::Object(obj) => String::from_utf8_lossy(obj.layout().class_name()).into_owned(),
            other => other.type_name().to_string(),
        },
    };
    out.push_str(&ty);
    out.push(' ');
    out.push_str(&String::from_utf8_lossy(&s.name));
    out.push_str(" ] { ");
    out.push_str(&match &value {
        Value::Object(_) => "Object".to_string(),
        Value::Array(_) => "Array".to_string(),
        other => other.to_php_string(),
    });
    out.push_str(" }\n");
    Ok(Value::string(out.as_bytes()))
}

// ---- ReflectionClassConstant and the enum cases ----------------------------

/// A class-constant reflector of class `class` (php uses
/// `ReflectionClassConstant` for `getReflectionConstants()` even on an enum,
/// and the two `ReflectionEnum*Case` classes for `ReflectionEnum::getCases`).
pub(crate) fn make_class_const(
    ctx: &mut Ctx,
    cid: u32,
    name: &[u8],
    class: &str,
) -> Result<Value, Unwind> {
    let decl = const_decl(ctx, cid, name);
    let case = ctx.class(cid).enum_index.contains_key(name);
    let st = ConstState {
        cid: decl,
        name: Box::from(name),
        case,
    };
    let o = super::common::new_reflector(ctx, class, st)?;
    o.set(b"name", Value::string(name));
    let cname = ctx.class(decl).name.clone();
    o.set(b"class", Value::string(&cname));
    Ok(Value::Object(o))
}

/// The class a constant (or enum case) is declared in.
fn const_decl(ctx: &Ctx, cid: u32, name: &[u8]) -> u32 {
    let def = ctx.class(cid);
    if def.enum_index.contains_key(name) {
        return cid;
    }
    def.consts.get(name).map_or(cid, |k| k.decl)
}

/// The modifier bits php reports for a class constant. An enum case is
/// public and nothing else.
pub(crate) fn const_modifiers(ctx: &Ctx, cid: u32, name: &[u8]) -> i64 {
    let def = ctx.class(cid);
    if def.enum_index.contains_key(name) {
        return IS_PUBLIC;
    }
    match def.consts.get(name) {
        Some(k) => {
            let mut bits = vis_bit(k.vis);
            if k.is_final {
                bits |= IS_FINAL;
            }
            bits
        }
        None => IS_PUBLIC,
    }
}

/// The value of a class constant, or the singleton object of an enum case.
///
/// An enum case is not a class constant in the runtime's model, so the case
/// object is fetched the only way an extension can: through the enum's own
/// `cases()`, which returns the singletons in declaration order.
pub(crate) fn const_value(ctx: &mut Ctx, cid: u32, name: &[u8]) -> Result<Value, Unwind> {
    if ctx.class(cid).enum_index.contains_key(name) {
        let cases = ctx.call_static_method(cid, b"cases", &[])?;
        if let Value::Array(a) = cases {
            for v in a.values() {
                if let Value::Object(o) = &*v.deref() {
                    if o.get_deref(b"name")
                        .is_some_and(|n| n.to_php_bytes() == name)
                    {
                        return Ok(Value::Object(o.clone()));
                    }
                }
            }
        }
        return Err(refl_error(format!(
            "Case {}::{} does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(name)
        )));
    }
    let decl = const_decl(ctx, cid, name);
    ctx.class_const(decl, name, Some(decl))
}

/// `ReflectionClassConstant::__construct(object|string $class, string $constant)`
fn const_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let cid = class_arg(ctx, &args[0])?;
    let name = str_arg(&args[1]);
    let def = ctx.class(cid).clone();
    let known = def.enum_index.contains_key(name.as_slice())
        || def
            .consts
            .get(name.as_slice())
            .is_some_and(|k| !(k.vis == Visibility::Private && k.decl != cid));
    if !known {
        return Err(refl_error(format!(
            "Constant {}::{} does not exist",
            def.name_str(),
            String::from_utf8_lossy(&name)
        )));
    }
    let decl = const_decl(ctx, cid, &name);
    recv.set(b"name", Value::string(&name));
    let cname = ctx.class(decl).name.clone();
    recv.set(b"class", Value::string(&cname));
    store(
        recv,
        ConstState {
            cid: decl,
            name: Box::from(&name[..]),
            case: def.enum_index.contains_key(name.as_slice()),
        },
    );
    Ok(Value::Null)
}

/// `ReflectionClassConstant::getName(): string`
fn const_get_name(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    Ok(Value::string(&s.name))
}

/// `ReflectionClassConstant::getValue(): mixed`
fn const_get_value(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    const_value(ctx, s.cid, &s.name)
}

/// `ReflectionClassConstant::getDeclaringClass(): ReflectionClass`
fn const_declaring_class(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    class::make_class(ctx, "ReflectionClass", s.cid)
}

/// `ReflectionClassConstant::getModifiers(): int`
fn const_get_modifiers(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    Ok(Value::Int(const_modifiers(ctx, s.cid, &s.name)))
}

/// One modifier predicate.
fn const_flag(ctx: &mut Ctx, o: Option<&Object>, bit: i64) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    Ok(Value::Bool(const_modifiers(ctx, s.cid, &s.name) & bit != 0))
}

/// `ReflectionClassConstant::isPublic(): bool`
fn const_is_public(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    const_flag(ctx, o, IS_PUBLIC)
}

/// `ReflectionClassConstant::isProtected(): bool`
fn const_is_protected(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    const_flag(ctx, o, IS_PROTECTED)
}

/// `ReflectionClassConstant::isPrivate(): bool`
fn const_is_private(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    const_flag(ctx, o, IS_PRIVATE)
}

/// `ReflectionClassConstant::isFinal(): bool`
fn const_is_final(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    const_flag(ctx, o, IS_FINAL)
}

/// `ReflectionClassConstant::isEnumCase(): bool`
fn const_is_enum_case(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    Ok(Value::Bool(s.case))
}

/// `ReflectionClassConstant::hasType(): bool`
fn const_has_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    Ok(Value::Bool(const_type_string(ctx, &s).is_some()))
}

/// `ReflectionClassConstant::getType(): ?ReflectionType`
fn const_get_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    let ty = const_type_string(ctx, &s);
    match ty {
        Some(t) => types::from_string(ctx, &t, Some(s.cid)),
        None => Ok(Value::Null),
    }
}

/// The canonical spelling of a typed class constant's type (php 8.3).
fn const_type_string(ctx: &Ctx, s: &ConstState) -> Option<String> {
    ctx.class(s.cid)
        .consts
        .get(&s.name)?
        .ty
        .as_ref()
        .map(ToString::to_string)
}

/// `ReflectionClassConstant::getDocComment(): string|false`
fn const_doc_comment(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _: ConstState = state(this(o)?)?;
    Ok(Value::Bool(false))
}

/// `ReflectionClassConstant::getAttributes(?string $name = null, int $flags = 0): array`
fn const_get_attributes(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _: ConstState = state(this(o)?)?;
    Ok(list(Vec::new()))
}

/// `ReflectionEnumUnitCase::getEnum(): ReflectionEnum`
fn case_get_enum(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    class::make_class(ctx, "ReflectionEnum", s.cid)
}

/// `ReflectionEnumBackedCase::getBackingValue(): string|int`
fn case_backing_value(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: ConstState = state(this(o)?)?;
    let case = const_value(ctx, s.cid, &s.name)?;
    match &case {
        Value::Object(obj) => Ok(obj.get_deref(b"value").unwrap_or(Value::Null)),
        _ => Ok(Value::Null),
    }
}

/// `ReflectionEnumUnitCase::__construct(object|string $class, string $constant)`
fn case_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let recv = this(o)?;
    let cid = class_arg(ctx, &args[0])?;
    let name = str_arg(&args[1]);
    if !ctx.class(cid).enum_index.contains_key(name.as_slice()) {
        return Err(refl_error(format!(
            "Case {}::{} does not exist",
            ctx.class(cid).name_str(),
            String::from_utf8_lossy(&name)
        )));
    }
    recv.set(b"name", Value::string(&name));
    let cname = ctx.class(cid).name.clone();
    recv.set(b"class", Value::string(&cname));
    store(
        recv,
        ConstState {
            cid,
            name: Box::from(&name[..]),
            case: true,
        },
    );
    Ok(Value::Null)
}

// ---- registration ----------------------------------------------------------

/// Register the property and class-constant reflectors.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ReflectionProperty")
        .implements(&["Reflector"])
        .prop("name", Visibility::Public, Value::string(b""))
        .prop("class", Visibility::Public, Value::string(b""))
        .class_const("IS_STATIC", Value::Int(IS_STATIC))
        .class_const("IS_READONLY", Value::Int(IS_READONLY))
        .class_const("IS_PUBLIC", Value::Int(IS_PUBLIC))
        .class_const("IS_PROTECTED", Value::Int(IS_PROTECTED))
        .class_const("IS_PRIVATE", Value::Int(IS_PRIVATE))
        .class_const("IS_ABSTRACT", Value::Int(super::common::IS_ABSTRACT))
        .class_const("IS_PROTECTED_SET", Value::Int(IS_PROTECTED_SET))
        .class_const("IS_PRIVATE_SET", Value::Int(IS_PRIVATE_SET))
        .class_const("IS_VIRTUAL", Value::Int(super::common::IS_VIRTUAL))
        .class_const("IS_FINAL", Value::Int(IS_FINAL))
        .method("__construct", nm!(2, Some(2), prop_construct))
        .method("getName", nm!(0, Some(0), prop_get_name))
        .method("getValue", nm!(0, Some(1), prop_get_value))
        .method("setValue", nm!(1, Some(2), prop_set_value))
        .method("isInitialized", nm!(0, Some(1), prop_is_initialized))
        .method("isPublic", nm!(0, Some(0), prop_is_public))
        .method("isProtected", nm!(0, Some(0), prop_is_protected))
        .method("isPrivate", nm!(0, Some(0), prop_is_private))
        .method("isStatic", nm!(0, Some(0), prop_is_static))
        .method("isReadOnly", nm!(0, Some(0), prop_is_readonly))
        .method("isDefault", nm!(0, Some(0), prop_is_default))
        .method("isPromoted", nm!(0, Some(0), prop_is_promoted))
        .method("isVirtual", nm!(0, Some(0), prop_is_virtual))
        .method("isPrivateSet", nm!(0, Some(0), prop_is_private_set))
        .method("isProtectedSet", nm!(0, Some(0), prop_is_protected_set))
        .method("isFinal", nm!(0, Some(0), prop_is_final))
        .method("isAbstract", nm!(0, Some(0), prop_is_abstract))
        .method("isDynamic", nm!(0, Some(0), prop_is_dynamic))
        .method("getMangledName", nm!(0, Some(0), prop_mangled_name))
        .method("hasHooks", nm!(0, Some(0), prop_has_hooks))
        .method("getHooks", nm!(0, Some(0), prop_get_hooks))
        .method("hasHook", nm!(1, Some(1), prop_has_hook))
        .method("getHook", nm!(1, Some(1), prop_get_hook))
        .method("getSettableType", nm!(0, Some(0), prop_settable_type))
        .method("getRawValue", nm!(1, Some(1), prop_get_raw_value))
        .method("setRawValue", nm!(2, Some(2), prop_set_raw_value))
        .method(
            "setRawValueWithoutLazyInitialization",
            nm!(2, Some(2), prop_set_raw_value_no_lazy),
        )
        .method("isLazy", nm!(1, Some(1), prop_is_lazy))
        .method("skipLazyInitialization", nm!(1, Some(1), prop_skip_lazy_init))
        .method("hasType", nm!(0, Some(0), prop_has_type))
        .method("getType", nm!(0, Some(0), prop_get_type))
        .method("hasDefaultValue", nm!(0, Some(0), prop_has_default))
        .method("getDefaultValue", nm!(0, Some(0), prop_get_default))
        .method("getDeclaringClass", nm!(0, Some(0), prop_declaring_class))
        .method("getModifiers", nm!(0, Some(0), prop_get_modifiers))
        .method("setAccessible", nm!(1, Some(1), prop_set_accessible))
        .method("getDocComment", nm!(0, Some(0), prop_doc_comment))
        .method("getAttributes", nm!(0, Some(2), prop_get_attributes))
        .method("__toString", nm!(0, Some(0), prop_to_string))
        .finish();

    r.class("ReflectionClassConstant")
        .implements(&["Reflector"])
        .prop("name", Visibility::Public, Value::string(b""))
        .prop("class", Visibility::Public, Value::string(b""))
        .class_const("IS_PUBLIC", Value::Int(IS_PUBLIC))
        .class_const("IS_PROTECTED", Value::Int(IS_PROTECTED))
        .class_const("IS_PRIVATE", Value::Int(IS_PRIVATE))
        .class_const("IS_FINAL", Value::Int(IS_FINAL))
        .method("__construct", nm!(2, Some(2), const_construct))
        .method("getName", nm!(0, Some(0), const_get_name))
        .method("getValue", nm!(0, Some(0), const_get_value))
        .method("getDeclaringClass", nm!(0, Some(0), const_declaring_class))
        .method("getModifiers", nm!(0, Some(0), const_get_modifiers))
        .method("isPublic", nm!(0, Some(0), const_is_public))
        .method("isProtected", nm!(0, Some(0), const_is_protected))
        .method("isPrivate", nm!(0, Some(0), const_is_private))
        .method("isFinal", nm!(0, Some(0), const_is_final))
        .method("isEnumCase", nm!(0, Some(0), const_is_enum_case))
        .method("hasType", nm!(0, Some(0), const_has_type))
        .method("getType", nm!(0, Some(0), const_get_type))
        .method("getDocComment", nm!(0, Some(0), const_doc_comment))
        .method("__toString", nm!(0, Some(0), const_to_string))
        .method("getAttributes", nm!(0, Some(2), const_get_attributes))
        .finish();

    r.class("ReflectionEnumUnitCase")
        .extends("ReflectionClassConstant")
        .method("__construct", nm!(2, Some(2), case_construct))
        .method("getEnum", nm!(0, Some(0), case_get_enum))
        .finish();

    r.class("ReflectionEnumBackedCase")
        .extends("ReflectionEnumUnitCase")
        .method("getBackingValue", nm!(0, Some(0), case_backing_value))
        .finish();
}
