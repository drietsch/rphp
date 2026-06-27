//! The runtime value.
//!
//! **M0 scope:** scalars only — `Null`, `Bool`, `Int` (full i64), `Float` (f64).
//! The target representation (per `specs/base/02-value-model.md`) is a 16-byte
//! `repr(C)` tagged cell with a union payload and reserved heap tags
//! (Str/Array/Object/Closure/Reference). This slice uses a safe Rust enum and
//! adds the heap tags later with `rphp-gc`/`rphp-heap`; the *operations* here
//! (arithmetic, comparison, casts) are the single source of truth that the
//! interpreter and, later, both JIT tiers and const-folding must agree with.
#![forbid(unsafe_code)]

use std::fmt;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
}

/// Recoverable value-level errors that surface as PHP `Error`s at runtime.
#[derive(Clone, PartialEq, Debug)]
pub enum ValueError {
    DivisionByZero,
    ModuloByZero,
    /// Unsupported operand types (the M0 slice is scalar-only).
    TypeError(&'static str),
}

pub type VResult = Result<Value, ValueError>;

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
        }
    }

    // ----- casts (8.x semantics for scalars) -----

    pub fn to_bool(&self) -> bool {
        match *self {
            Value::Null => false,
            Value::Bool(b) => b,
            Value::Int(i) => i != 0,
            Value::Float(f) => f != 0.0,
        }
    }

    pub fn to_int(&self) -> i64 {
        match *self {
            Value::Null => 0,
            Value::Bool(b) => b as i64,
            Value::Int(i) => i,
            // PHP truncates toward zero; NaN/Inf -> 0.
            Value::Float(f) => {
                if f.is_finite() {
                    f.trunc() as i64
                } else {
                    0
                }
            }
        }
    }

    pub fn to_float(&self) -> f64 {
        match *self {
            Value::Null => 0.0,
            Value::Bool(b) => b as i64 as f64,
            Value::Int(i) => i as f64,
            Value::Float(f) => f,
        }
    }

    /// PHP `echo`/string-cast form for scalars. Float formatting approximates
    /// PHP's default `precision=14` (a documented divergence axis, ADR-008).
    pub fn to_php_string(&self) -> String {
        match *self {
            Value::Null => String::new(),
            Value::Bool(true) => "1".to_string(),
            Value::Bool(false) => String::new(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_php_float(f),
        }
    }

    // ----- arithmetic (numeric scalars; int overflow promotes to float) -----

    fn as_number(&self) -> Result<Value, ValueError> {
        match self {
            Value::Int(_) | Value::Float(_) => Ok(*self),
            Value::Bool(b) => Ok(Value::Int(*b as i64)),
            Value::Null => Ok(Value::Int(0)),
        }
    }

    pub fn add(&self, rhs: &Value) -> VResult {
        numeric_binop(self, rhs, |a, b| a.checked_add(b).map(Value::Int).unwrap_or(Value::Float(a as f64 + b as f64)), |a, b| Value::Float(a + b))
    }

    pub fn sub(&self, rhs: &Value) -> VResult {
        numeric_binop(self, rhs, |a, b| a.checked_sub(b).map(Value::Int).unwrap_or(Value::Float(a as f64 - b as f64)), |a, b| Value::Float(a - b))
    }

    pub fn mul(&self, rhs: &Value) -> VResult {
        numeric_binop(self, rhs, |a, b| a.checked_mul(b).map(Value::Int).unwrap_or(Value::Float(a as f64 * b as f64)), |a, b| Value::Float(a * b))
    }

    pub fn div(&self, rhs: &Value) -> VResult {
        let (a, b) = (self.as_number()?, rhs.as_number()?);
        // PHP: division always considers float unless both ints divide evenly.
        if let (Value::Int(x), Value::Int(y)) = (a, b) {
            if y == 0 {
                return Err(ValueError::DivisionByZero);
            }
            if x % y == 0 {
                return Ok(Value::Int(x / y));
            }
            return Ok(Value::Float(x as f64 / y as f64));
        }
        let (x, y) = (a.to_float(), b.to_float());
        if y == 0.0 {
            return Err(ValueError::DivisionByZero);
        }
        Ok(Value::Float(x / y))
    }

    pub fn rem(&self, rhs: &Value) -> VResult {
        // PHP `%` operates on ints.
        let (a, b) = (self.to_int(), rhs.to_int());
        if b == 0 {
            return Err(ValueError::ModuloByZero);
        }
        Ok(Value::Int(a.wrapping_rem(b)))
    }

    pub fn pow(&self, rhs: &Value) -> VResult {
        let (a, b) = (self.as_number()?, rhs.as_number()?);
        if let (Value::Int(x), Value::Int(y)) = (a, b) {
            if y >= 0 && y <= u32::MAX as i64 {
                if let Some(r) = x.checked_pow(y as u32) {
                    return Ok(Value::Int(r));
                }
            }
        }
        Ok(Value::Float(a.to_float().powf(b.to_float())))
    }

    pub fn neg(&self) -> VResult {
        match self.as_number()? {
            Value::Int(i) => Ok(i.checked_neg().map(Value::Int).unwrap_or(Value::Float(-(i as f64)))),
            Value::Float(f) => Ok(Value::Float(-f)),
            _ => unreachable!(),
        }
    }

    pub fn not(&self) -> Value {
        Value::Bool(!self.to_bool())
    }

    // ----- comparison (PHP 8 scalar semantics) -----

    /// Loose `==`.
    pub fn loose_eq(&self, rhs: &Value) -> bool {
        use Value::*;
        match (self, rhs) {
            (Null, Null) => true,
            (Bool(_), _) | (_, Bool(_)) => self.to_bool() == rhs.to_bool(),
            (Null, _) | (_, Null) => self.to_bool() == rhs.to_bool(),
            (Int(a), Int(b)) => a == b,
            // any float involved -> compare as floats
            _ => self.to_float() == rhs.to_float(),
        }
    }

    /// Strict `===` (same type and value).
    pub fn identical(&self, rhs: &Value) -> bool {
        use Value::*;
        match (self, rhs) {
            (Null, Null) => true,
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            _ => false,
        }
    }

    /// `<=>` spaceship for numeric scalars: -1, 0, 1.
    pub fn spaceship(&self, rhs: &Value) -> i64 {
        use std::cmp::Ordering;
        if self.identical(rhs) {
            return 0;
        }
        let (a, b) = (self.to_float(), rhs.to_float());
        match a.partial_cmp(&b) {
            Some(Ordering::Less) => -1,
            Some(Ordering::Greater) => 1,
            _ => 0,
        }
    }

    pub fn lt(&self, rhs: &Value) -> bool { self.spaceship(rhs) < 0 }
    pub fn le(&self, rhs: &Value) -> bool { self.spaceship(rhs) <= 0 }
    pub fn gt(&self, rhs: &Value) -> bool { self.spaceship(rhs) > 0 }
    pub fn ge(&self, rhs: &Value) -> bool { self.spaceship(rhs) >= 0 }
}

fn numeric_binop(
    lhs: &Value,
    rhs: &Value,
    on_int: impl Fn(i64, i64) -> Value,
    on_float: impl Fn(f64, f64) -> Value,
) -> VResult {
    let (a, b) = (lhs.as_number()?, rhs.as_number()?);
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(on_int(x, y)),
        _ => Ok(on_float(a.to_float(), b.to_float())),
    }
}

/// PHP's default float-to-string: a `%.14G`-style format matching the default
/// `precision=14` ini setting that `echo`/string-cast use. Fixed notation for
/// exponents in `-4 ..= 13`, otherwise scientific with an uppercase `E`, a
/// signed exponent, and a forced `.0` mantissa. (`serialize_precision=-1`
/// shortest-round-trip, used by `var_export`/`json_encode`, is a separate path
/// added later.) Verified against stock PHP 8.5 by the differential suite.
fn format_php_float(f: f64) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-INF".to_string() } else { "INF".to_string() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.to_string();
    }

    const PREC: i32 = 14;
    let neg = f < 0.0;

    // Canonical 14-significant-digit scientific form: "d.ddddddddddddde{E}".
    // Rust's LowerExp yields one leading digit, lowercase `e`, no `+`, and no
    // leading zeros in the exponent — exactly what we need to re-lay-out.
    let sci = format!("{:.*e}", (PREC - 1) as usize, f.abs());
    let (mant, exp) = sci.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let digits: String = mant.chars().filter(|&c| c != '.').collect(); // PREC digits

    let mut out = String::new();
    if neg {
        out.push('-');
    }

    if e < -4 || e >= PREC {
        // Scientific.
        out.push_str(&digits[0..1]);
        out.push('.');
        let frac = digits[1..].trim_end_matches('0');
        out.push_str(if frac.is_empty() { "0" } else { frac });
        out.push('E');
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.unsigned_abs().to_string());
    } else if e >= 0 {
        // Fixed, magnitude >= 1.
        let intlen = (e + 1) as usize;
        if intlen >= digits.len() {
            out.push_str(&digits);
            out.push_str(&"0".repeat(intlen - digits.len()));
        } else {
            out.push_str(&digits[..intlen]);
            let frac = digits[intlen..].trim_end_matches('0');
            if !frac.is_empty() {
                out.push('.');
                out.push_str(frac);
            }
        }
    } else {
        // Fixed, magnitude < 1: "0.00…digits".
        let lead_zeros = (-e - 1) as usize;
        let frac_full = format!("{}{}", "0".repeat(lead_zeros), digits);
        let frac = frac_full.trim_end_matches('0');
        out.push_str("0.");
        out.push_str(if frac.is_empty() { "0" } else { frac });
    }
    out
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_php_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_overflow_promotes_to_float() {
        let r = Value::Int(i64::MAX).add(&Value::Int(1)).unwrap();
        assert!(matches!(r, Value::Float(_)));
    }

    #[test]
    fn div_even_is_int_else_float() {
        assert_eq!(Value::Int(6).div(&Value::Int(3)).unwrap(), Value::Int(2));
        assert_eq!(Value::Int(7).div(&Value::Int(2)).unwrap(), Value::Float(3.5));
        assert_eq!(Value::Int(1).div(&Value::Int(0)), Err(ValueError::DivisionByZero));
    }

    #[test]
    fn php8_bool_vs_int_compare() {
        // 0 == false  (true) ; 1 == true (true)
        assert!(Value::Int(0).loose_eq(&Value::Bool(false)));
        assert!(Value::Int(1).loose_eq(&Value::Bool(true)));
        assert!(!Value::Int(2).identical(&Value::Float(2.0)));
    }

    #[test]
    fn spaceship_and_string() {
        assert_eq!(Value::Int(1).spaceship(&Value::Int(2)), -1);
        assert_eq!(Value::Float(2.0).to_php_string(), "2");
        assert_eq!(Value::Float(3.5).to_php_string(), "3.5");
        assert_eq!(Value::Bool(true).to_php_string(), "1");
        assert_eq!(Value::Bool(false).to_php_string(), "");
        assert_eq!(Value::Null.to_php_string(), "");
    }

    #[test]
    fn float_formatting_matches_php() {
        let f = |x: f64| Value::Float(x).to_php_string();
        assert_eq!(f(5.0), "5");
        assert_eq!(f(3.5), "3.5");
        assert_eq!(f(0.1), "0.1");
        assert_eq!(f(1.0 / 3.0), "0.33333333333333");
        assert_eq!(f(100.0), "100");
        assert_eq!(f(1234567890.5), "1234567890.5");
        assert_eq!(f(1e14), "1.0E+14");
        assert_eq!(f(1e20), "1.0E+20");
        assert_eq!(f(1e-5), "1.0E-5");
        // i64::MAX + 1 promoted to float, as PHP prints it.
        assert_eq!(f(9223372036854775808.0), "9.2233720368548E+18");
        assert_eq!(f(-3.5), "-3.5");
    }

    #[test]
    fn pow_and_mod() {
        assert_eq!(Value::Int(2).pow(&Value::Int(10)).unwrap(), Value::Int(1024));
        assert_eq!(Value::Int(7).rem(&Value::Int(3)).unwrap(), Value::Int(1));
        assert_eq!(Value::Int(1).rem(&Value::Int(0)), Err(ValueError::ModuloByZero));
    }
}
