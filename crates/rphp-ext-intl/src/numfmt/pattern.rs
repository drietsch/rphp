//! ICU's `DecimalFormat` pattern language: `prefix number suffix[;negative]`
//! with `0` `#` `@` digits, `,` grouping, `.` the decimal point, `E` the
//! exponent, `%` `‰` `¤` `-` `+` in the affixes, `'…'` quoting, `*c`
//! padding — read into the property bag a formatter runs on the way
//! `PatternParser::parseToExistingProperties` does, and written back from
//! it the way `PatternStringUtils::propertiesToPatternString` does, so
//! that `getPattern()` prints what php prints (`#,##0.###`, `¤#,##0.00`,
//! `pre'-'#,##0.###;-#,##0.###`, `*x##0.0`).

use super::decimal::Decimal;
use crate::state::{U_MALFORMED_EXPONENTIAL_PATTERN, U_MULTIPLE_PAD_SPECIFIERS, U_PATTERN_SYNTAX_ERROR, U_UNEXPECTED_TOKEN, U_UNQUOTED_SPECIAL};

/// Where padding goes (`NumberFormatter::PAD_*`).
pub const PAD_BEFORE_PREFIX: i64 = 0;
pub const PAD_AFTER_PREFIX: i64 = 1;
pub const PAD_BEFORE_SUFFIX: i64 = 2;
pub const PAD_AFTER_SUFFIX: i64 = 3;

/// ICU's cap on the digit counts an attribute may set.
pub const MAX_INT_FRAC_SIG: i32 = 999;
/// The digit count `toPattern()` writes at most (ICU's `dosMax`).
const DOS_MAX: i32 = 100;

/// `DecimalFormatProperties`: the pattern-derived and attribute-set
/// values, with ICU's `-1` for "unset" so that its mapping rules read
/// the same here.
#[derive(Clone, Debug)]
pub struct Props {
    /// The affixes as pattern text (`¤`, `%`, `‰`, `-`, `+` keep their
    /// meaning, quotes as written); `None` negative ones mean "minus
    /// before the positive prefix".
    pub pos_prefix_pattern: String,
    pub pos_suffix_pattern: String,
    pub neg_prefix_pattern: Option<String>,
    pub neg_suffix_pattern: Option<String>,
    /// Literal overrides set through `setTextAttribute`: used verbatim
    /// and escaped into the pattern by `toPattern()`.
    pub pos_prefix: Option<String>,
    pub pos_suffix: Option<String>,
    pub neg_prefix: Option<String>,
    pub neg_suffix: Option<String>,
    pub min_int: i32,
    pub max_int: i32,
    pub min_frac: i32,
    pub max_frac: i32,
    pub min_sig: i32,
    pub max_sig: i32,
    pub grouping_used: bool,
    pub grouping_size: i32,
    pub secondary_grouping_size: i32,
    /// An explicit `MULTIPLIER` that is not a power of ten.
    pub multiplier: i64,
    /// `%` (2) and `‰` (3), or a power-of-ten `MULTIPLIER`.
    pub magnitude_multiplier: i32,
    /// `0.05` in the number part or `ROUNDING_INCREMENT`; 0 for none.
    pub rounding_increment: f64,
    pub decimal_separator_always_shown: bool,
    /// `#¤#`: the currency symbol stands where the decimal point goes.
    pub currency_as_decimal: bool,
    pub exponent_sign_always_shown: bool,
    /// `E0`: the minimum exponent digits; -1 for a plain pattern.
    pub min_exponent_digits: i32,
    pub format_width: i32,
    /// The padding character; `None` until a pattern or attribute sets it.
    pub pad_string: Option<String>,
    /// `None` until a pattern's `*c` or `PADDING_POSITION` sets it:
    /// `toPattern()` writes the padding only then.
    pub pad_position: Option<i64>,
}

impl Default for Props {
    fn default() -> Self {
        Props {
            pos_prefix_pattern: String::new(),
            pos_suffix_pattern: String::new(),
            neg_prefix_pattern: None,
            neg_suffix_pattern: None,
            pos_prefix: None,
            pos_suffix: None,
            neg_prefix: None,
            neg_suffix: None,
            min_int: -1,
            max_int: -1,
            min_frac: -1,
            max_frac: -1,
            min_sig: -1,
            max_sig: -1,
            grouping_used: true,
            grouping_size: -1,
            secondary_grouping_size: -1,
            multiplier: 1,
            magnitude_multiplier: 0,
            rounding_increment: 0.0,
            decimal_separator_always_shown: false,
            currency_as_decimal: false,
            exponent_sign_always_shown: false,
            min_exponent_digits: -1,
            format_width: -1,
            pad_string: None,
            pad_position: None,
        }
    }
}

/// Whether a pattern's rounding (fraction digits, increment) is taken:
/// a currency formatter defers to the currency's digits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IgnoreRounding {
    Never,
    IfCurrency,
    Always,
}

// ---- affix tokens ---------------------------------------------------------------------------

/// One token of an affix pattern (`AffixUtils`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    Literal(char),
    Minus,
    Plus,
    Percent,
    Permille,
    /// `¤` … `¤¤¤¤¤`: the count.
    Currency(usize),
}

/// The tokens of an affix pattern; `''` is a literal quote.
pub fn tokens(affix: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let chars: Vec<char> = affix.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                if chars.get(i + 1) == Some(&'\'') {
                    out.push(Token::Literal('\''));
                    i += 2;
                    continue;
                }
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        if chars.get(i + 1) == Some(&'\'') {
                            out.push(Token::Literal('\''));
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    out.push(Token::Literal(chars[i]));
                    i += 1;
                }
                i += 1;
            }
            '¤' => {
                let mut n = 0;
                while i < chars.len() && chars[i] == '¤' {
                    n += 1;
                    i += 1;
                }
                out.push(Token::Currency(n));
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            '‰' => {
                out.push(Token::Permille);
                i += 1;
            }
            _ => {
                out.push(Token::Literal(c));
                i += 1;
            }
        }
    }
    out
}

/// `AffixUtils::estimateLength`: the width an affix pattern contributes
/// to the padding width.
pub fn estimate_length(affix: &str) -> i32 {
    tokens(affix)
        .iter()
        .map(|t| match t {
            Token::Currency(n) => *n as i32,
            Token::Literal(c) => c.len_utf16() as i32,
            _ => 1,
        })
        .sum()
}

pub fn has_currency(affix: &str) -> bool {
    tokens(affix).iter().any(|t| matches!(t, Token::Currency(_)))
}

pub fn has_token(affix: &str, which: Token) -> bool {
    tokens(affix).contains(&which)
}

/// `AffixUtils::escape`: literal text as pattern text, the special
/// characters quoted in runs.
pub fn escape(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in text.chars() {
        match c {
            '\'' => out.push_str("''"),
            '-' | '+' | '%' | '‰' | '¤' => {
                if !inside {
                    out.push('\'');
                    inside = true;
                }
                out.push(c);
            }
            _ => {
                if inside {
                    out.push('\'');
                    inside = false;
                }
                out.push(c);
            }
        }
    }
    if inside {
        out.push('\'');
    }
    out
}

// ---- parsing --------------------------------------------------------------------------------

/// `ParsedSubpatternInfo`.
#[derive(Default)]
struct Subpattern {
    prefix: String,
    suffix: String,
    padding_location: Option<i64>,
    padding_string: String,
    integer_numerals: i32,
    integer_at_signs: i32,
    integer_trailing_hash: i32,
    integer_total: i32,
    fraction_numerals: i32,
    fraction_hash: i32,
    fraction_total: i32,
    has_decimal: bool,
    has_currency_decimal: bool,
    width_excluding_affixes: i32,
    /// Three 16-bit fields as ICU packs them: the last group's size in
    /// the low bits, `0xffff` for a group never opened.
    grouping_sizes: u64,
    exponent_zeros: i32,
    exponent_plus: bool,
    /// The rounding increment's digits, once a nonzero digit appeared.
    rounding: Decimal,
    rounding_seen: bool,
}

struct Parser<'a> {
    chars: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn next(&mut self) {
        self.i += 1;
    }

    fn subpattern(&mut self) -> Result<Subpattern, i64> {
        let mut s = Subpattern { grouping_sizes: 0x0000_ffff_ffff_0000, ..Default::default() };
        self.padding(&mut s, PAD_BEFORE_PREFIX)?;
        s.prefix = self.affix()?;
        self.padding(&mut s, PAD_AFTER_PREFIX)?;
        self.integer_format(&mut s)?;
        if self.peek() == Some('.') {
            self.next();
            s.has_decimal = true;
            s.width_excluding_affixes += 1;
            self.fraction_format(&mut s)?;
        } else if self.peek() == Some('¤') && self.chars.get(self.i + 1).is_some_and(|c| matches!(c, '#' | '0'..='9')) {
            // `#¤#`: the currency symbol as the decimal separator
            self.next();
            s.has_decimal = true;
            s.has_currency_decimal = true;
            s.width_excluding_affixes += 1;
            self.fraction_format(&mut s)?;
        }
        self.exponent(&mut s)?;
        self.padding(&mut s, PAD_BEFORE_SUFFIX)?;
        s.suffix = self.affix()?;
        self.padding(&mut s, PAD_AFTER_SUFFIX)?;
        Ok(s)
    }

    fn padding(&mut self, s: &mut Subpattern, pos: i64) -> Result<(), i64> {
        if self.peek() != Some('*') {
            return Ok(());
        }
        if s.padding_location.is_some() {
            return Err(U_MULTIPLE_PAD_SPECIFIERS);
        }
        s.padding_location = Some(pos);
        self.next();
        let start = self.i;
        self.literal()?;
        s.padding_string = self.chars[start..self.i].iter().collect();
        Ok(())
    }

    /// One literal: a quoted run or a single character.
    fn literal(&mut self) -> Result<(), i64> {
        match self.peek() {
            None => Err(U_PATTERN_SYNTAX_ERROR),
            Some('\'') => {
                self.next();
                while self.peek() != Some('\'') {
                    if self.peek().is_none() {
                        return Err(U_PATTERN_SYNTAX_ERROR);
                    }
                    self.next();
                }
                self.next();
                Ok(())
            }
            Some(_) => {
                self.next();
                Ok(())
            }
        }
    }

    fn affix(&mut self) -> Result<String, i64> {
        let start = self.i;
        loop {
            match self.peek() {
                None | Some('#' | '@' | ';' | '*' | '.' | ',' | '0'..='9') => break,
                Some('¤' | '%' | '‰' | '+' | '-') => self.next(),
                Some('\'') => self.literal()?,
                Some(_) => self.next(),
            }
        }
        Ok(self.chars[start..self.i].iter().collect())
    }

    fn integer_format(&mut self, s: &mut Subpattern) -> Result<(), i64> {
        loop {
            match self.peek() {
                Some(',') => {
                    s.width_excluding_affixes += 1;
                    s.grouping_sizes <<= 16;
                }
                Some('#') => {
                    if s.integer_numerals > 0 {
                        return Err(U_UNEXPECTED_TOKEN);
                    }
                    s.width_excluding_affixes += 1;
                    s.grouping_sizes += 1;
                    if s.integer_at_signs > 0 {
                        s.integer_trailing_hash += 1;
                    }
                    s.integer_total += 1;
                }
                Some('@') => {
                    if s.integer_numerals > 0 || s.integer_trailing_hash > 0 {
                        return Err(U_UNEXPECTED_TOKEN);
                    }
                    s.width_excluding_affixes += 1;
                    s.grouping_sizes += 1;
                    s.integer_at_signs += 1;
                    s.integer_total += 1;
                }
                Some(c @ '0'..='9') => {
                    if s.integer_trailing_hash > 0 || s.integer_at_signs > 0 {
                        return Err(U_UNEXPECTED_TOKEN);
                    }
                    s.width_excluding_affixes += 1;
                    s.grouping_sizes += 1;
                    s.integer_numerals += 1;
                    s.integer_total += 1;
                    let d = c as u8 - b'0';
                    if d != 0 || s.rounding_seen {
                        s.rounding_seen = true;
                        s.rounding.digits.push(d);
                    }
                }
                _ => break,
            }
            self.next();
        }
        let g1 = s.grouping_sizes & 0xffff;
        let g2 = (s.grouping_sizes >> 16) & 0xffff;
        let g3 = (s.grouping_sizes >> 32) & 0xffff;
        if g1 == 0 && g2 != 0xffff {
            return Err(U_UNEXPECTED_TOKEN);
        }
        if g2 == 0 && g3 != 0xffff {
            return Err(U_PATTERN_SYNTAX_ERROR);
        }
        Ok(())
    }

    fn fraction_format(&mut self, s: &mut Subpattern) -> Result<(), i64> {
        let mut zeros = 0i32;
        loop {
            match self.peek() {
                Some('#') => {
                    s.width_excluding_affixes += 1;
                    s.fraction_hash += 1;
                    s.fraction_total += 1;
                }
                Some(c @ '0'..='9') => {
                    if s.fraction_hash > 0 {
                        return Err(U_UNEXPECTED_TOKEN);
                    }
                    s.width_excluding_affixes += 1;
                    s.fraction_numerals += 1;
                    s.fraction_total += 1;
                    let d = c as u8 - b'0';
                    if d == 0 {
                        zeros += 1;
                    } else {
                        for _ in 0..zeros {
                            s.rounding.digits.push(0);
                            s.rounding.scale -= 1;
                        }
                        s.rounding.digits.push(d);
                        s.rounding.scale -= 1;
                        s.rounding_seen = true;
                        zeros = 0;
                    }
                }
                _ => break,
            }
            self.next();
        }
        Ok(())
    }

    fn exponent(&mut self, s: &mut Subpattern) -> Result<(), i64> {
        if self.peek() != Some('E') {
            return Ok(());
        }
        if (s.grouping_sizes & 0xffff_0000) != 0xffff_0000 {
            return Err(U_MALFORMED_EXPONENTIAL_PATTERN);
        }
        self.next();
        s.width_excluding_affixes += 1;
        if self.peek() == Some('+') {
            self.next();
            s.exponent_plus = true;
            s.width_excluding_affixes += 1;
        }
        while self.peek() == Some('0') {
            self.next();
            s.exponent_zeros += 1;
            s.width_excluding_affixes += 1;
        }
        Ok(())
    }
}

/// Read `pattern` into `p`, replacing every property a pattern sets
/// (`DecimalFormat::applyPattern`); an empty pattern resets the bag. The
/// ICU error code on a bad pattern.
pub fn apply(p: &mut Props, pattern: &str, ignore: IgnoreRounding) -> Result<(), i64> {
    if pattern.is_empty() {
        *p = Props::default();
        return Ok(());
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut parser = Parser { chars: &chars, i: 0 };
    let positive = parser.subpattern()?;
    let mut negative = None;
    if parser.peek() == Some(';') {
        parser.next();
        if parser.peek().is_some() {
            negative = Some(parser.subpattern()?);
        }
    }
    if parser.peek().is_some() {
        return Err(U_UNQUOTED_SPECIAL);
    }
    let pos = &positive;
    let ignore_rounding = match ignore {
        IgnoreRounding::Never => false,
        IgnoreRounding::IfCurrency => {
            pos.has_currency_decimal
                || has_currency(&pos.prefix)
                || has_currency(&pos.suffix)
                || negative.as_ref().is_some_and(|n| has_currency(&n.prefix) || has_currency(&n.suffix))
        }
        IgnoreRounding::Always => true,
    };

    // grouping
    let g1 = (pos.grouping_sizes & 0xffff) as i32;
    let g2 = ((pos.grouping_sizes >> 16) & 0xffff) as i32;
    let g3 = ((pos.grouping_sizes >> 32) & 0xffff) as i32;
    if g2 != 0xffff {
        p.grouping_size = g1;
        p.grouping_used = true;
    } else {
        p.grouping_size = -1;
        p.grouping_used = false;
    }
    p.secondary_grouping_size = if g3 != 0xffff { g2 } else { -1 };

    // for backwards compatibility, the pattern emits at least one digit
    let (min_int, min_frac) = if pos.integer_total == 0 && pos.fraction_total > 0 {
        (0, pos.fraction_numerals.max(1))
    } else if pos.integer_numerals == 0 && pos.fraction_numerals == 0 {
        (1, 0)
    } else {
        (pos.integer_numerals, pos.fraction_numerals)
    };

    // rounding
    if pos.integer_at_signs > 0 {
        p.min_frac = -1;
        p.max_frac = -1;
        p.rounding_increment = 0.0;
        p.min_sig = pos.integer_at_signs;
        p.max_sig = pos.integer_at_signs + pos.integer_trailing_hash;
    } else {
        if ignore_rounding {
            p.min_frac = -1;
            p.max_frac = -1;
            p.rounding_increment = 0.0;
        } else {
            p.min_frac = min_frac;
            p.max_frac = pos.fraction_total;
            p.rounding_increment = if pos.rounding_seen { pos.rounding.to_f64() } else { 0.0 };
        }
        p.min_sig = -1;
        p.max_sig = -1;
    }
    p.decimal_separator_always_shown = pos.has_decimal && pos.fraction_total == 0;
    p.currency_as_decimal = pos.has_currency_decimal;

    // scientific notation
    if pos.exponent_zeros > 0 {
        p.exponent_sign_always_shown = pos.exponent_plus;
        p.min_exponent_digits = pos.exponent_zeros;
        if pos.integer_at_signs == 0 {
            p.min_int = pos.integer_numerals;
            p.max_int = pos.integer_total;
        } else {
            p.min_int = 1;
            p.max_int = -1;
        }
    } else {
        p.exponent_sign_always_shown = false;
        p.min_exponent_digits = -1;
        p.min_int = min_int;
        p.max_int = -1;
    }

    // padding
    if let Some(loc) = pos.padding_location {
        p.format_width = pos.width_excluding_affixes + estimate_length(&pos.prefix) + estimate_length(&pos.suffix);
        let raw: Vec<char> = pos.padding_string.chars().collect();
        p.pad_string = Some(match raw.len() {
            1 => raw.iter().collect(),
            2 => {
                if raw[0] == '\'' {
                    "'".to_string()
                } else {
                    raw.iter().collect()
                }
            }
            n => raw[1..n - 1].iter().collect(),
        });
        p.pad_position = Some(loc);
    } else {
        p.format_width = -1;
        p.pad_string = None;
        p.pad_position = None;
    }

    // affixes
    p.pos_prefix_pattern = pos.prefix.clone();
    p.pos_suffix_pattern = pos.suffix.clone();
    match &negative {
        Some(n) => {
            p.neg_prefix_pattern = Some(n.prefix.clone());
            p.neg_suffix_pattern = Some(n.suffix.clone());
        }
        None => {
            p.neg_prefix_pattern = None;
            p.neg_suffix_pattern = None;
        }
    }
    // (the literal overrides of `setTextAttribute` survive a new pattern)

    // the magnitude multiplier
    let all = [Some(&pos.prefix), Some(&pos.suffix), negative.as_ref().map(|n| &n.prefix), negative.as_ref().map(|n| &n.suffix)];
    if all.iter().flatten().any(|a| has_token(a, Token::Percent)) {
        p.magnitude_multiplier = 2;
    } else if all.iter().flatten().any(|a| has_token(a, Token::Permille)) {
        p.magnitude_multiplier = 3;
    } else {
        p.magnitude_multiplier = 0;
    }
    Ok(())
}

// ---- toPattern ------------------------------------------------------------------------------

/// The affix patterns `toPattern()` and the formatter use: the escaped
/// literal when one is set, else the pattern text; a missing negative
/// one is the minus sign before the positive prefix pattern.
pub struct AffixPatterns {
    pub pos_prefix: String,
    pub pos_suffix: String,
    pub neg_prefix: String,
    pub neg_suffix: String,
}

impl Props {
    pub fn affix_patterns(&self) -> AffixPatterns {
        let pos_prefix = self.pos_prefix.as_deref().map_or_else(|| self.pos_prefix_pattern.clone(), escape);
        let pos_suffix = self.pos_suffix.as_deref().map_or_else(|| self.pos_suffix_pattern.clone(), escape);
        let neg_prefix = match (&self.neg_prefix, &self.neg_prefix_pattern) {
            (Some(lit), _) => escape(lit),
            (None, Some(pat)) => pat.clone(),
            (None, None) => format!("-{}", self.pos_prefix_pattern),
        };
        let neg_suffix = match (&self.neg_suffix, &self.neg_suffix_pattern) {
            (Some(lit), _) => escape(lit),
            (None, Some(pat)) => pat.clone(),
            (None, None) => self.pos_suffix_pattern.clone(),
        };
        AffixPatterns { pos_prefix, pos_suffix, neg_prefix, neg_suffix }
    }

    /// Whether the affixes carry a currency sign (`hasCurrencySign`).
    pub fn has_currency_sign(&self) -> bool {
        let a = self.affix_patterns();
        self.currency_as_decimal || has_currency(&a.pos_prefix) || has_currency(&a.pos_suffix) || has_currency(&a.neg_prefix) || has_currency(&a.neg_suffix)
    }

    /// `getMultiplier()`: the explicit multiplier, else the power of ten.
    pub fn effective_multiplier(&self) -> i64 {
        if self.multiplier != 1 {
            self.multiplier
        } else if self.magnitude_multiplier != 0 {
            10i64.pow(self.magnitude_multiplier.unsigned_abs())
        } else {
            1
        }
    }

    /// `setMultiplier()`: a power of ten becomes a magnitude shift.
    pub fn set_multiplier(&mut self, mut n: i64) {
        if n == 0 {
            n = 1;
        }
        let mut delta = 0;
        let mut v = n;
        while v != 1 {
            delta += 1;
            let t = v / 10;
            if t * 10 != v {
                delta = -1;
                break;
            }
            v = t;
        }
        if delta != -1 {
            self.magnitude_multiplier = delta;
            self.multiplier = 1;
        } else {
            self.magnitude_multiplier = 0;
            self.multiplier = n;
        }
    }
}

/// `toPattern()`: the pattern the properties describe.
pub fn to_pattern(p: &Props) -> String {
    let dos = |v: i32| v.min(DOS_MAX);
    let grouping_size = dos(p.secondary_grouping_size);
    let first_grouping_size = dos(p.grouping_size);
    let min_int = dos(p.min_int);
    let max_int = dos(p.max_int);
    let min_frac = dos(p.min_frac);
    let max_frac = dos(p.max_frac);
    let min_sig = dos(p.min_sig);
    let max_sig = dos(p.max_sig);
    let always_show_decimal = p.decimal_separator_always_shown;
    let exponent_digits = dos(p.min_exponent_digits);
    let exponent_plus = p.exponent_sign_always_shown;
    let a = p.affix_patterns();
    let (ppp, psp) = (a.pos_prefix, a.pos_suffix);
    let (npp, nsp) = (a.neg_prefix, a.neg_suffix);

    let mut sb: Vec<char> = ppp.chars().collect();
    let mut after_prefix = sb.len();

    // grouping sizes
    let (g1, g2) = if !p.grouping_used {
        (0, 0)
    } else if grouping_size != -1 && first_grouping_size != -1 && grouping_size != first_grouping_size {
        (grouping_size, first_grouping_size)
    } else if grouping_size != -1 {
        (0, grouping_size)
    } else if first_grouping_size != -1 {
        (0, first_grouping_size)
    } else {
        (0, 0)
    };
    let grouping_length = g1 + g2 + 1;

    // the digits to put in the pattern
    let mut digits: Vec<char> = Vec::new();
    let mut digits_scale = 0i32;
    if max_sig != -1 {
        while (digits.len() as i32) < min_sig {
            digits.push('@');
        }
        while (digits.len() as i32) < max_sig {
            digits.push('#');
        }
    } else if p.rounding_increment != 0.0 && !ignore_rounding_increment(p.rounding_increment, max_frac) {
        if let Some(inc) = Decimal::from_f64(p.rounding_increment) {
            let frac_len = inc.fraction_length();
            digits_scale = -frac_len;
            digits = inc.digits.iter().map(|d| (b'0' + d) as char).collect();
            for _ in 0..inc.scale.max(0) {
                digits.push('0');
            }
            if digits.is_empty() {
                digits.push('0');
            }
        }
    }
    while (digits.len() as i32) + digits_scale < min_int {
        digits.insert(0, '0');
    }
    while -digits_scale < min_frac {
        digits.push('0');
        digits_scale -= 1;
    }

    // write the digits
    let mut m0 = grouping_length.max(digits.len() as i32 + digits_scale);
    m0 = if max_int != DOS_MAX { max_int.max(m0) - 1 } else { m0 - 1 };
    let m_n = if max_frac != DOS_MAX { (-max_frac).min(digits_scale) } else { digits_scale };
    let mut magnitude = m0;
    while magnitude >= m_n {
        let di = digits.len() as i32 + digits_scale - magnitude - 1;
        if di < 0 || di >= digits.len() as i32 {
            sb.push('#');
        } else {
            sb.push(digits[di as usize]);
        }
        if magnitude > 0 && magnitude == g2 {
            sb.push(',');
        } else if magnitude > g2 && g1 > 0 && (magnitude - g2) % g1 == 0 {
            sb.push(',');
        } else if magnitude == 0 && (always_show_decimal || m_n < 0) {
            sb.push(if p.currency_as_decimal { '¤' } else { '.' });
        }
        magnitude -= 1;
    }

    // exponential notation
    if exponent_digits != -1 {
        sb.push('E');
        if exponent_plus {
            sb.push('+');
        }
        for _ in 0..exponent_digits {
            sb.push('0');
        }
    }

    // suffix
    let mut before_suffix = sb.len();
    sb.extend(psp.chars());

    // padding
    if p.format_width > 0 {
        if let Some(loc) = p.pad_position {
            while p.format_width - sb.len() as i32 > 0 {
                sb.insert(after_prefix, '#');
                before_suffix += 1;
            }
            let pad = p.pad_string.clone().unwrap_or_else(|| " ".to_string());
            let mut spec = vec!['*'];
            if pad == "'" {
                spec.extend("''".chars());
            } else {
                spec.extend(pad.chars());
            }
            match loc {
                PAD_AFTER_PREFIX => {
                    sb.splice(after_prefix..after_prefix, spec.iter().copied());
                    after_prefix += spec.len();
                    before_suffix += spec.len();
                }
                PAD_BEFORE_SUFFIX => {
                    sb.splice(before_suffix..before_suffix, spec.iter().copied());
                }
                PAD_AFTER_SUFFIX => {
                    sb.extend(spec.iter());
                }
                _ => {
                    sb.splice(0..0, spec.iter().copied());
                    after_prefix += spec.len();
                    before_suffix += spec.len();
                }
            }
        }
    }

    // the negative subpattern, unless it is the default one
    if npp != format!("-{ppp}") || nsp != psp {
        let number: Vec<char> = sb[after_prefix..before_suffix].to_vec();
        sb.push(';');
        sb.extend(npp.chars());
        sb.extend(number);
        sb.extend(nsp.chars());
    }
    sb.into_iter().collect()
}

/// `PatternStringUtils::ignoreRoundingIncrement`: an increment finer
/// than the maximum fraction digits is not applied.
pub fn ignore_rounding_increment(increment: f64, max_frac: i32) -> bool {
    if increment == 0.0 {
        return true;
    }
    if max_frac < 0 {
        return false;
    }
    let frac = Decimal::from_f64(increment).map_or(0, |d| d.fraction_length());
    frac > max_frac
}
