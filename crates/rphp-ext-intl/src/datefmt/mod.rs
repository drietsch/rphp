//! `IntlDateFormatter` and the `datefmt_*` functions: ICU's
//! `SimpleDateFormat` over the style patterns php 8.5.10's ICU carries
//! for every locale (`data/datefmt.rs`) and the calendar names it spells
//! (`data/datenames.rs`), both dumped from the oracle; the zone names
//! come from CLDR through ICU4X (`zone.rs`), the calendar arithmetic from
//! `civil.rs`, the instant and the zone database from php's own ext/date
//! through its bridge.
//!
//! What is here: the five date × five time styles and the four relative
//! date styles, a custom pattern, every field letter a date pattern may
//! carry, `format()` of a timestamp, a `DateTimeInterface`, an
//! `IntlCalendar` or a `localtime` array, `parse()`/`localtime()` in
//! ICU's lenient mode, and the timezone and calendar accessors.

pub(crate) mod civil;
mod parse;
mod render;
pub(crate) mod zone;

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_stdlib::date_bridge::{self, Zone};
use rphp_value::{Array, Object, Payload, Value};

use crate::data::{datefmt, datenames};
use crate::locale;
use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, IntlError, U_ILLEGAL_ARGUMENT_ERROR, U_PARSE_ERROR, U_UNSUPPORTED_ERROR};
use crate::{generated, locale_arg, opt_arg};
use civil::{Fields, WeekData};
use render::Item;

// styles
const FULL: i64 = 0;
const LONG: i64 = 1;
const MEDIUM: i64 = 2;
const SHORT: i64 = 3;
const NONE: i64 = -1;
const RELATIVE_FULL: i64 = 128;
const RELATIVE_LONG: i64 = 129;
const RELATIVE_MEDIUM: i64 = 130;
const RELATIVE_SHORT: i64 = 131;

// calendars
const TRADITIONAL: i64 = 0;
const GREGORIAN: i64 = 1;

/// The style index into a locale's pattern row.
fn style_index(style: i64) -> Option<usize> {
    match style {
        FULL | RELATIVE_FULL => Some(0),
        LONG | RELATIVE_LONG => Some(1),
        MEDIUM | RELATIVE_MEDIUM => Some(2),
        SHORT | RELATIVE_SHORT => Some(3),
        NONE => Some(4),
        _ => None,
    }
}

fn is_relative(style: i64) -> bool {
    (RELATIVE_FULL..=RELATIVE_SHORT).contains(&style)
}

/// A zone as this extension needs it: ICU's id, the IANA id CLDR's names
/// key on, the BCP-47 id, and php's own zone for the offsets.
#[derive(Clone, Debug)]
pub struct ZoneRef {
    pub id: String,
    pub iana: String,
    pub bcp47: String,
    pub php: Zone,
    /// ICU built this one as a `SimpleTimeZone` — a custom offset or a
    /// name its database does not carry — which has no rules but a
    /// nominal one-hour daylight saving.
    pub custom: bool,
}

impl ZoneRef {
    /// ICU's `TimeZone::createTimeZone`: the id as spelled when ICU knows
    /// it, `Etc/Unknown` when it does not.
    pub fn parse(name: &str) -> Option<ZoneRef> {
        use crate::data::tz::ZONES;
        if name == "Etc/Unknown" {
            return Some(ZoneRef::unknown());
        }
        if let Ok(i) = ZONES.binary_search_by(|(id, ..)| (*id).cmp(name)) {
            let (id, _canon, _region, iana, ..) = ZONES[i];
            let php = date_bridge::parse_zone(iana).or_else(|| date_bridge::parse_zone(id))?;
            return Some(ZoneRef { id: id.to_string(), iana: iana.to_string(), bcp47: bcp47_of(iana), php, custom: false });
        }
        // a custom offset: `GMT+05:30` and the spellings ICU takes for it
        let offset = parse_gmt_offset(name)?;
        Some(ZoneRef { id: render_gmt_id(offset), iana: String::new(), bcp47: "unk".to_string(), php: Zone::Offset(offset), custom: true })
    }

    /// The zone of a php `DateTimeZone` / `DateTime`.
    pub fn from_php(zone: Zone) -> ZoneRef {
        match &zone {
            Zone::Id(id) => ZoneRef::parse(id).unwrap_or_else(|| ZoneRef::unknown()),
            Zone::Abbr { name, .. } => ZoneRef::parse(name).unwrap_or_else(|| ZoneRef {
                id: name.clone(),
                iana: String::new(),
                bcp47: "unk".to_string(),
                php: zone.clone(),
                custom: true,
            }),
            Zone::Offset(secs) => ZoneRef { id: render_gmt_id(*secs), iana: String::new(), bcp47: "unk".to_string(), php: zone.clone(), custom: true },
        }
    }

    /// ICU's `Etc/Unknown` singleton.
    pub fn unknown() -> ZoneRef {
        ZoneRef { id: "Etc/Unknown".to_string(), iana: String::new(), bcp47: "unk".to_string(), php: Zone::Offset(0), custom: false }
    }

    /// The zone ICU builds for a name it does not know: the unknown id,
    /// but a `SimpleTimeZone`'s nominal daylight saving.
    pub fn unknown_custom() -> ZoneRef {
        ZoneRef { custom: true, ..ZoneRef::unknown() }
    }

    pub fn utc() -> ZoneRef {
        ZoneRef::parse("UTC").unwrap_or_else(ZoneRef::unknown)
    }

    pub fn offset_at(&self, ts: i64) -> i32 {
        date_bridge::zone_offset_at(&self.php, ts)
    }
}

/// The BCP-47 zone id of an IANA id (`America/New_York` → `usnyc`).
fn bcp47_of(iana: &str) -> String {
    let tz = icu::time::TimeZone::from_iana_id(iana);
    tz.to_string()
}

/// `GMT+5:30`, `GMT+0530`, `+05:30`: ICU takes the `GMT`-prefixed forms.
fn parse_gmt_offset(name: &str) -> Option<i32> {
    let upper = name.to_ascii_uppercase();
    let rest = upper.strip_prefix("GMT")?;
    if rest.is_empty() {
        return Some(0);
    }
    let (sign, rest) = match rest.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, rest.strip_prefix('+')?),
    };
    let rest = rest.to_string();
    let rest = rest.as_str();
    let digits: Vec<char> = rest.chars().filter(|c| *c != ':').collect();
    if digits.is_empty() || digits.len() > 6 || !digits.iter().all(char::is_ascii_digit) {
        return None;
    }
    let text: String = digits.iter().collect();
    let (h, m, s) = match (digits.len(), rest.contains(':')) {
        (1, _) | (2, _) => (text.parse::<i32>().ok()?, 0, 0),
        (3, _) => (text[..1].parse().ok()?, text[1..].parse().ok()?, 0),
        (4, _) => (text[..2].parse().ok()?, text[2..].parse().ok()?, 0),
        (5, _) => (text[..1].parse().ok()?, text[1..3].parse().ok()?, text[3..].parse().ok()?),
        (6, _) => (text[..2].parse().ok()?, text[2..4].parse().ok()?, text[4..].parse().ok()?),
        _ => return None,
    };
    if m > 59 || s > 59 {
        return None;
    }
    Some(sign * (h * 3600 + m * 60 + s))
}

/// The id ICU gives a custom offset zone (`GMT+05:30`, `GMT`).
fn render_gmt_id(secs: i32) -> String {
    if secs == 0 {
        return "GMT".to_string();
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if s == 0 {
        format!("GMT{sign}{h:02}:{m:02}")
    } else {
        format!("GMT{sign}{h:02}:{m:02}:{s:02}")
    }
}

/// The calendar a formatter runs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Calendar {
    Gregorian,
    /// The Buddhist era: the Gregorian fields with the year 543 higher.
    Buddhist,
}

impl Calendar {
    fn name(self) -> &'static str {
        match self {
            Calendar::Gregorian => "gregorian",
            Calendar::Buddhist => "buddhist",
        }
    }

    /// The locale's traditional calendar, where rphp has one.
    fn traditional(canonical: &str) -> Calendar {
        use crate::data::datenames::TRADITIONAL as TRAD;
        let head = canonical.split('@').next().unwrap_or("");
        let mut cand = head.to_string();
        loop {
            if let Ok(i) = TRAD.binary_search_by(|(l, _)| (*l).cmp(cand.as_str())) {
                return match TRAD[i].1 {
                    "buddhist" => Calendar::Buddhist,
                    _ => Calendar::Gregorian,
                };
            }
            match cand.rfind('_') {
                Some(i) => cand.truncate(i),
                None => break,
            }
        }
        Calendar::Gregorian
    }

    fn apply(self, mut f: Fields) -> Fields {
        if self == Calendar::Buddhist {
            f.year = f.ext_year + 543;
            f.era = 1;
        }
        f
    }
}

/// A formatter's state.
pub struct DateState {
    requested: String,
    valid: String,
    date_type: i64,
    time_type: i64,
    /// The date and time halves of the pattern, and the glue between
    /// them, for a relative style; otherwise the whole pattern is in
    /// `pattern`.
    pattern: String,
    relative: Option<(String, String, String)>,
    items: Option<Vec<Item>>,
    zone: ZoneRef,
    /// Whether the zone came from a calendar or a `null` (the default).
    calendar: Calendar,
    /// What `getCalendar()` reports.
    calendar_kind: i64,
    lenient: bool,
    names: &'static [&'static str; datenames::N],
    week: WeekData,
    icu_locale: icu::locale::Locale,
    digits: [char; 10],
    pub err: IntlError,
}

/// The names row of a locale.
pub(crate) fn names_of(canonical: &str) -> (String, &'static [&'static str; datenames::N]) {
    let (name, i) = crate::numfmt_resolve(datenames::LOCALES, canonical);
    (name, &datenames::ROWS[i])
}

/// The style patterns row of a locale.
fn patterns_of(canonical: &str) -> &'static [&'static str; 25] {
    let (_, i) = crate::numfmt_resolve(datefmt::LOCALES, canonical);
    &datefmt::ROWS[i]
}

impl DateState {
    fn new(requested: &str, date_type: i64, time_type: i64, zone: ZoneRef, calendar: Calendar, calendar_kind: i64, pattern: Option<&str>) -> Result<DateState, i64> {
        let canonical = locale::canonical(requested);
        let (valid, names) = names_of(&canonical);
        let bcp47 = locale::split(requested).0;
        let icu_locale: icu::locale::Locale = bcp47.parse().unwrap_or(icu::locale::Locale::UNKNOWN);
        let digits = digits_of(&icu_locale);
        let week = WeekData::for_locale(&bcp47);
        let rows = patterns_of(&canonical);
        let (di, ti) = (style_index(date_type).ok_or(U_ILLEGAL_ARGUMENT_ERROR)?, style_index(time_type).ok_or(U_ILLEGAL_ARGUMENT_ERROR)?);
        let mut relative = None;
        let pattern = match pattern {
            Some(p) => p.to_string(),
            None if is_relative(date_type) => {
                let date = rows[di * 5 + 4].to_string();
                let time = rows[4 * 5 + ti].to_string();
                let glue = names[311 + di].to_string();
                let combined = if time.is_empty() { date.clone() } else { glue.replace("{1}", &date).replace("{0}", &time) };
                relative = Some((date, time, glue));
                combined
            }
            None => rows[di * 5 + ti].to_string(),
        };
        let items = render::items(&pattern);
        Ok(DateState {
            requested: requested.to_string(),
            valid,
            date_type,
            time_type,
            pattern: if items.is_some() { pattern } else { String::new() },
            relative,
            items,
            zone,
            calendar,
            calendar_kind,
            lenient: true,
            names,
            week,
            icu_locale,
            digits,
            err: IntlError::default(),
        })
    }

    fn fields_at(&self, ts: i64, usec: u32) -> (Fields, i32) {
        let offset = self.zone.offset_at(ts);
        (self.calendar.apply(civil::fields_at(ts, usec, offset)), offset)
    }

    fn ctx(&self, ts: i64, offset: i32) -> render::Ctx<'_> {
        render::Ctx {
            names: self.names,
            week: self.week,
            locale: &self.icu_locale,
            zone: &self.zone,
            ts,
            offset,
            dst: 0,
            digits: self.digits,
        }
    }

    /// Format an instant.
    fn format(&self, ts: i64, usec: u32) -> Option<String> {
        let (f, offset) = self.fields_at(ts, usec);
        let ctx = self.ctx(ts, offset);
        match &self.relative {
            Some((date, time, glue)) => {
                // the relative word replaces the date half when the day is
                // one CLDR names
                let now = date_bridge::now_seconds();
                let today = civil::fields_at(now, 0, self.zone.offset_at(now)).days;
                let delta = f.days - today;
                let word = if (-2..=2).contains(&delta) { self.names[(306 + (delta + 2)) as usize] } else { "" };
                let date_text = if word.is_empty() {
                    render::render(&ctx, &f, &render::items(date)?)
                } else {
                    word.to_string()
                };
                if time.is_empty() {
                    return Some(date_text);
                }
                let time_text = render::render(&ctx, &f, &render::items(time)?);
                Some(glue.replace("{1}", &date_text).replace("{0}", &time_text))
            }
            None => {
                let items = self.items.as_ref()?;
                Some(render::render(&ctx, &f, items))
            }
        }
    }

    fn set_pattern(&mut self, pattern: &str) -> bool {
        match render::items(pattern) {
            Some(items) => {
                self.items = Some(items);
                self.pattern = pattern.to_string();
                self.relative = None;
                true
            }
            None => {
                self.items = None;
                self.pattern = String::new();
                self.relative = None;
                true
            }
        }
    }
}

/// The digits of a locale's numbering system.
fn digits_of(loc: &icu::locale::Locale) -> [char; 10] {
    use icu::decimal::DecimalFormatter;
    let prefs: icu::decimal::DecimalFormatterPreferences = loc.into();
    let ascii = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
    let Ok(f) = DecimalFormatter::try_new(prefs, Default::default()) else {
        return ascii;
    };
    let mut out = ascii;
    for (d, slot) in out.iter_mut().enumerate() {
        let text = f.format_to_string(&fixed_decimal::Decimal::from(d as u32));
        match text.chars().next() {
            Some(c) => *slot = c,
            None => return ascii,
        }
    }
    out
}

// ---- object plumbing --------------------------------------------------------------------------

fn with_state<R>(o: &Object, f: impl FnOnce(&mut DateState) -> R) -> Option<R> {
    o.with_payload::<DateState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn state_of<'a>(ctx: &mut Ctx, o: &'a Object) -> Result<&'a Object, Unwind> {
    if o.with_payload::<DateState, _>(|_| ()).is_none() {
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
    if let Some(copy) = src.with_payload::<DateState, _>(|s| DateState {
        requested: s.requested.clone(),
        valid: s.valid.clone(),
        date_type: s.date_type,
        time_type: s.time_type,
        pattern: s.pattern.clone(),
        relative: s.relative.clone(),
        items: s.items.clone(),
        zone: s.zone.clone(),
        calendar: s.calendar,
        calendar_kind: s.calendar_kind,
        lenient: s.lenient,
        names: s.names,
        week: s.week,
        icu_locale: s.icu_locale.clone(),
        digits: s.digits,
        err: IntlError::default(),
    }) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

/// The zone argument of a constructor or `setTimeZone`: a string, a
/// `DateTimeZone`, an `IntlTimeZone`, or `null` for the default.
pub(crate) fn zone_arg(ctx: &mut Ctx, v: Option<&Value>, who: &str) -> Result<ZoneRef, Unwind> {
    let Some(v) = v.filter(|v| !matches!(v.deref().as_ref(), Value::Null)) else {
        return Ok(ZoneRef::from_php(date_bridge::default_zone(ctx)));
    };
    match v.deref().into_owned() {
        Value::Object(o) => {
            if let Some(z) = crate::tz::zone_of(&o) {
                return Ok(z);
            }
            if let Some(z) = date_bridge::zone_of(&o) {
                return Ok(ZoneRef::from_php(z));
            }
            Err(Unwind::type_error(format!("{who}(): Argument #1 ($timezone) must be a valid timezone")))
        }
        other => {
            let name = String::from_utf8_lossy(&other.to_php_bytes()).into_owned();
            match ZoneRef::parse(&name) {
                Some(z) => Ok(z),
                None => Err(Unwind::exception("IntlException", format!("{who}(): No such time zone: \"{name}\""))),
            }
        }
    }
}

fn build(ctx: &mut Ctx, args: &[Value], who: &str) -> Result<Option<DateState>, Unwind> {
    state::reset_global(ctx);
    let requested = locale_arg(ctx, args, 0);
    crate::valid_language(ctx, &requested)?;
    let date_type = args.get(1).map_or(FULL, |v| v.deref().to_int());
    let time_type = args.get(2).map_or(FULL, |v| v.deref().to_int());
    let zone = zone_arg(ctx, args.get(3), who)?;
    // the calendar: a constant, an IntlCalendar, or null
    let mut calendar_kind = GREGORIAN;
    let mut calendar = Calendar::Gregorian;
    match opt_arg(args, 4) {
        None => {}
        Some(v) => match v.deref().into_owned() {
            Value::Object(_) => {}
            other => {
                let n = other.to_int();
                if !matches!(n, TRADITIONAL | GREGORIAN) {
                    return Err(Unwind::exception(
                        "IntlException",
                        format!(
                            "{who}(): Invalid value for calendar type; it must be one of IntlDateFormatter::TRADITIONAL (locale's default calendar) or IntlDateFormatter::GREGORIAN. Alternatively, it can be an IntlCalendar object"
                        ),
                    ));
                }
                calendar_kind = n;
                if n == TRADITIONAL {
                    calendar = Calendar::traditional(&locale::canonical(&requested));
                }
            }
        },
    }
    // php reads an empty pattern as none at all
    let pattern = opt_arg(args, 5).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned()).filter(|p| !p.is_empty());
    match DateState::new(&requested, date_type, time_type, zone, calendar, calendar_kind, pattern.as_deref()) {
        Ok(st) => Ok(Some(st)),
        Err(code) => {
            state::set_global(ctx, who, code, "invalid date format style")?;
            Ok(None)
        }
    }
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    match build(ctx, args, "IntlDateFormatter::__construct")? {
        Some(st) => {
            o.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Null)
        }
        None => Err(Unwind::exception("IntlException", "IntlDateFormatter::__construct(): invalid date format style")),
    }
}

fn create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    match build(ctx, args, "datefmt_create")? {
        Some(st) => {
            let cid = ctx.lookup_class_or_error(b"IntlDateFormatter")?;
            let obj = ctx.instantiate(cid);
            obj.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Object(obj))
        }
        None => Ok(Value::Null),
    }
}

// ---- formatting -------------------------------------------------------------------------------

/// The instant a `format()` argument names.
fn instant_of(ctx: &mut Ctx, v: &Value, st_zone: &ZoneRef) -> Option<(i64, u32)> {
    match v.deref().as_ref() {
        Value::Object(o) => {
            if let Some(instant) = crate::cal::instant_of(o) {
                return Some(instant);
            }
            let (ts, usec, _) = date_bridge::datetime_instant(o);
            Some((ts, usec))
        }
        Value::Array(a) => {
            // a `localtime` array, read in the formatter's zone
            let get = |k: &[u8]| a.get(&rphp_value::ArrayKey::str(k)).map(|v| v.to_int());
            let year = get(b"tm_year")? + 1900;
            let month = get(b"tm_mon").unwrap_or(0) + 1;
            let day = get(b"tm_mday").unwrap_or(1);
            let hour = get(b"tm_hour").unwrap_or(0);
            let min = get(b"tm_min").unwrap_or(0);
            let sec = get(b"tm_sec").unwrap_or(0);
            let days = civil::days_from_civil(year, month.clamp(1, 12) as u32, day.clamp(1, 31) as u32);
            let local = days * 86_400 + hour * 3600 + min * 60 + sec;
            // resolve the wall clock into an instant
            let guess = local - i64::from(st_zone.offset_at(local));
            let offset = st_zone.offset_at(guess);
            Some((local - i64::from(offset), 0))
        }
        Value::Float(f) => {
            let _ = ctx;
            Some((*f as i64, ((f.fract().abs() * 1e6) as u32) % 1_000_000))
        }
        other => Some((other.to_int(), 0)),
    }
}

fn format(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let arg = args.first().cloned().unwrap_or(Value::Null);
    let zone = with_state(o, |st| st.zone.clone()).unwrap_or_else(ZoneRef::unknown);
    let Some((ts, usec)) = instant_of(ctx, &arg, &zone) else {
        object_error(o, &who, U_ILLEGAL_ARGUMENT_ERROR, "Failed to convert IntlCalendar into a date/time value");
        return Ok(Value::Bool(false));
    };
    let out = with_state(o, |st| {
        st.err.reset();
        st.format(ts, usec)
    })
    .flatten();
    match out {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => Ok(Value::string(b"")),
    }
}

// ---- accessors --------------------------------------------------------------------------------

fn get_date_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |st| st.date_type).unwrap_or(0)))
}

fn get_time_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |st| st.time_type).unwrap_or(0)))
}

fn get_calendar(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |st| st.calendar_kind).unwrap_or(GREGORIAN)))
}

fn set_calendar(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let v = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    let requested = with_state(o, |st| st.requested.clone()).unwrap_or_default();
    let (kind, cal) = match v {
        Value::Null => (GREGORIAN, Calendar::Gregorian),
        Value::Object(_) => (GREGORIAN, Calendar::Gregorian),
        other => {
            let n = other.to_int();
            if !matches!(n, TRADITIONAL | GREGORIAN) {
                // php reports the failure, not an error on the object
                let _ = &who;
                return Ok(Value::Bool(false));
            }
            (n, if n == TRADITIONAL { Calendar::traditional(&locale::canonical(&requested)) } else { Calendar::Gregorian })
        }
    };
    with_state(o, |st| {
        st.err.reset();
        st.calendar_kind = kind;
        st.calendar = cal;
    });
    Ok(Value::Bool(true))
}

fn get_timezone_id(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let id = with_state(o, |st| st.zone.id.clone()).unwrap_or_default();
    Ok(Value::string(id.as_bytes()))
}

fn get_timezone(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let zone = with_state(o, |st| st.zone.clone()).unwrap_or_else(ZoneRef::unknown);
    crate::tz::new_object(ctx, zone)
}

fn set_timezone(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let zone = zone_arg(ctx, args.first(), &who)?;
    with_state(o, |st| {
        st.err.reset();
        st.zone = zone;
    });
    Ok(Value::Bool(true))
}

fn get_calendar_object(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let (zone, cal) = with_state(o, |st| (st.zone.clone(), st.calendar)).unwrap_or((ZoneRef::unknown(), Calendar::Gregorian));
    let requested = with_state(o, |st| st.requested.clone()).unwrap_or_default();
    crate::cal::new_object(ctx, zone, cal.name(), &requested)
}

fn get_pattern(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let p = with_state(o, |st| {
        st.err.reset();
        st.pattern.clone()
    })
    .unwrap_or_default();
    Ok(Value::string(p.as_bytes()))
}

fn set_pattern(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let text = crate::text_arg(args, 0);
    let ok = with_state(o, |st| {
        st.err.reset();
        st.set_pattern(&text)
    })
    .unwrap_or(false);
    Ok(Value::Bool(ok))
}

fn get_locale(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let kind = args.first().map_or(0, |v| v.deref().to_int());
    let name = with_state(o, |st| {
        if kind == 1 {
            st.valid.clone()
        } else {
            // ICU's ACTUAL_LOCALE of a date format is the bundle's language
            st.valid.split('_').next().unwrap_or("").to_string()
        }
    })
    .unwrap_or_default();
    Ok(Value::string(name.as_bytes()))
}

fn is_lenient(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Bool(with_state(o, |st| st.lenient).unwrap_or(true)))
}

fn set_lenient(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let on = args.first().map(|v| v.deref().to_bool()).unwrap_or(true);
    with_state(o, |st| {
        st.err.reset();
        st.lenient = on;
    });
    Ok(Value::Null)
}

fn get_error_code(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |st| st.err.code).unwrap_or(0)))
}

fn get_error_message(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::string(with_state(o, |st| st.err.message()).unwrap_or_default().as_bytes()))
}

// ---- parsing ----------------------------------------------------------------------------------

fn parse_impl(ctx: &mut Ctx, o: &Object, args: &mut [Value], pos_index: usize) -> Option<(Fields, i64, usize)> {
    let text = crate::text_arg(args, 0);
    let start = args.get(pos_index).map_or(0, |v| v.deref().to_int()).max(0) as usize;
    let _ = ctx;
    with_state(o, |st| st.parse(&text, start)).flatten()
}

fn parse(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    with_state(o, |st| st.err.reset());
    let end_index = 1;
    match parse_impl(ctx, o, args, end_index) {
        Some((_, ts, end)) => {
            if args.len() > end_index {
                args[end_index] = Value::Int(end as i64);
            }
            Ok(Value::Int(ts))
        }
        None => {
            object_error(o, &who, U_PARSE_ERROR, "Date parsing failed");
            Ok(Value::Bool(false))
        }
    }
}

fn localtime(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    with_state(o, |st| st.err.reset());
    let end_index = 1;
    match parse_impl(ctx, o, args, end_index) {
        Some((f, _, end)) => {
            if args.len() > end_index {
                args[end_index] = Value::Int(end as i64);
            }
            let mut a = Array::new();
            a.set(rphp_value::ArrayKey::str(b"tm_sec"), Value::Int(i64::from(f.second)));
            a.set(rphp_value::ArrayKey::str(b"tm_min"), Value::Int(i64::from(f.minute)));
            a.set(rphp_value::ArrayKey::str(b"tm_hour"), Value::Int(i64::from(f.hour)));
            a.set(rphp_value::ArrayKey::str(b"tm_year"), Value::Int(f.ext_year - 1900));
            a.set(rphp_value::ArrayKey::str(b"tm_mday"), Value::Int(i64::from(f.day)));
            a.set(rphp_value::ArrayKey::str(b"tm_wday"), Value::Int(i64::from(f.weekday)));
            a.set(rphp_value::ArrayKey::str(b"tm_yday"), Value::Int(i64::from(f.day_of_year) - 1));
            a.set(rphp_value::ArrayKey::str(b"tm_isdst"), Value::Int(0));
            Ok(Value::Array(a))
        }
        None => {
            object_error(o, &who, U_PARSE_ERROR, "Date parsing failed");
            Ok(Value::Bool(false))
        }
    }
}

// ---- formatObject -----------------------------------------------------------------------------

fn format_object(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = ctx.active_function_name();
    state::reset_global(ctx);
    let Some(Value::Object(obj)) = args.first().map(|v| v.deref().into_owned()) else {
        return Err(Unwind::type_error(format!("{who}(): Argument #1 ($datetime) must be of type object")));
    };
    let locale = locale_arg(ctx, args, 2);
    // the format: a style, a pair of styles, or a pattern
    let (date_type, time_type, pattern) = match opt_arg(args, 1).map(|v| v.deref().into_owned()) {
        None => (MEDIUM, MEDIUM, None),
        Some(Value::Array(a)) => {
            let mut it = a.values();
            let d = it.next().map_or(MEDIUM, |v| v.to_int());
            let t = it.next().map_or(MEDIUM, |v| v.to_int());
            if a.len() != 2 {
                state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "bad format array")?;
                return Ok(Value::Bool(false));
            }
            (d, t, None)
        }
        Some(Value::Str(s)) => {
            let text = String::from_utf8_lossy(s.as_bytes()).into_owned();
            if text.is_empty() {
                state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "the format is empty")?;
                return Ok(Value::Bool(false));
            }
            (NONE, NONE, Some(text))
        }
        Some(other) => {
            let n = other.to_int();
            if style_index(n).is_none() {
                state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "invalid date format style")?;
                return Ok(Value::Bool(false));
            }
            (n, n, None)
        }
    };
    // the object's own zone
    let zone = match crate::cal::zone_of(&obj) {
        Some(z) => z,
        None => {
            let (_, _, z) = date_bridge::datetime_instant(&obj);
            ZoneRef::from_php(z)
        }
    };
    let Some((ts, usec)) = instant_of(ctx, &Value::Object(obj), &zone) else {
        return Ok(Value::Bool(false));
    };
    match DateState::new(&locale, date_type, time_type, zone, Calendar::Gregorian, GREGORIAN, pattern.as_deref()) {
        Ok(st) => Ok(Value::string(st.format(ts, usec).unwrap_or_default().as_bytes())),
        Err(code) => {
            state::set_global(ctx, &who, code, "invalid date format style")?;
            Ok(Value::Bool(false))
        }
    }
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("create", create),
    ("format", format),
    ("formatObject", format_object),
    ("parse", parse),
    ("localtime", localtime),
    ("getDateType", get_date_type),
    ("getTimeType", get_time_type),
    ("getCalendar", get_calendar),
    ("setCalendar", set_calendar),
    ("getTimeZoneId", get_timezone_id),
    ("getCalendarObject", get_calendar_object),
    ("getTimeZone", get_timezone),
    ("setTimeZone", set_timezone),
    ("setPattern", set_pattern),
    ("getPattern", get_pattern),
    ("getLocale", get_locale),
    ("isLenient", is_lenient),
    ("setLenient", set_lenient),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
];

macro_rules! as_function {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            let obj = match args.first().map(|v| v.deref().into_owned()) {
                Some(Value::Object(o)) => o,
                _ => return Err(Unwind::type_error("Argument #1 ($formatter) must be of type IntlDateFormatter")),
            };
            let rest = &mut args[1..];
            $method(ctx, Some(&obj), rest)
        }
    };
}

as_function!(f_format, format);
as_function!(f_parse, parse);
as_function!(f_localtime, localtime);
as_function!(f_get_date_type, get_date_type);
as_function!(f_get_time_type, get_time_type);
as_function!(f_get_calendar, get_calendar);
as_function!(f_set_calendar, set_calendar);
as_function!(f_get_timezone_id, get_timezone_id);
as_function!(f_get_calendar_object, get_calendar_object);
as_function!(f_get_timezone, get_timezone);
as_function!(f_set_timezone, set_timezone);
as_function!(f_set_pattern, set_pattern);
as_function!(f_get_pattern, get_pattern);
as_function!(f_get_locale, get_locale);
as_function!(f_is_lenient, is_lenient);
as_function!(f_set_lenient, set_lenient);
as_function!(f_get_error_code, get_error_code);
as_function!(f_get_error_message, get_error_message);

fn f_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, None, args)
}

fn f_format_object(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    format_object(ctx, None, args)
}

static FUNCTIONS: &[FnImpl] = &[
    ("datefmt_create", f_create),
    ("datefmt_format", f_format),
    ("datefmt_format_object", f_format_object),
    ("datefmt_parse", f_parse),
    ("datefmt_localtime", f_localtime),
    ("datefmt_get_datetype", f_get_date_type),
    ("datefmt_get_timetype", f_get_time_type),
    ("datefmt_get_calendar", f_get_calendar),
    ("datefmt_set_calendar", f_set_calendar),
    ("datefmt_get_timezone_id", f_get_timezone_id),
    ("datefmt_get_calendar_object", f_get_calendar_object),
    ("datefmt_get_timezone", f_get_timezone),
    ("datefmt_set_timezone", f_set_timezone),
    ("datefmt_set_pattern", f_set_pattern),
    ("datefmt_get_pattern", f_get_pattern),
    ("datefmt_get_locale", f_get_locale),
    ("datefmt_is_lenient", f_is_lenient),
    ("datefmt_set_lenient", f_set_lenient),
    ("datefmt_get_error_code", f_get_error_code),
    ("datefmt_get_error_message", f_get_error_message),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLDATEFORMATTER, METHODS, |b| b.payload_clone(payload_clone));
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
    let _ = U_UNSUPPORTED_ERROR;
}

// ---- the subformats of a MessageFormat --------------------------------------------------------

/// A date subformat of a `MessageFormat` (`{d, date, …}`, `{d, time,
/// …}`): `DateFormat::createDateInstance` / `createTimeInstance` in the
/// locale's own calendar, or a pattern over it.
pub(crate) struct MsgDate(DateState);

impl MsgDate {
    /// `date_style` / `time_style` are the `IntlDateFormatter` style
    /// constants (`NONE` for the half left out).
    pub(crate) fn new(locale: &str, date_style: i64, time_style: i64, pattern: Option<&str>, zone: ZoneRef) -> Result<MsgDate, i64> {
        let calendar = Calendar::traditional(&locale::canonical(locale));
        DateState::new(locale, date_style, time_style, zone, calendar, TRADITIONAL, pattern).map(MsgDate)
    }

    /// Format an instant in milliseconds; `None` when the pattern has no
    /// valid form.
    pub(crate) fn format_millis(&self, ms: f64) -> Option<String> {
        if !ms.is_finite() {
            return None;
        }
        let ms = ms.floor() as i64;
        self.0.format(ms.div_euclid(1000), (ms.rem_euclid(1000) * 1000) as u32)
    }

    /// Parse at character `start`: the instant in seconds and the
    /// character index past the text.
    pub(crate) fn parse_at(&self, text: &str, start: usize) -> Option<(i64, usize)> {
        self.0.parse(text, start).map(|(_, ts, end)| (ts, end))
    }
}
