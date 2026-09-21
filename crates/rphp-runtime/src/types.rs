//! Declared-type checks and coercion (plan E5, php-src
//! `Zend/zend_execute.c: zend_verify_arg_type`, `zend_verify_weak_scalar_type_hint`,
//! `zend_verify_return_type`): parameters are checked when a call is
//! activated (under the *caller's* `strict_types`), return values at `Ret`
//! (under the callee's). The rules:
//!
//! 1. An exact match by type wins (`null` only when the type admits it;
//!    `int` → `float` widening is allowed even in strict mode).
//! 2. Otherwise, in coercive mode, a scalar is juggled in php's preference
//!    order `int → float → string → bool`: a float or numeric string to
//!    `int` when it fits (a fractional part is deprecated and truncated),
//!    any numeric to `float`, any scalar (or `Stringable` object) to
//!    `string`, any scalar to `bool`.
//! 3. Anything else is a `TypeError` with php's text.

use rphp_bytecode::{BuiltinType, TypeDecl};
use rphp_value::{Str, Value};

use crate::ops::value_name;
use crate::registry::Unwind;
use crate::unit::FuncRt;
use crate::Interp;

/// The outcome of a coercion attempt.
pub enum Coerced {
    /// The (possibly converted) value.
    Ok(Value),
    /// The value does not satisfy the type.
    Mismatch,
}

impl Interp {
    /// Whether `v` satisfies `ty` exactly (no juggling). `scope`/`static_class`
    /// resolve `self`/`parent`/`static`.
    fn matches_exact(&self, v: &Value, ty: &TypeDecl, scope: Option<u32>, static_class: Option<u32>) -> bool {
        match ty {
            TypeDecl::Named(name) => match v {
                Value::Object(o) => self
                    .class_by_name(name)
                    .is_some_and(|cid| self.object_instanceof(o, cid)),
                Value::Closure(_) => name.eq_ignore_ascii_case(b"closure"),
                _ => false,
            },
            TypeDecl::Builtin(b) => match b {
                BuiltinType::Int => matches!(v, Value::Int(_)),
                BuiltinType::Float => matches!(v, Value::Float(_)),
                BuiltinType::String => matches!(v, Value::Str(_)),
                BuiltinType::Bool => matches!(v, Value::Bool(_)),
                BuiltinType::True => matches!(v, Value::Bool(true)),
                BuiltinType::False => matches!(v, Value::Bool(false)),
                BuiltinType::Array => matches!(v, Value::Array(_)),
                BuiltinType::Object => matches!(v, Value::Object(_) | Value::Closure(_)),
                BuiltinType::Mixed => true,
                BuiltinType::Null | BuiltinType::Void => matches!(v, Value::Null),
                BuiltinType::Never => false,
                BuiltinType::Callable => self.is_callable(v),
                BuiltinType::Iterable => match v {
                    Value::Array(_) => true,
                    Value::Object(o) => self
                        .class_by_name(b"Traversable")
                        .is_some_and(|t| self.object_instanceof(o, t)),
                    _ => false,
                },
                BuiltinType::SelfTy => match (v, scope) {
                    (Value::Object(o), Some(c)) => self.object_instanceof(o, c),
                    _ => false,
                },
                BuiltinType::StaticTy => match (v, static_class.or(scope)) {
                    (Value::Object(o), Some(c)) => self.object_instanceof(o, c),
                    _ => false,
                },
                BuiltinType::ParentTy => match (v, scope.and_then(|c| self.classes[c as usize].parent)) {
                    (Value::Object(o), Some(p)) => self.object_instanceof(o, p),
                    _ => false,
                },
            },
            TypeDecl::Nullable(inner) => {
                matches!(v, Value::Null) || self.matches_exact(v, inner, scope, static_class)
            }
            TypeDecl::Union(parts) => parts
                .iter()
                .any(|p| self.matches_exact(v, p, scope, static_class)),
            TypeDecl::Intersection(parts) => parts
                .iter()
                .all(|p| self.matches_exact(v, p, scope, static_class)),
        }
    }

    /// Collect the scalar keywords a type admits for weak coercion.
    fn scalar_mask(ty: &TypeDecl, mask: &mut u8) {
        const INT: u8 = 1;
        const FLOAT: u8 = 2;
        const STRING: u8 = 4;
        const BOOL: u8 = 8;
        match ty {
            TypeDecl::Builtin(BuiltinType::Int) => *mask |= INT,
            TypeDecl::Builtin(BuiltinType::Float) => *mask |= FLOAT,
            TypeDecl::Builtin(BuiltinType::String) => *mask |= STRING,
            TypeDecl::Builtin(BuiltinType::Bool) => *mask |= BOOL,
            TypeDecl::Nullable(inner) => Self::scalar_mask(inner, mask),
            TypeDecl::Union(parts) => {
                for p in parts {
                    Self::scalar_mask(p, mask);
                }
            }
            _ => {}
        }
    }

    /// A float as an `int` parameter: `None` when it does not fit; a
    /// fractional part is deprecated (php 8.1) and truncated.
    fn float_to_int_weak(&mut self, f: f64, source: Option<&Str>) -> Result<Option<i64>, Unwind> {
        if f.is_nan() || f.is_infinite() || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&f) {
            return Ok(None);
        }
        let i = f as i64;
        if (i as f64) != f {
            let msg = match source {
                Some(s) => format!(
                    "Implicit conversion from float-string \"{}\" to int loses precision",
                    s.to_string_lossy()
                ),
                None => format!(
                    "Implicit conversion from float {} to int loses precision",
                    Value::Float(f).to_php_string()
                ),
            };
            self.deprecated(&msg)?;
        }
        Ok(Some(i))
    }

    /// `zend_parse_arg_long_weak`.
    fn to_int_weak(&mut self, v: &Value) -> Result<Option<i64>, Unwind> {
        Ok(match v {
            Value::Int(i) => Some(*i),
            Value::Float(f) => self.float_to_int_weak(*f, None)?,
            Value::Bool(b) => Some(i64::from(*b)),
            Value::Str(s) => {
                if !v.is_numeric() {
                    return Ok(None);
                }
                match v.to_number() {
                    Value::Int(i) => Some(i),
                    Value::Float(f) => self.float_to_int_weak(f, Some(s))?,
                    _ => None,
                }
            }
            _ => None,
        })
    }

    /// `zend_parse_arg_double_weak`.
    fn to_float_weak(&self, v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Str(_) if v.is_numeric() => Some(v.to_float()),
            _ => None,
        }
    }

    /// `zend_parse_arg_str_weak` (objects through `__toString`).
    fn to_string_weak(&mut self, v: &Value) -> Result<Option<Value>, Unwind> {
        Ok(match v {
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_) => {
                Some(Value::Str(Str::from_vec(v.to_php_bytes())))
            }
            Value::Object(o) if self.has_to_string(o) => Some(Value::Str(self.object_to_string(o)?)),
            _ => None,
        })
    }

    /// Check / coerce `v` against `ty`.
    pub fn coerce_to_type(
        &mut self,
        v: Value,
        ty: &TypeDecl,
        strict: bool,
        scope: Option<u32>,
        static_class: Option<u32>,
    ) -> Result<Coerced, Unwind> {
        let v = v.deref().into_owned();
        let v = if v.is_uninit() { Value::Null } else { v };
        if self.matches_exact(&v, ty, scope, static_class) {
            return Ok(Coerced::Ok(v));
        }
        let mut mask = 0u8;
        Self::scalar_mask(ty, &mut mask);
        let (has_int, has_float, has_string, has_bool) =
            (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0);
        // int → float widening holds in strict mode too.
        if let Value::Int(i) = v {
            if has_float {
                return Ok(Coerced::Ok(Value::Float(i as f64)));
            }
        }
        if strict || matches!(v, Value::Null | Value::Array(_) | Value::Resource(_) | Value::Closure(_)) {
            return Ok(Coerced::Mismatch);
        }
        if let Value::Object(_) = v {
            if has_string {
                if let Some(s) = self.to_string_weak(&v)? {
                    return Ok(Coerced::Ok(s));
                }
            }
            return Ok(Coerced::Mismatch);
        }
        if has_int {
            if has_float {
                if let Value::Str(_) = &v {
                    if v.is_numeric() {
                        return Ok(Coerced::Ok(v.to_number()));
                    }
                }
            }
            if let Some(i) = self.to_int_weak(&v)? {
                return Ok(Coerced::Ok(Value::Int(i)));
            }
        }
        if has_float {
            if let Some(f) = self.to_float_weak(&v) {
                return Ok(Coerced::Ok(Value::Float(f)));
            }
        }
        if has_string {
            if let Some(s) = self.to_string_weak(&v)? {
                return Ok(Coerced::Ok(s));
            }
        }
        if has_bool {
            return Ok(Coerced::Ok(Value::Bool(v.to_bool())));
        }
        Ok(Coerced::Mismatch)
    }

    /// php's spelling of a declared type in messages (`?int`,
    /// `Traversable|array` for `iterable`, `self` resolved to the class).
    pub fn type_display(&self, ty: &TypeDecl, scope: Option<u32>) -> String {
        match ty {
            TypeDecl::Builtin(BuiltinType::Iterable) => "Traversable|array".to_string(),
            TypeDecl::Builtin(BuiltinType::SelfTy) => match scope {
                Some(c) => self.classes[c as usize].name_str(),
                None => "self".to_string(),
            },
            TypeDecl::Nullable(inner) if matches!(**inner, TypeDecl::Builtin(BuiltinType::Iterable)) => {
                "Traversable|array|null".to_string()
            }
            _ => ty.to_string(),
        }
    }

    /// The word for a value in `…, <type> given` messages
    /// (`zend_zval_value_name`).
    pub fn given_name(&self, v: &Value) -> String {
        value_name(v)
    }

    /// Check the typed parameters of a just-activated frame (the caller's
    /// strictness `strict`): coerce in place, or return php's `TypeError`.
    /// `caller` is the `called in %s on line %d` site (`None` from a native).
    pub(crate) fn verify_params(
        &mut self,
        func: &FuncRt,
        base: usize,
        argc: usize,
        strict: bool,
        caller: Option<usize>,
    ) -> Result<(), Unwind> {
        let (scope, static_class) = {
            let f = self.frames.last().expect("callee frame");
            (f.scope, f.static_class)
        };
        for (i, p) in func.f.params.iter().enumerate() {
            let Some(ty) = &p.ty else { continue };
            if i >= argc || p.variadic {
                continue;
            }
            let slot = &self.stack[base + p.reg as usize];
            if slot.is_uninit() {
                continue; // filled by RecvInit
            }
            // An exact match leaves the register alone: no clone, no
            // comparison (an array argument would be compared element by
            // element otherwise).
            if self.matches_exact(&slot.deref(), ty, scope, static_class) {
                continue;
            }
            let v = slot.deref().into_owned();
            match self.coerce_to_type(v.clone(), ty, strict, scope, static_class)? {
                Coerced::Ok(nv) => {
                    Value::assign(&mut self.stack[base + p.reg as usize], nv);
                }
                Coerced::Mismatch => {
                    let fname = self.callable_display_name(func);
                    let mut msg = format!(
                        "{fname}(): Argument #{} (${}) must be of type {}, {} given",
                        i + 1,
                        String::from_utf8_lossy(&p.name),
                        self.type_display(ty, scope),
                        self.given_name(&v)
                    );
                    // The caller's site (a frame index: the file and line
                    // are only spelled out for the message).
                    if let Some(fi) = caller {
                        let f = &self.frames[fi];
                        let (file, line) = (self.frame_file(f), self.frame_line(f));
                        msg.push_str(&format!(", called in {file} on line {line}"));
                    }
                    return Err(Unwind::type_error(msg));
                }
            }
        }
        Ok(())
    }

    /// Check a return value against the function's declared return type
    /// (`Ret`): `None` is `return;` / falling off the end.
    pub(crate) fn verify_return(&mut self, func: &FuncRt, v: Option<Value>, strict: bool) -> Result<Value, Unwind> {
        let Some(ty) = &func.f.ret_ty else {
            return Ok(v.unwrap_or(Value::Null));
        };
        let (scope, static_class) = {
            let f = self.frames.last().expect("frame");
            (f.scope, f.static_class)
        };
        let is_void = matches!(ty, TypeDecl::Builtin(BuiltinType::Void));
        let Some(v) = v else {
            if is_void {
                return Ok(Value::Null);
            }
            return Err(Unwind::type_error(format!(
                "{}(): Return value must be of type {}, none returned",
                self.callable_display_name(func),
                self.type_display(ty, scope)
            )));
        };
        if is_void {
            return Ok(Value::Null);
        }
        if matches!(ty, TypeDecl::Builtin(BuiltinType::Never)) {
            return Err(Unwind::type_error(format!(
                "{}(): never-returning function must not implicitly return",
                self.callable_display_name(func)
            )));
        }
        match self.coerce_to_type(v.clone(), ty, strict, scope, static_class)? {
            Coerced::Ok(nv) => Ok(nv),
            Coerced::Mismatch => Err(Unwind::type_error(format!(
                "{}(): Return value must be of type {}, {} returned",
                self.callable_display_name(func),
                self.type_display(ty, scope),
                self.given_name(&v)
            ))),
        }
    }
}
