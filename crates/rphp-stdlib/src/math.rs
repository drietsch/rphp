//! Math builtins. Numeric coercion goes through [`Value::to_number`] so a
//! numeric string behaves the way it does in arithmetic; comparisons (`max`/
//! `min`) reuse the engine's spaceship ordering.
use rphp_value::Value;

use rphp_runtime::{Ctx, NativeFn, NativeResult, nf, Registry, Unwind};

/// The math constants (php-src `ext/standard/math.c` / `basic_functions.c`):
/// `M_*`, `INF`, `NAN`, `PHP_ROUND_*` with the manifest's exact values.
#[allow(clippy::approx_constant)] // M_EULER: `f64::consts::EULER_GAMMA` is unstable
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("M_E", std::f64::consts::E),
        ("M_LOG2E", std::f64::consts::LOG2_E),
        ("M_LOG10E", std::f64::consts::LOG10_E),
        ("M_LN2", std::f64::consts::LN_2),
        ("M_LN10", std::f64::consts::LN_10),
        ("M_PI", std::f64::consts::PI),
        ("M_PI_2", std::f64::consts::FRAC_PI_2),
        ("M_PI_4", std::f64::consts::FRAC_PI_4),
        ("M_1_PI", std::f64::consts::FRAC_1_PI),
        ("M_2_PI", std::f64::consts::FRAC_2_PI),
        ("M_SQRTPI", 1.772_453_850_905_516),
        ("M_2_SQRTPI", std::f64::consts::FRAC_2_SQRT_PI),
        ("M_LNPI", 1.144_729_885_849_400_2),
        ("M_EULER", 0.577_215_664_901_532_9),
        ("M_SQRT2", std::f64::consts::SQRT_2),
        ("M_SQRT3", 1.732_050_807_568_877_2),
        ("M_SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("INF", f64::INFINITY),
        ("NAN", f64::NAN),
    ] {
        r.constant(name, Value::Float(v));
    }
    for (name, v) in [
        ("PHP_ROUND_HALF_UP", 1),
        ("PHP_ROUND_HALF_DOWN", 2),
        ("PHP_ROUND_HALF_EVEN", 3),
        ("PHP_ROUND_HALF_ODD", 4),
    ] {
        r.constant(name, Value::Int(v));
    }
}

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("abs", 1, Some(1), abs),
    nf!("max", 1, None, max),
    nf!("min", 1, None, min),
    nf!("floor", 1, Some(1), floor),
    nf!("ceil", 1, Some(1), ceil),
    nf!("round", 1, Some(3), round),
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
    nf!("log1p", 1, Some(1), log1p),
    nf!("fpow", 2, Some(2), fpow),
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
    nf!("base_convert", 3, Some(3), base_convert),
];

pub(crate) fn abs(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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

pub(crate) fn max(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let values = operands("max", args)?;
    let mut best = values[0].clone();
    for v in &values[1..] {
        if v.gt(&best) {
            best = v.clone();
        }
    }
    Ok(best)
}

pub(crate) fn min(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let values = operands("min", args)?;
    let mut best = values[0].clone();
    for v in &values[1..] {
        if v.lt(&best) {
            best = v.clone();
        }
    }
    Ok(best)
}

pub(crate) fn floor(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Float(args[0].to_float().floor()))
}

pub(crate) fn ceil(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Float(args[0].to_float().ceil()))
}

/// `round(int|float $num, int $precision = 0, int|RoundingMode $mode = RoundingMode::HalfAwayFromZero): float`
/// — php-src `_php_math_round`: the value is scaled by `10^precision`,
/// truncated towards zero, and the rounding decision is taken by comparing
/// the *original* value against the candidate edge in its own scale (so
/// `round(0.285, 2)` is `0.29`, `round(2.675, 2)` is `2.68`). The eight
/// modes are the `PHP_ROUND_*` / `RoundingMode` values 1–8 (the enum
/// object itself lands with plan E6). An int with a non-negative precision
/// is returned as-is.
pub(crate) fn round(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let places = args.get(1).map_or(0, Value::to_int);
    let mode = args.get(2).map_or(ROUND_HALF_UP, Value::to_int);
    if !(ROUND_HALF_UP..=ROUND_AWAY_FROM_ZERO).contains(&mode) {
        return Err(Unwind::value_error(
            "round(): Argument #3 ($mode) must be a valid rounding mode (RoundingMode::*)",
        ));
    }
    let value = match args[0].to_number() {
        Value::Int(i) if places >= 0 => return Ok(Value::Float(i as f64)),
        Value::Int(i) => i as f64,
        Value::Float(f) => f,
        other => other.to_float(),
    };
    let places = places.clamp(i32::MIN as i64 + 1, i32::MAX as i64) as i32;
    Ok(Value::Float(php_math_round(value, places, mode)))
}

const ROUND_HALF_UP: i64 = 1;
const ROUND_HALF_DOWN: i64 = 2;
const ROUND_HALF_EVEN: i64 = 3;
const ROUND_HALF_ODD: i64 = 4;
const ROUND_CEILING: i64 = 5;
const ROUND_FLOOR: i64 = 6;
const ROUND_TOWARD_ZERO: i64 = 7;
const ROUND_AWAY_FROM_ZERO: i64 = 8;

/// `php_intpow10`: exact powers of ten up to 1e22, `pow` beyond.
fn intpow10(power: i32) -> f64 {
    const POWERS: [f64; 23] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15,
        1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
    ];
    if !(0..=22).contains(&power) {
        return 10f64.powi(power);
    }
    POWERS[power as usize]
}

/// php-src `_php_math_round(value, places, mode)`.
fn php_math_round(value: f64, places: i32, mode: i64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    let exponent = intpow10(places.abs());
    let scaled = if places > 0 { value * exponent } else { value / exponent };
    // The integer part in the scaled space, truncated towards zero.
    let integral = if value >= 0.0 { scaled.floor() } else { scaled.ceil() };
    if !integral.is_finite() {
        return value;
    }
    // Candidates measured in the value's own scale, where `value` is exact.
    let unscale = |x: f64| if places > 0 { x / exponent } else { x * exponent };
    let value_abs = value.abs();
    let integral_abs = integral.abs();
    let edge_half = unscale(integral_abs + 0.5);
    let edge_int = unscale(integral_abs);
    let away = match mode {
        ROUND_HALF_UP => value_abs >= edge_half,
        ROUND_HALF_DOWN => value_abs > edge_half,
        ROUND_HALF_EVEN => value_abs > edge_half || (value_abs == edge_half && integral_abs % 2.0 != 0.0),
        ROUND_HALF_ODD => value_abs > edge_half || (value_abs == edge_half && integral_abs % 2.0 == 0.0),
        ROUND_CEILING => value > 0.0 && value_abs > edge_int,
        ROUND_FLOOR => value < 0.0 && value_abs > edge_int,
        ROUND_TOWARD_ZERO => false,
        _ => value_abs > edge_int, // ROUND_AWAY_FROM_ZERO
    };
    let rounded = if away { integral + 1.0f64.copysign(value) } else { integral };
    if places.abs() < 23 {
        unscale(rounded)
    } else {
        // Beyond the exact power-of-ten table php round-trips through the
        // decimal text (`"%15fe%d"` + `strtod`).
        let text = format!("{rounded:.6}e{}", -places);
        match text.parse::<f64>() {
            Ok(f) if f.is_finite() => f,
            _ => value,
        }
    }
}

pub(crate) fn sqrt(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A negative operand yields NAN, as PHP does (no exception).
    Ok(Value::Float(args[0].to_float().sqrt()))
}

pub(crate) fn intdiv(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let a = args[0].to_int();
    let b = args[1].to_int();
    if b == 0 {
        return Err(Unwind::division_by_zero("Division by zero"));
    }
    match a.checked_div(b) {
        Some(q) => Ok(Value::Int(q)),
        // i64::MIN / -1 overflows; PHP raises an ArithmeticError here.
        None => Err(Unwind::arithmetic_error(
            "Division of PHP_INT_MIN by -1 is not an integer",
        )),
    }
}

// ---- helpers ----------------------------------------------------------------

/// The operand list for `max`/`min`: the elements of a lone array argument, or
/// the argument list itself. A single non-array argument is an error, and an
/// empty array has no extreme — both match PHP's messages.
fn operands(func: &str, args: &[Value]) -> Result<Vec<Value>, Unwind> {
    if args.len() == 1 {
        return match &args[0] {
            Value::Array(a) => {
                let values: Vec<Value> = a.iter().map(|(_, v)| v.clone()).collect();
                if values.is_empty() {
                    Err(Unwind::value_error(format!(
                        "{func}(): Argument #1 ($value) must contain at least one element"
                    )))
                } else {
                    Ok(values)
                }
            }
            _ => Err(Unwind::type_error(format!(
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

pub(crate) fn pi(_: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Float(std::f64::consts::PI))
}

pub(crate) fn pow(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // Reuse the engine's `**` semantics: an int base with a non-negative int
    // exponent whose result fits i64 stays Int (pow(2,3) => 8); anything else
    // (negative/float exponent, float operand, overflow) is Float. `to_number`
    // guarantees numeric operands, so `pow` cannot raise a TypeError here.
    let (base, exp) = (args[0].to_number(), args[1].to_number());
    if base.to_float() == 0.0 && exp.to_float() < 0.0 {
        ctx.deprecated("Power of base 0 and negative exponent is deprecated")?;
    }
    match base.pow(&exp) {
        Ok(v) => Ok(v),
        Err(_) => Ok(Value::Float(f64::NAN)),
    }
}

/// `fpow(float $num, float $exponent): float` (8.4) — always the IEEE float
/// power, with no int fast path and no zero-base deprecation.
pub(crate) fn fpow(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(args, f64::powf)
}

pub(crate) fn exp(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::exp)
}

pub(crate) fn log(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
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
                Err(Unwind::value_error(
                    "log(): Argument #2 ($base) must be greater than 0",
                ))
            } else {
                Ok(Value::Float(num.ln() / base.ln()))
            }
        }
    }
}

pub(crate) fn log10(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::log10)
}

pub(crate) fn sin(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::sin)
}

pub(crate) fn cos(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::cos)
}

pub(crate) fn tan(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::tan)
}

pub(crate) fn asin(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::asin)
}

pub(crate) fn acos(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::acos)
}

pub(crate) fn atan(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::atan)
}

pub(crate) fn atan2(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // PHP's argument order is atan2($y, $x); f64::atan2 is self.atan2(other) =
    // y.atan2(x), so args[0] is y and args[1] is x — same order.
    binary(args, f64::atan2)
}

pub(crate) fn sinh(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::sinh)
}

pub(crate) fn cosh(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::cosh)
}

pub(crate) fn tanh(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::tanh)
}

pub(crate) fn asinh(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::asinh)
}

pub(crate) fn acosh(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::acosh)
}

pub(crate) fn atanh(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::atanh)
}

pub(crate) fn deg2rad(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // Matches php-src layout `(num / 180) * M_PI` for bit-identical rounding.
    unary(args, |d| (d / 180.0) * std::f64::consts::PI)
}

pub(crate) fn rad2deg(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, |r| (r * 180.0) / std::f64::consts::PI)
}

pub(crate) fn hypot(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(args, f64::hypot)
}

pub(crate) fn fmod(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // C `fmod`: remainder takes the sign of the dividend (Rust's `%` on f64).
    binary(args, |a, b| a % b)
}

pub(crate) fn fdiv(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // IEEE division: divide-by-zero yields ±INF (or NAN for 0/0), never an error.
    binary(args, |a, b| a / b)
}

pub(crate) fn expm1(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::exp_m1)
}

pub(crate) fn log1p(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    unary(args, f64::ln_1p)
}

// ---- float predicates -------------------------------------------------------

pub(crate) fn is_nan(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_float().is_nan()))
}

pub(crate) fn is_finite(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_float().is_finite()))
}

pub(crate) fn is_infinite(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_float().is_infinite()))
}

// ---- base conversion --------------------------------------------------------

pub(crate) fn dechex(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // PHP prints the unsigned 64-bit pattern, so dechex(-1) == "ffffffffffffffff".
    Ok(Value::string(
        format!("{:x}", args[0].to_int() as u64).as_bytes(),
    ))
}

pub(crate) fn decbin(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::string(
        format!("{:b}", args[0].to_int() as u64).as_bytes(),
    ))
}

pub(crate) fn decoct(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::string(
        format!("{:o}", args[0].to_int() as u64).as_bytes(),
    ))
}

pub(crate) fn hexdec(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    base_to_number(ctx, &args[0].to_php_bytes(), 16)
}

pub(crate) fn bindec(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    base_to_number(ctx, &args[0].to_php_bytes(), 2)
}

pub(crate) fn octdec(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    base_to_number(ctx, &args[0].to_php_bytes(), 8)
}

/// `base_convert(string $num, int $from_base, int $to_base): string`
pub(crate) fn base_convert(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from = args[1].to_int();
    let to = args[2].to_int();
    if !(2..=36).contains(&from) {
        return Err(Unwind::value_error(
            "base_convert(): Argument #2 ($from_base) must be between 2 and 36 (inclusive)",
        ));
    }
    if !(2..=36).contains(&to) {
        return Err(Unwind::value_error(
            "base_convert(): Argument #3 ($to_base) must be between 2 and 36 (inclusive)",
        ));
    }
    let n = base_to_number(ctx, &args[0].to_php_bytes(), from)?;
    Ok(Value::string(number_to_base(&n, to).as_bytes()))
}

/// Parse `s` as a number in `base` (2..=36), mirroring php-src's
/// `_php_math_basetozval`: surrounding whitespace and the matching `0x` /
/// `0o` / `0b` prefix are stripped; characters outside the base are skipped
/// with php's E_DEPRECATED; the running total accumulates in an `i64` until
/// it would exceed `i64::MAX`, then switches to `f64` (so values past the
/// signed-64-bit range come back as floats, e.g. hexdec("ffffffffffffffff")).
fn base_to_number(ctx: &mut Ctx, s: &[u8], base: i64) -> NativeResult {
    let mut start = 0;
    let mut end = s.len();
    while start < end && is_c_space(s[start]) {
        start += 1;
    }
    while end > start && is_c_space(s[end - 1]) {
        end -= 1;
    }
    if end - start >= 2 && s[start] == b'0' {
        let prefix = s[start + 1] | 0x20;
        if (base == 16 && prefix == b'x') || (base == 8 && prefix == b'o') || (base == 2 && prefix == b'b') {
            start += 2;
        }
    }
    let mut num: i64 = 0;
    let mut fnum: f64 = 0.0;
    let mut overflowed = false;
    let mut invalid = false;
    let cutoff = i64::MAX / base;
    let cutlim = i64::MAX % base;
    for &ch in &s[start..end] {
        let digit = match ch {
            b'0'..=b'9' => (ch - b'0') as i64,
            b'A'..=b'Z' => (ch - b'A' + 10) as i64,
            b'a'..=b'z' => (ch - b'a' + 10) as i64,
            _ => {
                invalid = true;
                continue;
            }
        };
        if digit >= base {
            invalid = true;
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
    if invalid {
        ctx.deprecated("Invalid characters passed for attempted conversion, these have been ignored")?;
    }
    Ok(if overflowed { Value::Float(fnum) } else { Value::Int(num) })
}

/// `_php_math_zvaltobase`: an int renders as its unsigned 64-bit pattern; a
/// float is floored and peeled digit by digit with `fmod`, php's imprecision
/// included (at most 64 digits).
fn number_to_base(n: &Value, base: i64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::new();
    match n {
        Value::Float(f) => {
            let mut fvalue = f.floor();
            if fvalue.is_infinite() {
                return String::new();
            }
            loop {
                let i = (fvalue % base as f64) as usize;
                buf.push(DIGITS[i.min(35)]);
                fvalue /= base as f64;
                if buf.len() >= 64 || fvalue.abs() < 1.0 {
                    break;
                }
            }
        }
        other => {
            let mut value = other.to_int() as u64;
            loop {
                buf.push(DIGITS[(value % base as u64) as usize]);
                value /= base as u64;
                if value == 0 {
                    break;
                }
            }
        }
    }
    buf.reverse();
    String::from_utf8(buf).expect("ascii digits")
}

/// C `isspace` in the C locale: space, `\t`, `\n`, `\v`, `\f`, `\r`
/// (Rust's `is_ascii_whitespace` leaves out the vertical tab).
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[cfg(test)]
mod tests {
    use rphp_runtime::ErrorKind;
    use rphp_value::Value;

    use crate::tests::{call_err, call_named};

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    fn round(v: f64, places: i64, mode: i64) -> f64 {
        match call_named(b"round", &[Value::Float(v), Value::Int(places), Value::Int(mode)]) {
            Value::Float(f) => f,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn round_matches_php_pre_rounding_and_modes() {
        assert_eq!(round(0.285, 2, 1), 0.29);
        assert_eq!(round(2.675, 2, 1), 2.68);
        assert_eq!(round(1.005, 2, 1), 1.01);
        assert_eq!(round(5.055, 2, 1), 5.06);
        assert_eq!(round(1234.5678, -2, 1), 1200.0);
        assert_eq!(round(-2.5, 0, 1), -3.0);
        assert_eq!(round(2.5, 0, 2), 2.0);
        assert_eq!(round(2.5, 0, 3), 2.0);
        assert_eq!(round(3.5, 0, 3), 4.0);
        assert_eq!(round(2.5, 0, 4), 3.0);
        assert_eq!(round(-2.5, 0, 3), -2.0);
        assert_eq!(round(5.5, 0, 5), 6.0);
        assert_eq!(round(-5.5, 0, 5), -5.0);
        assert_eq!(round(5.5, 0, 6), 5.0);
        assert_eq!(round(-5.5, 0, 6), -6.0);
        assert_eq!(round(5.5, 0, 7), 5.0);
        assert_eq!(round(1.231, 2, 8), 1.24);
        assert_eq!(round(1.1, 30, 1), 1.1);
        assert_eq!(round(1.5, 400, 1), 1.5);
        assert_eq!(round(1e300, 10, 1), 1e300);
        assert_eq!(round(4503599627370497.0, 0, 1), 4503599627370497.0);
        assert!(round(-0.4, 0, 1).is_sign_negative());
        assert_eq!(round(12345678901234567890.0, -5, 1), 1.23456789012346e19);
        assert_eq!(call_named(b"round", &[Value::Int(15), Value::Int(-1)]), Value::Float(20.0));
        assert_eq!(call_named(b"round", &[Value::Int(5), Value::Int(2)]), Value::Float(5.0));
        assert_eq!(call_err(b"round", &[Value::Float(1.5), Value::Int(0), Value::Int(9)]).kind(), Some(ErrorKind::ValueError));
    }

    #[test]
    fn base_conversion_matches_php() {
        assert_eq!(call_named(b"base_convert", &[s("ff"), Value::Int(16), Value::Int(2)]), s("11111111"));
        assert_eq!(call_named(b"base_convert", &[s(" 0xff "), Value::Int(16), Value::Int(10)]), s("255"));
        assert_eq!(
            call_named(b"base_convert", &[s("ffffffffffffffff"), Value::Int(16), Value::Int(10)]),
            s("18446744073709552046")
        );
        assert_eq!(
            call_named(b"base_convert", &[s("zzzzzzzzzzzzzzzzzzzz"), Value::Int(36), Value::Int(2)]),
            s("0001111111101000000000000000000000000000000000000000000000000000")
        );
        assert_eq!(call_named(b"hexdec", &[s("0x1A")]), Value::Int(26));
        assert_eq!(call_named(b"octdec", &[s("0o17")]), Value::Int(15));
        assert_eq!(call_err(b"base_convert", &[s("1"), Value::Int(1), Value::Int(10)]).kind(), Some(ErrorKind::ValueError));
        assert_eq!(call_named(b"log1p", &[Value::Int(0)]), Value::Float(0.0));
        assert_eq!(call_named(b"fpow", &[Value::Int(2), Value::Int(3)]), Value::Float(8.0));
    }
}
