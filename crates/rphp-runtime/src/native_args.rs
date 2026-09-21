//! Named arguments and skipped positions on a native call: `json_decode($s,
//! flags: JSON_THROW_ON_ERROR)`.
//!
//! A user function's frame carries its parameter names and its `RecvInit`
//! ops fill the defaults, so `call_user_func` does both by itself. A native
//! has neither: its `nf!` row is a handler and an arity range. php's
//! arginfo — the names and the default expressions of every native, as the
//! stubs spell them — lives in the generated [`crate::native_params`] table
//! (`cargo xtask native-params`), and this module applies it to a call
//! window before the handler runs:
//!
//! * each named argument lands in its parameter's position, an unknown one
//!   is php's `Unknown named parameter $x` (or, for a variadic native, the
//!   `ArgumentCountError` "does not accept unknown named parameters" —
//!   except for the few natives that forward them to the callable they
//!   invoke, [`PASS_THROUGH`], which pick them up with
//!   [`Interp::take_extra_named`]);
//! * a required parameter a call skipped is `Argument #n ($x) not passed`;
//! * an optional one it skipped whose default php itself does not know
//!   (`<default>` in the table: `array_keys()`'s `$filter_value`) is
//!   `must be passed explicitly, because the default value is not known`;
//! * an optional one it skipped gets its default, evaluated from the stub's
//!   expression: literals, `[]`, constants, `Class::CONST`, `Enum::Case`,
//!   `Class::class` and `|` / `*` of those — the handler then sees the same
//!   value an explicit argument would have carried, so it needs no idea the
//!   position was skipped. An expression this cannot evaluate falls back to
//!   `null`, which is what the position held before this module existed.

use rphp_value::{Array, Value};

use crate::native_params::NATIVE_PARAMS;
use crate::{Interp, Unwind};

/// One native's parameters: name, declared type (as `ReflectionType`
/// prints it) and default expression (`None` = required, `...` =
/// variadic, `<default>` = optional but unknown).
pub type ParamRow = crate::native_params::Params;

fn entry(key: &str) -> Option<&'static (&'static str, Option<&'static str>, ParamRow)> {
    NATIVE_PARAMS
        .binary_search_by(|(n, _, _)| (*n).cmp(key))
        .ok()
        .map(|i| &NATIVE_PARAMS[i])
}

/// The parameters of native `key` (lowercase; `class::method`), when php
/// declares it.
pub(crate) fn params_of(key: &str) -> Option<ParamRow> {
    entry(key).map(|e| e.2)
}

/// php's arginfo for a native, by the name php reports (`f` or
/// `Class::m`, any case): what Reflection describes.
pub fn native_arginfo(display: &str) -> Option<ParamRow> {
    params_of(&display.to_ascii_lowercase())
}

/// A native's declared return type, as `ReflectionType` prints it;
/// `None` when php declares none (a constructor) or the native is unknown.
pub fn native_return_type(display: &str) -> Option<&'static str> {
    entry(&display.to_ascii_lowercase()).and_then(|e| e.1)
}

/// Whether a stub default expression is one constant fetch
/// (`COUNT_NORMAL`, `RoundingMode::HalfAwayFromZero`): php's
/// `ReflectionParameter::isDefaultValueConstant()` for a native.
pub fn native_default_is_constant(expr: &str) -> bool {
    let bare = expr.trim_start_matches('\\');
    !bare.is_empty()
        && !matches!(bare, "null" | "true" | "false" | "..." | "<default>")
        && !bare.ends_with("::class")
        && !bare.bytes().next().is_some_and(|b| b.is_ascii_digit() || b == b'-')
        && bare
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':' || b == b'\\')
}

/// The natives whose variadic takes unknown named arguments and hands them
/// on to the callable they invoke (php's `Z_PARAM_VARIADIC_WITH_NAMED`).
const PASS_THROUGH: &[&str] = &[
    "call_user_func",
    "closure::__invoke",
    "closure::call",
    "fiber::start",
    "reflectionclass::newinstance",
    "reflectionfunction::invoke",
    "reflectionmethod::invoke",
];

/// Named arguments as the call sequence collects them: name, value.
pub(crate) type NamedArgs = Vec<(Box<[u8]>, Value)>;

/// Why a native's named arguments did not bind.
pub(crate) enum NamedFault {
    /// Raised before the callee's frame exists (php's `Unknown named
    /// parameter`, `overwrites previous argument`): the trace starts at
    /// the caller.
    Outside(Unwind),
    /// Raised from inside the callee's frame, which php pushes first (the
    /// `ArgumentCountError`s): the trace shows the call, skipped positions
    /// as `NULL`, the unknown named arguments by name.
    Inside(Unwind, NamedArgs),
}

impl Interp {
    /// Place `named` into `args` for native `display` (`f` or `Class::m`,
    /// as php names it in messages) and fill the positions the call
    /// skipped; see the module documentation for the rules. The unknown
    /// named arguments a pass-through native forwards come back for its
    /// frame.
    pub(crate) fn bind_native_named(
        &mut self,
        display: &str,
        args: &mut Vec<Value>,
        named: NamedArgs,
    ) -> Result<NamedArgs, NamedFault> {
        if named.is_empty() {
            return Ok(Vec::new());
        }
        let key = display.to_ascii_lowercase();
        let Some(params) = params_of(&key) else {
            return Err(NamedFault::Outside(Unwind::error(format!(
                "Unknown named parameter ${}",
                String::from_utf8_lossy(&named[0].0)
            ))));
        };
        let variadic = params.last().is_some_and(|(_, _, d)| *d == Some("..."));
        let declared = if variadic { params.len() - 1 } else { params.len() };
        let mut extra: NamedArgs = Vec::new();
        for (name, v) in named {
            let pos = params
                .iter()
                .take(declared)
                .position(|(n, _, _)| n.as_bytes() == name.as_ref());
            match pos {
                Some(p) => {
                    if p < args.len() {
                        if !args[p].is_uninit() {
                            return Err(NamedFault::Outside(Unwind::error(format!(
                                "Named parameter ${} overwrites previous argument",
                                String::from_utf8_lossy(&name)
                            ))));
                        }
                        args[p] = v;
                    } else {
                        while args.len() < p {
                            args.push(Value::Uninit);
                        }
                        args.push(v);
                    }
                }
                None if variadic => extra.push((name, v)),
                None => {
                    return Err(NamedFault::Outside(Unwind::error(format!(
                        "Unknown named parameter ${}",
                        String::from_utf8_lossy(&name)
                    ))))
                }
            }
        }
        if !extra.is_empty() && !PASS_THROUGH.contains(&key.as_str()) {
            return Err(NamedFault::Inside(
                Unwind::argument_count_error(format!(
                    "{display}() does not accept unknown named parameters"
                )),
                extra,
            ));
        }
        for (i, (n, _, d)) in params.iter().take(declared).enumerate() {
            if d.is_none() && args.get(i).is_none_or(Value::is_uninit) {
                return Err(NamedFault::Inside(
                    Unwind::argument_count_error(format!(
                        "{display}(): Argument #{} (${n}) not passed",
                        i + 1
                    )),
                    extra,
                ));
            }
        }
        for i in 0..args.len() {
            if args[i].is_uninit() {
                let (name, expr) = params
                    .get(i)
                    .map(|(n, _, d)| (*n, d.unwrap_or("null")))
                    .unwrap_or(("", "null"));
                if expr == "<default>" {
                    return Err(NamedFault::Inside(
                        Unwind::argument_count_error(format!(
                            "{display}(): Argument #{} (${name}) must be passed explicitly, because the default value is not known",
                            i + 1
                        )),
                        extra,
                    ));
                }
                args[i] = self.native_default(expr);
            }
        }
        Ok(extra)
    }

    /// [`Interp::bind_native_named`] for the `Closure` method trampoline,
    /// whose window carries the receiver at position 0 ahead of php's
    /// parameters.
    pub(crate) fn bind_closure_method_named(
        &mut self,
        display: &str,
        args: &mut Vec<Value>,
        named: NamedArgs,
    ) -> Result<NamedArgs, NamedFault> {
        if named.is_empty() {
            return Ok(Vec::new());
        }
        let receiver = args.remove(0);
        let r = self.bind_native_named(display, args, named);
        args.insert(0, receiver);
        r
    }

    /// The unknown named arguments the running pass-through native was
    /// called with (`call_user_func($f, x: 1)`), to forward to the callable
    /// it invokes; taken once.
    pub fn take_extra_named(&mut self) -> NamedArgs {
        self.frames
            .last_mut()
            .map(|f| std::mem::take(&mut f.extra_mut().extra_named))
            .unwrap_or_default()
    }

    /// Evaluate a stub default expression; `null` when it is beyond the
    /// grammar this knows.
    pub fn native_default(&mut self, expr: &str) -> Value {
        self.default_expr(expr.trim()).unwrap_or(Value::Null)
    }

    fn default_expr(&mut self, expr: &str) -> Option<Value> {
        if let Some((l, r)) = split_top(expr, '|') {
            let (l, r) = (self.default_expr(l)?, self.default_expr(r)?);
            return match (l, r) {
                (Value::Int(a), Value::Int(b)) => Some(Value::Int(a | b)),
                _ => None,
            };
        }
        if let Some((l, r)) = split_top(expr, '*') {
            let (l, r) = (self.default_expr(l)?, self.default_expr(r)?);
            return match (l, r) {
                (Value::Int(a), Value::Int(b)) => Some(Value::Int(a.checked_mul(b)?)),
                _ => None,
            };
        }
        match expr {
            "null" => return Some(Value::Null),
            "true" => return Some(Value::Bool(true)),
            "false" => return Some(Value::Bool(false)),
            "[]" => return Some(Value::Array(Array::new())),
            _ => {}
        }
        if let Some(v) = int_literal(expr) {
            return Some(Value::Int(v));
        }
        if expr.bytes().next().is_some_and(|b| b.is_ascii_digit() || b == b'-') {
            return expr.parse::<f64>().ok().map(Value::Float);
        }
        if let Some(inner) = expr
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| expr.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        {
            return Some(Value::string(inner.as_bytes()));
        }
        if let Some((class, member)) = expr.rsplit_once("::") {
            let class = class.trim_start_matches('\\');
            let cid = self.lookup_class(class.as_bytes()).ok()??;
            if member == "class" {
                return Some(Value::string(self.classes[cid as usize].name_str().as_bytes()));
            }
            return self.class_const(cid, member.as_bytes(), None).ok();
        }
        self.constant(expr.trim_start_matches('\\').as_bytes())
    }
}

/// Split `expr` at the last top-level `op` (outside `::`-joined names it
/// is always top level; the stubs use no parentheses).
fn split_top(expr: &str, op: char) -> Option<(&str, &str)> {
    let at = expr.rfind(op)?;
    Some((expr[..at].trim(), expr[at + 1..].trim()))
}

/// A php integer literal as the stubs write one: decimal, `0x`, `0o`/`0`
/// octal, `0b`, an optional leading minus.
fn int_literal(s: &str) -> Option<i64> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let v = if let Some(h) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(b) = digits.strip_prefix("0b").or_else(|| digits.strip_prefix("0B")) {
        i64::from_str_radix(b, 2).ok()?
    } else if let Some(o) = digits.strip_prefix("0o").or_else(|| digits.strip_prefix("0O")) {
        i64::from_str_radix(o, 8).ok()?
    } else if digits.len() > 1 && digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit()) {
        i64::from_str_radix(&digits[1..], 8).ok()?
    } else if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse::<i64>().ok()?
    } else {
        return None;
    };
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_literals() {
        assert_eq!(int_literal("512"), Some(512));
        assert_eq!(int_literal("-1"), Some(-1));
        assert_eq!(int_literal("0666"), Some(0o666));
        assert_eq!(int_literal("0"), Some(0));
        assert_eq!(int_literal("0x1F"), Some(31));
        assert_eq!(int_literal("1.0"), None);
        assert_eq!(int_literal("SORT_REGULAR"), None);
    }

    #[test]
    fn table_is_sorted_for_binary_search() {
        assert!(NATIVE_PARAMS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(params_of("json_decode").is_some());
        assert_eq!(params_of("json_decode").unwrap()[3], ("flags", Some("int"), Some("0")));
        assert_eq!(native_return_type("json_decode"), Some("mixed"));
        assert_eq!(native_return_type("ArrayObject::__construct"), None);
        assert!(params_of("pdo::query").is_some());
        assert!(params_of("no_such_native").is_none());
    }
}
