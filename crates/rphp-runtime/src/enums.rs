//! Enums (plan E6).
//!
//! **This module owns enum cases.** A case is a *singleton* object: the first
//! evaluation of `E::Case` materializes it into
//! [`EnumCaseInfo::instance`](crate::EnumCaseInfo::instance) and every later
//! one — including `from()`, `tryFrom()` and `cases()` — hands back the same
//! object, so `E::A === E::A` holds and cases compare by identity.
//!
//! What lands here as E6 fills in:
//!
//! * materializing a case with its `name` (and `value` for a backed enum) as
//!   readonly properties;
//! * `cases()`, `from()` and `tryFrom()`, with php's `ValueError` text for a
//!   value that matches no case;
//! * the `UnitEnum` / `BackedEnum` interfaces, which every enum implements
//!   implicitly, so `instanceof` and type checks see them;
//! * the restrictions php enforces on enums: no instance properties, no
//!   `new`, no inheritance, constants and methods allowed.
//!
//! The implicit members — the `name`/`value` properties, the `UnitEnum` /
//! `BackedEnum` interfaces and the three methods below — are attached to the
//! [`ClassSpec`](crate::ClassSpec) of every enum declaration by
//! [`Interp::enum_implicits`] before the class links (`unit.rs`).
//!
//! The argument handling of `from()`/`tryFrom()` follows php's own parameter
//! parsing, which is *not* simply the backing type: an int-backed enum
//! accepts `int` (so `N::from([])` says "must be of type int"), a
//! string-backed one accepts `string|int` and converts an accepted int to its
//! string form (`S::from(1.5)` is the int `1` and then `"1"`).

use rphp_bytecode::{BuiltinType, TypeDecl, Visibility};
use rphp_value::{Array, Object, Value};

use crate::class::{
    ClassSpec, EnumBacking, EnumCaseValue, MethodBody, MethodSpec, NativeMethod, PropDefault,
};
use crate::frame::NativeTarget;
use crate::registry::{Ctx, NativeResult, Unwind};
use crate::types::Coerced;
use crate::Interp;

/// `UnitEnum::cases()` — every case of the enum, in declaration order.
const CASES: NativeMethod = NativeMethod {
    handler: enum_cases_handler,
    min_args: 0,
    max_args: Some(0),
    params: &[],
    by_ref: 0,
    is_static: true,
    is_final: false,
};

/// `BackedEnum::from()` — the case with this backing value, or a `ValueError`.
const FROM: NativeMethod = NativeMethod {
    handler: enum_from_handler,
    min_args: 1,
    max_args: Some(1),
    params: &["value"],
    by_ref: 0,
    is_static: true,
    is_final: false,
};

/// `BackedEnum::tryFrom()` — the case with this backing value, or `null`.
const TRY_FROM: NativeMethod = NativeMethod {
    handler: enum_try_from_handler,
    min_args: 1,
    max_args: Some(1),
    params: &["value"],
    by_ref: 0,
    is_static: true,
    is_final: false,
};

fn enum_cases_handler(ctx: &mut Ctx, _this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    let cid = ctx.enum_method_class()?;
    ctx.enum_cases_array(cid)
}

fn enum_from_handler(ctx: &mut Ctx, _this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let cid = ctx.enum_method_class()?;
    ctx.enum_lookup(cid, args[0].clone(), "from", false)
}

fn enum_try_from_handler(
    ctx: &mut Ctx,
    _this: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let cid = ctx.enum_method_class()?;
    ctx.enum_lookup(cid, args[0].clone(), "tryFrom", true)
}

impl Interp {
    /// `E::Case` — the singleton instance of an enum case.
    pub(crate) fn enum_case(&mut self, cid: u32, name: &[u8]) -> Result<Value, Unwind> {
        let class = self.classes[cid as usize].clone();
        let Some(&idx) = class.enum_index.get(name) else {
            return Err(Unwind::error(format!(
                "Undefined constant {}::{}",
                class.name_str(),
                String::from_utf8_lossy(name)
            )));
        };
        self.enum_case_at(cid, idx)
    }

    /// Build the backed-enum lookup table the way php does — on first touch,
    /// not at declaration: every case initializer runs, and two cases that
    /// end up with the same value are php's `Error`. A pure enum has no
    /// table and nothing to check.
    fn ensure_enum_table(&mut self, cid: u32) -> Result<(), Unwind> {
        let class = self.classes[cid as usize].clone();
        if class.enum_table_built.get() || class.enum_backing == EnumBacking::None {
            return Ok(());
        }
        // Set it first: an initializer that reaches back into the enum must
        // not restart the build.
        class.enum_table_built.set(true);
        let mut seen: Vec<(Box<[u8]>, Value)> = Vec::new();
        for i in 0..class.enum_cases.len() {
            let Some(v) = self.enum_case_value(cid, i as u16)? else {
                continue;
            };
            let name = class.enum_cases[i].name.clone();
            if let Some((first, _)) = seen.iter().find(|(_, s)| s.identical(&v)) {
                return Err(Unwind::error(format!(
                    "Duplicate value in enum {} for cases {} and {}",
                    class.name_str(),
                    String::from_utf8_lossy(first),
                    String::from_utf8_lossy(&name)
                )));
            }
            seen.push((name, v));
        }
        Ok(())
    }

    /// The backing value of case `idx`, running its initializer the first
    /// time it is asked for. php requires a constant *expression*, not a
    /// literal, so `case A = 1 << 0;` arrives here as a thunk.
    pub(crate) fn enum_case_value(&mut self, cid: u32, idx: u16) -> Result<Option<Value>, Unwind> {
        let info = self.classes[cid as usize].enum_cases[idx as usize].clone();
        let state = info.value.borrow().clone();
        match state {
            EnumCaseValue::None => Ok(None),
            EnumCaseValue::Ready(v) => Ok(Some(v)),
            EnumCaseValue::Pending(fid) => {
                let v = self.run_thunk(fid, None, Some(cid))?;
                *info.value.borrow_mut() = EnumCaseValue::Ready(v.clone());
                Ok(Some(v))
            }
        }
    }

    /// The singleton object of case `idx`, materialized on first use.
    fn enum_case_at(&mut self, cid: u32, idx: u16) -> Result<Value, Unwind> {
        let info = self.classes[cid as usize].enum_cases[idx as usize].clone();
        let cached = info.instance.borrow().clone();
        if let Some(v) = cached {
            return Ok(v);
        }
        self.ensure_enum_table(cid)?;
        let value = self.enum_case_value(cid, idx)?;
        let obj = self.instantiate(cid);
        // The formatters and `clone` recognise a case by this flag, so they
        // never have to consult the class table (`ObjFlags::ENUM_CASE`).
        obj.add_flags(rphp_value::ObjFlags::ENUM_CASE);
        obj.set(b"name", Value::string(&info.name));
        if let Some(v) = value {
            obj.set(b"value", v);
        }
        let v = Value::Object(obj);
        *info.instance.borrow_mut() = Some(v.clone());
        Ok(v)
    }

    /// `E::cases()` — every case, in declaration order.
    pub(crate) fn enum_cases_array(&mut self, cid: u32) -> Result<Value, Unwind> {
        let n = self.classes[cid as usize].enum_cases.len();
        let mut arr = Array::new();
        for i in 0..n {
            let case = self.enum_case_at(cid, i as u16)?;
            arr.push(case);
        }
        Ok(Value::Array(arr))
    }

    /// `E::from($v)` / `E::tryFrom($v)`.
    fn enum_lookup(
        &mut self,
        cid: u32,
        value: Value,
        method: &str,
        try_from: bool,
    ) -> Result<Value, Unwind> {
        let class = self.classes[cid as usize].clone();
        let backing = class.enum_backing;
        let value = self.enum_arg(cid, value, method, backing)?;
        self.ensure_enum_table(cid)?;
        let mut found = None;
        for i in 0..class.enum_cases.len() {
            if self
                .enum_case_value(cid, i as u16)?
                .is_some_and(|v| v.identical(&value))
            {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(i) => self.enum_case_at(cid, i as u16),
            None if try_from => Ok(Value::Null),
            None => Err(Unwind::value_error(format!(
                "{} is not a valid backing value for enum {}",
                enum_value_repr(&value),
                class.name_str()
            ))),
        }
    }

    /// Check and convert the argument of `from()`/`tryFrom()` the way php's
    /// own parameter parsing does (see the module header).
    fn enum_arg(
        &mut self,
        cid: u32,
        value: Value,
        method: &str,
        backing: EnumBacking,
    ) -> Result<Value, Unwind> {
        let class = self.classes[cid as usize].name_str();
        let accepted = match backing {
            EnumBacking::Int => TypeDecl::Builtin(BuiltinType::Int),
            // A string-backed enum accepts an int too and stringifies it.
            _ => TypeDecl::Union(vec![
                TypeDecl::Builtin(BuiltinType::String),
                TypeDecl::Builtin(BuiltinType::Int),
            ]),
        };
        let value = if matches!(value, Value::Null | Value::Uninit) {
            // php: the declared parameter is `string|int`, so null is the
            // deprecated implicit conversion to its first scalar, int 0.
            self.deprecated(&format!(
                "{class}::{method}(): Passing null to parameter #1 ($value) of type string|int is deprecated"
            ))?;
            Value::Int(0)
        } else {
            value
        };
        let strict = self
            .current_user_frame()
            .map_or(self.strict_default, |f| f.strict);
        let given = self.given_name(&value);
        let coerced = match self.coerce_to_type(value, &accepted, strict, None, None)? {
            Coerced::Ok(v) => v,
            Coerced::Mismatch => {
                return Err(Unwind::type_error(format!(
                    "{class}::{method}(): Argument #1 ($value) must be of type {accepted}, {given} given"
                )))
            }
        };
        Ok(match (backing, coerced) {
            (EnumBacking::String, Value::Int(i)) => Value::string(i.to_string().as_bytes()),
            (_, v) => v,
        })
    }

    /// The enum a native enum method is running for: its declaring class,
    /// which the native frame carries. Enums are final, so the declaring and
    /// the called class are always the same.
    fn enum_method_class(&self) -> Result<u32, Unwind> {
        match self.frames.last().and_then(|f| f.native.as_ref()) {
            Some((NativeTarget::Method(m), _)) => Ok(m.decl),
            _ => Err(Unwind::error(
                "internal error: an enum method ran outside its native frame",
            )),
        }
    }

    /// The members php gives every enum declaration implicitly: the readonly
    /// `name` (and `value`) properties, the `UnitEnum` / `BackedEnum`
    /// interfaces and `cases()` / `from()` / `tryFrom()`. Applied to the spec
    /// before it links, so they are ordinary own members afterwards.
    pub(crate) fn enum_implicits(&self, spec: &mut ClassSpec) {
        let backed = spec.enum_backing != EnumBacking::None;
        let value_ty = match spec.enum_backing {
            EnumBacking::Int => Some(TypeDecl::Builtin(BuiltinType::Int)),
            EnumBacking::String => Some(TypeDecl::Builtin(BuiltinType::String)),
            EnumBacking::None => None,
        };
        let mut props = vec![crate::class::PropSpec {
            name: Box::from(&b"name"[..]),
            vis: Visibility::Public,
            set_vis: None,
            ty: Some(TypeDecl::Builtin(BuiltinType::String)),
            readonly: true,
            hooks: None,
            doc: None,
            default: PropDefault::Value(Value::Uninit),
        }];
        if let Some(ty) = value_ty {
            props.push(crate::class::PropSpec {
                name: Box::from(&b"value"[..]),
                vis: Visibility::Public,
                set_vis: None,
                doc: None,
                ty: Some(ty),
                readonly: true,
                hooks: None,
                default: PropDefault::Value(Value::Uninit),
            });
        }
        props.extend(std::mem::take(&mut spec.props));
        spec.props = props;
        let mut ifaces: Vec<&[u8]> = vec![b"UnitEnum"];
        if backed {
            ifaces.push(b"BackedEnum");
        }
        for name in ifaces {
            if let Some(id) = self.class_by_name(name) {
                if !spec.interfaces.contains(&id) {
                    spec.interfaces.push(id);
                }
            }
        }
        // The declared methods come first and the implicit ones last, which
        // is the order `get_class_methods()` reports. Redeclaring one of them
        // is a compile error in php, so the implicit versions always win.
        let mut methods: Vec<MethodSpec> = std::mem::take(&mut spec.methods)
            .into_iter()
            .filter(|m| !is_implicit_method(&m.name))
            .collect();
        methods.push(native_method(b"cases", CASES));
        if backed {
            methods.push(native_method(b"from", FROM));
            methods.push(native_method(b"tryFrom", TRY_FROM));
        }
        spec.methods = methods;
    }
}

/// Whether a method name is one of the implicit enum methods.
fn is_implicit_method(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(b"cases")
        || name.eq_ignore_ascii_case(b"from")
        || name.eq_ignore_ascii_case(b"tryFrom")
}

/// One implicit enum method as a [`MethodSpec`].
fn native_method(name: &[u8], m: NativeMethod) -> MethodSpec {
    MethodSpec {
        name: Box::from(name),
        body: MethodBody::Native(m),
        vis: Visibility::Public,
        is_static: true,
        is_abstract: false,
        is_final: false,
    }
}

/// php's rendering of a backing value in the `ValueError`: a string is
/// double-quoted (never escaped), an int is bare.
fn enum_value_repr(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("\"{}\"", s.to_string_lossy()),
        other => other.to_php_string(),
    }
}
