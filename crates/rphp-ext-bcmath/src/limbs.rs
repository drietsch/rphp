//! Unsigned magnitudes in base 10⁹ limbs (little-endian), for the two
//! operations whose digit-by-digit cost would be quadratic in *digits*
//! rather than in limbs: multiplication and long division. libbcmath does
//! the same with its `BC_VECTOR` chunks (base 10⁸); the results are exact
//! integers either way, so the chunk width does not show.

/// The limb base.
const BASE: u64 = 1_000_000_000;
/// Decimal digits per limb.
const WIDTH: usize = 9;

/// Big-endian decimal digits (each 0–9) to limbs.
pub(crate) fn from_digits(digits: &[u8]) -> Vec<u64> {
    let mut out = Vec::with_capacity(digits.len() / WIDTH + 1);
    let mut end = digits.len();
    while end > 0 {
        let start = end.saturating_sub(WIDTH);
        let mut limb = 0u64;
        for &d in &digits[start..end] {
            limb = limb * 10 + u64::from(d);
        }
        out.push(limb);
        end = start;
    }
    trim(&mut out);
    out
}

/// Limbs to exactly `width` big-endian decimal digits, zero-padded on the
/// left (the value must fit).
pub(crate) fn to_digits(limbs: &[u64], width: usize) -> Vec<u8> {
    let mut out = vec![0u8; width];
    let mut pos = width;
    'outer: for &limb in limbs {
        let mut l = limb;
        for _ in 0..WIDTH {
            if pos == 0 {
                break 'outer;
            }
            pos -= 1;
            out[pos] = (l % 10) as u8;
            l /= 10;
        }
    }
    out
}

/// Drop high zero limbs (an empty vector is zero).
fn trim(v: &mut Vec<u64>) {
    while v.last() == Some(&0) {
        v.pop();
    }
}

/// `a * b`.
pub(crate) fn mul(a: &[u64], b: &[u64]) -> Vec<u64> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u64; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        if x == 0 {
            continue;
        }
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            // x*y < 10¹⁸ and out + carry < 2·10⁹: no overflow of u64.
            let t = out[i + j] + x * y + carry;
            out[i + j] = t % BASE;
            carry = t / BASE;
        }
        let mut k = i + b.len();
        while carry > 0 {
            let t = out[k] + carry;
            out[k] = t % BASE;
            carry = t / BASE;
            k += 1;
        }
    }
    trim(&mut out);
    out
}

/// `floor(a / b)` for a non-zero `b` (Knuth's algorithm D).
pub(crate) fn div(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    trim(&mut a);
    trim(&mut b);
    assert!(!b.is_empty(), "division by zero limbs");
    if cmp(&a, &b) == std::cmp::Ordering::Less {
        return Vec::new();
    }
    if b.len() == 1 {
        let d = b[0];
        let mut q = vec![0u64; a.len()];
        let mut rem = 0u64;
        for i in (0..a.len()).rev() {
            let cur = rem * BASE + a[i];
            q[i] = cur / d;
            rem = cur % d;
        }
        trim(&mut q);
        return q;
    }
    // Normalize so the divisor's top limb is at least BASE / 2.
    let f = BASE / (b[b.len() - 1] + 1);
    let u = scale(&a, f);
    let v = scale(&b, f);
    let n = v.len();
    let mut u = u;
    if u.len() == a.len() {
        u.push(0);
    }
    // `u` has m + n + 1 limbs.
    while u.len() < a.len() + 1 {
        u.push(0);
    }
    let m = u.len() - n - 1;
    let mut q = vec![0u64; m + 1];
    let vt = v[n - 1];
    let vs = v[n - 2];
    for j in (0..=m).rev() {
        let num = u[j + n] * BASE + u[j + n - 1];
        let mut qhat = num / vt;
        let mut rhat = num % vt;
        while qhat >= BASE || qhat * vs > rhat * BASE + u[j + n - 2] {
            qhat -= 1;
            rhat += vt;
            if rhat >= BASE {
                break;
            }
        }
        // u[j..=j+n] -= qhat * v
        let mut borrow = 0i64;
        let mut carry = 0u64;
        for i in 0..n {
            let p = qhat * v[i] + carry;
            carry = p / BASE;
            let t = u[i + j] as i64 - (p % BASE) as i64 - borrow;
            if t < 0 {
                u[i + j] = (t + BASE as i64) as u64;
                borrow = 1;
            } else {
                u[i + j] = t as u64;
                borrow = 0;
            }
        }
        let t = u[j + n] as i64 - carry as i64 - borrow;
        if t < 0 {
            u[j + n] = (t + BASE as i64) as u64;
            // Added back: qhat was one too large.
            qhat -= 1;
            let mut c = 0u64;
            for i in 0..n {
                let s = u[i + j] + v[i] + c;
                u[i + j] = s % BASE;
                c = s / BASE;
            }
            u[j + n] = (u[j + n] + c) % BASE;
        } else {
            u[j + n] = t as u64;
        }
        q[j] = qhat;
    }
    trim(&mut q);
    q
}

/// `a * f` for a small factor.
fn scale(a: &[u64], f: u64) -> Vec<u64> {
    let mut out = Vec::with_capacity(a.len() + 1);
    let mut carry = 0u64;
    for &x in a {
        let t = x * f + carry;
        out.push(t % BASE);
        carry = t / BASE;
    }
    if carry > 0 {
        out.push(carry);
    }
    out
}

fn cmp(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Vec<u64> {
        from_digits(&s.bytes().map(|b| b - b'0').collect::<Vec<_>>())
    }

    fn s(v: &[u64]) -> String {
        let d = to_digits(v, 60);
        let t: String = d.iter().map(|d| (b'0' + d) as char).collect();
        let t = t.trim_start_matches('0');
        if t.is_empty() { "0".into() } else { t.into() }
    }

    #[test]
    fn mul_and_div_roundtrip() {
        let a = n("123456789012345678901234567890");
        let b = n("987654321987654321");
        let p = mul(&a, &b);
        assert_eq!(s(&p), "121932631246761163237311385323609205901126352690");
        assert_eq!(s(&div(&p, &b)), "123456789012345678901234567890");
        assert_eq!(s(&div(&n("1000000000000000000000"), &n("7"))), "142857142857142857142");
        assert_eq!(s(&div(&n("5"), &n("7"))), "0");
        let big = n("99999999999999999999999999999999999999");
        let d = n("99999999999999999999");
        assert_eq!(s(&div(&big, &d)), "1000000000000000000");
        assert_eq!(s(&div(&n("340282366920938463463374607431768211456"), &n("18446744073709551616"))), "18446744073709551616");
    }
}
