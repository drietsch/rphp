//! The plumbing every `Reflection*` class shares: the hidden state each
//! instance carries, the helpers that build one, php's modifier bits, and
//! the two workarounds this module needs because `rphp-stdlib` may not
//! depend on `rphp-bytecode` (see the module header of `reflection.rs`).

use rphp_runtime::{
    ClassDef, ClassFlags, ClassKind, Ctx, MethodDef, NativeId, NativeMethod, NativeResult,
    PropDefault, Unwind, Visibility,
};
use rphp_value::{Array, ArrayKey, Closure, Object, Payload, Value};
use std::rc::Rc;

// ---- php's modifier bits ---------------------------------------------------
//
// `ReflectionMethod::IS_*`, `ReflectionProperty::IS_*` and
// `ReflectionClass::IS_*` are the raw `ZEND_ACC_*` bits, so one set of
// constants serves every class that exposes them.

/// `IS_PUBLIC`
pub(crate) const IS_PUBLIC: i64 = 1;
/// `IS_PROTECTED`
pub(crate) const IS_PROTECTED: i64 = 2;
/// `IS_PRIVATE`
pub(crate) const IS_PRIVATE: i64 = 4;
/// `IS_STATIC`
pub(crate) const IS_STATIC: i64 = 16;
/// `IS_FINAL`
pub(crate) const IS_FINAL: i64 = 32;
/// `IS_ABSTRACT` (a method) / `IS_EXPLICIT_ABSTRACT` (a class).
pub(crate) const IS_ABSTRACT: i64 = 64;
/// `ReflectionProperty::IS_READONLY`
pub(crate) const IS_READONLY: i64 = 128;
/// `ReflectionProperty::IS_VIRTUAL`
pub(crate) const IS_VIRTUAL: i64 = 512;
/// `ReflectionProperty::IS_PROTECTED_SET`
pub(crate) const IS_PROTECTED_SET: i64 = 2048;
/// `ReflectionProperty::IS_PRIVATE_SET`
pub(crate) const IS_PRIVATE_SET: i64 = 4096;
/// `ReflectionClass::IS_IMPLICIT_ABSTRACT`
pub(crate) const IS_IMPLICIT_ABSTRACT: i64 = 16;
/// `ReflectionClass::IS_READONLY`
pub(crate) const IS_CLASS_READONLY: i64 = 65536;

/// The visibility bit php reports for a member.
pub(crate) fn vis_bit(v: Visibility) -> i64 {
    match v {
        Visibility::Public => IS_PUBLIC,
        Visibility::Protected => IS_PROTECTED,
        Visibility::Private => IS_PRIVATE,
    }
}

// ---- the state a reflector carries ----------------------------------------

/// What a `ReflectionClass` / `ReflectionObject` / `ReflectionEnum` holds:
/// the process-wide class id, plus the instance a `ReflectionObject` was
/// built over (php lists that instance's dynamic properties too).
#[derive(Clone)]
pub(crate) struct ClassState {
    /// The reflected class.
    pub cid: u32,
    /// The instance, for `ReflectionObject`.
    pub obj: Option<Object>,
}

/// Which function a `ReflectionFunction` / `ReflectionMethod` names. Kept as
/// a *reference*, not a resolved handle, so a reflector stays valid when the
/// class table grows under it.
#[derive(Clone)]
pub(crate) enum FnTarget {
    /// A compiled function, by process-wide id.
    User(u32),
    /// A registered native.
    Native(NativeId),
    /// A method: the class it was looked up on plus the declared name.
    Method { cid: u32, name: Box<[u8]> },
    /// A property hook (`$name::get`), compiled as a function of `decl`.
    Hook { fid: u32, decl: u32 },
    /// A closure value.
    Closure(Closure),
}

/// The state of a `ReflectionFunction` / `ReflectionMethod`.
#[derive(Clone)]
pub(crate) struct FnState {
    /// The function or method.
    pub target: FnTarget,
    /// The receiver a `ReflectionMethod` over a closure keeps (unused for the
    /// other targets; `getClosureThis()` reads it once the closure target
    /// carries one).
    #[allow(dead_code)]
    pub this: Option<Object>,
}

/// The state of a `ReflectionParameter`: the function it belongs to and the
/// declaration position.
#[derive(Clone)]
pub(crate) struct ParamState {
    /// The owning function.
    pub owner: FnTarget,
    /// Zero-based position in the declaration.
    pub index: usize,
}

/// The state of a `ReflectionProperty`.
#[derive(Clone)]
pub(crate) struct PropState {
    /// The class the property was looked up on.
    pub cid: u32,
    /// The property name without the `$`.
    pub name: Box<[u8]>,
    /// A dynamic property has no declaration; `getValue` reads it off the
    /// instance and every modifier answers "public".
    pub dynamic: bool,
}

/// The state of a `ReflectionClassConstant` / `ReflectionEnum*Case`.
#[derive(Clone)]
pub(crate) struct ConstState {
    /// The class the constant was looked up on.
    pub cid: u32,
    /// The constant (or case) name.
    pub name: Box<[u8]>,
    /// Whether this names an enum case rather than an ordinary constant.
    pub case: bool,
}

/// A parsed `#[Name(args)]` as `ReflectionAttribute` keeps it. The compiled
/// [`AttrDef`](rphp_bytecode::AttrDef) cannot be stored directly: naming that
/// type would need a dependency this crate does not have.
#[derive(Clone)]
pub(crate) struct AttrInfo {
    /// The attribute class name as the compiler resolved it.
    pub name: Box<[u8]>,
    /// The arguments in order, each with its name when it was written as a
    /// named argument.
    pub args: Vec<(Option<Box<[u8]>>, InitKey)>,
    /// The `Attribute::TARGET_*` bit of the place it is attached to.
    pub target: i64,
    /// Whether the same attribute class occurs more than once on that place.
    pub repeated: bool,
    /// What the argument initializers resolve against.
    pub owner: AttrOwner,
}

/// Where an attribute's argument initializers live.
///
/// A function's (or parameter's) attribute resolves through that function's
/// constant pool and unit. A class-level attribute has no function to hold a
/// pool, so the compiler makes every one of its arguments a **thunk** in the
/// class's unit, and the class is what it resolves against.
#[derive(Clone)]
pub(crate) enum AttrOwner {
    Fn(FnTarget),
    Class(u32),
}

/// An initializer the compiler left behind: either an index into the owner's
/// constant pool or a unit-local thunk function. A copy of
/// [`InitRef`](rphp_bytecode::InitRef) (see [`parse_init_ref`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InitKey {
    /// `InitRef::Const(i)` — index into the owning function's pool.
    Const(u32),
    /// `InitRef::Thunk(f)` — a unit-local zero-argument function id.
    Thunk(u32),
}

/// Read an `InitRef` without naming its type.
///
/// `rphp-stdlib` depends on `rphp-runtime` and `rphp-value` only, so
/// `rphp_bytecode::InitRef` cannot be matched on here. Its derived `Debug` is
/// `Const(3)` / `Thunk(7)`, which carries both the discriminant and the
/// payload, so this parses that. **Replace with a direct `match` as soon as
/// the crate may depend on `rphp-bytecode`** (or the runtime re-exports the
/// type); nothing else in this module is written this way.
pub(crate) fn parse_init_ref(dbg: &str) -> Option<InitKey> {
    let (tag, rest) = dbg.split_once('(')?;
    let num = rest.strip_suffix(')')?.trim().parse::<u32>().ok()?;
    match tag.trim() {
        "Const" => Some(InitKey::Const(num)),
        "Thunk" => Some(InitKey::Thunk(num)),
        _ => None,
    }
}

// ---- building and reading a reflector -------------------------------------

/// The receiver of an instance method.
pub(crate) fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// The hidden state of a reflector, cloned out so the payload borrow ends
/// before anything re-enters the interpreter.
///
/// php's wording for a reflector whose constructor never ran (a subclass with
/// its own `__construct` that does not call `parent::__construct`).
pub(crate) fn state<T: Clone + 'static>(o: &Object) -> Result<T, Unwind> {
    o.with_payload::<T, _>(|s| s.clone())
        .ok_or_else(|| Unwind::error("Internal error: Failed to retrieve the reflection object"))
}

/// Attach hidden state to a reflector.
pub(crate) fn store<T: 'static>(o: &Object, s: T) {
    o.set_payload(Payload::Native(Box::new(s)));
}

/// A fresh instance of a native reflection class with its hidden state set.
/// The constructor is not run: these classes' constructors only do what the
/// caller has already done.
pub(crate) fn new_reflector<T: 'static>(
    ctx: &mut Ctx,
    class: &str,
    state: T,
) -> Result<Object, Unwind> {
    let cid = ctx
        .class_by_name(class.as_bytes())
        .ok_or_else(|| Unwind::error(format!("Class \"{class}\" not found")))?;
    let o = ctx.instantiate(cid);
    store(&o, state);
    Ok(o)
}

/// `throw new ReflectionException($msg)`.
pub(crate) fn refl_error(msg: impl Into<String>) -> Unwind {
    Unwind::exception("ReflectionException", msg)
}

// ---- argument helpers ------------------------------------------------------

/// An argument as a byte string (php coerces scalars for a `string`
/// parameter, and every reflection entry point here takes one).
pub(crate) fn str_arg(v: &Value) -> Vec<u8> {
    v.deref().to_php_bytes()
}

/// An optional argument: `None` for an absent or null slot.
pub(crate) fn opt_arg(args: &[Value], i: usize) -> Option<Value> {
    match args.get(i) {
        None => None,
        Some(v) => match &*v.deref() {
            Value::Null | Value::Uninit => None,
            other => Some(other.clone()),
        },
    }
}

/// The object an argument holds, if it is one.
pub(crate) fn obj_arg(v: &Value) -> Option<Object> {
    match &*v.deref() {
        Value::Object(o) => Some(o.clone()),
        _ => None,
    }
}

/// php's `object|string $objectOrClass`: an object contributes its class, a
/// string is looked up (with autoload) and a missing one is
/// `Class "X" does not exist`.
pub(crate) fn class_arg(ctx: &mut Ctx, v: &Value) -> Result<u32, Unwind> {
    if let Some(cid) = ctx.class_of_value(&v.deref()) {
        return Ok(cid);
    }
    let name = str_arg(v);
    class_by_name_or_error(ctx, &name)
}

/// Look a class name up, autoloading, with php's `ReflectionException`.
pub(crate) fn class_by_name_or_error(ctx: &mut Ctx, name: &[u8]) -> Result<u32, Unwind> {
    match ctx.lookup_class(name)? {
        Some(cid) => Ok(cid),
        None => Err(refl_error(format!(
            "Class \"{}\" does not exist",
            String::from_utf8_lossy(name)
        ))),
    }
}

// ---- class-model walks -----------------------------------------------------

/// The class and its ancestors, own class first — the order php reports
/// members in.
pub(crate) fn class_chain(ctx: &Ctx, cid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = Some(cid);
    while let Some(c) = cur {
        out.push(c);
        cur = ctx.class(c).parent;
    }
    out
}

/// The declared properties php lists for a class, in php's order: for each
/// class from the reflected one up the chain, its own instance properties in
/// slot order and then its own static properties.
///
/// A **private** property declared in an ancestor is left out, as php does
/// (it is not visible to the subclass at all).
///
/// *Divergence.* php interleaves static and instance properties in source
/// order within one class; the runtime splits them into `props` and
/// `static_props` at declaration time and the relative order between the two
/// lists is lost, so instance properties come first here.
pub(crate) fn ordered_props(ctx: &Ctx, cid: u32) -> Vec<(Box<[u8]>, u32)> {
    let def = ctx.class(cid);
    let mut out = Vec::new();
    // Both lists are already flattened parent-first and hold each property
    // exactly once, with `decl` pointing at the most derived declaration —
    // so grouping them by declaring class in chain order gives php's order
    // without listing a redeclared property twice.
    for c in class_chain(ctx, cid) {
        for p in &def.props {
            if p.decl == c && !(c != cid && p.vis == Visibility::Private) {
                out.push((p.name.clone(), c));
            }
        }
        for s in &def.static_props {
            if s.decl == c && !(c != cid && s.vis == Visibility::Private) {
                out.push((s.name.clone(), c));
            }
        }
    }
    out
}

/// Where a declared property of `cid` lives (its declaring class), or `None`
/// when the class has no such property.
///
/// A **private** property of an ancestor is invisible: php answers
/// `hasProperty()` with `false` and `getProperty()` with
/// `Property C::$p does not exist`. Methods behave the other way round — an
/// ancestor's private method is still found by `getMethod()` — so the two
/// lookups cannot share one rule.
pub(crate) fn declared_prop(ctx: &Ctx, cid: u32, name: &[u8]) -> Option<u32> {
    let def = ctx.class(cid);
    if let Some(p) = def.prop(name) {
        return (p.decl == cid || p.vis != Visibility::Private).then_some(p.decl);
    }
    if let Some(&i) = def.static_index.get(name) {
        let s = &def.static_props[i as usize];
        return (s.decl == cid || s.vis != Visibility::Private).then_some(s.decl);
    }
    None
}

/// The methods php lists for a class: the flattened table in the runtime's
/// `method_order` (own methods in declaration order, then the parent's, then
/// the interfaces'), minus the **private** methods of ancestors, which php
/// keeps out of the listing even though `hasMethod` still finds them.
pub(crate) fn ordered_methods(ctx: &Ctx, cid: u32) -> Vec<Rc<MethodDef>> {
    let def = ctx.class(cid);
    let mut out = Vec::new();
    for k in &def.method_order {
        let Some(m) = def.methods.get(k) else {
            continue;
        };
        if m.vis == Visibility::Private && m.decl != cid {
            continue;
        }
        out.push(m.clone());
    }
    out
}

// ---- static properties -----------------------------------------------------

/// The value of a static property, evaluating its initializer the way the
/// engine's first access would.
///
/// The engine's own entry point (`Interp::static_prop_cell`) is
/// `pub(crate)`, so this repeats its lazy-initialization protocol over the
/// public [`StaticPropInfo`](rphp_runtime::StaticPropInfo) fields: mark the
/// cell ready *before* evaluating, so an initializer that reaches its own
/// property sees the raw cell rather than recursing.
pub(crate) fn static_prop_value(ctx: &mut Ctx, cid: u32, idx: usize) -> Result<Value, Unwind> {
    let def = ctx.class(cid).clone();
    let info = &def.static_props[idx];
    let cell = info.cell.clone();
    let ready = info.ready.clone();
    if ready.get() {
        let v = cell.borrow().clone().unref();
        return Ok(v);
    }
    let init = info.init.clone();
    let decl = info.decl;
    ready.set(true);
    let v = match init {
        None => return Ok(Value::Uninit),
        Some(PropDefault::Value(v)) => v,
        Some(PropDefault::Thunk(fid)) => run_thunk(ctx, fid, Some(decl))?,
    };
    if v.is_uninit() {
        return Ok(v);
    }
    Value::assign(&mut cell.borrow_mut(), v.clone());
    Ok(v)
}

/// Write a static property through its shared cell, bypassing visibility the
/// way `ReflectionClass::setStaticPropertyValue` does.
pub(crate) fn set_static_prop(ctx: &mut Ctx, cid: u32, idx: usize, v: Value) {
    let def = ctx.class(cid).clone();
    let info = &def.static_props[idx];
    info.ready.set(true);
    Value::assign(&mut info.cell.borrow_mut(), v);
}

/// Run a zero-argument initializer thunk in `scope`'s class scope.
///
/// `Interp::run_thunk` is `pub(crate)`, so the thunk is invoked as an ordinary
/// closure: the engine appends `[$this, scope, called class]` to every
/// closure's captures and reads that tail back in `Interp::closure_binding`,
/// so a closure built with exactly that tail runs the body under the right
/// `self`/`static`.
pub(crate) fn run_thunk(ctx: &mut Ctx, fid: u32, scope: Option<u32>) -> Result<Value, Unwind> {
    let scope_v = scope.map_or(Value::Null, |c| Value::Int(i64::from(c)));
    let c = ctx.new_closure(fid, vec![Value::Null, scope_v.clone(), scope_v]);
    ctx.call_value(&Value::Closure(c), &[])
}

// ---- names -----------------------------------------------------------------

/// The part of a fully-qualified name after the last `\`.
pub(crate) fn short_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b'\\') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// The namespace part of a fully-qualified name (empty when there is none).
pub(crate) fn namespace_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b'\\') {
        Some(i) => &name[..i],
        None => &[],
    }
}

/// Whether a class is an enum.
pub(crate) fn is_enum(def: &ClassDef) -> bool {
    matches!(def.kind, ClassKind::Enum { .. })
}

/// `ReflectionClass::getModifiers()`: the `ZEND_ACC_*` bits php reports.
/// Interfaces and traits report `0`; an enum is always final.
pub(crate) fn class_modifiers(def: &ClassDef) -> i64 {
    match def.kind {
        ClassKind::Interface | ClassKind::Trait => 0,
        ClassKind::Enum { .. } => IS_FINAL,
        ClassKind::Class => {
            let mut m = 0;
            if def.flags.contains(ClassFlags::ABSTRACT) {
                m |= IS_ABSTRACT;
            }
            if def.flags.contains(ClassFlags::FINAL) {
                m |= IS_FINAL;
            }
            if def.flags.contains(ClassFlags::READONLY) {
                m |= IS_CLASS_READONLY;
            }
            m
        }
    }
}

/// `ReflectionMethod::getModifiers()`.
pub(crate) fn method_modifiers(m: &MethodDef) -> i64 {
    let mut bits = vis_bit(m.vis);
    if m.is_static {
        bits |= IS_STATIC;
    }
    if m.is_abstract {
        bits |= IS_ABSTRACT;
    }
    if m.is_final {
        bits |= IS_FINAL;
    }
    bits
}

// ---- small value helpers ---------------------------------------------------

/// A php array from an iterator of values (a packed list).
pub(crate) fn list(values: impl IntoIterator<Item = Value>) -> Value {
    let mut a = Array::new();
    for v in values {
        a.push(v);
    }
    Value::Array(a)
}

/// A string key.
pub(crate) fn key(b: &[u8]) -> ArrayKey {
    ArrayKey::str(b)
}

/// A `static` native-method descriptor (`nm!` builds instance methods).
pub(crate) fn snm(min: u8, max: Option<u8>, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

/// The body of a `Reflector` signature; never reached (an abstract method is
/// refused by dispatch before a body could run).
fn abstract_body(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot call abstract method"))
}

/// An abstract instance-method signature.
pub(crate) fn sig(min: u8, max: Option<u8>) -> NativeMethod {
    NativeMethod {
        handler: abstract_body,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: false,
        is_final: false,
    }
}

/// Turn a declaration's compiled attribute list into [`AttrInfo`]s.
///
/// A macro rather than a function because the element type is
/// `rphp_bytecode::AttrDef`, which this crate cannot name; expanding at the
/// call site lets inference supply it. `$owner` is the [`FnTarget`] whose
/// constant pool the argument initializers resolve against.
macro_rules! attr_list {
    ($attrs:expr, $owner:expr) => {{
        let attrs = $attrs;
        let owner: crate::reflection::common::AttrOwner = $owner;
        let lnames: Vec<Vec<u8>> = attrs.iter().map(|a| a.name.to_ascii_lowercase()).collect();
        let mut out: Vec<crate::reflection::common::AttrInfo> = Vec::new();
        for a in attrs.iter() {
            let lname = a.name.to_ascii_lowercase();
            let repeated = lnames.iter().filter(|n| **n == lname).count() > 1;
            let mut args = Vec::new();
            for (n, ir) in a.args.iter() {
                if let Some(k) = crate::reflection::common::parse_init_ref(&format!("{:?}", ir)) {
                    args.push((n.clone(), k));
                }
            }
            out.push(crate::reflection::common::AttrInfo {
                name: a.name.clone(),
                args,
                target: i64::from(a.target.mask()),
                repeated,
                owner: owner.clone(),
            });
        }
        out
    }};
}

pub(crate) use attr_list;
