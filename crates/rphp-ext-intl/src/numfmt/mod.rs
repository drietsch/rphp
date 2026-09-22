//! `NumberFormatter` and the `numfmt_*` functions: ICU's `DecimalFormat`
//! over the patterns and symbols php 8.5.10's ICU carries for every
//! locale (`data/numfmt.rs`, dumped from the oracle), the currency
//! symbols and fraction digits from the same dump (`data/currency.rs`)
//! and the currency plural names from ICU4X. A formatter holds the
//! property bag its pattern and attributes set (`pattern.rs`), the
//! symbols, and its own last error; each format resolves the bag into
//! the display settings the way `NumberPropertyMapper::oldToNew` does.
//!
//! What is here: the pattern-driven styles (DECIMAL, CURRENCY and its
//! ISO / plural / accounting / cash variants, PERCENT, SCIENTIFIC,
//! PATTERN_DECIMAL), formatting of ints and doubles under ICU's rounding
//! modes on decimal digits, significant digits, rounding increments,
//! padding, currency spacing, the two compact styles through ICU4X,
//! and ICU's strict and lenient parsers (`parse.rs`). The rule-based
//! styles (SPELLOUT, ORDINAL, DURATION, PATTERN_RULEBASED) have no
//! engine yet: their constructor fails with `U_UNSUPPORTED_ERROR`.

mod decimal;
mod parse;
mod pattern;

use icu_provider::prelude::*;
use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

use crate::data::numfmt::{LANGUAGES, LOCALES, REGION_CURRENCIES, ROWS};
use crate::locale;
use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{
    self, IntlError, U_FORMAT_INEXACT_ERROR, U_ILLEGAL_ARGUMENT_ERROR, U_INVALID_FORMAT_ERROR, U_INVARIANT_CONVERSION_ERROR, U_PARSE_ERROR, U_UNSUPPORTED_ERROR, U_USING_DEFAULT_WARNING_CODE,
};
use crate::{generated, locale_arg, opt_arg};
use decimal::{Decimal, ROUND_HALFEVEN};
use pattern::{IgnoreRounding, Props, Token, MAX_INT_FRAC_SIG};

// styles
const PATTERN_DECIMAL: i64 = 0;
const DECIMAL: i64 = 1;
const CURRENCY: i64 = 2;
const PERCENT: i64 = 3;
const SCIENTIFIC: i64 = 4;
const SPELLOUT: i64 = 5;
const ORDINAL: i64 = 6;
const DURATION: i64 = 7;
const PATTERN_RULEBASED: i64 = 9;
const CURRENCY_ISO: i64 = 10;
const CURRENCY_PLURAL: i64 = 11;
const CURRENCY_ACCOUNTING: i64 = 12;
const CASH_CURRENCY: i64 = 13;
const DECIMAL_COMPACT_SHORT: i64 = 14;
const DECIMAL_COMPACT_LONG: i64 = 15;
const CURRENCY_STANDARD: i64 = 16;

// format/parse types
const TYPE_DEFAULT: i64 = 0;
const TYPE_INT32: i64 = 1;
const TYPE_INT64: i64 = 2;
const TYPE_DOUBLE: i64 = 3;
const TYPE_CURRENCY: i64 = 4;

// attributes
const PARSE_INT_ONLY: i64 = 0;
const GROUPING_USED: i64 = 1;
const DECIMAL_ALWAYS_SHOWN: i64 = 2;
const MAX_INTEGER_DIGITS: i64 = 3;
const MIN_INTEGER_DIGITS: i64 = 4;
const INTEGER_DIGITS: i64 = 5;
const MAX_FRACTION_DIGITS: i64 = 6;
const MIN_FRACTION_DIGITS: i64 = 7;
const FRACTION_DIGITS: i64 = 8;
const MULTIPLIER: i64 = 9;
const GROUPING_SIZE: i64 = 10;
const ROUNDING_MODE: i64 = 11;
const ROUNDING_INCREMENT: i64 = 12;
const FORMAT_WIDTH: i64 = 13;
const PADDING_POSITION: i64 = 14;
const SECONDARY_GROUPING_SIZE: i64 = 15;
const SIGNIFICANT_DIGITS_USED: i64 = 16;
const MIN_SIGNIFICANT_DIGITS: i64 = 17;
const MAX_SIGNIFICANT_DIGITS: i64 = 18;
const LENIENT_PARSE: i64 = 19;

// text attributes
const POSITIVE_PREFIX: i64 = 0;
const POSITIVE_SUFFIX: i64 = 1;
const NEGATIVE_PREFIX: i64 = 2;
const NEGATIVE_SUFFIX: i64 = 3;
const PADDING_CHARACTER: i64 = 4;
const CURRENCY_CODE: i64 = 5;

// symbols
const SYM_DECIMAL: usize = 0;
const SYM_GROUPING: usize = 1;
const SYM_PERCENT: usize = 3;
const SYM_ZERO: usize = 4;
const SYM_MINUS: usize = 6;
const SYM_PLUS: usize = 7;
const SYM_CURRENCY: usize = 8;
const SYM_INTL_CURRENCY: usize = 9;
const SYM_MONETARY_DECIMAL: usize = 10;
const SYM_EXPONENTIAL: usize = 11;
const SYM_PERMILL: usize = 12;
const SYM_INFINITY: usize = 14;
const SYM_NAN: usize = 15;
const SYM_MONETARY_GROUPING: usize = 17;
/// `UNUM_ONE_DIGIT_SYMBOL`: the last symbol the C API exposes.
const SYM_ONE_DIGIT: usize = 18;

/// `getAttribute(MAX_INTEGER_DIGITS)` for "unlimited".
const MAX_INT_UNLIMITED: i64 = 2_000_000_000;
/// `NumberFormatter::ROUND_UNNECESSARY`.
const ROUND_UNNECESSARY: i64 = 7;

/// A formatter's state.
pub struct NumState {
    style: i64,
    /// The locale as given (ICU form).
    requested: String,
    valid: String,
    actual: String,
    symbols: [String; 18],
    /// The digit strings, `0` to `9`.
    digits: [String; 10],
    /// `setSymbol(CURRENCY_SYMBOL)` / `(INTL_CURRENCY_SYMBOL)`: the
    /// custom text wins over the currency's own until the currency
    /// changes.
    custom_currency_symbol: bool,
    custom_intl_symbol: bool,
    props: Props,
    /// `properties.currency`: set explicitly (`CURRENCY_CODE`, a
    /// `formatCurrency` clone); otherwise the locale's.
    currency: Option<String>,
    /// CASH_CURRENCY: the cash usage's digits and increments.
    cash: bool,
    /// CURRENCY_PLURAL.
    plural: bool,
    rounding_mode: i64,
    parse_int_only: bool,
    lenient: bool,
    pub err: IntlError,
}

/// ICU's bundle resolution over a sorted locale table: the id itself,
/// its likely-subtags form, then the prefixes of that; `en` when nothing
/// matches. Returns the bundle's name and its index.
fn resolve_in(table: &'static [(&'static str, u16)], canonical: &str) -> (String, usize) {
    let head = canonical.split('@').next().unwrap_or("").to_string();
    let find = |name: &str| table.binary_search_by(|(l, _)| (*l).cmp(name)).ok().map(|i| table[i].1 as usize);
    if let Some(i) = find(&head) {
        return (head, i);
    }
    let mut cand = locale::maximized(&head).unwrap_or_else(|| head.clone());
    loop {
        if let Some(i) = find(&cand) {
            return (cand, i);
        }
        match cand.rfind('_') {
            Some(i) => cand.truncate(i),
            None => break,
        }
    }
    let mut cand = head;
    loop {
        if let Some(i) = find(&cand) {
            return (cand, i);
        }
        match cand.rfind('_') {
            Some(i) => cand.truncate(i),
            None => break,
        }
    }
    ("en".to_string(), find("en").unwrap_or(0))
}

/// The number data row for a locale.
fn resolve_row(canonical: &str) -> (String, &'static crate::data::numfmt::NumRow) {
    let (name, i) = resolve_in(LOCALES, canonical);
    (name, &ROWS[i])
}

/// The currency of a locale (`ucurr_forLocale`): its `currency`
/// keyword, else its region's; `None` without one.
fn locale_currency(canonical: &str) -> Option<String> {
    if let Some(v) = locale::keyword(canonical, "currency") {
        return Some(v.to_ascii_uppercase());
    }
    let (bcp47, _) = locale::split(canonical);
    let region = bcp47.split('-').skip(1).find(|s| (s.len() == 2 && s.chars().all(|c| c.is_ascii_uppercase())) || (s.len() == 3 && s.chars().all(|c| c.is_ascii_digit())))?;
    REGION_CURRENCIES.iter().find(|(k, _)| *k == region).map(|(_, c)| (*c).to_string())
}

/// A currency's fraction digits and rounding increment (in units of the
/// last digit) for a usage.
fn currency_fraction(code: &str, cash: bool) -> (i32, i32) {
    use crate::data::currency::FRACTIONS;
    match FRACTIONS.binary_search_by(|(c, ..)| (*c).cmp(code)) {
        Ok(i) => {
            let (_, d, inc, cd, cinc) = FRACTIONS[i];
            if cash {
                (i32::from(cd), i32::from(cinc))
            } else {
                (i32::from(d), i32::from(inc))
            }
        }
        Err(_) => (2, 1),
    }
}

/// A currency's symbol in a locale (`ucurr_getName(UCURR_SYMBOL_NAME)`):
/// the oracle's table (`data/currency.rs`), the code itself when the
/// locale spells it so.
fn currency_symbol(canonical: &str, code: &str) -> String {
    use crate::data::currency::{LOCALES as CUR_LOCALES, MAPS};
    let (_, i) = resolve_in(CUR_LOCALES, canonical);
    let map = MAPS[i];
    match map.binary_search_by(|(c, _)| (*c).cmp(code)) {
        Ok(j) => map[j].1.to_string(),
        Err(_) => code.to_string(),
    }
}

/// Every (text, code) pair `parseCurrency()` may match in a locale: the
/// symbols and the ISO codes.
fn currency_symbols(canonical: &str) -> Vec<(String, String)> {
    use crate::data::currency::{CODES, LOCALES as CUR_LOCALES, MAPS};
    let (_, i) = resolve_in(CUR_LOCALES, canonical);
    let mut v: Vec<(String, String)> = MAPS[i].iter().map(|(c, s)| ((*s).to_string(), (*c).to_string())).collect();
    for c in CODES {
        v.push(((*c).to_string(), (*c).to_string()));
    }
    v
}

/// A currency's plural display name in a locale (`¤¤¤`) for the plural
/// category of the formatted number.
fn currency_plural_name(bcp47: &str, code: &str, operands: icu::plurals::PluralOperands) -> String {
    use icu_experimental::dimension::provider::currency::extended::CurrencyExtendedDataV1;
    let Ok(loc) = bcp47.parse::<icu::locale::Locale>() else {
        return code.to_string();
    };
    let upper = code.to_ascii_uppercase();
    let Ok(attrs) = DataMarkerAttributes::try_from_str(&upper) else {
        return code.to_string();
    };
    let Ok(rules) = icu::plurals::PluralRules::try_new_cardinal((&loc).into()) else {
        return code.to_string();
    };
    // the baked data has no fallback of its own: walk the locale's parents
    let mut chain: Vec<icu::locale::Locale> = vec![loc.clone()];
    let mut l = icu::locale::Locale::from(loc.id.clone());
    l.id.variants.clear();
    if l != loc {
        chain.push(l.clone());
    }
    if l.id.region.is_some() {
        l.id.region = None;
        chain.push(l.clone());
    }
    if l.id.script.is_some() {
        l.id.script = None;
        chain.push(l.clone());
    }
    chain.push(icu::locale::Locale::UNKNOWN);
    for cand in &chain {
        let data_locale = DataLocale::from(cand);
        let req = DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attrs, &data_locale),
            ..Default::default()
        };
        if let Ok(resp) = DataProvider::<CurrencyExtendedDataV1>::load(&icu_experimental::provider::Baked, req) {
            return resp.payload.get().get(operands.clone(), &rules).to_string();
        }
    }
    code.to_string()
}

/// Every plural form of a currency's long name in a locale, for the
/// parser.
fn currency_long_names(bcp47: &str, code: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sample in [1u64, 2, 3, 5, 11, 21, 100, 0] {
        let name = currency_plural_name(bcp47, code, icu::plurals::PluralOperands::from(sample));
        if name != code && !out.contains(&name) {
            out.push(name);
        }
    }
    let half = fixed_decimal::Decimal::try_from_str("0.5").map(|d| icu::plurals::PluralOperands::from(&d));
    if let Ok(op) = half {
        let name = currency_plural_name(bcp47, code, op);
        if name != code && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// How the digits are rounded (`Precision`).
#[derive(Clone, Debug)]
enum Precision {
    /// No rounding at all (`#E0`).
    Unlimited,
    Fraction { min: i32, max: i32 },
    Significant { min: i32, max: i32 },
    Increment { inc: Decimal, min_frac: i32 },
}

/// Scientific notation settings.
#[derive(Clone, Copy, Debug)]
struct Scientific {
    /// The exponent is a multiple of this (1 for plain scientific).
    interval: i32,
    /// `000.0E0`: always show that many integer digits.
    require_min_int: bool,
    min_int: i32,
    min_exponent_digits: i32,
    plus: bool,
}

/// The property bag resolved for one format (`oldToNew` and the
/// exported properties).
struct Resolved {
    use_currency: bool,
    currency: String,
    min_int: i32,
    max_int: i32,
    min_frac: i32,
    max_frac: i32,
    precision: Precision,
    rounding_increment: f64,
    scientific: Option<Scientific>,
}

impl NumState {
    /// A formatter for `locale` in `style`; the ICU error code for a
    /// style without an engine or a pattern that does not parse.
    fn new(requested: &str, style: i64, custom_pattern: Option<&str>) -> Result<Self, i64> {
        let canonical = locale::canonical(requested);
        let (valid, row) = resolve_row(&canonical);
        let actual = valid.clone();
        let pattern: String = match style {
            PATTERN_DECIMAL => custom_pattern.unwrap_or("").to_string(),
            DECIMAL => row.patterns[0].to_string(),
            // the compact styles run on ICU4X; their bag stays empty (`#`)
            DECIMAL_COMPACT_SHORT | DECIMAL_COMPACT_LONG => String::new(),
            CURRENCY | CURRENCY_STANDARD | CASH_CURRENCY => row.patterns[1].to_string(),
            CURRENCY_ISO => row.patterns[1].replace('¤', "¤¤"),
            CURRENCY_PLURAL => row.patterns[5].to_string(),
            PERCENT => row.patterns[2].to_string(),
            SCIENTIFIC => row.patterns[3].to_string(),
            CURRENCY_ACCOUNTING => row.patterns[4].to_string(),
            SPELLOUT | ORDINAL | DURATION | PATTERN_RULEBASED => return Err(U_UNSUPPORTED_ERROR),
            _ => return Err(U_UNSUPPORTED_ERROR),
        };
        let currency_style = matches!(style, CURRENCY | CURRENCY_STANDARD | CASH_CURRENCY | CURRENCY_ISO | CURRENCY_PLURAL | CURRENCY_ACCOUNTING);
        let mut props = Props::default();
        pattern::apply(&mut props, &pattern, if currency_style { IgnoreRounding::Always } else { IgnoreRounding::IfCurrency })?;
        let mut symbols: [String; 18] = std::array::from_fn(|i| row.symbols[i].to_string());
        // the symbols carry the locale's currency, or the placeholders
        match locale_currency(&canonical) {
            Some(code) => {
                symbols[SYM_CURRENCY] = currency_symbol(&canonical, &code);
                symbols[SYM_INTL_CURRENCY] = code;
            }
            None => {
                symbols[SYM_CURRENCY] = "¤".to_string();
                symbols[SYM_INTL_CURRENCY] = "¤¤".to_string();
            }
        }
        let zero = symbols[SYM_ZERO].chars().next().unwrap_or('0');
        let digits: [String; 10] = std::array::from_fn(|i| char::from_u32(zero as u32 + i as u32).unwrap_or('0').to_string());
        Ok(NumState {
            style,
            requested: requested.to_string(),
            valid,
            actual,
            symbols,
            digits,
            custom_currency_symbol: false,
            custom_intl_symbol: false,
            props,
            currency: None,
            cash: style == CASH_CURRENCY,
            plural: style == CURRENCY_PLURAL,
            rounding_mode: ROUND_HALFEVEN,
            parse_int_only: false,
            lenient: false,
            // the locale fallback leaves ICU's warning in the object
            err: IntlError { code: U_USING_DEFAULT_WARNING_CODE, msg: None },
        })
    }

    fn canonical(&self) -> String {
        locale::canonical(&self.requested)
    }

    fn bcp47(&self) -> String {
        locale::split(&self.requested).0
    }

    /// The currency the formatter is on: explicit, else the locale's,
    /// else `XXX`.
    fn resolved_currency(&self) -> String {
        self.currency.clone().or_else(|| locale_currency(&self.canonical())).unwrap_or_else(|| "XXX".to_string())
    }

    fn use_currency(&self) -> bool {
        self.currency.is_some() || self.plural || self.cash || self.props.has_currency_sign()
    }

    /// `CURRENCY_CODE`: the resolved currency when one is in use.
    fn currency_code(&self) -> String {
        if self.use_currency() {
            self.resolved_currency()
        } else {
            "XXX".to_string()
        }
    }

    /// `setCurrency()`: the code validated as ICU's `CurrencyUnit` does,
    /// and the symbols following it.
    fn set_currency(&mut self, code: &str) -> Result<(), i64> {
        let chars: Vec<char> = code.chars().collect();
        let iso = if chars.is_empty() {
            "XXX".to_string()
        } else if chars.len() < 3 {
            return Err(U_ILLEGAL_ARGUMENT_ERROR);
        } else if !chars[..3].iter().all(char::is_ascii) {
            return Err(U_INVARIANT_CONVERSION_ERROR);
        } else {
            chars[..3].iter().map(|c| c.to_ascii_uppercase()).collect()
        };
        self.currency = Some(iso.clone());
        self.symbols[SYM_CURRENCY] = currency_symbol(&self.canonical(), &iso);
        self.symbols[SYM_INTL_CURRENCY] = iso;
        self.custom_currency_symbol = false;
        self.custom_intl_symbol = false;
        Ok(())
    }

    // ---- resolution ------------------------------------------------------------------------

    /// `NumberPropertyMapper::oldToNew`: the display settings this
    /// property bag means.
    fn resolve(&self) -> Resolved {
        let p = &self.props;
        let use_currency = self.use_currency();
        let currency = self.resolved_currency();
        let mut min_int = p.min_int;
        let mut max_int = p.max_int;
        let mut min_frac = p.min_frac;
        let mut max_frac = p.max_frac;
        let mut min_sig = p.min_sig;
        let mut max_sig = p.max_sig;
        let explicit_frac = min_frac != -1 || max_frac != -1;
        let explicit_sig = min_sig != -1 || max_sig != -1;
        let (cur_digits, cur_inc) = currency_fraction(&currency, self.cash);
        if use_currency && (min_frac == -1 || max_frac == -1) {
            if min_frac == -1 && max_frac == -1 {
                min_frac = cur_digits;
                max_frac = cur_digits;
            } else if min_frac == -1 {
                min_frac = max_frac.min(cur_digits);
            } else {
                max_frac = min_frac.max(cur_digits);
            }
        }
        // validate: the minimum overrides the maximum when they conflict
        if min_int == 0 && max_frac != 0 {
            min_frac = min_frac.max(0);
            max_frac = if max_frac < 0 { -1 } else { max_frac.max(min_frac) };
            min_int = 0;
            max_int = if max_int < 0 || max_int > MAX_INT_FRAC_SIG { -1 } else { max_int };
        } else {
            min_frac = min_frac.max(0);
            max_frac = if max_frac < 0 { -1 } else { max_frac.max(min_frac) };
            min_int = if min_int <= 0 || min_int > MAX_INT_FRAC_SIG { 1 } else { min_int };
            max_int = if max_int < 0 {
                -1
            } else if max_int < min_int {
                min_int
            } else if max_int > MAX_INT_FRAC_SIG {
                -1
            } else {
                max_int
            };
        }
        let mut rounding_increment = p.rounding_increment;
        let precision = if self.cash && use_currency {
            // the cash usage: the currency's own digits and increment
            if cur_inc > 1 {
                let mut inc = Decimal::from_i64(i64::from(cur_inc));
                inc.shift(-cur_digits);
                rounding_increment = inc.to_f64();
                Precision::Increment { inc, min_frac: cur_digits }
            } else {
                Precision::Fraction { min: cur_digits, max: cur_digits }
            }
        } else if p.rounding_increment != 0.0 {
            if pattern::ignore_rounding_increment(p.rounding_increment, max_frac) {
                Precision::Fraction { min: min_frac, max: max_frac }
            } else {
                let inc = Decimal::from_f64(p.rounding_increment).unwrap_or_else(Decimal::zero);
                Precision::Increment { inc, min_frac }
            }
        } else if explicit_sig {
            min_sig = min_sig.clamp(1, MAX_INT_FRAC_SIG);
            max_sig = if max_sig < 0 { MAX_INT_FRAC_SIG } else { max_sig.max(min_sig).min(MAX_INT_FRAC_SIG) };
            Precision::Significant { min: min_sig, max: max_sig }
        } else if explicit_frac || use_currency {
            Precision::Fraction { min: min_frac, max: max_frac }
        } else {
            // nothing set (an empty pattern): six fraction digits
            Precision::Fraction { min: 0, max: 6 }
        };
        // scientific notation
        let mut scientific = None;
        let mut precision = precision;
        if p.min_exponent_digits != -1 {
            let (mut s_min, mut s_max) = (min_int, max_int);
            if s_max > 8 {
                s_max = -1;
                s_min = 1;
            } else if s_max > s_min && s_min > 1 {
                s_min = 1;
            }
            let interval = if s_max < 0 { 1 } else { s_max.max(1) };
            scientific = Some(Scientific {
                interval,
                require_min_int: s_max == s_min,
                min_int: s_min,
                min_exponent_digits: p.min_exponent_digits,
                plus: p.exponent_sign_always_shown,
            });
            if let Precision::Fraction { .. } = precision {
                // the rounding follows the pattern's raw digit counts
                let (max_int_, mut min_int_, min_frac_, max_frac_) = (p.max_int, p.min_int, p.min_frac, p.max_frac);
                if min_int_ == 0 && max_frac_ == 0 {
                    precision = Precision::Unlimited;
                } else if min_int_ == 0 && min_frac_ == 0 {
                    precision = Precision::Significant { min: 1, max: max_frac_ + 1 };
                } else {
                    let max_sig_ = min_int_ + max_frac_;
                    if max_int_ > min_int_ && min_int_ > 1 {
                        min_int_ = 1;
                    }
                    let min_sig_ = min_int_ + min_frac_;
                    precision = Precision::Significant { min: min_sig_, max: max_sig_ };
                }
            }
        }
        Resolved { use_currency, currency, min_int, max_int, min_frac, max_frac, precision, rounding_increment, scientific }
    }

    // ---- affixes ---------------------------------------------------------------------------

    /// The text of `¤`, `¤¤` and `¤¤¤` in an affix.
    fn currency_text(&self, r: &Resolved, count: usize, operands: Option<icu::plurals::PluralOperands>) -> String {
        match count {
            1 => {
                if self.custom_currency_symbol {
                    self.symbols[SYM_CURRENCY].clone()
                } else {
                    currency_symbol(&self.canonical(), &r.currency)
                }
            }
            2 => {
                if self.custom_intl_symbol {
                    self.symbols[SYM_INTL_CURRENCY].clone()
                } else {
                    r.currency.clone()
                }
            }
            3 => currency_plural_name(&self.bcp47(), &r.currency, operands.unwrap_or_else(|| icu::plurals::PluralOperands::from(2u64))),
            5 => currency_symbol(&self.canonical(), &r.currency),
            _ => "\u{FFFD}".to_string(),
        }
    }

    /// An affix pattern rendered: the special characters substituted by
    /// their symbols, quotes resolved; also whether the text ends
    /// (prefix) or starts (suffix) with the currency, for the spacing.
    fn render_affix(&self, r: &Resolved, affix: &str, operands: Option<icu::plurals::PluralOperands>) -> (String, bool, bool) {
        let toks = pattern::tokens(affix);
        let mut out = String::new();
        let mut first_len = 0;
        let mut last_len = 0;
        for t in &toks {
            let before = out.len();
            match t {
                Token::Literal(c) => out.push(*c),
                Token::Minus => out.push_str(&self.symbols[SYM_MINUS]),
                Token::Plus => out.push_str(&self.symbols[SYM_PLUS]),
                Token::Percent => out.push_str(&self.symbols[SYM_PERCENT]),
                Token::Permille => out.push_str(&self.symbols[SYM_PERMILL]),
                Token::Currency(n) => out.push_str(&self.currency_text(r, *n, operands.clone())),
            }
            if before == 0 {
                first_len = out.len();
            }
            last_len = out.len() - before;
        }
        // an empty currency text leaves no currency field to space
        let starts = matches!(toks.first(), Some(Token::Currency(_))) && first_len > 0;
        let ends = matches!(toks.last(), Some(Token::Currency(_))) && last_len > 0;
        (out, starts, ends)
    }

    /// The affixes for a sign: the literal overrides verbatim, else the
    /// patterns rendered. Each with its "currency adjacent" flag.
    fn affixes(&self, r: &Resolved, negative: bool, operands: Option<icu::plurals::PluralOperands>) -> ((String, bool), (String, bool)) {
        self.affixes_of(&self.props, r, negative, operands)
    }

    fn affixes_of(&self, props: &Props, r: &Resolved, negative: bool, operands: Option<icu::plurals::PluralOperands>) -> ((String, bool), (String, bool)) {
        let a = props.affix_patterns();
        let (prefix_lit, suffix_lit, prefix_pat, suffix_pat) = if negative {
            (&props.neg_prefix, &props.neg_suffix, a.neg_prefix, a.neg_suffix)
        } else {
            (&props.pos_prefix, &props.pos_suffix, a.pos_prefix, a.pos_suffix)
        };
        let prefix = match prefix_lit {
            Some(lit) => (lit.clone(), false),
            None => {
                let (text, _, ends) = self.render_affix(r, &prefix_pat, operands.clone());
                (text, ends)
            }
        };
        let suffix = match suffix_lit {
            Some(lit) => (lit.clone(), false),
            None => {
                let (text, starts, _) = self.render_affix(r, &suffix_pat, operands);
                (text, starts)
            }
        };
        (prefix, suffix)
    }

    /// `getTextAttribute(POSITIVE_PREFIX)` and friends: what an affix
    /// renders as.
    fn affix_text(&self, negative: bool, prefix: bool) -> String {
        let r = self.resolve();
        let (p, s) = self.affixes(&r, negative, None);
        if prefix {
            p.0
        } else {
            s.0
        }
    }

    // ---- formatting ------------------------------------------------------------------------

    fn digit_text(&self, d: u8) -> &str {
        &self.digits[usize::from(d.min(9))]
    }

    /// The digits of a rounded quantity with the separators and the
    /// integer width applied.
    fn render_number(&self, r: &Resolved, q: &Decimal, min_int: i32, min_frac: i32) -> String {
        let p = &self.props;
        let (dec_sep, grp_sep) = if r.use_currency {
            (&self.symbols[SYM_MONETARY_DECIMAL], &self.symbols[SYM_MONETARY_GROUPING])
        } else {
            (&self.symbols[SYM_DECIMAL], &self.symbols[SYM_GROUPING])
        };
        let mut int_digits = q.integer_digits();
        let mut frac_digits = q.fraction_digits();
        while frac_digits.len() < min_frac.max(0) as usize {
            frac_digits.push(0);
        }
        while (int_digits.len() as i32) < min_int {
            int_digits.insert(0, 0);
        }
        if r.max_int >= 0 && int_digits.len() as i32 > r.max_int {
            let cut = int_digits.len() - r.max_int as usize;
            int_digits.drain(..cut);
        }
        let mut out = String::new();
        let n = int_digits.len();
        let g1 = p.grouping_size;
        let g2 = if p.secondary_grouping_size > 0 { p.secondary_grouping_size } else { g1 };
        for (i, d) in int_digits.iter().enumerate() {
            let from_right = (n - i) as i32;
            if p.grouping_used && g1 > 0 && i > 0 {
                let boundary = from_right == g1 || (from_right > g1 && (from_right - g1) % g2 == 0);
                if boundary {
                    out.push_str(grp_sep);
                }
            }
            out.push_str(self.digit_text(*d));
        }
        if !frac_digits.is_empty() || p.decimal_separator_always_shown {
            if p.currency_as_decimal {
                out.push_str(&self.currency_text(r, 1, None));
            } else {
                out.push_str(dec_sep);
            }
            for d in &frac_digits {
                out.push_str(self.digit_text(*d));
            }
        }
        if out.is_empty() {
            out.push_str(self.digit_text(0));
        }
        out
    }

    /// Round `q` under the precision; the minimum fraction digits the
    /// display owes. `None` under `ROUND_UNNECESSARY` when rounding
    /// changed the value.
    fn apply_precision(&self, precision: &Precision, q: &mut Decimal) -> Option<i32> {
        let mut before = q.clone();
        before.normalize();
        let min_frac = match precision {
            Precision::Unlimited => 0,
            Precision::Fraction { min, max } => {
                if *max >= 0 {
                    q.round_at(-max, self.rounding_mode);
                }
                *min
            }
            Precision::Significant { min, max } => {
                q.round_sig(*max as usize, self.rounding_mode);
                let mag = if q.is_zero() { 0 } else { q.magnitude() };
                (min - 1 - mag).max(0)
            }
            Precision::Increment { inc, min_frac } => {
                q.round_to(inc, self.rounding_mode);
                *min_frac
            }
        };
        q.normalize();
        if self.rounding_mode == ROUND_UNNECESSARY {
            let mut after = q.clone();
            after.normalize();
            if after.digits != before.digits || after.scale != before.scale {
                return None;
            }
        }
        Some(min_frac)
    }

    /// Format a finite decimal quantity.
    fn format_decimal(&self, r: &Resolved, mut q: Decimal) -> Option<String> {
        let p = &self.props;
        // the multiplier
        if p.multiplier != 1 {
            q.multiply_by_i64(p.multiplier);
        }
        if p.magnitude_multiplier != 0 {
            q.shift(p.magnitude_multiplier);
        }
        let negative = q.negative;
        let mut body;
        let mut exponent: Option<i32> = None;
        let min_frac_shown;
        if let Some(sci) = &r.scientific {
            // scientific: shift so that the mantissa has the digits the
            // pattern asks for, rounding first so a carry keeps the range
            let mut min_frac_owed;
            if q.is_zero() {
                min_frac_owed = self.apply_precision(&r.precision, &mut q)?;
                if sci.require_min_int {
                    if let Precision::Significant { min, .. } = r.precision {
                        // "00.000E0" shows all its digits for zero
                        min_frac_owed = (min - sci.min_int).max(0);
                    }
                }
                exponent = Some(0);
            } else {
                let mult = |mag: i32| -> i32 {
                    let digits_shown = if sci.require_min_int {
                        sci.interval
                    } else if sci.interval <= 1 {
                        1
                    } else {
                        ((mag % sci.interval + sci.interval) % sci.interval) + 1
                    };
                    digits_shown - mag - 1
                };
                let mut m = mult(q.magnitude());
                q.shift(m);
                let mut probe = q.clone();
                min_frac_owed = self.apply_precision(&r.precision, &mut probe)?;
                if !probe.is_zero() && probe.magnitude() != q.magnitude() {
                    // the rounding carried into a new magnitude
                    let m2 = mult(probe.magnitude() - m);
                    q.shift(m2 - m);
                    m = m2;
                    probe = q.clone();
                    min_frac_owed = self.apply_precision(&r.precision, &mut probe)?;
                }
                q = probe;
                exponent = Some(-m);
            }
            let min_int_shown = if sci.require_min_int { sci.min_int.max(1) } else { 1 };
            min_frac_shown = min_frac_owed;
            body = self.render_number(r, &q, min_int_shown, min_frac_owed);
        } else {
            let min_frac = self.apply_precision(&r.precision, &mut q)?;
            min_frac_shown = min_frac;
            body = self.render_number(r, &q, r.min_int, min_frac);
        }
        if let (Some(e), Some(sci)) = (exponent, &r.scientific) {
            body.push_str(&self.symbols[SYM_EXPONENTIAL]);
            if e < 0 {
                body.push_str(&self.symbols[SYM_MINUS]);
            } else if sci.plus {
                body.push_str(&self.symbols[SYM_PLUS]);
            }
            let digits = e.unsigned_abs().to_string();
            for _ in digits.len()..sci.min_exponent_digits.max(1) as usize {
                body.push_str(self.digit_text(0));
            }
            for c in digits.chars() {
                body.push_str(self.digit_text(c as u8 - b'0'));
            }
        }
        // the plural form of `¤¤¤` follows the digits as displayed
        let operands = plural_operands(&q, min_frac_shown);
        let ((prefix, prefix_cur), (suffix, suffix_cur)) = self.affixes(r, negative, Some(operands));
        let (prefix, suffix) = self.currency_spacing(r, prefix, prefix_cur, suffix, suffix_cur, &body);
        Some(self.pad(prefix, body, suffix))
    }

    /// CLDR's currency spacing: a currency symbol that ends in a letter
    /// next to a digit gets a no-break space between them.
    fn currency_spacing(&self, r: &Resolved, mut prefix: String, prefix_cur: bool, mut suffix: String, suffix_cur: bool, body: &str) -> (String, String) {
        if !r.use_currency {
            return (prefix, suffix);
        }
        let is_symbol_char = |c: char| {
            let gc = icu::properties::CodePointMapData::<icu::properties::props::GeneralCategory>::new().get(c);
            matches!(
                gc,
                icu::properties::props::GeneralCategory::MathSymbol
                    | icu::properties::props::GeneralCategory::CurrencySymbol
                    | icu::properties::props::GeneralCategory::ModifierSymbol
                    | icu::properties::props::GeneralCategory::OtherSymbol
            )
        };
        // currencyMatch is `[[:^S:]&[:^Z:]]`, surroundingMatch `[:digit:]`
        let currency_match = |c: char| !is_symbol_char(c) && !c.is_whitespace();
        let is_digit = |c: char| c.is_ascii_digit() || crate::uchar::decimal_digit(c as u32).is_some();
        let body_first = body.chars().next().unwrap_or(' ');
        let body_last = body.chars().last().unwrap_or(' ');
        if prefix_cur {
            if let Some(last) = prefix.chars().last() {
                if currency_match(last) && is_digit(body_first) {
                    prefix.push('\u{A0}');
                }
            }
        }
        if suffix_cur {
            if let Some(first) = suffix.chars().next() {
                if currency_match(first) && is_digit(body_last) {
                    suffix.insert(0, '\u{A0}');
                }
            }
        }
        (prefix, suffix)
    }

    fn pad(&self, prefix: String, body: String, suffix: String) -> String {
        let p = &self.props;
        let width = p.format_width;
        let total = (prefix.chars().count() + body.chars().count() + suffix.chars().count()) as i32;
        if width <= 0 || total >= width {
            return format!("{prefix}{body}{suffix}");
        }
        let pad_char = p.pad_string.as_deref().and_then(|s| s.chars().next()).unwrap_or(' ');
        let pad: String = std::iter::repeat(pad_char).take((width - total) as usize).collect();
        match p.pad_position.unwrap_or(pattern::PAD_BEFORE_PREFIX) {
            pattern::PAD_AFTER_PREFIX => format!("{prefix}{pad}{body}{suffix}"),
            pattern::PAD_BEFORE_SUFFIX => format!("{prefix}{body}{pad}{suffix}"),
            pattern::PAD_AFTER_SUFFIX => format!("{prefix}{body}{suffix}{pad}"),
            _ => format!("{pad}{prefix}{body}{suffix}"),
        }
    }

    /// Format a value of either numeric kind; `None` on a rounding the
    /// mode forbids.
    fn format_value(&self, v: &Value) -> Option<String> {
        let r = self.resolve();
        match v {
            Value::Int(i) => self.format_decimal(&r, Decimal::from_i64(*i)),
            Value::Float(f) => self.format_f64(&r, *f),
            _ => None,
        }
    }

    fn format_f64(&self, r: &Resolved, f: f64) -> Option<String> {
        if f.is_nan() {
            let ((prefix, _), (suffix, _)) = self.affixes(r, false, None);
            return Some(format!("{prefix}{}{suffix}", self.symbols[SYM_NAN]));
        }
        if f.is_infinite() {
            let ((prefix, _), (suffix, _)) = self.affixes(r, f < 0.0, None);
            return Some(format!("{prefix}{}{suffix}", self.symbols[SYM_INFINITY]));
        }
        let q = Decimal::from_f64(f)?;
        self.format_decimal(r, q)
    }

    /// The compact styles through ICU4X (ICU rounds a compact number to
    /// two significant digits past the first, `1.2M`, `1.23K`).
    fn format_compact(&self, v: &Value) -> Option<String> {
        use icu::decimal::CompactDecimalFormatter;
        let loc: icu::locale::Locale = self.bcp47().parse().ok()?;
        let f = if self.style == DECIMAL_COMPACT_SHORT {
            CompactDecimalFormatter::try_new_short(loc.into(), Default::default()).ok()?
        } else {
            CompactDecimalFormatter::try_new_long(loc.into(), Default::default()).ok()?
        };
        let d = match v {
            Value::Int(i) => fixed_decimal::Decimal::from(*i),
            Value::Float(x) => {
                let q = Decimal::from_f64(*x)?;
                let mut text = String::new();
                if q.negative {
                    text.push('-');
                }
                text.push_str(&q.digits.iter().map(|d| (b'0' + d) as char).collect::<String>());
                if text.is_empty() || text == "-" {
                    text.push('0');
                }
                text.push('e');
                text.push_str(&q.scale.to_string());
                fixed_decimal::Decimal::try_from_str(&text).ok()?
            }
            _ => return None,
        };
        Some(f.format_to_string(&d))
    }

    /// A currency amount in a compact style: the mantissa rounded to the
    /// currency's digits (`€1.23K`), the locale's currency affixes around
    /// it.
    fn format_compact_currency(&self, r: &Resolved, amount: f64) -> Option<String> {
        use icu::decimal::CompactDecimalFormatter;
        if !amount.is_finite() {
            return self.format_f64(r, amount);
        }
        let loc: icu::locale::Locale = self.bcp47().parse().ok()?;
        // CLDR has compact currency patterns in the short form only
        let f = CompactDecimalFormatter::try_new_short(loc.into(), Default::default()).ok()?;
        let mut q = Decimal::from_f64(amount)?;
        let negative = q.negative;
        q.negative = false;
        let mag = if q.is_zero() { 0 } else { q.magnitude() };
        let exp = f.compact_exponent_for_magnitude(mag as i16);
        q.shift(-i32::from(exp));
        let (digits, _) = currency_fraction(&r.currency, self.cash);
        q.round_at(-digits, self.rounding_mode);
        if !q.is_zero() && q.magnitude() > mag - i32::from(exp) {
            // the rounding carried past the compact boundary
            let exp2 = f.compact_exponent_for_magnitude((q.magnitude() + i32::from(exp)) as i16);
            if exp2 != exp {
                q.shift(i32::from(exp) - i32::from(exp2));
                q.round_at(-digits, self.rounding_mode);
                return self.finish_compact_currency(r, &f, q, exp2, digits, negative);
            }
        }
        self.finish_compact_currency(r, &f, q, exp, digits, negative)
    }

    fn finish_compact_currency(&self, r: &Resolved, f: &icu::decimal::CompactDecimalFormatter, q: Decimal, exp: u8, digits: i32, negative: bool) -> Option<String> {
        let mut text: String = q.integer_digits().iter().map(|d| (b'0' + d) as char).collect();
        if text.is_empty() {
            text.push('0');
        }
        let mut frac = q.fraction_digits();
        while (frac.len() as i32) < digits {
            frac.push(0);
        }
        if !frac.is_empty() {
            text.push('.');
            text.extend(frac.iter().map(|d| (b'0' + d) as char));
        }
        let sig = fixed_decimal::Decimal::try_from_str(&text).ok()?;
        let body = writeable::Writeable::write_to_string(&f.format_with_exponent(&sig, exp).ok()?).into_owned();
        // the locale's currency pattern lends its affixes
        let (_, row) = resolve_row(&self.canonical());
        let mut props = Props::default();
        pattern::apply(&mut props, row.patterns[1], IgnoreRounding::Always).ok()?;
        let ((prefix, prefix_cur), (suffix, suffix_cur)) = self.affixes_of(&props, r, negative, None);
        let (prefix, suffix) = self.currency_spacing(r, prefix, prefix_cur, suffix, suffix_cur, &body);
        Some(format!("{prefix}{body}{suffix}"))
    }

    // ---- parsing ---------------------------------------------------------------------------

    /// Parse `text` from `start` with ICU's parser over this formatter's
    /// settings.
    fn parse_text(&self, text: &str, start: usize, parse_currency: bool) -> parse::Parsed {
        let r = self.resolve();
        let a = self.props.affix_patterns();
        let strict = !self.lenient;
        let affixes = parse::affix_pairs(&a.pos_prefix, &a.pos_suffix, &a.neg_prefix, &a.neg_suffix, strict);
        let (dec_sep, grp_sep) = if r.use_currency {
            (self.symbols[SYM_MONETARY_DECIMAL].as_str(), self.symbols[SYM_MONETARY_GROUPING].as_str())
        } else {
            (self.symbols[SYM_DECIMAL].as_str(), self.symbols[SYM_GROUPING].as_str())
        };
        let own_symbol = if self.custom_currency_symbol { self.symbols[SYM_CURRENCY].clone() } else { currency_symbol(&self.canonical(), &r.currency) };
        let own_names = currency_long_names(&self.bcp47(), &r.currency);
        let mut foreign: Vec<(String, String, bool)> = Vec::new();
        if parse_currency {
            for (text, code) in currency_symbols(&self.canonical()) {
                foreign.push((text, code, false));
            }
            if !strict {
                // lenient: every currency's long names
                for code in crate::data::currency::CODES {
                    for name in currency_long_names(&self.bcp47(), code) {
                        foreign.push((name, (*code).to_string(), true));
                    }
                }
            }
        }
        let all_affixes = [a.pos_prefix.as_str(), a.pos_suffix.as_str(), a.neg_prefix.as_str(), a.neg_suffix.as_str()];
        let cfg = parse::Config {
            strict,
            integer_only: self.parse_int_only,
            parse_currency,
            has_currency: self.props.has_currency_sign(),
            affixes,
            grouping_enabled: self.props.grouping_used && self.props.grouping_size > 0,
            grouping1: self.props.grouping_size,
            grouping2: self.props.secondary_grouping_size,
            decimal_sep: dec_sep,
            grouping_sep: grp_sep,
            digits: &self.digits,
            exponent_sep: &self.symbols[SYM_EXPONENTIAL],
            nan: &self.symbols[SYM_NAN],
            infinity: &self.symbols[SYM_INFINITY],
            percent: &self.symbols[SYM_PERCENT],
            permille: &self.symbols[SYM_PERMILL],
            minus: &self.symbols[SYM_MINUS],
            plus: &self.symbols[SYM_PLUS],
            padding: if self.props.format_width > 0 { Some(self.props.pad_string.as_deref().unwrap_or(" ")) } else { None },
            currency: (&own_symbol, &r.currency),
            own_names,
            foreign,
            pattern_percent: all_affixes.iter().any(|x| pattern::has_token(x, Token::Percent)),
            pattern_permille: all_affixes.iter().any(|x| pattern::has_token(x, Token::Permille)),
        };
        let mut parsed = parse::parse(&cfg, text, start);
        // the pattern's multiplier undone
        if parsed.ok {
            if let parse::Number::Decimal(q) = &mut parsed.value {
                if self.props.magnitude_multiplier != 0 {
                    q.shift(-self.props.magnitude_multiplier);
                } else if self.props.multiplier != 1 && self.props.multiplier != 0 {
                    let f = q.to_f64() / self.props.multiplier as f64;
                    *q = Decimal::from_f64(f).unwrap_or_else(Decimal::zero);
                }
            }
        }
        parsed
    }
}

/// The plural operands of a quantity as it displays: `1.00` is not `1`.
fn plural_operands(q: &Decimal, min_frac: i32) -> icu::plurals::PluralOperands {
    let mut text = String::new();
    let int: String = q.integer_digits().iter().map(|d| (b'0' + d) as char).collect();
    text.push_str(if int.is_empty() { "0" } else { &int });
    let mut frac = q.fraction_digits();
    while (frac.len() as i32) < min_frac {
        frac.push(0);
    }
    if !frac.is_empty() {
        text.push('.');
        text.extend(frac.iter().map(|d| (b'0' + d) as char));
    }
    match fixed_decimal::Decimal::try_from_str(&text) {
        Ok(d) => icu::plurals::PluralOperands::from(&d),
        Err(_) => icu::plurals::PluralOperands::from(2u64),
    }
}

// ---- object plumbing -------------------------------------------------------------------------

fn with_state<R>(o: &Object, f: impl FnOnce(&mut NumState) -> R) -> Option<R> {
    o.with_payload::<NumState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn state_of<'a>(ctx: &mut Ctx, o: &'a Object) -> Result<&'a Object, Unwind> {
    if o.with_payload::<NumState, _>(|_| ()).is_none() {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Object not initialized")?;
        return Err(Unwind::error("Object not initialized"));
    }
    Ok(o)
}

fn object_error(o: &Object, who: &str, code: i64, msg: &str) {
    with_state(o, |st| {
        st.err.code = code;
        st.err.msg = Some(format!("{who}(): {msg}"));
    });
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = src.with_payload::<NumState, _>(|s| NumState {
        style: s.style,
        requested: s.requested.clone(),
        valid: s.valid.clone(),
        actual: s.actual.clone(),
        symbols: s.symbols.clone(),
        digits: s.digits.clone(),
        custom_currency_symbol: s.custom_currency_symbol,
        custom_intl_symbol: s.custom_intl_symbol,
        props: s.props.clone(),
        currency: s.currency.clone(),
        cash: s.cash,
        plural: s.plural,
        rounding_mode: s.rounding_mode,
        parse_int_only: s.parse_int_only,
        lenient: s.lenient,
        err: IntlError::default(),
    }) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

/// The constructor's locale check (php 8.4+): the language must be one
/// ICU knows.
fn valid_language(ctx: &mut Ctx, requested: &str) -> Result<(), Unwind> {
    let canonical = locale::canonical(requested);
    let lang = canonical.split(['_', '@']).next().unwrap_or("");
    if lang.is_empty() || LANGUAGES.binary_search(&lang).is_err() {
        let who = ctx.active_function_name();
        return Err(Unwind::value_error(format!("{who}(): Argument #1 ($locale) \"{requested}\" is invalid")));
    }
    Ok(())
}

fn build(ctx: &mut Ctx, args: &[Value], who: &str) -> Result<Option<NumState>, Unwind> {
    state::reset_global(ctx);
    let requested = locale_arg(ctx, args, 0);
    valid_language(ctx, &requested)?;
    let style = args.get(1).map_or(DECIMAL, Value::to_int);
    let pattern = opt_arg(args, 2).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    match NumState::new(&requested, style, pattern.as_deref()) {
        Ok(st) => Ok(Some(st)),
        Err(code) => {
            state::set_global(ctx, who, code, "number formatter creation failed")?;
            Ok(None)
        }
    }
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match build(ctx, args, "NumberFormatter::__construct")? {
        Some(st) => {
            o.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Null)
        }
        // php 8.5: a constructor that cannot build throws
        None => Err(Unwind::exception("IntlException", "NumberFormatter::__construct(): number formatter creation failed")),
    }
}

fn create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    match build(ctx, args, "numfmt_create")? {
        Some(st) => {
            let cid = ctx.lookup_class_or_error(b"NumberFormatter")?;
            let obj = ctx.instantiate(cid);
            obj.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Object(obj))
        }
        None => Ok(Value::Null),
    }
}

// ---- formatting ----------------------------------------------------------------------------

/// `zval_get_long` of a formatter's number: php's modular cast of an
/// out-of-range float.
fn php_long(v: &Value) -> i64 {
    match v {
        Value::Float(f) => {
            if !f.is_finite() {
                0
            } else if (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(f) {
                *f as i64
            } else {
                let two64 = 18_446_744_073_709_551_616.0f64;
                (f.rem_euclid(two64) as u64) as i64
            }
        }
        other => other.to_int(),
    }
}

fn format(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let num = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Int(0));
    // `int|float`: a numeric string coerces
    let num = if matches!(num, Value::Str(_)) { num.to_number() } else { num };
    let ty = args.get(1).map_or(TYPE_DEFAULT, |v| v.deref().to_int());
    let value = match ty {
        TYPE_DEFAULT => match num {
            Value::Float(f) => Value::Float(f),
            other => Value::Int(other.to_int()),
        },
        TYPE_INT32 => Value::Int(i64::from(php_long(&num) as i32)),
        TYPE_INT64 => Value::Int(match num {
            // a C cast: saturating on this platform, 0 for NaN
            Value::Float(f) => {
                if f.is_nan() {
                    0
                } else {
                    f as i64
                }
            }
            other => other.to_int(),
        }),
        TYPE_DOUBLE => Value::Float(num.to_float()),
        TYPE_CURRENCY => {
            return Err(Unwind::value_error(format!(
                "{who}(): Argument #2 ($type) cannot be NumberFormatter::TYPE_CURRENCY constant, use NumberFormatter::formatCurrency() method instead"
            )))
        }
        _ => return Err(Unwind::value_error(format!("{who}(): Argument #2 ($type) must be a NumberFormatter::TYPE_* constant"))),
    };
    let out = with_state(o, |st| {
        st.err.reset();
        if matches!(st.style, DECIMAL_COMPACT_SHORT | DECIMAL_COMPACT_LONG) {
            st.format_compact(&value)
        } else {
            st.format_value(&value)
        }
    })
    .flatten();
    match out {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => {
            object_error(o, &who, U_FORMAT_INEXACT_ERROR, "Number formatting failed");
            Ok(Value::Bool(false))
        }
    }
}

fn format_currency(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let amount = args.first().map_or(0.0, |v| v.deref().to_float());
    let code = crate::text_arg(args, 1);
    let out = with_state(o, |st| {
        st.err.reset();
        let same = code.len() >= 3 && code.chars().take(3).map(|c| c.to_ascii_uppercase()).collect::<String>() == st.currency_code();
        let compact = matches!(st.style, DECIMAL_COMPACT_SHORT | DECIMAL_COMPACT_LONG);
        if same {
            // the formatter's own currency: formatted as is, custom
            // symbols included
            let r = st.resolve();
            return Ok(if compact { st.format_compact_currency(&r, amount) } else { st.format_f64(&r, amount) });
        }
        // another currency formats through a clone set to it
        let saved = (st.currency.clone(), st.symbols[SYM_CURRENCY].clone(), st.symbols[SYM_INTL_CURRENCY].clone(), st.custom_currency_symbol, st.custom_intl_symbol);
        let result = st.set_currency(&code).map(|()| {
            let r = st.resolve();
            if compact {
                st.format_compact_currency(&r, amount)
            } else {
                st.format_f64(&r, amount)
            }
        });
        st.currency = saved.0;
        st.symbols[SYM_CURRENCY] = saved.1;
        st.symbols[SYM_INTL_CURRENCY] = saved.2;
        st.custom_currency_symbol = saved.3;
        st.custom_intl_symbol = saved.4;
        result
    })
    .unwrap_or(Err(U_ILLEGAL_ARGUMENT_ERROR));
    match out {
        Ok(Some(s)) => Ok(Value::string(s.as_bytes())),
        Ok(None) => {
            object_error(o, &who, U_FORMAT_INEXACT_ERROR, "Number formatting failed");
            Ok(Value::Bool(false))
        }
        Err(code) => {
            object_error(o, &who, code, "Number formatting failed");
            Ok(Value::Bool(false))
        }
    }
}

// ---- parsing ------------------------------------------------------------------------------------

fn parse_impl(ctx: &mut Ctx, o: &Object, args: &mut [Value], ty: i64, pos_index: usize, parse_currency: bool) -> NativeResult {
    let who = ctx.active_function_name();
    let text = crate::text_arg(args, 0);
    let len = text.chars().count() as i64;
    let start = args.get(pos_index).map_or(0, |v| v.deref().to_int());
    // ICU: a start past the end parses nothing, silently
    if start < 0 || start > len {
        with_state(o, |st| st.err.reset());
        return Ok(match ty {
            TYPE_INT32 | TYPE_INT64 => Value::Int(0),
            _ => Value::Float(0.0),
        });
    }
    let parsed = with_state(o, |st| {
        st.err.reset();
        st.parse_text(&text, start as usize, parse_currency)
    });
    let Some(parsed) = parsed else {
        return Ok(Value::Bool(false));
    };
    if args.len() > pos_index {
        args[pos_index] = Value::Int(parsed.char_end as i64);
    }
    if !parsed.ok {
        object_error(o, &who, U_PARSE_ERROR, "Number parsing failed");
        return Ok(Value::Bool(false));
    }
    if parse_currency {
        if let Some(code) = &parsed.currency {
            if args.len() > 1 {
                args[1] = Value::string(code.as_bytes());
            }
        }
    }
    let fail = |o: &Object| {
        object_error(o, &who, U_INVALID_FORMAT_ERROR, "Number parsing failed");
        Value::Bool(false)
    };
    Ok(match (&parsed.value, ty) {
        (parse::Number::Nan, TYPE_INT32 | TYPE_INT64) => Value::Int(0),
        (parse::Number::Nan, _) => Value::Float(f64::NAN),
        (parse::Number::Infinity, TYPE_INT32 | TYPE_INT64) => fail(o),
        (parse::Number::Infinity, _) => Value::Float(if parsed.negative { f64::NEG_INFINITY } else { f64::INFINITY }),
        (parse::Number::Decimal(q), TYPE_INT32) => match q.to_i64() {
            Some(v) if i32::try_from(v).is_ok() => Value::Int(v),
            _ => fail(o),
        },
        (parse::Number::Decimal(q), TYPE_INT64) => match q.to_i64() {
            Some(v) => Value::Int(v),
            None => fail(o),
        },
        (parse::Number::Decimal(q), _) => Value::Float(q.to_f64()),
        (parse::Number::None, _) => fail(o),
    })
}

fn parse(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let ty = args.get(1).map_or(TYPE_DOUBLE, |v| v.deref().to_int());
    match ty {
        TYPE_INT32 | TYPE_INT64 | TYPE_DOUBLE => {}
        TYPE_CURRENCY => {
            return Err(Unwind::value_error(format!(
                "{who}(): Argument #2 ($type) cannot be NumberFormatter::TYPE_CURRENCY constant, use NumberFormatter::parseCurrency() method instead"
            )))
        }
        _ => return Err(Unwind::value_error(format!("{who}(): Argument #2 ($type) must be a NumberFormatter::TYPE_* constant"))),
    }
    parse_impl(ctx, o, args, ty, 2, false)
}

fn parse_currency(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    parse_impl(ctx, o, args, TYPE_DOUBLE, 2, true)
}

// ---- attributes ---------------------------------------------------------------------------------

fn get_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let id = args.first().map_or(-1, Value::to_int);
    let v = with_state(o, |st| {
        st.err.reset();
        let p = &st.props;
        let r = st.resolve();
        // ICU reports -1 for an unset value; php turns that into an error
        let or_unset = |v: i64| if v < 0 { None } else { Some(Value::Int(v)) };
        match id {
            PARSE_INT_ONLY => Some(Value::Int(i64::from(st.parse_int_only))),
            GROUPING_USED => Some(Value::Int(i64::from(p.grouping_used))),
            DECIMAL_ALWAYS_SHOWN => Some(Value::Int(i64::from(p.decimal_separator_always_shown))),
            MAX_INTEGER_DIGITS => Some(Value::Int(if r.max_int < 0 { MAX_INT_UNLIMITED } else { i64::from(r.max_int) })),
            MIN_INTEGER_DIGITS | INTEGER_DIGITS => Some(Value::Int(i64::from(r.min_int))),
            MAX_FRACTION_DIGITS => Some(Value::Int(i64::from(r.max_frac.max(0)))),
            MIN_FRACTION_DIGITS | FRACTION_DIGITS => Some(Value::Int(i64::from(r.min_frac))),
            MULTIPLIER => Some(Value::Int(p.effective_multiplier())),
            GROUPING_SIZE => Some(Value::Int(i64::from(p.grouping_size.max(0)))),
            ROUNDING_MODE => or_unset(st.rounding_mode),
            ROUNDING_INCREMENT => Some(Value::Float(r.rounding_increment)),
            FORMAT_WIDTH => or_unset(i64::from(p.format_width)),
            PADDING_POSITION => or_unset(p.pad_position.unwrap_or(pattern::PAD_BEFORE_PREFIX)),
            SECONDARY_GROUPING_SIZE => Some(Value::Int(i64::from(p.secondary_grouping_size.max(0)))),
            SIGNIFICANT_DIGITS_USED => Some(Value::Int(i64::from(p.min_sig != -1 || p.max_sig != -1))),
            MIN_SIGNIFICANT_DIGITS => or_unset(i64::from(p.min_sig)),
            MAX_SIGNIFICANT_DIGITS => or_unset(i64::from(p.max_sig)),
            LENIENT_PARSE => Some(Value::Int(i64::from(st.lenient))),
            _ => None,
        }
    })
    .flatten();
    match v {
        Some(v) => Ok(v),
        None => {
            object_error(o, &who, U_UNSUPPORTED_ERROR, "Error getting attribute value");
            Ok(Value::Bool(false))
        }
    }
}

fn set_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let id = args.first().map_or(-1, Value::to_int);
    let value = args.get(1).map(|v| v.deref().into_owned()).unwrap_or(Value::Int(0));
    let n = value.to_int();
    let clamp = |v: i64| -> i32 { v.clamp(-1, i64::from(MAX_INT_FRAC_SIG)) as i32 };
    let ok = with_state(o, |st| {
        st.err.reset();
        let p = &mut st.props;
        match id {
            PARSE_INT_ONLY => st.parse_int_only = n != 0,
            GROUPING_USED => p.grouping_used = n != 0,
            DECIMAL_ALWAYS_SHOWN => p.decimal_separator_always_shown = n != 0,
            MAX_INTEGER_DIGITS => {
                let v = clamp(n.max(0));
                if p.min_int >= 0 && p.min_int > v {
                    p.min_int = v;
                }
                p.max_int = v;
            }
            MIN_INTEGER_DIGITS => {
                let v = clamp(n.max(0));
                if p.max_int >= 0 && p.max_int < v {
                    p.max_int = v;
                }
                p.min_int = v;
            }
            INTEGER_DIGITS => {
                let v = clamp(n.max(0));
                p.min_int = v;
                p.max_int = v;
            }
            MAX_FRACTION_DIGITS => {
                let v = clamp(n.max(0));
                if p.min_frac >= 0 && p.min_frac > v {
                    p.min_frac = v;
                }
                p.max_frac = v;
            }
            MIN_FRACTION_DIGITS => {
                let v = clamp(n.max(0));
                if p.max_frac >= 0 && p.max_frac < v {
                    p.max_frac = v;
                }
                p.min_frac = v;
            }
            FRACTION_DIGITS => {
                let v = clamp(n.max(0));
                p.min_frac = v;
                p.max_frac = v;
            }
            MULTIPLIER => p.set_multiplier(n),
            GROUPING_SIZE => p.grouping_size = clamp(n),
            ROUNDING_MODE => st.rounding_mode = n,
            ROUNDING_INCREMENT => {
                let f = value.to_float();
                p.rounding_increment = if f > 0.0 { f } else { 0.0 };
            }
            FORMAT_WIDTH => p.format_width = n.clamp(-1, i64::from(i32::MAX)) as i32,
            PADDING_POSITION => p.pad_position = Some(n),
            SECONDARY_GROUPING_SIZE => p.secondary_grouping_size = clamp(n),
            SIGNIFICANT_DIGITS_USED => {
                let used = p.min_sig != -1 || p.max_sig != -1;
                if (n != 0) != used {
                    p.min_sig = if n != 0 { 1 } else { -1 };
                    p.max_sig = if n != 0 { 6 } else { -1 };
                }
            }
            MIN_SIGNIFICANT_DIGITS => {
                let v = clamp(n.max(1));
                if p.max_sig >= 0 && p.max_sig < v {
                    p.max_sig = v;
                }
                p.min_sig = v;
            }
            MAX_SIGNIFICANT_DIGITS => {
                let v = clamp(n.max(1));
                if p.min_sig >= 0 && p.min_sig > v {
                    p.min_sig = v;
                }
                p.max_sig = v;
            }
            LENIENT_PARSE => st.lenient = n != 0,
            _ => return false,
        }
        true
    })
    .unwrap_or(false);
    if !ok {
        object_error(o, &who, U_UNSUPPORTED_ERROR, "Error setting attribute value");
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

fn get_text_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let id = args.first().map_or(-1, Value::to_int);
    let v = with_state(o, |st| {
        st.err.reset();
        Some(match id {
            POSITIVE_PREFIX => st.affix_text(false, true),
            POSITIVE_SUFFIX => st.affix_text(false, false),
            NEGATIVE_PREFIX => st.affix_text(true, true),
            NEGATIVE_SUFFIX => st.affix_text(true, false),
            PADDING_CHARACTER => st.props.pad_string.clone().unwrap_or_else(|| " ".to_string()),
            CURRENCY_CODE => st.currency_code(),
            _ => return None,
        })
    })
    .flatten();
    match v {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => {
            object_error(o, &who, U_UNSUPPORTED_ERROR, "Error getting attribute value");
            Ok(Value::Bool(false))
        }
    }
}

fn set_text_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let id = args.first().map_or(-1, Value::to_int);
    let text = crate::text_arg(args, 1);
    let result = with_state(o, |st| {
        st.err.reset();
        match id {
            POSITIVE_PREFIX => st.props.pos_prefix = Some(text.clone()),
            POSITIVE_SUFFIX => st.props.pos_suffix = Some(text.clone()),
            NEGATIVE_PREFIX => st.props.neg_prefix = Some(text.clone()),
            NEGATIVE_SUFFIX => st.props.neg_suffix = Some(text.clone()),
            PADDING_CHARACTER => st.props.pad_string = Some(text.chars().next().map_or(" ".to_string(), |c| c.to_string())),
            CURRENCY_CODE => return st.set_currency(&text),
            _ => return Err(U_UNSUPPORTED_ERROR),
        }
        Ok(())
    })
    .unwrap_or(Err(U_UNSUPPORTED_ERROR));
    match result {
        Ok(()) => Ok(Value::Bool(true)),
        Err(code) => {
            object_error(o, &who, code, "Error setting text attribute");
            Ok(Value::Bool(false))
        }
    }
}

fn get_symbol(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let id = args.first().map_or(-1, Value::to_int);
    if !(0..=SYM_ONE_DIGIT as i64).contains(&id) {
        return Ok(Value::Bool(false));
    }
    let s = with_state(o, |st| {
        st.err.reset();
        match id as usize {
            SYM_ONE_DIGIT => st.digits[1].clone(),
            SYM_ZERO => st.digits[0].clone(),
            i => st.symbols[i].clone(),
        }
    })
    .unwrap_or_default();
    Ok(Value::string(s.as_bytes()))
}

fn set_symbol(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let id = args.first().map_or(-1, Value::to_int);
    let text = crate::text_arg(args, 1);
    if !(0..=SYM_ONE_DIGIT as i64).contains(&id) {
        return Ok(Value::Bool(false));
    }
    with_state(o, |st| {
        st.err.reset();
        match id as usize {
            SYM_ONE_DIGIT => st.digits[1] = text,
            SYM_ZERO => {
                // a new zero that is a digit brings the other nine along
                let mut chars = text.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    if crate::uchar::decimal_digit(c as u32) == Some(0) {
                        for (i, d) in st.digits.iter_mut().enumerate() {
                            *d = char::from_u32(c as u32 + i as u32).unwrap_or(c).to_string();
                        }
                    }
                }
                st.digits[0] = text.clone();
                st.symbols[SYM_ZERO] = text;
            }
            SYM_CURRENCY => {
                st.custom_currency_symbol = true;
                st.symbols[SYM_CURRENCY] = text;
            }
            SYM_INTL_CURRENCY => {
                st.custom_intl_symbol = true;
                st.symbols[SYM_INTL_CURRENCY] = text;
            }
            i => st.symbols[i] = text,
        }
    });
    Ok(Value::Bool(true))
}

fn get_pattern(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let s = with_state(o, |st| {
        st.err.reset();
        // a currency formatter's pattern shows the rounding it resolved
        let r = st.resolve();
        if r.use_currency {
            let mut p = st.props.clone();
            p.min_frac = r.min_frac;
            p.max_frac = r.max_frac;
            p.rounding_increment = r.rounding_increment;
            pattern::to_pattern(&p)
        } else {
            pattern::to_pattern(&st.props)
        }
    })
    .unwrap_or_default();
    Ok(Value::string(s.as_bytes()))
}

fn set_pattern(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let text = crate::text_arg(args, 0);
    let result = with_state(o, |st| {
        st.err.reset();
        let mut props = st.props.clone();
        pattern::apply(&mut props, &text, IgnoreRounding::Never)?;
        st.props = props;
        if text.is_empty() {
            // an empty pattern resets the whole property bag
            st.currency = None;
            st.rounding_mode = ROUND_HALFEVEN;
            st.parse_int_only = false;
        }
        Ok(())
    })
    .unwrap_or(Err(U_ILLEGAL_ARGUMENT_ERROR));
    match result {
        Ok(()) => Ok(Value::Bool(true)),
        Err(code) => {
            object_error(o, &who, code, "Error setting pattern value at line 0, offset 0");
            Ok(Value::Bool(false))
        }
    }
}

fn get_locale(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let kind = args.first().map_or(1, Value::to_int);
    if !matches!(kind, 0 | 1) {
        object_error(o, &who, U_ILLEGAL_ARGUMENT_ERROR, "Error getting locale");
        return Ok(Value::Bool(false));
    }
    let name = with_state(o, |st| if kind == 0 { st.actual.clone() } else { st.valid.clone() }).unwrap_or_default();
    Ok(Value::string(name.as_bytes()))
}

fn get_error_code(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |st| st.err.code).unwrap_or(0)))
}

fn get_error_message(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::string(with_state(o, |st| st.err.message()).unwrap_or_default().as_bytes()))
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("create", create),
    ("format", format),
    ("parse", parse),
    ("formatCurrency", format_currency),
    ("parseCurrency", parse_currency),
    ("setAttribute", set_attribute),
    ("getAttribute", get_attribute),
    ("setTextAttribute", set_text_attribute),
    ("getTextAttribute", get_text_attribute),
    ("setSymbol", set_symbol),
    ("getSymbol", get_symbol),
    ("setPattern", set_pattern),
    ("getPattern", get_pattern),
    ("getLocale", get_locale),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
];

macro_rules! as_function {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            let obj = match args.first().map(|v| v.deref().into_owned()) {
                Some(Value::Object(o)) => o,
                _ => return Err(Unwind::type_error("Argument #1 ($formatter) must be of type NumberFormatter")),
            };
            let rest = &mut args[1..];
            $method(ctx, Some(&obj), rest)
        }
    };
}

as_function!(f_format, format);
as_function!(f_parse, parse);
as_function!(f_format_currency, format_currency);
as_function!(f_parse_currency, parse_currency);
as_function!(f_set_attribute, set_attribute);
as_function!(f_get_attribute, get_attribute);
as_function!(f_set_text_attribute, set_text_attribute);
as_function!(f_get_text_attribute, get_text_attribute);
as_function!(f_set_symbol, set_symbol);
as_function!(f_get_symbol, get_symbol);
as_function!(f_set_pattern, set_pattern);
as_function!(f_get_pattern, get_pattern);
as_function!(f_get_locale, get_locale);
as_function!(f_get_error_code, get_error_code);
as_function!(f_get_error_message, get_error_message);

fn f_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, None, args)
}

static FUNCTIONS: &[FnImpl] = &[
    ("numfmt_create", f_create),
    ("numfmt_format", f_format),
    ("numfmt_parse", f_parse),
    ("numfmt_format_currency", f_format_currency),
    ("numfmt_parse_currency", f_parse_currency),
    ("numfmt_set_attribute", f_set_attribute),
    ("numfmt_get_attribute", f_get_attribute),
    ("numfmt_set_text_attribute", f_set_text_attribute),
    ("numfmt_get_text_attribute", f_get_text_attribute),
    ("numfmt_set_symbol", f_set_symbol),
    ("numfmt_get_symbol", f_get_symbol),
    ("numfmt_set_pattern", f_set_pattern),
    ("numfmt_get_pattern", f_get_pattern),
    ("numfmt_get_locale", f_get_locale),
    ("numfmt_get_error_code", f_get_error_code),
    ("numfmt_get_error_message", f_get_error_message),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::NUMBERFORMATTER, METHODS, |b| b.payload_clone(payload_clone));
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
