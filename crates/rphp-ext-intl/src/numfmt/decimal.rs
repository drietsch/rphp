//! The decimal quantity ICU formats: a sign, a digit string and a scale
//! (`value = digits × 10^scale`), built from an integer exactly or from a
//! double through its shortest round-tripping decimal form (what ICU's
//! `double-conversion` gives `DecimalFormat`), then rounded on decimal
//! digits under one of ICU's rounding modes.

/// `NumberFormatter::ROUND_*`.
pub const ROUND_CEILING: i64 = 0;
pub const ROUND_FLOOR: i64 = 1;
pub const ROUND_DOWN: i64 = 2;
pub const ROUND_UP: i64 = 3;
pub const ROUND_HALFEVEN: i64 = 4;
pub const ROUND_HALFDOWN: i64 = 5;
pub const ROUND_HALFUP: i64 = 6;
pub const ROUND_HALFODD: i64 = 8;

#[derive(Clone, Debug, Default)]
pub struct Decimal {
    pub negative: bool,
    /// Most significant first, no leading zeros (empty for zero).
    pub digits: Vec<u8>,
    /// `value = digits × 10^scale`.
    pub scale: i32,
}

impl Decimal {
    pub fn zero() -> Self {
        Decimal { negative: false, digits: Vec::new(), scale: 0 }
    }

    pub fn from_i64(v: i64) -> Self {
        let negative = v < 0;
        let mut u = v.unsigned_abs();
        let mut digits = Vec::new();
        while u > 0 {
            digits.push((u % 10) as u8);
            u /= 10;
        }
        digits.reverse();
        Decimal { negative, digits, scale: 0 }
    }

    /// The shortest decimal that reads back as `v` (ICU's conversion of a
    /// double), `None` for NaN and the infinities.
    pub fn from_f64(v: f64) -> Option<Self> {
        if !v.is_finite() {
            return None;
        }
        let negative = v.is_sign_negative();
        let s = format!("{:e}", v.abs());
        let (mantissa, exp) = s.split_once('e')?;
        let exp: i32 = exp.parse().ok()?;
        let mut digits: Vec<u8> = Vec::new();
        let mut frac_len = 0i32;
        let mut after_point = false;
        for c in mantissa.chars() {
            if c == '.' {
                after_point = true;
                continue;
            }
            if let Some(d) = c.to_digit(10) {
                digits.push(d as u8);
                if after_point {
                    frac_len += 1;
                }
            }
        }
        let mut d = Decimal { negative, digits, scale: exp - frac_len };
        d.normalize();
        Some(d)
    }

    /// Drop leading zeros and trailing zeros (into the scale).
    pub fn normalize(&mut self) {
        while self.digits.first() == Some(&0) {
            self.digits.remove(0);
        }
        while self.digits.last() == Some(&0) {
            self.digits.pop();
            self.scale += 1;
        }
        if self.digits.is_empty() {
            self.scale = 0;
        }
    }

    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// The position (power of ten) of the most significant digit.
    pub fn magnitude(&self) -> i32 {
        self.digits.len() as i32 - 1 + self.scale
    }

    /// Multiply by a power of ten.
    pub fn shift(&mut self, by: i32) {
        if !self.is_zero() {
            self.scale = self.scale.saturating_add(by);
        }
    }

    /// The digit at power-of-ten position `pos` (0 for the units).
    pub fn digit_at(&self, pos: i32) -> u8 {
        let idx = self.magnitude() - pos;
        if idx < 0 {
            return 0;
        }
        self.digits.get(idx as usize).copied().unwrap_or(0)
    }

    /// Round so that no digit sits below position `pos` (`-2` keeps two
    /// fraction digits), under `mode`.
    pub fn round_at(&mut self, pos: i32, mode: i64) {
        if self.is_zero() || self.scale >= pos {
            self.normalize();
            return;
        }
        // digits to drop: those at positions < pos
        let keep = (self.magnitude() - pos + 1).max(0) as usize;
        let dropped = &self.digits[keep.min(self.digits.len())..];
        let first = dropped.first().copied().unwrap_or(0);
        let rest_nonzero = dropped.iter().skip(1).any(|d| *d != 0);
        let exactly_half = first == 5 && !rest_nonzero;
        let more_than_half = first > 5 || (first == 5 && rest_nonzero);
        let any_dropped = first != 0 || rest_nonzero;
        let last_kept = if keep == 0 { 0 } else { self.digits[keep - 1] };
        let round_up = match mode {
            ROUND_CEILING => any_dropped && !self.negative,
            ROUND_FLOOR => any_dropped && self.negative,
            ROUND_DOWN => false,
            ROUND_UP => any_dropped,
            ROUND_HALFDOWN => more_than_half,
            ROUND_HALFUP => more_than_half || exactly_half,
            ROUND_HALFODD => more_than_half || (exactly_half && last_kept % 2 == 0),
            _ => more_than_half || (exactly_half && last_kept % 2 == 1),
        };
        self.digits.truncate(keep);
        self.scale = pos;
        if round_up {
            let mut i = self.digits.len();
            loop {
                if i == 0 {
                    self.digits.insert(0, 1);
                    break;
                }
                i -= 1;
                if self.digits[i] == 9 {
                    self.digits[i] = 0;
                } else {
                    self.digits[i] += 1;
                    break;
                }
            }
        }
        self.normalize();
        if self.is_zero() {
            // keep the sign of a negative zero result out of the digits
            self.scale = 0;
        }
    }

    /// Round to `n` significant digits.
    pub fn round_sig(&mut self, n: usize, mode: i64) {
        if self.is_zero() || n == 0 {
            return;
        }
        let pos = self.magnitude() - n as i32 + 1;
        self.round_at(pos, mode);
    }

    /// Round to a multiple of `inc`.
    pub fn round_to(&mut self, inc: &Decimal, mode: i64) {
        if inc.is_zero() || self.is_zero() {
            return;
        }
        // both as integers at the finer scale
        let scale = self.scale.min(inc.scale);
        let Some(v) = self.as_u128_at(scale) else {
            return;
        };
        let Some(i) = inc.as_u128_at(scale) else {
            return;
        };
        let q = v / i;
        let r = v % i;
        let twice = r.checked_mul(2);
        let round_up = match mode {
            ROUND_CEILING => r != 0 && !self.negative,
            ROUND_FLOOR => r != 0 && self.negative,
            ROUND_DOWN => false,
            ROUND_UP => r != 0,
            ROUND_HALFDOWN => twice.is_some_and(|t| t > i),
            ROUND_HALFUP => twice.is_some_and(|t| t >= i),
            ROUND_HALFODD => twice.is_some_and(|t| t > i || (t == i && q % 2 == 0)),
            _ => twice.is_some_and(|t| t > i || (t == i && q % 2 == 1)),
        };
        let result = (q + u128::from(round_up)) * i;
        let mut digits = Vec::new();
        let mut x = result;
        while x > 0 {
            digits.push((x % 10) as u8);
            x /= 10;
        }
        digits.reverse();
        self.digits = digits;
        self.scale = scale;
        self.normalize();
    }

    fn as_u128_at(&self, scale: i32) -> Option<u128> {
        let mut v: u128 = 0;
        for d in &self.digits {
            v = v.checked_mul(10)?.checked_add(u128::from(*d))?;
        }
        let extra = self.scale - scale;
        for _ in 0..extra {
            v = v.checked_mul(10)?;
        }
        Some(v)
    }

    /// The integer digits (units and above), most significant first; an
    /// empty vector for a value below one.
    pub fn integer_digits(&self) -> Vec<u8> {
        let mag = self.magnitude();
        if self.is_zero() || mag < 0 {
            return Vec::new();
        }
        (0..=mag).rev().map(|p| self.digit_at(p)).collect()
    }

    /// The fraction digits, from the tenths down to the last stored digit.
    pub fn fraction_digits(&self) -> Vec<u8> {
        if self.is_zero() || self.scale >= 0 {
            return Vec::new();
        }
        (self.scale..0).rev().map(|p| self.digit_at(p)).collect()
    }

    /// The value as an `f64` (for parsing results).
    pub fn to_f64(&self) -> f64 {
        let mut s = String::new();
        if self.negative {
            s.push('-');
        }
        if self.digits.is_empty() {
            s.push('0');
        }
        for d in &self.digits {
            s.push((b'0' + d) as char);
        }
        s.push('e');
        s.push_str(&self.scale.to_string());
        s.parse().unwrap_or(0.0)
    }

    /// Multiply by an integer exactly (through `u128` while it fits, the
    /// double product otherwise).
    pub fn multiply_by_i64(&mut self, m: i64) {
        if m == 1 || self.is_zero() {
            return;
        }
        if m == 0 {
            *self = Decimal::zero();
            return;
        }
        let mut v: u128 = 0;
        let mut fits = true;
        for d in &self.digits {
            match v.checked_mul(10).and_then(|x| x.checked_add(u128::from(*d))) {
                Some(x) => v = x,
                None => {
                    fits = false;
                    break;
                }
            }
        }
        let product = if fits { v.checked_mul(m.unsigned_abs() as u128) } else { None };
        match product {
            Some(pv) => {
                let mut digits = Vec::new();
                let mut x = pv;
                while x > 0 {
                    digits.push((x % 10) as u8);
                    x /= 10;
                }
                digits.reverse();
                self.digits = digits;
                self.negative ^= m < 0;
                self.normalize();
            }
            None => {
                let f = self.to_f64() * m as f64;
                *self = Decimal::from_f64(f).unwrap_or_else(Decimal::zero);
            }
        }
    }

    /// The number of digits below the units (0 for an integer).
    pub fn fraction_length(&self) -> i32 {
        (-self.scale).max(0)
    }

    /// The value as an `i64` when it is integral and fits.
    pub fn to_i64(&self) -> Option<i64> {
        let mut v: i128 = 0;
        for d in &self.digits {
            v = v.checked_mul(10)?.checked_add(i128::from(*d))?;
            if v > (1i128 << 70) {
                return None;
            }
        }
        if self.scale > 0 {
            for _ in 0..self.scale {
                v = v.checked_mul(10)?;
                if v > (1i128 << 70) {
                    return None;
                }
            }
        } else if self.scale < 0 {
            // truncate toward zero
            for _ in 0..(-self.scale) {
                v /= 10;
            }
        }
        i64::try_from(if self.negative { -v } else { v }).ok()
    }
}
