//! `Collator` and the `collator_*` functions over ICU4X's collator: the
//! locale's tailoring, the attribute set php exposes (strength, alternate
//! handling, case first/level, numeric ordering; French and Hiragana
//! modes are accepted and recorded), `compare()`, sort keys byte-for-byte
//! with ICU's, and php's three sort flavours (`SORT_REGULAR` compares
//! numeric strings as numbers and mixed types as php's `<=>` does).

use std::cmp::Ordering;

use icu::collator::options::{AlternateHandling, CaseLevel, CollatorOptions, Strength};
use icu::collator::preferences::{CollationCaseFirst, CollationNumericOrdering, CollationType};
use icu::collator::{Collator as IcuCollator, CollatorPreferences};
use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Payload, Value};

use crate::locale;
use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, IntlError, U_ILLEGAL_ARGUMENT_ERROR, U_INVALID_CHAR_FOUND};
use crate::tables::{COLLATION_LOCALES, COLLATION_ROOT_ONLY};
use crate::{generated, locale_arg, str_arg};

// attribute ids
const FRENCH_COLLATION: i64 = 0;
const ALTERNATE_HANDLING: i64 = 1;
const CASE_FIRST: i64 = 2;
const CASE_LEVEL: i64 = 3;
const NORMALIZATION_MODE: i64 = 4;
const STRENGTH: i64 = 5;
const HIRAGANA_QUATERNARY_MODE: i64 = 6;
const NUMERIC_COLLATION: i64 = 7;
// attribute values
const DEFAULT_VALUE: i64 = -1;
const PRIMARY: i64 = 0;
const SECONDARY: i64 = 1;
const TERTIARY: i64 = 2;
const QUATERNARY: i64 = 3;
const IDENTICAL: i64 = 15;
const OFF: i64 = 16;
const ON: i64 = 17;
const SHIFTED: i64 = 20;
const NON_IGNORABLE: i64 = 21;
const LOWER_FIRST: i64 = 24;
const UPPER_FIRST: i64 = 25;
// sort flags
const SORT_REGULAR: i64 = 0;
const SORT_STRING: i64 = 1;
const SORT_NUMERIC: i64 = 2;

/// The object's state: the requested locale, the attribute values, the
/// collator built from them, its own last error.
pub struct CollatorState {
    /// The locale as given (ICU form, keywords included).
    requested: String,
    /// The bundle ICU resolves to (`VALID_LOCALE`).
    valid: String,
    /// The tailoring in use (`ACTUAL_LOCALE`).
    actual: String,
    /// Attribute values by id, php's constants; `None` is the default.
    attrs: [Option<i64>; 8],
    /// Locale-implied defaults (French for `fr_CA`, …).
    locale_defaults: [i64; 8],
    collator: Option<IcuCollator>,
    pub err: IntlError,
}

/// The bundle ICU would resolve a locale to: the longest prefix of the
/// canonical id that has a collation bundle, `root` when none.
fn resolve_bundle(canonical: &str) -> String {
    let head = canonical.split('@').next().unwrap_or("");
    let mut cand = head.to_string();
    loop {
        if COLLATION_LOCALES.contains(&cand.as_str()) {
            return cand;
        }
        match cand.rfind('_') {
            Some(i) => cand.truncate(i),
            None => return "root".to_string(),
        }
    }
}

fn build_collator(bcp47: &str, keywords: &[(String, String)], attrs: &[Option<i64>; 8]) -> Option<IcuCollator> {
    let loc: icu::locale::Locale = bcp47.parse().unwrap_or(icu::locale::Locale::UNKNOWN);
    let mut prefs = CollatorPreferences::from(&loc);
    if let Some((_, v)) = keywords.iter().find(|(k, _)| k == "collation") {
        prefs.collation_type = match v.as_str() {
            "phonebook" | "phonebk" => Some(CollationType::Phonebk),
            "traditional" | "trad" => Some(CollationType::Trad),
            "pinyin" => Some(CollationType::Pinyin),
            "stroke" => Some(CollationType::Stroke),
            "zhuyin" => Some(CollationType::Zhuyin),
            "unihan" => Some(CollationType::Unihan),
            "dictionary" | "dict" => Some(CollationType::Dict),
            "emoji" => Some(CollationType::Emoji),
            "eor" => Some(CollationType::Eor),
            "search" => Some(CollationType::Search),
            "compat" => Some(CollationType::Compat),
            "ducet" => Some(CollationType::Ducet),
            "phonetic" => Some(CollationType::Phonetic),
            "searchjl" => Some(CollationType::Searchjl),
            "standard" => Some(CollationType::Standard),
            _ => None,
        };
    }
    match attrs[CASE_FIRST as usize] {
        Some(LOWER_FIRST) => prefs.case_first = Some(CollationCaseFirst::Lower),
        Some(UPPER_FIRST) => prefs.case_first = Some(CollationCaseFirst::Upper),
        Some(OFF) => prefs.case_first = Some(CollationCaseFirst::False),
        _ => {}
    }
    match attrs[NUMERIC_COLLATION as usize] {
        Some(ON) => prefs.numeric_ordering = Some(CollationNumericOrdering::True),
        Some(OFF) => prefs.numeric_ordering = Some(CollationNumericOrdering::False),
        _ => {}
    }
    let mut options = CollatorOptions::default();
    options.strength = match attrs[STRENGTH as usize] {
        Some(PRIMARY) => Some(Strength::Primary),
        Some(SECONDARY) => Some(Strength::Secondary),
        Some(TERTIARY) => Some(Strength::Tertiary),
        Some(QUATERNARY) => Some(Strength::Quaternary),
        Some(IDENTICAL) => Some(Strength::Identical),
        _ => None,
    };
    options.alternate_handling = match attrs[ALTERNATE_HANDLING as usize] {
        Some(SHIFTED) => Some(AlternateHandling::Shifted),
        Some(NON_IGNORABLE) => Some(AlternateHandling::NonIgnorable),
        _ => None,
    };
    options.case_level = match attrs[CASE_LEVEL as usize] {
        Some(ON) => Some(CaseLevel::On),
        Some(OFF) => Some(CaseLevel::Off),
        _ => None,
    };
    IcuCollator::try_new(prefs, options).ok().map(|c| c.static_to_owned())
}

impl CollatorState {
    fn new(requested: &str) -> Self {
        let canonical = locale::canonical(requested);
        let (bcp47, keywords) = locale::split(requested);
        let valid = {
            let mut v = resolve_bundle(&canonical);
            if let Some(i) = canonical.find('@') {
                v.push_str(&canonical[i..]);
            }
            v
        };
        let actual = if COLLATION_ROOT_ONLY.contains(&valid.split('@').next().unwrap_or("")) && !valid.contains('@') {
            "root".to_string()
        } else {
            match valid.split('@').next().unwrap_or("") {
                "nb" | "nb_NO" | "nn" => format!("no{}", &valid[valid.find('@').unwrap_or(valid.len())..]),
                _ => valid.clone(),
            }
        };
        let mut locale_defaults = [OFF, NON_IGNORABLE, OFF, OFF, OFF, TERTIARY, OFF, OFF];
        let lang = bcp47.split('-').next().unwrap_or("");
        if valid.starts_with("fr_CA") {
            locale_defaults[FRENCH_COLLATION as usize] = ON;
        }
        if lang == "da" {
            locale_defaults[CASE_FIRST as usize] = UPPER_FIRST;
        }
        if matches!(lang, "el" | "he" | "hi" | "th" | "vi" | "ar" | "bn" | "km" | "lo" | "my" | "si" | "ta" | "te" | "kn" | "ml" | "mr" | "gu" | "pa" | "or" | "ne" | "as" | "bo" | "dz" | "fa" | "ur" | "ps" | "ug" | "ku" | "yi" | "ka" | "hy" | "am" | "ti" | "chr" | "lkt" | "ff" | "ha" | "ig" | "yo") {
            locale_defaults[NORMALIZATION_MODE as usize] = ON;
        }
        if lang == "th" {
            locale_defaults[ALTERNATE_HANDLING as usize] = SHIFTED;
        }
        let attrs = [None; 8];
        let collator = build_collator(&bcp47, &keywords, &attrs);
        CollatorState {
            requested: requested.to_string(),
            valid,
            actual,
            attrs,
            locale_defaults,
            collator,
            err: IntlError::default(),
        }
    }

    fn rebuild(&mut self) {
        let (bcp47, keywords) = locale::split(&self.requested);
        self.collator = build_collator(&bcp47, &keywords, &self.attrs);
    }

    fn attribute(&self, id: i64) -> Option<i64> {
        if !(0..8).contains(&id) {
            return None;
        }
        Some(self.attrs[id as usize].unwrap_or(self.locale_defaults[id as usize]))
    }

    fn compare(&self, a: &str, b: &str) -> Ordering {
        match &self.collator {
            Some(c) => c.as_borrowed().compare(a, b),
            None => a.cmp(b),
        }
    }

    /// ICU's `ucol_getSortKey` bytes (without its terminating NUL): ICU4X
    /// writes each level buffer whole, ICU drops the NO_CE terminator
    /// (`01`) ending every secondary/tertiary/quaternary buffer — so those
    /// go; a case level (`CASE_LEVEL` on) carries no terminator.
    fn sort_key(&self, s: &str) -> Vec<u8> {
        let mut raw: Vec<u8> = Vec::new();
        if let Some(c) = &self.collator {
            let _ = c.as_borrowed().write_sort_key_to(s, &mut raw);
        }
        if self.attribute(CASE_LEVEL) == Some(ON) {
            return raw;
        }
        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        // the primary level runs up to the first separator
        while i < raw.len() && raw[i] != 1 {
            out.push(raw[i]);
            i += 1;
        }
        while i < raw.len() {
            // a separator, then the level's bytes, then the terminator
            out.push(1);
            i += 1;
            while i < raw.len() && raw[i] != 1 {
                out.push(raw[i]);
                i += 1;
            }
            i += 1;
        }
        out
    }
}

fn with_state<R>(o: &Object, f: impl FnOnce(&mut CollatorState) -> R) -> Option<R> {
    o.with_payload::<CollatorState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// The receiver's state, or php's "Object not initialized" error.
fn state_of<'a>(ctx: &mut Ctx, o: &'a Object) -> Result<&'a Object, Unwind> {
    if o.with_payload::<CollatorState, _>(|_| ()).is_none() {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Object not initialized")?;
        return Err(Unwind::error("Object not initialized"));
    }
    Ok(o)
}

/// `intl_error_set(COLLATOR_ERROR_P(co), …)`: the object's error alone,
/// as the Collator methods set theirs.
fn object_error(o: &Object, who: &str, code: i64, msg: &str) {
    with_state(o, |st| {
        st.err.code = code;
        st.err.msg = Some(format!("{who}(): {msg}"));
    });
}

// ---- construction -----------------------------------------------------------------------

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    state::reset_global(ctx);
    let loc = locale_arg(ctx, args, 0);
    let st = CollatorState::new(&loc);
    o.set_payload(Payload::Native(Box::new(st)));
    Ok(Value::Null)
}

fn create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let loc = locale_arg(ctx, args, 0);
    let cid = ctx.lookup_class_or_error(b"Collator")?;
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(CollatorState::new(&loc))));
    Ok(Value::Object(obj))
}

// ---- comparing --------------------------------------------------------------------------

fn compare(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let a = str_arg(args, 0);
    let b = str_arg(args, 1);
    let who = ctx.active_function_name();
    let (Ok(a), Ok(b)) = (std::str::from_utf8(&a), std::str::from_utf8(&b)) else {
        let which = if std::str::from_utf8(&a).is_err() { "first" } else { "second" };
        let msg = format!("Error converting {which} argument to UTF-16");
        with_state(o, |s| {
            s.err.code = U_INVALID_CHAR_FOUND;
            s.err.msg = Some(format!("{who}(): {msg}"));
        });
        return Ok(Value::Bool(false));
    };
    let ord = with_state(o, |s| {
        s.err.reset();
        s.compare(a, b)
    })
    .unwrap_or(Ordering::Equal);
    Ok(Value::Int(match ord {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }))
}

fn get_sort_key(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let s = str_arg(args, 0);
    let who = ctx.active_function_name();
    let Ok(text) = std::str::from_utf8(&s) else {
        with_state(o, |st| {
            st.err.code = U_INVALID_CHAR_FOUND;
            st.err.msg = Some(format!("{who}(): Error converting first argument to UTF-16"));
        });
        return Ok(Value::Bool(false));
    };
    let key = with_state(o, |st| st.sort_key(text)).unwrap_or_default();
    Ok(Value::string(&key))
}

// ---- sorting ----------------------------------------------------------------------------

/// php's `collator_is_numeric`: a string that is a number, as a float.
fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::Str(s) => {
            let text = std::str::from_utf8(s.as_bytes()).ok()?;
            let t = text.trim_start();
            if t.is_empty() {
                return None;
            }
            match rphp_value::numeric_string(t.as_bytes()) {
                Some(Value::Int(i)) => Some(i as f64),
                Some(Value::Float(f)) => Some(f),
                _ => None,
            }
        }
        _ => None,
    }
}

fn compare_values(st: &CollatorState, a: &Value, b: &Value, flags: i64) -> Ordering {
    let a = a.deref().into_owned();
    let b = b.deref().into_owned();
    match flags {
        SORT_NUMERIC => a.to_float().partial_cmp(&b.to_float()).unwrap_or(Ordering::Equal),
        SORT_STRING => {
            let sa = String::from_utf8_lossy(&a.to_php_bytes()).into_owned();
            let sb = String::from_utf8_lossy(&b.to_php_bytes()).into_owned();
            st.compare(&sa, &sb)
        }
        _ => match (&a, &b) {
            (Value::Str(x), Value::Str(y)) => match (numeric(&a), numeric(&b)) {
                (Some(p), Some(q)) => p.partial_cmp(&q).unwrap_or(Ordering::Equal),
                _ => st.compare(&String::from_utf8_lossy(x.as_bytes()), &String::from_utf8_lossy(y.as_bytes())),
            },
            _ => match a.spaceship(&b) {
                x if x < 0 => Ordering::Less,
                0 => Ordering::Equal,
                _ => Ordering::Greater,
            },
        },
    }
}

fn sort_impl(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value], keep_keys: bool) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let flags = args.get(1).map_or(SORT_REGULAR, Value::to_int);
    let Value::Array(arr) = std::mem::replace(&mut args[0], Value::Null) else {
        return Ok(Value::Bool(false));
    };
    let mut entries: Vec<(ArrayKey, Value)> = arr.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    with_state(o, |st| {
        st.err.reset();
        entries.sort_by(|x, y| compare_values(st, &x.1, &y.1, flags));
    });
    let mut out = Array::new();
    for (k, v) in entries {
        if keep_keys {
            out.set(k, v);
        } else {
            out.push(v);
        }
    }
    args[0] = Value::Array(out);
    Ok(Value::Bool(true))
}

fn sort(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, o, args, false)
}

fn asort(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    sort_impl(ctx, o, args, true)
}

fn sort_with_sort_keys(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let Value::Array(arr) = std::mem::replace(&mut args[0], Value::Null) else {
        return Ok(Value::Bool(false));
    };
    let mut entries: Vec<(Vec<u8>, Value)> = Vec::with_capacity(arr.len());
    with_state(o, |st| {
        st.err.reset();
        for (_, v) in arr.iter() {
            // php keys the string values only; anything else sorts as "".
            let text = match &*v.deref() {
                Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                _ => String::new(),
            };
            entries.push((st.sort_key(&text), v.clone()));
        }
    });
    entries.sort_by(|x, y| x.0.cmp(&y.0));
    let mut out = Array::new();
    for (_, v) in entries {
        out.push(v);
    }
    args[0] = Value::Array(out);
    Ok(Value::Bool(true))
}

// ---- attributes ---------------------------------------------------------------------------

fn get_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let id = args.first().map_or(0, Value::to_int);
    let who = ctx.active_function_name();
    let value = with_state(o, |st| st.attribute(id)).flatten();
    match value {
        Some(v) => {
            with_state(o, |st| st.err.reset());
            Ok(Value::Int(v))
        }
        None => {
            object_error(o, &who, U_ILLEGAL_ARGUMENT_ERROR, "Error getting attribute value");
            Ok(Value::Bool(false))
        }
    }
}

fn valid_attribute_value(id: i64, value: i64) -> bool {
    if value == DEFAULT_VALUE {
        return true;
    }
    match id {
        FRENCH_COLLATION | CASE_LEVEL | NORMALIZATION_MODE | HIRAGANA_QUATERNARY_MODE | NUMERIC_COLLATION => {
            matches!(value, OFF | ON)
        }
        ALTERNATE_HANDLING => matches!(value, SHIFTED | NON_IGNORABLE),
        CASE_FIRST => matches!(value, OFF | LOWER_FIRST | UPPER_FIRST),
        STRENGTH => matches!(value, PRIMARY | SECONDARY | TERTIARY | QUATERNARY | IDENTICAL),
        _ => false,
    }
}

fn set_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let id = args.first().map_or(0, Value::to_int);
    let value = args.get(1).map_or(0, Value::to_int);
    let who = ctx.active_function_name();
    if !(0..8).contains(&id) || !valid_attribute_value(id, value) {
        object_error(o, &who, U_ILLEGAL_ARGUMENT_ERROR, "Error setting attribute value");
        return Ok(Value::Bool(false));
    }
    with_state(o, |st| {
        st.err.reset();
        st.attrs[id as usize] = if value == DEFAULT_VALUE { None } else { Some(value) };
        st.rebuild();
    });
    Ok(Value::Bool(true))
}

fn get_strength(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |st| st.attribute(STRENGTH)).flatten().unwrap_or(TERTIARY)))
}

fn set_strength(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let value = args.first().map_or(TERTIARY, Value::to_int);
    if !valid_attribute_value(STRENGTH, value) {
        // ICU accepts any strength and complains later; php answers true.
        return Ok(Value::Bool(true));
    }
    with_state(o, |st| {
        st.attrs[STRENGTH as usize] = if value == DEFAULT_VALUE { None } else { Some(value) };
        st.rebuild();
    });
    Ok(Value::Bool(true))
}

fn get_locale(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let kind = args.first().map_or(1, Value::to_int);
    let who = ctx.active_function_name();
    if !matches!(kind, 0 | 1) {
        object_error(o, &who, U_ILLEGAL_ARGUMENT_ERROR, "Error getting locale by type");
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
    ("compare", compare),
    ("sort", sort),
    ("sortWithSortKeys", sort_with_sort_keys),
    ("asort", asort),
    ("getAttribute", get_attribute),
    ("setAttribute", set_attribute),
    ("getStrength", get_strength),
    ("setStrength", set_strength),
    ("getLocale", get_locale),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
    ("getSortKey", get_sort_key),
];

/// `collator_*($object, …)`: the object is the first argument.
macro_rules! as_function {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            let obj = match args.first().map(|v| v.deref().into_owned()) {
                Some(Value::Object(o)) => o,
                _ => return Err(Unwind::type_error("Argument #1 ($object) must be of type Collator")),
            };
            let rest = &mut args[1..];
            $method(ctx, Some(&obj), rest)
        }
    };
}

as_function!(f_compare, compare);
as_function!(f_sort, sort);
as_function!(f_sort_with_sort_keys, sort_with_sort_keys);
as_function!(f_asort, asort);
as_function!(f_get_attribute, get_attribute);
as_function!(f_set_attribute, set_attribute);
as_function!(f_get_strength, get_strength);
as_function!(f_set_strength, set_strength);
as_function!(f_get_locale, get_locale);
as_function!(f_get_error_code, get_error_code);
as_function!(f_get_error_message, get_error_message);
as_function!(f_get_sort_key, get_sort_key);

fn f_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, None, args)
}

static FUNCTIONS: &[FnImpl] = &[
    ("collator_create", f_create),
    ("collator_compare", f_compare),
    ("collator_get_attribute", f_get_attribute),
    ("collator_set_attribute", f_set_attribute),
    ("collator_get_strength", f_get_strength),
    ("collator_set_strength", f_set_strength),
    ("collator_sort", f_sort),
    ("collator_sort_with_sort_keys", f_sort_with_sort_keys),
    ("collator_asort", f_asort),
    ("collator_get_locale", f_get_locale),
    ("collator_get_error_code", f_get_error_code),
    ("collator_get_error_message", f_get_error_message),
    ("collator_get_sort_key", f_get_sort_key),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::COLLATOR, METHODS, |b| b.uncloneable());
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
