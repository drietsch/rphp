//! `IntlChar`: ICU's `u_*` character API over ICU4X's properties and case
//! mapper, with the pieces ICU4X has no data for taken from ICU 78 via
//! php (`tables.rs`): the character-name correction aliases, the `Age`
//! and `Block` ranges, the property and property-value names. Character
//! names come from `unicode_names2` (the algorithmic Hangul and ideograph
//! names included).
//!
//! A `$codepoint` argument is an int or a one-code-point string; anything
//! else answers `null`, as php does. The `is*` predicates follow ICU's
//! definitions (`u_isalpha` is General_Category L*, `u_isUAlphabetic` the
//! Alphabetic property, `u_isspace` ICU's space class, `u_isWhitespace`
//! Java's), the case mappings are the simple single-code-point ones and
//! answer in the argument's shape (int in, int out).

#![allow(deprecated)] // ICU4X marks the ICU-numbering accessors deprecated; php's constants are ICU's numbers

use icu::casemap::CaseMapperBorrowed;
use icu::normalizer::properties::Decomposed;
use icu::normalizer::DecomposingNormalizerBorrowed;
use icu::properties::props::{
    BidiClass, BidiMirroringGlyph, BidiPairedBracketType, CanonicalCombiningClass, EastAsianWidth, GeneralCategory,
    GraphemeClusterBreak, HangulSyllableType, IndicConjunctBreak, IndicSyllabicCategory, JoiningGroup, JoiningType,
    LineBreak, NumericType, Script, SentenceBreak, VerticalOrientation, WordBreak,
};
use icu::properties::{props, CodePointMapData, CodePointSetData};
use rphp_runtime::{Ctx, NativeResult, Registry};
use rphp_value::{Array, Object, Value};

use crate::generated;
use crate::shape::{register_class, MethodImpl};
use crate::state::{self, U_ILLEGAL_CHAR_FOUND};
use crate::tables::{AGE_RANGES, BLOCK_RANGES, INT_PROPERTY_RANGES, NAME_ALIASES, PROPERTY_NAMES, PROPERTY_VALUE_NAMES};

const UNICODE_CHAR_NAME: i64 = 0;
const UNICODE_10_CHAR_NAME: i64 = 1;
const EXTENDED_CHAR_NAME: i64 = 2;
const CHAR_NAME_ALIAS: i64 = 3;

const NO_NUMERIC_VALUE: f64 = -123456789.0;

/// How the argument was given, so a case mapping answers in kind.
enum Shape {
    Int,
    Str,
}

/// The code point of a `string|int $codepoint` argument.
fn codepoint(v: &Value) -> Option<(u32, Shape)> {
    match &*v.deref() {
        Value::Int(i) => u32::try_from(*i).ok().filter(|c| *c <= 0x10FFFF).map(|c| (c, Shape::Int)),
        Value::Str(s) => {
            let text = std::str::from_utf8(s.as_bytes()).ok()?;
            let mut it = text.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Some((c as u32, Shape::Str)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn cp_arg(args: &[Value]) -> Option<(u32, Shape)> {
    args.first().and_then(codepoint)
}

/// A code point as a `char`; surrogates have no `char`, so a placeholder
/// stands in for property lookups that do not care.
fn as_char(cp: u32) -> char {
    char::from_u32(cp).unwrap_or('\u{FFFD}')
}

/// A code point as UTF-8 bytes — a surrogate too, as ICU's converter
/// writes it (php's `IntlChar::chr(0xD800)` is those three bytes).
fn utf8_of(cp: u32) -> Vec<u8> {
    match char::from_u32(cp) {
        Some(c) => c.to_string().into_bytes(),
        None => vec![0xE0 | (cp >> 12) as u8, 0x80 | ((cp >> 6) & 0x3F) as u8, 0x80 | (cp & 0x3F) as u8],
    }
}

fn answer_cp(cp: u32, shape: Shape) -> Value {
    match shape {
        Shape::Int => Value::Int(i64::from(cp)),
        Shape::Str => Value::string(&utf8_of(cp)),
    }
}

fn gc(cp: u32) -> GeneralCategory {
    CodePointMapData::<GeneralCategory>::new().get32(cp)
}

fn in_set<P: props::BinaryProperty>(cp: u32) -> bool {
    CodePointSetData::new::<P>().contains32(cp)
}

// ---- predicates ------------------------------------------------------------------------------

macro_rules! predicate {
    ($name:ident, $body:expr) => {
        fn $name(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
            let Some((cp, _)) = cp_arg(args) else {
                return Ok(Value::Null);
            };
            let f: fn(u32) -> bool = $body;
            Ok(Value::Bool(f(cp)))
        }
    };
}

fn is_letter(g: GeneralCategory) -> bool {
    matches!(
        g,
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

fn is_mark(g: GeneralCategory) -> bool {
    matches!(g, GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark)
}

fn is_number(g: GeneralCategory) -> bool {
    matches!(g, GeneralCategory::DecimalNumber | GeneralCategory::LetterNumber | GeneralCategory::OtherNumber)
}

fn is_punct(g: GeneralCategory) -> bool {
    matches!(
        g,
        GeneralCategory::DashPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::OtherPunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
    )
}

/// ICU's `u_isspace`: Z* categories plus the ISO controls that are
/// white space (TAB..CR, U+1C..U+1F, U+85).
fn icu_isspace(cp: u32) -> bool {
    matches!(cp, 0x09..=0x0D | 0x1C..=0x1F | 0x85)
        || matches!(gc(cp), GeneralCategory::SpaceSeparator | GeneralCategory::LineSeparator | GeneralCategory::ParagraphSeparator)
}

/// ICU's `u_isJavaSpaceChar`: the Z* categories.
fn java_space(cp: u32) -> bool {
    matches!(gc(cp), GeneralCategory::SpaceSeparator | GeneralCategory::LineSeparator | GeneralCategory::ParagraphSeparator)
}

/// ICU's `u_isWhitespace`: Java's `isWhitespace` — Z* except the
/// no-break spaces, plus TAB..CR and U+1C..U+1F.
fn java_whitespace(cp: u32) -> bool {
    matches!(cp, 0x09..=0x0D | 0x1C..=0x1F) || (java_space(cp) && !matches!(cp, 0xA0 | 0x2007 | 0x202F))
}

predicate!(isalpha, |cp| is_letter(gc(cp)));
predicate!(isalnum, |cp| {
    let g = gc(cp);
    is_letter(g) || g == GeneralCategory::DecimalNumber
});
predicate!(isbase, |cp| {
    let g = gc(cp);
    is_letter(g) || is_number(g) || matches!(g, GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark)
});
predicate!(isblank, |cp| cp == 0x09 || gc(cp) == GeneralCategory::SpaceSeparator);
predicate!(iscntrl, |cp| matches!(
    gc(cp),
    GeneralCategory::Control | GeneralCategory::Format | GeneralCategory::LineSeparator | GeneralCategory::ParagraphSeparator
));
predicate!(isdefined, |cp| gc(cp) != GeneralCategory::Unassigned);
predicate!(isdigit, |cp| gc(cp) == GeneralCategory::DecimalNumber);
predicate!(isgraph, |cp| !matches!(
    gc(cp),
    GeneralCategory::Control
        | GeneralCategory::Format
        | GeneralCategory::Surrogate
        | GeneralCategory::Unassigned
        | GeneralCategory::SpaceSeparator
        | GeneralCategory::LineSeparator
        | GeneralCategory::ParagraphSeparator
));
predicate!(isprint, |cp| !matches!(
    gc(cp),
    GeneralCategory::Control
        | GeneralCategory::Format
        | GeneralCategory::Surrogate
        | GeneralCategory::PrivateUse
        | GeneralCategory::Unassigned
));
predicate!(ispunct, |cp| is_punct(gc(cp)));
predicate!(isspace, icu_isspace);
predicate!(is_java_space_char, java_space);
predicate!(is_whitespace, java_whitespace);
predicate!(is_u_white_space, |cp| in_set::<props::WhiteSpace>(cp));
predicate!(islower, |cp| gc(cp) == GeneralCategory::LowercaseLetter);
predicate!(isupper, |cp| gc(cp) == GeneralCategory::UppercaseLetter);
predicate!(istitle, |cp| gc(cp) == GeneralCategory::TitlecaseLetter);
predicate!(is_u_lowercase, |cp| in_set::<props::Lowercase>(cp));
predicate!(is_u_uppercase, |cp| in_set::<props::Uppercase>(cp));
predicate!(is_u_alphabetic, |cp| in_set::<props::Alphabetic>(cp));
predicate!(isxdigit, |cp| in_set::<props::Xdigit>(cp));
predicate!(is_iso_control, |cp| matches!(cp, 0..=0x1F | 0x7F..=0x9F));
predicate!(is_mirrored, |cp| in_set::<props::BidiMirrored>(cp));
// ICU 78's `u_isIDStart`/`u_isIDPart` are UAX #31's ID_Start/ID_Continue.
predicate!(is_id_start, |cp| in_set::<props::IdStart>(cp));
predicate!(is_id_part, |cp| in_set::<props::IdContinue>(cp));
predicate!(is_id_ignorable, id_ignorable);
predicate!(is_java_id_start, |cp| {
    let g = gc(cp);
    is_letter(g) || g == GeneralCategory::CurrencySymbol || g == GeneralCategory::ConnectorPunctuation
});
predicate!(is_java_id_part, |cp| {
    let g = gc(cp);
    is_letter(g)
        || g == GeneralCategory::LetterNumber
        || g == GeneralCategory::CurrencySymbol
        || g == GeneralCategory::ConnectorPunctuation
        || g == GeneralCategory::DecimalNumber
        || is_mark(g)
        || id_ignorable(cp)
});

/// ICU's `u_isIDIgnorable`: format controls and the non-whitespace ISO
/// controls.
fn id_ignorable(cp: u32) -> bool {
    matches!(cp, 0..=0x08 | 0x0E..=0x1B | 0x7F..=0x9F) || gc(cp) == GeneralCategory::Format
}

// ---- simple accessors ------------------------------------------------------------------------

fn ord(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| Value::Int(i64::from(cp))))
}

fn chr(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| Value::string(&utf8_of(cp))))
}

fn char_type(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| Value::Int(i64::from(gc(cp) as u8))))
}

fn char_direction(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| {
        Value::Int(i64::from(CodePointMapData::<BidiClass>::new().get32(cp).to_icu4c_value()))
    }))
}

fn get_combining_class(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| {
        Value::Int(i64::from(CodePointMapData::<CanonicalCombiningClass>::new().get32(cp).to_icu4c_value()))
    }))
}

fn char_mirror(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, shape)| {
        let m = CodePointMapData::<BidiMirroringGlyph>::new().get32(cp);
        answer_cp(m.mirroring_glyph.map_or(cp, |c| c as u32), shape)
    }))
}

fn get_bidi_paired_bracket(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, shape)| {
        let m = CodePointMapData::<BidiMirroringGlyph>::new().get32(cp);
        let paired = match m.paired_bracket_type {
            BidiPairedBracketType::None => cp,
            _ => m.mirroring_glyph.map_or(cp, |c| c as u32),
        };
        answer_cp(paired, shape)
    }))
}

fn to_case(args: &[Value], f: impl Fn(char) -> char) -> Value {
    cp_arg(args).map_or(Value::Null, |(cp, shape)| match char::from_u32(cp) {
        Some(c) => answer_cp(f(c) as u32, shape),
        None => answer_cp(cp, shape),
    })
}

fn tolower(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(to_case(args, |c| CaseMapperBorrowed::new().simple_lowercase(c)))
}

fn toupper(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(to_case(args, |c| CaseMapperBorrowed::new().simple_uppercase(c)))
}

fn totitle(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(to_case(args, |c| CaseMapperBorrowed::new().simple_titlecase(c)))
}

fn fold_case(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let turkic = args.get(1).is_some_and(|v| v.to_int() & 1 != 0);
    Ok(to_case(args, |c| {
        let m = CaseMapperBorrowed::new();
        if turkic {
            m.simple_fold_turkic(c)
        } else {
            m.simple_fold(c)
        }
    }))
}

/// The decimal digit value of an `Nd` character: its offset in the run of
/// ten it belongs to (every `Nd` digit sits in a contiguous 0–9 run).
pub(crate) fn decimal_digit(cp: u32) -> Option<u32> {
    if gc(cp) != GeneralCategory::DecimalNumber {
        return None;
    }
    let mut k = 0u32;
    let mut c = cp;
    while c > 0 && gc(c - 1) == GeneralCategory::DecimalNumber {
        c -= 1;
        k += 1;
    }
    Some(k % 10)
}

fn char_digit_value(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| Value::Int(decimal_digit(cp).map_or(-1, i64::from))))
}

fn digit(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((cp, _)) = cp_arg(args) else {
        return Ok(Value::Null);
    };
    let base = args.get(1).map_or(10, Value::to_int);
    if !(2..=36).contains(&base) {
        return Ok(Value::Bool(false));
    }
    let value = match decimal_digit(cp) {
        Some(d) => Some(i64::from(d)),
        None => {
            let c = as_char(cp);
            // ICU's `u_digit`: ASCII and fullwidth letters count for bases
            // past 10.
            let letter = match c {
                'a'..='z' => Some(c as i64 - 'a' as i64 + 10),
                'A'..='Z' => Some(c as i64 - 'A' as i64 + 10),
                '\u{FF41}'..='\u{FF5A}' => Some(c as i64 - 0xFF41 + 10),
                '\u{FF21}'..='\u{FF3A}' => Some(c as i64 - 0xFF21 + 10),
                _ => None,
            };
            letter
        }
    };
    Ok(match value {
        Some(v) if v < base => Value::Int(v),
        _ => Value::Bool(false),
    })
}

fn for_digit(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let d = args.first().map_or(0, Value::to_int);
    let base = args.get(1).map_or(10, Value::to_int);
    if !(2..=36).contains(&base) || d < 0 || d >= base {
        return Ok(Value::Int(0));
    }
    Ok(Value::Int(if d < 10 { i64::from(b'0') + d } else { i64::from(b'a') + d - 10 }))
}

fn get_numeric_value(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |(cp, _)| {
        Value::Float(match decimal_digit(cp) {
            Some(d) => f64::from(d),
            None => match numeric_value(cp) {
                Some(v) => v,
                None => NO_NUMERIC_VALUE,
            },
        })
    }))
}

/// Numeric values ICU4X does not carry: a few common non-decimal numbers.
fn numeric_value(cp: u32) -> Option<f64> {
    Some(match cp {
        0x00BC => 0.25,
        0x00BD => 0.5,
        0x00BE => 0.75,
        0x00B2 => 2.0,
        0x00B3 => 3.0,
        0x00B9 => 1.0,
        0x2070 => 0.0,
        0x2074..=0x2079 => f64::from(cp - 0x2070),
        0x2080..=0x2089 => f64::from(cp - 0x2080),
        0x2150 => 1.0 / 7.0,
        0x2151 => 1.0 / 9.0,
        0x2152 => 0.1,
        0x2153 => 1.0 / 3.0,
        0x2154 => 2.0 / 3.0,
        0x2155 => 0.2,
        0x2156 => 0.4,
        0x2157 => 0.6,
        0x2158 => 0.8,
        0x2159 => 1.0 / 6.0,
        0x215A => 5.0 / 6.0,
        0x215B => 0.125,
        0x215C => 0.375,
        0x215D => 0.625,
        0x215E => 0.875,
        0x215F => 1.0,
        0x2160..=0x216B => f64::from(cp - 0x2160 + 1),
        0x216C => 50.0,
        0x216D => 100.0,
        0x216E => 500.0,
        0x216F => 1000.0,
        0x2170..=0x217B => f64::from(cp - 0x2170 + 1),
        0x217C => 50.0,
        0x217D => 100.0,
        0x217E => 500.0,
        0x217F => 1000.0,
        0x2460..=0x2473 => f64::from(cp - 0x2460 + 1),
        0x2474..=0x2487 => f64::from(cp - 0x2474 + 1),
        0x2488..=0x249B => f64::from(cp - 0x2488 + 1),
        0x24EA => 0.0,
        0x24EB..=0x24F4 => f64::from(cp - 0x24EB + 11),
        0x24F5..=0x24FE => f64::from(cp - 0x24F5 + 1),
        0x24FF => 0.0,
        0x2776..=0x277F => f64::from(cp - 0x2776 + 1),
        0x2780..=0x2789 => f64::from(cp - 0x2780 + 1),
        0x278A..=0x2793 => f64::from(cp - 0x278A + 1),
        0x3007 => 0.0,
        0x3021..=0x3029 => f64::from(cp - 0x3021 + 1),
        0x4E00 => 1.0,
        0x4E8C => 2.0,
        0x4E09 => 3.0,
        0x56DB => 4.0,
        0x4E94 => 5.0,
        0x516D => 6.0,
        0x4E03 => 7.0,
        0x516B => 8.0,
        0x4E5D => 9.0,
        0x5341 => 10.0,
        0x767E => 100.0,
        0x5343 => 1000.0,
        0x842C | 0x4E07 => 10000.0,
        0x5104 => 100_000_000.0,
        0x96F6 => 0.0,
        _ => return None,
    })
}

// ---- names ------------------------------------------------------------------------------------

fn extended_name(cp: u32) -> String {
    let kind = match gc(cp) {
        GeneralCategory::Control => "control",
        GeneralCategory::PrivateUse => "private use area",
        GeneralCategory::Surrogate => "surrogate",
        _ if (cp & 0xFFFE) == 0xFFFE || (0xFDD0..=0xFDEF).contains(&cp) => "noncharacter",
        _ => "unassigned",
    };
    format!("<{kind}-{cp:04X}>")
}

fn unicode_name(cp: u32) -> Option<String> {
    if let Some(n) = char::from_u32(cp).and_then(unicode_names2::name) {
        return Some(n.to_string());
    }
    // the algorithmic names `unicode_names2` does not generate
    let prefix = match cp {
        0x17000..=0x187F7 | 0x18D00..=0x18D08 => "TANGUT IDEOGRAPH-",
        0x18B00..=0x18CD5 | 0x18CFF => "KHITAN SMALL SCRIPT CHARACTER-",
        0x1B170..=0x1B2FB => "NUSHU CHARACTER-",
        0x13460..=0x143FA => "EGYPTIAN HIEROGLYPH-",
        _ => return None,
    };
    Some(format!("{prefix}{cp:04X}"))
}

fn char_name(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((cp, _)) = cp_arg(args) else {
        return Ok(Value::Null);
    };
    let choice = args.get(1).map_or(UNICODE_CHAR_NAME, Value::to_int);
    let name = match choice {
        UNICODE_CHAR_NAME => unicode_name(cp).unwrap_or_default(),
        EXTENDED_CHAR_NAME => unicode_name(cp).unwrap_or_else(|| extended_name(cp)),
        CHAR_NAME_ALIAS => NAME_ALIASES.iter().find(|(c, _)| *c == cp).map_or(String::new(), |(_, n)| (*n).to_string()),
        UNICODE_10_CHAR_NAME => String::new(),
        // ICU answers nothing for a choice it does not know; php passes
        // that on as `null` (a negative choice reads as an empty name).
        c if c < 0 => String::new(),
        _ => return Ok(Value::Null),
    };
    let _ = ctx;
    Ok(Value::string(name.as_bytes()))
}

fn char_from_name(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let name = crate::text_arg(args, 0);
    let choice = args.get(1).map_or(UNICODE_CHAR_NAME, Value::to_int);
    let who = ctx.active_function_name();
    let found = match choice {
        UNICODE_CHAR_NAME | EXTENDED_CHAR_NAME => {
            let mut r = unicode_names2::character(&name).map(|c| c as u32);
            if r.is_none() {
                // the algorithmic names above
                for (prefix, lo, hi) in [
                    ("TANGUT IDEOGRAPH-", 0x17000u32, 0x18D08u32),
                    ("KHITAN SMALL SCRIPT CHARACTER-", 0x18B00, 0x18CFF),
                    ("NUSHU CHARACTER-", 0x1B170, 0x1B2FB),
                    ("EGYPTIAN HIEROGLYPH-", 0x13460, 0x143FA),
                ] {
                    if let Some(hex) = name.to_ascii_uppercase().strip_prefix(prefix) {
                        r = u32::from_str_radix(hex, 16).ok().filter(|c| (lo..=hi).contains(c));
                    }
                }
            }
            if r.is_none() && choice == EXTENDED_CHAR_NAME {
                // `<control-0000>` and the like
                if let Some(rest) = name.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
                    if let Some((_, hex)) = rest.rsplit_once('-') {
                        r = u32::from_str_radix(hex, 16).ok().filter(|c| *c <= 0x10FFFF);
                    }
                }
            }
            r
        }
        CHAR_NAME_ALIAS => NAME_ALIASES.iter().find(|(_, n)| n.eq_ignore_ascii_case(&name)).map(|(c, _)| *c),
        _ => None,
    };
    let _ = who;
    match found {
        Some(cp) => Ok(Value::Int(i64::from(cp))),
        None => {
            state::set_global_code(ctx, U_ILLEGAL_CHAR_FOUND);
            Ok(Value::Null)
        }
    }
}

fn enum_char_names(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((start, _)) = args.first().and_then(codepoint) else {
        return Ok(Value::Bool(false));
    };
    let Some((end, _)) = args.get(1).and_then(codepoint) else {
        return Ok(Value::Bool(false));
    };
    let callback = args.get(2).cloned().unwrap_or(Value::Null);
    let choice = args.get(3).map_or(UNICODE_CHAR_NAME, Value::to_int);
    for cp in start..end {
        let name = match choice {
            EXTENDED_CHAR_NAME => Some(unicode_name(cp).unwrap_or_else(|| extended_name(cp))),
            CHAR_NAME_ALIAS => NAME_ALIASES.iter().find(|(c, _)| *c == cp).map(|(_, n)| (*n).to_string()),
            UNICODE_10_CHAR_NAME => None,
            _ => unicode_name(cp),
        };
        if let Some(n) = name {
            ctx.call_value(&callback, &[Value::Int(i64::from(cp)), Value::Int(choice), Value::string(n.as_bytes())])?;
        }
    }
    Ok(Value::Bool(true))
}

fn enum_char_types(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let callback = args.first().cloned().unwrap_or(Value::Null);
    let map = CodePointMapData::<GeneralCategory>::new();
    for range in map.iter_ranges() {
        let (start, end) = (*range.range.start(), *range.range.end());
        ctx.call_value(
            &callback,
            &[Value::Int(i64::from(start)), Value::Int(i64::from(end) + 1), Value::Int(i64::from(range.value as u8))],
        )?;
    }
    Ok(Value::Null)
}

// ---- properties -------------------------------------------------------------------------------

fn char_age(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((cp, _)) = cp_arg(args) else {
        return Ok(Value::Null);
    };
    let (major, minor) = AGE_RANGES
        .iter()
        .find(|(a, b, _, _)| (*a..=*b).contains(&cp))
        .map_or((0, 0), |(_, _, ma, mi)| (*ma, *mi));
    let mut a = Array::new();
    for v in [major, minor, 0, 0] {
        a.push(Value::Int(i64::from(v)));
    }
    Ok(Value::Array(a))
}

fn get_block_code(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((cp, _)) = cp_arg(args) else {
        return Ok(Value::Null);
    };
    let code = BLOCK_RANGES.iter().find(|(a, b, _)| (*a..=*b).contains(&cp)).map_or(0, |(_, _, c)| *c);
    Ok(Value::Int(i64::from(code)))
}

fn get_unicode_version(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    for v in [17, 0, 0, 0] {
        a.push(Value::Int(v));
    }
    Ok(Value::Array(a))
}

/// The binary properties by ICU's `UProperty` number.
fn binary_property(cp: u32, property: i64) -> Option<bool> {
    Some(match property {
        0 => in_set::<props::Alphabetic>(cp),
        1 => in_set::<props::AsciiHexDigit>(cp),
        2 => in_set::<props::BidiControl>(cp),
        3 => in_set::<props::BidiMirrored>(cp),
        4 => in_set::<props::Dash>(cp),
        5 => in_set::<props::DefaultIgnorableCodePoint>(cp),
        6 => in_set::<props::Deprecated>(cp),
        7 => in_set::<props::Diacritic>(cp),
        8 => in_set::<props::Extender>(cp),
        9 => in_set::<props::FullCompositionExclusion>(cp),
        10 => in_set::<props::GraphemeBase>(cp),
        11 => in_set::<props::GraphemeExtend>(cp),
        12 => in_set::<props::GraphemeLink>(cp),
        13 => in_set::<props::HexDigit>(cp),
        14 => in_set::<props::Hyphen>(cp),
        15 => in_set::<props::IdContinue>(cp),
        16 => in_set::<props::IdStart>(cp),
        17 => in_set::<props::Ideographic>(cp),
        18 => in_set::<props::IdsBinaryOperator>(cp),
        19 => in_set::<props::IdsTrinaryOperator>(cp),
        20 => in_set::<props::JoinControl>(cp),
        21 => in_set::<props::LogicalOrderException>(cp),
        22 => in_set::<props::Lowercase>(cp),
        23 => in_set::<props::Math>(cp),
        24 => in_set::<props::NoncharacterCodePoint>(cp),
        25 => in_set::<props::QuotationMark>(cp),
        26 => in_set::<props::Radical>(cp),
        27 => in_set::<props::SoftDotted>(cp),
        28 => in_set::<props::TerminalPunctuation>(cp),
        29 => in_set::<props::UnifiedIdeograph>(cp),
        30 => in_set::<props::Uppercase>(cp),
        31 => in_set::<props::WhiteSpace>(cp),
        32 => in_set::<props::XidContinue>(cp),
        33 => in_set::<props::XidStart>(cp),
        34 => in_set::<props::CaseSensitive>(cp),
        35 => in_set::<props::SentenceTerminal>(cp),
        36 => in_set::<props::VariationSelector>(cp),
        37 => in_set::<props::NfdInert>(cp),
        38 => in_set::<props::NfkdInert>(cp),
        39 => in_set::<props::NfcInert>(cp),
        40 => in_set::<props::NfkcInert>(cp),
        41 => in_set::<props::SegmentStarter>(cp),
        42 => in_set::<props::PatternSyntax>(cp),
        43 => in_set::<props::PatternWhiteSpace>(cp),
        44 => in_set::<props::Alnum>(cp),
        45 => in_set::<props::Blank>(cp),
        46 => in_set::<props::Graph>(cp),
        47 => in_set::<props::Print>(cp),
        48 => in_set::<props::Xdigit>(cp),
        49 => in_set::<props::Cased>(cp),
        50 => in_set::<props::CaseIgnorable>(cp),
        51 => in_set::<props::ChangesWhenLowercased>(cp),
        52 => in_set::<props::ChangesWhenUppercased>(cp),
        53 => in_set::<props::ChangesWhenTitlecased>(cp),
        54 => in_set::<props::ChangesWhenCasefolded>(cp),
        55 => in_set::<props::ChangesWhenCasemapped>(cp),
        56 => in_set::<props::ChangesWhenNfkcCasefolded>(cp),
        57 => in_set::<props::Emoji>(cp),
        58 => in_set::<props::EmojiPresentation>(cp),
        59 => in_set::<props::EmojiModifier>(cp),
        60 => in_set::<props::EmojiModifierBase>(cp),
        61 => in_set::<props::EmojiComponent>(cp),
        62 => in_set::<props::RegionalIndicator>(cp),
        63 => in_set::<props::PrependedConcatenationMark>(cp),
        64 => in_set::<props::ExtendedPictographic>(cp),
        71 => in_set::<props::IdsUnaryOperator>(cp),
        72 => in_set::<props::IdCompatMathStart>(cp),
        73 => in_set::<props::IdCompatMathContinue>(cp),
        74 => in_set::<props::ModifierCombiningMark>(cp),
        _ => return None,
    })
}

fn has_binary_property(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((cp, _)) = cp_arg(args) else {
        return Ok(Value::Null);
    };
    let property = args.get(1).map_or(-1, Value::to_int);
    Ok(Value::Bool(binary_property(cp, property).unwrap_or(false)))
}

/// The enumerated properties by ICU's `UProperty` number, in ICU's value
/// numbering (ICU4X mirrors it).
fn int_property(cp: u32, property: i64) -> Option<i64> {
    let v = match property {
        0x1000 => i64::from(CodePointMapData::<BidiClass>::new().get32(cp).to_icu4c_value()),
        0x1001 => i64::from(BLOCK_RANGES.iter().find(|(a, b, _)| (*a..=*b).contains(&cp)).map_or(0, |(_, _, c)| *c)),
        0x1002 => i64::from(CodePointMapData::<CanonicalCombiningClass>::new().get32(cp).to_icu4c_value()),
        0x1004 => i64::from(CodePointMapData::<EastAsianWidth>::new().get32(cp).to_icu4c_value()),
        0x1005 => i64::from(gc(cp) as u8),
        0x1006 => i64::from(CodePointMapData::<JoiningGroup>::new().get32(cp).to_icu4c_value()),
        0x1007 => i64::from(CodePointMapData::<JoiningType>::new().get32(cp).to_icu4c_value()),
        0x1008 => i64::from(CodePointMapData::<LineBreak>::new().get32(cp).to_icu4c_value()),
        0x1009 => i64::from(CodePointMapData::<NumericType>::new().get32(cp).to_icu4c_value()),
        0x100A => i64::from(CodePointMapData::<Script>::new().get32(cp).to_icu4c_value()),
        0x100B => i64::from(CodePointMapData::<HangulSyllableType>::new().get32(cp).to_icu4c_value()),
        0x100C | 0x100D | 0x100E | 0x100F => {
            // the quick checks: No when the character alone does not
            // normalize to itself; for the composing forms Maybe (2) when
            // it could combine with what precedes it.
            let s = as_char(cp).to_string();
            let normalized = match property {
                0x100C => DecomposingNormalizerBorrowed::new_nfd().is_normalized(&s),
                0x100D => DecomposingNormalizerBorrowed::new_nfkd().is_normalized(&s),
                0x100E => icu::normalizer::ComposingNormalizerBorrowed::new_nfc().is_normalized(&s),
                _ => icu::normalizer::ComposingNormalizerBorrowed::new_nfkc().is_normalized(&s),
            };
            if !normalized {
                0
            } else if property >= 0x100E && may_compose_second(cp) {
                2
            } else {
                1
            }
        }
        0x1010 | 0x1011 => {
            let s = as_char(cp).to_string();
            let nfd = DecomposingNormalizerBorrowed::new_nfd().normalize(&s);
            let ccc = CodePointMapData::<CanonicalCombiningClass>::new();
            let c = if property == 0x1010 { nfd.chars().next() } else { nfd.chars().last() };
            i64::from(c.map_or(0, |c| ccc.get(c).to_icu4c_value()))
        }
        0x1012 => i64::from(CodePointMapData::<GraphemeClusterBreak>::new().get32(cp).to_icu4c_value()),
        0x1013 => i64::from(CodePointMapData::<SentenceBreak>::new().get32(cp).to_icu4c_value()),
        0x1014 => i64::from(CodePointMapData::<WordBreak>::new().get32(cp).to_icu4c_value()),
        0x1015 => match CodePointMapData::<BidiMirroringGlyph>::new().get32(cp).paired_bracket_type {
            BidiPairedBracketType::None => 0,
            BidiPairedBracketType::Open => 1,
            BidiPairedBracketType::Close => 2,
            _ => 0,
        },
        0x1017 => i64::from(CodePointMapData::<IndicSyllabicCategory>::new().get32(cp).to_icu4c_value()),
        0x1018 => i64::from(CodePointMapData::<VerticalOrientation>::new().get32(cp).to_icu4c_value()),
        0x101B => i64::from(CodePointMapData::<IndicConjunctBreak>::new().get32(cp).to_icu4c_value()),
        0x2000 => 1i64 << (gc(cp) as u8),
        p if (0..0x1000).contains(&p) => i64::from(binary_property(cp, p)?),
        _ => return None,
    };
    Some(v)
}

/// Whether `cp` can be the second character of a canonical composition
/// (NFC_QC=Maybe): a Hangul vowel or trailing jamo, or a character some
/// canonical decomposition ends with — that set is gathered once from
/// the decomposition data.
fn may_compose_second(cp: u32) -> bool {
    use std::sync::OnceLock;
    static SECONDS: OnceLock<Vec<u32>> = OnceLock::new();
    if (0x1161..=0x1175).contains(&cp) || (0x11A8..=0x11C2).contains(&cp) {
        return true;
    }
    let seconds = SECONDS.get_or_init(|| {
        let decomp = icu::normalizer::properties::CanonicalDecompositionBorrowed::new();
        let exclusions = CodePointSetData::new::<props::FullCompositionExclusion>();
        let mut v: Vec<u32> = Vec::new();
        for c in (0u32..0x30000).filter_map(char::from_u32) {
            if let Decomposed::Expansion(_, b) = decomp.decompose(c) {
                if !exclusions.contains(c) {
                    v.push(b as u32);
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    });
    seconds.binary_search(&cp).is_ok()
}

fn get_int_property_value(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some((cp, _)) = cp_arg(args) else {
        return Ok(Value::Null);
    };
    let property = args.get(1).map_or(-1, Value::to_int);
    Ok(Value::Int(int_property(cp, property).unwrap_or(0)))
}

fn get_int_property_min_value(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let property = args.first().map_or(-1, Value::to_int);
    Ok(Value::Int(INT_PROPERTY_RANGES.iter().find(|(p, _, _)| *p == property).map_or(0, |(_, min, _)| *min)))
}

fn get_int_property_max_value(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let property = args.first().map_or(-1, Value::to_int);
    Ok(Value::Int(match INT_PROPERTY_RANGES.iter().find(|(p, _, _)| *p == property) {
        Some((_, _, max)) => *max,
        None if (0..0x1000).contains(&property) && PROPERTY_NAMES.iter().any(|(p, _, _)| *p == property) => 1,
        None => -1,
    }))
}

/// ICU's loose property-name matching: case, spaces, hyphens and
/// underscores do not count.
fn loose(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).flat_map(char::to_lowercase).collect()
}

fn get_property_name(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let property = args.first().map_or(-1, Value::to_int);
    let choice = args.get(1).map_or(1, Value::to_int);
    let Some((_, short, long)) = PROPERTY_NAMES.iter().find(|(p, _, _)| *p == property) else {
        return Ok(Value::Bool(false));
    };
    let name = match choice {
        0 => short,
        1 => long,
        _ => return Ok(Value::Bool(false)),
    };
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::string(name.as_bytes()))
}

fn get_property_enum(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let alias = loose(&crate::text_arg(args, 0));
    let found = PROPERTY_NAMES
        .iter()
        .find(|(_, s, l)| (!s.is_empty() && loose(s) == alias) || (!l.is_empty() && loose(l) == alias))
        .map_or(-1, |(p, _, _)| *p);
    Ok(Value::Int(found))
}

fn get_property_value_name(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let property = args.first().map_or(-1, Value::to_int);
    let value = args.get(1).map_or(-1, Value::to_int);
    let choice = args.get(2).map_or(1, Value::to_int);
    if (0..0x1000).contains(&property) {
        // a binary property's values are No/Yes
        return Ok(match (value, choice) {
            (0, 0) => Value::string(b"N"),
            (0, 1) => Value::string(b"No"),
            (0, 2) => Value::string(b"F"),
            (0, 3) => Value::string(b"False"),
            (1, 0) => Value::string(b"Y"),
            (1, 1) => Value::string(b"Yes"),
            (1, 2) => Value::string(b"T"),
            (1, 3) => Value::string(b"True"),
            _ => Value::Bool(false),
        });
    }
    let Some((_, _, short, long)) = PROPERTY_VALUE_NAMES.iter().find(|(p, v, _, _)| *p == property && *v == value) else {
        return Ok(Value::Bool(false));
    };
    let name = match choice {
        0 => short,
        1 => long,
        _ => return Ok(Value::Bool(false)),
    };
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::string(name.as_bytes()))
}

fn get_property_value_enum(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let property = args.first().map_or(-1, Value::to_int);
    let name = loose(&crate::text_arg(args, 1));
    if (0..0x1000).contains(&property) {
        return Ok(Value::Int(match name.as_str() {
            "n" | "no" | "f" | "false" => 0,
            "y" | "yes" | "t" | "true" => 1,
            _ => -1,
        }));
    }
    let found = PROPERTY_VALUE_NAMES
        .iter()
        .find(|(p, _, s, l)| *p == property && ((!s.is_empty() && loose(s) == name) || (!l.is_empty() && loose(l) == name)))
        .map_or(-1, |(_, v, _, _)| *v);
    Ok(Value::Int(found))
}

fn get_fc_nfkc_closure(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(cp_arg(args).map_or(Value::Null, |_| Value::string(b"")))
}

static METHODS: &[MethodImpl] = &[
    ("hasBinaryProperty", has_binary_property),
    ("charAge", char_age),
    ("charDigitValue", char_digit_value),
    ("charDirection", char_direction),
    ("charFromName", char_from_name),
    ("charMirror", char_mirror),
    ("charName", char_name),
    ("charType", char_type),
    ("chr", chr),
    ("digit", digit),
    ("enumCharNames", enum_char_names),
    ("enumCharTypes", enum_char_types),
    ("foldCase", fold_case),
    ("forDigit", for_digit),
    ("getBidiPairedBracket", get_bidi_paired_bracket),
    ("getBlockCode", get_block_code),
    ("getCombiningClass", get_combining_class),
    ("getFC_NFKC_Closure", get_fc_nfkc_closure),
    ("getIntPropertyMaxValue", get_int_property_max_value),
    ("getIntPropertyMinValue", get_int_property_min_value),
    ("getIntPropertyValue", get_int_property_value),
    ("getNumericValue", get_numeric_value),
    ("getPropertyEnum", get_property_enum),
    ("getPropertyName", get_property_name),
    ("getPropertyValueEnum", get_property_value_enum),
    ("getPropertyValueName", get_property_value_name),
    ("getUnicodeVersion", get_unicode_version),
    ("isalnum", isalnum),
    ("isalpha", isalpha),
    ("isbase", isbase),
    ("isblank", isblank),
    ("iscntrl", iscntrl),
    ("isdefined", isdefined),
    ("isdigit", isdigit),
    ("isgraph", isgraph),
    ("isIDIgnorable", is_id_ignorable),
    ("isIDPart", is_id_part),
    ("isIDStart", is_id_start),
    ("isISOControl", is_iso_control),
    ("isJavaIDPart", is_java_id_part),
    ("isJavaIDStart", is_java_id_start),
    ("isJavaSpaceChar", is_java_space_char),
    ("islower", islower),
    ("isMirrored", is_mirrored),
    ("isprint", isprint),
    ("ispunct", ispunct),
    ("isspace", isspace),
    ("istitle", istitle),
    ("isUAlphabetic", is_u_alphabetic),
    ("isULowercase", is_u_lowercase),
    ("isupper", isupper),
    ("isUUppercase", is_u_uppercase),
    ("isUWhiteSpace", is_u_white_space),
    ("isWhitespace", is_whitespace),
    ("isxdigit", isxdigit),
    ("ord", ord),
    ("tolower", tolower),
    ("totitle", totitle),
    ("toupper", toupper),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLCHAR, METHODS, |b| b);
}
