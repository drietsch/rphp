//! `MessageFormatter` and the `msgfmt_*` functions: ICU's `MessageFormat`
//! over the pattern tree `pattern.rs` parses, with php's own layer on
//! top (`msgformat_helpers.cpp`): the argument types it reads off the
//! pattern, the conversion of each php value into ICU's `Formattable`,
//! and the error texts.
//!
//! The subformats are the crate's own: `NumberFormatter`'s decimal,
//! currency, percent, integer and pattern formats, `IntlDateFormatter`'s
//! date and time styles and patterns, the date skeletons through
//! `IntlDatePatternGenerator`, the rule-based spellout, ordinal and
//! duration formats; `plural` and `selectordinal` select through ICU 78's
//! plural rules (`plurals.rs`).

mod format;
pub(crate) mod pattern;
#[rustfmt::skip]
mod plural_data;
pub(crate) mod plurals;

use std::rc::Rc;

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Payload, Value};

use crate::datefmt::{MsgDate, ZoneRef};
use crate::numfmt::{MsgNumber, MsgStyle};
use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, IntlError, U_ARGUMENT_TYPE_MISMATCH, U_ILLEGAL_ARGUMENT_ERROR, U_INVALID_CHAR_FOUND, U_PATTERN_SYNTAX_ERROR, U_USING_DEFAULT_WARNING, U_ZERO_ERROR};
use crate::{generated, locale};
use pattern::{Kind, Message};

/// `U_MESSAGE_PARSE_ERROR`.
const U_MESSAGE_PARSE_ERROR: i64 = 6;
/// `U_UNSUPPORTED_ERROR`.
const U_UNSUPPORTED_ERROR: i64 = 16;

/// `ULOC_FULLNAME_CAPACITY - 1`.
const MAX_LOCALE_LEN: usize = 156;

// IntlDateFormatter's style constants
const FULL: i64 = 0;
const LONG: i64 = 1;
const MEDIUM: i64 = 2;
const SHORT: i64 = 3;
const NONE: i64 = -1;

/// ICU's `Formattable::Type` as far as MessageFormat uses it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FType {
    /// `kObject`: nothing known (a gap in the numbered arguments).
    Object,
    String,
    Double,
    /// `kLong`: a 32-bit integer.
    Long,
    Int64,
    Date,
}

/// A `Formattable` value.
#[derive(Clone, Debug)]
pub(crate) enum Fmt {
    Str(String),
    Double(f64),
    Int(i64),
    /// Milliseconds since the epoch.
    Date(f64),
}

impl Fmt {
    fn is_numeric(&self) -> bool {
        matches!(self, Fmt::Double(_) | Fmt::Int(_))
    }

    fn as_f64(&self) -> f64 {
        match self {
            Fmt::Double(d) | Fmt::Date(d) => *d,
            Fmt::Int(i) => *i as f64,
            Fmt::Str(_) => 0.0,
        }
    }
}

/// A subformat of a simple argument.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Sub {
    Number(MsgNumber),
    Date(MsgDate),
    Rbnf(crate::rbnf::Rbnf),
}

/// A compiled pattern: the tree, its subformats (by `slot`), ICU's
/// argument type list and what php's layer reads off the pattern.
pub(crate) struct Compiled {
    pub root: Message,
    pub subs: Vec<Sub>,
    /// `getArgTypeList()`: by argument number, the last type declared.
    pub arg_types: Vec<FType>,
    pub conflicts: bool,
    /// `usesNamedArguments()`.
    pub named: bool,
    /// The creation's warning (`U_USING_DEFAULT_WARNING` once a number
    /// format was built).
    pub warning: i64,
}

/// A formatter's state.
pub struct MsgState {
    /// The locale's ICU name (`getLocale()`).
    locale: String,
    /// The locale the data is read for.
    data_locale: String,
    /// The pattern as given (`getPattern()`).
    source: String,
    /// `None` after a failed `setPattern()`: ICU's cleared pattern.
    compiled: Option<Rc<Compiled>>,
    pub err: IntlError,
}

/// A creation failure: the ICU code and, for a syntax error, php's text
/// of the position.
struct CreateError {
    code: i64,
    syntax: Option<String>,
    offset: usize,
}

// ---- compiling ------------------------------------------------------------------------------

/// `findKeyword`: the index of the trimmed, lower-cased text in `list`.
fn find_keyword(s: &str, list: &[&str]) -> Option<usize> {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c <= ' ').to_lowercase();
    list.iter().position(|k| *k == t)
}

fn skip_pattern_white(s: &str) -> &str {
    s.trim_start_matches(|c: char| (c as u32) < 0x10000 && pattern::is_white(c as u16))
}

struct Compiler<'a> {
    locale: &'a str,
    top_zone: ZoneRef,
    nested_zone: ZoneRef,
    subs: Vec<Sub>,
    arg_types: Vec<FType>,
    conflicts: bool,
    named: bool,
    warning: i64,
}

impl Compiler<'_> {
    fn walk(&mut self, m: &mut Message, depth: usize) -> Result<(), i64> {
        for item in &mut m.items {
            let pattern::Item::Arg(arg) = item else {
                continue;
            };
            if arg.number.is_none() {
                self.named = true;
            }
            let ty = match &mut arg.kind {
                Kind::None => FType::String,
                Kind::Simple { ty, style, slot } => {
                    let (sub, ftype) = self.create(ty, style.as_deref().unwrap_or(""), depth)?;
                    *slot = self.subs.len();
                    self.subs.push(sub);
                    ftype
                }
                Kind::Choice(_) | Kind::Plural { .. } => FType::Double,
                Kind::Select(_) => FType::String,
            };
            if let Some(n) = arg.number {
                let n = n as usize;
                if self.arg_types.len() <= n {
                    self.arg_types.resize(n + 1, FType::Object);
                }
                if self.arg_types[n] != FType::Object && self.arg_types[n] != ty {
                    self.conflicts = true;
                }
                self.arg_types[n] = ty;
            }
            match &mut arg.kind {
                Kind::Choice(cases) => {
                    for c in cases {
                        self.walk(&mut c.message, depth + 1)?;
                    }
                }
                Kind::Plural { cases, .. } => {
                    for (_, m) in cases {
                        self.walk(m, depth + 1)?;
                    }
                }
                Kind::Select(cases) => {
                    for (_, m) in cases {
                        self.walk(m, depth + 1)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `createAppropriateFormat`.
    fn create(&mut self, ty: &str, style: &str, depth: usize) -> Result<(Sub, FType), i64> {
        let zone = if depth == 0 { self.top_zone.clone() } else { self.nested_zone.clone() };
        match find_keyword(ty, &["number", "date", "time", "spellout", "ordinal", "duration"]) {
            Some(0) => {
                let (st, ft) = match find_keyword(style, &["", "currency", "percent", "integer"]) {
                    Some(0) => (MsgStyle::Decimal, FType::Double),
                    Some(1) => (MsgStyle::Currency, FType::Double),
                    Some(2) => (MsgStyle::Percent, FType::Double),
                    Some(3) => (MsgStyle::Integer, FType::Long),
                    _ => {
                        if let Some(skeleton) = skip_pattern_white(style).strip_prefix("::") {
                            return Ok((Sub::Number(MsgNumber::from_skeleton(self.locale, skeleton)?), FType::Double));
                        }
                        (MsgStyle::Pattern(style), FType::Double)
                    }
                };
                self.warning = U_USING_DEFAULT_WARNING_CODE;
                Ok((Sub::Number(MsgNumber::new(self.locale, st)?), ft))
            }
            Some(t @ (1 | 2)) => {
                let trimmed = skip_pattern_white(style);
                if let Some(skeleton) = trimmed.strip_prefix("::") {
                    let pattern = crate::patgen::best_pattern_for(self.locale, skeleton)?;
                    return Ok((Sub::Date(MsgDate::new(self.locale, MEDIUM, MEDIUM, Some(&pattern), zone)?), FType::Date));
                }
                let id = find_keyword(style, &["", "short", "medium", "long", "full"]);
                let st = match id {
                    Some(1) => SHORT,
                    Some(3) => LONG,
                    Some(4) => FULL,
                    _ => MEDIUM,
                };
                let (d, tm) = if t == 1 { (st, NONE) } else { (NONE, st) };
                let pattern = if id.is_none() { Some(style) } else { None };
                Ok((Sub::Date(MsgDate::new(self.locale, d, tm, pattern, zone)?), FType::Date))
            }
            Some(t @ (3..=5)) => {
                self.warning = U_USING_DEFAULT_WARNING_CODE;
                let kind = match t {
                    3 => crate::rbnf::Kind::Spellout,
                    4 => crate::rbnf::Kind::Ordinal,
                    _ => crate::rbnf::Kind::Duration,
                };
                let mut f = crate::rbnf::Rbnf::new(self.locale, kind).ok_or(U_UNSUPPORTED_ERROR)?;
                if !style.is_empty() {
                    // an unknown rule set is ignored
                    let _ = f.set_default_rule_set(style);
                }
                Ok((Sub::Rbnf(f), FType::Double))
            }
            _ => Err(U_ILLEGAL_ARGUMENT_ERROR),
        }
    }
}

const U_USING_DEFAULT_WARNING_CODE: i64 = U_USING_DEFAULT_WARNING;

/// ICU's default zone: the `TZ` environment when it names a zone, else
/// php's own default.
fn icu_default_zone(ctx: &mut Ctx) -> ZoneRef {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':');
        if !tz.is_empty() {
            if let Some(z) = ZoneRef::parse(tz) {
                if z.id != "Etc/Unknown" {
                    return z;
                }
            }
        }
    }
    ZoneRef::from_php(rphp_stdlib::date_bridge::default_zone(ctx))
}

/// Parse and compile a pattern for a locale.
fn compile(ctx: &mut Ctx, data_locale: &str, source: &str) -> Result<Compiled, CreateError> {
    let mut root = pattern::parse(source).map_err(|e| CreateError {
        code: e.code,
        syntax: (e.code == U_PATTERN_SYNTAX_ERROR).then(|| e.describe()),
        offset: e.offset,
    })?;
    let top_zone = ZoneRef::from_php(rphp_stdlib::date_bridge::default_zone(ctx));
    let nested_zone = icu_default_zone(ctx);
    let mut c = Compiler {
        locale: data_locale,
        top_zone,
        nested_zone,
        subs: Vec::new(),
        arg_types: Vec::new(),
        conflicts: false,
        named: false,
        warning: U_ZERO_ERROR,
    };
    c.walk(&mut root, 0).map_err(|code| CreateError { code, syntax: None, offset: 0 })?;
    Ok(Compiled { root, subs: c.subs, arg_types: c.arg_types, conflicts: c.conflicts, named: c.named, warning: c.warning })
}

/// The locale the data is read for: the requested one, or the default
/// when ICU has nothing for its language (`ures_open`'s fallback).
fn data_locale_of(ctx: &Ctx, canonical: &str) -> String {
    let lang = canonical.split(['_', '@']).next().unwrap_or("");
    if lang.is_empty() || crate::data::numfmt::LANGUAGES.binary_search(&lang).is_ok() {
        canonical.to_string()
    } else {
        locale::canonical(&state::default_locale(ctx))
    }
}

/// The locale argument: the ini default when empty.
fn locale_of(ctx: &Ctx, bytes: &[u8]) -> String {
    if bytes.is_empty() {
        state::default_locale(ctx)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

impl MsgState {
    fn build(ctx: &mut Ctx, requested: &str, source: &str) -> Result<MsgState, CreateError> {
        let canonical = locale::canonical(requested);
        let data_locale = data_locale_of(ctx, &canonical);
        if source.is_empty() {
            // umsg_open() with no pattern
            return Err(CreateError { code: U_ILLEGAL_ARGUMENT_ERROR, syntax: None, offset: 0 });
        }
        let compiled = compile(ctx, &data_locale, source)?;
        if compiled.conflicts {
            // umsg_open() refuses what its varargs could not pass
            return Err(CreateError { code: U_ARGUMENT_TYPE_MISMATCH, syntax: None, offset: 0 });
        }
        let warning = compiled.warning;
        Ok(MsgState {
            locale: canonical,
            data_locale,
            source: source.to_string(),
            compiled: Some(Rc::new(compiled)),
            err: IntlError { code: warning, msg: None },
        })
    }
}

// ---- php's argument layer --------------------------------------------------------------------

/// The types php reads off a pattern with named arguments
/// (`umsg_parse_format`): by name or number; `Err` on a conflict.
fn named_types(c: &Compiled) -> Result<Vec<(ArgKey, FType)>, ()> {
    fn walk(m: &Message, out: &mut Vec<(ArgKey, FType)>) -> Result<(), ()> {
        for item in &m.items {
            let pattern::Item::Arg(arg) = item else {
                continue;
            };
            let key = match arg.number {
                Some(n) => ArgKey::Num(i64::from(n)),
                None => ArgKey::Name(arg.name.clone()),
            };
            let ty = match &arg.kind {
                Kind::None => FType::String,
                Kind::Simple { ty, style, .. } => match ty.as_str() {
                    "number" => match style.as_deref() {
                        Some("integer") => FType::Int64,
                        _ => FType::Double,
                    },
                    "date" | "time" => FType::Date,
                    "spellout" | "ordinal" | "duration" => FType::Double,
                    // php leaves its variable unset for a type spelled
                    // otherwise; ICU's own reading stands in
                    other => match find_keyword(other, &["number", "date", "time"]) {
                        Some(0) => FType::Double,
                        Some(_) => FType::Date,
                        None => FType::Double,
                    },
                },
                Kind::Choice(_) | Kind::Plural { .. } => FType::Double,
                Kind::Select(_) => FType::String,
            };
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, t)) => {
                    if *t != ty {
                        return Err(());
                    }
                }
                None => out.push((key, ty)),
            }
            match &arg.kind {
                Kind::Choice(cases) => {
                    for c in cases {
                        walk(&c.message, out)?;
                    }
                }
                Kind::Plural { cases, .. } => {
                    for (_, m) in cases {
                        walk(m, out)?;
                    }
                }
                Kind::Select(cases) => {
                    for (_, m) in cases {
                        walk(m, out)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(&c.root, &mut out)?;
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
enum ArgKey {
    Num(i64),
    Name(String),
}

/// A php-layer failure: code and message.
struct ArgError(i64, String);

/// `is_numeric_string` for a date argument: the number, or `None`.
fn numeric_string(s: &[u8]) -> Option<f64> {
    match rphp_value::numeric_string(s) {
        Some(Value::Int(i)) => Some(i as f64),
        Some(Value::Float(f)) => Some(f),
        _ => None,
    }
}

/// `intl_zval_to_millis`.
fn to_millis(ctx: &mut Ctx, v: &Value) -> Result<f64, ArgError> {
    match v {
        Value::Str(s) => match numeric_string(s.as_bytes()) {
            Some(f) => Ok(f * 1000.0),
            None => Err(ArgError(
                U_ILLEGAL_ARGUMENT_ERROR,
                format!("string '{}' is not numeric, which would be required for it to be a valid date", String::from_utf8_lossy(s.as_bytes())),
            )),
        },
        Value::Int(i) => Ok(*i as f64 * 1000.0),
        Value::Float(f) => Ok(*f * 1000.0),
        Value::Object(o) => {
            if is_instance(ctx, o, b"DateTimeInterface") {
                let (ts, usec, _) = rphp_stdlib::date_bridge::datetime_instant(o);
                Ok(ts as f64 * 1000.0 + f64::from(usec / 1000))
            } else if is_instance(ctx, o, b"IntlCalendar") {
                match crate::cal::instant_of(o) {
                    Some((ts, usec)) => Ok(ts as f64 * 1000.0 + f64::from(usec / 1000)),
                    None => Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "IntlCalendar object is not properly constructed".to_string())),
                }
            } else {
                Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "invalid object type for date/time (only IntlCalendar and DateTimeInterface permitted)".to_string()))
            }
        }
        _ => Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "invalid PHP type for date".to_string())),
    }
}

fn is_instance(ctx: &Ctx, o: &Object, class: &[u8]) -> bool {
    ctx.class_by_name(class).is_some_and(|t| ctx.instanceof_class(o.class_id(), t))
}

/// `zval_get_double`.
fn php_double(ctx: &mut Ctx, v: &Value) -> Result<f64, Unwind> {
    Ok(match v {
        Value::Object(o) => {
            let name = String::from_utf8_lossy(ctx.class_of(o).name.as_ref()).into_owned();
            ctx.warn(&format!("Object of class {name} could not be converted to float"))?;
            1.0
        }
        other => other.to_float(),
    })
}

/// `zval_get_long`.
fn php_long(ctx: &mut Ctx, v: &Value) -> Result<i64, Unwind> {
    Ok(match v {
        Value::Object(o) => {
            let name = String::from_utf8_lossy(ctx.class_of(o).name.as_ref()).into_owned();
            ctx.warn(&format!("Object of class {name} could not be converted to int"))?;
            1
        }
        other => other.to_int(),
    })
}

/// The string conversion of an argument, which must be UTF-8.
fn string_arg(ctx: &mut Ctx, v: &Value) -> Result<Result<Fmt, ArgError>, Unwind> {
    let s = ctx.to_string(v)?;
    Ok(match std::str::from_utf8(s.as_bytes()) {
        Ok(t) => Ok(Fmt::Str(t.to_string())),
        Err(_) => Err(ArgError(U_INVALID_CHAR_FOUND, format!("Invalid UTF-8 data in string argument: '{}'", String::from_utf8_lossy(s.as_bytes())))),
    })
}

/// `umsg_format_helper`'s conversion of the php arguments: the named
/// `Formattable`s, or the first failure.
fn convert_args(ctx: &mut Ctx, c: &Compiled, args: &Array) -> Result<Result<Vec<(String, Fmt)>, ArgError>, Unwind> {
    let named = if c.named {
        match named_types(c) {
            Ok(t) => Some(t),
            Err(()) => return Ok(Err(ArgError(U_ARGUMENT_TYPE_MISMATCH, "Inconsistent types declared for an argument".to_string()))),
        }
    } else {
        None
    };
    let mut out = Vec::with_capacity(args.len());
    for (k, v) in args.iter() {
        let v = v.deref().into_owned();
        let (key, ty) = match k {
            ArrayKey::Int(i) => {
                if *i < 0 || *i > i64::from(i32::MAX) {
                    return Ok(Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "Found negative or too large array key".to_string())));
                }
                let ty = match &named {
                    Some(t) => t.iter().find(|(key, _)| *key == ArgKey::Num(*i)).map(|(_, t)| *t),
                    None => c.arg_types.get(*i as usize).copied(),
                };
                (i.to_string(), ty.unwrap_or(FType::Object))
            }
            ArrayKey::Str(s) => {
                let Ok(name) = std::str::from_utf8(s.as_bytes()) else {
                    return Ok(Err(ArgError(U_INVALID_CHAR_FOUND, format!("Invalid UTF-8 data in argument key: '{}'", String::from_utf8_lossy(s.as_bytes())))));
                };
                let ty = match &named {
                    Some(t) => t.iter().find(|(key, _)| *key == ArgKey::Name(name.to_string())).map(|(_, t)| *t),
                    None => None,
                };
                (name.to_string(), ty.unwrap_or(FType::Object))
            }
        };
        let fmt = match ty {
            FType::String => match string_arg(ctx, &v)? {
                Ok(f) => f,
                Err(e) => return Ok(Err(e)),
            },
            FType::Double => Fmt::Double(php_double(ctx, &v)?),
            FType::Long => match &v {
                Value::Float(f) => {
                    if *f > f64::from(i32::MAX) || *f < f64::from(i32::MIN) {
                        return Ok(Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "Found PHP float with absolute value too large for 32 bit integer argument".to_string())));
                    }
                    Fmt::Int(*f as i32 as i64)
                }
                Value::Int(i) => {
                    if *i > i64::from(i32::MAX) || *i < i64::from(i32::MIN) {
                        return Ok(Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "Found PHP integer with absolute value too large for 32 bit integer argument".to_string())));
                    }
                    Fmt::Int(*i)
                }
                other => Fmt::Int(i64::from(php_long(ctx, other)? as i32)),
            },
            FType::Int64 => match &v {
                Value::Float(f) => {
                    if *f > i64::MAX as f64 || *f < i64::MIN as f64 {
                        return Ok(Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, "Found PHP float with absolute value too large for 64 bit integer argument".to_string())));
                    }
                    Fmt::Int(*f as i64)
                }
                Value::Int(i) => Fmt::Int(*i),
                other => Fmt::Int(php_long(ctx, other)?),
            },
            FType::Date => match to_millis(ctx, &v) {
                Ok(ms) => Fmt::Date(ms),
                Err(_) => {
                    return Ok(Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, format!("The argument for key '{key}' cannot be used as a date or time"))));
                }
            },
            FType::Object => match &v {
                Value::Float(f) => Fmt::Double(*f),
                Value::Int(i) => Fmt::Int(*i),
                Value::Null | Value::Bool(false) => Fmt::Int(0),
                Value::Bool(true) => Fmt::Int(1),
                Value::Str(_) | Value::Object(_) => match string_arg(ctx, &v)? {
                    Ok(f) => f,
                    Err(e) => return Ok(Err(e)),
                },
                _ => {
                    return Ok(Err(ArgError(U_ILLEGAL_ARGUMENT_ERROR, format!("No strategy to convert the value given for the argument with key '{key}' is available"))));
                }
            },
        };
        out.push((key, fmt));
    }
    Ok(Ok(out))
}

// ---- object plumbing ---------------------------------------------------------------------------

fn with_state<R>(o: &Object, f: impl FnOnce(&mut MsgState) -> R) -> Option<R> {
    o.with_payload::<MsgState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// `MSG_FORMAT_METHOD_FETCH_OBJECT`: the formatter must be built.
fn fetch<'a>(ctx: &mut Ctx, o: &'a Object) -> Result<&'a Object, Unwind> {
    state::reset_global(ctx);
    if with_state(o, |st| st.err.reset()).is_none() {
        return Err(Unwind::error("Found unconstructed MessageFormatter"));
    }
    Ok(o)
}

fn object_error(ctx: &mut Ctx, o: &Object, code: i64, msg: &str) -> Result<(), Unwind> {
    let who = ctx.active_function_name();
    let mut err = with_state(o, |st| st.err.clone()).unwrap_or_default();
    let r = state::set_both(ctx, &mut err, &who, code, msg);
    with_state(o, |st| st.err = err);
    r
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    let copy = src.with_payload::<MsgState, _>(|s| MsgState {
        locale: s.locale.clone(),
        data_locale: s.data_locale.clone(),
        source: s.source.clone(),
        compiled: s.compiled.clone(),
        err: IntlError::default(),
    });
    match copy {
        Some(st) => {
            dst.set_payload(Payload::Native(Box::new(st)));
            Ok(())
        }
        None => Err(Unwind::error("Cannot clone uninitialized MessageFormatter")),
    }
}

/// The constructor's and `create()`'s checks: the locale's length and
/// the pattern's encoding.
fn inputs(ctx: &mut Ctx, args: &[Value]) -> Result<(String, Option<String>), (i64, String)> {
    let locale = args.first().map(Value::to_php_bytes).unwrap_or_default();
    if locale.len() > MAX_LOCALE_LEN {
        return Err((U_ILLEGAL_ARGUMENT_ERROR, format!("Locale string too long, should be no longer than {MAX_LOCALE_LEN} characters")));
    }
    let locale = locale_of(ctx, &locale);
    let pattern = args.get(1).map(Value::to_php_bytes).unwrap_or_default();
    Ok((locale, String::from_utf8(pattern).ok()))
}

/// Build a formatter for the constructor or `create()`: the state, or the
/// error code and php's message.
fn build(ctx: &mut Ctx, args: &[Value]) -> Result<MsgState, (i64, String)> {
    let (locale, pattern) = inputs(ctx, args)?;
    let Some(pattern) = pattern else {
        return Err((U_INVALID_CHAR_FOUND, "error converting pattern to UTF-16".to_string()));
    };
    MsgState::build(ctx, &locale, &pattern).map_err(|e| match e.syntax {
        Some(pos) => (e.code, format!("pattern syntax error ({pos})")),
        None => (e.code, "message formatter creation failed".to_string()),
    })
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    state::reset_global(ctx);
    match build(ctx, args) {
        Ok(st) => {
            o.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Null)
        }
        Err((code, msg)) => {
            // the constructor always throws, whatever the ini says
            let who = ctx.active_function_name();
            let prefixed = format!("{who}(): {msg}");
            let g = state::global(ctx);
            g.code = code;
            g.msg = Some(prefixed.clone());
            Err(Unwind::exception("IntlException", prefixed))
        }
    }
}

fn create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    match build(ctx, args) {
        Ok(st) => {
            let warning = st.err.code;
            let cid = ctx.lookup_class_or_error(b"MessageFormatter")?;
            let obj = ctx.instantiate(cid);
            obj.set_payload(Payload::Native(Box::new(st)));
            state::set_global_code(ctx, warning);
            Ok(Value::Object(obj))
        }
        Err((code, msg)) => {
            let who = ctx.active_function_name();
            state::set_global(ctx, &who, code, &msg)?;
            Ok(Value::Null)
        }
    }
}

/// The `array $values` argument.
fn array_arg(ctx: &Ctx, args: &[Value], i: usize, name: &str) -> Result<Array, Unwind> {
    match args.get(i).map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => Ok(a),
        other => {
            let who = ctx.active_function_name();
            let given = other.as_ref().map_or("null".to_string(), |v| rphp_runtime::value_name(v).to_string());
            Err(Unwind::type_error(format!("{who}(): Argument #{} (${name}) must be of type array, {given} given", i + 1)))
        }
    }
}

/// Format: the text, or the failure (code, message).
fn do_format(ctx: &mut Ctx, compiled: Option<Rc<Compiled>>, data_locale: &str, args: &Array) -> Result<Result<String, ArgError>, Unwind> {
    let Some(c) = compiled else {
        // ICU's cleared pattern
        return Ok(match convert_args(ctx, &Compiled::empty(), args)? {
            Ok(_) => Ok("{}".to_string()),
            Err(e) => Err(e),
        });
    };
    let values = match convert_args(ctx, &c, args)? {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    Ok(format::format(&c, data_locale, &values).map_err(|code| ArgError(code, "Call to ICU MessageFormat::format() has failed".to_string())))
}

impl Compiled {
    fn empty() -> Compiled {
        Compiled { root: Message::default(), subs: Vec::new(), arg_types: Vec::new(), conflicts: false, named: false, warning: 0 }
    }
}

fn format_method(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let values = array_arg(ctx, args, 0, "values")?;
    let o = fetch(ctx, o)?;
    let (compiled, data_locale) = with_state(o, |st| (st.compiled.clone(), st.data_locale.clone())).unwrap_or((None, String::new()));
    match do_format(ctx, compiled, &data_locale, &values)? {
        Ok(s) => Ok(Value::string(s.as_bytes())),
        Err(ArgError(code, msg)) => {
            object_error(ctx, o, code, &msg)?;
            Ok(Value::Bool(false))
        }
    }
}

fn format_message(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = ctx.active_function_name();
    let values = array_arg(ctx, args, 2, "values")?;
    let locale = args.first().map(Value::to_php_bytes).unwrap_or_default();
    if locale.len() > MAX_LOCALE_LEN {
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, &format!("Locale string too long, should be no longer than {MAX_LOCALE_LEN} characters"))?;
        return Ok(Value::Null);
    }
    let locale = locale_of(ctx, &locale);
    let Ok(pattern) = String::from_utf8(args.get(1).map(Value::to_php_bytes).unwrap_or_default()) else {
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "error converting pattern to UTF-16")?;
        return Ok(Value::Bool(false));
    };
    let st = match MsgState::build(ctx, &locale, &pattern) {
        Ok(st) => st,
        Err(e) => {
            match e.syntax {
                Some(pos) => state::set_global(ctx, &who, e.code, &format!("pattern syntax error ({pos})"))?,
                None => {
                    // only the message: the code stays what it was
                    let code = state::global(ctx).code;
                    state::set_global(ctx, &who, code, "Creating message formatter failed")?;
                }
            }
            return Ok(Value::Bool(false));
        }
    };
    match do_format(ctx, st.compiled.clone(), &st.data_locale, &values)? {
        Ok(s) => {
            state::set_global_code(ctx, U_ZERO_ERROR);
            Ok(Value::string(s.as_bytes()))
        }
        Err(ArgError(code, msg)) => {
            state::set_global(ctx, &who, code, &msg)?;
            Ok(Value::Bool(false))
        }
    }
}

/// Parse with a built state: the values, or the ICU code.
fn do_parse(st: &MsgState, source: &str) -> Result<Vec<Value>, i64> {
    match &st.compiled {
        Some(c) => format::parse(c, source),
        None => Err(U_MESSAGE_PARSE_ERROR),
    }
}

fn parse_method(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = fetch(ctx, this(o)?)?;
    let Ok(source) = String::from_utf8(args.first().map(Value::to_php_bytes).unwrap_or_default()) else {
        object_error(ctx, o, U_INVALID_CHAR_FOUND, "Converting parse string failed")?;
        return Ok(Value::Bool(false));
    };
    let r = with_state(o, |st| do_parse(st, &source)).unwrap_or(Err(U_MESSAGE_PARSE_ERROR));
    match r {
        Ok(values) => Ok(list_of(values)),
        Err(code) => {
            object_error(ctx, o, code, "Parsing failed")?;
            Ok(Value::Bool(false))
        }
    }
}

fn parse_message(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = ctx.active_function_name();
    let locale = args.first().map(Value::to_php_bytes).unwrap_or_default();
    if locale.len() > MAX_LOCALE_LEN {
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, &format!("Locale string too long, should be no longer than {MAX_LOCALE_LEN} characters"))?;
        return Ok(Value::Null);
    }
    let locale = locale_of(ctx, &locale);
    let Ok(pattern) = String::from_utf8(args.get(1).map(Value::to_php_bytes).unwrap_or_default()) else {
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "error converting pattern to UTF-16")?;
        return Ok(Value::Bool(false));
    };
    let st = match MsgState::build(ctx, &locale, &pattern) {
        Ok(st) => st,
        Err(e) => {
            state::set_global(ctx, &who, e.code, "Creating message formatter failed")?;
            return Ok(Value::Bool(false));
        }
    };
    let Ok(source) = String::from_utf8(args.get(2).map(Value::to_php_bytes).unwrap_or_default()) else {
        state::set_global(ctx, &who, U_INVALID_CHAR_FOUND, "Converting parse string failed")?;
        return Ok(Value::Bool(false));
    };
    match do_parse(&st, &source) {
        Ok(values) => {
            state::set_global_code(ctx, U_ZERO_ERROR);
            Ok(list_of(values))
        }
        Err(code) => {
            state::set_global(ctx, &who, code, "Parsing failed")?;
            Ok(Value::Bool(false))
        }
    }
}

fn set_pattern(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = fetch(ctx, this(o)?)?;
    let Ok(text) = String::from_utf8(args.first().map(Value::to_php_bytes).unwrap_or_default()) else {
        object_error(ctx, o, U_INVALID_CHAR_FOUND, "Error converting pattern to UTF-16")?;
        return Ok(Value::Bool(false));
    };
    let data_locale = with_state(o, |st| st.data_locale.clone()).unwrap_or_default();
    let result = if text.is_empty() { Ok(Compiled::empty()) } else { compile(ctx, &data_locale, &text) };
    match result {
        Ok(c) => {
            with_state(o, |st| {
                st.err.code = c.warning;
                st.compiled = Some(Rc::new(c));
                st.source = text;
            });
            Ok(Value::Bool(true))
        }
        Err(e) => {
            with_state(o, |st| st.compiled = None);
            let msg = format!("Error setting symbol value at line 0, offset {}", e.offset);
            // intl_errors_set_custom_msg: the object's code, the message on both
            let who = ctx.active_function_name();
            let mut err = IntlError { code: e.code, msg: None };
            state::set_global_code(ctx, 0);
            let r = state::set_both(ctx, &mut err, &who, e.code, &msg);
            // the global code stays what the fetch reset it to
            state::set_global_code(ctx, 0);
            with_state(o, |st| st.err = err);
            r?;
            Ok(Value::Bool(false))
        }
    }
}

fn get_pattern(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = fetch(ctx, this(o)?)?;
    Ok(Value::string(with_state(o, |st| st.source.clone()).unwrap_or_default().as_bytes()))
}

fn get_locale(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = fetch(ctx, this(o)?)?;
    Ok(Value::string(with_state(o, |st| st.locale.clone()).unwrap_or_default().as_bytes()))
}

fn get_error_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |st| st.err.code).unwrap_or(0)))
}

fn get_error_message(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::string(with_state(o, |st| st.err.message()).unwrap_or_else(|| "U_ZERO_ERROR".to_string()).as_bytes()))
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("create", create),
    ("format", format_method),
    ("formatMessage", format_message),
    ("parse", parse_method),
    ("parseMessage", parse_message),
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
                Some(Value::Object(o)) if is_instance(ctx, &o, b"MessageFormatter") => o,
                other => {
                    let who = ctx.active_function_name();
                    let given = other.as_ref().map_or("null".to_string(), |v| rphp_runtime::value_name(v).to_string());
                    return Err(Unwind::type_error(format!("{who}(): Argument #1 ($formatter) must be of type MessageFormatter, {given} given")));
                }
            };
            let rest = &mut args[1..];
            $method(ctx, Some(&obj), rest)
        }
    };
}

as_function!(f_format, format_method);
as_function!(f_parse, parse_method);
as_function!(f_set_pattern, set_pattern);
as_function!(f_get_pattern, get_pattern);
as_function!(f_get_locale, get_locale);
as_function!(f_get_error_code, get_error_code);
as_function!(f_get_error_message, get_error_message);

fn f_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, None, args)
}

fn f_format_message(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    format_message(ctx, None, args)
}

fn f_parse_message(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    parse_message(ctx, None, args)
}

static FUNCTIONS: &[FnImpl] = &[
    ("msgfmt_create", f_create),
    ("msgfmt_format", f_format),
    ("msgfmt_format_message", f_format_message),
    ("msgfmt_parse", f_parse),
    ("msgfmt_parse_message", f_parse_message),
    ("msgfmt_set_pattern", f_set_pattern),
    ("msgfmt_get_pattern", f_get_pattern),
    ("msgfmt_get_locale", f_get_locale),
    ("msgfmt_get_error_code", f_get_error_code),
    ("msgfmt_get_error_message", f_get_error_message),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::MESSAGEFORMATTER, METHODS, |b| b.payload_clone(payload_clone));
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}

/// A php list of the parsed values.
fn list_of(values: Vec<Value>) -> Value {
    let mut a = Array::new();
    for v in values {
        a.push(v);
    }
    Value::Array(a)
}
