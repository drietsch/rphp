//! `IntlListFormatter` (php 8.5): ICU's `ListFormatter` over the list
//! patterns php 8.5.10's ICU carries for every locale, type and width
//! (`data.rs`, read back from the oracle), with ICU's contextual
//! handlers — Spanish `y`→`e` and `o`→`u`, Hebrew `ו`→`ו-`.

mod data;

use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

use crate::shape::{register_class, MethodImpl};
use crate::state::{self, IntlError, U_INVALID_CHAR_FOUND};
use crate::{generated, locale};

/// `ULOC_FULLNAME_CAPACITY - 1`.
const MAX_LOCALE_LEN: usize = 156;

/// A formatter's state: its locale's row, the style and the language
/// the contextual handlers key on.
pub struct ListState {
    row: &'static [&'static str; 54],
    style: usize,
    lang: String,
    pub err: IntlError,
}

/// The contextual pattern swap ICU's `createPatternHandler` builds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    None,
    SpanishE,
    SpanishU,
    HebrewDash,
}

/// Spanish: `y` becomes `e` before `i…` and `hi…` (not `hia…`, `hie…`).
fn should_change_to_e(text: &[u16]) -> bool {
    let lc = |i: usize| text.get(i).map(|&c| c | 0x20);
    if text.is_empty() {
        return false;
    }
    if lc(0) == Some(u16::from(b'h')) && lc(1) == Some(u16::from(b'i')) && (text.len() == 2 || !matches!(lc(2), Some(c) if c == u16::from(b'a') || c == u16::from(b'e'))) {
        return true;
    }
    lc(0) == Some(u16::from(b'i'))
}

/// Spanish: `o` becomes `u` before `o…`, `ho…`, `8…`, and `11` alone or
/// before a space.
fn should_change_to_u(text: &[u16]) -> bool {
    let lc = |i: usize| text.get(i).map(|&c| c | 0x20);
    if text.is_empty() {
        return false;
    }
    if lc(0) == Some(u16::from(b'o')) || text[0] == u16::from(b'8') {
        return true;
    }
    if lc(0) == Some(u16::from(b'h')) && lc(1) == Some(u16::from(b'o')) {
        return true;
    }
    text.len() >= 2 && text[0] == u16::from(b'1') && text[1] == u16::from(b'1') && (text.len() == 2 || text[2] == u16::from(b' '))
}

/// Hebrew: `ו` takes a dash before anything not in the Hebrew script.
fn should_change_to_vav_dash(text: &str) -> bool {
    match text.chars().next() {
        None => false,
        Some(c) => {
            let script = icu::properties::CodePointMapData::<icu::properties::props::Script>::new().get(c);
            script != icu::properties::props::Script::Hebrew
        }
    }
}

impl ListState {
    fn part(&self, i: usize) -> &'static str {
        self.row[self.style * 6 + i]
    }

    fn context(&self) -> Context {
        let is = |infix: &str, suffix: &str, want: &str| infix == want && suffix.is_empty();
        let (two, end) = ((self.part(0), self.part(1)), (self.part(4), self.part(5)));
        match self.lang.as_str() {
            "es" => {
                if is(two.0, two.1, " y ") || is(end.0, end.1, " y ") {
                    Context::SpanishE
                } else if is(two.0, two.1, " o ") || is(end.0, end.1, " o ") {
                    Context::SpanishU
                } else {
                    Context::None
                }
            }
            "he" | "iw" => {
                if is(two.0, two.1, " \u{5D5}") || is(end.0, end.1, " \u{5D5}") {
                    Context::HebrewDash
                } else {
                    Context::None
                }
            }
            _ => Context::None,
        }
    }

    /// The infix of the `2` (`end = false`) or `end` pattern before
    /// `next`, the contextual swap applied.
    fn infix(&self, end: bool, next: &str) -> &'static str {
        let (infix, suffix) = if end { (self.part(4), self.part(5)) } else { (self.part(0), self.part(1)) };
        if !suffix.is_empty() {
            return infix;
        }
        match self.context() {
            Context::SpanishE if infix == " y " && should_change_to_e(&next.encode_utf16().collect::<Vec<_>>()) => " e ",
            Context::SpanishU if infix == " o " && should_change_to_u(&next.encode_utf16().collect::<Vec<_>>()) => " u ",
            Context::HebrewDash if infix == " \u{5D5}" && should_change_to_vav_dash(next) => " \u{5D5}-",
            _ => infix,
        }
    }

    fn format(&self, items: &[String]) -> String {
        match items.len() {
            0 => String::new(),
            1 => items[0].clone(),
            2 => format!("{}{}{}{}", items[0], self.infix(false, &items[1]), items[1], self.part(1)),
            n => {
                let mut out = format!("{}{}{}", items[0], self.part(2), items[1]);
                for item in &items[2..n - 1] {
                    out.push_str(self.part(3));
                    out.push_str(item);
                }
                let last = &items[n - 1];
                out.push_str(self.infix(true, last));
                out.push_str(last);
                out.push_str(self.part(5));
                out
            }
        }
    }
}

/// The data row of a locale: ICU's bundle fallback, the default locale
/// when nothing of the language is there.
fn row_of(ctx: &Ctx, canonical: &str) -> &'static [&'static str; 54] {
    let (name, i) = crate::numfmt_resolve(data::LOCALES, canonical);
    let lang = canonical.split(['_', '@']).next().unwrap_or("");
    if name == "en" && lang != "en" {
        let default = locale::canonical(&state::default_locale(ctx));
        let (_, j) = crate::numfmt_resolve(data::LOCALES, &default);
        return &data::ROWS[j];
    }
    &data::ROWS[i]
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = o.ok_or_else(|| Unwind::error("Non-static method called statically"))?;
    let who = ctx.active_function_name();
    let bytes = args.first().map(Value::to_php_bytes).unwrap_or_default();
    let requested = if bytes.is_empty() { state::default_locale(ctx) } else { String::from_utf8_lossy(&bytes).into_owned() };
    if requested.len() > MAX_LOCALE_LEN {
        return Err(Unwind::value_error(format!("{who}(): Argument #1 ($locale) must be less than or equal to {MAX_LOCALE_LEN} characters")));
    }
    // uloc_getISO3Language() of the id as given
    let lang = requested.split(['_', '-', '@', '.']).next().unwrap_or("").to_ascii_lowercase();
    if lang.is_empty() || crate::data::numfmt::LANGUAGES.binary_search(&lang.as_str()).is_err() {
        return Err(Unwind::value_error(format!("{who}(): Argument #1 ($locale) \"{requested}\" is invalid")));
    }
    let ty = args.get(1).map_or(0, |v| v.deref().to_int());
    let width = args.get(2).map_or(0, |v| v.deref().to_int());
    if !(0..=2).contains(&ty) {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($type) must be one of IntlListFormatter::TYPE_AND, IntlListFormatter::TYPE_OR, or IntlListFormatter::TYPE_UNITS"
        )));
    }
    if !(0..=2).contains(&width) {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #3 ($width) must be one of IntlListFormatter::WIDTH_WIDE, IntlListFormatter::WIDTH_SHORT, or IntlListFormatter::WIDTH_NARROW"
        )));
    }
    let canonical = locale::canonical(&requested);
    let row = row_of(ctx, &canonical);
    let lang = canonical.split(['_', '@']).next().unwrap_or("").to_string();
    o.set_payload(Payload::Native(Box::new(ListState { row, style: (ty * 3 + width) as usize, lang, err: IntlError::default() })));
    Ok(Value::Null)
}

fn format(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = o.ok_or_else(|| Unwind::error("Non-static method called statically"))?;
    let who = ctx.active_function_name();
    let list = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => a,
        other => {
            let given = other.as_ref().map_or("null".to_string(), rphp_runtime::value_name);
            return Err(Unwind::type_error(format!("{who}(): Argument #1 ($strings) must be of type array, {given} given")));
        }
    };
    state::reset_global(ctx);
    o.with_payload::<ListState, _>(|st| st.err.reset());
    let mut items = Vec::with_capacity(list.len());
    for v in list.values() {
        let s = ctx.to_string(v)?;
        match String::from_utf8(s.as_bytes().to_vec()) {
            Ok(t) => items.push(t),
            Err(_) => {
                let mut err = IntlError::default();
                state::set_both(ctx, &mut err, &who, U_INVALID_CHAR_FOUND, "Failed to convert string to UTF-16")?;
                o.with_payload::<ListState, _>(|st| st.err = err);
                return Ok(Value::Bool(false));
            }
        }
    }
    let Some(text) = o.with_payload::<ListState, _>(|st| st.format(&items)) else {
        return Err(Unwind::error("IntlListFormatter object is not initialized"));
    };
    Ok(Value::string(text.as_bytes()))
}

fn get_error_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = o.ok_or_else(|| Unwind::error("Non-static method called statically"))?;
    Ok(Value::Int(o.with_payload::<ListState, _>(|st| st.err.code).unwrap_or(0)))
}

fn get_error_message(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = o.ok_or_else(|| Unwind::error("Non-static method called statically"))?;
    let m = o.with_payload::<ListState, _>(|st| st.err.message()).unwrap_or_else(|| "U_ZERO_ERROR".to_string());
    Ok(Value::string(m.as_bytes()))
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("format", format),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLLISTFORMATTER, METHODS, |b| b.uncloneable());
}
