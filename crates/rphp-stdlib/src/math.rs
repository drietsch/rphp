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
