//! ICU's number parser (`NumberParserImpl` as `DecimalFormat::parse`
//! drives it): a list of matchers tried in order at every position
//! until none advances — the affix pairs of the pattern, the currency,
//! NaN and infinity, padding and ignorables, the digits with their
//! grouping and decimal separators, the exponent — followed by the
//! validators (a number was seen; in strict mode a whole affix pair; a
//! currency when one was asked for). Strict mode is what php's
//! `parse()` runs unless `LENIENT_PARSE` is set: exact affixes, only
//! the locale's separators and their close equivalents, grouping sizes
//! that must match the pattern.

use super::decimal::Decimal;
use super::pattern::{tokens, Token};

/// One affix pair of the pattern as the parser sees it.
pub struct AffixPair {
    /// `None` for an empty affix; the parser then requires nothing there.
    pub prefix: Option<Vec<Token>>,
    pub suffix: Option<Vec<Token>>,
    pub negative: bool,
}

/// The formatter's parsing configuration.
pub struct Config<'a> {
    pub strict: bool,
    pub integer_only: bool,
    /// `parseCurrency()`: any currency of the locale may appear, and one
    /// must.
    pub parse_currency: bool,
    /// The pattern carries a currency sign: the currency matcher runs.
    pub has_currency: bool,
    pub affixes: Vec<AffixPair>,
    pub grouping_enabled: bool,
    /// Primary and secondary grouping sizes.
    pub grouping1: i32,
    pub grouping2: i32,
    pub decimal_sep: &'a str,
    pub grouping_sep: &'a str,
    /// The ten digit strings (custom ones through `setSymbol`).
    pub digits: &'a [String; 10],
    pub exponent_sep: &'a str,
    pub nan: &'a str,
    pub infinity: &'a str,
    pub percent: &'a str,
    pub permille: &'a str,
    pub minus: &'a str,
    pub plus: &'a str,
    /// The padding character when a width is set.
    pub padding: Option<&'a str>,
    /// The formatter's own currency: (symbol, ISO code).
    pub currency: (&'a str, &'a str),
    /// The formatter's own currency's long names (every plural form).
    pub own_names: Vec<String>,
    /// The other currencies `parseCurrency()` may match: (text, code,
    /// case-folded), symbols and codes, and in lenient mode long names.
    pub foreign: Vec<(String, String, bool)>,
    /// `%` / `‰` in the pattern (lenient mode matches them anywhere).
    pub pattern_percent: bool,
    pub pattern_permille: bool,
}

/// What a parse produced.
pub struct Parsed {
    pub value: Number,
    pub negative: bool,
    /// The characters consumed (php's position on success and its error
    /// index on failure).
    pub char_end: usize,
    pub currency: Option<String>,
    /// The parse succeeded.
    pub ok: bool,
}

#[derive(Clone, Debug)]
pub enum Number {
    Decimal(Decimal),
    Nan,
    Infinity,
    None,
}

// ---- the character classes ----------------------------------------------------------------------

const STRICT_COMMA: &[char] = &[',', '\u{066B}', '\u{FE10}', '\u{FE50}', '\u{FF0C}'];
const COMMA: &[char] = &[',', '\u{060C}', '\u{066B}', '\u{066C}', '\u{3001}', '\u{FE10}', '\u{FE11}', '\u{FE50}', '\u{FE51}', '\u{FF0C}', '\u{FF64}'];
const STRICT_PERIOD: &[char] = &['.', '\u{2024}', '\u{FE52}', '\u{FF0E}', '\u{FF61}'];
const PERIOD: &[char] = &['.', '\u{2024}', '\u{3002}', '\u{FE12}', '\u{FE52}', '\u{FF0E}', '\u{FF61}'];
const OTHER_GROUPING: &[char] = &[
    ' ', '\u{00A0}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{3000}', '\'', '\u{2019}',
];
/// Lenient grouping: every separator class at once.
const ALL_SEPARATORS: &[char] = &[
    ',', '\u{060C}', '\u{066B}', '\u{066C}', '\u{3001}', '\u{FE10}', '\u{FE11}', '\u{FE50}', '\u{FE51}', '\u{FF0C}', '\u{FF64}', '.', '\u{2024}', '\u{3002}', '\u{FE12}', '\u{FE52}', '\u{FF0E}', '\u{FF61}', ' ', '\u{00A0}', '\u{2000}', '\u{2001}',
    '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{3000}', '\'', '\u{2019}',
];
const MINUS_SIGNS: &[char] = &['-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2212}', '\u{207B}', '\u{208B}', '\u{FE63}', '\u{FF0D}', '\u{2796}'];
const PLUS_SIGNS: &[char] = &['+', '\u{FF0B}', '\u{FB29}', '\u{FE62}', '\u{207A}', '\u{208A}'];
const PERCENT_SIGNS: &[char] = &['%', '\u{066A}', '\u{FE6A}', '\u{FF05}'];
const PERMILLE_SIGNS: &[char] = &['‰', '\u{0609}'];
const BIDI_CONTROLS: &[char] = &['\u{200E}', '\u{200F}', '\u{061C}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'];

/// A separator class: the locale's own string, plus a set of
/// equivalents.
struct SepSet<'a> {
    own: &'a str,
    set: &'static [char],
}

impl SepSet<'_> {
    /// The length of the separator at the start of `s`, if any.
    fn matches(&self, s: &[char]) -> usize {
        let own: Vec<char> = self.own.chars().collect();
        if !own.is_empty() && s.starts_with(&own) {
            return own.len();
        }
        if s.first().is_some_and(|c| self.set.contains(c)) {
            return 1;
        }
        0
    }
}

/// `unisets::chooseFrom`: the equivalence class of a separator.
fn sep_class(sep: &str, strict: bool, grouping: bool) -> &'static [char] {
    let c = sep.chars().next().unwrap_or('\0');
    if sep.chars().count() == 1 {
        if COMMA.contains(&c) {
            return if strict { STRICT_COMMA } else { COMMA };
        }
        if PERIOD.contains(&c) {
            return if strict { STRICT_PERIOD } else { PERIOD };
        }
    }
    if grouping {
        OTHER_GROUPING
    } else {
        &[]
    }
}

fn digit_value(cfg: &Config, s: &[char]) -> Option<(u8, usize)> {
    // the locale's own digit strings first
    for (d, text) in cfg.digits.iter().enumerate() {
        let t: Vec<char> = text.chars().collect();
        if !t.is_empty() && s.starts_with(&t) {
            return Some((d as u8, t.len()));
        }
    }
    let c = *s.first()?;
    if let Some(d) = c.to_digit(10) {
        return Some((d as u8, 1));
    }
    crate::uchar::decimal_digit(c as u32).map(|d| (d as u8, 1))
}

/// Case-insensitive prefix match (ICU's parsers fold case), returning
/// the length consumed.
fn starts_with_fold(s: &[char], needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || s.len() < n.len() {
        return None;
    }
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    if s.iter().zip(n.iter()).all(|(a, b)| a == b || fold(*a) == fold(*b)) {
        Some(n.len())
    } else {
        None
    }
}

// ---- the parser ----------------------------------------------------------------------------------

struct State {
    /// Matched affix pair index, once a prefix was seen.
    prefix_of: Option<usize>,
    suffix_of: Option<usize>,
    seen_number: bool,
    seen_exponent: bool,
    number: Number,
    negative: bool,
    currency: Option<String>,
    char_end: usize,
    percent: bool,
    permille: bool,
}

/// Parse `text` from `start` (a char offset).
pub fn parse(cfg: &Config, text: &str, start: usize) -> Parsed {
    let chars: Vec<char> = text.chars().collect();
    let mut st = State {
        prefix_of: None,
        suffix_of: None,
        seen_number: false,
        seen_exponent: false,
        number: Number::None,
        negative: false,
        currency: None,
        char_end: start,
        percent: false,
        permille: false,
    };
    let mut pos = start.min(chars.len());
    let sorted_affixes = affix_order(cfg);
    // the greedy loop: the first matcher that advances restarts the list
    loop {
        if pos >= chars.len() {
            break;
        }
        let before = pos;
        let seg = &chars[pos..];
        // affix matchers
        for &ai in &sorted_affixes {
            let pair = &cfg.affixes[ai];
            if !st.seen_number {
                if st.prefix_of.is_some() {
                    break;
                }
                if let Some(prefix) = &pair.prefix {
                    if let Some(n) = match_affix(cfg, prefix, seg, &mut st) {
                        st.prefix_of = Some(ai);
                        pos += n;
                        st.char_end = pos;
                        break;
                    }
                }
            } else {
                if st.suffix_of.is_some() {
                    break;
                }
                if let Some(suffix) = &pair.suffix {
                    // the suffix must belong to the matched prefix's pair
                    let same_prefix = match st.prefix_of {
                        None => pair.prefix.is_none(),
                        Some(pi) => affix_eq(&cfg.affixes[pi].prefix, &pair.prefix),
                    };
                    if !same_prefix {
                        continue;
                    }
                    if let Some(n) = match_affix(cfg, suffix, seg, &mut st) {
                        st.suffix_of = Some(ai);
                        pos += n;
                        st.char_end = pos;
                        break;
                    }
                }
            }
        }
        if pos != before {
            continue;
        }
        // the currency on its own
        if (cfg.parse_currency || cfg.has_currency) && st.currency.is_none() {
            if let Some((n, code)) = match_currency(cfg, seg) {
                st.currency = Some(code);
                pos += n;
                st.char_end = pos;
                continue;
            }
        }
        // NaN, infinity
        if !st.seen_number {
            if let Some(n) = starts_with_fold(seg, cfg.nan) {
                st.number = Number::Nan;
                st.seen_number = true;
                pos += n;
                st.char_end = pos;
                continue;
            }
            if let Some(n) = starts_with_fold(seg, cfg.infinity) {
                st.number = Number::Infinity;
                st.seen_number = true;
                pos += n;
                st.char_end = pos;
                continue;
            }
        }
        // padding and ignorables (not counted as consumed)
        if let Some(pad) = cfg.padding {
            let p: Vec<char> = pad.chars().collect();
            if !p.is_empty() && seg.starts_with(&p) {
                pos += p.len();
                continue;
            }
        }
        if let Some(c) = seg.first() {
            let ignorable = BIDI_CONTROLS.contains(c) || (!cfg.strict && (c.is_whitespace() || *c == '\t'));
            if ignorable {
                pos += 1;
                continue;
            }
        }
        // the digits
        if !st.seen_number {
            if let Some((q, n)) = match_decimal(cfg, seg) {
                st.number = Number::Decimal(q);
                st.seen_number = true;
                pos += n;
                st.char_end = pos;
                continue;
            }
        }
        // the exponent
        if st.seen_number && !st.seen_exponent {
            if let Number::Decimal(q) = &mut st.number {
                if let Some((e, n)) = match_exponent(cfg, seg) {
                    q.shift(e);
                    st.seen_exponent = true;
                    pos += n;
                    st.char_end = pos;
                    continue;
                }
            }
        }
        // percent / permille anywhere, lenient mode only
        if !cfg.strict {
            if cfg.pattern_percent && !st.percent {
                if let Some(n) = match_sign(seg, cfg.percent, PERCENT_SIGNS) {
                    st.percent = true;
                    pos += n;
                    st.char_end = pos;
                    continue;
                }
            }
            if cfg.pattern_permille && !st.permille {
                if let Some(n) = match_sign(seg, cfg.permille, PERMILLE_SIGNS) {
                    st.permille = true;
                    pos += n;
                    st.char_end = pos;
                    continue;
                }
            }
            // a lone sign before the number in lenient mode
            if !st.seen_number {
                if let Some(n) = match_sign(seg, cfg.minus, MINUS_SIGNS) {
                    st.negative = true;
                    pos += n;
                    st.char_end = pos;
                    continue;
                }
                if let Some(n) = match_sign(seg, cfg.plus, PLUS_SIGNS) {
                    pos += n;
                    st.char_end = pos;
                    continue;
                }
            }
        }
        break;
    }

    // post-processing: which affix pair matched
    let mut whole_pair = false;
    for pair in &cfg.affixes {
        let prefix_ok = match st.prefix_of {
            None => pair.prefix.is_none(),
            Some(pi) => affix_eq(&cfg.affixes[pi].prefix, &pair.prefix),
        };
        let suffix_ok = match st.suffix_of {
            None => pair.suffix.is_none(),
            Some(si) => affix_eq(&cfg.affixes[si].suffix, &pair.suffix),
        };
        if prefix_ok && suffix_ok {
            whole_pair = true;
            if pair.negative {
                st.negative = true;
            }
            for t in pair.prefix.iter().chain(pair.suffix.iter()).flatten() {
                match t {
                    Token::Percent => st.percent = true,
                    Token::Permille => st.permille = true,
                    _ => {}
                }
            }
            break;
        }
    }

    let mut ok = st.seen_number;
    if cfg.strict && !whole_pair {
        ok = false;
    }
    if cfg.parse_currency && st.currency.is_none() {
        ok = false;
    }
    if ok {
        if let Number::Decimal(q) = &mut st.number {
            // (the pattern's multiplier, not the flag, scales a percentage)
            q.negative = st.negative;
        }
    }
    Parsed { value: st.number, negative: st.negative, char_end: st.char_end, currency: st.currency, ok }
}

/// Longer prefixes first, then longer suffixes (ICU sorts its affix
/// matchers so the most specific wins).
fn affix_order(cfg: &Config) -> Vec<usize> {
    let len = |a: &Option<Vec<Token>>| a.as_ref().map_or(0, |t| t.len());
    let mut idx: Vec<usize> = (0..cfg.affixes.len()).collect();
    idx.sort_by(|&a, &b| {
        let (pa, pb) = (&cfg.affixes[a], &cfg.affixes[b]);
        len(&pb.prefix).cmp(&len(&pa.prefix)).then(len(&pb.suffix).cmp(&len(&pa.suffix))).then(a.cmp(&b))
    });
    idx
}

fn affix_eq(a: &Option<Vec<Token>>, b: &Option<Vec<Token>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Match an affix's tokens in sequence; the length consumed.
fn match_affix(cfg: &Config, affix: &[Token], seg: &[char], st: &mut State) -> Option<usize> {
    let mut i = 0;
    for t in affix {
        let rest = &seg[i..];
        let n = match t {
            Token::Literal(c) => {
                if !cfg.strict && c.is_whitespace() {
                    // lenient: whitespace in the pattern is optional
                    let mut k = 0;
                    while k < rest.len() && rest[k].is_whitespace() {
                        k += 1;
                    }
                    k
                } else {
                    let fold = |x: char| x.to_lowercase().next().unwrap_or(x);
                    match rest.first() {
                        Some(x) if x == c || fold(*x) == fold(*c) => 1,
                        _ => return None,
                    }
                }
            }
            Token::Minus => match_sign(rest, cfg.minus, MINUS_SIGNS)?,
            Token::Plus => match_sign(rest, cfg.plus, PLUS_SIGNS)?,
            Token::Percent => match_sign(rest, cfg.percent, PERCENT_SIGNS)?,
            Token::Permille => match_sign(rest, cfg.permille, PERMILLE_SIGNS)?,
            Token::Currency(_) => {
                let (n, code) = match_currency(cfg, rest)?;
                if st.currency.is_none() {
                    st.currency = Some(code);
                }
                n
            }
        };
        i += n;
    }
    Some(i)
}

fn match_sign(seg: &[char], own: &str, set: &[char]) -> Option<usize> {
    let o: Vec<char> = own.chars().collect();
    if !o.is_empty() && seg.starts_with(&o) {
        return Some(o.len());
    }
    if seg.first().is_some_and(|c| set.contains(c)) {
        return Some(1);
    }
    None
}

/// The formatter's currency (symbol or code, case-folded), or with the
/// full data any currency of the locale; the longest match.
fn match_currency(cfg: &Config, seg: &[char]) -> Option<(usize, String)> {
    let mut best: Option<(usize, String)> = None;
    let mut consider = |n: usize, code: &str| {
        if n > 0 && best.as_ref().is_none_or(|(m, _)| n > *m) {
            best = Some((n, code.to_string()));
        }
    };
    if let Some(n) = starts_with_fold(seg, cfg.currency.0) {
        consider(n, cfg.currency.1);
    }
    if let Some(n) = starts_with_fold(seg, cfg.currency.1) {
        consider(n, cfg.currency.1);
    }
    for name in &cfg.own_names {
        if let Some(n) = starts_with_fold(seg, name) {
            consider(n, cfg.currency.1);
        }
    }
    if cfg.parse_currency {
        for (text, code, fold) in &cfg.foreign {
            if *fold {
                if let Some(n) = starts_with_fold(seg, text) {
                    consider(n, code);
                }
            } else {
                let t: Vec<char> = text.chars().collect();
                if !t.is_empty() && seg.starts_with(&t) {
                    consider(t.len(), code);
                }
            }
        }
    }
    best
}

/// The digits with their separators (`DecimalMatcher`): `None` when no
/// digit was read or strict grouping rejected the run.
fn match_decimal(cfg: &Config, seg: &[char]) -> Option<(Decimal, usize)> {
    let decimal = SepSet { own: cfg.decimal_sep, set: sep_class(cfg.decimal_sep, cfg.strict, false) };
    let grouping = SepSet { own: cfg.grouping_sep, set: if cfg.strict { sep_class(cfg.grouping_sep, true, true) } else { ALL_SEPARATORS } };
    let mut digits: Vec<u8> = Vec::new();
    let mut frac = 0i32;
    let mut i = 0usize;
    let mut seen_point = false;
    // each grouping separator: (digits before it, position before it)
    let mut seps: Vec<(usize, usize)> = Vec::new();
    let mut end_of_number = 0usize;
    // lenient: a leading grouping separator is skipped
    if !cfg.strict && cfg.grouping_enabled {
        let n = grouping.matches(seg);
        if n > 0 && digit_value(cfg, &seg[n..]).is_some() {
            i = n;
        }
    }
    loop {
        let rest = &seg[i..];
        if rest.is_empty() {
            break;
        }
        if let Some((d, n)) = digit_value(cfg, rest) {
            digits.push(d);
            if seen_point {
                frac += 1;
            }
            i += n;
            end_of_number = i;
            continue;
        }
        if digits.is_empty() && !seen_point && decimal.matches(rest) == 0 {
            break;
        }
        // the decimal separator
        let dn = decimal.matches(rest);
        if dn > 0 && !seen_point {
            if cfg.integer_only {
                break;
            }
            seen_point = true;
            i += dn;
            end_of_number = i;
            continue;
        }
        // a grouping separator followed by a digit
        let gn = grouping.matches(rest);
        if gn > 0 && !seen_point && cfg.grouping_enabled && !digits.is_empty() && digit_value(cfg, &rest[gn..]).is_some() {
            seps.push((digits.len(), i));
            i += gn;
            continue;
        }
        break;
    }
    if digits.is_empty() {
        return None;
    }
    let int_len = digits.len() - frac as usize;
    if !seps.is_empty() {
        // the size of every group: before the first separator, between,
        // after the last
        let first = seps[0].0;
        let mut between: Vec<usize> = Vec::new();
        for w in seps.windows(2) {
            between.push(w[1].0 - w[0].0);
        }
        let last = int_len - seps[seps.len() - 1].0;
        if cfg.strict {
            let g1 = cfg.grouping1.max(1) as usize;
            let g2 = if cfg.grouping2 > 0 { cfg.grouping2 as usize } else { g1 };
            if first > g2 || between.iter().any(|s| *s != g2) || last != g1 {
                return None;
            }
        } else {
            // lenient: a one-digit group is no group; the number ends
            // before the separator that opened it
            let mut after: Vec<usize> = between.clone();
            after.push(last);
            if let Some(k) = after.iter().position(|s| *s == 1) {
                let (count, at) = seps[k];
                let mut q = Decimal { negative: false, digits: digits[..count].to_vec(), scale: 0 };
                q.normalize();
                return Some((q, at));
            }
        }
    }
    let mut q = Decimal { negative: false, digits, scale: -frac };
    q.normalize();
    Some((q, end_of_number))
}

/// `E`, an optional sign, digits; nothing when no digit follows.
fn match_exponent(cfg: &Config, seg: &[char]) -> Option<(i32, usize)> {
    let n = starts_with_fold(seg, cfg.exponent_sep)?;
    let mut i = n;
    let mut negative = false;
    if let Some(k) = match_sign(&seg[i..], cfg.minus, MINUS_SIGNS) {
        negative = true;
        i += k;
    } else if let Some(k) = match_sign(&seg[i..], cfg.plus, PLUS_SIGNS) {
        i += k;
    }
    let mut e: i32 = 0;
    let mut any = false;
    while let Some((d, k)) = digit_value(cfg, &seg[i..]) {
        e = e.saturating_mul(10).saturating_add(i32::from(d));
        any = true;
        i += k;
    }
    if !any {
        return None;
    }
    Some((if negative { -e } else { e }, i))
}

/// The affix pairs of a pattern for the parser: the positive and the
/// negative pair, and in lenient mode their unpaired halves.
pub fn affix_pairs(pos_prefix: &str, pos_suffix: &str, neg_prefix: &str, neg_suffix: &str, strict: bool) -> Vec<AffixPair> {
    let tok = |s: &str| {
        let t = tokens(s);
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    let (pp, ps, np, ns) = (tok(pos_prefix), tok(pos_suffix), tok(neg_prefix), tok(neg_suffix));
    let mut pairs = vec![
        AffixPair { prefix: pp.clone(), suffix: ps.clone(), negative: false },
        AffixPair { prefix: np.clone(), suffix: ns.clone(), negative: true },
    ];
    if !strict {
        pairs.push(AffixPair { prefix: pp, suffix: None, negative: false });
        pairs.push(AffixPair { prefix: None, suffix: ps, negative: false });
        pairs.push(AffixPair { prefix: np, suffix: None, negative: true });
        pairs.push(AffixPair { prefix: None, suffix: ns, negative: true });
    }
    pairs
}
