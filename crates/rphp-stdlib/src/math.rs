//! Math builtins. Numeric coercion goes through [`Value::to_number`] so a
//! numeric string behaves the way it does in arithmetic; comparisons (`max`/
//! `min`) reuse the engine's spaceship ordering.
use rphp_value::Value;

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("abs", 1, Some(1), abs),
    nf!("max", 1, None, max),
    nf!("min", 1, None, min),
    nf!("floor", 1, Some(1), floor),
    nf!("ceil", 1, Some(1), ceil),
    nf!("round", 1, Some(2), round),
    nf!("sqrt", 1, Some(1), sqrt),
    nf!("intdiv", 2, Some(2), intdiv),
    // --- float-returning: powers, logs, trigonometry ---
    nf!("pi", 0, Some(0), pi),
    nf!("pow", 2, Some(2), pow),
    nf!("exp", 1, Some(1), exp),
    nf!("log", 1, Some(2), log),
    nf!("log10", 1, Some(1), log10),
    nf!("sin", 1, Some(1), sin),
    nf!("cos", 1, Some(1), cos),
    nf!("tan", 1, Some(1), tan),
    nf!("asin", 1, Some(1), asin),
    nf!("acos", 1, Some(1), acos),
    nf!("atan", 1, Some(1), atan),
    nf!("atan2", 2, Some(2), atan2),
    nf!("sinh", 1, Some(1), sinh),
    nf!("cosh", 1, Some(1), cosh),
    nf!("tanh", 1, Some(1), tanh),
    nf!("asinh", 1, Some(1), asinh),
    nf!("acosh", 1, Some(1), acosh),
    nf!("atanh", 1, Some(1), atanh),
    nf!("deg2rad", 1, Some(1), deg2rad),
    nf!("rad2deg", 1, Some(1), rad2deg),
    nf!("hypot", 2, Some(2), hypot),
    nf!("fmod", 2, Some(2), fmod),
    nf!("fdiv", 2, Some(2), fdiv),
    nf!("expm1", 1, Some(1), expm1),
    // --- float predicates ---
    nf!("is_nan", 1, Some(1), is_nan),
    nf!("is_finite", 1, Some(1), is_finite),
    nf!("is_infinite", 1, Some(1), is_infinite),
    // --- base conversion ---
    nf!("dechex", 1, Some(1), dechex),
    nf!("decbin", 1, Some(1), decbin),
    nf!("decoct", 1, Some(1), decoct),
    nf!("hexdec", 1, Some(1), hexdec),
    nf!("bindec", 1, Some(1), bindec),
    nf!("octdec", 1, Some(1), octdec),
];

pub(crate) fn abs(_: &mut Ctx, args: &[Value]) -> NativeResult {
    match args[0].to_number() {
        // i64::MIN has no positive counterpart; promote to float as PHP does.
        Value::Int(i) => Ok(i
            .checked_abs()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Float((i as f64).abs()))),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        // `to_number` only ever yields Int or Float.
        other => Ok(other),
    }
}

pub(crate) fn max(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let values = operands("max", args)?;
    let mut best = values[0].clone();
    for v in &values[1..] {
        if v.gt(&best) {
            best = v.clone();
        }
    }
    Ok(best)
}

pub(crate) fn min(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let values = operands("min", args)?;
    let mut best = values[0].clone();
    for v in &values[1..] {
        if v.lt(&best) {
            best = v.clone();
        }
    }
    Ok(best)
}

pub(crate) fn floor(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Float(args[0].to_float().floor()))
}

pub(crate) fn ceil(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Float(args[0].to_float().ceil()))
}

pub(crate) fn round(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let x = args[0].to_float();
    let precision = args.get(1).map_or(0, Value::to_int);
    let factor = 10f64.powi(precision as i32);
    // f64::round is round-half-away-from-zero, matching PHP's default mode.
    Ok(Value::Float((x * factor).round() / factor))
}

pub(crate) fn sqrt(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // A negative operand yields NAN, as PHP does (no exception).
    Ok(Value::Float(args[0].to_float().sqrt()))
}

pub(crate) fn intdiv(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let a = args[0].to_int();
    let b = args[1].to_int();
    if b == 0 {
        return Err(NativeError::new("Division by zero"));
    }
    match a.checked_div(b) {
        Some(q) => Ok(Value::Int(q)),
        // i64::MIN / -1 overflows; PHP raises an ArithmeticError here.
        None => Err(NativeError::new(
            "Division of PHP_INT_MIN by -1 is not an integer",
        )),
    }
}

// ---- helpers ----------------------------------------------------------------

/// The operand list for `max`/`min`: the elements of a lone array argument, or
/// the argument list itself. A single non-array argument is an error, and an
/// empty array has no extreme — both match PHP's messages.
fn operands(func: &str, args: &[Value]) -> Result<Vec<Value>, NativeError> {
    if args.len() == 1 {
        return match &args[0] {
            Value::Array(a) => {
                let values: Vec<Value> = a.iter().map(|(_, v)| v.clone()).collect();
                if values.is_empty() {
                    Err(NativeError::new(format!(
                        "{func}(): Argument #1 ($value) must contain at least one element"
                    )))
                } else {
                    Ok(values)
                }
            }
            _ => Err(NativeError::new(format!(
                "{func}(): When only one argument is passed, it must be of type array"
            ))),
        };
    }
    Ok(args.to_vec())
}

/// Apply a unary `f64 -> f64` (a libm routine) to the first argument and box the
/// result as a PHP float. Domain errors surface as `NAN`/`±INF`, never an
/// exception — matching PHP, which leaves these to the C math library.
fn unary(args: &[Value], f: fn(f64) -> f64) -> NativeResult {
    Ok(Value::Float(f(args[0].to_float())))
}

/// Same as [`unary`] for the two-argument libm routines.
fn binary(args: &[Value], f: fn(f64, f64) -> f64) -> NativeResult {
    Ok(Value::Float(f(args[0].to_float(), args[1].to_float())))
}

// ---- powers, logs, trigonometry --------------------------------------------

pub(crate) fn pi(_: &mut Ctx, _args: &[Value]) -> NativeResult {
    Ok(Value::Float(std::f64::consts::PI))
}

pub(crate) fn pow(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // Reuse the engine's `**` semantics: an int base with a non-negative int
    // exponent whose result fits i64 stays Int (pow(2,3) => 8); anything else
    // (negative/float exponent, float operand, overflow) is Float. `to_number`
    // guarantees numeric operands, so `pow` cannot raise a TypeError here.
    match args[0].to_number().pow(&args[1].to_number()) {
        Ok(v) => Ok(v),
        Err(_) => Ok(Value::Float(f64::NAN)),
    }
}

pub(crate) fn exp(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::exp)
}

pub(crate) fn log(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let num = args[0].to_float();
    match args.get(1) {
        // One argument: natural logarithm.
        None => Ok(Value::Float(num.ln())),
        Some(base) => {
            let base = base.to_float();
            // PHP special-cases 10 and 2 to the dedicated, more accurate
            // routines, treats base 1 as NAN, and rejects non-positive bases.
            if base == 10.0 {
                Ok(Value::Float(num.log10()))
            } else if base == 2.0 {
                Ok(Value::Float(num.log2()))
            } else if base == 1.0 {
                Ok(Value::Float(f64::NAN))
            } else if base <= 0.0 {
                Err(NativeError::new(
                    "log(): Argument #2 ($base) must be greater than 0",
                ))
            } else {
                Ok(Value::Float(num.ln() / base.ln()))
            }
        }
    }
}

pub(crate) fn log10(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::log10)
}

pub(crate) fn sin(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::sin)
}

pub(crate) fn cos(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::cos)
}

pub(crate) fn tan(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::tan)
}

pub(crate) fn asin(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::asin)
}

pub(crate) fn acos(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::acos)
}

pub(crate) fn atan(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::atan)
}

pub(crate) fn atan2(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // PHP's argument order is atan2($y, $x); f64::atan2 is self.atan2(other) =
    // y.atan2(x), so args[0] is y and args[1] is x — same order.
    binary(args, f64::atan2)
}

pub(crate) fn sinh(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::sinh)
}

pub(crate) fn cosh(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::cosh)
}

pub(crate) fn tanh(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::tanh)
}

pub(crate) fn asinh(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::asinh)
}

pub(crate) fn acosh(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::acosh)
}

pub(crate) fn atanh(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::atanh)
}

pub(crate) fn deg2rad(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // Matches php-src layout `(num / 180) * M_PI` for bit-identical rounding.
    unary(args, |d| (d / 180.0) * std::f64::consts::PI)
}

pub(crate) fn rad2deg(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, |r| (r * 180.0) / std::f64::consts::PI)
}

pub(crate) fn hypot(_: &mut Ctx, args: &[Value]) -> NativeResult {
    binary(args, f64::hypot)
}

pub(crate) fn fmod(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // C `fmod`: remainder takes the sign of the dividend (Rust's `%` on f64).
    binary(args, |a, b| a % b)
}

pub(crate) fn fdiv(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // IEEE division: divide-by-zero yields ±INF (or NAN for 0/0), never an error.
    binary(args, |a, b| a / b)
}

pub(crate) fn expm1(_: &mut Ctx, args: &[Value]) -> NativeResult {
    unary(args, f64::exp_m1)
}

// ---- float predicates -------------------------------------------------------

pub(crate) fn is_nan(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_float().is_nan()))
}

pub(crate) fn is_finite(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_float().is_finite()))
}

pub(crate) fn is_infinite(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_float().is_infinite()))
}

// ---- base conversion --------------------------------------------------------

pub(crate) fn dechex(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // PHP prints the unsigned 64-bit pattern, so dechex(-1) == "ffffffffffffffff".
    Ok(Value::string(
        format!("{:x}", args[0].to_int() as u64).as_bytes(),
    ))
}

pub(crate) fn decbin(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::string(
        format!("{:b}", args[0].to_int() as u64).as_bytes(),
    ))
}

pub(crate) fn decoct(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(Value::string(
        format!("{:o}", args[0].to_int() as u64).as_bytes(),
    ))
}

pub(crate) fn hexdec(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(base_to_number(&args[0].to_php_bytes(), 16))
}

pub(crate) fn bindec(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(base_to_number(&args[0].to_php_bytes(), 2))
}

pub(crate) fn octdec(_: &mut Ctx, args: &[Value]) -> NativeResult {
    Ok(base_to_number(&args[0].to_php_bytes(), 8))
}

/// Parse `s` as a number in `base` (2/8/16), mirroring php-src's
/// `_php_math_basetozval`: characters outside the base are silently skipped, the
/// running total accumulates in an `i64` until it would exceed `i64::MAX`, then
/// switches to `f64` (so values past the signed-64-bit range come back as
/// floats, e.g. hexdec("ffffffffffffffff")). PHP additionally emits an
/// E_DEPRECATED on invalid characters; we skip them quietly (see caveats).
fn base_to_number(s: &[u8], base: i64) -> Value {
    let mut num: i64 = 0;
    let mut fnum: f64 = 0.0;
    let mut overflowed = false;
    let cutoff = i64::MAX / base;
    let cutlim = i64::MAX % base;
    for &ch in s {
        let digit = match ch {
            b'0'..=b'9' => (ch - b'0') as i64,
            b'A'..=b'Z' => (ch - b'A' + 10) as i64,
            b'a'..=b'z' => (ch - b'a' + 10) as i64,
            _ => continue,
        };
        if digit >= base {
            continue;
        }
        if overflowed {
            fnum = fnum * base as f64 + digit as f64;
        } else if num < cutoff || (num == cutoff && digit <= cutlim) {
            num = num * base + digit;
        } else {
            // First digit that would overflow i64: replay in float from here on.
            overflowed = true;
            fnum = num as f64 * base as f64 + digit as f64;
        }
    }
    if overflowed {
        Value::Float(fnum)
    } else {
        Value::Int(num)
    }
}
