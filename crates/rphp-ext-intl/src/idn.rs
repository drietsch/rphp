//! `idn_to_ascii()` / `idn_to_utf8()`: UTS #46 processing as ICU's
//! `uidna_nameToASCII` / `uidna_nameToUnicode` do it for php — the map
//! and normalize steps from ICU4X's mapper, punycode for the `xn--`
//! labels, the validity checks reported as ICU's `UIDNA_ERROR_*` bits in
//! the `$idna_info` array, `IDNA_NONTRANSITIONAL_*` deciding whether the
//! deviation characters (`ß`, `ς`, ZWJ, ZWNJ) map, `isTransitionalDifferent`
//! saying whether they occurred.

use icu::normalizer::uts46::Uts46MapperBorrowed;
use icu::properties::{props::GeneralCategory, CodePointMapData};
use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use crate::shape::{register_functions, FnImpl};
use crate::{generated, str_arg};

const IDNA_ALLOW_UNASSIGNED: i64 = 1;
const IDNA_USE_STD3_RULES: i64 = 2;
const IDNA_CHECK_BIDI: i64 = 4;
const IDNA_CHECK_CONTEXTJ: i64 = 8;
const IDNA_NONTRANSITIONAL_TO_ASCII: i64 = 16;
const IDNA_NONTRANSITIONAL_TO_UNICODE: i64 = 32;
const INTL_IDNA_VARIANT_UTS46: i64 = 1;

const ERROR_EMPTY_LABEL: i64 = 1;
const ERROR_LABEL_TOO_LONG: i64 = 2;
const ERROR_DOMAIN_NAME_TOO_LONG: i64 = 4;
const ERROR_LEADING_HYPHEN: i64 = 8;
const ERROR_TRAILING_HYPHEN: i64 = 16;
const ERROR_HYPHEN_3_4: i64 = 32;
const ERROR_LEADING_COMBINING_MARK: i64 = 64;
const ERROR_DISALLOWED: i64 = 128;
const ERROR_PUNYCODE: i64 = 256;
const ERROR_LABEL_HAS_DOT: i64 = 512;
const ERROR_INVALID_ACE_LABEL: i64 = 1024;
const ERROR_BIDI: i64 = 2048;
const ERROR_CONTEXTJ: i64 = 4096;

/// A processed domain: the result text, its error bits, whether a
/// deviation character occurred.
struct Processed {
    result: String,
    errors: i64,
    transitional_different: bool,
}

fn is_deviation(c: char) -> bool {
    matches!(c, '\u{00DF}' | '\u{03C2}' | '\u{200C}' | '\u{200D}')
}

/// Transitional processing maps the deviation characters first.
fn transitional_map(c: char) -> Option<&'static str> {
    match c {
        '\u{00DF}' => Some("ss"),
        '\u{03C2}' => Some("\u{03C3}"),
        '\u{200C}' | '\u{200D}' => Some(""),
        _ => None,
    }
}

fn is_mark(c: char) -> bool {
    let gc = CodePointMapData::<GeneralCategory>::new().get(c);
    matches!(gc, GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark)
}

/// One label's validity checks (UTS #46 §4.1), as ICU flags them.
fn check_label(label: &[char], flags: i64, errors: &mut i64) {
    if label.is_empty() {
        *errors |= ERROR_EMPTY_LABEL;
        return;
    }
    if label.len() >= 4 && label[2] == '-' && label[3] == '-' {
        *errors |= ERROR_HYPHEN_3_4;
    }
    if label[0] == '-' {
        *errors |= ERROR_LEADING_HYPHEN;
    }
    if label[label.len() - 1] == '-' {
        *errors |= ERROR_TRAILING_HYPHEN;
    }
    if label.contains(&'.') {
        *errors |= ERROR_LABEL_HAS_DOT;
    }
    if is_mark(label[0]) {
        *errors |= ERROR_LEADING_COMBINING_MARK;
    }
    if label.contains(&'\u{FFFD}') {
        *errors |= ERROR_DISALLOWED;
    }
    if flags & IDNA_USE_STD3_RULES != 0 && label.iter().any(|c| c.is_ascii() && !(c.is_ascii_alphanumeric() || *c == '-')) {
        *errors |= ERROR_DISALLOWED;
    }
    if flags & IDNA_CHECK_CONTEXTJ != 0 {
        let mapper = Uts46MapperBorrowed::new();
        for (i, c) in label.iter().enumerate() {
            if matches!(c, '\u{200C}' | '\u{200D}') && !(i > 0 && mapper.is_virama(label[i - 1])) {
                *errors |= ERROR_CONTEXTJ;
            }
        }
    }
    if flags & IDNA_CHECK_BIDI != 0 {
        // RFC 5893's first rule only: a label with RTL characters must not
        // start with a number or a mark.
        let rtl = label.iter().any(|c| matches!(*c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF));
        if rtl && (label[0].is_ascii_digit() || is_mark(label[0])) {
            *errors |= ERROR_BIDI;
        }
    }
}

/// The processing behind both directions: `to_ascii` decides the output
/// form of the non-ASCII labels.
fn process(domain: &str, flags: i64, to_ascii: bool) -> Processed {
    let transitional = if to_ascii {
        flags & IDNA_NONTRANSITIONAL_TO_ASCII == 0
    } else {
        flags & IDNA_NONTRANSITIONAL_TO_UNICODE == 0
    };
    let mapper = Uts46MapperBorrowed::new();
    // map + normalize the whole domain (deviation characters kept)
    let mut pre = String::new();
    let mut transitional_different = false;
    for c in domain.chars() {
        if is_deviation(c) {
            transitional_different = true;
            if transitional {
                if let Some(m) = transitional_map(c) {
                    pre.push_str(m);
                    continue;
                }
            }
        }
        pre.push(c);
    }
    let mapped: String = mapper.map_normalize(pre.chars()).collect();
    let mut errors = 0i64;
    let mut out_labels: Vec<String> = Vec::new();
    let labels: Vec<&str> = mapped.split('.').collect();
    let n = labels.len();
    for (i, label) in labels.iter().enumerate() {
        // a trailing dot leaves an empty last label that is no error
        if label.is_empty() && i == n - 1 && n > 1 {
            out_labels.push(String::new());
            continue;
        }
        let lower = label.to_ascii_lowercase();
        if lower.starts_with("xn--") {
            let ace = &label[4..];
            let decoded = idna::punycode::decode(ace);
            let valid = decoded.as_ref().is_some_and(|d| {
                !d.is_empty()
                    && d.iter().any(|c| !c.is_ascii())
                    && mapper.normalize_validate(d.iter().copied()).collect::<String>() == d.iter().collect::<String>()
                    && !d.iter().any(|c| is_deviation(*c) && transitional)
            });
            match decoded {
                Some(d) if valid => {
                    check_label(&d, flags, &mut errors);
                    if d.contains(&'\u{FFFD}') {
                        errors |= ERROR_INVALID_ACE_LABEL;
                    }
                    if to_ascii {
                        out_labels.push(lower.clone());
                    } else {
                        out_labels.push(d.iter().collect());
                    }
                }
                Some(_) => {
                    errors |= ERROR_INVALID_ACE_LABEL;
                    out_labels.push(format!("{label}\u{FFFD}"));
                }
                None => {
                    errors |= ERROR_PUNYCODE;
                    out_labels.push(format!("{label}\u{FFFD}"));
                }
            }
            continue;
        }
        let mut chars: Vec<char> = label.chars().collect();
        let before = errors;
        check_label(&chars, flags, &mut errors);
        if errors != before {
            // ICU leaves a label with errors in its Unicode form, a leading
            // combining mark replaced by U+FFFD.
            if errors & !before & ERROR_LEADING_COMBINING_MARK != 0 {
                chars[0] = '\u{FFFD}';
            }
            out_labels.push(chars.iter().collect());
            continue;
        }
        if to_ascii && chars.iter().any(|c| !c.is_ascii()) {
            match idna::punycode::encode(&chars) {
                Some(p) => out_labels.push(format!("xn--{p}")),
                None => {
                    errors |= ERROR_PUNYCODE;
                    out_labels.push(label.to_string());
                }
            }
        } else {
            out_labels.push(label.to_string());
        }
    }
    let result = out_labels.join(".");
    if to_ascii {
        for l in &out_labels {
            if l.len() > 63 {
                errors |= ERROR_LABEL_TOO_LONG;
            }
        }
        let len = result.strip_suffix('.').map_or(result.len(), str::len);
        if len > 253 {
            errors |= ERROR_DOMAIN_NAME_TOO_LONG;
        }
    }
    Processed {
        result,
        errors,
        transitional_different,
    }
}

fn info_array(p: &Processed) -> Value {
    let mut a = Array::new();
    a.set(ArrayKey::str(b"result"), Value::string(p.result.as_bytes()));
    a.set(ArrayKey::str(b"isTransitionalDifferent"), Value::Bool(p.transitional_different));
    a.set(ArrayKey::str(b"errors"), Value::Int(p.errors));
    Value::Array(a)
}

fn idn(ctx: &mut Ctx, args: &mut [Value], to_ascii: bool) -> NativeResult {
    let domain = str_arg(args, 0);
    let flags = args.get(1).map_or(IDNA_NONTRANSITIONAL_TO_ASCII | IDNA_NONTRANSITIONAL_TO_UNICODE, Value::to_int);
    let variant = args.get(2).map_or(INTL_IDNA_VARIANT_UTS46, Value::to_int);
    let who = ctx.active_function_name();
    if variant != INTL_IDNA_VARIANT_UTS46 {
        return Err(Unwind::value_error(format!("{who}(): Argument #2 ($flags) must be INTL_IDNA_VARIANT_UTS46")));
    }
    if domain.is_empty() {
        return Err(Unwind::value_error(format!("{who}(): Argument #1 ($domain) must not be empty")));
    }
    if domain.len() >= 255 {
        // ICU's buffer overflows before any info is filled.
        if args.len() > 3 {
            args[3] = Value::Array(Array::new());
        }
        return Ok(Value::Bool(false));
    }
    let text = String::from_utf8_lossy(&domain).into_owned();
    let _ = IDNA_ALLOW_UNASSIGNED;
    let p = process(&text, flags, to_ascii);
    if args.len() > 3 {
        args[3] = info_array(&p);
    }
    if p.errors != 0 {
        return Ok(Value::Bool(false));
    }
    Ok(Value::string(p.result.as_bytes()))
}

fn idn_to_ascii(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    idn(ctx, args, true)
}

fn idn_to_utf8(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    idn(ctx, args, false)
}

static FUNCTIONS: &[FnImpl] = &[("idn_to_ascii", idn_to_ascii), ("idn_to_utf8", idn_to_utf8)];

pub fn register(r: &mut Registry) {
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
