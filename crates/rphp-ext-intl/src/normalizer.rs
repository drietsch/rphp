//! `Normalizer` and the `normalizer_*` functions over ICU4X's normalizer:
//! NFC/NFD/NFKC/NFKD, ICU's NFKC_Casefold (NFKD → case fold → NFKC, the
//! default ignorables dropped), the raw single-level decomposition
//! `getRawDecomposition()` answers.

use icu::casemap::CaseMapperBorrowed;
use icu::normalizer::properties::{CanonicalDecompositionBorrowed, Decomposed};
use icu::normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use icu::properties::{props::DefaultIgnorableCodePoint, CodePointSetData};
use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, U_ILLEGAL_ARGUMENT_ERROR, U_INVALID_CHAR_FOUND};
use crate::{generated, str_arg};

const FORM_D: i64 = 4;
const FORM_KD: i64 = 8;
const FORM_C: i64 = 16;
const FORM_KC: i64 = 32;
const FORM_KC_CF: i64 = 48;

/// `toNFKC_Casefold`: NFKC(toCasefold(NFKD(x))) without the default
/// ignorables.
fn nfkc_casefold(text: &str) -> String {
    let nfkd = DecomposingNormalizerBorrowed::new_nfkd().normalize(text);
    let folded = CaseMapperBorrowed::new().fold_string(&nfkd);
    let nfkc = ComposingNormalizerBorrowed::new_nfkc().normalize(&folded);
    let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    nfkc.chars().filter(|c| !ignorable.contains(*c)).collect()
}

fn normalize_form(text: &str, form: i64) -> String {
    match form {
        FORM_D => DecomposingNormalizerBorrowed::new_nfd().normalize(text).into_owned(),
        FORM_KD => DecomposingNormalizerBorrowed::new_nfkd().normalize(text).into_owned(),
        FORM_KC => ComposingNormalizerBorrowed::new_nfkc().normalize(text).into_owned(),
        FORM_KC_CF => nfkc_casefold(text),
        _ => ComposingNormalizerBorrowed::new_nfc().normalize(text).into_owned(),
    }
}

fn is_normalized_form(text: &str, form: i64) -> bool {
    match form {
        FORM_D => DecomposingNormalizerBorrowed::new_nfd().is_normalized(text),
        FORM_KD => DecomposingNormalizerBorrowed::new_nfkd().is_normalized(text),
        FORM_KC => ComposingNormalizerBorrowed::new_nfkc().is_normalized(text),
        FORM_KC_CF => nfkc_casefold(text) == text,
        _ => ComposingNormalizerBorrowed::new_nfc().is_normalized(text),
    }
}

fn valid_form(form: i64) -> bool {
    matches!(form, FORM_D | FORM_KD | FORM_C | FORM_KC | FORM_KC_CF)
}

fn form_arg(ctx: &mut Ctx, args: &[Value]) -> Result<i64, Unwind> {
    let form = args.get(1).map_or(FORM_C, Value::to_int);
    if !valid_form(form) {
        let who = ctx.active_function_name();
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($form) must be a a valid normalization form"
        )));
    }
    Ok(form)
}

fn normalize(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let form = form_arg(ctx, args)?;
    let input = str_arg(args, 0);
    let Ok(text) = std::str::from_utf8(&input) else {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_INVALID_CHAR_FOUND, "Error converting input string to UTF-16")?;
        return Ok(Value::Bool(false));
    };
    Ok(Value::string(normalize_form(text, form).as_bytes()))
}

fn is_normalized(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let form = form_arg(ctx, args)?;
    let input = str_arg(args, 0);
    let Ok(text) = std::str::from_utf8(&input) else {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_INVALID_CHAR_FOUND, "Error converting string to UTF-16.")?;
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(is_normalized_form(text, form)))
}

/// `getRawDecomposition`: one code point's single-level decomposition
/// under the form, `null` when it has none.
fn get_raw_decomposition(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let form = args.get(1).map_or(FORM_C, Value::to_int);
    let input = str_arg(args, 0);
    let who = ctx.active_function_name();
    let text = match std::str::from_utf8(&input) {
        Ok(t) => t,
        Err(_) => {
            state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Code point out of range")?;
            return Ok(Value::Null);
        }
    };
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Input string must be exactly one UTF-8 encoded code point long.")?;
        return Ok(Value::Null);
    };
    if !valid_form(form) {
        return Ok(Value::string(b""));
    }
    // ICU's `Normalizer2::getRawDecomposition`: a Hangul syllable
    // decomposes algorithmically under every form; otherwise the single
    // mapping of the form's data — the canonical one when the character
    // has it, else (NFKC data) its compatibility mapping, and for
    // NFKC_Casefold the folded mapping.
    let canonical = match CanonicalDecompositionBorrowed::new().decompose(c) {
        Decomposed::Default => None,
        Decomposed::Singleton(s) => Some(s.to_string()),
        Decomposed::Expansion(a, b) => Some(format!("{a}{b}")),
    };
    let hangul = (0xAC00..=0xD7A3).contains(&(c as u32));
    let result = match form {
        FORM_C | FORM_D => canonical,
        FORM_KC | FORM_KD => canonical.or_else(|| {
            let s = c.to_string();
            let d = DecomposingNormalizerBorrowed::new_nfkd().normalize(&s);
            (d != s).then(|| d.into_owned())
        }),
        _ if hangul => canonical,
        _ => {
            let s = c.to_string();
            let d = nfkc_casefold(&s);
            (d != s).then_some(d)
        }
    };
    Ok(match result {
        Some(s) => Value::string(s.as_bytes()),
        None => Value::Null,
    })
}

fn f_normalize(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    normalize(ctx, None, args)
}

fn f_is_normalized(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    is_normalized(ctx, None, args)
}

fn f_get_raw_decomposition(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    get_raw_decomposition(ctx, None, args)
}

static METHODS: &[MethodImpl] = &[
    ("normalize", normalize),
    ("isNormalized", is_normalized),
    ("getRawDecomposition", get_raw_decomposition),
];

static FUNCTIONS: &[FnImpl] = &[
    ("normalizer_normalize", f_normalize),
    ("normalizer_is_normalized", f_is_normalized),
    ("normalizer_get_raw_decomposition", f_get_raw_decomposition),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::NORMALIZER, METHODS, |b| b);
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
