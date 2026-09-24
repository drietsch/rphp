//! ICU's `RuleBasedNumberFormat`: the spellout, ordinal and duration
//! formats. The rules are the ones php 8.5.10's ICU builds each locale's
//! formatter from (`data.rs`, `NumberFormatter::getPattern()` of the
//! oracle); the engine follows ICU's `rbnf.cpp`, `nfrs.cpp`, `nfrule.cpp`
//! and `nfsubs.cpp` — rule descriptors with radix and exponent, the
//! bracketed optional text that makes two rules of one, the rollback
//! rule, the fraction rules picked by the locale's decimal separator,
//! fraction rule sets, the `<<`, `>>`, `>>>`, `==` substitutions over a
//! rule set or a `DecimalFormat` pattern, and `$(cardinal,…)$` /
//! `$(ordinal,…)$` plural text. Parsing text back is not done here.

mod data;

use std::rc::Rc;

use rphp_value::Value;

use crate::numfmt::{MsgNumber, MsgStyle};

/// The rule sets of the three `URBNFRuleSetTag`s php's MessageFormat uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Spellout,
    Ordinal,
    Duration,
}

// ICU's special base values
const NEGATIVE: i64 = -1;
const IMPROPER_FRACTION: i64 = -2;
const PROPER_FRACTION: i64 = -3;
const MASTER: i64 = -4;
const INFINITY: i64 = -5;
const NAN: i64 = -6;

/// `RECURSION_LIMIT`.
const RECURSION_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug)]
enum SubKind {
    Same,
    Multiplier,
    /// `rule_to_use`: the `>>>` substitution's predecessor rule.
    Modulus(Option<usize>),
    Integral,
    Fractional { by_digits: bool, use_spaces: bool },
    Absolute,
    Numerator { denominator: f64, with_zeros: bool },
}

#[derive(Clone, Debug)]
struct Sub {
    pos: usize,
    kind: SubKind,
    divisor: i64,
    set: Option<usize>,
    /// A `DecimalFormat` pattern's format (index into `formats`).
    fmt: Option<usize>,
}

#[derive(Clone, Debug)]
struct Plural {
    ordinal: bool,
    cases: Vec<(String, String)>,
    /// Character span of `$(…)$` in the rule text.
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct Rule {
    base: i64,
    radix: i64,
    exponent: i32,
    text: Vec<char>,
    subs: Vec<Sub>,
    plural: Option<Plural>,
    decimal_point: char,
}

impl Rule {
    fn new(base: i64) -> Rule {
        Rule { base, radix: 10, exponent: 0, text: Vec::new(), subs: Vec::new(), plural: None, decimal_point: '.' }
    }

    fn divisor(&self) -> i64 {
        pow(self.radix, self.exponent)
    }

    /// `setBaseValue`: the radix back to 10 and the exponent recomputed.
    fn set_base(&mut self, base: i64) {
        self.base = base;
        self.radix = 10;
        self.exponent = if base >= 1 { expected_exponent(base, 10) } else { 0 };
        let d = self.divisor();
        for s in &mut self.subs {
            s.divisor = d;
        }
    }

    fn should_roll_back(&self, number: i64) -> bool {
        if self.subs.iter().any(|s| matches!(s.kind, SubKind::Modulus(_))) {
            let re = self.divisor();
            return re != 0 && number % re == 0 && self.base % re != 0;
        }
        false
    }
}

#[derive(Clone, Debug, Default)]
struct RuleSet {
    name: String,
    public: bool,
    fraction: bool,
    rules: Vec<usize>,
    negative: Option<usize>,
    improper: Option<usize>,
    proper: Option<usize>,
    master: Option<usize>,
    infinity: Option<usize>,
    nan: Option<usize>,
}

fn pow(radix: i64, exponent: i32) -> i64 {
    let mut r: i64 = 1;
    for _ in 0..exponent.max(0) {
        r = r.saturating_mul(radix);
    }
    r
}

/// `NFRule::expectedExponent`.
fn expected_exponent(base: i64, radix: i64) -> i32 {
    if radix == 0 || base < 1 {
        return 0;
    }
    let mut e = ((base as f64).ln() / (radix as f64).ln()) as i32;
    if pow(radix, e + 1) <= base {
        e += 1;
    }
    e
}

/// `util64_fromDouble`: clamped to ±2^54, truncated toward zero.
fn from_double(d: f64) -> i64 {
    if d.is_nan() {
        return 0;
    }
    let mant = 18_014_398_509_481_984.0f64;
    let d = d.clamp(-mant, mant);
    let r = d.abs().floor() as i64;
    if d < 0.0 {
        -r
    } else {
        r
    }
}

/// `uprv_round`.
fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn lcm(a: i64, b: i64) -> i64 {
    let g = gcd(a, b);
    if g == 0 {
        0
    } else {
        a / g * b
    }
}

/// The parsed rules of one formatter.
struct Rules {
    sets: Vec<RuleSet>,
    rules: Vec<Rule>,
    formats: Vec<(String, MsgNumber)>,
    /// The locale's plain decimal format, for NaN and infinity.
    plain: MsgNumber,
    /// The locale the plural texts select for.
    locale: String,
    /// A format failed (the recursion limit): ICU's status, after which
    /// the number formats and plural texts put out nothing.
    failed: std::cell::Cell<bool>,
}

fn is_white(c: char) -> bool {
    (c as u32) < 0x10000 && crate::msgfmt::pattern::is_white(c as u16)
}

/// `RuleBasedNumberFormat::stripWhitespace`.
fn strip_whitespace(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut start = 0;
    while start < chars.len() {
        while start < chars.len() && is_white(chars[start]) {
            start += 1;
        }
        match chars[start..].iter().position(|&c| c == ';') {
            Some(p) => {
                out.extend(&chars[start..=start + p]);
                start += p + 1;
            }
            None => {
                out.extend(&chars[start..]);
                break;
            }
        }
    }
    out
}

struct Builder<'a> {
    locale: &'a str,
    decimal_sep: char,
    sets: Vec<RuleSet>,
    rules: Vec<Rule>,
    formats: Vec<(String, MsgNumber)>,
    wants_cardinal: bool,
    wants_ordinal: bool,
    /// The default rule set while parsing (a numerator's `<<`).
    default_set: usize,
}

impl Builder<'_> {
    fn find_set(&self, name: &str) -> Option<usize> {
        self.sets.iter().position(|s| s.name == name)
    }

    fn format_for(&mut self, pattern: &str) -> Option<usize> {
        if let Some(i) = self.formats.iter().position(|(p, _)| p == pattern) {
            return Some(i);
        }
        let f = MsgNumber::new(self.locale, MsgStyle::Pattern(pattern)).ok()?;
        self.formats.push((pattern.to_string(), f));
        Some(self.formats.len() - 1)
    }

    /// `NFRule::parseRuleDescriptor`: the rule with its base, radix and
    /// exponent, and the rule text after the descriptor.
    fn parse_descriptor(&self, text: &[char]) -> (Rule, Vec<char>) {
        let mut rule = Rule::new(0);
        let mut body: Vec<char> = text.to_vec();
        if let Some(colon) = text.iter().position(|&c| c == ':') {
            let descriptor: Vec<char> = text[..colon].to_vec();
            let mut p = colon + 1;
            while p < text.len() && is_white(text[p]) {
                p += 1;
            }
            body = text[p..].to_vec();
            let first = descriptor.first().copied().unwrap_or(' ');
            let last = descriptor.last().copied().unwrap_or(' ');
            if first.is_ascii_digit() && last != 'x' {
                let mut val: i64 = 0;
                let mut p = 0;
                let mut c = ' ';
                while p < descriptor.len() {
                    c = descriptor[p];
                    if c.is_ascii_digit() {
                        val = val.wrapping_mul(10).wrapping_add(i64::from(c as u8 - b'0'));
                    } else if c == '/' || c == '>' {
                        break;
                    }
                    p += 1;
                }
                rule.set_base(val);
                if c == '/' {
                    let mut v: i64 = 0;
                    p += 1;
                    while p < descriptor.len() {
                        c = descriptor[p];
                        if c.is_ascii_digit() {
                            v = v * 10 + i64::from(c as u8 - b'0');
                        } else if c == '>' {
                            break;
                        }
                        p += 1;
                    }
                    if v != 0 {
                        rule.radix = v;
                        rule.exponent = expected_exponent(rule.base, v);
                    }
                }
                if c == '>' {
                    while p < descriptor.len() {
                        if descriptor[p] == '>' && rule.exponent > 0 {
                            rule.exponent -= 1;
                        }
                        p += 1;
                    }
                }
            } else if descriptor == ['-', 'x'] {
                rule.base = NEGATIVE;
            } else if descriptor.len() == 3 {
                if first == '0' && last == 'x' {
                    rule.base = PROPER_FRACTION;
                    rule.decimal_point = descriptor[1];
                } else if first == 'x' && last == 'x' {
                    rule.base = IMPROPER_FRACTION;
                    rule.decimal_point = descriptor[1];
                } else if first == 'x' && last == '0' {
                    rule.base = MASTER;
                    rule.decimal_point = descriptor[1];
                } else if descriptor == ['N', 'a', 'N'] {
                    rule.base = NAN;
                } else if descriptor == ['I', 'n', 'f'] {
                    rule.base = INFINITY;
                }
            }
        }
        if body.first() == Some(&'\'') {
            body.remove(0);
        }
        (rule, body)
    }

    fn is_fraction_rule(base: i64) -> bool {
        matches!(base, IMPROPER_FRACTION | PROPER_FRACTION | MASTER)
    }

    /// `NFSubstitution::makeSubstitution`.
    fn make_sub(&mut self, pos: usize, rule: &Rule, predecessor: Option<usize>, owner: usize, token: &str) -> Option<Sub> {
        let chars: Vec<char> = token.chars().collect();
        let first = *chars.first()?;
        // the description without its token characters
        let inner: String = if chars.len() >= 2 && chars[0] == chars[chars.len() - 1] { chars[1..chars.len() - 1].iter().collect() } else { String::new() };
        let mut sub = Sub { pos, kind: SubKind::Same, divisor: rule.divisor(), set: None, fmt: None };
        let owner_fraction = self.sets[owner].fraction;
        let mut default_set = owner;
        match first {
            '<' => {
                if rule.base == NEGATIVE {
                    return None;
                }
                if Self::is_fraction_rule(rule.base) {
                    sub.kind = SubKind::Integral;
                } else if owner_fraction {
                    let with_zeros = token.ends_with("<<") && token.chars().count() > 2;
                    sub.kind = SubKind::Numerator { denominator: rule.base as f64, with_zeros };
                    default_set = self.default_set;
                } else {
                    sub.kind = SubKind::Multiplier;
                }
            }
            '>' => {
                if rule.base == NEGATIVE {
                    sub.kind = SubKind::Absolute;
                } else if Self::is_fraction_rule(rule.base) {
                    sub.kind = SubKind::Fractional { by_digits: false, use_spaces: true };
                } else if owner_fraction {
                    return None;
                } else {
                    let rule_to_use = if token == ">>>" { predecessor } else { None };
                    sub.kind = SubKind::Modulus(rule_to_use);
                }
            }
            '=' => sub.kind = SubKind::Same,
            _ => return None,
        }
        // the numerator's "<%set<<" names its set with one '<' trimmed
        let inner = if let SubKind::Numerator { with_zeros: true, .. } = sub.kind { inner.strip_suffix('<').unwrap_or(&inner).to_string() } else { inner };
        if inner.is_empty() {
            sub.set = Some(default_set);
        } else if inner.starts_with('%') {
            sub.set = self.find_set(&inner);
        } else if inner.starts_with('#') || inner.starts_with('0') {
            sub.fmt = self.format_for(&inner);
        } else if inner.starts_with('>') {
            sub.set = Some(owner);
        } else {
            return None;
        }
        if let SubKind::Fractional { .. } = sub.kind {
            if token == ">>" || token == ">>>" || sub.set == Some(owner) {
                sub.kind = SubKind::Fractional { by_digits: true, use_spaces: token != ">>>" };
            } else if let Some(s) = sub.set {
                self.sets[s].fraction = true;
            }
        }
        Some(sub)
    }

    /// `NFRule::extractSubstitution`: the first substitution token in the
    /// text, removed from it.
    fn extract_sub(&mut self, rule: &mut Rule, predecessor: Option<usize>, owner: usize) -> Option<Sub> {
        const PREFIXES: [&str; 11] = ["<<", "<%", "<#", "<0", ">>", ">%", ">#", ">0", "=%", "=#", "=0"];
        let text: String = rule.text.iter().collect();
        let mut start: Option<usize> = None;
        for p in PREFIXES {
            if let Some(i) = text.find(p) {
                let ci = text[..i].chars().count();
                if start.is_none_or(|s| ci < s) {
                    start = Some(ci);
                }
            }
        }
        let start = start?;
        let t = &rule.text;
        let end = if t.len() >= start + 3 && t[start] == '>' && t[start + 1] == '>' && t[start + 2] == '>' && find_from(t, &['>', '>', '>'], 0) == Some(start) {
            start + 2
        } else {
            let c = t[start];
            let mut e = t[start + 1..].iter().position(|&x| x == c).map(|p| p + start + 1)?;
            if c == '<' && e < t.len() - 1 && t[e + 1] == c {
                e += 1;
            }
            e
        };
        let token: String = t[start..=end].iter().collect();
        let snapshot = rule.clone();
        let sub = self.make_sub(start, &snapshot, predecessor, owner, &token);
        rule.text.drain(start..=end);
        sub
    }

    /// `NFRule::extractSubstitutions` and the plural text.
    fn finish_rule(&mut self, rule: &mut Rule, text: Vec<char>, predecessor: Option<usize>, owner: usize) {
        rule.text = text;
        rule.subs.clear();
        if let Some(s1) = self.extract_sub(rule, predecessor, owner) {
            rule.subs.push(s1);
            if let Some(s2) = self.extract_sub(rule, predecessor, owner) {
                rule.subs.push(s2);
            }
        }
        let t = &rule.text;
        if let Some(start) = find_from(t, &['$', '('], 0) {
            if let Some(end) = find_from(t, &[')', '$'], start) {
                let inner: String = t[start + 2..end].iter().collect();
                if let Some((ty, body)) = inner.split_once(',') {
                    let ordinal = ty.starts_with("ordinal");
                    if ordinal {
                        self.wants_ordinal = true;
                    } else {
                        self.wants_cardinal = true;
                    }
                    rule.plural = Some(Plural { ordinal, cases: parse_plural_cases(body), start, end: end + 2 });
                }
            }
        }
    }

    /// `NFRule::makeRules`: one rule, or two for bracketed text.
    fn make_rules(&mut self, desc: &[char], owner: usize, list: &mut Vec<usize>) {
        let (mut rule1, text) = self.parse_descriptor(desc);
        let predecessor = list.last().copied();
        let brack1 = text.iter().position(|&c| c == '[');
        let brack2 = brack1.and_then(|_| text.iter().position(|&c| c == ']'));
        let special = matches!(rule1.base, PROPER_FRACTION | NEGATIVE | INFINITY | NAN);
        let mut rule2: Option<Rule> = None;
        match (brack1, brack2) {
            (Some(b1), Some(b2)) if b1 < b2 && !special => {
                let or_else = text.iter().position(|&c| c == '|');
                let divisor = rule1.divisor();
                if (rule1.base > 0 && divisor != 0 && rule1.base % divisor == 0) || rule1.base == IMPROPER_FRACTION || rule1.base == MASTER {
                    let mut r2 = Rule::new(0);
                    if rule1.base >= 0 {
                        r2.base = rule1.base;
                        if !self.sets[owner].fraction {
                            rule1.base += 1;
                        }
                    } else if rule1.base == IMPROPER_FRACTION {
                        r2.base = PROPER_FRACTION;
                    } else if rule1.base == MASTER {
                        r2.base = rule1.base;
                        rule1.base = IMPROPER_FRACTION;
                    }
                    r2.radix = rule1.radix;
                    r2.exponent = rule1.exponent;
                    r2.decimal_point = rule1.decimal_point;
                    let mut sbuf: Vec<char> = text[..b1].to_vec();
                    if let Some(o) = or_else {
                        if o > b1 && o < b2 {
                            sbuf.extend(&text[o + 1..b2]);
                        }
                    }
                    sbuf.extend(&text[b2 + 1..]);
                    self.finish_rule(&mut r2, sbuf, predecessor, owner);
                    rule2 = Some(r2);
                }
                let mut sbuf: Vec<char> = text[..b1].to_vec();
                match or_else {
                    Some(o) if o > b1 && o < b2 => sbuf.extend(&text[b1 + 1..o]),
                    _ => sbuf.extend(&text[b1 + 1..b2]),
                }
                sbuf.extend(&text[b2 + 1..]);
                self.finish_rule(&mut rule1, sbuf, predecessor, owner);
            }
            _ => self.finish_rule(&mut rule1, text, predecessor, owner),
        }
        for r in rule2.into_iter().chain(std::iter::once(rule1)) {
            let idx = self.rules.len();
            let base = r.base;
            let dp = r.decimal_point;
            self.rules.push(r);
            if base >= 0 {
                list.push(idx);
            } else {
                let sep = self.decimal_sep;
                let set = &mut self.sets[owner];
                let slot = match base {
                    NEGATIVE => &mut set.negative,
                    IMPROPER_FRACTION => &mut set.improper,
                    PROPER_FRACTION => &mut set.proper,
                    MASTER => &mut set.master,
                    INFINITY => &mut set.infinity,
                    _ => &mut set.nan,
                };
                let fraction = matches!(base, IMPROPER_FRACTION | PROPER_FRACTION | MASTER);
                // setBestFractionRule: the one whose point is the locale's
                if !fraction || slot.is_none() || dp == sep {
                    *slot = Some(idx);
                }
            }
        }
    }

    /// `NFRuleSet::parseRules`.
    fn parse_rules(&mut self, owner: usize, body: &[char]) {
        let mut list: Vec<usize> = Vec::new();
        let mut old = 0;
        loop {
            let p = body[old..].iter().position(|&c| c == ';').map_or(body.len(), |p| p + old);
            let desc = &body[old..p];
            self.make_rules(desc, owner, &mut list);
            old = p + 1;
            if old >= body.len() {
                break;
            }
        }
        // the default base values
        let fraction = self.sets[owner].fraction;
        let mut default_base: i64 = 0;
        for &i in &list {
            let r = &mut self.rules[i];
            if r.base == 0 {
                r.set_base(default_base);
            } else {
                default_base = r.base;
            }
            if !fraction {
                default_base += 1;
            }
        }
        self.sets[owner].rules = list;
    }
}

/// `initDefaultRuleSet`: `%spellout-numbering`, `%digits-ordinal` or
/// `%duration`, else the last public rule set.
fn default_of(sets: &[RuleSet]) -> usize {
    for (i, s) in sets.iter().enumerate() {
        if s.name == "%spellout-numbering" || s.name == "%digits-ordinal" || s.name == "%duration" {
            return i;
        }
    }
    let last = sets.len().saturating_sub(1);
    if sets.get(last).is_some_and(|s| s.public) {
        return last;
    }
    (0..last).rev().find(|&i| sets[i].public).unwrap_or(last)
}

fn find_from(t: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || t.len() < needle.len() {
        return None;
    }
    (from..=t.len() - needle.len()).find(|&i| t[i..i + needle.len()] == *needle)
}

/// The `keyword{text}` pairs of a rule's plural pattern.
fn parse_plural_cases(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let k0 = i;
        while i < chars.len() && chars[i] != '{' {
            i += 1;
        }
        let key: String = chars[k0..i].iter().collect::<String>().trim().to_string();
        if i >= chars.len() {
            break;
        }
        i += 1;
        let t0 = i;
        let mut depth = 1;
        while i < chars.len() {
            match chars[i] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let text: String = chars[t0..i.min(chars.len())].iter().collect();
        out.push((key, text));
        i += 1;
    }
    out
}

/// Which rule applies: a real one, or the default NaN / infinity rule.
#[derive(Clone, Copy)]
enum Pick {
    Rule(usize),
    DefaultNan,
    DefaultInfinity,
}

impl Rules {
    fn parse(text: &str, locale: &str) -> Option<Rules> {
        let plain = MsgNumber::new(locale, MsgStyle::Decimal).ok()?;
        let decimal_sep = plain.decimal_symbol().chars().next().unwrap_or('.');
        let description = strip_whitespace(text);
        let chars: Vec<char> = description.chars().collect();
        // split at ";%"
        let mut pieces: Vec<Vec<char>> = Vec::new();
        let mut start = 0;
        let mut p = 0;
        while p + 1 < chars.len() {
            if chars[p] == ';' && chars[p + 1] == '%' && p >= 1 {
                pieces.push(chars[start..=p].to_vec());
                start = p + 1;
            }
            p += 1;
        }
        pieces.push(chars[start..].to_vec());
        let mut b = Builder { locale, decimal_sep, sets: Vec::new(), rules: Vec::new(), formats: Vec::new(), wants_cardinal: false, wants_ordinal: false, default_set: 0 };
        let mut bodies: Vec<Vec<char>> = Vec::new();
        for piece in pieces {
            let (name, body) = if piece.first() == Some(&'%') {
                let colon = piece.iter().position(|&c| c == ':')?;
                let mut pos = colon + 1;
                while pos < piece.len() && is_white(piece[pos]) {
                    pos += 1;
                }
                (piece[..colon].iter().collect::<String>(), piece[pos..].to_vec())
            } else {
                ("%default".to_string(), piece)
            };
            let public = !name.starts_with("%%");
            let name = name.strip_suffix("-noparse").map(str::to_string).unwrap_or(name);
            b.sets.push(RuleSet { name, public, ..Default::default() });
            bodies.push(body);
        }
        b.default_set = default_of(&b.sets);
        for (i, body) in bodies.iter().enumerate() {
            b.parse_rules(i, body);
        }
        Some(Rules { sets: b.sets, rules: b.rules, formats: b.formats, plain, locale: locale.to_string(), failed: std::cell::Cell::new(false) })
    }

    fn default_set(&self) -> usize {
        default_of(&self.sets)
    }

    fn find_normal_rule(&self, set: usize, number: i64) -> Option<Pick> {
        let s = &self.sets[set];
        if s.fraction {
            return self.find_fraction_rule(set, number as f64);
        }
        let mut number = number;
        if number < 0 {
            if let Some(r) = s.negative {
                return Some(Pick::Rule(r));
            }
            number = number.wrapping_neg();
        }
        let rules = &s.rules;
        let mut hi = rules.len();
        if hi > 0 {
            let mut lo = 0;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let b = self.rules[rules[mid]].base;
                if b == number {
                    return Some(Pick::Rule(rules[mid]));
                } else if b > number {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            if hi == 0 {
                return None;
            }
            let mut result = rules[hi - 1];
            if self.rules[result].should_roll_back(number) {
                if hi == 1 {
                    return None;
                }
                result = rules[hi - 2];
            }
            return Some(Pick::Rule(result));
        }
        s.master.map(Pick::Rule)
    }

    fn find_double_rule(&self, set: usize, number: f64) -> Option<Pick> {
        let s = &self.sets[set];
        if s.fraction {
            return self.find_fraction_rule(set, number);
        }
        if number.is_nan() {
            return Some(s.nan.map_or(Pick::DefaultNan, Pick::Rule));
        }
        let mut number = number;
        if number < 0.0 {
            if let Some(r) = s.negative {
                return Some(Pick::Rule(r));
            }
            number = -number;
        }
        if number.is_infinite() {
            return Some(s.infinity.map_or(Pick::DefaultInfinity, Pick::Rule));
        }
        if number != number.floor() {
            if number < 1.0 {
                if let Some(r) = s.proper {
                    return Some(Pick::Rule(r));
                }
            }
            if let Some(r) = s.improper {
                return Some(Pick::Rule(r));
            }
        }
        if let Some(r) = s.master {
            return Some(Pick::Rule(r));
        }
        self.find_normal_rule(set, from_double(number + 0.5))
    }

    /// `findFractionRuleSetRule`.
    fn find_fraction_rule(&self, set: usize, number: f64) -> Option<Pick> {
        let rules = &self.sets[set].rules;
        let first = *rules.first()?;
        let mut l = self.rules[first].base;
        for &r in &rules[1..] {
            l = lcm(l, self.rules[r].base);
        }
        if l == 0 {
            return Some(Pick::Rule(first));
        }
        let numerator = from_double(number * l as f64 + 0.5);
        let mut difference = i64::MAX;
        let mut winner = 0;
        for (i, &r) in rules.iter().enumerate() {
            let mut temp = numerator.wrapping_mul(self.rules[r].base) % l;
            if l - temp < temp {
                temp = l - temp;
            }
            if temp < difference {
                difference = temp;
                winner = i;
                if difference == 0 {
                    break;
                }
            }
        }
        if winner + 1 < rules.len() && self.rules[rules[winner + 1]].base == self.rules[rules[winner]].base {
            let n = self.rules[rules[winner]].base as f64 * number;
            if !(0.5..2.0).contains(&n) {
                winner += 1;
            }
        }
        Some(Pick::Rule(rules[winner]))
    }

    fn set_i64(&self, set: usize, number: i64, depth: usize) -> String {
        if depth >= RECURSION_LIMIT {
            self.failed.set(true);
            return String::new();
        }
        match self.find_normal_rule(set, number) {
            Some(Pick::Rule(r)) => self.rule_i64(r, number, depth + 1),
            Some(p) => self.default_text(p),
            None => String::new(),
        }
    }

    fn set_f64(&self, set: usize, number: f64, depth: usize) -> String {
        if depth >= RECURSION_LIMIT {
            self.failed.set(true);
            return String::new();
        }
        match self.find_double_rule(set, number) {
            Some(Pick::Rule(r)) => self.rule_f64(r, number, depth + 1),
            Some(p) => self.default_text(p),
            None => String::new(),
        }
    }

    fn default_text(&self, p: Pick) -> String {
        if self.failed.get() {
            return String::new();
        }
        let v = match p {
            Pick::DefaultNan => f64::NAN,
            _ => f64::INFINITY,
        };
        self.plain.format(&Value::Float(v)).unwrap_or_default()
    }

    fn plural_text(&self, p: &Plural, n: i32) -> String {
        if self.failed.get() {
            return String::new();
        }
        for (k, t) in &p.cases {
            if let Some(v) = k.strip_prefix('=') {
                if v.trim().parse::<f64>().ok() == Some(f64::from(n)) {
                    return t.clone();
                }
            }
        }
        let name = crate::msgfmt::plurals::select(&self.locale, p.ordinal, &crate::msgfmt::plurals::Operands::from_int(i64::from(n)));
        p.cases
            .iter()
            .find(|(k, _)| k == name)
            .or_else(|| p.cases.iter().find(|(k, _)| k == "other"))
            .map(|(_, t)| t.clone())
            .unwrap_or_default()
    }

    /// The rule text with its plural part resolved, and the shifted
    /// substitution positions.
    fn assemble(&self, r: &Rule, plural: Option<String>, parts: Vec<String>) -> String {
        let mut text: Vec<char> = r.text.clone();
        let mut shift: isize = 0;
        let mut pstart = usize::MAX;
        if let (Some(p), Some(plural)) = (&r.plural, plural) {
            let replaced: Vec<char> = plural.chars().collect();
            let span = p.end.min(text.len()) - p.start;
            shift = replaced.len() as isize - span as isize;
            text.splice(p.start..p.end.min(text.len()), replaced);
            pstart = p.start;
        }
        // the second substitution first, so the first one's place holds
        for (sub, s) in r.subs.iter().zip(parts).rev() {
            let mut at = sub.pos as isize;
            if sub.pos > pstart {
                at += shift;
            }
            let at = (at.max(0) as usize).min(text.len());
            text.splice(at..at, s.chars());
        }
        text.into_iter().collect()
    }

    fn rule_i64(&self, r: usize, number: i64, depth: usize) -> String {
        let rule = &self.rules[r];
        let d = rule.divisor().max(1);
        let plural = rule.plural.as_ref().map(|p| self.plural_text(p, (number / d) as i32));
        // the second substitution goes first (ICU inserts back to front)
        let mut parts: Vec<String> = rule.subs.iter().rev().map(|s| self.sub_i64(s, number, depth)).collect();
        parts.reverse();
        self.assemble(rule, plural, parts)
    }

    fn rule_f64(&self, r: usize, number: f64, depth: usize) -> String {
        let rule = &self.rules[r];
        let d = rule.divisor() as f64;
        let plural = rule.plural.as_ref().map(|p| {
            let v = if (0.0..1.0).contains(&number) { round(number * d) } else { number / d };
            self.plural_text(p, v as i32)
        });
        let mut parts: Vec<String> = rule.subs.iter().rev().map(|s| self.sub_f64(s, number, depth)).collect();
        parts.reverse();
        self.assemble(rule, plural, parts)
    }

    fn fmt_f64(&self, i: usize, v: f64) -> String {
        if self.failed.get() {
            return String::new();
        }
        self.formats[i].1.format(&Value::Float(v)).unwrap_or_default()
    }

    fn sub_i64(&self, s: &Sub, number: i64, depth: usize) -> String {
        let t = match s.kind {
            SubKind::Same | SubKind::Integral => number,
            SubKind::Multiplier => number / s.divisor.max(1),
            SubKind::Modulus(rule) => {
                let m = number % s.divisor.max(1);
                if let Some(r) = rule {
                    return self.rule_i64(r, m, depth);
                }
                m
            }
            SubKind::Absolute => number.wrapping_abs(),
            SubKind::Fractional { .. } => 0,
            SubKind::Numerator { denominator, .. } => from_double(round(number as f64 * denominator)),
        };
        if let Some(set) = s.set {
            return self.set_i64(set, t, depth);
        }
        if let Some(f) = s.fmt {
            if number <= 9_007_199_254_740_991 {
                let mut x = self.transform_f64(s, number as f64);
                if self.formats[f].1.max_frac() == 0 {
                    x = x.floor();
                }
                return self.fmt_f64(f, x);
            }
            if self.failed.get() {
                return String::new();
            }
            return self.formats[f].1.format(&Value::Int(t)).unwrap_or_default();
        }
        String::new()
    }

    fn transform_f64(&self, s: &Sub, n: f64) -> f64 {
        match s.kind {
            SubKind::Same => n,
            SubKind::Multiplier => {
                if s.set.is_some() {
                    (n / s.divisor as f64).floor()
                } else {
                    n / s.divisor as f64
                }
            }
            SubKind::Modulus(_) => n % s.divisor as f64,
            SubKind::Integral => n.floor(),
            SubKind::Fractional { .. } => n - n.floor(),
            SubKind::Absolute => n.abs(),
            SubKind::Numerator { denominator, .. } => round(n * denominator),
        }
    }

    fn sub_f64(&self, s: &Sub, number: f64, depth: usize) -> String {
        match s.kind {
            SubKind::Fractional { by_digits: true, use_spaces } => {
                let Some(set) = s.set else {
                    return String::new();
                };
                let digits = fraction_digits(number);
                if digits.is_empty() {
                    return self.set_i64(set, 0, depth);
                }
                let sep = if use_spaces { " " } else { "" };
                return digits.iter().map(|&d| self.set_i64(set, i64::from(d), depth)).collect::<Vec<_>>().join(sep);
            }
            SubKind::Numerator { denominator, with_zeros } => {
                let t = round(number * denominator);
                let long = from_double(t);
                let mut out = String::new();
                if let (true, Some(set)) = (with_zeros, s.set) {
                    let mut nf = long;
                    loop {
                        nf = nf.saturating_mul(10);
                        if (nf as f64) >= denominator {
                            break;
                        }
                        out.push_str(&self.set_i64(set, 0, depth));
                        out.push(' ');
                    }
                }
                match s.set {
                    Some(set) if t == long as f64 => out.push_str(&self.set_i64(set, long, depth)),
                    Some(set) => out.push_str(&self.set_f64(set, t, depth)),
                    None => {
                        if let Some(f) = s.fmt {
                            out.push_str(&self.fmt_f64(f, t));
                        }
                    }
                }
                return out;
            }
            SubKind::Modulus(Some(r)) => {
                let t = number % s.divisor as f64;
                return self.rule_f64(r, t, depth);
            }
            _ => {}
        }
        let t = self.transform_f64(s, number);
        if t.is_infinite() {
            if let Some(set) = s.set {
                return match self.find_double_rule(set, f64::INFINITY) {
                    Some(Pick::Rule(r)) => self.rule_f64(r, t, depth),
                    Some(p) => self.default_text(p),
                    None => String::new(),
                };
            }
        }
        match (s.set, s.fmt) {
            (Some(set), _) if t == t.floor() => self.set_i64(set, from_double(t), depth),
            (Some(set), _) => self.set_f64(set, t, depth),
            (None, Some(f)) => self.fmt_f64(f, t),
            _ => String::new(),
        }
    }
}

/// The fraction digits of a double as ICU's `DecimalQuantity` holds them
/// (shortest form, at most 20), most significant first.
fn fraction_digits(n: f64) -> Vec<u8> {
    let text = format!("{}", n.abs());
    let Some((_, frac)) = text.split_once('.') else {
        return Vec::new();
    };
    let mut d: Vec<u8> = frac.bytes().map(|b| b - b'0').collect();
    if d.len() > 20 {
        // round half-even at 20 digits
        let round_up = d[20] > 5 || (d[20] == 5 && (d[21..].iter().any(|&x| x != 0) || d[19] % 2 == 1));
        d.truncate(20);
        if round_up {
            let mut i = 20;
            while i > 0 {
                i -= 1;
                if d[i] == 9 {
                    d[i] = 0;
                } else {
                    d[i] += 1;
                    break;
                }
            }
        }
    }
    while d.last() == Some(&0) {
        d.pop();
    }
    d
}

/// A rule-based number format: the parsed rules and its default rule set.
#[derive(Clone)]
pub(crate) struct Rbnf {
    rules: Rc<Rules>,
    default: usize,
}

impl std::fmt::Debug for Rbnf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rbnf({})", self.rules.sets[self.default].name)
    }
}

impl Rbnf {
    /// The format of `kind` for a locale (ICU's bundle fallback).
    pub(crate) fn new(locale: &str, kind: Kind) -> Option<Rbnf> {
        let canonical = crate::locale::canonical(locale);
        let (_, row) = crate::numfmt_resolve(data::LOCALES, &canonical);
        let (texts, _actual) = data::ROWS[row];
        let text = data::RULES[usize::from(texts[kind as usize])];
        // the requested locale's symbols and plural rules, whichever
        // bundle the rules came from
        let rules = Rules::parse(text, &canonical)?;
        let default = rules.default_set();
        Some(Rbnf { rules: Rc::new(rules), default })
    }

    /// `setDefaultRuleSet`: a public rule set by name.
    pub(crate) fn set_default_rule_set(&mut self, name: &str) -> bool {
        if name.starts_with("%%") {
            return false;
        }
        match self.rules.sets.iter().position(|s| s.name == name) {
            Some(i) => {
                self.default = i;
                true
            }
            None => false,
        }
    }

    pub(crate) fn format_i64(&self, n: i64) -> String {
        self.rules.failed.set(false);
        self.rules.set_i64(self.default, n, 0)
    }

    pub(crate) fn format_f64(&self, n: f64) -> String {
        self.rules.failed.set(false);
        self.rules.set_f64(self.default, n, 0)
    }

    /// Parsing spelled-out text back is not implemented.
    pub(crate) fn parse_at(&self, _text: &str, _start: usize) -> Option<(Value, usize)> {
        None
    }
}
