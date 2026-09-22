//! `IntlTimeZone`: ICU's view of a time zone — its id and the canonical,
//! IANA, Windows and region metadata php's ICU carries (`data/tz.rs`,
//! dumped from the oracle), the offsets and DST rules read from php's own
//! timezone database through ext/date, and the display names CLDR spells
//! (through ICU4X).

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_stdlib::date_bridge::{self, Zone};
use rphp_value::{Object, Payload, Value};

use crate::data::tz as data;
use crate::datefmt::zone::{name as zone_name, Style};
use crate::datefmt::ZoneRef;
use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, IntlError, U_ILLEGAL_ARGUMENT_ERROR};
use crate::{generated, locale_arg, opt_arg};

// display name types
const DISPLAY_SHORT: i64 = 1;
const DISPLAY_LONG: i64 = 2;
const DISPLAY_SHORT_GENERIC: i64 = 3;
const DISPLAY_LONG_GENERIC: i64 = 4;
const DISPLAY_SHORT_GMT: i64 = 5;
const DISPLAY_LONG_GMT: i64 = 6;
const DISPLAY_SHORT_COMMONLY_USED: i64 = 7;
const DISPLAY_GENERIC_LOCATION: i64 = 8;

// enumeration types
const TYPE_ANY: i64 = 0;
const TYPE_CANONICAL: i64 = 1;
const TYPE_CANONICAL_LOCATION: i64 = 2;

/// A zone object's state.
pub struct TzState {
    pub zone: ZoneRef,
    pub err: IntlError,
}

/// The row of a zone id, when ICU knows it.
type Row = (&'static str, &'static str, &'static str, &'static str, &'static str, u16, i32, i32, bool);

fn row(id: &str) -> Option<&'static Row> {
    data::ZONES.binary_search_by(|(z, ..)| (*z).cmp(id)).ok().map(|i| &data::ZONES[i])
}

/// The zone an `IntlTimeZone` object carries.
pub fn zone_of(o: &Object) -> Option<ZoneRef> {
    o.with_payload::<TzState, _>(|s| s.zone.clone())
}

/// A new `IntlTimeZone` for a zone.
pub fn new_object(ctx: &mut Ctx, zone: ZoneRef) -> NativeResult {
    let cid = ctx.lookup_class_or_error(b"IntlTimeZone")?;
    let obj = ctx.instantiate(cid);
    install(&obj, zone);
    Ok(Value::Object(obj))
}

/// Install the state and the four slots php shows in a var_dump.
fn install(o: &Object, zone: ZoneRef) {
    let now = date_bridge::now_seconds();
    o.set(b"valid", Value::Bool(zone.id != "Etc/Unknown"));
    o.set(b"id", Value::string(zone.id.as_bytes()));
    o.set(b"rawOffset", Value::Int(i64::from(raw_offset(&zone)) * 1000));
    o.set(b"currentOffset", Value::Int(i64::from(zone.offset_at(now)) * 1000));
    o.set_payload(Payload::Native(Box::new(TzState { zone, err: IntlError::default() })));
}

/// The zone's standard offset, as ICU reports it.
fn raw_offset(zone: &ZoneRef) -> i32 {
    if let Some(r) = row(&zone.id) {
        return r.6 / 1000;
    }
    match &zone.php {
        Zone::Offset(secs) => *secs,
        Zone::Abbr { offset, .. } => *offset,
        Zone::Id(_) => zone.offset_at(date_bridge::now_seconds()),
    }
}

/// Whether the zone observes daylight time.
fn uses_dst(zone: &ZoneRef) -> bool {
    row(&zone.id).is_some_and(|r| r.8)
}

/// The DST saving a zone applies, in seconds. A zone ICU has no rules
/// for is a `SimpleTimeZone`, whose saving is one hour whether or not it
/// ever applies.
fn dst_savings(zone: &ZoneRef) -> i32 {
    match row(&zone.id) {
        Some(r) => r.7 / 1000,
        None if zone.custom => 3600,
        None => 0,
    }
}

fn with_state<R>(o: &Object, f: impl FnOnce(&mut TzState) -> R) -> Option<R> {
    o.with_payload::<TzState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn state_of<'a>(ctx: &mut Ctx, o: &'a Object) -> Result<&'a Object, Unwind> {
    if o.with_payload::<TzState, _>(|_| ()).is_none() {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Object not initialized")?;
        return Err(Unwind::error("Object not initialized"));
    }
    Ok(o)
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(zone) = src.with_payload::<TzState, _>(|s| s.zone.clone()) {
        install(dst, zone);
    }
    Ok(())
}

// ---- constructors -----------------------------------------------------------------------------

fn construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Call to private IntlTimeZone::__construct() from global scope"))
}

fn create_time_zone(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    match ZoneRef::parse(&id) {
        Some(z) => new_object(ctx, z),
        None => {
            state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
            new_object(ctx, ZoneRef::unknown_custom())
        }
    }
}

fn create_default(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    // ICU's default zone comes from the environment, not from
    // `date.timezone`
    let zone = std::env::var("TZ").ok().and_then(|tz| ZoneRef::parse(&tz)).unwrap_or_else(|| ZoneRef::from_php(date_bridge::default_zone(ctx)));
    new_object(ctx, zone)
}

fn get_gmt(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    new_object(ctx, ZoneRef::parse("GMT").unwrap_or_else(ZoneRef::unknown))
}

fn get_unknown(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    new_object(ctx, ZoneRef::unknown())
}

fn from_date_time_zone(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let who = ctx.active_function_name();
    let Some(Value::Object(o)) = args.first().map(|v| v.deref().into_owned()) else {
        return Err(Unwind::type_error(format!("{who}(): Argument #1 ($timezone) must be of type DateTimeZone")));
    };
    match date_bridge::zone_of(&o) {
        Some(z) => new_object(ctx, ZoneRef::from_php(z)),
        None => {
            state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "could not convert DateTimeZone to IntlTimeZone")?;
            Ok(Value::Null)
        }
    }
}

fn to_date_time_zone(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let zone = with_state(o, |s| s.zone.clone()).unwrap_or_else(ZoneRef::unknown);
    let who = ctx.active_function_name();
    let php = if zone.id == "Etc/Unknown" {
        None
    } else if zone.id.starts_with("GMT") {
        Some(Zone::Offset(raw_offset(&zone)))
    } else {
        Some(zone.php.clone())
    };
    match php {
        Some(z) => {
            // php spells the zone by the id ICU gave it, where its own
            // database knows that name
            let z = if matches!(z, Zone::Offset(_)) { z } else { date_bridge::parse_zone(&zone.id).unwrap_or(z) };
            date_bridge::new_datetime_zone(ctx, z)
        }
        None => Err(Unwind::exception("IntlException", format!("{who}(): DateTimeZone constructor threw exception"))),
    }
}

// ---- accessors --------------------------------------------------------------------------------

fn get_id(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let id = with_state(o, |s| s.zone.id.clone()).unwrap_or_default();
    Ok(Value::string(id.as_bytes()))
}

fn get_raw_offset(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let v = with_state(o, |s| raw_offset(&s.zone)).unwrap_or(0);
    Ok(Value::Int(i64::from(v) * 1000))
}

fn get_dst_savings(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let v = with_state(o, |s| dst_savings(&s.zone)).unwrap_or(0);
    Ok(Value::Int(i64::from(v) * 1000))
}

fn use_daylight_time(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let v = with_state(o, |s| uses_dst(&s.zone)).unwrap_or(false);
    Ok(Value::Bool(v))
}

fn get_offset(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let millis = args.first().map_or(0.0, |v| v.deref().to_float());
    let local = args.get(1).map(|v| v.deref().to_bool()).unwrap_or(false);
    let zone = with_state(o, |s| s.zone.clone()).unwrap_or_else(ZoneRef::unknown);
    let secs = (millis / 1000.0) as i64;
    let ts = if local {
        let guess = secs - i64::from(zone.offset_at(secs));
        guess
    } else {
        secs
    };
    let total = zone.offset_at(ts);
    let raw = raw_offset(&zone);
    if args.len() > 2 {
        args[2] = Value::Int(i64::from(raw) * 1000);
    }
    if args.len() > 3 {
        args[3] = Value::Int(i64::from(total - raw) * 1000);
    }
    Ok(Value::Bool(true))
}

fn has_same_rules(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let mine = with_state(o, |s| s.zone.clone()).unwrap_or_else(ZoneRef::unknown);
    let Some(Value::Object(other)) = args.first().map(|v| v.deref().into_owned()) else {
        return Ok(Value::Bool(false));
    };
    let Some(theirs) = zone_of(&other) else {
        return Ok(Value::Bool(false));
    };
    // the same rules: the same offsets over a decade of samples
    let now = date_bridge::now_seconds();
    let same = (0..40).all(|k| {
        let ts = now - 86_400 * 91 * k;
        mine.offset_at(ts) == theirs.offset_at(ts)
    });
    Ok(Value::Bool(same))
}

fn get_display_name(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let daylight = args.first().map(|v| v.deref().to_bool()).unwrap_or(false);
    let kind = args.get(1).map_or(DISPLAY_LONG, |v| v.deref().to_int());
    let locale = locale_arg(ctx, args, 2);
    let zone = with_state(o, |s| s.zone.clone()).unwrap_or_else(ZoneRef::unknown);
    let bcp47 = crate::locale::split(&locale).0;
    let loc: icu::locale::Locale = bcp47.parse().unwrap_or(icu::locale::Locale::UNKNOWN);
    let now = date_bridge::now_seconds();
    let raw = raw_offset(&zone);
    // ICU asks CLDR for the daylight name only where the zone has rules;
    // elsewhere the standard offset is what it prints
    let offset = if daylight && uses_dst(&zone) { raw + dst_savings(&zone) } else { raw };
    let style = match kind {
        DISPLAY_SHORT | DISPLAY_SHORT_COMMONLY_USED => {
            if daylight {
                Style::DaylightShort
            } else {
                Style::SpecificShort
            }
        }
        DISPLAY_LONG => {
            if daylight {
                Style::DaylightLong
            } else {
                Style::SpecificLong
            }
        }
        DISPLAY_SHORT_GENERIC => Style::GenericShort,
        DISPLAY_LONG_GENERIC => Style::GenericLong,
        DISPLAY_SHORT_GMT => {
            return Ok(Value::string(crate::datefmt::zone::iso_offset(offset, true, false, 2, false).as_bytes()));
        }
        DISPLAY_LONG_GMT => Style::OffsetLong,
        DISPLAY_GENERIC_LOCATION => Style::Location,
        _ => Style::SpecificLong,
    };
    let text = zone_name(&loc, &zone.iana, offset, now, style).unwrap_or_default();
    Ok(Value::string(text.as_bytes()))
}

fn get_error_code(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |s| s.err.code).unwrap_or(0)))
}

fn get_error_message(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::string(with_state(o, |s| s.err.message()).unwrap_or_default().as_bytes()))
}

// ---- the static metadata ----------------------------------------------------------------------

fn get_canonical_id(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    if id == "Etc/Unknown" {
        state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
        if args.len() > 1 {
            args[1] = Value::Bool(false);
        }
        return Ok(Value::string(id.as_bytes()));
    }
    match row(&id) {
        Some((_, canon, ..)) => {
            if args.len() > 1 {
                args[1] = Value::Bool(true);
            }
            Ok(Value::string(canon.as_bytes()))
        }
        None => {
            // a custom offset zone is its own canonical id
            match ZoneRef::parse(&id) {
                Some(z) if !z.id.is_empty() && z.id != "Etc/Unknown" => {
                    if args.len() > 1 {
                        args[1] = Value::Bool(false);
                    }
                    Ok(Value::string(z.id.as_bytes()))
                }
                _ => {
                    state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
                    Ok(Value::Bool(false))
                }
            }
        }
    }
}

fn get_region(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    match row(&id).map(|r| r.2).filter(|r| !r.is_empty()) {
        Some(r) => Ok(Value::string(r.as_bytes())),
        None => {
            state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
            Ok(Value::Bool(false))
        }
    }
}

fn get_iana_id(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    match row(&id).map(|r| r.3).filter(|s| !s.is_empty()) {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => {
            state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
            Ok(Value::Bool(false))
        }
    }
}

fn get_windows_id(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    match row(&id).map(|r| r.4).filter(|s| !s.is_empty()) {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => {
            state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
            Ok(Value::Bool(false))
        }
    }
}

fn get_id_for_windows_id(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let win = crate::text_arg(args, 0);
    let region = opt_arg(args, 1).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    let pick = |r: &str| data::WINDOWS.iter().find(|(w, reg, _)| *w == win && *reg == r).map(|(_, _, id)| *id);
    let found = match &region {
        Some(r) => pick(r).or_else(|| pick("")),
        None => pick(""),
    };
    match found {
        Some(id) => Ok(Value::string(id.as_bytes())),
        None => {
            state::set_global_code(ctx, U_ILLEGAL_ARGUMENT_ERROR);
            Ok(Value::Bool(false))
        }
    }
}

fn count_equivalent_ids(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    let n = row(&id).map_or(0, |r| data::GROUPS[r.5 as usize].len());
    Ok(Value::Int(if n <= 1 { 0 } else { n as i64 }))
}

fn get_equivalent_id(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let id = crate::text_arg(args, 0);
    let index = args.get(1).map_or(0, |v| v.deref().to_int());
    let members = row(&id).map(|r| data::GROUPS[r.5 as usize]).filter(|m| m.len() > 1);
    match members.and_then(|m| usize::try_from(index).ok().and_then(|i| m.get(i))) {
        Some(s) => Ok(Value::string(s.as_bytes())),
        None => Ok(Value::string(b"")),
    }
}

fn get_tz_data_version(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    Ok(Value::string(data::TZDATA_VERSION.as_bytes()))
}

/// The id list an enumeration walks.
fn enumeration_ids(kind: i64, region: Option<&str>, raw_offset_ms: Option<i64>) -> Vec<&'static str> {
    let base: Vec<&'static str> = match kind {
        TYPE_CANONICAL => data::TYPE_CANONICAL.to_vec(),
        TYPE_CANONICAL_LOCATION => data::TYPE_CANONICAL_LOCATION.to_vec(),
        _ => data::ZONES.iter().map(|(id, ..)| *id).collect(),
    };
    base.into_iter()
        .filter(|id| match region {
            Some(r) => row(id).is_some_and(|row| row.2 == r),
            None => true,
        })
        .filter(|id| match raw_offset_ms {
            Some(ms) => row(id).is_some_and(|r| i64::from(r.6) == ms),
            None => true,
        })
        .collect()
}

/// An `IntlIterator` over a list of ids.
fn ids_iterator(ctx: &mut Ctx, ids: Vec<&'static str>) -> NativeResult {
    let mut a = rphp_value::Array::new();
    for id in ids {
        a.push(Value::string(id.as_bytes()));
    }
    crate::iterator::new_object(ctx, a)
}

fn create_enumeration(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let (region, offset) = match opt_arg(args, 0).map(|v| v.deref().into_owned()) {
        None => (None, None),
        Some(Value::Int(n)) => (None, Some(n)),
        Some(Value::Float(f)) => (None, Some(f as i64)),
        Some(other) => (Some(String::from_utf8_lossy(&other.to_php_bytes()).into_owned()), None),
    };
    let ids = enumeration_ids(TYPE_ANY, region.as_deref(), offset);
    ids_iterator(ctx, ids)
}

fn create_time_zone_id_enumeration(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let kind = args.first().map_or(TYPE_ANY, |v| v.deref().to_int());
    let region = opt_arg(args, 1).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    let offset = opt_arg(args, 2).map(|v| v.to_int());
    let ids = enumeration_ids(kind, region.as_deref(), offset);
    ids_iterator(ctx, ids)
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("createTimeZone", create_time_zone),
    ("createDefault", create_default),
    ("getGMT", get_gmt),
    ("getUnknown", get_unknown),
    ("fromDateTimeZone", from_date_time_zone),
    ("toDateTimeZone", to_date_time_zone),
    ("getID", get_id),
    ("getRawOffset", get_raw_offset),
    ("getDSTSavings", get_dst_savings),
    ("useDaylightTime", use_daylight_time),
    ("getOffset", get_offset),
    ("hasSameRules", has_same_rules),
    ("getDisplayName", get_display_name),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
    ("getCanonicalID", get_canonical_id),
    ("getRegion", get_region),
    ("getIanaID", get_iana_id),
    ("getWindowsID", get_windows_id),
    ("getIDForWindowsID", get_id_for_windows_id),
    ("countEquivalentIDs", count_equivalent_ids),
    ("getEquivalentID", get_equivalent_id),
    ("getTZDataVersion", get_tz_data_version),
    ("createEnumeration", create_enumeration),
    ("createTimeZoneIDEnumeration", create_time_zone_id_enumeration),
];

macro_rules! as_function {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            let obj = match args.first().map(|v| v.deref().into_owned()) {
                Some(Value::Object(o)) => o,
                _ => return Err(Unwind::type_error("Argument #1 ($timezone) must be of type IntlTimeZone")),
            };
            let rest = &mut args[1..];
            $method(ctx, Some(&obj), rest)
        }
    };
}

macro_rules! as_static {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            $method(ctx, None, args)
        }
    };
}

as_function!(f_to_date_time_zone, to_date_time_zone);
as_function!(f_get_id, get_id);
as_function!(f_get_raw_offset, get_raw_offset);
as_function!(f_get_dst_savings, get_dst_savings);
as_function!(f_use_daylight_time, use_daylight_time);
as_function!(f_get_offset, get_offset);
as_function!(f_has_same_rules, has_same_rules);
as_function!(f_get_display_name, get_display_name);
as_function!(f_get_error_code, get_error_code);
as_function!(f_get_error_message, get_error_message);
as_static!(f_create_time_zone, create_time_zone);
as_static!(f_create_default, create_default);
as_static!(f_get_gmt, get_gmt);
as_static!(f_get_unknown, get_unknown);
as_static!(f_from_date_time_zone, from_date_time_zone);
as_static!(f_get_canonical_id, get_canonical_id);
as_static!(f_get_region, get_region);
as_static!(f_get_iana_id, get_iana_id);
as_static!(f_get_windows_id, get_windows_id);
as_static!(f_get_id_for_windows_id, get_id_for_windows_id);
as_static!(f_count_equivalent_ids, count_equivalent_ids);
as_static!(f_get_equivalent_id, get_equivalent_id);
as_static!(f_get_tz_data_version, get_tz_data_version);
as_static!(f_create_enumeration, create_enumeration);
as_static!(f_create_time_zone_id_enumeration, create_time_zone_id_enumeration);

static FUNCTIONS: &[FnImpl] = &[
    ("intltz_create_time_zone", f_create_time_zone),
    ("intltz_create_default", f_create_default),
    ("intltz_get_gmt", f_get_gmt),
    ("intltz_get_unknown", f_get_unknown),
    ("intltz_from_date_time_zone", f_from_date_time_zone),
    ("intltz_to_date_time_zone", f_to_date_time_zone),
    ("intltz_get_id", f_get_id),
    ("intltz_get_raw_offset", f_get_raw_offset),
    ("intltz_get_dst_savings", f_get_dst_savings),
    ("intltz_use_daylight_time", f_use_daylight_time),
    ("intltz_get_offset", f_get_offset),
    ("intltz_has_same_rules", f_has_same_rules),
    ("intltz_get_display_name", f_get_display_name),
    ("intltz_get_error_code", f_get_error_code),
    ("intltz_get_error_message", f_get_error_message),
    ("intltz_get_canonical_id", f_get_canonical_id),
    ("intltz_get_region", f_get_region),
    ("intltz_get_iana_id", f_get_iana_id),
    ("intltz_get_windows_id", f_get_windows_id),
    ("intltz_get_id_for_windows_id", f_get_id_for_windows_id),
    ("intltz_count_equivalent_ids", f_count_equivalent_ids),
    ("intltz_get_equivalent_id", f_get_equivalent_id),
    ("intltz_get_tz_data_version", f_get_tz_data_version),
    ("intltz_create_enumeration", f_create_enumeration),
    ("intltz_create_time_zone_id_enumeration", f_create_time_zone_id_enumeration),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLTIMEZONE, METHODS, |b| b.payload_clone(payload_clone));
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
