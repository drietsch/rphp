//! ICU's `PluralRules` over the rules php 8.5.10's ICU carries
//! (`plural_data.rs`, dumped from the oracle — ICU4X's baked data leaves
//! out some of the languages ICU has): the CLDR rule syntax evaluated on
//! the operands of a decimal as displayed.

use super::plural_data::{CARDINAL, ORDINAL, RULES};

/// The plural operands of a non-negative decimal text (`12.50`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Operands {
    n: f64,
    i: u64,
    v: u64,
    w: u64,
    f: u64,
    t: u64,
    e: u64,
}

impl Operands {
    /// The operands of a decimal text: digits, an optional `.` and the
    /// visible fraction digits.
    pub(crate) fn from_text(text: &str) -> Operands {
        let text = text.trim_start_matches('-');
        let (int, frac) = text.split_once('.').unwrap_or((text, ""));
        let i = int.parse::<u64>().unwrap_or(u64::MAX);
        let v = frac.len() as u64;
        let trimmed = frac.trim_end_matches('0');
        let w = trimmed.len() as u64;
        let f = if frac.is_empty() { 0 } else { frac.chars().take(18).collect::<String>().parse::<u64>().unwrap_or(0) };
        let t = if trimmed.is_empty() { 0 } else { trimmed.chars().take(18).collect::<String>().parse::<u64>().unwrap_or(0) };
        let n = text.parse::<f64>().unwrap_or(0.0);
        Operands { n, i, v, w, f, t, e: 0 }
    }

    /// The operands of a double in its shortest form (`select(double)`).
    pub(crate) fn from_f64(x: f64) -> Operands {
        let x = x.abs();
        if !x.is_finite() {
            return Operands { n: x, ..Default::default() };
        }
        let text = if x.fract() == 0.0 && x < 1e18 { format!("{x:.0}") } else { format!("{x}") };
        Operands::from_text(&text)
    }

    pub(crate) fn from_int(n: i64) -> Operands {
        Operands::from_text(&n.unsigned_abs().to_string())
    }

    fn get(&self, op: char) -> f64 {
        match op {
            'n' => self.n,
            'i' => self.i as f64,
            'v' => self.v as f64,
            'w' => self.w as f64,
            'f' => self.f as f64,
            't' => self.t as f64,
            'e' | 'c' => self.e as f64,
            _ => 0.0,
        }
    }
}

/// One relation: `operand [% mod] (= | !=) ranges`.
fn relation(rel: &str, ops: &Operands) -> bool {
    let (lhs, rhs, negate) = if let Some((l, r)) = rel.split_once("!=") {
        (l, r, true)
    } else if let Some((l, r)) = rel.split_once('=') {
        (l, r, false)
    } else if let Some((l, r)) = rel.split_once(" not in ") {
        (l, r, true)
    } else if let Some((l, r)) = rel.split_once(" in ") {
        (l, r, false)
    } else if let Some((l, r)) = rel.split_once(" is not ") {
        (l, r, true)
    } else if let Some((l, r)) = rel.split_once(" is ") {
        (l, r, false)
    } else {
        return false;
    };
    let lhs = lhs.trim();
    let (op, modulus) = match lhs.split_once('%') {
        Some((o, m)) => (o.trim(), m.trim().parse::<f64>().ok()),
        None => (lhs, None),
    };
    let mut value = ops.get(op.chars().next().unwrap_or('n'));
    if let Some(m) = modulus {
        if m != 0.0 {
            value %= m;
        }
    }
    let integral = value.fract() == 0.0;
    let hit = rhs.split(',').any(|range| {
        let range = range.trim();
        match range.split_once("..") {
            Some((a, b)) => {
                let (a, b) = (a.trim().parse::<f64>().unwrap_or(f64::NAN), b.trim().parse::<f64>().unwrap_or(f64::NAN));
                integral && value >= a && value <= b
            }
            None => range.parse::<f64>().is_ok_and(|x| x == value),
        }
    });
    hit != negate
}

fn condition(cond: &str, ops: &Operands) -> bool {
    if cond.trim().is_empty() {
        return true;
    }
    cond.split(" or ").any(|and| and.split(" and ").all(|rel| relation(rel, ops)))
}

/// The rule set of a locale: the id itself, then its truncations.
fn rule_set(table: &'static [(&'static str, &'static str)], locale: &str) -> Option<&'static str> {
    let mut cand = locale.split('@').next().unwrap_or("").to_string();
    loop {
        if let Ok(i) = table.binary_search_by(|(l, _)| (*l).cmp(cand.as_str())) {
            return Some(table[i].1);
        }
        let i = cand.rfind('_')?;
        cand.truncate(i);
    }
}

/// `PluralRules::select`: the keyword of a number in a locale.
pub(crate) fn select(locale: &str, ordinal: bool, ops: &Operands) -> &'static str {
    let canonical = crate::locale::canonical(locale);
    let Some(set) = rule_set(if ordinal { ORDINAL } else { CARDINAL }, &canonical) else {
        return "other";
    };
    let Ok(i) = RULES.binary_search_by(|(s, _)| (*s).cmp(set)) else {
        return "other";
    };
    for (kw, cond) in RULES[i].1 {
        if *kw != "other" && condition(cond, ops) {
            return kw;
        }
    }
    "other"
}
