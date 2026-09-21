//! `ext/intl` in Rust, over ICU4X (ADR-037): php's `Locale`, `Normalizer`,
//! the `grapheme_*` functions, `idn_to_*`, `Collator`, `IntlChar`,
//! `NumberFormatter`, `IntlDateFormatter`, `IntlCalendar`, `IntlTimeZone`,
//! `MessageFormatter`, `Transliterator`, `IntlBreakIterator`,
//! `IntlListFormatter`, `Spoofchecker`, `UConverter`, `ResourceBundle` —
//! the classes, functions and constants php 8.5.10's intl declares (the
//! generated descriptor tables under `generated/` are its arginfo), with
//! CLDR data compiled in.
//!
//! What php decides, this crate decides the same way — measured against
//! php 8.5.10 over ICU 78.3: the error conventions (`state.rs`), which
//! calls answer `false` and which `null`, the messages
//! `intl_get_error_message()` carries, ICU's own locale-tag parsing
//! (`uloc_*`) behind `Locale`, ICU's `DecimalFormat` patterns behind
//! `NumberFormatter`, `SimpleDateFormat` patterns behind
//! `IntlDateFormatter`, `MessageFormat` behind `MessageFormatter`. Where
//! the CLDR data differs between ICU 78 and ICU4X's release the corpus
//! says so.
#![forbid(unsafe_code)]

mod collator;
mod generated;
mod grapheme;
mod idn;
mod locale;
mod normalizer;
mod shape;
mod state;
mod uchar;

pub use state::{error_name, is_failure};

use rphp_runtime::{Ctx, NativeResult, Registry};
use rphp_value::Value;

use crate::shape::register_functions;

/// Register the extension.
pub fn register(r: &mut Registry) {
    if r.interp().class_by_name(b"IntlException").is_some() {
        return;
    }
    r.extension("intl");
    for ini in generated::ini::INI {
        r.interp().ini.register(ini.name, ini.default.unwrap_or(""));
    }
    for c in generated::consts::CONSTANTS {
        let v = shape::const_value(&c.value);
        if c.deprecated {
            r.deprecated_constant(c.name, v, "");
        } else {
            r.constant(c.name, v);
        }
    }
    shape::register_class(r, &generated::classes::INTLEXCEPTION, &[], |b| b);
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
    locale::register(r);
    normalizer::register(r);
    grapheme::register(r);
    idn::register(r);
    collator::register(r);
    uchar::register(r);
}

/// The extension's own functions: the error accessors.
static FUNCTIONS: &[shape::FnImpl] = &[
    ("intl_get_error_code", intl_get_error_code),
    ("intl_get_error_message", intl_get_error_message),
    ("intl_is_failure", intl_is_failure),
    ("intl_error_name", intl_error_name),
];

fn intl_get_error_code(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state::global(ctx).code))
}

fn intl_get_error_message(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(state::global(ctx).message().as_bytes()))
}

fn intl_is_failure(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(is_failure(args[0].to_int())))
}

fn intl_error_name(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::string(error_name(args[0].to_int()).as_bytes()))
}

/// The string bytes of an argument.
pub(crate) fn str_arg(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i).map(Value::to_php_bytes).unwrap_or_default()
}

/// An argument as UTF-8 text (invalid bytes replaced).
pub(crate) fn text_arg(args: &[Value], i: usize) -> String {
    String::from_utf8_lossy(&str_arg(args, i)).into_owned()
}

/// An optional argument that is absent when missing or `null`.
pub(crate) fn opt_arg(args: &[Value], i: usize) -> Option<&Value> {
    args.get(i).filter(|v| !matches!(&**v, Value::Null))
}

/// A `string $locale` argument: the ini default when empty or absent
/// (every intl entry point treats `""` and `null` as "the default").
pub(crate) fn locale_arg(ctx: &Ctx, args: &[Value], i: usize) -> String {
    match opt_arg(args, i) {
        Some(v) => {
            let s = String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
            if s.is_empty() {
                state::default_locale(ctx)
            } else {
                s
            }
        }
        None => state::default_locale(ctx),
    }
}
