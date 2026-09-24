//! php-src `ext/date`'s procedural surface: `date`, `mktime`, `strtotime`,
//! `getdate`, `date_parse` and the timezone listings.
//!
//! **Where the default timezone lives.** php keeps it apart from the ini
//! table: `date_default_timezone_set()` does *not* change
//! `ini_get("date.timezone")`, and `date_default_timezone_get()` falls back to
//! the ini entry only while nothing has been set. That slot is a thread-local
//! here, the way `random.rs` keeps its engine state, until the interpreter
//! grows a per-extension request slot.
//!
//! **Everything is civil arithmetic.** A timestamp is broken down in a
//! timezone, the format characters read those fields, and the way back is a
//! single resolution step. That is why `mktime()` takes the *local* wall
//! clock and why `strtotime("+24 hours")` across a spring-forward is 23 real
//! hours.
//!
//! **The timezone tables are php's own.** `timezone_identifiers_list()` and
//! `timezone_abbreviations_list()` answer from the snapshot php ships (see
//! `tz.rs`), not from the host's `/usr/share/zoneinfo`, which carries names
//! php does not know and orders them differently.

use std::cell::RefCell;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use super::civil;
use super::format::{self, Rendered, DAY_NAMES, MONTH_NAMES};
use super::parse;
use super::tz::{self, Tz};

/// This extension's registry contribution (see `lib.rs`). The clock
/// functions (`time`, `microtime`, `hrtime`, `sleep`, …) belong to this
/// extension in php-src but live in `uniqid.rs` here, and the `DateTime`
/// family arrives with the class wave.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("date", 1, Some(2), date),
    nf!("gmdate", 1, Some(2), gmdate),
    nf!("idate", 1, Some(2), idate),
    nf!("mktime", 1, Some(6), mktime),
    nf!("gmmktime", 1, Some(6), gmmktime),
    nf!("checkdate", 3, Some(3), checkdate),
    nf!("strtotime", 1, Some(2), strtotime),
    nf!("date_default_timezone_get", 0, Some(0), date_default_timezone_get),
    nf!("date_default_timezone_set", 1, Some(1), date_default_timezone_set),
    nf!("getdate", 0, Some(1), getdate),
    nf!("localtime", 0, Some(2), localtime),
    nf!("date_parse", 1, Some(1), date_parse),
    nf!("date_parse_from_format", 2, Some(2), date_parse_from_format),
    nf!("date_sun_info", 3, Some(3), super::sun::date_sun_info),
    nf!("date_sunrise", 1, Some(6), super::sun::date_sunrise),
    nf!("date_sunset", 1, Some(6), super::sun::date_sunset),
    nf!("timezone_version_get", 0, Some(0), timezone_version_get),
    nf!("timezone_identifiers_list", 0, Some(2), timezone_identifiers_list),
    nf!("timezone_abbreviations_list", 0, Some(0), timezone_abbreviations_list),
    nf!("strftime", 1, Some(2), strftime),
    nf!("gmstrftime", 1, Some(2), gmstrftime),
];

/// The `DATE_*` format constants and the `SUNFUNCS_RET_*` return modes.
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, value) in [
        ("DATE_ATOM", r"Y-m-d\TH:i:sP"),
        ("DATE_COOKIE", "l, d-M-Y H:i:s T"),
        ("DATE_ISO8601", r"Y-m-d\TH:i:sO"),
        ("DATE_ISO8601_EXPANDED", r"X-m-d\TH:i:sP"),
        ("DATE_RFC822", "D, d M y H:i:s O"),
        ("DATE_RFC850", "l, d-M-y H:i:s T"),
        ("DATE_RFC1036", "D, d M y H:i:s O"),
        ("DATE_RFC1123", "D, d M Y H:i:s O"),
        ("DATE_RFC7231", r"D, d M Y H:i:s \G\M\T"),
        ("DATE_RFC2822", "D, d M Y H:i:s O"),
        ("DATE_RFC3339", r"Y-m-d\TH:i:sP"),
        ("DATE_RFC3339_EXTENDED", r"Y-m-d\TH:i:s.vP"),
        ("DATE_RSS", "D, d M Y H:i:s O"),
        ("DATE_W3C", r"Y-m-d\TH:i:sP"),
    ] {
        r.constant(name, Value::string(value.as_bytes()));
    }
    for (name, v) in [("SUNFUNCS_RET_TIMESTAMP", 0), ("SUNFUNCS_RET_STRING", 1), ("SUNFUNCS_RET_DOUBLE", 2)] {
        r.deprecated_constant(
            name,
            Value::Int(v),
            " since 8.4, as date_sunrise() and date_sunset() were deprecated in 8.1",
        );
    }
}

// ---- the default timezone ------------------------------------------------------

thread_local! {
    /// What `date_default_timezone_set()` last chose, spelled as the caller
    /// wrote it: php echoes `europe/berlin` back verbatim.
    static DEFAULT_TZ: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// php's `RSHUTDOWN`: `date_default_timezone_set()` lasts one request.
pub(crate) fn request_shutdown() {
    DEFAULT_TZ.with(|t| *t.borrow_mut() = None);
}

/// The name `date_default_timezone_get()` answers with.
fn default_tz_name(ctx: &Ctx) -> String {
    if let Some(name) = DEFAULT_TZ.with(|t| t.borrow().clone()) {
        return name;
    }
    match ctx.ini_get("date.timezone") {
        Some(v) if !v.is_empty() && tz::is_known_id(v) => v.to_string(),
        _ => "UTC".to_string(),
    }
}

/// The timezone every function without an explicit one works in.
pub(crate) fn default_tz(ctx: &Ctx) -> Tz {
    Tz::Id(default_tz_name(ctx))
}

// ---- argument coercion ----------------------------------------------------------

/// php's weak `string` parameter coercion, with php's diagnostics.
fn str_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<Vec<u8>, Unwind> {
    match &*v.deref() {
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{who}(): Passing null to parameter #{pos} (${name}) of type string is deprecated"
            ))?;
            Ok(Vec::new())
        }
        Value::Array(_) => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type string, array given"
        ))),
        other @ (Value::Object(_) | Value::Closure(_)) => {
            let owned = other.clone();
            match ctx.to_string(&owned) {
                Ok(s) => Ok(s.as_bytes().to_vec()),
                Err(_) => Err(Unwind::type_error(format!(
                    "{who}(): Argument #{pos} (${name}) must be of type string, {} given",
                    rphp_runtime::value_name(&owned)
                ))),
            }
        }
        other => Ok(other.to_php_bytes()),
    }
}

/// php's weak `int` parameter coercion: a numeric string or a whole float
/// converts, a fractional float is deprecated and truncated, `null` is the
/// 8.1 null-to-non-nullable deprecation.
fn int_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<i64, Unwind> {
    match &*v.deref() {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() {
                ctx.deprecated(&format!(
                    "Implicit conversion from float {} to int loses precision",
                    Value::Float(*f).to_php_string()
                ))?;
            }
            Ok(*f as i64)
        }
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{who}(): Passing null to parameter #{pos} (${name}) of type int is deprecated"
            ))?;
            Ok(0)
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(s.to_int()),
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type int, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// A `?int` parameter: `null` is a value in its own right, not a deprecation.
fn opt_int_arg(
    ctx: &mut Ctx,
    who: &str,
    pos: u32,
    name: &str,
    v: Option<&Value>,
) -> Result<Option<i64>, Unwind> {
    let Some(v) = v else { return Ok(None) };
    match &*v.deref() {
        Value::Null | Value::Uninit => Ok(None),
        Value::Int(i) => Ok(Some(*i)),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        Value::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() {
                ctx.deprecated(&format!(
                    "Implicit conversion from float {} to int loses precision",
                    Value::Float(*f).to_php_string()
                ))?;
            }
            Ok(Some(*f as i64))
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(Some(s.to_int())),
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type ?int, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// The wall clock `now` in `zone`, which every `?int $timestamp = null`
/// parameter defaults to.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- date / gmdate / idate --------------------------------------------------------

/// `date(string $format, ?int $timestamp = null): string`
pub(crate) fn date(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let fmt = str_arg(ctx, "date", 1, "format", &args[0])?;
    let ts = opt_int_arg(ctx, "date", 2, "timestamp", args.get(1))?.unwrap_or_else(now_seconds);
    let zone = default_tz(ctx);
    let r = format::render(&zone, ts, 0);
    Ok(Value::string(&format::format(&r, &fmt)))
}

/// The zone the `gm*` functions work in: UTC by name — `date("e")` answers
/// `UTC` — but `GMT` by abbreviation, which is what `date("T")` answers.
fn gm_render(ts: i64) -> Rendered {
    let mut r = format::render(&Tz::utc(), ts, 0);
    r.abbr = "GMT".to_string();
    r
}

/// `gmdate(string $format, ?int $timestamp = null): string` — the same in UTC.
pub(crate) fn gmdate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let fmt = str_arg(ctx, "gmdate", 1, "format", &args[0])?;
    let ts = opt_int_arg(ctx, "gmdate", 2, "timestamp", args.get(1))?.unwrap_or_else(now_seconds);
    Ok(Value::string(&format::format(&gm_render(ts), &fmt)))
}

/// `idate(string $format, ?int $timestamp = null): int|false` — one format
/// character, read back as an integer. Characters whose output is not a
/// number (`D`, `l`, `T`, `c`, …) are rejected, and so is a format that is
/// not exactly one character long.
pub(crate) fn idate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let fmt = str_arg(ctx, "idate", 1, "format", &args[0])?;
    let ts = opt_int_arg(ctx, "idate", 2, "timestamp", args.get(1))?.unwrap_or_else(now_seconds);
    if fmt.len() != 1 {
        ctx.warn("idate(): idate format is one char")?;
        return Ok(Value::Bool(false));
    }
    if !b"BdgGhHiIjLmnNostUwWyYzZ".contains(&fmt[0]) {
        ctx.warn("idate(): Unrecognized date format token")?;
        return Ok(Value::Bool(false));
    }
    let zone = default_tz(ctx);
    let r = format::render(&zone, ts, 0);
    let text = format::format(&r, &fmt);
    // Every accepted character renders as digits with an optional sign, so
    // reading it back is exact.
    let s = String::from_utf8_lossy(&text);
    Ok(Value::Int(s.parse::<i64>().unwrap_or(0)))
}

// ---- mktime / gmmktime / checkdate -------------------------------------------------

/// The shared body of `mktime` and `gmmktime`: the arguments that are missing
/// come from `now` *in the target zone*, and the fields are normalized the
/// way php normalizes them, so `mktime(0, 0, 0, 13, 1, 2000)` is January 2001.
fn mktime_in(ctx: &mut Ctx, who: &str, zone: &Tz, args: &mut [Value]) -> NativeResult {
    let now_ts = now_seconds();
    let now = format::render(zone, now_ts, 0);
    let names = ["hour", "minute", "second", "month", "day", "year"];
    let mut parts: [i64; 6] = [
        now.hour as i64,
        now.minute as i64,
        now.second as i64,
        now.month as i64,
        now.day as i64,
        now.year,
    ];
    for (i, name) in names.iter().enumerate() {
        let Some(v) = args.get(i) else { break };
        if i == 0 {
            parts[0] = int_arg(ctx, who, 1, name, v)?;
        } else if let Some(n) = opt_int_arg(ctx, who, i as u32 + 1, name, Some(v))? {
            parts[i] = n;
        }
    }
    // php's two-digit year window: 0–69 is 2000–2069 and 70–100 is
    // 1970–2000, so `mktime(0, 0, 0, 1, 1, 100)` is the year 2000.
    let year = match parts[5] {
        y @ 0..=69 => y + 2000,
        y @ 70..=100 => y + 1900,
        y => y,
    };
    let mut c = parse::Civil {
        y: year,
        mo: parts[3],
        d: parts[4],
        h: parts[0],
        mi: parts[1],
        s: parts[2],
        us: 0,
    };
    parse::normalize(&mut c);
    // php seeds the fields from *now*, and `now` carries the offset in force
    // at this moment. That offset is the tie-break across a fall-back
    // overlap, which is why `mktime()` and `strtotime()` can disagree about
    // an ambiguous wall clock: in `Europe/Berlin` during summer,
    // `mktime(2, 30, 0, 10, 31, 2021)` is CEST where
    // `strtotime("2021-10-31 02:30")` is CET.
    let prefer = zone.offset_at(now_ts);
    Ok(Value::Int(zone.resolve(parse::sse_of(&c), Some(prefer))))
}

/// `mktime(int $hour, ?int $minute = null, ?int $second = null, ?int $month = null, ?int $day = null, ?int $year = null): int|false`
pub(crate) fn mktime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let zone = default_tz(ctx);
    mktime_in(ctx, "mktime", &zone, args)
}

/// `gmmktime(...)` — the same reading in UTC.
pub(crate) fn gmmktime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    mktime_in(ctx, "gmmktime", &Tz::utc(), args)
}

/// `checkdate(int $month, int $day, int $year): bool` — php accepts years
/// 1 to 32767 only, and no month or day outside its calendar range.
pub(crate) fn checkdate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let month = int_arg(ctx, "checkdate", 1, "month", &args[0])?;
    let day = int_arg(ctx, "checkdate", 2, "day", &args[1])?;
    let year = int_arg(ctx, "checkdate", 3, "year", &args[2])?;
    let ok = (1..=32767).contains(&year)
        && (1..=12).contains(&month)
        && day >= 1
        && day <= civil::days_in_month(year, month as u32) as i64;
    Ok(Value::Bool(ok))
}

// ---- strtotime -------------------------------------------------------------------

/// `strtotime(string $datetime, ?int $baseTimestamp = null): int|false` —
/// `false` for any input the scanner reported an error for, which is why
/// `strtotime(" ")` is the base timestamp but `strtotime("")` is `false`.
pub(crate) fn strtotime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let text = str_arg(ctx, "strtotime", 1, "datetime", &args[0])?;
    let base = opt_int_arg(ctx, "strtotime", 2, "baseTimestamp", args.get(1))?
        .unwrap_or_else(now_seconds);
    let zone = default_tz(ctx);
    match parse::strtotime(&text, base, &zone) {
        Some(ts) => Ok(Value::Int(ts)),
        None => Ok(Value::Bool(false)),
    }
}

// ---- the default timezone ---------------------------------------------------------

/// `date_default_timezone_get(): string`
pub(crate) fn date_default_timezone_get(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(default_tz_name(ctx).as_bytes()))
}

/// `date_default_timezone_set(string $timezoneId): bool` — identifiers only.
/// An offset is not one: `date_default_timezone_set("+05:00")` is a notice
/// and `false`. The spelling is kept as written, so a later
/// `date_default_timezone_get()` answers `europe/berlin` verbatim.
pub(crate) fn date_default_timezone_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = str_arg(ctx, "date_default_timezone_set", 1, "timezoneId", &args[0])?;
    let text = String::from_utf8_lossy(&name).into_owned();
    if !tz::is_known_id(&text) {
        ctx.notice(&format!(
            "date_default_timezone_set(): Timezone ID '{text}' is invalid"
        ))?;
        return Ok(Value::Bool(false));
    }
    DEFAULT_TZ.with(|t| *t.borrow_mut() = Some(text));
    Ok(Value::Bool(true))
}

// ---- getdate / localtime -----------------------------------------------------------

/// `getdate(?int $timestamp = null): array`
pub(crate) fn getdate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ts = opt_int_arg(ctx, "getdate", 1, "timestamp", args.first())?.unwrap_or_else(now_seconds);
    let zone = default_tz(ctx);
    let r = format::render(&zone, ts, 0);
    let wday = civil::weekday(civil::days_from_civil(r.year, r.month as i64, r.day as i64));
    let mut a = Array::new();
    a.set(ArrayKey::str(b"seconds"), Value::Int(r.second as i64));
    a.set(ArrayKey::str(b"minutes"), Value::Int(r.minute as i64));
    a.set(ArrayKey::str(b"hours"), Value::Int(r.hour as i64));
    a.set(ArrayKey::str(b"mday"), Value::Int(r.day as i64));
    a.set(ArrayKey::str(b"wday"), Value::Int(wday as i64));
    a.set(ArrayKey::str(b"mon"), Value::Int(r.month as i64));
    a.set(ArrayKey::str(b"year"), Value::Int(r.year));
    a.set(
        ArrayKey::str(b"yday"),
        Value::Int(civil::day_of_year(r.year, r.month, r.day) as i64),
    );
    a.set(
        ArrayKey::str(b"weekday"),
        Value::string(DAY_NAMES[wday as usize].as_bytes()),
    );
    a.set(
        ArrayKey::str(b"month"),
        Value::string(MONTH_NAMES[(r.month - 1) as usize].as_bytes()),
    );
    a.set(ArrayKey::Int(0), Value::Int(ts));
    Ok(Value::Array(a))
}

/// `localtime(?int $timestamp = null, bool $associative = false): array` —
/// C's `struct tm`: the month is zero-based and the year counts from 1900.
pub(crate) fn localtime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ts =
        opt_int_arg(ctx, "localtime", 1, "timestamp", args.first())?.unwrap_or_else(now_seconds);
    let assoc = args.get(1).is_some_and(Value::to_bool);
    let zone = default_tz(ctx);
    let r = format::render(&zone, ts, 0);
    let wday = civil::weekday(civil::days_from_civil(r.year, r.month as i64, r.day as i64));
    let fields: [(&[u8], i64); 9] = [
        (b"tm_sec", r.second as i64),
        (b"tm_min", r.minute as i64),
        (b"tm_hour", r.hour as i64),
        (b"tm_mday", r.day as i64),
        (b"tm_mon", r.month as i64 - 1),
        (b"tm_year", r.year - 1900),
        (b"tm_wday", wday as i64),
        (b"tm_yday", civil::day_of_year(r.year, r.month, r.day) as i64),
        (b"tm_isdst", i64::from(r.dst)),
    ];
    let mut a = Array::new();
    for (name, value) in fields {
        if assoc {
            a.set(ArrayKey::str(name), Value::Int(value));
        } else {
            a.push(Value::Int(value));
        }
    }
    Ok(Value::Array(a))
}

// ---- date_parse ---------------------------------------------------------------------

/// A field that was never pinned down reports as `false`, not `null`.
fn opt_field(v: Option<i64>) -> Value {
    match v {
        Some(n) => Value::Int(n),
        None => Value::Bool(false),
    }
}

/// The `warnings` / `errors` sub-arrays: keyed by the byte offset the
/// message was raised at, so two messages at the same offset collapse into
/// one entry while the count still shows both.
fn diagnostics<S: AsRef<str>>(items: &[(usize, S)]) -> Value {
    let mut a = Array::new();
    for (pos, text) in items {
        a.set(ArrayKey::Int(*pos as i64), Value::string(text.as_ref().as_bytes()));
    }
    Value::Array(a)
}

/// The `relative` sub-array php reports for a parse that carried relative
/// amounts.
fn relative_array(p: &parse::Parsed) -> Value {
    let rel = &p.rel;
    let mut a = Array::new();
    a.set(ArrayKey::str(b"year"), Value::Int(rel.y));
    a.set(ArrayKey::str(b"month"), Value::Int(rel.m));
    // php files the ordinal of `<n> <weekday> of` as plain days, keeping the
    // month walk itself out of the reported structure.
    let extra_days = match rel.nth_weekday {
        Some((n, _)) => (if n > 0 { n - 1 } else { n }) * 7,
        None => 0,
    };
    a.set(ArrayKey::str(b"day"), Value::Int(rel.d + extra_days));
    a.set(ArrayKey::str(b"hour"), Value::Int(rel.h));
    a.set(ArrayKey::str(b"minute"), Value::Int(rel.i));
    a.set(ArrayKey::str(b"second"), Value::Int(rel.s));
    let weekday = rel.weekday.or(rel.nth_weekday.map(|(_, w)| w));
    if let Some(w) = weekday {
        a.set(ArrayKey::str(b"weekday"), Value::Int(w));
    }
    if let Some(n) = rel.special_weekday {
        a.set(ArrayKey::str(b"weekdays"), Value::Int(n));
    }
    match rel.first_last_day_of {
        1 => a.set(ArrayKey::str(b"first_day_of_month"), Value::Bool(true)),
        2 => a.set(ArrayKey::str(b"last_day_of_month"), Value::Bool(true)),
        _ => {}
    }
    Value::Array(a)
}

/// The array shape `date_parse()` and `date_parse_from_format()` share.
fn parse_result(p: &parse::Parsed) -> Value {
    parse_result_with(p, &p.warnings, &p.errors)
}

/// [`parse_result`] with the diagnostics given apart from the fields
/// (`date_parse_from_format()`'s scanner owns its message strings).
fn parse_result_with<S: AsRef<str>>(p: &parse::Parsed, warnings: &[(usize, S)], errors: &[(usize, S)]) -> Value {
    let mut a = Array::new();
    a.set(ArrayKey::str(b"year"), opt_field(p.y));
    a.set(ArrayKey::str(b"month"), opt_field(p.m));
    a.set(ArrayKey::str(b"day"), opt_field(p.d));
    a.set(ArrayKey::str(b"hour"), opt_field(p.h));
    a.set(ArrayKey::str(b"minute"), opt_field(p.i));
    a.set(ArrayKey::str(b"second"), opt_field(p.s));
    a.set(
        ArrayKey::str(b"fraction"),
        match p.us {
            Some(us) => Value::Float(us as f64 / 1_000_000.0),
            None => Value::Bool(false),
        },
    );
    a.set(
        ArrayKey::str(b"warning_count"),
        Value::Int(warnings.len() as i64),
    );
    a.set(ArrayKey::str(b"warnings"), diagnostics(warnings));
    a.set(
        ArrayKey::str(b"error_count"),
        Value::Int(errors.len() as i64),
    );
    a.set(ArrayKey::str(b"errors"), diagnostics(errors));
    a.set(ArrayKey::str(b"is_localtime"), Value::Bool(p.have_zone));
    if p.have_zone {
        // A name the scanner could not resolve leaves the type at 0 with no
        // zone keys at all, which is how php reports it.
        let Some(zone) = p.zone.clone() else {
            a.set(ArrayKey::str(b"zone_type"), Value::Int(0));
            if p.has_relative() {
                a.set(ArrayKey::str(b"relative"), relative_array(p));
            }
            return Value::Array(a);
        };
        a.set(ArrayKey::str(b"zone_type"), Value::Int(zone.type_id()));
        match &zone {
            Tz::Offset(secs) => {
                a.set(ArrayKey::str(b"zone"), Value::Int(*secs as i64));
                a.set(ArrayKey::str(b"is_dst"), Value::Bool(false));
            }
            Tz::Abbr { name, offset, dst } => {
                // timelib keeps an abbreviation's DST hour apart from its
                // `zone` (`CEST` is 3600 with `is_dst`).
                let base = *offset as i64 - if *dst { 3600 } else { 0 };
                a.set(ArrayKey::str(b"zone"), Value::Int(base));
                a.set(ArrayKey::str(b"is_dst"), Value::Bool(*dst));
                a.set(ArrayKey::str(b"tz_abbr"), Value::string(name.as_bytes()));
            }
            Tz::Id(id) => {
                // php reports the abbreviation too for the one identifier its
                // parser reaches through the abbreviation table, `UTC`.
                if id == "UTC" {
                    a.set(ArrayKey::str(b"tz_abbr"), Value::string(b"UTC"));
                }
                a.set(ArrayKey::str(b"tz_id"), Value::string(id.as_bytes()));
            }
        }
    }
    if p.has_relative() {
        a.set(ArrayKey::str(b"relative"), relative_array(p));
    }
    Value::Array(a)
}

/// `date_parse(string $datetime): array` — the scanner's result without
/// resolving it against a base time.
pub(crate) fn date_parse(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let text = str_arg(ctx, "date_parse", 1, "datetime", &args[0])?;
    let mut p = parse::parse(&text);
    // php checks the calendar once a month was named, filling the missing
    // parts with its "unset" sentinel — which is why a bare `january` is
    // reported as an invalid date but a bare `1999` is not.
    if let Some(m) = p.m {
        const UNSET: i64 = -99_999;
        let y = p.y.unwrap_or(UNSET);
        let d = p.d.unwrap_or(UNSET);
        if !(1..=12).contains(&m) || d < 1 || d > civil::days_in_month(y, m as u32) as i64 {
            p.warnings.push((text.len() + 1, "The parsed date was invalid"));
        }
    }
    // The clock is checked the same way: php's scanner accepts hour 24 and
    // second 60 and warns afterwards rather than refusing them.
    if p.have_time != 0
        && (p.h.unwrap_or(0) > 23 || p.i.unwrap_or(0) > 59 || p.s.unwrap_or(0) > 59)
    {
        p.warnings.push((text.len() + 1, "The parsed time was invalid"));
    }
    Ok(parse_result(&p))
}

/// `date_parse_from_format(string $format, string $datetime): array` —
/// `DateTime::createFromFormat()`'s scanner, reported without resolving:
/// what the format never named stays `false`.
pub(crate) fn date_parse_from_format(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let fmt = str_arg(ctx, "date_parse_from_format", 1, "format", &args[0])?;
    let text = str_arg(ctx, "date_parse_from_format", 2, "datetime", &args[1])?;
    let scan = super::classes::fromformat::scan(&fmt, &text);
    let mut p = scan.parsed;
    if !scan.us_given {
        p.us = None;
    }
    // `U` is written as epoch fields plus a relative second count; php
    // reports the broken-down UTC time it names.
    if scan.from_unix {
        let ts = p.rel.s;
        p.rel.s = 0;
        p.have_relative = false;
        let (y, m, d) = civil::civil_from_days(ts.div_euclid(86_400));
        let rem = ts.rem_euclid(86_400);
        p.y = Some(y);
        p.m = Some(m as i64);
        p.d = Some(d as i64);
        p.h = Some(rem / 3600);
        p.i = Some(rem / 60 % 60);
        p.s = Some(rem % 60);
    }
    Ok(parse_result_with(&p, &scan.warnings, &scan.errors))
}

/// `timezone_version_get(): string` — the version of the timezone data in
/// use, in php's `YYYY.N` spelling (`2026c` is `2026.3`). rphp reads the
/// system database (what `jiff` loads); with no version file php's own
/// answer for system tzdata, `0.system`, is the honest one.
pub(crate) fn timezone_version_get(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(tzdb_version().as_bytes()))
}

fn tzdb_version() -> String {
    let dir = std::env::var("TZDIR").unwrap_or_else(|_| "/usr/share/zoneinfo".to_string());
    let dir = std::path::Path::new(&dir);
    // macOS ships `+VERSION`; the tzdata packages keep the release in the
    // first line of `tzdata.zi` (`# version 2026c`).
    let raw = std::fs::read_to_string(dir.join("+VERSION"))
        .ok()
        .or_else(|| {
            let zi = std::fs::read_to_string(dir.join("tzdata.zi")).ok()?;
            zi.lines().next()?.strip_prefix("# version ").map(str::to_string)
        })
        .unwrap_or_default();
    let raw = raw.trim();
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    match (raw.get(..digits), raw.as_bytes().get(digits)) {
        (Some(year), Some(&letter)) if digits == 4 && letter.is_ascii_lowercase() && raw.len() == 5 => {
            format!("{year}.{}", letter - b'a' + 1)
        }
        _ => "0.system".to_string(),
    }
}

// ---- timezone listings ----------------------------------------------------------------

/// `DateTimeZone::PER_COUNTRY`.
const GROUP_PER_COUNTRY: i64 = 4096;
/// `DateTimeZone::ALL`.
const GROUP_ALL: i64 = 2047;
/// `DateTimeZone::ALL_WITH_BC`. The backward-compatibility names appear only
/// for exactly this value: `ALL | 2048` is not enough, and `2048` on its own
/// lists nothing.
const GROUP_ALL_WITH_BC: i64 = 4095;

/// The bit `DateTimeZone` gives the group an identifier's prefix puts it in.
fn group_of(id: &str) -> i64 {
    match id.split('/').next().unwrap_or("") {
        "Africa" => 1,
        "America" => 2,
        "Antarctica" => 4,
        "Arctic" => 8,
        "Asia" => 16,
        "Atlantic" => 32,
        "Australia" => 64,
        "Europe" => 128,
        "Indian" => 256,
        "Pacific" => 512,
        "UTC" => 1024,
        _ => 0,
    }
}

/// `timezone_identifiers_list(int $timezoneGroup = DateTimeZone::ALL, ?string $countryCode = null): array`
pub(crate) fn timezone_identifiers_list(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let group = match args.first() {
        Some(v) => int_arg(ctx, "timezone_identifiers_list", 1, "timezoneGroup", v)?,
        None => GROUP_ALL,
    };
    let country = match args.get(1) {
        Some(v) if !matches!(&*v.deref(), Value::Null | Value::Uninit) => Some(str_arg(
            ctx,
            "timezone_identifiers_list",
            2,
            "countryCode",
            v,
        )?),
        _ => None,
    };
    let per_country = group & GROUP_PER_COUNTRY != 0;
    if per_country && country.as_ref().is_none_or(|c| c.len() != 2) {
        return Err(Unwind::value_error(
            "timezone_identifiers_list(): Argument #2 ($countryCode) must be a two-letter ISO 3166-1 compatible country code when argument #1 ($timezoneGroup) is DateTimeZone::PER_COUNTRY",
        ));
    }
    let with_bc = group == GROUP_ALL_WITH_BC;
    // php matches the country code exactly: `de` lists nothing.
    let wanted_country = country.map(|c| String::from_utf8_lossy(&c).into_owned());
    let mut a = Array::new();
    for (id, cc, bc) in tz::IDENTIFIERS {
        if *bc && !with_bc {
            continue;
        }
        let keep = if per_country {
            wanted_country.as_deref() == Some(*cc)
        } else if *bc {
            // A backward-compatibility name has no region of its own
            // (`Brazil/Acre`, `US/Eastern`), so it rides on the group value
            // that asked for them at all.
            true
        } else {
            group_of(id) & group != 0
        };
        if keep {
            a.push(Value::string(id.as_bytes()));
        }
    }
    Ok(Value::Array(a))
}

/// `timezone_abbreviations_list(): array` — every abbreviation php knows,
/// each mapped to the list of zones that use it. The military zones (`a` …
/// `z`) have no zone of their own, so their `timezone_id` is `null`.
pub(crate) fn timezone_abbreviations_list(_: &mut Ctx, _a: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    let mut current: Option<(&str, Array)> = None;
    for (abbr, dst, offset, id) in tz::ABBREVIATION_ROWS {
        let mut row = Array::new();
        row.set(ArrayKey::str(b"dst"), Value::Bool(*dst));
        row.set(ArrayKey::str(b"offset"), Value::Int(*offset as i64));
        row.set(
            ArrayKey::str(b"timezone_id"),
            if id.is_empty() {
                Value::Null
            } else {
                Value::string(id.as_bytes())
            },
        );
        match &mut current {
            Some((name, list)) if *name == *abbr => list.push(Value::Array(row)),
            _ => {
                if let Some((name, list)) = current.take() {
                    out.set(ArrayKey::str(name.as_bytes()), Value::Array(list));
                }
                let mut list = Array::new();
                list.push(Value::Array(row));
                current = Some((abbr, list));
            }
        }
    }
    if let Some((name, list)) = current.take() {
        out.set(ArrayKey::str(name.as_bytes()), Value::Array(list));
    }
    Ok(Value::Array(out))
}

// ---- strftime / gmstrftime -----------------------------------------------------------

/// `%U` and `%W`: the week of the year counted from the first Sunday (`0`)
/// or the first Monday (`1`), with the days before it in week zero.
fn week_number(yday: i64, wday: i64, first_day: i64) -> i64 {
    (yday + 7 - (wday - first_day).rem_euclid(7)) / 7
}

/// C's `strftime` over one rendered instant. An unknown conversion loses its
/// `%` and keeps the letter, which is what the platform C library does with
/// `%q`, `%E` and `%O`.
fn strftime_format(r: &Rendered, fmt: &[u8]) -> Vec<u8> {
    let wday = civil::weekday(civil::days_from_civil(r.year, r.month as i64, r.day as i64)) as i64;
    let yday = civil::day_of_year(r.year, r.month, r.day) as i64;
    let (iso_year, iso_week) = civil::iso_week(r.year, r.month, r.day);
    let day_name = DAY_NAMES[wday as usize];
    let month_name = MONTH_NAMES[(r.month - 1) as usize];
    let hour12 = match r.hour % 12 {
        0 => 12,
        h => h,
    };
    let date_us = format!("{:02}/{:02}/{:02}", r.month, r.day, r.year.rem_euclid(100));
    let time_iso = format!("{:02}:{:02}:{:02}", r.hour, r.minute, r.second);
    let ctime = format!(
        "{} {} {:2} {} {}",
        &day_name[..3],
        &month_name[..3],
        r.day,
        time_iso,
        r.year
    );
    let mut out: Vec<u8> = Vec::with_capacity(fmt.len() * 2);
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&c) = fmt.get(i) else {
            out.push(b'%');
            break;
        };
        i += 1;
        let piece: String = match c {
            b'a' => day_name[..3].to_string(),
            b'A' => day_name.to_string(),
            b'b' | b'h' => month_name[..3].to_string(),
            b'B' => month_name.to_string(),
            b'c' => ctime.clone(),
            b'C' => format!("{:02}", r.year.div_euclid(100)),
            b'd' => format!("{:02}", r.day),
            b'D' | b'x' => date_us.clone(),
            b'e' => format!("{:2}", r.day),
            b'F' => format!("{:04}-{:02}-{:02}", r.year, r.month, r.day),
            b'g' => format!("{:02}", iso_year.rem_euclid(100)),
            b'G' => iso_year.to_string(),
            b'H' => format!("{:02}", r.hour),
            b'I' => format!("{hour12:02}"),
            b'j' => format!("{:03}", yday + 1),
            b'k' => format!("{:2}", r.hour),
            b'l' => format!("{hour12:2}"),
            b'm' => format!("{:02}", r.month),
            b'M' => format!("{:02}", r.minute),
            b'n' => "\n".to_string(),
            b'p' => (if r.hour < 12 { "AM" } else { "PM" }).to_string(),
            b'r' => format!(
                "{hour12:02}:{:02}:{:02} {}",
                r.minute,
                r.second,
                if r.hour < 12 { "AM" } else { "PM" }
            ),
            b'R' => format!("{:02}:{:02}", r.hour, r.minute),
            // php hands the broken-down time back to the C library, which
            // reads `%s` off the *civil* fields: in a zone east of UTC it is
            // ahead of the timestamp that produced it.
            b's' => (civil::days_from_civil(r.year, r.month as i64, r.day as i64) * 86_400
                + r.hour as i64 * 3600
                + r.minute as i64 * 60
                + r.second as i64)
                .to_string(),
            b'S' => format!("{:02}", r.second),
            b't' => "\t".to_string(),
            b'T' | b'X' => time_iso.clone(),
            b'u' => (if wday == 0 { 7 } else { wday }).to_string(),
            b'U' => format!("{:02}", week_number(yday, wday, 0)),
            b'V' => format!("{iso_week:02}"),
            b'w' => wday.to_string(),
            b'W' => format!("{:02}", week_number(yday, wday, 1)),
            b'y' => format!("{:02}", r.year.rem_euclid(100)),
            b'Y' => r.year.to_string(),
            b'z' => {
                let sign = if r.offset < 0 { '-' } else { '+' };
                let a = r.offset.unsigned_abs();
                format!("{sign}{:02}{:02}", a / 3600, (a / 60) % 60)
            }
            b'Z' => r.abbr.clone(),
            b'+' => format!(
                "{} {} {:2} {} {} {}",
                &day_name[..3],
                &month_name[..3],
                r.day,
                time_iso,
                r.abbr,
                r.year
            ),
            b'%' => "%".to_string(),
            other => {
                out.push(other);
                continue;
            }
        };
        out.extend_from_slice(piece.as_bytes());
    }
    out
}

/// `strftime(string $format, ?int $timestamp = null): string|false`
pub(crate) fn strftime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated(
        "Function strftime() is deprecated since 8.1, use IntlDateFormatter::format() instead",
    )?;
    let fmt = str_arg(ctx, "strftime", 1, "format", &args[0])?;
    let ts = opt_int_arg(ctx, "strftime", 2, "timestamp", args.get(1))?.unwrap_or_else(now_seconds);
    let zone = default_tz(ctx);
    let r = format::render(&zone, ts, 0);
    Ok(Value::string(&strftime_format(&r, &fmt)))
}

/// `gmstrftime(string $format, ?int $timestamp = null): string|false`
pub(crate) fn gmstrftime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated(
        "Function gmstrftime() is deprecated since 8.1, use IntlDateFormatter::format() instead",
    )?;
    let fmt = str_arg(ctx, "gmstrftime", 1, "format", &args[0])?;
    let ts =
        opt_int_arg(ctx, "gmstrftime", 2, "timestamp", args.get(1))?.unwrap_or_else(now_seconds);
    Ok(Value::string(&strftime_format(&gm_render(ts), &fmt)))
}
