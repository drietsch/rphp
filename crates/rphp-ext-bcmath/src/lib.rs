//! `ext/bcmath`: arbitrary-precision decimal arithmetic — the `bc*`
//! functions and php 8.4's `BcMath\Number`.
//!
//! The numbers are libbcmath's `bc_num` ([`num::BcNum`]): decimal digits
//! with a sign, an integer length and a scale, and every operation keeps
//! libbcmath's rules for the scale of its result and for the sign of a
//! zero. Results are *truncated* to the requested scale, never rounded
//! (`bcround()` is the one that rounds). The default scale is the
//! `bcmath.scale` ini directive, which `bcscale()` reads and writes.
#![forbid(unsafe_code)]

mod limbs;
mod num;
mod number;

use rphp_runtime::{nf, Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, Str, Value};

use crate::num::BcNum;

/// php's `INT_MAX`, the largest scale.
pub(crate) const INT_MAX: i64 = i32::MAX as i64;

/// Register the extension.
pub fn register(r: &mut Registry) {
    r.extension("bcmath");
    r.interp().ini.register("bcmath.scale", "0");
    r.functions(&[
        nf!("bcadd", 2, Some(3), bcadd),
        nf!("bcsub", 2, Some(3), bcsub),
        nf!("bcmul", 2, Some(3), bcmul),
        nf!("bcdiv", 2, Some(3), bcdiv),
        nf!("bcmod", 2, Some(3), bcmod),
        nf!("bcdivmod", 2, Some(3), bcdivmod),
        nf!("bcpowmod", 3, Some(4), bcpowmod),
        nf!("bcpow", 2, Some(3), bcpow),
        nf!("bcsqrt", 1, Some(2), bcsqrt),
        nf!("bccomp", 2, Some(3), bccomp),
        nf!("bcscale", 0, Some(1), bcscale),
        nf!("bcfloor", 1, Some(1), bcfloor),
        nf!("bcceil", 1, Some(1), bcceil),
        nf!("bcround", 1, Some(3), bcround),
    ]);
    number::register(r);
}

/// `BCG(bc_precision)`: the `bcmath.scale` directive as php's
/// `OnUpdateScale` accepts it (a quantity from 0 to `INT_MAX`).
fn default_scale(ctx: &Ctx) -> usize {
    let v = ctx.ini_get("bcmath.scale").unwrap_or("0").trim();
    let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
    match digits.parse::<i64>() {
        Ok(n) if !v.starts_with('-') && n <= INT_MAX => n as usize,
        _ => 0,
    }
}

/// php's `bcmath_check_scale`: `must be between 0 and 2147483647`.
pub(crate) fn check_scale(v: i64, func: &str, argno: usize, pname: &str) -> Result<usize, Unwind> {
    if !(0..=INT_MAX).contains(&v) {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #{argno} (${pname}) must be between 0 and {INT_MAX}"
        )));
    }
    Ok(v as usize)
}

/// The optional `?int $scale` argument at `idx`: the default scale when
/// absent or null.
fn scale_arg(ctx: &Ctx, args: &[Value], idx: usize, func: &str) -> Result<usize, Unwind> {
    match args.get(idx).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => Ok(default_scale(ctx)),
        Some(v) => check_scale(v.to_int(), func, idx + 1, "scale"),
    }
}

/// A numeric-string argument, or `… is not well-formed`.
fn num_arg(v: &Value, func: &str, argno: usize, pname: &str) -> Result<BcNum, Unwind> {
    BcNum::parse(&v.to_php_bytes(), 0, true).map(|(n, _)| n).ok_or_else(|| not_well_formed(func, argno, pname))
}

pub(crate) fn not_well_formed(func: &str, argno: usize, pname: &str) -> Unwind {
    Unwind::value_error(format!("{func}(): Argument #{argno} (${pname}) is not well-formed"))
}

fn str_value(n: &BcNum, scale: usize) -> Value {
    Value::Str(Str::from_vec(n.to_str(scale)))
}

/// The shared shape of `bcadd`/`bcsub`/`bcmul`/`bcdiv`/`bcmod`: scale
/// first, then the two operands.
fn binary(
    ctx: &mut Ctx,
    args: &mut [Value],
    func: &str,
    op: impl FnOnce(&BcNum, &BcNum, usize) -> Result<BcNum, Unwind>,
) -> NativeResult {
    let scale = scale_arg(ctx, args, 2, func)?;
    let a = num_arg(&args[0], func, 1, "num1")?;
    let b = num_arg(&args[1], func, 2, "num2")?;
    Ok(str_value(&op(&a, &b, scale)?, scale))
}

/// `bcadd(string $num1, string $num2, ?int $scale = null): string`
fn bcadd(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(ctx, args, "bcadd", |a, b, s| Ok(num::add(a, b, s)))
}

/// `bcsub(string $num1, string $num2, ?int $scale = null): string`
fn bcsub(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(ctx, args, "bcsub", |a, b, s| Ok(num::sub(a, b, s)))
}

/// `bcmul(string $num1, string $num2, ?int $scale = null): string`
fn bcmul(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(ctx, args, "bcmul", |a, b, s| Ok(num::multiply(a, b, s)))
}

/// `bcdiv(string $num1, string $num2, ?int $scale = null): string`
fn bcdiv(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(ctx, args, "bcdiv", |a, b, s| {
        num::divide(a, b, s).ok_or_else(|| Unwind::division_by_zero("Division by zero"))
    })
}

/// `bcmod(string $num1, string $num2, ?int $scale = null): string`
fn bcmod(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    binary(ctx, args, "bcmod", |a, b, s| {
        num::modulo(a, b, s).ok_or_else(|| Unwind::division_by_zero("Modulo by zero"))
    })
}

/// `bcdivmod(string $num1, string $num2, ?int $scale = null): array` —
/// `[quotient, remainder]`, the quotient an integer.
fn bcdivmod(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let scale = scale_arg(ctx, args, 2, "bcdivmod")?;
    let a = num_arg(&args[0], "bcdivmod", 1, "num1")?;
    let b = num_arg(&args[1], "bcdivmod", 2, "num2")?;
    let (q, r) = num::divmod(&a, &b, scale).ok_or_else(|| Unwind::division_by_zero("Division by zero"))?;
    let mut out = Array::new();
    out.push(str_value(&q, 0));
    out.push(str_value(&r, scale));
    Ok(Value::Array(out))
}

/// `bcpowmod(string $num, string $exponent, string $modulus, ?int $scale = null): string`
fn bcpowmod(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "bcpowmod";
    let scale = scale_arg(ctx, args, 3, F)?;
    let base = num_arg(&args[0], F, 1, "num")?;
    let expo = num_arg(&args[1], F, 2, "exponent")?;
    let modulus = num_arg(&args[2], F, 3, "modulus")?;
    match num::raisemod(&base, &expo, &modulus, scale) {
        Ok(r) => Ok(str_value(&r, scale)),
        Err(e) => Err(raisemod_error(e, F, |n| ["num", "exponent", "modulus"][n - 1], 0)),
    }
}

/// The errors of `bc_raisemod`, for the function (argument numbers from 1)
/// and for `Number::powmod()` (`shift` = 1: the base is `$this`).
pub(crate) fn raisemod_error(
    e: num::RaiseModError,
    func: &str,
    pname: impl Fn(usize) -> &'static str,
    shift: usize,
) -> Unwind {
    use num::RaiseModError::*;
    let arg = |n: usize, what: &str| {
        let n = n - shift;
        Unwind::value_error(format!("{func}(): Argument #{n} (${}) {what}", pname(n)))
    };
    match e {
        BaseHasFractional if shift == 1 => Unwind::value_error("Base number cannot have a fractional part"),
        BaseHasFractional => arg(1, "cannot have a fractional part"),
        ExpoHasFractional => arg(2, "cannot have a fractional part"),
        ExpoIsNegative => arg(2, "must be greater than or equal to 0"),
        ModHasFractional => arg(3, "cannot have a fractional part"),
        ModIsZero => Unwind::division_by_zero("Modulo by zero"),
    }
}

/// `bcpow(string $num, string $exponent, ?int $scale = null): string`
fn bcpow(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "bcpow";
    let scale = scale_arg(ctx, args, 2, F)?;
    let base = num_arg(&args[0], F, 1, "num")?;
    let expo = num_arg(&args[1], F, 2, "exponent")?;
    if expo.scale != 0 {
        return Err(Unwind::value_error(format!("{F}(): Argument #2 ($exponent) cannot have a fractional part")));
    }
    let e = expo.to_long();
    if e == 0 && !expo.is_zero() {
        return Err(Unwind::value_error(format!("{F}(): Argument #2 ($exponent) is too large")));
    }
    let r = num::raise(&base, e, scale).map_err(|e| raise_error(e, Some((F, 2, "exponent"))))?;
    Ok(str_value(&r, scale))
}

/// php's `bc_pow_err`: `arg` names the argument, `None` for an operator.
pub(crate) fn raise_error(e: num::RaiseError, arg: Option<(&str, usize, &str)>) -> Unwind {
    match e {
        num::RaiseError::DivideByZero => Unwind::division_by_zero("Negative power of zero"),
        num::RaiseError::Overflow => match arg {
            None => Unwind::value_error("exponent is too large, the number of digits overflowed"),
            Some((func, n, pname)) => Unwind::value_error(format!(
                "{func}(): Argument #{n} (${pname}) exponent is too large, the number of digits overflowed"
            )),
        },
    }
}

/// `bcsqrt(string $num, ?int $scale = null): string`
fn bcsqrt(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let scale = scale_arg(ctx, args, 1, "bcsqrt")?;
    let n = num_arg(&args[0], "bcsqrt", 1, "num")?;
    match num::sqrt(&n, scale) {
        Some(r) => Ok(str_value(&r, scale)),
        None => Err(Unwind::value_error("bcsqrt(): Argument #1 ($num) must be greater than or equal to 0")),
    }
}

/// `bccomp(string $num1, string $num2, ?int $scale = null): int` — both
/// operands are cut to the scale while they are read.
fn bccomp(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let scale = scale_arg(ctx, args, 2, "bccomp")?;
    let a = BcNum::parse(&args[0].to_php_bytes(), scale, false).ok_or_else(|| not_well_formed("bccomp", 1, "num1"))?.0;
    let b = BcNum::parse(&args[1].to_php_bytes(), scale, false).ok_or_else(|| not_well_formed("bccomp", 2, "num2"))?.0;
    Ok(Value::Int(num::compare(&a, &b, scale)))
}

/// `bcscale(?int $scale = null): int` — the previous default scale; a
/// new one is stored in `bcmath.scale`.
fn bcscale(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let old = default_scale(ctx);
    if let Some(v) = args.first().map(|v| v.deref().into_owned()) {
        if !matches!(v, Value::Null) {
            let s = check_scale(v.to_int(), "bcscale", 1, "scale")?;
            ctx.ini_set("bcmath.scale", &s.to_string());
        }
    }
    Ok(Value::Int(old as i64))
}

/// `bcfloor(string $num): string`
fn bcfloor(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let n = num_arg(&args[0], "bcfloor", 1, "num")?;
    Ok(str_value(&num::floor_or_ceil(&n, true), 0))
}

/// `bcceil(string $num): string`
fn bcceil(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let n = num_arg(&args[0], "bcceil", 1, "num")?;
    Ok(str_value(&num::floor_or_ceil(&n, false), 0))
}

/// `bcround(string $num, int $precision = 0, RoundingMode $mode = RoundingMode::HalfAwayFromZero): string`
fn bcround(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let precision = args.get(1).map_or(0, |v| v.deref().to_int());
    let mode = rounding_mode(ctx, args.get(2), "bcround", 3)?;
    check_precision(precision, "bcround", 2)?;
    let n = num_arg(&args[0], "bcround", 1, "num")?;
    let (r, scale) = num::round(&n, precision, mode);
    Ok(str_value(&r, scale))
}

/// php's `bcmath_check_precision`.
pub(crate) fn check_precision(p: i64, func: &str, argno: usize) -> Result<(), Unwind> {
    if p > INT_MAX {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #{argno} ($precision) must be between {} and {INT_MAX}",
            i64::MIN
        )));
    }
    Ok(())
}

/// A `RoundingMode $mode` argument as a `PHP_ROUND_*` mode
/// (`php_math_round_mode_from_enum`); absent, `HalfAwayFromZero`.
pub(crate) fn rounding_mode(ctx: &Ctx, v: Option<&Value>, func: &str, argno: usize) -> Result<i64, Unwind> {
    let Some(v) = v else {
        return Ok(num::ROUND_HALF_UP);
    };
    let v = v.deref().into_owned();
    if let Value::Object(o) = &v {
        if ctx.class_name_of(o) == "RoundingMode" {
            let name = o.get(b"name").map(|n| n.to_php_bytes()).unwrap_or_default();
            return Ok(match name.as_slice() {
                b"HalfAwayFromZero" => num::ROUND_HALF_UP,
                b"HalfTowardsZero" => num::ROUND_HALF_DOWN,
                b"HalfEven" => num::ROUND_HALF_EVEN,
                b"HalfOdd" => num::ROUND_HALF_ODD,
                b"TowardsZero" => num::ROUND_TOWARD_ZERO,
                b"AwayFromZero" => num::ROUND_AWAY_FROM_ZERO,
                b"NegativeInfinity" => num::ROUND_FLOOR,
                _ => num::ROUND_CEILING,
            });
        }
    }
    Err(Unwind::type_error(format!(
        "{func}(): Argument #{argno} ($mode) must be of type RoundingMode, {} given",
        rphp_runtime::value_name(&v)
    )))
}
