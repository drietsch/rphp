//! libbcmath's `bc_num`: a sign, `len` integer digits and `scale`
//! fractional digits, one decimal digit per byte. The operations follow
//! libbcmath step by step — which scale a result carries, when a zero
//! loses its sign and when it keeps it (`-0.00` survives a truncating
//! multiply and orders below `0`, as in php) — because those fields are
//! observable through `BcMath\Number` and through the next operation.
//! Only the digit arithmetic itself (the product, the quotient) is done
//! differently, on base-10⁹ limbs ([`crate::limbs`]); it is exact either
//! way.

use std::cmp::Ordering;

use crate::limbs;

/// A decimal number (`bc_num`).
#[derive(Clone, Debug)]
pub(crate) struct BcNum {
    pub neg: bool,
    /// Integer digits (at least one).
    pub len: usize,
    /// Fractional digits.
    pub scale: usize,
    /// `len + scale` digits, most significant first.
    pub d: Vec<u8>,
}

/// The rounding modes (`PHP_ROUND_*`).
pub(crate) const ROUND_HALF_UP: i64 = 1;
pub(crate) const ROUND_HALF_DOWN: i64 = 2;
pub(crate) const ROUND_HALF_EVEN: i64 = 3;
pub(crate) const ROUND_HALF_ODD: i64 = 4;
pub(crate) const ROUND_CEILING: i64 = 5;
pub(crate) const ROUND_FLOOR: i64 = 6;
pub(crate) const ROUND_TOWARD_ZERO: i64 = 7;
pub(crate) const ROUND_AWAY_FROM_ZERO: i64 = 8;

/// `bc_raise`'s failures.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RaiseError {
    DivideByZero,
    Overflow,
}

/// `bc_raisemod`'s failures.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RaiseModError {
    BaseHasFractional,
    ExpoHasFractional,
    ExpoIsNegative,
    ModHasFractional,
    ModIsZero,
}

impl BcNum {
    /// `bc_new_num(len, scale)`: zero with that shape.
    pub fn new(len: usize, scale: usize) -> BcNum {
        BcNum { neg: false, len, scale, d: vec![0; len + scale] }
    }

    pub fn zero() -> BcNum {
        BcNum::new(1, 0)
    }

    pub fn one() -> BcNum {
        BcNum { neg: false, len: 1, scale: 0, d: vec![1] }
    }

    fn two() -> BcNum {
        BcNum { neg: false, len: 1, scale: 0, d: vec![2] }
    }

    /// `bc_long2num`.
    pub fn from_i64(v: i64) -> BcNum {
        if v == 0 {
            return BcNum::zero();
        }
        let digits: Vec<u8> = v.unsigned_abs().to_string().bytes().map(|b| b - b'0').collect();
        BcNum { neg: v < 0, len: digits.len(), scale: 0, d: digits }
    }

    /// Set `scale` lower (`num->n_scale = MIN(...)`), dropping digits.
    fn truncate_scale(&mut self, scale: usize) {
        if scale < self.scale {
            self.scale = scale;
            self.d.truncate(self.len + scale);
        }
    }

    /// `bc_is_zero_for_scale`.
    pub fn is_zero_for_scale(&self, scale: usize) -> bool {
        let n = self.len + scale.min(self.scale);
        self.d[..n].iter().all(|&x| x == 0)
    }

    /// `bc_is_zero`.
    pub fn is_zero(&self) -> bool {
        self.d.iter().all(|&x| x == 0)
    }

    /// `bc_is_near_zero`: every digit up to `scale` is 0 but a last 1.
    fn is_near_zero(&self, scale: usize) -> bool {
        let n = self.len + scale.min(self.scale);
        let digits = &self.d[..n];
        match digits.iter().position(|&x| x != 0) {
            None => true,
            Some(i) => i == n - 1 && digits[i] == 1,
        }
    }

    /// `_bc_rm_leading_zeros`.
    fn rm_leading_zeros(&mut self) {
        let lead = self.d[..self.len - 1].iter().take_while(|&&x| x == 0).count();
        if lead > 0 {
            self.d.drain(..lead);
            self.len -= lead;
        }
    }

    /// `bc_rm_trailing_zeros`.
    pub fn rm_trailing_zeros(&mut self) {
        while self.scale > 0 && self.d[self.len + self.scale - 1] == 0 {
            self.scale -= 1;
        }
        self.d.truncate(self.len + self.scale);
    }

    /// `bc_str2num`: `None` for a string that is not a number. With
    /// `auto_scale` the whole fraction is kept; otherwise it is truncated
    /// to `scale` (`bccomp`). The second value is the fraction's length as
    /// written, trailing zeros included (`BcMath\Number`'s scale).
    pub fn parse(s: &[u8], scale: usize, auto_scale: bool) -> Option<(BcNum, usize)> {
        // A NUL ends the string, as the C scanner sees it.
        let s = match s.iter().position(|&b| b == 0) {
            Some(i) => &s[..i],
            None => s,
        };
        let mut p = 0;
        if matches!(s.first(), Some(b'+' | b'-')) {
            p = 1;
        }
        while s.get(p) == Some(&b'0') {
            p += 1;
        }
        let int_start = p;
        while s.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
        let int_digits = &s[int_start..p];
        let mut frac: &[u8] = &[];
        let mut full_scale = 0;
        if p < s.len() {
            if s[p] != b'.' {
                return None;
            }
            let fs = p + 1;
            let mut fe = fs;
            while s.get(fe).is_some_and(u8::is_ascii_digit) {
                fe += 1;
            }
            if fe != s.len() {
                return None;
            }
            full_scale = fe - fs;
            let mut end = fe;
            while end > fs && s[end - 1] == b'0' {
                end -= 1;
            }
            frac = &s[fs..end];
            if !auto_scale && frac.len() > scale {
                frac = &frac[..scale];
                let mut e = frac.len();
                while e > 0 && frac[e - 1] == b'0' {
                    e -= 1;
                }
                frac = &frac[..e];
            }
        }
        if int_digits.is_empty() && frac.is_empty() {
            return Some((BcNum::zero(), full_scale));
        }
        let mut d = Vec::with_capacity(int_digits.len().max(1) + frac.len());
        if int_digits.is_empty() {
            d.push(0);
        } else {
            d.extend(int_digits.iter().map(|b| b - b'0'));
        }
        let len = d.len();
        d.extend(frac.iter().map(|b| b - b'0'));
        Some((BcNum { neg: s.first() == Some(&b'-'), len, scale: frac.len(), d }, full_scale))
    }

    /// `bc_num2str_ex(num, scale)`: the digits with exactly `scale`
    /// fractional places (truncated or zero-padded); a sign only when the
    /// printed digits are not all zero.
    pub fn to_str(&self, scale: usize) -> Vec<u8> {
        let min_scale = self.scale.min(scale);
        let sign = self.neg && !self.is_zero_for_scale(min_scale);
        let mut out = Vec::with_capacity(self.len + scale + 2);
        if sign {
            out.push(b'-');
        }
        out.extend(self.d[..self.len].iter().map(|d| b'0' + d));
        if scale > 0 {
            out.push(b'.');
            out.extend(self.d[self.len..self.len + min_scale].iter().map(|d| b'0' + d));
            out.resize(out.len() + (scale - min_scale), b'0');
        }
        out
    }

    /// `bc_num2long`: the integer part, or 0 when it does not fit.
    pub fn to_long(&self) -> i64 {
        let mut val: i64 = 0;
        for &n in &self.d[..self.len] {
            let Some(v) = val.checked_mul(10).and_then(|v| v.checked_add(i64::from(n))) else {
                return 0;
            };
            val = v;
        }
        if self.neg {
            -val
        } else {
            val
        }
    }

    /// Whether the magnitude is exactly one (`_bc_do_compare(n, one,
    /// n->n_scale, false) == EQUAL`).
    fn is_abs_one(&self) -> bool {
        do_compare(self, &BcNum::one(), self.scale, false) == Ordering::Equal
    }
}

/// `_bc_do_compare`: `n1` against `n2` on the first `scale` fractional
/// digits, by magnitude unless `use_sign`.
pub(crate) fn do_compare(n1: &BcNum, n2: &BcNum, scale: usize, use_sign: bool) -> Ordering {
    let result = |left_abs_greater: bool| -> Ordering {
        if left_abs_greater == (!use_sign || !n1.neg) {
            Ordering::Greater
        } else {
            Ordering::Less
        }
    };
    if use_sign && n1.neg != n2.neg {
        // e.g. `0.00 <=> -0.00` when the scale in effect hides the digits.
        if (n1.scale > scale || n2.scale > scale)
            && n1.len == 1
            && n2.len == 1
            && n1.d[0] == 0
            && n2.d[0] == 0
            && n1.is_zero_for_scale(scale)
            && n2.is_zero_for_scale(scale)
        {
            return Ordering::Equal;
        }
        return if n1.neg { Ordering::Less } else { Ordering::Greater };
    }
    if n1.len != n2.len {
        return result(n1.len > n2.len);
    }
    let s1 = n1.scale.min(scale);
    let s2 = n2.scale.min(scale);
    let count = n1.len + s1.min(s2);
    for i in 0..count {
        if n1.d[i] != n2.d[i] {
            return result(n1.d[i] > n2.d[i]);
        }
    }
    if s1 > s2 {
        if n1.d[count..n1.len + s1].iter().any(|&x| x != 0) {
            return result(true);
        }
    } else if s2 > s1 && n2.d[count..n2.len + s2].iter().any(|&x| x != 0) {
        return result(false);
    }
    Ordering::Equal
}

/// `bc_compare`.
pub(crate) fn compare(n1: &BcNum, n2: &BcNum, scale: usize) -> i64 {
    match do_compare(n1, n2, scale, true) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// The digit at integer position `i` counted from the decimal point
/// leftwards (`0` = units), or fraction position `-(k+1)`.
#[inline]
fn digit_at(n: &BcNum, int_pos: isize) -> u8 {
    // int_pos >= 0: integer digit; < 0: fractional digit -int_pos-1.
    let idx = n.len as isize - 1 - int_pos;
    if idx < 0 || idx as usize >= n.d.len() {
        0
    } else {
        n.d[idx as usize]
    }
}

/// `_bc_do_add`: the magnitudes' sum (sign plus).
fn do_add(n1: &BcNum, n2: &BcNum) -> BcNum {
    let len = n1.len.max(n2.len) + 1;
    let scale = n1.scale.max(n2.scale);
    let mut d = vec![0u8; len + scale];
    let mut carry = 0u8;
    for k in (0..len + scale).rev() {
        let pos = (len as isize - 1) - k as isize;
        let s = digit_at(n1, pos) + digit_at(n2, pos) + carry;
        if s >= 10 {
            d[k] = s - 10;
            carry = 1;
        } else {
            d[k] = s;
            carry = 0;
        }
    }
    let mut r = BcNum { neg: false, len, scale, d };
    r.rm_leading_zeros();
    r
}

/// `_bc_do_sub`: `|n1| - |n2|`, `|n1|` being the larger (sign plus).
fn do_sub(n1: &BcNum, n2: &BcNum) -> BcNum {
    let len = n1.len.max(n2.len);
    let scale = n1.scale.max(n2.scale);
    let mut d = vec![0u8; len + scale];
    let mut borrow = 0i8;
    for k in (0..len + scale).rev() {
        let pos = (len as isize - 1) - k as isize;
        let mut v = digit_at(n1, pos) as i8 - digit_at(n2, pos) as i8 - borrow;
        if v < 0 {
            v += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        d[k] = v as u8;
    }
    let mut r = BcNum { neg: false, len, scale, d };
    r.rm_leading_zeros();
    r
}

/// `bc_add(n1, n2, scale_min)`.
pub(crate) fn add(n1: &BcNum, n2: &BcNum, scale_min: usize) -> BcNum {
    if n1.neg == n2.neg {
        let mut sum = do_add(n1, n2);
        sum.neg = n1.neg;
        return sum;
    }
    match do_compare(n1, n2, scale_min, false) {
        Ordering::Less => {
            let mut sum = do_sub(n2, n1);
            sum.neg = n2.neg;
            sum
        }
        Ordering::Equal => BcNum::new(1, scale_min.max(n1.scale.max(n2.scale))),
        Ordering::Greater => {
            let mut sum = do_sub(n1, n2);
            sum.neg = n1.neg;
            sum
        }
    }
}

/// `bc_sub(n1, n2, scale_min)`.
pub(crate) fn sub(n1: &BcNum, n2: &BcNum, scale_min: usize) -> BcNum {
    if n1.neg != n2.neg {
        let mut diff = do_add(n1, n2);
        diff.neg = n1.neg;
        return diff;
    }
    match do_compare(n1, n2, scale_min, false) {
        Ordering::Less => {
            let mut diff = do_sub(n2, n1);
            diff.neg = !n2.neg;
            diff
        }
        Ordering::Equal => BcNum::new(1, scale_min.max(n1.scale.max(n2.scale))),
        Ordering::Greater => {
            let mut diff = do_sub(n1, n2);
            diff.neg = n1.neg;
            diff
        }
    }
}

/// `bc_multiply(n1, n2, scale)`: the exact product, cut to
/// `min(full scale, max(scale, n1.scale, n2.scale))` digits.
pub(crate) fn multiply(n1: &BcNum, n2: &BcNum, scale: usize) -> BcNum {
    let full_scale = n1.scale + n2.scale;
    let prod_scale = full_scale.min(scale.max(n1.scale.max(n2.scale)));
    let a = limbs::from_digits(&n1.d);
    let b = limbs::from_digits(&n2.d);
    let p = limbs::mul(&a, &b);
    let total = n1.d.len() + n2.d.len();
    let digits = limbs::to_digits(&p, total);
    let len = total - full_scale;
    let mut prod = BcNum { neg: n1.neg != n2.neg, len, scale: full_scale, d: digits };
    prod.truncate_scale(prod_scale);
    prod.rm_leading_zeros();
    if prod.is_zero() {
        prod.neg = false;
        prod.truncate_scale(0);
    }
    prod
}

/// `bc_divide(numerator, divisor, scale)`: the quotient truncated to
/// `scale` digits; `None` for a zero divisor.
pub(crate) fn divide(numerator: &BcNum, divisor: &BcNum, scale: usize) -> Option<BcNum> {
    if divisor.is_zero() {
        return None;
    }
    let quot_scale = scale;
    if numerator.is_zero() {
        return Some(BcNum::zero());
    }
    let sign = numerator.neg != divisor.neg;
    // A divisor of ±1: the numerator's own digits.
    if divisor.is_abs_one() {
        let qs = numerator.scale.min(quot_scale);
        let mut q = BcNum {
            neg: sign,
            len: numerator.len,
            scale: qs,
            d: numerator.d[..numerator.len + qs].to_vec(),
        };
        q.scale = qs;
        return Some(q);
    }
    let num_digits = &numerator.d;
    let mut numerator_size = numerator.len + quot_scale + divisor.scale;
    let lead = num_digits.iter().take(numerator_size).take_while(|&&x| x == 0).count();
    if lead == numerator_size {
        return Some(BcNum::zero());
    }
    numerator_size -= lead;
    let num_ptr = &num_digits[lead..];
    let div_lead = divisor.d.iter().take_while(|&&x| x == 0).count();
    let div_ptr = &divisor.d[div_lead..];
    let mut divisor_size = div_ptr.len();
    if divisor_size > numerator_size {
        return Some(BcNum::zero());
    }
    let trailing = div_ptr[1..divisor_size].iter().rev().take_while(|&&x| x == 0).count();
    divisor_size -= trailing;
    numerator_size -= trailing;
    let quot_size = numerator_size - divisor_size + 1;
    let (qlen, qscale) = if quot_size > quot_scale {
        (quot_size - quot_scale, quot_scale)
    } else {
        (1, quot_scale)
    };
    let numerator_readable = numerator.len + numerator.scale - lead;
    // A divisor that is a power of ten: the numerator's digits, shifted.
    if divisor_size == 1 && div_ptr[0] == 1 {
        let mut d = Vec::with_capacity(qlen + qscale);
        // Leading zeros when the quotient is below one.
        d.resize((quot_scale + 1).saturating_sub(quot_size), 0);
        let use_size = quot_size.min(numerator_readable);
        d.extend_from_slice(&num_ptr[..use_size]);
        let mut q = BcNum { neg: sign, len: qlen, scale: qscale, d };
        if use_size < q.len {
            // e.g. 12.3 / 0.01 = 1230
            q.d.resize(q.len, 0);
            q.scale = 0;
        } else {
            let filled = q.d.len();
            q.scale -= q.len + q.scale - filled;
        }
        return Some(q);
    }
    // The long division of the digit windows.
    let mut window: Vec<u8> = num_ptr[..numerator_size.min(numerator_readable)].to_vec();
    window.resize(numerator_size, 0);
    let qv = limbs::div(&limbs::from_digits(&window), &limbs::from_digits(&div_ptr[..divisor_size]));
    let mut q = BcNum {
        neg: false,
        len: qlen,
        scale: qscale,
        d: limbs::to_digits(&qv, qlen + qscale),
    };
    q.rm_leading_zeros();
    if q.is_zero() {
        q.truncate_scale(0);
    } else {
        q.neg = sign;
    }
    Some(q)
}

/// `bc_divmod(num1, num2, scale)`: the integer quotient and the
/// remainder `num1 - q * num2` at `scale` digits; `None` for a zero
/// divisor.
pub(crate) fn divmod(num1: &BcNum, num2: &BcNum, scale: usize) -> Option<(BcNum, BcNum)> {
    if num2.is_zero() {
        return None;
    }
    let rscale = num1.scale.max(num2.scale + scale);
    let quot = divide(num1, num2, 0).expect("non-zero divisor");
    let temp = multiply(&quot, num2, rscale);
    let mut rem = sub(num1, &temp, rscale);
    rem.truncate_scale(scale);
    if rem.is_zero() {
        rem.neg = false;
        rem.truncate_scale(0);
    }
    Some((quot, rem))
}

/// `bc_modulo`.
pub(crate) fn modulo(num1: &BcNum, num2: &BcNum, scale: usize) -> Option<BcNum> {
    divmod(num1, num2, scale).map(|(_, r)| r)
}

/// `bc_raise(base, exponent, scale)`.
pub(crate) fn raise(base: &BcNum, exponent: i64, scale: usize) -> Result<BcNum, RaiseError> {
    if exponent == 0 {
        return Ok(BcNum::one());
    }
    let (is_neg, exp) = (exponent < 0, exponent.unsigned_abs());
    let rscale = if is_neg { scale } else { 0 };
    if base.is_zero() {
        if is_neg {
            return Err(RaiseError::DivideByZero);
        }
        return Ok(BcNum::zero());
    }
    let exp_us = usize::try_from(exp).map_err(|_| RaiseError::Overflow)?;
    let power_len = base.len.checked_mul(exp_us).ok_or(RaiseError::Overflow)?;
    let power_scale = base.scale.checked_mul(exp_us).ok_or(RaiseError::Overflow)?;
    let full = power_len.checked_add(power_scale).ok_or(RaiseError::Overflow)?;
    // Exponentiation by squaring on the limbs.
    let mut b = limbs::from_digits(&base.d);
    let mut acc: Option<Vec<u64>> = None;
    let mut e = exp;
    while e > 0 {
        if e & 1 == 1 {
            acc = Some(match acc {
                None => b.clone(),
                Some(a) => limbs::mul(&a, &b),
            });
        }
        e >>= 1;
        if e > 0 {
            b = limbs::mul(&b, &b);
        }
    }
    let p = acc.expect("exponent > 0");
    let mut power = BcNum {
        neg: base.neg && exp & 1 == 1,
        len: power_len,
        scale: power_scale,
        d: limbs::to_digits(&p, full),
    };
    power.rm_leading_zeros();
    if power.is_zero() {
        power.neg = false;
        power.truncate_scale(0);
    }
    if is_neg {
        return divide(&BcNum::one(), &power, rscale).ok_or(RaiseError::DivideByZero);
    }
    power.truncate_scale(scale);
    Ok(power)
}

/// `bc_raisemod(base, expo, mod, scale)`.
pub(crate) fn raisemod(base: &BcNum, expo: &BcNum, modulus: &BcNum, scale: usize) -> Result<BcNum, RaiseModError> {
    if base.scale != 0 {
        return Err(RaiseModError::BaseHasFractional);
    }
    if expo.scale != 0 {
        return Err(RaiseModError::ExpoHasFractional);
    }
    if expo.neg {
        return Err(RaiseModError::ExpoIsNegative);
    }
    if modulus.scale != 0 {
        return Err(RaiseModError::ModHasFractional);
    }
    if modulus.is_zero() {
        return Err(RaiseModError::ModIsZero);
    }
    if modulus.is_abs_one() {
        return Ok(BcNum::new(1, scale));
    }
    let two = BcNum::two();
    let mut power = base.clone();
    let mut exponent = expo.clone();
    let mut temp = BcNum::one();
    while !exponent.is_zero() {
        let (q, parity) = divmod(&exponent, &two, 0).expect("two");
        exponent = q;
        if !parity.is_zero() {
            temp = multiply(&temp, &power, scale);
            temp = modulo(&temp, modulus, scale).expect("non-zero modulus");
        }
        power = multiply(&power, &power, scale);
        power = modulo(&power, modulus, scale).expect("non-zero modulus");
    }
    Ok(temp)
}

/// `bc_sqrt(num, scale)`: `None` for a negative number.
pub(crate) fn sqrt(num: &BcNum, scale: usize) -> Option<BcNum> {
    if num.neg {
        return None;
    }
    if num.is_zero() {
        return Some(BcNum::zero());
    }
    let one = BcNum::one();
    let cmp_one = do_compare(num, &one, num.scale, true);
    if cmp_one == Ordering::Equal {
        return Some(one);
    }
    let rscale = scale.max(num.scale);
    let point5 = BcNum { neg: false, len: 1, scale: 1, d: vec![0, 5] };
    let (mut guess, mut cscale) = if cmp_one == Ordering::Less {
        (BcNum::one(), num.scale)
    } else {
        // 10^(floor(len / 2)).
        let mut half = multiply(&BcNum::from_i64(num.len as i64), &point5, 0);
        half.truncate_scale(0);
        let e = half.to_long();
        (raise(&BcNum::from_i64(10), e, 0).expect("ten"), 3)
    };
    loop {
        let guess1 = guess.clone();
        guess = divide(num, &guess, cscale).expect("non-zero guess");
        guess = add(&guess, &guess1, 0);
        guess = multiply(&guess, &point5, cscale);
        let diff = sub(&guess, &guess1, cscale + 1);
        if diff.is_near_zero(cscale) {
            if cscale < rscale + 1 {
                cscale = (cscale * 3).min(rscale + 1);
            } else {
                break;
            }
        }
    }
    divide(&guess, &one, rscale)
}

/// `bc_floor_or_ceil`.
pub(crate) fn floor_or_ceil(num: &BcNum, is_floor: bool) -> BcNum {
    let mut result = BcNum { neg: num.neg, len: num.len, scale: 0, d: num.d[..num.len].to_vec() };
    let unchanged = num.scale == 0 || result.neg != is_floor;
    if !unchanged && num.d[num.len..].iter().any(|&x| x != 0) {
        let neg = result.neg;
        result = do_add(&result, &BcNum::one());
        result.neg = neg;
    }
    if result.is_zero() {
        result.neg = false;
    }
    result
}

/// `bc_round(num, precision, mode)`: the rounded number and the scale it
/// is printed with.
pub(crate) fn round(num: &BcNum, precision: i64, mode: i64) -> (BcNum, usize) {
    if precision < 0 && (num.len as u128) < u128::from((-(precision + 1)) as u64) + 1 {
        match mode {
            ROUND_HALF_UP | ROUND_HALF_DOWN | ROUND_HALF_EVEN | ROUND_HALF_ODD | ROUND_TOWARD_ZERO => {
                return (BcNum::zero(), 0);
            }
            ROUND_CEILING if num.neg => return (BcNum::zero(), 0),
            ROUND_FLOOR if !num.neg => return (BcNum::zero(), 0),
            _ => {}
        }
        if num.is_zero() {
            return (BcNum::zero(), 0);
        }
        let magnitude = precision.unsigned_abs() as usize;
        let mut r = BcNum::new(magnitude + 1, 0);
        r.d[0] = 1;
        r.neg = num.neg;
        return (r, 0);
    }
    if precision >= 0 && num.scale as u64 <= precision as u64 {
        let p = precision as usize;
        let mut r = num.clone();
        r.scale = p;
        r.d.resize(r.len + p, 0);
        return (r, p);
    }
    // Negative results returned early above: no underflow.
    let rounded_len = (num.len as i64 + precision) as usize;
    let mut result = if rounded_len == 0 {
        BcNum::new(1, 0)
    } else {
        let mut r = BcNum::new(num.len, if precision > 0 { precision as usize } else { 0 });
        r.d[..rounded_len].copy_from_slice(&num.d[..rounded_len]);
        r
    };
    result.neg = num.neg;
    let next = num.d[rounded_len];
    // Whether the magnitude goes up; `None` asks for the remaining digits.
    let decided = match mode {
        ROUND_HALF_UP => Some(next >= 5),
        ROUND_HALF_DOWN | ROUND_HALF_EVEN | ROUND_HALF_ODD => {
            if next > 5 {
                Some(true)
            } else if next < 5 {
                Some(false)
            } else {
                None
            }
        }
        ROUND_CEILING => {
            if num.neg {
                Some(false)
            } else if next > 0 {
                Some(true)
            } else {
                None
            }
        }
        ROUND_FLOOR => {
            if !num.neg {
                Some(false)
            } else if next > 0 {
                Some(true)
            } else {
                None
            }
        }
        ROUND_TOWARD_ZERO => Some(false),
        _ => {
            if next > 0 {
                Some(true)
            } else {
                None
            }
        }
    };
    let up = match decided {
        Some(up) => up,
        None => {
            let rest = &num.d[rounded_len + 1..];
            if rest.iter().any(|&x| x != 0) {
                true
            } else {
                match mode {
                    ROUND_HALF_EVEN => !(rounded_len == 0 || num.d[rounded_len - 1].is_multiple_of(2)),
                    ROUND_HALF_ODD => !(rounded_len != 0 && num.d[rounded_len - 1] % 2 == 1),
                    _ => false,
                }
            }
        }
    };
    if up {
        let tmp = if rounded_len == 0 {
            let mut t = BcNum::new(num.len + 1, 0);
            t.d[0] = 1;
            t.neg = num.neg;
            t
        } else {
            let mut one = BcNum::new(result.len, result.scale);
            one.d[rounded_len - 1] = 1;
            let mut t = do_add(&result, &one);
            t.neg = result.neg;
            t
        };
        result = tmp;
    }
    let scale = result.scale;
    if result.is_zero() {
        result.neg = false;
        result.truncate_scale(0);
    }
    (result, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> BcNum {
        BcNum::parse(s.as_bytes(), 0, true).unwrap().0
    }

    fn s(v: &BcNum, scale: usize) -> String {
        String::from_utf8(v.to_str(scale)).unwrap()
    }

    #[test]
    fn arithmetic_matches_bc() {
        assert_eq!(s(&add(&n("1.5"), &n("2.25"), 3), 3), "3.750");
        assert_eq!(s(&sub(&n("1"), &n("2.5"), 1), 1), "-1.5");
        assert_eq!(s(&multiply(&n("1.25"), &n("-1.25"), 1), 1), "-1.5");
        assert_eq!(s(&divide(&n("10"), &n("3"), 5).unwrap(), 5), "3.33333");
        assert_eq!(s(&divide(&n("12.3"), &n("0.01"), 0).unwrap(), 0), "1230");
        assert_eq!(s(&modulo(&n("-7"), &n("3"), 0).unwrap(), 0), "-1");
        assert_eq!(s(&raise(&n("2"), 100, 0).unwrap(), 0), "1267650600228229401496703205376");
        assert_eq!(s(&raise(&n("2"), -2, 4).unwrap(), 4), "0.2500");
        assert_eq!(s(&sqrt(&n("2"), 10).unwrap(), 10), "1.4142135623");
        assert_eq!(s(&raisemod(&n("4"), &n("13"), &n("497"), 0).unwrap(), 0), "445");
        let (r, sc) = round(&n("1.955"), 2, ROUND_HALF_UP);
        assert_eq!(s(&r, sc), "1.96");
        assert_eq!(s(&floor_or_ceil(&n("-1.5"), true), 0), "-2");
    }
}
