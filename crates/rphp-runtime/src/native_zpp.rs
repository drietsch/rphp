//! php's parameter parsing for natives (`zend_parse_parameters`, the
//! `Z_PARAM_*` macros): before a handler runs, every positional argument
//! is checked against the parameter type the stub declares — the
//! generated [`crate::native_params`] table — and coerced the way php's
//! weak mode coerces it, or refused with php's `TypeError`.
//!
//! The rules (`Zend/zend_API.c`, `zend_parse_arg_*_weak`;
//! `Zend/zend_execute.c`, `zend_verify_weak_scalar_type_hint`):
//!
//! * a value whose type the parameter lists is taken as it is;
//! * `null` for a non-nullable *scalar* parameter is php 8.1's
//!   `Passing null to parameter #n ($x) of type T is deprecated`, then the
//!   scalar's zero (`0`, `0.0`, `""`, `false`); for a parameter that lists
//!   no scalar (`array`, `Countable|array`) it is the `TypeError`;
//! * otherwise the preference order is int, float, string, bool: an int
//!   parameter takes a whole float (a fractional one with php's
//!   `Implicit conversion from float … to int loses precision`), a numeric
//!   string (a float-string likewise), a bool; an `int|float` parameter
//!   takes a numeric string as the number it spells; a float parameter
//!   takes ints, numeric strings and bools; a string parameter takes any
//!   scalar (an object with `__toString()` was fitted earlier, by
//!   [`crate::Interp::coerce_object_params`]); a bool parameter takes any
//!   scalar; an `array` parameter takes nothing else;
//! * under the caller's `declare(strict_types=1)` only an int for a float
//!   parameter converts;
//! * a by-reference position, a variadic tail, a parameter the table does
//!   not type (`mixed`, an untyped `$key`) and one that lists `callable`,
//!   `iterable`, `object`, a class or `resource` are left to the handler.
//!
//! Every native's parameter specs are resolved once, at registration
//! ([`ParamSpec`]), so a call pays a discriminant check per argument.

use rphp_value::{numeric_string, Value};

use crate::registry::{NativeFn, Unwind};
use crate::{value_name, Interp};

/// What a parameter accepts, as a bit set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypeMask(u16);

impl TypeMask {
    pub const INT: TypeMask = TypeMask(1);
    pub const FLOAT: TypeMask = TypeMask(2);
    pub const STRING: TypeMask = TypeMask(4);
    pub const BOOL: TypeMask = TypeMask(8);
    pub const NULL: TypeMask = TypeMask(16);
    pub const ARRAY: TypeMask = TypeMask(32);
    /// `callable`, `iterable`, `object`, a class, `resource`, `mixed`:
    /// something the handler judges.
    pub const OTHER: TypeMask = TypeMask(64);
    /// `mixed` (or an untyped parameter): anything at all.
    pub const ANY: TypeMask = TypeMask(128);

    fn has(self, other: TypeMask) -> bool {
        self.0 & other.0 != 0
    }

    fn or(self, other: TypeMask) -> TypeMask {
        TypeMask(self.0 | other.0)
    }

    /// Whether any scalar (int, float, string, bool) is accepted.
    fn any_scalar(self) -> bool {
        self.has(TypeMask::INT.or(TypeMask::FLOAT).or(TypeMask::STRING).or(TypeMask::BOOL))
    }

    /// The mask a stub's type string denotes.
    pub fn parse(ty: &str) -> TypeMask {
        let mut m = TypeMask(0);
        for part in ty.split('|') {
            let part = part.trim();
            let (nullable, part) = match part.strip_prefix('?') {
                Some(rest) => (true, rest),
                None => (false, part),
            };
            if nullable {
                m = m.or(TypeMask::NULL);
            }
            m = m.or(match part {
                "int" => TypeMask::INT,
                "float" => TypeMask::FLOAT,
                "string" => TypeMask::STRING,
                "bool" | "false" | "true" => TypeMask::BOOL,
                "null" => TypeMask::NULL,
                "array" => TypeMask::ARRAY,
                "mixed" => TypeMask::ANY,
                "void" | "never" => TypeMask(0),
                _ => TypeMask::OTHER,
            });
        }
        m
    }
}

/// One parameter's contract, resolved from the table at registration.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    /// The type as the stub spells it (for the messages).
    pub ty: &'static str,
    pub mask: TypeMask,
    pub variadic: bool,
}

/// The specs of a native's parameters, from its table row.
pub(crate) fn specs_of(row: crate::native_args::ParamRow) -> std::rc::Rc<[ParamSpec]> {
    row.iter()
        .map(|(name, ty, default)| ParamSpec {
            name,
            ty: ty.unwrap_or("mixed"),
            mask: match ty {
                Some(t) => TypeMask::parse(t),
                None => TypeMask::ANY,
            },
            variadic: *default == Some("..."),
        })
        .collect()
}

/// The verdict of one weak coercion.
enum Fit {
    /// The value as it should reach the handler.
    Ok(Value),
    /// No member of the type takes it.
    Mismatch,
}

impl Interp {
    /// Check and coerce `args` for native `f` (see the module notes);
    /// `strict` is the caller's `strict_types`.
    pub(crate) fn parse_native_params(
        &mut self,
        f: &NativeFn,
        specs: &[ParamSpec],
        args: &mut [Value],
        strict: bool,
    ) -> Result<(), Unwind> {
        self.parse_params_named(&|| f.name.to_string(), f.by_ref, specs, args, strict)
    }

    /// [`Self::parse_native_params`] by display name (`Class::method`,
    /// built only for a message) and by-reference mask.
    pub(crate) fn parse_params_named(
        &mut self,
        display: &dyn Fn() -> String,
        by_ref: u32,
        specs: &[ParamSpec],
        args: &mut [Value],
        strict: bool,
    ) -> Result<(), Unwind> {
        if specs.is_empty() {
            return Ok(());
        }
        for (i, arg) in args.iter_mut().enumerate() {
            let Some(spec) = specs.get(i).filter(|s| !s.variadic) else {
                break;
            };
            // A parameter that lists something only the handler can judge
            // (`callable`, a class) is left to it whole: a string may be a
            // callable, an array a callable or a `Countable`'s kin.
            let is_by_ref = i < 32 && by_ref & (1 << i) != 0;
            if spec.mask.has(TypeMask::ANY) || spec.mask.has(TypeMask::OTHER) || is_by_ref {
                continue;
            }
            let mask = spec.mask;
            // The value's own type, when the parameter lists it.
            let exact = match &*arg {
                Value::Int(_) => mask.has(TypeMask::INT),
                Value::Float(_) => mask.has(TypeMask::FLOAT),
                Value::Str(_) => mask.has(TypeMask::STRING),
                Value::Bool(_) => mask.has(TypeMask::BOOL),
                Value::Null | Value::Uninit => mask.has(TypeMask::NULL),
                Value::Array(_) => mask.has(TypeMask::ARRAY),
                // Objects, closures, resources and references are the
                // handler's (or `coerce_object_params`'s) to judge.
                _ => true,
            };
            if exact {
                continue;
            }
            // A skipped optional position stays for `RecvInit`-style
            // defaulting in the handler.
            if arg.is_uninit() {
                continue;
            }
            let fit = if strict {
                // Strict: only an int widens to a float; `null` is refused.
                match &*arg {
                    Value::Int(n) if mask.has(TypeMask::FLOAT) => Fit::Ok(Value::Float(*n as f64)),
                    _ => Fit::Mismatch,
                }
            } else if let Value::Null = arg {
                self.fit_null(display, i, spec)?
            } else {
                self.fit_weak(arg, mask)?
            };
            match fit {
                Fit::Ok(v) => *arg = v,
                Fit::Mismatch => {
                    return Err(Unwind::type_error(format!(
                        "{}(): Argument #{} (${}) must be of type {}, {} given",
                        display(),
                        i + 1,
                        spec.name,
                        spec.ty,
                        value_name(arg)
                    )));
                }
            }
        }
        Ok(())
    }

    /// `null` for a non-nullable parameter: a scalar's zero after php's
    /// deprecation, else a mismatch.
    fn fit_null(&mut self, display: &dyn Fn() -> String, i: usize, spec: &ParamSpec) -> Result<Fit, Unwind> {
        let mask = spec.mask;
        if !mask.any_scalar() {
            return Ok(Fit::Mismatch);
        }
        self.deprecated(&format!(
            "{}(): Passing null to parameter #{} (${}) of type {} is deprecated",
            display(),
            i + 1,
            spec.name,
            spec.ty
        ))?;
        // The first scalar parser in php's order takes it.
        Ok(Fit::Ok(if mask.has(TypeMask::INT) {
            Value::Int(0)
        } else if mask.has(TypeMask::FLOAT) {
            Value::Float(0.0)
        } else if mask.has(TypeMask::STRING) {
            Value::string(b"")
        } else {
            Value::Bool(false)
        }))
    }

    /// php's weak scalar coercion, in its preference order (the same
    /// helpers a typed user parameter goes through, `types.rs`).
    fn fit_weak(&mut self, arg: &Value, mask: TypeMask) -> Result<Fit, Unwind> {
        // Nothing but a scalar converts to a scalar.
        if !matches!(arg, Value::Int(_) | Value::Float(_) | Value::Str(_) | Value::Bool(_)) {
            return Ok(Fit::Mismatch);
        }
        if mask.has(TypeMask::INT) {
            // An `int|float` parameter takes a numeric string as the number
            // it spells.
            if mask.has(TypeMask::FLOAT) {
                if let Value::Str(s) = arg {
                    if let Some(n) = numeric_string(s.as_bytes()) {
                        return Ok(Fit::Ok(n));
                    }
                }
            }
            if let Some(n) = self.to_int_weak(arg)? {
                return Ok(Fit::Ok(Value::Int(n)));
            }
        }
        if mask.has(TypeMask::FLOAT) {
            if let Some(d) = self.to_float_weak(arg) {
                return Ok(Fit::Ok(Value::Float(d)));
            }
        }
        if mask.has(TypeMask::STRING) {
            return Ok(Fit::Ok(Value::Str(rphp_value::Str::from_vec(arg.to_php_bytes()))));
        }
        if mask.has(TypeMask::BOOL) {
            return Ok(Fit::Ok(Value::Bool(arg.to_bool())));
        }
        Ok(Fit::Mismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_parse_the_stub_spellings() {
        assert!(TypeMask::parse("?string").has(TypeMask::NULL));
        assert!(TypeMask::parse("?string").has(TypeMask::STRING));
        let u = TypeMask::parse("int|float");
        assert!(u.has(TypeMask::INT) && u.has(TypeMask::FLOAT) && !u.has(TypeMask::STRING));
        assert!(TypeMask::parse("Countable|array").has(TypeMask::OTHER));
        assert!(!TypeMask::parse("Countable|array").any_scalar());
        assert!(TypeMask::parse("mixed").has(TypeMask::ANY));
    }
}
