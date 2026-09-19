//! `DateTimeZone` (php-src `ext/date/php_date.c`, the `timezone_*` half).
//!
//! The class is a thin shell around [`Tz`]: `__construct` runs php's
//! three-step resolution (offset, then abbreviation, then identifier) and
//! everything else reads the result. The two slots php shows —
//! `timezone_type` and `timezone` — are kept in step with it.
//!
//! `getTransitions()` is the one method that needs more than [`Tz`]: it
//! walks the zone database itself. php's shape is a synthetic first row for
//! the lower bound followed by every transition inside the window, and
//! `false` for a zone that has no transitions to report — an offset or an
//! abbreviation.

use jiff::tz::TimeZone;
use jiff::Timestamp;

use rphp_runtime::{nm, ClassKind, Ctx, NativeMethod, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, ArrayKey, Object, Value};

use super::super::format;
use super::super::tz::{self, Tz};
use super::{date_throw, int_arg, obj_arg, put_zone, str_arg, this, zone_state};

/// A static method descriptor (the instance one is `nm!`).
pub(crate) fn snm(min: u8, max: Option<u8>, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

/// `DateTimeZone::__construct(string $timezone)`
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = str_arg(ctx, "DateTimeZone::__construct", 1, "timezone", &args[0])?;
    let name = String::from_utf8_lossy(&name).into_owned();
    match Tz::parse(&name) {
        Some(tz) => {
            put_zone(o, tz);
            Ok(Value::Null)
        }
        None => Err(date_throw(
            ctx,
            "DateInvalidTimeZoneException",
            format!("DateTimeZone::__construct(): Unknown or bad timezone ({name})"),
        )),
    }
}

/// The zone behind a receiver, or php's `DateObjectError` for an instance
/// whose constructor never ran.
pub(crate) fn zone_of(ctx: &mut Ctx, o: &Object, who: &str) -> Result<Tz, Unwind> {
    match zone_state(o) {
        Some(tz) => Ok(tz),
        None => Err(date_throw(
            ctx,
            "DateObjectError",
            format!("{who}(): The DateTimeZone object has not been correctly initialized by its constructor"),
        )),
    }
}

/// `DateTimeZone::getName(): string`
pub(crate) fn get_name(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let tz = zone_of(ctx, o, "DateTimeZone::getName")?;
    Ok(Value::string(tz.name().as_bytes()))
}

/// `DateTimeZone::getOffset(DateTimeInterface $datetime): int`
pub(crate) fn get_offset(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let tz = zone_of(ctx, o, "DateTimeZone::getOffset")?;
    let target = obj_arg(
        ctx,
        "DateTimeZone::getOffset",
        1,
        "datetime",
        "DateTimeInterface",
        false,
        args.first(),
    )?
    .ok_or_else(|| {
        Unwind::type_error(
            "DateTimeZone::getOffset(): Argument #1 ($datetime) must be of type DateTimeInterface, null given",
        )
    })?;
    Ok(Value::Int(tz.offset_at(super::dt_state(&target).ts) as i64))
}

/// The lowest and highest second jiff's calendar can be asked about.
const JIFF_LOW: i64 = -377_705_023_201;
/// See [`JIFF_LOW`].
const JIFF_HIGH: i64 = 253_402_207_200;

/// A second count jiff will accept.
fn stamp(secs: i64) -> Timestamp {
    Timestamp::from_second(secs.clamp(JIFF_LOW, JIFF_HIGH)).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// One row of `getTransitions()`: the instant, its UTC rendering, and the
/// state that starts there.
fn transition_row(ts: i64, offset: i32, dst: bool, abbr: &str) -> Value {
    let mut a = Array::new();
    a.set(ArrayKey::str(b"ts"), Value::Int(ts));
    let rendered = format::render(&Tz::utc(), ts, 0);
    a.set(
        ArrayKey::str(b"time"),
        Value::string(&format::format(&rendered, b"c")),
    );
    a.set(ArrayKey::str(b"offset"), Value::Int(offset as i64));
    a.set(ArrayKey::str(b"isdst"), Value::Bool(dst));
    a.set(ArrayKey::str(b"abbr"), Value::string(abbr.as_bytes()));
    Value::Array(a)
}

/// `DateTimeZone::getTransitions(int $timestampBegin = PHP_INT_MIN, int $timestampEnd = PHP_INT_MAX): array|false`
pub(crate) fn get_transitions(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let o = this(o)?;
    let tz = zone_of(ctx, o, "DateTimeZone::getTransitions")?;
    let begin = match args.first() {
        Some(v) => int_arg(ctx, "DateTimeZone::getTransitions", 1, "timestampBegin", v)?,
        None => i64::MIN,
    };
    let end = match args.get(1) {
        Some(v) => int_arg(ctx, "DateTimeZone::getTransitions", 2, "timestampEnd", v)?,
        None => i64::MAX,
    };
    // Only an identifier has a transition table; php answers `false` for the
    // other two shapes rather than a one-row list.
    let Tz::Id(id) = &tz else {
        return Ok(Value::Bool(false));
    };
    let Ok(zone) = TimeZone::get(id) else {
        return Ok(Value::Bool(false));
    };
    let mut out = Array::new();
    if begin > end {
        return Ok(Value::Array(out));
    }
    let (offset, abbr, dst) = tz.info(begin);
    out.push(transition_row(begin, offset, dst, &abbr));
    for t in zone.following(stamp(begin)) {
        let ts = t.timestamp().as_second();
        if ts > end {
            break;
        }
        out.push(transition_row(
            ts,
            t.offset().seconds(),
            t.dst().is_dst(),
            t.abbreviation(),
        ));
    }
    Ok(Value::Array(out))
}

/// `DateTimeZone::listIdentifiers(int $timezoneGroup = DateTimeZone::ALL, ?string $countryCode = null): array`
fn list_identifiers(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    super::super::funcs::timezone_identifiers_list(ctx, args)
}

/// `DateTimeZone::listAbbreviations(): array`
fn list_abbreviations(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    super::super::funcs::timezone_abbreviations_list(ctx, args)
}

/// The `[timezone_type, timezone]` bag `__serialize`, `var_export` and
/// `serialize` all go through.
fn state_array(tz: &Tz) -> Array {
    let mut a = Array::new();
    a.set(ArrayKey::str(b"timezone_type"), Value::Int(tz.type_id()));
    a.set(
        ArrayKey::str(b"timezone"),
        Value::string(tz.name().as_bytes()),
    );
    a
}

/// `DateTimeZone::__serialize(): array`
fn magic_serialize(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let tz = zone_of(ctx, o, "DateTimeZone::__serialize")?;
    Ok(Value::Array(state_array(&tz)))
}

/// The name a property bag carries, for `__unserialize` / `__set_state`.
fn name_from_bag(bag: &Array) -> Option<String> {
    bag.get_deref(&ArrayKey::str(b"timezone"))
        .map(|v| v.to_php_string())
}

/// `DateTimeZone::__unserialize(array $data): void`
fn magic_unserialize(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Value::Array(bag) = &*args[0].deref() else {
        return Err(Unwind::type_error(
            "DateTimeZone::__unserialize(): Argument #1 ($data) must be of type array",
        ));
    };
    let name = name_from_bag(bag).unwrap_or_default();
    match Tz::parse(&name) {
        Some(tz) => {
            put_zone(o, tz);
            Ok(Value::Null)
        }
        None => Err(date_throw(
            ctx,
            "DateInvalidTimeZoneException",
            format!("DateTimeZone::__unserialize(): Unknown or bad timezone ({name})"),
        )),
    }
}

/// `DateTimeZone::__wakeup(): void` — the pre-`__serialize` form, where the
/// two properties are already on the object.
fn magic_wakeup(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = o
        .get_deref(b"timezone")
        .map(|v| v.to_php_string())
        .unwrap_or_default();
    match Tz::parse(&name) {
        Some(tz) => {
            put_zone(o, tz);
            Ok(Value::Null)
        }
        None => Err(date_throw(
            ctx,
            "DateInvalidTimeZoneException",
            format!("DateTimeZone::__wakeup(): Unknown or bad timezone ({name})"),
        )),
    }
}

/// `DateTimeZone::__set_state(array $array): DateTimeZone`
fn set_state(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Value::Array(bag) = &*args[0].deref() else {
        return Err(Unwind::type_error(
            "DateTimeZone::__set_state(): Argument #1 ($array) must be of type array",
        ));
    };
    let name = name_from_bag(bag).unwrap_or_default();
    let Some(tz) = Tz::parse(&name) else {
        return Err(date_throw(
            ctx,
            "DateInvalidTimeZoneException",
            format!("DateTimeZone::__set_state(): Unknown or bad timezone ({name})"),
        ));
    };
    Ok(Value::Object(new_zone(ctx, tz)?))
}

/// A fresh `DateTimeZone` carrying `tz`, for every place that answers one.
pub(crate) fn new_zone(ctx: &mut Ctx, tz: Tz) -> Result<Object, Unwind> {
    let cid = ctx
        .class_by_name(b"DateTimeZone")
        .ok_or_else(|| Unwind::error("Class \"DateTimeZone\" not found"))?;
    let o = ctx.instantiate(cid);
    put_zone(&o, tz);
    Ok(o)
}

/// `timezone_name_from_abbr(string $abbr, int $utcOffset = -1, int $isDST = -1): string|false`
pub(crate) fn name_from_abbr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let abbr = str_arg(ctx, "timezone_name_from_abbr", 1, "abbr", &args[0])?;
    let abbr = String::from_utf8_lossy(&abbr).to_ascii_lowercase();
    let offset = match args.get(1) {
        Some(v) => int_arg(ctx, "timezone_name_from_abbr", 2, "utcOffset", v)?,
        None => -1,
    };
    let dst = match args.get(2) {
        Some(v) => int_arg(ctx, "timezone_name_from_abbr", 3, "isDST", v)?,
        None => -1,
    };
    // With no abbreviation php walks its own table and answers the first row
    // whose offset and DST flag match — an order rphp's table does not share,
    // so the answers for that path are pinned here, read off the oracle
    // across every quarter-hour offset.
    if abbr.is_empty() && offset != -1 {
        for (row_offset, row_dst, id) in OFFSET_ONLY {
            if *row_offset != offset {
                continue;
            }
            if dst != -1 && *row_dst != (dst != 0) {
                continue;
            }
            return Ok(Value::string(id.as_bytes()));
        }
        return Ok(Value::Bool(false));
    }
    for (name, row_dst, row_offset, id) in tz::ABBREVIATION_ROWS {
        if id.is_empty() {
            continue;
        }
        if !abbr.is_empty() && *name != abbr {
            continue;
        }
        if offset != -1 && *row_offset as i64 != offset {
            continue;
        }
        if dst != -1 && *row_dst != (dst != 0) {
            continue;
        }
        return Ok(Value::string(id.as_bytes()));
    }
    Ok(Value::Bool(false))
}

/// Register `DateTimeZone`.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("DateTimeZone")
        .kind(ClassKind::Class)
        .class_const("AFRICA", Value::Int(1))
        .class_const("AMERICA", Value::Int(2))
        .class_const("ANTARCTICA", Value::Int(4))
        .class_const("ARCTIC", Value::Int(8))
        .class_const("ASIA", Value::Int(16))
        .class_const("ATLANTIC", Value::Int(32))
        .class_const("AUSTRALIA", Value::Int(64))
        .class_const("EUROPE", Value::Int(128))
        .class_const("INDIAN", Value::Int(256))
        .class_const("PACIFIC", Value::Int(512))
        .class_const("UTC", Value::Int(1024))
        .class_const("ALL", Value::Int(2047))
        .class_const("ALL_WITH_BC", Value::Int(4095))
        .class_const("PER_COUNTRY", Value::Int(4096))
        // The two keys php's debug handler shows, in php's order.
        .prop("timezone_type", Visibility::Public, Value::Int(3))
        .prop("timezone", Visibility::Public, Value::string(b"UTC"))
        .payload_clone(super::zone_clone)
        .method("__construct", nm!(1, Some(1), construct))
        .method("getName", nm!(0, Some(0), get_name))
        .method("getOffset", nm!(1, Some(1), get_offset))
        .method("getTransitions", nm!(0, Some(2), get_transitions))
        .method("listAbbreviations", snm(0, Some(0), list_abbreviations))
        .method("listIdentifiers", snm(0, Some(2), list_identifiers))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("__wakeup", nm!(0, Some(0), magic_wakeup))
        .method("__set_state", snm(1, Some(1), set_state))
        .finish();
}

/// What php answers for `timezone_name_from_abbr("", $offset, $isDST)`: the
/// first row of *its* abbreviation table with that offset and flag. Read off
/// the oracle for every quarter-hour offset in `±14:00`, because the order of
/// a table is not something a re-derivation can reproduce.
const OFFSET_ONLY: &[(i64, bool, &str)] = &[
    (-39600, false, "Pacific/Apia"),
    (-36000, false, "Pacific/Honolulu"),
    (-32400, false, "America/Anchorage"),
    (-28800, false, "America/Los_Angeles"),
    (-28800, true, "America/Anchorage"),
    (-25200, false, "America/Denver"),
    (-25200, true, "America/Los_Angeles"),
    (-21600, false, "America/Chicago"),
    (-21600, true, "America/Denver"),
    (-18000, false, "America/New_York"),
    (-18000, true, "America/Chicago"),
    (-16200, false, "America/Caracas"),
    (-14400, false, "America/Halifax"),
    (-14400, true, "America/New_York"),
    (-10800, false, "America/Sao_Paulo"),
    (-10800, true, "America/Halifax"),
    (-7200, true, "America/Sao_Paulo"),
    (-3600, false, "Atlantic/Azores"),
    (0, false, "Europe/London"),
    (0, true, "Atlantic/Azores"),
    (3600, false, "Europe/Paris"),
    (3600, true, "Europe/London"),
    (7200, false, "Europe/Helsinki"),
    (7200, true, "Europe/Paris"),
    (10800, false, "Europe/Moscow"),
    (10800, true, "Europe/Helsinki"),
    (14400, false, "Asia/Dubai"),
    (14400, true, "Europe/Moscow"),
    (18000, false, "Asia/Karachi"),
    (19800, false, "Asia/Kolkata"),
    (20700, false, "Asia/Katmandu"),
    (21600, true, "Asia/Yekaterinburg"),
    (25200, false, "Asia/Krasnoyarsk"),
    (25200, true, "Asia/Novosibirsk"),
    (28800, false, "Asia/Shanghai"),
    (28800, true, "Asia/Krasnoyarsk"),
    (32400, false, "Asia/Tokyo"),
    (36000, false, "Australia/Melbourne"),
    (37800, true, "Australia/Adelaide"),
    (39600, true, "Australia/Melbourne"),
    (43200, false, "Pacific/Auckland"),
    (46800, true, "Pacific/Auckland"),
];
