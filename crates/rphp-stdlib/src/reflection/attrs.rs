//! `Attribute` and `ReflectionAttribute` (php-src `Zend/zend_attributes.c`
//! and `ext/reflection/php_reflection.c`).
//!
//! **What an instance holds.** `ReflectionAttribute` declares the public
//! `$name` slot php dumps since 8.5; the compiled attribute — its class name,
//! its argument initializers, the `Attribute::TARGET_*` bit of the place it
//! is attached to and whether the same class occurs there more than once —
//! lives in the payload as an [`AttrInfo`]. Its constructor is `private`, as
//! php's is: an attribute reflector only ever comes from a `getAttributes()`
//! call.
//!
//! **Where the data comes from.** The compiler lowers every `#[...]` group
//! into an `AttrDef` — the resolved class name, the arguments as
//! unevaluated initializers, the target bit — on the function, parameter,
//! class, property, class constant or enum case it decorates. A function's
//! arguments resolve through that function's constant pool; a class-level
//! attribute has no pool, so every one of its arguments is a thunk in the
//! class's unit, run in the class's scope when `getArguments()` or
//! `newInstance()` asks. php evaluates them just as lazily.

use rphp_runtime::{
    nm, Callable, Ctx, MethodBody, NativeResult, Registry, Unwind, Visibility,
};
use rphp_value::{Array, ArrayKey, Object, Value};

use super::common::{list, new_reflector, opt_arg, state, str_arg, this, AttrInfo, InitKey};
use super::func;

/// `Attribute::TARGET_CLASS`
const TARGET_CLASS: i64 = 1;
/// `Attribute::TARGET_FUNCTION`
const TARGET_FUNCTION: i64 = 2;
/// `Attribute::TARGET_METHOD`
const TARGET_METHOD: i64 = 4;
/// `Attribute::TARGET_PROPERTY`
const TARGET_PROPERTY: i64 = 8;
/// `Attribute::TARGET_CLASS_CONSTANT`
const TARGET_CLASS_CONSTANT: i64 = 16;
/// `Attribute::TARGET_PARAMETER`
const TARGET_PARAMETER: i64 = 32;
/// `Attribute::TARGET_CONSTANT` (php 8.5).
const TARGET_CONSTANT: i64 = 64;
/// `Attribute::TARGET_ALL`
const TARGET_ALL: i64 = 127;
/// `Attribute::IS_REPEATABLE`
const IS_REPEATABLE: i64 = 128;
/// `ReflectionAttribute::IS_INSTANCEOF`
const IS_INSTANCEOF: i64 = 2;

/// php's word for each target bit, in the order `zend_attributes.c` lists
/// them in `Attribute "X" cannot target … (allowed targets: …)`.
const TARGET_WORDS: &[(i64, &str)] = &[
    (TARGET_CLASS, "class"),
    (TARGET_FUNCTION, "function"),
    (TARGET_METHOD, "method"),
    (TARGET_PROPERTY, "property"),
    (TARGET_CLASS_CONSTANT, "class constant"),
    (TARGET_PARAMETER, "parameter"),
    (TARGET_CONSTANT, "constant"),
];

/// The word php uses for one target bit.
fn target_word(bit: i64) -> &'static str {
    TARGET_WORDS
        .iter()
        .find(|(b, _)| *b == bit)
        .map_or("unknown", |(_, w)| *w)
}

/// The `allowed targets: …` list of a flag word.
fn allowed_targets(flags: i64) -> String {
    TARGET_WORDS
        .iter()
        .filter(|(b, _)| flags & b != 0)
        .map(|(_, w)| *w)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build the `ReflectionAttribute` objects for `infos`, applying php's
/// `$name` / `$flags` filter.
pub(crate) fn filtered(ctx: &mut Ctx, infos: Vec<AttrInfo>, args: &mut [Value]) -> NativeResult {
    let want = opt_arg(args, 0).map(|v| str_arg(&v));
    let flags = opt_arg(args, 1).map_or(0, |v| v.to_int());
    let mut out = Vec::new();
    for info in infos {
        if let Some(w) = &want {
            if !matches_name(ctx, &info, w, flags & IS_INSTANCEOF != 0)? {
                continue;
            }
        }
        let name = info.name.clone();
        let o = new_reflector(ctx, "ReflectionAttribute", info)?;
        o.set(b"name", Value::string(&name));
        out.push(Value::Object(o));
    }
    Ok(list(out))
}

/// Whether an attribute passes the name filter: an exact (case-insensitive)
/// match, or — with `ReflectionAttribute::IS_INSTANCEOF` — a subclass test.
fn matches_name(
    ctx: &mut Ctx,
    info: &AttrInfo,
    want: &[u8],
    instanceof: bool,
) -> Result<bool, Unwind> {
    if !instanceof {
        return Ok(info.name.eq_ignore_ascii_case(want));
    }
    let (Some(a), Some(b)) = (ctx.lookup_class(&info.name)?, ctx.lookup_class(want)?) else {
        return Ok(false);
    };
    Ok(ctx.instanceof_class(a, b))
}

/// `ReflectionAttribute::getName(): string`
fn get_name(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let a: AttrInfo = state(this(o)?)?;
    Ok(Value::string(&a.name))
}

/// `ReflectionAttribute::getTarget(): int`
fn get_target(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let a: AttrInfo = state(this(o)?)?;
    Ok(Value::Int(a.target))
}

/// `ReflectionAttribute::isRepeated(): bool`
fn is_repeated(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let a: AttrInfo = state(this(o)?)?;
    Ok(Value::Bool(a.repeated))
}

/// `ReflectionAttribute::getArguments(): array` — positional arguments keep
/// their position, named ones become string keys.
fn get_arguments(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let a: AttrInfo = state(this(o)?)?;
    let (pos, named) = arguments(ctx, &a)?;
    let mut out = Array::new();
    for v in pos {
        out.push(v);
    }
    for (k, v) in named {
        out.set(ArrayKey::Str(k), v);
    }
    Ok(Value::Array(out))
}

/// `ReflectionAttribute::__toString(): string`:
///
/// ```text
/// Attribute [ Tag ] {
///   - Arguments [2] {
///     Argument #0 [ 'm' ]
///     Argument #1 [ list = [0 => 1, 'k' => 'v'] ]
///   }
/// }
/// ```
///
/// and just `Attribute [ Tag ]` when there are no arguments.
fn attr_to_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let a: AttrInfo = state(this(o)?)?;
    let mut out = format!("Attribute [ {} ]", String::from_utf8_lossy(&a.name));
    if a.args.is_empty() {
        out.push('\n');
        return Ok(Value::string(out.as_bytes()));
    }
    let (pos, named) = arguments(ctx, &a)?;
    out.push_str(&format!(" {{\n  - Arguments [{}] {{\n", pos.len() + named.len()));
    let mut n = 0;
    for v in &pos {
        out.push_str(&format!("    Argument #{n} [ {} ]\n", func::export_default(v)));
        n += 1;
    }
    for (k, v) in &named {
        out.push_str(&format!(
            "    Argument #{n} [ {} = {} ]\n",
            String::from_utf8_lossy(k),
            func::export_default(v)
        ));
        n += 1;
    }
    out.push_str("  }\n}\n");
    Ok(Value::string(out.as_bytes()))
}

/// Evaluate an attribute's argument initializers against its owner's
/// constant pool.
fn arguments(ctx: &mut Ctx, a: &AttrInfo) -> Result<(Vec<Value>, Vec<(Box<[u8]>, Value)>), Unwind> {
    let mut pos = Vec::new();
    let mut named = Vec::new();
    // A class-level attribute: every argument is a thunk in the class's
    // unit, run in the class's own scope (`self::CONST` works).
    if let super::common::AttrOwner::Class(cid) = a.owner {
        let base = ctx.class(cid).unit.as_ref().map(|u| u.func_base);
        for (name, init) in &a.args {
            let v = match (init, base) {
                (InitKey::Thunk(local), Some(base)) => {
                    super::common::run_thunk(ctx, base + local, Some(cid))?
                }
                _ => Value::Null,
            };
            match name {
                Some(n) => named.push((n.clone(), v)),
                None => pos.push(v),
            }
        }
        return Ok((pos, named));
    }
    let super::common::AttrOwner::Fn(target) = &a.owner else {
        unreachable!("the class owner is handled above");
    };
    let i = func::info(ctx, target)?;
    for (name, init) in &a.args {
        let v = match init {
            InitKey::Const(idx) => i
                .func
                .as_ref()
                .and_then(|f| f.f.consts.get(*idx as usize))
                .and_then(|c| c.try_to_value())
                .unwrap_or(Value::Null),
            InitKey::Thunk(local) => {
                let Some(f) = i.func.clone() else {
                    continue;
                };
                super::common::run_thunk(ctx, f.unit.func_base + local, i.class)?
            }
        };
        match name {
            Some(n) => named.push((n.clone(), v)),
            None => pos.push(v),
        }
    }
    Ok((pos, named))
}

/// The `flags` of an attribute class's own `#[Attribute(...)]` declaration:
/// `None` when the class does not carry one at all, which is what makes
/// `newInstance()` raise php's `Attempting to use non-attribute class`.
///
/// php's default is `TARGET_ALL`, and the flags argument — when there is one
/// — is a constant expression the compiler left as a thunk.
fn attribute_flags(ctx: &mut Ctx, cid: u32) -> Option<i64> {
    let attrs = ctx.class(cid).attrs.clone();
    let infos = super::common::attr_list!(&attrs, super::common::AttrOwner::Class(cid));
    let found = infos
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(b"Attribute"))?;
    if found.args.is_empty() {
        return Some(TARGET_ALL);
    }
    match arguments(ctx, &found) {
        Ok((pos, named)) => Some(
            pos.first()
                .or_else(|| named.iter().find(|(n, _)| n.as_ref() == b"flags").map(|(_, v)| v))
                .map_or(TARGET_ALL, |v| v.to_int()),
        ),
        Err(_) => Some(TARGET_ALL),
    }
}

/// `ReflectionAttribute::newInstance(): object`
///
/// php validates the attribute class *here*, not at declaration: the class
/// must exist, must carry `#[Attribute]`, must allow this target, and must
/// be repeatable if it occurs more than once — then the arguments are passed
/// to its constructor as ordinary positional and named arguments.
fn new_instance(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let a: AttrInfo = state(this(o)?)?;
    let display = String::from_utf8_lossy(&a.name).into_owned();
    let Some(cid) = ctx.lookup_class(&a.name)? else {
        return Err(Unwind::error(format!(
            "Attribute class \"{display}\" not found"
        )));
    };
    let Some(flags) = attribute_flags(ctx, cid) else {
        return Err(Unwind::error(format!(
            "Attempting to use non-attribute class \"{display}\" as attribute"
        )));
    };
    if flags & a.target == 0 {
        return Err(Unwind::error(format!(
            "Attribute \"{display}\" cannot target {} (allowed targets: {})",
            target_word(a.target),
            allowed_targets(flags)
        )));
    }
    if a.repeated && flags & IS_REPEATABLE == 0 {
        return Err(Unwind::error(format!(
            "Attribute \"{display}\" must not be repeated"
        )));
    }
    let (pos, named) = arguments(ctx, &a)?;
    let obj = ctx.new_object(cid)?;
    let Some(m) = ctx.resolve_method(cid, b"__construct") else {
        return Ok(Value::Object(obj));
    };
    let scope = ctx.calling_scope();
    if !ctx.access_ok_public(m.vis, m.decl, scope) {
        let word = match m.vis {
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Public => "public",
        };
        return Err(Unwind::error(format!(
            "Call to {word} {}::__construct() from {}",
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
    ctx.call_resolved_named(callable, &pos, named)?;
    Ok(Value::Object(obj))
}

/// Register `ReflectionAttribute` (`Attribute` itself is php-written, in
/// the embed prelude, so it can carry its own `#[Attribute]`).
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ReflectionAttribute")
        .implements(&["Reflector"])
        .prop("name", Visibility::Public, Value::string(b""))
        .class_const("IS_INSTANCEOF", Value::Int(IS_INSTANCEOF))
        .method_vis(
            "__construct",
            Visibility::Private,
            nm!(0, Some(0), private_construct),
        )
        .method("getName", nm!(0, Some(0), get_name))
        .method("getTarget", nm!(0, Some(0), get_target))
        .method("isRepeated", nm!(0, Some(0), is_repeated))
        .method("getArguments", nm!(0, Some(0), get_arguments))
        .method("__toString", nm!(0, Some(0), attr_to_string))
        .method("newInstance", nm!(0, Some(0), new_instance))
        .finish();
}

/// `ReflectionAttribute::__construct()` is private in php and never runs.
fn private_construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}
