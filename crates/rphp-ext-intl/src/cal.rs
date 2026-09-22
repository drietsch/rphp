//! `IntlCalendar` and `IntlGregorianCalendar`: an instant, a zone and a
//! locale's week rules, with ICU's field numbering over them. The
//! calendar arithmetic is `datefmt::civil`'s, the zone database php's
//! own through ext/date.

use rphp_runtime::{Ctx, ErrLevel, Interp, NativeResult, Registry, Unwind};
use rphp_stdlib::date_bridge;
use rphp_value::{Array, Object, Payload, Value};

use crate::datefmt::{civil, ZoneRef};
use crate::locale;
use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, IntlError, U_ILLEGAL_ARGUMENT_ERROR};
use crate::{generated, locale_arg, opt_arg};

// `UCalendarDateFields`
const FIELD_ERA: i64 = 0;
const FIELD_YEAR: i64 = 1;
const FIELD_MONTH: i64 = 2;
const FIELD_WEEK_OF_YEAR: i64 = 3;
const FIELD_WEEK_OF_MONTH: i64 = 4;
const FIELD_DATE: i64 = 5;
const FIELD_DAY_OF_YEAR: i64 = 6;
const FIELD_DAY_OF_WEEK: i64 = 7;
const FIELD_DAY_OF_WEEK_IN_MONTH: i64 = 8;
const FIELD_AM_PM: i64 = 9;
const FIELD_HOUR: i64 = 10;
const FIELD_HOUR_OF_DAY: i64 = 11;
const FIELD_MINUTE: i64 = 12;
const FIELD_SECOND: i64 = 13;
const FIELD_MILLISECOND: i64 = 14;
const FIELD_ZONE_OFFSET: i64 = 15;
const FIELD_DST_OFFSET: i64 = 16;
const FIELD_YEAR_WOY: i64 = 17;
const FIELD_DOW_LOCAL: i64 = 18;
const FIELD_EXTENDED_YEAR: i64 = 19;
const FIELD_JULIAN_DAY: i64 = 20;
const FIELD_MILLISECONDS_IN_DAY: i64 = 21;
const FIELD_IS_LEAP_MONTH: i64 = 22;
const FIELD_COUNT: i64 = 23;

/// A calendar's state.
pub struct CalState {
    /// The instant in milliseconds since the epoch.
    millis: i64,
    zone: ZoneRef,
    requested: String,
    valid: String,
    week: civil::WeekData,
    lenient: bool,
    pub err: IntlError,
}

impl CalState {
    fn new(zone: ZoneRef, locale: &str, millis: i64) -> CalState {
        let canonical = locale::canonical(locale);
        let bcp47 = locale::split(locale).0;
        let (valid, _) = crate::datefmt::names_of(&canonical);
        CalState {
            millis,
            zone,
            requested: locale.to_string(),
            valid,
            week: civil::WeekData::for_locale(&bcp47),
            lenient: true,
            err: IntlError::default(),
        }
    }

    fn seconds(&self) -> i64 {
        self.millis.div_euclid(1000)
    }

    fn fields(&self) -> civil::Fields {
        let ts = self.seconds();
        let offset = self.zone.offset_at(ts);
        civil::fields_at(ts, (self.millis.rem_euclid(1000) * 1000) as u32, offset)
    }

    fn raw_offset(&self) -> i32 {
        let ts = self.seconds();
        let mut min = self.zone.offset_at(ts);
        for k in 1..13 {
            min = min.min(self.zone.offset_at(ts - 86_400 * 30 * k));
        }
        min
    }

    fn get(&self, field: i64) -> Option<i64> {
        let f = self.fields();
        let offset = self.zone.offset_at(self.seconds());
        let raw = self.raw_offset();
        Some(match field {
            FIELD_ERA => i64::from(f.era),
            FIELD_YEAR => f.year,
            FIELD_MONTH => i64::from(f.month) - 1,
            FIELD_WEEK_OF_YEAR => i64::from(self.week.week_of_year(&f).0),
            FIELD_WEEK_OF_MONTH => i64::from(self.week.week_of_month(&f)),
            FIELD_DATE => i64::from(f.day),
            FIELD_DAY_OF_YEAR => i64::from(f.day_of_year),
            FIELD_DAY_OF_WEEK => i64::from(f.weekday) + 1,
            FIELD_DAY_OF_WEEK_IN_MONTH => i64::from((f.day - 1) / 7 + 1),
            FIELD_AM_PM => i64::from(f.hour >= 12),
            FIELD_HOUR => i64::from(f.hour % 12),
            FIELD_HOUR_OF_DAY => i64::from(f.hour),
            FIELD_MINUTE => i64::from(f.minute),
            FIELD_SECOND => i64::from(f.second),
            FIELD_MILLISECOND => i64::from(f.millis),
            FIELD_ZONE_OFFSET => i64::from(raw) * 1000,
            FIELD_DST_OFFSET => i64::from(offset - raw) * 1000,
            FIELD_YEAR_WOY => self.week.week_of_year(&f).1,
            FIELD_DOW_LOCAL => i64::from((f.weekday + 7 - self.week.first_day) % 7) + 1,
            FIELD_EXTENDED_YEAR => f.ext_year,
            FIELD_JULIAN_DAY => f.days + 2_440_588,
            FIELD_MILLISECONDS_IN_DAY => (i64::from(f.hour) * 3600 + i64::from(f.minute) * 60 + i64::from(f.second)) * 1000 + i64::from(f.millis),
            FIELD_IS_LEAP_MONTH => 0,
            _ => return None,
        })
    }

    /// Set the fields of the local date and recompute the instant.
    fn set_fields(&mut self, year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) {
        let f = self.fields();
        let days = civil::days_from_civil(year, 1, 1) + month.rem_euclid(12) * 0; // the month rolls below
        let _ = days;
        let (y, m) = (year + month.div_euclid(12), month.rem_euclid(12) + 1);
        let start = civil::days_from_civil(y, m as u32, 1);
        let days = start + day - 1;
        let millis_in_day = (hour * 3600 + minute * 60 + second) * 1000 + i64::from(f.millis);
        let local = days * 86_400 * 1000 + millis_in_day;
        self.set_local_millis(local);
    }

    /// Resolve a wall clock (in local milliseconds) into an instant.
    fn set_local_millis(&mut self, local: i64) {
        let secs = local.div_euclid(1000);
        let guess = secs - i64::from(self.zone.offset_at(secs));
        let offset = self.zone.offset_at(guess);
        self.millis = local - i64::from(offset) * 1000;
    }

    fn local_millis(&self) -> i64 {
        self.millis + i64::from(self.zone.offset_at(self.seconds())) * 1000
    }

    /// `add()`: the field moved, the rest kept.
    fn add(&mut self, field: i64, amount: i64) {
        let f = self.fields();
        match field {
            FIELD_YEAR | FIELD_EXTENDED_YEAR | FIELD_YEAR_WOY => self.set_fields(f.ext_year + amount, i64::from(f.month) - 1, i64::from(f.day), i64::from(f.hour), i64::from(f.minute), i64::from(f.second)),
            FIELD_MONTH => self.set_fields(f.ext_year, i64::from(f.month) - 1 + amount, i64::from(f.day), i64::from(f.hour), i64::from(f.minute), i64::from(f.second)),
            FIELD_DATE | FIELD_DAY_OF_YEAR | FIELD_DAY_OF_WEEK => self.set_local_millis(self.local_millis() + amount * 86_400_000),
            FIELD_WEEK_OF_YEAR | FIELD_WEEK_OF_MONTH | FIELD_DAY_OF_WEEK_IN_MONTH => self.set_local_millis(self.local_millis() + amount * 7 * 86_400_000),
            FIELD_HOUR | FIELD_HOUR_OF_DAY => self.millis += amount * 3_600_000,
            FIELD_MINUTE => self.millis += amount * 60_000,
            FIELD_SECOND => self.millis += amount * 1000,
            FIELD_MILLISECOND | FIELD_MILLISECONDS_IN_DAY => self.millis += amount,
            FIELD_AM_PM => self.millis += amount * 12 * 3_600_000,
            _ => {}
        }
    }

    /// `roll()`: the field moved without carrying into the larger ones.
    fn roll(&mut self, field: i64, amount: i64) {
        let f = self.fields();
        match field {
            FIELD_MONTH => {
                let m = (i64::from(f.month) - 1 + amount).rem_euclid(12);
                let day = i64::from(f.day).min(i64::from(civil::days_in_month(f.ext_year, m as u32 + 1, f.julian)));
                self.set_fields(f.ext_year, m, day, i64::from(f.hour), i64::from(f.minute), i64::from(f.second));
            }
            FIELD_DATE => {
                let len = i64::from(civil::days_in_month(f.ext_year, f.month, f.julian));
                let d = (i64::from(f.day) - 1 + amount).rem_euclid(len) + 1;
                self.set_fields(f.ext_year, i64::from(f.month) - 1, d, i64::from(f.hour), i64::from(f.minute), i64::from(f.second));
            }
            FIELD_HOUR_OF_DAY => {
                let h = (i64::from(f.hour) + amount).rem_euclid(24);
                self.set_fields(f.ext_year, i64::from(f.month) - 1, i64::from(f.day), h, i64::from(f.minute), i64::from(f.second));
            }
            FIELD_MINUTE => {
                let m = (i64::from(f.minute) + amount).rem_euclid(60);
                self.set_fields(f.ext_year, i64::from(f.month) - 1, i64::from(f.day), i64::from(f.hour), m, i64::from(f.second));
            }
            FIELD_SECOND => {
                let s = (i64::from(f.second) + amount).rem_euclid(60);
                self.set_fields(f.ext_year, i64::from(f.month) - 1, i64::from(f.day), i64::from(f.hour), i64::from(f.minute), s);
            }
            _ => self.add(field, amount),
        }
    }
}

/// The instant of an `IntlCalendar` object, in seconds and microseconds.
pub fn instant_of(o: &Object) -> Option<(i64, u32)> {
    o.with_payload::<CalState, _>(|s| (s.millis.div_euclid(1000), (s.millis.rem_euclid(1000) * 1000) as u32))
}

/// The zone of an `IntlCalendar` object.
pub fn zone_of(o: &Object) -> Option<ZoneRef> {
    o.with_payload::<CalState, _>(|s| s.zone.clone())
}

/// A new `IntlGregorianCalendar` (what `createInstance` hands back).
pub fn new_object(ctx: &mut Ctx, zone: ZoneRef, _calendar: &str, locale: &str) -> NativeResult {
    let cid = ctx.lookup_class_or_error(b"IntlGregorianCalendar")?;
    let obj = ctx.instantiate(cid);
    let now = date_bridge::now_seconds() * 1000;
    obj.set_payload(Payload::Native(Box::new(CalState::new(zone, locale, now))));
    Ok(Value::Object(obj))
}

fn with_state<R>(o: &Object, f: impl FnOnce(&mut CalState) -> R) -> Option<R> {
    o.with_payload::<CalState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn state_of<'a>(ctx: &mut Ctx, o: &'a Object) -> Result<&'a Object, Unwind> {
    if o.with_payload::<CalState, _>(|_| ()).is_none() {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Object not initialized")?;
        return Err(Unwind::error("Object not initialized"));
    }
    Ok(o)
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = src.with_payload::<CalState, _>(|s| CalState {
        millis: s.millis,
        zone: s.zone.clone(),
        requested: s.requested.clone(),
        valid: s.valid.clone(),
        week: s.week,
        lenient: s.lenient,
        err: IntlError::default(),
    }) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

/// The zone and locale arguments every constructor takes.
fn zone_and_locale(ctx: &mut Ctx, args: &[Value], zi: usize, li: usize, who: &str) -> Result<(ZoneRef, String), Unwind> {
    let zone = crate::datefmt::zone_arg(ctx, args.get(zi), who)?;
    let locale = locale_arg(ctx, args, li);
    Ok((zone, locale))
}

fn create_instance(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let who = ctx.active_function_name();
    let (zone, locale) = zone_and_locale(ctx, args, 0, 1, &who)?;
    new_object(ctx, zone, "gregorian", &locale)
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    state::reset_global(ctx);
    let who = ctx.active_function_name();
    // `IntlGregorianCalendar($timezone, $locale)` or the deprecated
    // `($year, $month, $day[, $hour, $minute, $second])`
    let numeric = args.len() >= 3 && args.iter().take(3).all(|v| matches!(v.deref().as_ref(), Value::Int(_) | Value::Float(_)));
    if numeric {
        ctx.emit_error(
            ErrLevel::Deprecated,
            &format!("Calling {who}() with more than 2 arguments is deprecated, use either IntlGregorianCalendar::createFromDate() or IntlGregorianCalendar::createFromDateTime() instead"),
        )?;
        let n = |i: usize| args.get(i).map_or(0, |v| v.deref().to_int());
        let zone = ZoneRef::from_php(date_bridge::default_zone(ctx));
        let locale = crate::state::default_locale(ctx);
        let mut st = CalState::new(zone, &locale, 0);
        st.set_fields(n(0), n(1), n(2), n(3), n(4), n(5));
        o.set_payload(Payload::Native(Box::new(st)));
        return Ok(Value::Null);
    }
    let (zone, locale) = zone_and_locale(ctx, args, 0, 1, &who)?;
    let now = date_bridge::now_seconds() * 1000;
    o.set_payload(Payload::Native(Box::new(CalState::new(zone, &locale, now))));
    Ok(Value::Null)
}

fn from_date_time(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let who = ctx.active_function_name();
    let locale = locale_arg(ctx, args, 1);
    let (ts, usec, zone) = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Object(o)) => date_bridge::datetime_instant(&o),
        Some(other) => {
            // a date string, read through php's parser
            let text = String::from_utf8_lossy(&other.to_php_bytes()).into_owned();
            let cid = ctx.lookup_class_or_error(b"DateTime")?;
            let obj = ctx.instantiate(cid);
            let mut cargs = [Value::string(text.as_bytes())];
            ctx.call_method(&obj, b"__construct", &mut cargs)?;
            date_bridge::datetime_instant(&obj)
        }
        None => return Err(Unwind::type_error(format!("{who}(): Argument #1 ($datetime) must be of type DateTime|string"))),
    };
    let cid = ctx.lookup_class_or_error(b"IntlGregorianCalendar")?;
    let obj = ctx.instantiate(cid);
    let millis = ts * 1000 + i64::from(usec / 1000);
    obj.set_payload(Payload::Native(Box::new(CalState::new(ZoneRef::from_php(zone), &locale, millis))));
    Ok(Value::Object(obj))
}

fn to_date_time(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let (millis, zone) = with_state(o, |s| (s.millis, s.zone.clone())).unwrap_or((0, ZoneRef::utc()));
    let php = date_bridge::parse_zone(&zone.id).unwrap_or(zone.php.clone());
    date_bridge::new_datetime(ctx, millis.div_euclid(1000), (millis.rem_euclid(1000) * 1000) as u32, php, false)
}

// ---- accessors --------------------------------------------------------------------------------

fn get_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    Ok(Value::string(b"gregorian"))
}

fn get_time(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Float(with_state(o, |s| s.millis).unwrap_or(0) as f64))
}

fn set_time(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let millis = args.first().map_or(0.0, |v| v.deref().to_float());
    with_state(o, |s| {
        s.err.reset();
        s.millis = millis as i64;
    });
    Ok(Value::Bool(true))
}

fn get_field(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let field = args.first().map_or(-1, |v| v.deref().to_int());
    if !(0..FIELD_COUNT).contains(&field) {
        return Err(Unwind::value_error(format!("{who}(): Argument #1 ($field) must be a valid field")));
    }
    match with_state(o, |s| s.get(field)).flatten() {
        Some(v) => Ok(Value::Int(v)),
        None => Ok(Value::Bool(false)),
    }
}

fn set_field(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let n = |i: usize| args.get(i).map(|v| v.deref().to_int());
    if args.len() > 2 {
        ctx.emit_error(
            ErrLevel::Deprecated,
            &format!("Calling {who}() with more than 2 arguments is deprecated, use either IntlCalendar::setDate() or IntlCalendar::setDateTime() instead"),
        )?;
        with_state(o, |s| {
            let f = s.fields();
            s.set_fields(
                n(0).unwrap_or(f.ext_year),
                n(1).unwrap_or(i64::from(f.month) - 1),
                n(2).unwrap_or(i64::from(f.day)),
                n(3).unwrap_or(i64::from(f.hour)),
                n(4).unwrap_or(i64::from(f.minute)),
                n(5).unwrap_or(i64::from(f.second)),
            );
        });
        return Ok(Value::Bool(true));
    }
    let field = n(0).unwrap_or(-1);
    let value = n(1).unwrap_or(0);
    with_state(o, |s| {
        s.err.reset();
        let f = s.fields();
        let cur = s.get(field).unwrap_or(0);
        match field {
            FIELD_YEAR | FIELD_EXTENDED_YEAR => s.set_fields(value, i64::from(f.month) - 1, i64::from(f.day), i64::from(f.hour), i64::from(f.minute), i64::from(f.second)),
            FIELD_MONTH => s.set_fields(f.ext_year, value, i64::from(f.day), i64::from(f.hour), i64::from(f.minute), i64::from(f.second)),
            FIELD_DATE => s.set_fields(f.ext_year, i64::from(f.month) - 1, value, i64::from(f.hour), i64::from(f.minute), i64::from(f.second)),
            FIELD_HOUR_OF_DAY | FIELD_HOUR => s.set_fields(f.ext_year, i64::from(f.month) - 1, i64::from(f.day), value, i64::from(f.minute), i64::from(f.second)),
            FIELD_MINUTE => s.set_fields(f.ext_year, i64::from(f.month) - 1, i64::from(f.day), i64::from(f.hour), value, i64::from(f.second)),
            FIELD_SECOND => s.set_fields(f.ext_year, i64::from(f.month) - 1, i64::from(f.day), i64::from(f.hour), i64::from(f.minute), value),
            FIELD_MILLISECOND => s.millis = s.millis - i64::from(f.millis) + value,
            _ => s.add(field, value - cur),
        }
    });
    Ok(Value::Bool(true))
}

fn add(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let field = args.first().map_or(-1, |v| v.deref().to_int());
    let amount = args.get(1).map_or(0, |v| v.deref().to_int());
    with_state(o, |s| {
        s.err.reset();
        s.add(field, amount);
    });
    Ok(Value::Bool(true))
}

fn roll(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let field = args.first().map_or(-1, |v| v.deref().to_int());
    let amount = args.get(1).map(|v| v.deref().into_owned()).map_or(0, |v| match v {
        Value::Bool(b) => {
            if b {
                1
            } else {
                -1
            }
        }
        other => other.to_int(),
    });
    with_state(o, |s| {
        s.err.reset();
        s.roll(field, amount);
    });
    Ok(Value::Bool(true))
}

fn get_time_zone(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let zone = with_state(o, |s| s.zone.clone()).unwrap_or_else(ZoneRef::utc);
    crate::tz::new_object(ctx, zone)
}

fn set_time_zone(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let who = ctx.active_function_name();
    let zone = crate::datefmt::zone_arg(ctx, args.first(), &who)?;
    with_state(o, |s| {
        s.err.reset();
        s.zone = zone;
    });
    Ok(Value::Bool(true))
}

fn get_locale(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let kind = args.first().map_or(0, |v| v.deref().to_int());
    let name = with_state(o, |s| if kind == 1 { s.valid.clone() } else { s.valid.split('_').next().unwrap_or("").to_string() }).unwrap_or_default();
    Ok(Value::string(name.as_bytes()))
}

fn get_first_day_of_week(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |s| i64::from(s.week.first_day) + 1).unwrap_or(1)))
}

fn get_minimal_days_in_first_week(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |s| i64::from(s.week.min_days)).unwrap_or(1)))
}

fn set_minimal_days_in_first_week(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let n = args.first().map_or(1, |v| v.deref().to_int());
    with_state(o, |s| s.week.min_days = n.clamp(1, 7) as u32);
    Ok(Value::Bool(true))
}

fn set_first_day_of_week(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let n = args.first().map_or(1, |v| v.deref().to_int());
    with_state(o, |s| s.week.first_day = (n.clamp(1, 7) - 1) as u32);
    Ok(Value::Bool(true))
}

fn is_lenient(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Bool(with_state(o, |s| s.lenient).unwrap_or(true)))
}

fn set_lenient(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let on = args.first().map(|v| v.deref().to_bool()).unwrap_or(true);
    with_state(o, |s| s.lenient = on);
    Ok(Value::Bool(true))
}

fn in_daylight_time(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Bool(with_state(o, |s| s.get(FIELD_DST_OFFSET).unwrap_or(0) != 0).unwrap_or(false)))
}

fn equals(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let mine = with_state(o, |s| s.millis).unwrap_or(0);
    let theirs = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Object(other)) => other.with_payload::<CalState, _>(|s| s.millis),
        _ => None,
    };
    Ok(Value::Bool(theirs == Some(mine)))
}

fn after(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let mine = with_state(o, |s| s.millis).unwrap_or(0);
    let theirs = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Object(other)) => other.with_payload::<CalState, _>(|s| s.millis),
        _ => None,
    };
    Ok(Value::Bool(theirs.is_some_and(|t| mine > t)))
}

fn before(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let mine = with_state(o, |s| s.millis).unwrap_or(0);
    let theirs = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Object(other)) => other.with_payload::<CalState, _>(|s| s.millis),
        _ => None,
    };
    Ok(Value::Bool(theirs.is_some_and(|t| mine < t)))
}

fn field_difference(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let target = args.first().map_or(0.0, |v| v.deref().to_float()) as i64;
    let field = args.get(1).map_or(-1, |v| v.deref().to_int());
    let n = with_state(o, |s| {
        // ICU moves the calendar towards the target one unit at a time
        let mut count: i64 = 0;
        let forward = target > s.millis;
        loop {
            let before = s.millis;
            s.add(field, if forward { 1 } else { -1 });
            if (forward && s.millis > target) || (!forward && s.millis < target) {
                s.millis = before;
                break;
            }
            if s.millis == before {
                break;
            }
            count += if forward { 1 } else { -1 };
            if count.abs() > 1_000_000 {
                break;
            }
        }
        count
    })
    .unwrap_or(0);
    Ok(Value::Int(n))
}

fn field_range(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value], which: u8) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let field = args.first().map_or(-1, |v| v.deref().to_int());
    let v = with_state(o, |s| {
        let f = s.fields();
        let (min, least_max, greatest_min, max, actual_min, actual_max) = match field {
            FIELD_ERA => (0, 1, 0, 1, 0, 1),
            FIELD_YEAR => (1, 140_742_049_819_618, 1, 144_683_225_803_776, 1, 144_683_225_803_776),
            FIELD_MONTH => (0, 11, 0, 11, 0, 11),
            FIELD_WEEK_OF_YEAR => (1, 52, 1, 53, 1, 52),
            FIELD_WEEK_OF_MONTH => (0, 4, 0, 6, 1, 5),
            FIELD_DATE => (1, 28, 1, 31, 1, i64::from(civil::days_in_month(f.ext_year, f.month, f.julian))),
            FIELD_DAY_OF_YEAR => (1, 365, 1, 366, 1, if civil::is_leap(f.ext_year, f.julian) { 366 } else { 365 }),
            FIELD_DAY_OF_WEEK => (1, 7, 1, 7, 1, 7),
            FIELD_DAY_OF_WEEK_IN_MONTH => (-1, 4, -1, 5, 1, (i64::from(civil::days_in_month(f.ext_year, f.month, f.julian)) + 6) / 7),
            FIELD_AM_PM => (0, 1, 0, 1, 0, 1),
            FIELD_HOUR => (0, 11, 0, 11, 0, 11),
            FIELD_HOUR_OF_DAY => (0, 23, 0, 23, 0, 23),
            FIELD_MINUTE | FIELD_SECOND => (0, 59, 0, 59, 0, 59),
            FIELD_MILLISECOND => (0, 999, 0, 999, 0, 999),
            FIELD_MILLISECONDS_IN_DAY => (0, 86_399_999, 0, 86_399_999, 0, 86_399_999),
            _ => (0, 0, 0, 0, 0, 0),
        };
        match which {
            0 => min,
            1 => max,
            2 => least_max,
            3 => greatest_min,
            4 => actual_min,
            _ => actual_max,
        }
    })
    .unwrap_or(0);
    Ok(Value::Int(v))
}

macro_rules! range_method {
    ($name:ident, $which:expr) => {
        fn $name(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
            field_range(ctx, o, args, $which)
        }
    };
}

range_method!(get_minimum, 0);
range_method!(get_maximum, 1);
range_method!(get_least_maximum, 2);
range_method!(get_greatest_minimum, 3);
range_method!(get_actual_minimum, 4);
range_method!(get_actual_maximum, 5);

fn is_set(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let field = args.first().map_or(-1, |v| v.deref().to_int());
    Ok(Value::Bool(with_state(o, |s| s.get(field).is_some()).unwrap_or(false)))
}

fn clear(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    if opt_arg(args, 0).is_none() {
        with_state(o, |s| s.millis = 0);
    }
    Ok(Value::Bool(true))
}

fn get_day_of_week_type(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let day = args.first().map_or(1, |v| v.deref().to_int());
    // 0 = weekday, 1 = weekend; CLDR's weekend is Saturday and Sunday in
    // the locales rphp carries week data for
    let weekend = matches!(day, 1 | 7);
    let _ = o;
    Ok(Value::Int(i64::from(weekend)))
}

fn is_weekend(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    let dow = with_state(o, |s| s.get(FIELD_DAY_OF_WEEK).unwrap_or(1)).unwrap_or(1);
    Ok(Value::Bool(matches!(dow, 1 | 7)))
}

fn get_weekend_transition(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    Ok(Value::Int(86_400_000))
}

fn get_error_code(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::Int(with_state(o, |s| s.err.code).unwrap_or(0)))
}

fn get_error_message(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = state_of(ctx, this(o)?)?;
    Ok(Value::string(with_state(o, |s| s.err.message()).unwrap_or_default().as_bytes()))
}

fn get_available_locales(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let mut a = Array::new();
    for (name, _) in crate::data::datefmt::LOCALES {
        a.push(Value::string(name.as_bytes()));
    }
    Ok(Value::Array(a))
}

fn get_keyword_values_for_locale(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    a.push(Value::string(b"gregorian"));
    crate::iterator::new_object(ctx, a)
}

fn get_repeated_wall_time_option(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    Ok(Value::Int(0))
}

fn get_skipped_wall_time_option(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    Ok(Value::Int(0))
}

// ---- IntlGregorianCalendar's own methods ------------------------------------------------------

fn get_gregorian_change(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    Ok(Value::Float(civil::CUTOVER_DAYS as f64 * 86_400_000.0))
}

fn set_gregorian_change(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    Ok(Value::Bool(true))
}

fn is_leap_year(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let _ = state_of(ctx, this(o)?)?;
    let year = args.first().map_or(1970, |v| v.deref().to_int());
    Ok(Value::Bool(civil::is_leap(year, false)))
}

static METHODS: &[MethodImpl] = &[
    ("createInstance", create_instance),
    ("fromDateTime", from_date_time),
    ("toDateTime", to_date_time),
    ("getType", get_type),
    ("getTime", get_time),
    ("setTime", set_time),
    ("get", get_field),
    ("set", set_field),
    ("add", add),
    ("roll", roll),
    ("getTimeZone", get_time_zone),
    ("setTimeZone", set_time_zone),
    ("getLocale", get_locale),
    ("getFirstDayOfWeek", get_first_day_of_week),
    ("setFirstDayOfWeek", set_first_day_of_week),
    ("getMinimalDaysInFirstWeek", get_minimal_days_in_first_week),
    ("setMinimalDaysInFirstWeek", set_minimal_days_in_first_week),
    ("isLenient", is_lenient),
    ("setLenient", set_lenient),
    ("inDaylightTime", in_daylight_time),
    ("equals", equals),
    ("after", after),
    ("before", before),
    ("fieldDifference", field_difference),
    ("getMinimum", get_minimum),
    ("getMaximum", get_maximum),
    ("getLeastMaximum", get_least_maximum),
    ("getGreatestMinimum", get_greatest_minimum),
    ("getActualMinimum", get_actual_minimum),
    ("getActualMaximum", get_actual_maximum),
    ("isSet", is_set),
    ("clear", clear),
    ("getDayOfWeekType", get_day_of_week_type),
    ("isWeekend", is_weekend),
    ("getWeekendTransition", get_weekend_transition),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
    ("getAvailableLocales", get_available_locales),
    ("getKeywordValuesForLocale", get_keyword_values_for_locale),
    ("getRepeatedWallTimeOption", get_repeated_wall_time_option),
    ("getSkippedWallTimeOption", get_skipped_wall_time_option),
];

static GREGORIAN_METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("getGregorianChange", get_gregorian_change),
    ("setGregorianChange", set_gregorian_change),
    ("isLeapYear", is_leap_year),
];

macro_rules! as_function {
    ($name:ident, $method:ident) => {
        fn $name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
            let obj = match args.first().map(|v| v.deref().into_owned()) {
                Some(Value::Object(o)) => o,
                _ => return Err(Unwind::type_error("Argument #1 ($calendar) must be of type IntlCalendar")),
            };
            let rest = &mut args[1..];
            $method(ctx, Some(&obj), rest)
        }
    };
}

as_function!(f_to_date_time, to_date_time);
as_function!(f_get_type, get_type);
as_function!(f_get_time, get_time);
as_function!(f_set_time, set_time);
as_function!(f_get_field, get_field);
as_function!(f_set_field, set_field);
as_function!(f_add, add);
as_function!(f_roll, roll);
as_function!(f_get_time_zone, get_time_zone);
as_function!(f_set_time_zone, set_time_zone);
as_function!(f_get_locale, get_locale);
as_function!(f_get_first_day_of_week, get_first_day_of_week);
as_function!(f_set_first_day_of_week, set_first_day_of_week);
as_function!(f_get_minimal_days_in_first_week, get_minimal_days_in_first_week);
as_function!(f_set_minimal_days_in_first_week, set_minimal_days_in_first_week);
as_function!(f_is_lenient, is_lenient);
as_function!(f_set_lenient, set_lenient);
as_function!(f_in_daylight_time, in_daylight_time);
as_function!(f_equals, equals);
as_function!(f_after, after);
as_function!(f_before, before);
as_function!(f_field_difference, field_difference);
as_function!(f_get_minimum, get_minimum);
as_function!(f_get_maximum, get_maximum);
as_function!(f_get_least_maximum, get_least_maximum);
as_function!(f_get_greatest_minimum, get_greatest_minimum);
as_function!(f_get_actual_minimum, get_actual_minimum);
as_function!(f_get_actual_maximum, get_actual_maximum);
as_function!(f_is_set, is_set);
as_function!(f_clear, clear);
as_function!(f_get_day_of_week_type, get_day_of_week_type);
as_function!(f_is_weekend, is_weekend);
as_function!(f_get_weekend_transition, get_weekend_transition);
as_function!(f_get_error_code, get_error_code);
as_function!(f_get_error_message, get_error_message);
as_function!(f_get_repeated_wall_time_option, get_repeated_wall_time_option);
as_function!(f_get_skipped_wall_time_option, get_skipped_wall_time_option);

fn f_create_instance(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create_instance(ctx, None, args)
}

fn f_from_date_time(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    from_date_time(ctx, None, args)
}

fn f_get_available_locales(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    get_available_locales(ctx, None, args)
}

fn f_get_keyword_values_for_locale(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    get_keyword_values_for_locale(ctx, None, args)
}

static FUNCTIONS: &[FnImpl] = &[
    ("intlcal_create_instance", f_create_instance),
    ("intlcal_from_date_time", f_from_date_time),
    ("intlcal_to_date_time", f_to_date_time),
    ("intlcal_get_type", f_get_type),
    ("intlcal_get_time", f_get_time),
    ("intlcal_set_time", f_set_time),
    ("intlcal_get", f_get_field),
    ("intlcal_set", f_set_field),
    ("intlcal_add", f_add),
    ("intlcal_roll", f_roll),
    ("intlcal_get_time_zone", f_get_time_zone),
    ("intlcal_set_time_zone", f_set_time_zone),
    ("intlcal_get_locale", f_get_locale),
    ("intlcal_get_first_day_of_week", f_get_first_day_of_week),
    ("intlcal_set_first_day_of_week", f_set_first_day_of_week),
    ("intlcal_get_minimal_days_in_first_week", f_get_minimal_days_in_first_week),
    ("intlcal_set_minimal_days_in_first_week", f_set_minimal_days_in_first_week),
    ("intlcal_is_lenient", f_is_lenient),
    ("intlcal_set_lenient", f_set_lenient),
    ("intlcal_in_daylight_time", f_in_daylight_time),
    ("intlcal_equals", f_equals),
    ("intlcal_after", f_after),
    ("intlcal_before", f_before),
    ("intlcal_field_difference", f_field_difference),
    ("intlcal_get_minimum", f_get_minimum),
    ("intlcal_get_maximum", f_get_maximum),
    ("intlcal_get_least_maximum", f_get_least_maximum),
    ("intlcal_get_greatest_minimum", f_get_greatest_minimum),
    ("intlcal_get_actual_minimum", f_get_actual_minimum),
    ("intlcal_get_actual_maximum", f_get_actual_maximum),
    ("intlcal_is_set", f_is_set),
    ("intlcal_clear", f_clear),
    ("intlcal_get_day_of_week_type", f_get_day_of_week_type),
    ("intlcal_is_weekend", f_is_weekend),
    ("intlcal_get_weekend_transition", f_get_weekend_transition),
    ("intlcal_get_error_code", f_get_error_code),
    ("intlcal_get_error_message", f_get_error_message),
    ("intlcal_get_available_locales", f_get_available_locales),
    ("intlcal_get_keyword_values_for_locale", f_get_keyword_values_for_locale),
    ("intlcal_get_repeated_wall_time_option", f_get_repeated_wall_time_option),
    ("intlcal_get_skipped_wall_time_option", f_get_skipped_wall_time_option),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLCALENDAR, METHODS, |b| b.payload_clone(payload_clone));
    register_class(r, &generated::classes::INTLGREGORIANCALENDAR, GREGORIAN_METHODS, |b| b.payload_clone(payload_clone));
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
