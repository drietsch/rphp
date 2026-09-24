//! `ResourceBundle`: what rphp can answer without ICU's resource data
//! files — the locale lists (`getLocales()`, `resourcebundle_locales()`)
//! of the main locale tree and of the rule-based number format tree, as
//! php 8.5.10's ICU 78.3 lists them. Opening a bundle and reading its
//! resources (`__construct`, `create`, `get`, `count`, iteration) needs
//! ICU's whole locale data tree, which rphp does not carry: those methods
//! throw rphp's not-implemented `Error`.

use rphp_runtime::{Ctx, NativeResult, Registry};
use rphp_value::{Array, Object, Value};

use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, U_MISSING_RESOURCE_ERROR};
use crate::{generated, tables};

/// `ResourceBundle::getLocales("ICUDATA-rbnf")`.
static RBNF_LOCALES: &[&str] = &[
    "af", "ak", "am", "ar", "ar_SA", "az", "be", "bg", "bs", "ca", "ccp", "chr", "cs", "cy", "da", "de", "de_CH", "ee", "el", "en", "en_001", "en_IN", "eo", "es", "es_419", "es_DO",
    "es_GT", "es_HN", "es_MX", "es_NI", "es_PA", "es_PR", "es_SV", "es_US", "et", "fa", "fa_AF", "ff", "fi", "fil", "fo", "fr", "fr_BE", "fr_CH", "ga", "gu", "he", "hi", "hr", "hu", "hy", "id",
    "is", "it", "ja", "ka", "kk", "kl", "km", "ko", "ky", "lb", "lo", "lrc", "lt", "lv", "mk", "ms", "mt", "my", "nb", "ne", "nl", "nn", "no", "pl", "pt", "pt_PT", "qu", "ro", "ru", "se",
    "sk", "sl", "sq", "sr", "sr_Latn", "su", "sv", "sw", "ta", "th", "tr", "uk", "vec", "vi", "yue", "yue_Hans", "zh", "zh_Hant", "zh_Hant_MO", "zh_Hant_TW",
];

fn get_locales(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let name = crate::str_arg(args, 0);
    let list: &[&str] = match name.as_slice() {
        b"" | b"ICUDATA" => tables::AVAILABLE,
        b"ICUDATA-rbnf" => RBNF_LOCALES,
        other if other.starts_with(b"ICUDATA-") => {
            let who = ctx.active_function_name();
            return Err(rphp_runtime::Unwind::error(format!("{who}(): the locale list of the \"{}\" tree is not implemented by rphp's intl yet", String::from_utf8_lossy(other))));
        }
        _ => {
            let who = ctx.active_function_name();
            return state::fail(ctx, &who, U_MISSING_RESOURCE_ERROR, "Cannot fetch locales list");
        }
    };
    let mut a = Array::new();
    for l in list {
        a.push(Value::string(l.as_bytes()));
    }
    Ok(Value::Array(a))
}

fn f_locales(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    get_locales(ctx, None, args)
}

static METHODS: &[MethodImpl] = &[("getLocales", get_locales)];

static FUNCTIONS: &[FnImpl] = &[("resourcebundle_locales", f_locales)];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::RESOURCEBUNDLE, METHODS, |b| b.uncloneable());
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
