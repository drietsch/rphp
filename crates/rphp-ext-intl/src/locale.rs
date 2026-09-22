//! `Locale` and the `locale_*` functions: ICU's `uloc_*` locale-ID
//! parsing as php 8.5.10 over ICU 78 shows it (`intl/locale-parse.php`),
//! with ICU4X for what needs data — likely subtags, display names, script
//! direction.
//!
//! An ICU locale ID is `lang[_Script][_REGION][_VARIANT…][@key=value;…]`
//! with `-` accepted for `_`; ICU shortens 3-letter language and region
//! codes with a 2-letter equivalent (`tables.rs`), title-cases the script,
//! upper-cases region and variants, and reads a BCP-47 tail — `-u-…`
//! (keywords, `va` a variant), `-x-…` (private use), `-t-…` and any other
//! singleton — into `@` keywords when canonicalizing. php adds its own
//! grandfathered-tag table, the `parseLocale` / `composeLocale` array
//! shape, the `lookup` / `filterMatches` range matching, and the
//! `acceptFromHttp` walk over ICU's available locales.

use hashbrown::HashMap;
use icu::locale::{Direction, LocaleDirectionality, LocaleExpander};
use icu_experimental::displaynames::multi::{
    LanguageDisplayNames, LocaleDisplayNamesFormatter, RegionDisplayNames, ScriptDisplayNames, VariantDisplayNames,
};
use icu_experimental::displaynames::DisplayNamesOptions;
use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Value};

use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, U_ILLEGAL_ARGUMENT_ERROR};
use crate::tables::{AVAILABLE, LANGUAGES_3, REGIONS_3};
use crate::{generated, locale_arg, opt_arg, text_arg};

/// `INTL_MAX_LOCALE_LEN`: a longer locale ID is refused.
const MAX_LOCALE_LEN: usize = 156;

/// RFC 4646's grandfathered tags, php's table (`LOC_GRANDFATHERED`), with
/// the preferred value where the registry names one.
const GRANDFATHERED: &[(&str, Option<&str>)] = &[
    ("art-lojban", Some("jbo")),
    ("cel-gaulish", None),
    ("en-GB-oed", None),
    ("i-ami", Some("ami")),
    ("i-bnn", Some("bnn")),
    ("i-default", None),
    ("i-enochian", None),
    ("i-hak", Some("hak")),
    ("i-klingon", Some("tlh")),
    ("i-lux", Some("lb")),
    ("i-mingo", None),
    ("i-navajo", Some("nv")),
    ("i-pwn", Some("pwn")),
    ("i-tao", Some("tao")),
    ("i-tay", Some("tay")),
    ("i-tsu", Some("tsu")),
    ("no-bok", Some("nb")),
    ("no-nyn", Some("nn")),
    ("sgn-BE-FR", Some("sfb")),
    ("sgn-BE-NL", Some("vgt")),
    ("sgn-BR", Some("bzs")),
    ("sgn-CH-DE", Some("sgg")),
    ("sgn-CO", Some("csn")),
    ("sgn-DE", Some("gsg")),
    ("sgn-DK", Some("dsl")),
    ("sgn-ES", Some("ssp")),
    ("sgn-FR", Some("fsl")),
    ("sgn-GB", Some("bfi")),
    ("sgn-GR", Some("gss")),
    ("sgn-IE", Some("isg")),
    ("sgn-IT", Some("ise")),
    ("sgn-JP", Some("jsl")),
    ("sgn-MX", Some("mfs")),
    ("sgn-NI", Some("ncs")),
    ("sgn-NL", Some("dse")),
    ("sgn-NO", Some("nsl")),
    ("sgn-PT", Some("psr")),
    ("sgn-SE", Some("swl")),
    ("sgn-US", Some("ase")),
    ("sgn-ZA", Some("sfs")),
    ("zh-cmn", Some("cmn")),
    ("zh-cmn-Hans", Some("cmn-Hans")),
    ("zh-cmn-Hant", Some("cmn-Hant")),
    ("zh-gan", Some("gan")),
    ("zh-guoyu", Some("cmn")),
    ("zh-hakka", Some("hak")),
    ("zh-min", None),
    ("zh-min-nan", Some("nan")),
    ("zh-wuu", Some("wuu")),
    ("zh-xiang", Some("hsn")),
];

fn grandfathered(tag: &str) -> Option<&'static (&'static str, Option<&'static str>)> {
    GRANDFATHERED.iter().find(|(g, _)| g.eq_ignore_ascii_case(tag))
}

/// What ICU's `uloc_canonicalize` makes of a grandfathered tag (its own
/// table, not php's preferred values).
const CANONICAL_GRANDFATHERED: &[(&str, &str)] = &[
    ("art-lojban", "jbo"),
    ("cel-gaulish", "cel__GAULISH"),
    ("en-GB-oed", "en_GB_OED"),
    ("i-ami", "ami"),
    ("i-bnn", "bnn"),
    ("i-default", "en@x=i-default"),
    ("i-enochian", "@x=i-enochian"),
    ("i-hak", "hak"),
    ("i-klingon", "tlh"),
    ("i-lux", "lb"),
    ("i-mingo", "see@x=i-mingo"),
    ("i-navajo", "nv"),
    ("i-pwn", "pwn"),
    ("i-tao", "tao"),
    ("i-tay", "tay"),
    ("i-tsu", "tsu"),
    ("no-bok", "no_BOK"),
    ("no-nyn", "no_NYN"),
    ("zh-gan", "gan"),
    ("zh-guoyu", "zh"),
    ("zh-hakka", "hak"),
    ("zh-min-nan", "nan"),
    ("zh-wuu", "wuu"),
    ("zh-xiang", "hsn"),
];

/// ICU's canonical keyword for a BCP-47 `-u-` key, where one exists.
fn unicode_key_name(key: &str) -> &str {
    match key {
        "ca" => "calendar",
        "co" => "collation",
        "cu" => "currency",
        "nu" => "numbers",
        "hc" => "hours",
        "tz" => "timezone",
        "ks" => "colstrength",
        "kf" => "colcasefirst",
        "kn" => "colnumeric",
        "kb" => "colbackwards",
        "kc" => "colcaselevel",
        "kk" => "colnormalization",
        "kr" => "colreorder",
        "ka" => "colalternate",
        "kh" => "colhiraganaquaternary",
        "ms" => "measure",
        "vt" => "variabletop",
        other => other,
    }
}

/// ICU's legacy type for a BCP-47 `-u-` value under `key`.
fn unicode_type_value(key: &str, value: &str) -> String {
    let mapped = match (key, value) {
        ("ca", "ethioaa") => "ethiopic-amete-alem",
        ("ca", "gregory") => "gregorian",
        ("ca", "islamicc") => "islamic-civil",
        ("co", "dict") => "dictionary",
        ("co", "gb2312") => "gb2312han",
        ("co", "phonebk") => "phonebook",
        ("co", "trad") => "traditional",
        ("nu", "traditio") => "traditional",
        ("ks", "level1") => "primary",
        ("ks", "level2") => "secondary",
        ("ks", "level3") => "tertiary",
        ("ks", "level4") => "quaternary",
        ("ks", "identic") => "identical",
        ("kf", "false") => "no",
        ("kn" | "kb" | "kc" | "kk" | "kh", "true") => "yes",
        ("kn" | "kb" | "kc" | "kk" | "kh", "false") => "no",
        ("ka", "noignore") => "non-ignorable",
        ("ms", "uksystem") => "imperial",
        ("tz", v) => return tz_from_bcp47(v),
        (_, v) => v,
    };
    mapped.to_string()
}

/// A BCP-47 time zone id (`usnyc`) as its canonical IANA name, or itself.
fn tz_from_bcp47(v: &str) -> String {
    if v == "true" {
        return v.to_string();
    }
    let Ok(subtag) = icu::locale::subtags::Subtag::try_from_str(v) else {
        return v.to_string();
    };
    let wanted = icu::time::TimeZone(subtag);
    let parser = icu::time::zone::iana::IanaParserExtended::new();
    parser
        .iter()
        .find(|e| e.time_zone == wanted)
        .map_or_else(|| v.to_string(), |e| e.canonical.to_string())
}

/// The pieces of an ICU locale ID, scanned the `uloc_*` way.
#[derive(Debug, Default, Clone)]
struct Parsed {
    language: String,
    script: String,
    region: String,
    /// Variants, upper-cased, in order.
    variants: Vec<String>,
    /// `@` keywords, in source order (`canonicalize` sorts them).
    keywords: Vec<(String, String)>,
    /// A private-use tail (`-x-…`), its subtags.
    private: Vec<String>,
    /// Whether the head used `-` separators only.
    hyphenated: bool,
    /// A tag that is only private use (`x-…`): no language in its
    /// canonical form.
    private_only: bool,
    /// Subtags of a singleton `uloc_getVariant` still reports as variants
    /// (any singleton but `u`, `x` and `t`).
    singleton_variants: Vec<String>,
}

fn is_sep(b: u8) -> bool {
    b == b'_' || b == b'-'
}

/// `_getLanguage` + `_getScript` + `_getCountry` + `_getVariant` with ICU
/// 78's singleton handling: `singleton_aware` turns a `-u-`/`-x-`/`-t-`
/// tail (and any other singleton) into keywords instead of variants.
fn scan(id: &str, singleton_aware: bool) -> Parsed {
    let mut p = Parsed::default();
    let (head, tail) = match id.find('@') {
        Some(i) => (&id[..i], Some(&id[i + 1..])),
        None => (id, None),
    };
    let head = match head.find('.') {
        Some(i) => &head[..i],
        None => head,
    };
    if let Some(tail) = tail {
        for kv in tail.split(';') {
            if let Some((k, v)) = kv.split_once('=') {
                let k = k.trim().to_ascii_lowercase();
                if !k.is_empty() {
                    p.keywords.push((k, v.trim().to_string()));
                }
            }
        }
    }
    p.hyphenated = head.contains('-') && !head.contains('_');
    let bytes = head.as_bytes();
    let mut pos = 0;
    // language
    let start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
        pos += 1;
    }
    let lang = head[start..pos].to_ascii_lowercase();
    // `x-…` / `i-…`: ICU keeps the whole private/irregular tag as the language.
    if lang.len() == 1 && matches!(lang.as_str(), "x" | "i") && pos < bytes.len() && is_sep(bytes[pos]) {
        p.language = head.to_string();
        if lang == "x" {
            p.private = head[pos + 1..].split(|c| c == '-' || c == '_').filter(|s| !s.is_empty()).map(str::to_string).collect();
            p.keywords.push(("x".into(), p.private.join("-")));
            p.private_only = true;
        }
        return p;
    }
    p.language = match lang.as_str() {
        "root" | "und" => String::new(),
        l if l.len() == 3 => LANGUAGES_3.iter().find(|(t, _)| *t == l).map_or(l.to_string(), |(_, two)| (*two).to_string()),
        l => l.to_string(),
    };
    if pos >= bytes.len() || !is_sep(bytes[pos]) {
        return p;
    }
    // script: exactly four letters up to a separator or the end
    let after = pos + 1;
    let seg_end = |from: usize| {
        let mut e = from;
        while e < bytes.len() && !is_sep(bytes[e]) {
            e += 1;
        }
        e
    };
    let mut cur = after;
    let e = seg_end(cur);
    let seg = &head[cur..e];
    if seg.len() == 4 && seg.bytes().all(|b| b.is_ascii_alphabetic()) {
        let mut s = seg.to_ascii_lowercase();
        s[..1].make_ascii_uppercase();
        p.script = s;
        cur = e;
        if cur >= bytes.len() {
            return p;
        }
        cur += 1;
    }
    // region: two letters, three letters or three digits
    let e = seg_end(cur);
    let seg = &head[cur..e];
    let alpha = seg.bytes().all(|b| b.is_ascii_alphabetic());
    if (seg.len() == 2 && alpha) || (seg.len() == 3 && (alpha || seg.bytes().all(|b| b.is_ascii_digit()))) {
        let up = seg.to_ascii_uppercase();
        p.region = if up.len() == 3 && alpha {
            REGIONS_3.iter().find(|(t, _)| *t == up).map_or(up.clone(), |(_, two)| (*two).to_string())
        } else {
            up
        };
        cur = e;
        if cur >= bytes.len() {
            return p;
        }
        cur += 1;
    }
    // variants and the BCP-47 tail
    let rest: Vec<&str> = head[cur.min(head.len())..].split(|c| c == '-' || c == '_').collect();
    let mut i = 0;
    while i < rest.len() {
        let t = rest[i];
        if t.is_empty() {
            i += 1;
            continue;
        }
        if singleton_aware && t.len() == 1 && t.is_ascii() {
            let s = t.to_ascii_lowercase();
            // subtags up to the next singleton
            let mut j = i + 1;
            while j < rest.len() && !(rest[j].len() == 1 && rest[j].is_ascii() && s != "x") {
                j += 1;
            }
            let subs: Vec<String> = rest[i + 1..j].iter().filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()).collect();
            match s.as_str() {
                "x" => {
                    p.private = subs;
                    p.keywords.push(("x".into(), p.private.join("-")));
                    return p;
                }
                "u" => {
                    // key (2 chars) then type subtags until the next key
                    let mut k = 0;
                    while k < subs.len() {
                        let key = &subs[k];
                        if key.len() != 2 {
                            k += 1;
                            continue;
                        }
                        let mut m = k + 1;
                        while m < subs.len() && subs[m].len() != 2 {
                            m += 1;
                        }
                        let value = if m == k + 1 { "true".to_string() } else { subs[k + 1..m].join("-") };
                        if key == "va" {
                            p.variants.push(value.to_ascii_uppercase());
                        } else {
                            p.keywords.push((unicode_key_name(key).to_string(), unicode_type_value(key, &value)));
                        }
                        k = m;
                    }
                }
                other if matches!(other, "t") => {
                    p.keywords.push((other.to_string(), subs.join("-")));
                }
                other => {
                    p.keywords.push((other.to_string(), subs.join("-")));
                    p.singleton_variants.push(other.to_ascii_uppercase());
                    p.singleton_variants.extend(subs.iter().map(|s| s.to_ascii_uppercase()));
                }
            }
            i = j;
            continue;
        }
        p.variants.push(t.to_ascii_uppercase());
        i += 1;
    }
    p
}

impl Parsed {
    /// `uloc_canonicalize`'s spelling: `lang_Script_REGION_VARIANT@k=v;…`
    /// with the keywords sorted by name.
    fn canonical(&self) -> String {
        let mut out = if self.private_only { String::new() } else { self.language.clone() };
        if !self.script.is_empty() {
            out.push('_');
            out.push_str(&self.script);
        }
        if !self.region.is_empty() || !self.variants.is_empty() {
            out.push('_');
            out.push_str(&self.region);
        }
        for v in &self.variants {
            out.push('_');
            out.push_str(v);
        }
        if !self.keywords.is_empty() {
            let mut kws = self.keywords.clone();
            kws.sort_by(|a, b| a.0.cmp(&b.0));
            kws.dedup_by(|a, b| a.0 == b.0);
            out.push('@');
            let parts: Vec<String> = kws.iter().map(|(k, v)| format!("{k}={v}")).collect();
            out.push_str(&parts.join(";"));
        }
        out
    }

    /// The BCP-47 form ICU4X parses (`en-Latn-US`; `und` for no language).
    fn bcp47(&self) -> String {
        let mut out = if self.language.is_empty() { "und".to_string() } else { self.language.clone() };
        if !self.script.is_empty() {
            out.push('-');
            out.push_str(&self.script);
        }
        if !self.region.is_empty() {
            out.push('-');
            out.push_str(&self.region);
        }
        for v in &self.variants {
            if (5..=8).contains(&v.len()) || (v.len() == 4 && v.as_bytes()[0].is_ascii_digit()) {
                out.push('-');
                out.push_str(&v.to_ascii_lowercase());
            }
        }
        out
    }
}

/// The scan behind the single-subtag accessors: singleton-aware only for
/// a hyphenated tag, as `uloc_getVariant` is.
fn scan_subtags(id: &str) -> Parsed {
    let hyphenated = id.contains('-') && !id.split('@').next().unwrap_or("").contains('_');
    scan(id, hyphenated)
}

fn too_long(ctx: &mut Ctx, id: &str) -> Result<bool, Unwind> {
    if id.len() > MAX_LOCALE_LEN {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Locale string too long, should be no longer than 156 characters")?;
        return Ok(true);
    }
    Ok(false)
}

// ---- the accessors --------------------------------------------------------------

fn get_primary_language(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    if grandfathered(&id).is_some() {
        return Ok(Value::string(id.as_bytes()));
    }
    Ok(Value::string(scan_subtags(&id).language.as_bytes()))
}

fn get_script(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    if grandfathered(&id).is_some() {
        return Ok(Value::Null);
    }
    Ok(Value::string(scan_subtags(&id).script.as_bytes()))
}

fn get_region(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    if grandfathered(&id).is_some() {
        return Ok(Value::Null);
    }
    Ok(Value::string(scan_subtags(&id).region.as_bytes()))
}

fn get_all_variants(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    let mut a = Array::new();
    if grandfathered(&id).is_none() {
        let p = scan_subtags(&id);
        for v in p.variants.iter().chain(p.singleton_variants.iter()) {
            a.push(Value::string(v.as_bytes()));
        }
    }
    Ok(Value::Array(a))
}

fn get_keywords(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    // `uloc_openKeywords` reads a BCP-47 tail only from a hyphenated tag.
    let p = scan(&id, id.contains('-') && !id.contains('_'));
    if p.keywords.is_empty() {
        return Ok(Value::Null);
    }
    let mut kws = p.keywords;
    kws.sort_by(|a, b| a.0.cmp(&b.0));
    kws.dedup_by(|a, b| a.0 == b.0);
    let mut a = Array::new();
    for (k, v) in kws {
        a.set(ArrayKey::str(k.as_bytes()), Value::string(v.as_bytes()));
    }
    Ok(Value::Array(a))
}

/// `uloc_canonicalize` as php exposes it.
fn canonical_of(id: &str) -> String {
    if let Some((_, c)) = CANONICAL_GRANDFATHERED.iter().find(|(g, _)| g.eq_ignore_ascii_case(id)) {
        return (*c).to_string();
    }
    scan(id, true).canonical()
}

fn canonicalize(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    Ok(Value::string(canonical_of(&id).as_bytes()))
}

fn parse_locale(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    let mut a = Array::new();
    if grandfathered(&id).is_some() {
        a.set(ArrayKey::str(b"grandfathered"), Value::string(id.as_bytes()));
        return Ok(Value::Array(a));
    }
    // php strips the tail from the first singleton before asking ICU for
    // the subtags, and reads the private-use part itself.
    let mut p = scan(&id, true);
    if p.hyphenated || id.contains('-') {
        // php cuts the tag at its first singleton before asking ICU, so a
        // `-u-va-` variant is not seen here.
        let stripped = scan(&id, false);
        let before_singleton: Vec<String> = stripped
            .variants
            .iter()
            .take_while(|v| v.len() != 1)
            .cloned()
            .collect();
        p.variants = before_singleton;
    }
    if !p.language.is_empty() && !p.private_only {
        a.set(ArrayKey::str(b"language"), Value::string(p.language.as_bytes()));
    }
    if !p.script.is_empty() {
        a.set(ArrayKey::str(b"script"), Value::string(p.script.as_bytes()));
    }
    if !p.region.is_empty() {
        a.set(ArrayKey::str(b"region"), Value::string(p.region.as_bytes()));
    }
    for (i, v) in p.variants.iter().enumerate() {
        a.set(ArrayKey::str(format!("variant{i}").as_bytes()), Value::string(v.as_bytes()));
    }
    for (i, v) in p.private.iter().enumerate() {
        a.set(ArrayKey::str(format!("private{i}").as_bytes()), Value::string(v.as_bytes()));
    }
    Ok(Value::Array(a))
}

/// `locale_compose`: the array shape php reads (`language`, `script`,
/// `region`, `variant`/`variantN`, `extlang`/`extlangN`, `private`/
/// `privateN`, or `grandfathered` alone).
fn compose_locale(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let arr = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => a,
        _ => return Ok(Value::Bool(false)),
    };
    if arr.len() == 0 {
        return Ok(Value::Bool(false));
    }
    let get = |key: &str| arr.get(&ArrayKey::str(key.as_bytes())).map(|v| v.deref().into_owned());
    let not_string = |ctx: &mut Ctx| -> NativeResult {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "parameter array element is not a string")?;
        Ok(Value::Bool(false))
    };
    let mut out = String::new();
    match get("grandfathered") {
        Some(Value::Str(s)) => return Ok(Value::string(s.as_bytes())),
        Some(_) => return not_string(ctx),
        None => {}
    }
    match get("language") {
        Some(Value::Str(s)) => out.push_str(&String::from_utf8_lossy(s.as_bytes())),
        Some(_) => return not_string(ctx),
        None => {
            return Err(Unwind::value_error(
                "Locale::composeLocale(): Argument #1 ($subtags) must contain a \"language\" key",
            ))
        }
    }
    // A multi-valued key: a string, a list, or `keyN` entries.
    let multiple = |ctx: &mut Ctx, out: &mut String, key: &str, max: usize| -> Result<bool, Unwind> {
        let prefix = if key == "private" { "_x" } else { "" };
        match get(key) {
            Some(Value::Str(s)) => {
                out.push_str(prefix);
                out.push('_');
                out.push_str(&String::from_utf8_lossy(s.as_bytes()));
                return Ok(true);
            }
            Some(Value::Array(list)) => {
                let mut first = true;
                for (_, v) in list.iter() {
                    let Value::Str(s) = &*v.deref() else {
                        not_string(ctx)?;
                        return Ok(false);
                    };
                    if first {
                        out.push_str(prefix);
                        first = false;
                    }
                    out.push('_');
                    out.push_str(&String::from_utf8_lossy(s.as_bytes()));
                }
                return Ok(true);
            }
            Some(_) => {
                not_string(ctx)?;
                return Ok(false);
            }
            None => {}
        }
        let mut first = true;
        for i in 0..max {
            match get(&format!("{key}{i}")) {
                Some(Value::Str(s)) => {
                    if first {
                        out.push_str(prefix);
                        first = false;
                    }
                    out.push('_');
                    out.push_str(&String::from_utf8_lossy(s.as_bytes()));
                }
                Some(_) => {
                    not_string(ctx)?;
                    return Ok(false);
                }
                None => {}
            }
        }
        Ok(true)
    };
    if !multiple(ctx, &mut out, "extlang", 3)? {
        return Ok(Value::Bool(false));
    }
    for key in ["script", "region"] {
        match get(key) {
            Some(Value::Str(s)) => {
                out.push('_');
                out.push_str(&String::from_utf8_lossy(s.as_bytes()));
            }
            Some(_) => return not_string(ctx),
            None => {}
        }
    }
    if !multiple(ctx, &mut out, "variant", 15)? {
        return Ok(Value::Bool(false));
    }
    if !multiple(ctx, &mut out, "private", 15)? {
        return Ok(Value::Bool(false));
    }
    Ok(Value::string(out.as_bytes()))
}

// ---- the default ------------------------------------------------------------------

fn get_default(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(state::default_locale(ctx).as_bytes()))
}

fn set_default(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let mut name = text_arg(args, 0);
    if name.is_empty() {
        name = state::ICU_DEFAULT_LOCALE.to_string();
    }
    ctx.ini_set("intl.default_locale", &name);
    Ok(Value::Bool(true))
}

// ---- likely subtags, direction --------------------------------------------------

fn with_icu4x_locale(id: &str, f: impl FnOnce(&mut icu::locale::Locale)) -> Option<String> {
    let p = scan(id, true);
    let mut loc: icu::locale::Locale = p.bcp47().parse().ok()?;
    f(&mut loc);
    let lang = loc.id.language.to_string();
    let mut q = Parsed {
        language: if lang == "und" { String::new() } else { lang },
        script: loc.id.script.map(|s| s.to_string()).unwrap_or_default(),
        region: loc.id.region.map(|r| r.to_string()).unwrap_or_default(),
        variants: loc.id.variants.iter().map(|v| v.to_string().to_ascii_uppercase()).collect(),
        keywords: p.keywords.clone(),
        private: p.private.clone(),
        hyphenated: false,
        private_only: p.private_only,
        singleton_variants: Vec::new(),
    };
    // Variants ICU4X cannot carry (`POSIX`-style 5+ letters it does carry;
    // shorter ones it dropped) come back from the scan.
    for v in &p.variants {
        if !q.variants.contains(v) {
            q.variants.push(v.clone());
        }
    }
    Some(q.canonical())
}

fn add_likely_subtags(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    let expander = LocaleExpander::new_extended();
    match with_icu4x_locale(&id, |l| {
        expander.maximize(&mut l.id);
    }) {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => Ok(Value::string(canonical_of(&id).as_bytes())),
    }
}

fn minimize_subtags(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    if too_long(ctx, &id)? {
        return Ok(Value::Bool(false));
    }
    let expander = LocaleExpander::new_extended();
    match with_icu4x_locale(&id, |l| {
        expander.minimize(&mut l.id);
    }) {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => Ok(Value::string(canonical_of(&id).as_bytes())),
    }
}

fn is_right_to_left(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = locale_arg(ctx, args, 0);
    let p = scan(&id, true);
    let Ok(loc) = p.bcp47().parse::<icu::locale::Locale>() else {
        return Ok(Value::Bool(false));
    };
    let dir = LocaleDirectionality::new_common();
    Ok(Value::Bool(matches!(dir.get(&loc.id), Some(Direction::RightToLeft))))
}

// ---- display names ------------------------------------------------------------------

fn display_prefs(disp: &str) -> icu::locale::Locale {
    let p = scan(disp, true);
    p.bcp47().parse().unwrap_or_else(|_| icu::locale::Locale::UNKNOWN)
}

fn get_display_language(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    display_part(ctx, args, Part::Language)
}

fn get_display_script(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    display_part(ctx, args, Part::Script)
}

fn get_display_region(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    display_part(ctx, args, Part::Region)
}

fn get_display_variant(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    display_part(ctx, args, Part::Variant)
}

fn get_display_name(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    display_part(ctx, args, Part::Name)
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Language,
    Script,
    Region,
    Variant,
    Name,
}

fn display_part(ctx: &mut Ctx, args: &mut [Value], part: Part) -> NativeResult {
    state::reset_global(ctx);
    let raw = text_arg(args, 0);
    if raw.len() > MAX_LOCALE_LEN {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "name too long")?;
        return Ok(Value::Bool(false));
    }
    let mut id = if raw.is_empty() { state::default_locale(ctx) } else { raw };
    if part != Part::Name {
        if let Some((_, preferred)) = grandfathered(&id) {
            if part != Part::Language {
                return Ok(Value::Bool(false));
            }
            if let Some(p) = preferred {
                id = p.to_string();
            }
        }
    }
    let disp = match opt_arg(args, 1) {
        Some(v) => String::from_utf8_lossy(&v.to_php_bytes()).into_owned(),
        None => state::default_locale(ctx),
    };
    let prefs = display_prefs(&disp);
    let p = scan(&id, true);
    let options = DisplayNamesOptions::default();
    let text = match part {
        Part::Language => {
            if p.language.is_empty() {
                String::new()
            } else {
                let names = LanguageDisplayNames::try_new(prefs.clone().into(), options);
                match (names, p.language.parse::<icu::locale::subtags::Language>()) {
                    (Ok(n), Ok(l)) => n.of(l).map_or(p.language.clone(), str::to_string),
                    _ => p.language.clone(),
                }
            }
        }
        Part::Script => {
            if p.script.is_empty() {
                String::new()
            } else {
                let names = ScriptDisplayNames::try_new(prefs.clone().into(), options);
                match (names, p.script.parse::<icu::locale::subtags::Script>()) {
                    (Ok(n), Ok(s)) => n.of(s).map_or(p.script.clone(), str::to_string),
                    _ => p.script.clone(),
                }
            }
        }
        Part::Region => {
            if p.region.is_empty() {
                String::new()
            } else {
                let names = RegionDisplayNames::try_new(prefs.clone().into(), options);
                match (names, p.region.parse::<icu::locale::subtags::Region>()) {
                    (Ok(n), Ok(r)) => n.of(r).map_or(p.region.clone(), str::to_string),
                    _ => p.region.clone(),
                }
            }
        }
        Part::Variant => {
            let names = VariantDisplayNames::try_new(prefs.clone().into(), options).ok();
            let parts: Vec<String> = p
                .variants
                .iter()
                .map(|v| {
                    let lower = v.to_ascii_lowercase();
                    names
                        .as_ref()
                        .and_then(|n| lower.parse::<icu::locale::subtags::Variant>().ok().and_then(|vv| n.of(vv).map(str::to_string)))
                        .unwrap_or_else(|| v.clone())
                })
                .collect();
            parts.join(", ")
        }
        Part::Name => {
            let names = LocaleDisplayNamesFormatter::try_new(prefs.clone().into(), options).ok();
            match (names, p.bcp47().parse::<icu::locale::Locale>()) {
                (Some(n), Ok(l)) if !(p.language.is_empty() && p.script.is_empty() && p.region.is_empty()) => {
                    let mut s = n.of(&l).into_owned();
                    if !p.keywords.is_empty() {
                        s = append_keywords_display(s, &p);
                    }
                    s
                }
                _ => p.canonical(),
            }
        }
    };
    Ok(Value::string(text.as_bytes()))
}

/// The `(…, Key=Value)` tail ICU adds for keywords in a display name.
fn append_keywords_display(mut s: String, p: &Parsed) -> String {
    let mut kws = p.keywords.clone();
    kws.sort_by(|a, b| a.0.cmp(&b.0));
    let parts: Vec<String> = kws.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let tail = parts.join(", ");
    if let Some(i) = s.rfind(')') {
        s.insert_str(i, &format!(", {tail}"));
        s
    } else {
        format!("{s} ({tail})")
    }
}

// ---- matching -------------------------------------------------------------------------

/// php's `strToMatch`: lower-case, `-` → `_`.
fn to_match(s: &str) -> String {
    s.chars().map(|c| if c == '-' { '_' } else { c.to_ascii_lowercase() }).collect()
}

fn filter_matches(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let lang_tag = text_arg(args, 0);
    let mut range = text_arg(args, 1);
    let canonical = args.get(2).is_some_and(Value::to_bool);
    if range.is_empty() {
        range = state::default_locale(ctx);
    }
    if range == "*" {
        return Ok(Value::Bool(true));
    }
    if too_long(ctx, &range)? || too_long(ctx, &lang_tag)? {
        return Ok(Value::Bool(false));
    }
    let (tag, rng) = if canonical {
        (to_match(&canonical_of(&lang_tag)), to_match(&canonical_of(&range)))
    } else {
        (to_match(&lang_tag), to_match(&range))
    };
    if tag.is_empty() || rng.is_empty() {
        return Ok(Value::Bool(false));
    }
    if let Some(rest) = tag.strip_prefix(rng.as_str()) {
        let next = rest.as_bytes().first().copied();
        if next.is_none() || next == Some(b'_') || next == Some(b'-') || (canonical && next == Some(b'@')) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

/// php's `getStrrtokenPos`: the range shortened at its last separator,
/// skipping a singleton with it.
fn shorten(range: &str, pos: usize) -> Option<usize> {
    let b = range.as_bytes();
    let mut i = pos;
    while i > 0 {
        i -= 1;
        if b[i] == b'_' || b[i] == b'-' || b[i] == b'@' {
            let r = if i >= 2 && (b[i - 2] == b'_' || b[i - 2] == b'-') { i - 2 } else { i };
            return (r >= 1).then_some(r);
        }
    }
    None
}

fn lookup(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let tags = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => a,
        _ => return Ok(Value::string(b"")),
    };
    let canonical = args.get(2).is_some_and(Value::to_bool);
    let default = opt_arg(args, 3).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    let mut range = text_arg(args, 1);
    if range.is_empty() {
        range = default.clone().unwrap_or_else(|| state::default_locale(ctx));
    }
    if too_long(ctx, &range)? {
        return Ok(Value::Bool(false));
    }
    if tags.len() == 0 {
        return Ok(Value::string(b""));
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    for (_, v) in tags.iter() {
        let Value::Str(s) = &*v.deref() else {
            return Err(Unwind::type_error("Locale::lookup(): Argument #2 ($locale) must only contain string values"));
        };
        let raw = String::from_utf8_lossy(s.as_bytes()).into_owned();
        if raw.contains('\0') {
            return Err(Unwind::value_error("Locale::lookup(): Argument #2 ($locale) must not contain any null bytes"));
        }
        let m = if canonical { to_match(&canonical_of(&raw)) } else { to_match(&raw) };
        entries.push((m, raw));
    }
    let rng = if canonical { to_match(&canonical_of(&range)) } else { to_match(&range) };
    let mut pos = rng.len();
    let mut found: Option<String> = None;
    while pos > 0 {
        if let Some((m, raw)) = entries.iter().find(|(m, _)| m.len() == pos && rng.starts_with(m.as_str())) {
            found = Some(if canonical { m.clone() } else { raw.clone() });
            break;
        }
        match shorten(&rng, pos) {
            Some(p) => pos = p,
            None => break,
        }
    }
    match found {
        Some(s) if !s.is_empty() => Ok(Value::string(s.as_bytes())),
        _ => match default {
            Some(d) => Ok(Value::string(d.as_bytes())),
            None => Ok(Value::string(b"")),
        },
    }
}

/// `uloc_acceptLanguageFromHTTP` over ICU's available locales: the header's
/// tags by descending quality, each tried as spelled, then shortened at
/// its last separator; `false` when nothing matches.
fn accept_from_http(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let header = text_arg(args, 0);
    let mut items: Vec<(f64, usize, String)> = Vec::new();
    for (i, part) in header.split(',').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut q = 1.0f64;
        let mut tag = part;
        if let Some((t, params)) = part.split_once(';') {
            tag = t.trim();
            for param in params.split(';') {
                if let Some((k, v)) = param.split_once('=') {
                    if k.trim() == "q" {
                        q = v.trim().parse().unwrap_or(0.0);
                    }
                }
            }
        }
        if tag.len() > MAX_LOCALE_LEN {
            let who = ctx.active_function_name();
            state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "locale string too long")?;
            return Ok(Value::Bool(false));
        }
        if tag == "*" || q <= 0.0 {
            continue;
        }
        items.push((q, i, tag.to_string()));
    }
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let available: HashMap<String, &str> = AVAILABLE.iter().map(|l| (l.to_ascii_lowercase(), *l)).collect();
    for (_, _, tag) in &items {
        let mut cand = canonical_of(tag).split('@').next().unwrap_or("").to_string();
        while !cand.is_empty() {
            if let Some(found) = available.get(&cand.to_ascii_lowercase()) {
                return Ok(Value::string(found.as_bytes()));
            }
            match cand.rfind('_') {
                Some(i) => cand.truncate(i),
                None => break,
            }
        }
    }
    Ok(Value::Bool(false))
}

// ---- registration ---------------------------------------------------------------------

/// The methods, all static.
static METHODS: &[MethodImpl] = &[
    ("getDefault", get_default),
    ("setDefault", set_default),
    ("getPrimaryLanguage", get_primary_language),
    ("getScript", get_script),
    ("getRegion", get_region),
    ("getKeywords", get_keywords),
    ("getDisplayScript", get_display_script),
    ("getDisplayRegion", get_display_region),
    ("getDisplayName", get_display_name),
    ("getDisplayLanguage", get_display_language),
    ("getDisplayVariant", get_display_variant),
    ("composeLocale", compose_locale),
    ("parseLocale", parse_locale),
    ("getAllVariants", get_all_variants),
    ("filterMatches", filter_matches),
    ("lookup", lookup),
    ("canonicalize", canonicalize),
    ("acceptFromHttp", accept_from_http),
    ("addLikelySubtags", add_likely_subtags),
    ("minimizeSubtags", minimize_subtags),
    ("isRightToLeft", is_right_to_left),
];

macro_rules! as_function {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            $method(ctx, None, args)
        }
    };
}

as_function!(f_get_default, get_default);
as_function!(f_set_default, set_default);
as_function!(f_get_primary_language, get_primary_language);
as_function!(f_get_script, get_script);
as_function!(f_get_region, get_region);
as_function!(f_get_keywords, get_keywords);
as_function!(f_get_display_script, get_display_script);
as_function!(f_get_display_region, get_display_region);
as_function!(f_get_display_name, get_display_name);
as_function!(f_get_display_language, get_display_language);
as_function!(f_get_display_variant, get_display_variant);
as_function!(f_compose, compose_locale);
as_function!(f_parse, parse_locale);
as_function!(f_get_all_variants, get_all_variants);
as_function!(f_filter_matches, filter_matches);
as_function!(f_lookup, lookup);
as_function!(f_canonicalize, canonicalize);
as_function!(f_accept_from_http, accept_from_http);
as_function!(f_add_likely_subtags, add_likely_subtags);
as_function!(f_minimize_subtags, minimize_subtags);
as_function!(f_is_right_to_left, is_right_to_left);

static FUNCTIONS: &[FnImpl] = &[
    ("locale_get_default", f_get_default),
    ("locale_set_default", f_set_default),
    ("locale_get_primary_language", f_get_primary_language),
    ("locale_get_script", f_get_script),
    ("locale_get_region", f_get_region),
    ("locale_get_keywords", f_get_keywords),
    ("locale_get_display_script", f_get_display_script),
    ("locale_get_display_region", f_get_display_region),
    ("locale_get_display_name", f_get_display_name),
    ("locale_get_display_language", f_get_display_language),
    ("locale_get_display_variant", f_get_display_variant),
    ("locale_compose", f_compose),
    ("locale_parse", f_parse),
    ("locale_get_all_variants", f_get_all_variants),
    ("locale_filter_matches", f_filter_matches),
    ("locale_lookup", f_lookup),
    ("locale_canonicalize", f_canonicalize),
    ("locale_accept_from_http", f_accept_from_http),
    ("locale_add_likely_subtags", f_add_likely_subtags),
    ("locale_minimize_subtags", f_minimize_subtags),
    ("locale_is_right_to_left", f_is_right_to_left),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::LOCALE, METHODS, |b| b);
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}

/// The ICU form of a locale ID for another module: `lang_REGION` style
/// through the same scan, keywords kept.
pub(crate) fn canonical(id: &str) -> String {
    canonical_of(id)
}

/// The ID with its likely subtags (`zh_CN` → `zh_Hans_CN`), keywords
/// dropped — how ICU's resource loading names a locale's bundle.
pub(crate) fn maximized(id: &str) -> Option<String> {
    let expander = LocaleExpander::new_extended();
    let p = scan(id, true);
    let mut loc: icu::locale::Locale = p.bcp47().parse().ok()?;
    expander.maximize(&mut loc.id);
    let lang = loc.id.language.to_string();
    if lang == "und" {
        return None;
    }
    let mut out = lang;
    if let Some(s) = loc.id.script {
        out.push('_');
        out.push_str(&s.to_string());
    }
    if let Some(r) = loc.id.region {
        out.push('_');
        out.push_str(&r.to_string());
    }
    for v in &p.variants {
        out.push('_');
        out.push_str(v);
    }
    Some(out)
}

/// The BCP-47 form ICU4X wants, and the `@` keywords of an ID.
pub(crate) fn split(id: &str) -> (String, Vec<(String, String)>) {
    let p = scan(id, true);
    (p.bcp47(), p.keywords)
}

/// The keyword `key` of an ID, if given (`@calendar=…`, `-u-ca-…`).
#[allow(dead_code)]
pub(crate) fn keyword(id: &str, key: &str) -> Option<String> {
    scan(id, true).keywords.into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}
