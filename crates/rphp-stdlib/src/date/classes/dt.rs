//! `DateTimeInterface`, `DateTime` and `DateTimeImmutable`.
//!
//! The two concrete classes share every method body; they differ only in
//! what [`answer`] does with the new state — write it back into the
//! receiver and return `$this`, or seed a fresh instance of the receiver's
//! own class. That is the whole mutable/immutable distinction, and keeping
//! it in one place is why `DateTimeImmutable::setTime()` cannot forget to
//! be immutable.
//!
//! Every calendar operation is civil: the instant is broken down in the
//! object's zone, the fields are moved, normalized by
//! [`parse::normalize`](super::super::parse::normalize), and turned back
//! into an instant exactly once. Across a fold the object's *current*
//! offset is offered to [`Tz::resolve`] as the preference, which is php's
//! rule for `modify()` and the reason it can differ from a fresh
//! `strtotime()` of the same wall clock.

use rphp_runtime::{nm, ClassBuilder, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, ArrayKey, Object, Value};

use super::super::civil;
use super::super::format;
use super::super::parse::{self, Civil, Parsed};
use super::super::tz::Tz;
use super::zone::{new_zone, snm};
use super::{
    date_throw, default_tz, dt_state, int_arg, malformed_string, now, obj_arg, put_dt, str_arg,
    this, DtState,
};

/// Whether a receiver is one of the immutable classes.
fn is_immutable(ctx: &Ctx, o: &Object) -> bool {
    ctx.class_by_name(b"DateTimeImmutable")
        .is_some_and(|cid| ctx.object_instanceof(o, cid))
}

/// `DateTime::method` or `DateTimeImmutable::method` — php names the
/// receiver's class in every coercion diagnostic.
fn who(ctx: &Ctx, o: &Object, method: &str) -> String {
    let class = if is_immutable(ctx, o) {
        "DateTimeImmutable"
    } else {
        "DateTime"
    };
    format!("{class}::{method}")
}

/// What every mutating method returns: `$this` with the new state for
/// `DateTime`, a fresh instance of the receiver's class for
/// `DateTimeImmutable`.
fn answer(ctx: &mut Ctx, o: &Object, st: DtState) -> NativeResult {
    if is_immutable(ctx, o) {
        let fresh = ctx.instantiate(o.class_id());
        put_dt(&fresh, st);
        return Ok(Value::Object(fresh));
    }
    put_dt(o, st);
    Ok(Value::Object(o.clone()))
}

// ---- the civil pipeline ----------------------------------------------------------

/// The object's instant broken down in its own zone.
fn fields(st: &DtState) -> Civil {
    let r = format::render(&st.tz, st.ts, st.usec);
    Civil {
        y: r.year,
        mo: r.month as i64,
        d: r.day as i64,
        h: r.hour as i64,
        mi: r.minute as i64,
        s: r.second as i64,
        us: st.usec as i64,
    }
}

/// Turn broken-down fields back into an instant in `tz`, offering the
/// offset the object already carried as the fold preference.
fn resolve_in(tz: &Tz, mut c: Civil, prefer: Option<i32>) -> (i64, u32) {
    parse::normalize(&mut c);
    (tz.resolve(parse::sse_of(&c), prefer), c.us as u32)
}

/// Apply a scanned string to an existing instant — the body `modify()` and
/// the two constructors share. `prefer` is the offset a fold resolves to.
fn apply_parse(p: &Parsed, st: &DtState, base_us: u32, prefer: Option<i32>) -> DtState {
    let mut c = parse::fill_and_adjust(p, st.ts, &st.tz);
    // `fill_and_adjust` starts the microseconds at zero, because
    // `strtotime()` has none to carry. A date object does: they survive
    // every relative move, and only a string that reset the clock
    // (a bare date) clears them.
    if p.us.is_none() && !(p.have_date && p.have_time == 0) {
        c.us += base_us as i64;
        parse::normalize(&mut c);
    }
    let tz = p.zone.clone().unwrap_or_else(|| st.tz.clone());
    let (ts, usec) = resolve_in(&tz, c, prefer);
    DtState { ts, usec, tz }
}

/// A `Parsed` that carries nothing but the relative amounts of an interval,
/// for `add()` / `sub()`.
fn relative_only(mut p: Parsed, negate: bool) -> Parsed {
    p.y = None;
    p.m = None;
    p.d = None;
    p.h = None;
    p.i = None;
    p.s = None;
    p.us = None;
    p.have_date = false;
    p.have_time = 0;
    p.have_zone = false;
    p.zone = None;
    p.have_relative = true;
    p.errors.clear();
    p.warnings.clear();
    // php copies an interval's *relative* struct and nothing else, and
    // `first day of` is not part of it: `add(createFromDateString('first
    // day of next month'))` moves a month and keeps the day of the month,
    // where `modify()` of the same string lands on the first.
    p.rel.first_last_day_of = 0;
    if negate {
        p.rel.y = -p.rel.y;
        p.rel.m = -p.rel.m;
        p.rel.d = -p.rel.d;
        p.rel.h = -p.rel.h;
        p.rel.i = -p.rel.i;
        p.rel.s = -p.rel.s;
        p.rel.us = -p.rel.us;
        p.rel.special_weekday = p.rel.special_weekday.map(|n| -n);
    }
    p
}

// ---- construction -------------------------------------------------------------

/// The shared body of `DateTime::__construct` and
/// `DateTimeImmutable::__construct`.
fn construct_body(ctx: &mut Ctx, o: &Object, who: &str, args: &mut [Value]) -> NativeResult {
    let text = match args.first() {
        Some(v) => str_arg(ctx, who, 1, "datetime", v)?,
        None => b"now".to_vec(),
    };
    let arg_tz = obj_arg(ctx, who, 2, "timezone", "DateTimeZone", true, args.get(1))?;
    let arg_tz = match arg_tz {
        Some(z) => Some(super::zone::zone_of(ctx, &z, who)?),
        None => None,
    };
    let st = parse_into(ctx, &text, arg_tz, None)?;
    put_dt(o, st);
    Ok(Value::Null)
}

/// Scan `text` and resolve it against the wall clock, the way a constructor
/// does. `arg_tz` is used only when the string itself named no zone.
fn parse_into(
    ctx: &mut Ctx,
    text: &[u8],
    arg_tz: Option<Tz>,
    who: Option<&str>,
) -> Result<DtState, Unwind> {
    let p = parse::parse(text);
    record_errors(&p);
    if let Some(err) = p.errors.first().copied() {
        return Err(malformed_string(ctx, who, text, err));
    }
    let tz = p.zone.clone().or(arg_tz).unwrap_or_else(|| default_tz(ctx));
    let (base_ts, base_us) = now();
    let base = DtState {
        ts: base_ts,
        usec: base_us,
        tz,
    };
    Ok(apply_parse(&p, &base, base_us, None))
}

/// Feed a scan's diagnostics to `getLastErrors()`.
fn record_errors(p: &Parsed) {
    super::set_last_errors(
        p.warnings
            .iter()
            .map(|(i, m)| (*i, m.to_string()))
            .collect(),
        p.errors.iter().map(|(i, m)| (*i, m.to_string())).collect(),
    );
}

/// `DateTime::__construct(string $datetime = "now", ?DateTimeZone $timezone = null)`
fn construct_mutable(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    construct_body(ctx, o, "DateTime::__construct", args)
}

/// `DateTimeImmutable::__construct(string $datetime = "now", ?DateTimeZone $timezone = null)`
fn construct_immutable(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    construct_body(ctx, o, "DateTimeImmutable::__construct", args)
}

/// A fresh instance of `class` carrying `st`, without running a
/// constructor — what every static factory answers.
fn new_dt(ctx: &mut Ctx, class: &[u8], st: DtState) -> NativeResult {
    let cid = ctx.class_by_name(class).ok_or_else(|| {
        Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(class)
        ))
    })?;
    let o = ctx.instantiate(cid);
    put_dt(&o, st);
    Ok(Value::Object(o))
}

// ---- reading ----------------------------------------------------------------------

/// `DateTimeInterface::format(string $format): string`
pub(crate) fn format(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "format");
    let fmt = str_arg(ctx, &who, 1, "format", &args[0])?;
    let st = dt_state(o);
    let r = format::render(&st.tz, st.ts, st.usec);
    Ok(Value::string(&format::format(&r, &fmt)))
}

/// `DateTimeInterface::getTimestamp(): int`
pub(crate) fn get_timestamp(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(dt_state(this(o)?).ts))
}

/// `DateTimeInterface::getOffset(): int`
pub(crate) fn get_offset(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = dt_state(this(o)?);
    Ok(Value::Int(st.tz.offset_at(st.ts) as i64))
}

/// `DateTimeInterface::getMicrosecond(): int` (8.4)
fn get_microsecond(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(dt_state(this(o)?).usec)))
}

/// `DateTimeInterface::getTimezone(): DateTimeZone|false`
pub(crate) fn get_timezone(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = dt_state(this(o)?);
    Ok(Value::Object(new_zone(ctx, st.tz)?))
}

/// `DateTime::getLastErrors(): array|false`
fn get_last_errors(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(super::last_errors())
}

// ---- mutation ------------------------------------------------------------------------

/// `DateTime::modify(string $modifier): static`
pub(crate) fn modify(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "modify");
    let text = str_arg(ctx, &who, 1, "modifier", &args[0])?;
    let st = dt_state(o);
    let p = parse::parse(&text);
    record_errors(&p);
    if let Some(err) = p.errors.first().copied() {
        return Err(malformed_string(ctx, Some(who.as_str()), &text, err));
    }
    let prefer = st.tz.offset_at(st.ts);
    let next = apply_parse(&p, &st, st.usec, Some(prefer));
    answer(ctx, o, next)
}

/// The instant `st` moves to when `iv` is added to (or subtracted from) it.
///
/// php has *two* additions and the difference is observable. `add()` on a
/// parsed duration moves the calendar civilly but the clock absolutely, so
/// `PT24H` across a spring-forward really is 86 400 seconds (13:00, not
/// 12:00) while `P1D` is the same wall clock the next day.
/// `DatePeriod`'s step and `add()` on a `createFromDateString` interval —
/// which php implements by re-running `modify()` — are civil throughout,
/// so `PT24H` lands on 12:00 there. `wall` picks between the two.
pub(crate) fn advance(
    ctx: &mut Ctx,
    st: &DtState,
    iv: &Object,
    negate: bool,
    wall: bool,
) -> Result<DtState, Unwind> {
    let relative = super::interval::relative_of(ctx, iv)?;
    let prefer = st.tz.offset_at(st.ts);
    if wall || super::interval::is_from_string(iv) {
        let p = relative_only(relative, negate);
        return Ok(apply_parse(&p, st, st.usec, Some(prefer)));
    }
    let mut p = relative_only(relative, negate);
    let seconds = p.rel.h * 3600 + p.rel.i * 60 + p.rel.s;
    let micros = p.rel.us;
    p.rel.h = 0;
    p.rel.i = 0;
    p.rel.s = 0;
    p.rel.us = 0;
    let mid = apply_parse(&p, st, st.usec, Some(prefer));
    let total = i64::from(mid.usec) + micros;
    Ok(DtState {
        ts: mid.ts + seconds + total.div_euclid(1_000_000),
        usec: total.rem_euclid(1_000_000) as u32,
        tz: mid.tz,
    })
}

/// The shared body of `add()` and `sub()`.
fn shift(ctx: &mut Ctx, o: &Object, iv: &Object, negate: bool) -> NativeResult {
    let st = dt_state(o);
    let next = advance(ctx, &st, iv, negate, false)?;
    answer(ctx, o, next)
}

/// `DateTime::add(DateInterval $interval): static`
pub(crate) fn add(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "add");
    let iv = require_interval(ctx, &who, args.first())?;
    shift(ctx, o, &iv, false)
}

/// `DateTime::sub(DateInterval $interval): static`
pub(crate) fn sub(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "sub");
    let iv = require_interval(ctx, &who, args.first())?;
    shift(ctx, o, &iv, true)
}

/// A non-nullable `DateInterval` parameter.
fn require_interval(ctx: &mut Ctx, who: &str, v: Option<&Value>) -> Result<Object, Unwind> {
    obj_arg(ctx, who, 1, "interval", "DateInterval", false, v)?.ok_or_else(|| {
        Unwind::type_error(format!(
            "{who}(): Argument #1 ($interval) must be of type DateInterval, null given"
        ))
    })
}

/// `DateTime::setTimestamp(int $timestamp): static`
pub(crate) fn set_timestamp(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "setTimestamp");
    let ts = int_arg(ctx, &who, 1, "timestamp", &args[0])?;
    let st = dt_state(o);
    answer(
        ctx,
        o,
        DtState {
            ts,
            usec: 0,
            tz: st.tz,
        },
    )
}

/// `DateTime::setTimezone(DateTimeZone $timezone): static`
pub(crate) fn set_timezone(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "setTimezone");
    let z = obj_arg(
        ctx,
        &who,
        1,
        "timezone",
        "DateTimeZone",
        false,
        args.first(),
    )?
    .ok_or_else(|| {
        Unwind::type_error(format!(
            "{who}(): Argument #1 ($timezone) must be of type DateTimeZone, null given"
        ))
    })?;
    let tz = super::zone::zone_of(ctx, &z, &who)?;
    let st = dt_state(o);
    answer(
        ctx,
        o,
        DtState {
            ts: st.ts,
            usec: st.usec,
            tz,
        },
    )
}

/// `DateTime::setMicrosecond(int $microsecond): static` (8.4)
fn set_microsecond(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "setMicrosecond");
    let us = int_arg(ctx, &who, 1, "microsecond", &args[0])?;
    if !(0..=999_999).contains(&us) {
        return Err(date_throw(
            ctx,
            "DateRangeError",
            format!("{who}(): Argument #1 ($microsecond) must be between 0 and 999999, {us} given"),
        ));
    }
    let st = dt_state(o);
    answer(
        ctx,
        o,
        DtState {
            ts: st.ts,
            usec: us as u32,
            tz: st.tz,
        },
    )
}

/// `DateTime::setDate(int $year, int $month, int $day): static`
pub(crate) fn set_date(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "setDate");
    let y = int_arg(ctx, &who, 1, "year", &args[0])?;
    let m = int_arg(ctx, &who, 2, "month", &args[1])?;
    let d = int_arg(ctx, &who, 3, "day", &args[2])?;
    let st = dt_state(o);
    let mut c = fields(&st);
    c.y = y;
    c.mo = m;
    c.d = d;
    let prefer = st.tz.offset_at(st.ts);
    let (ts, usec) = resolve_in(&st.tz, c, Some(prefer));
    answer(
        ctx,
        o,
        DtState {
            ts,
            usec,
            tz: st.tz,
        },
    )
}

/// `DateTime::setISODate(int $year, int $week, int $dayOfWeek = 1): static`
pub(crate) fn set_iso_date(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "setISODate");
    let y = int_arg(ctx, &who, 1, "year", &args[0])?;
    let w = int_arg(ctx, &who, 2, "week", &args[1])?;
    let dow = match args.get(2) {
        Some(v) => int_arg(ctx, &who, 3, "dayOfWeek", v)?,
        None => 1,
    };
    let st = dt_state(o);
    // 4 January is in ISO week 1 of every year; stepping back to its Monday
    // gives the week-1 anchor php counts from.
    let jan4 = civil::days_from_civil(y, 1, 4);
    let iso_dow = (jan4 + 3).rem_euclid(7) + 1;
    let anchor = jan4 - (iso_dow - 1);
    let (ny, nm, nd) = civil::civil_from_days(anchor + (w - 1) * 7 + (dow - 1));
    let mut c = fields(&st);
    c.y = ny;
    c.mo = nm as i64;
    c.d = nd as i64;
    let prefer = st.tz.offset_at(st.ts);
    let (ts, usec) = resolve_in(&st.tz, c, Some(prefer));
    answer(
        ctx,
        o,
        DtState {
            ts,
            usec,
            tz: st.tz,
        },
    )
}

/// `DateTime::setTime(int $hour, int $minute, int $second = 0, int $microsecond = 0): static`
pub(crate) fn set_time(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "setTime");
    let h = int_arg(ctx, &who, 1, "hour", &args[0])?;
    let mi = int_arg(ctx, &who, 2, "minute", &args[1])?;
    let s = match args.get(2) {
        Some(v) => int_arg(ctx, &who, 3, "second", v)?,
        None => 0,
    };
    let us = match args.get(3) {
        Some(v) => int_arg(ctx, &who, 4, "microsecond", v)?,
        None => 0,
    };
    let st = dt_state(o);
    let mut c = fields(&st);
    c.h = h;
    c.mi = mi;
    c.s = s;
    c.us = us;
    let prefer = st.tz.offset_at(st.ts);
    let (ts, usec) = resolve_in(&st.tz, c, Some(prefer));
    answer(
        ctx,
        o,
        DtState {
            ts,
            usec,
            tz: st.tz,
        },
    )
}

// ---- diff -----------------------------------------------------------------------------

/// Whether two zones are the one php compares by `tz_info` identity: both
/// database identifiers naming the same zone. The spelling is compared
/// case-insensitively because php's lookup is.
fn same_zone(a: &Tz, b: &Tz) -> bool {
    match (a, b) {
        (Tz::Id(x), Tz::Id(y)) => x.eq_ignore_ascii_case(y),
        _ => false,
    }
}

/// timelib's `do_range_limit`: carry `a` into `b` so that `a` lands in
/// `start..end`.
fn range_limit(start: i64, end: i64, adj: i64, a: &mut i64, b: &mut i64) {
    if *a < start {
        let k = (-*a - 1) / adj + 1;
        *b -= k;
        *a += adj * k;
    }
    if *a >= end {
        let k = *a / adj;
        *b += k;
        *a -= adj * k;
    }
}

/// The instant `y` years, `m` months and `d` days after `st`, in `st`'s own
/// zone — the anchor the time part of a `diff()` is measured from.
fn add_ymd(st: &DtState, y: i64, m: i64, d: i64) -> i64 {
    let prefer = st.tz.offset_at(st.ts);
    let mut c = fields(st);
    c.y += y;
    c.mo += m;
    parse::normalize(&mut c);
    c.d += d;
    resolve_in(&st.tz, c, Some(prefer)).0
}

/// What `diff()` produces, in `DateInterval`'s field order.
pub(crate) struct Diff {
    pub(crate) y: i64,
    pub(crate) m: i64,
    pub(crate) d: i64,
    pub(crate) h: i64,
    pub(crate) i: i64,
    pub(crate) s: i64,
    pub(crate) us: i64,
    pub(crate) days: i64,
    pub(crate) invert: i64,
}

/// php's `DateTimeInterface::diff()` (see this module's header for the
/// model and how it was fitted).
pub(crate) fn difference(a: &DtState, b: &DtState) -> Diff {
    let (one, two, invert) = if (a.ts, a.usec) > (b.ts, b.usec) {
        (b, a, 1)
    } else {
        (a, b, 0)
    };
    let of = format::render(&one.tz, one.ts, one.usec);
    let tf = format::render(&two.tz, two.ts, two.usec);
    let delta = (tf.offset - of.offset) as i64;
    // timelib corrects a DST change only inside one zone; between two zones
    // the constant frame shift is taken out of the fields instead.
    let corr = if same_zone(&one.tz, &two.tz) {
        delta
    } else {
        0
    };
    let cross = delta - corr;

    let mut y = tf.year - of.year;
    let mut m = tf.month as i64 - of.month as i64;
    let mut d = tf.day as i64 - of.day as i64;
    let mut h = tf.hour as i64 - of.hour as i64 - cross / 3600;
    let mut i = tf.minute as i64 - of.minute as i64 - (cross % 3600) / 60;
    let mut s = tf.second as i64 - of.second as i64;
    let mut us = i64::from(two.usec) - i64::from(one.usec);
    range_limit(0, 1_000_000, 1_000_000, &mut us, &mut s);
    range_limit(0, 60, 60, &mut s, &mut i);
    range_limit(0, 60, 60, &mut i, &mut h);
    range_limit(0, 24, 24, &mut h, &mut d);
    range_limit(0, 12, 12, &mut m, &mut y);

    // timelib's `do_range_limit_days_relative`: a negative day count borrows
    // whole months, counted backwards from the later end or forwards from
    // the earlier one depending on which way the interval reads.
    let base = if invert == 1 { &of } else { &tf };
    let (mut by, mut bm) = (base.year, base.month as i64);
    if invert == 0 {
        while d < 0 {
            bm -= 1;
            if bm < 1 {
                bm += 12;
                by -= 1;
            }
            d += i64::from(civil::days_in_month(by, bm as u32));
            m -= 1;
        }
    } else {
        while d < 0 {
            d += i64::from(civil::days_in_month(by, bm as u32));
            m -= 1;
            bm += 1;
            if bm > 12 {
                bm -= 12;
                by += 1;
            }
        }
    }
    range_limit(0, 12, 12, &mut m, &mut y);

    // Within one zone the calendar part may swallow a short or a long day.
    // php corrects the clock part by the offset the anchor actually lands
    // on and does *not* re-normalize, which is how `h=6, i=-29` happens.
    if corr != 0 && invert == 0 {
        let anchor = add_ymd(one, y, m, d);
        let c = (tf.offset - one.tz.offset_at(anchor)) as i64;
        h -= c / 3600;
        i -= (c % 3600) / 60;
    }
    Diff {
        y,
        m,
        d,
        h,
        i,
        s,
        us,
        // C integer division, so a span shorter than the correction
        // truncates towards zero rather than down.
        days: (two.ts - one.ts + corr) / 86_400,
        invert,
    }
}

/// `DateTimeInterface::diff(DateTimeInterface $targetObject, bool $absolute = false): DateInterval`
pub(crate) fn diff(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = who(ctx, o, "diff");
    let target = obj_arg(
        ctx,
        &who,
        1,
        "targetObject",
        "DateTimeInterface",
        false,
        args.first(),
    )?
    .ok_or_else(|| {
        Unwind::type_error(format!(
            "{who}(): Argument #1 ($targetObject) must be of type DateTimeInterface, null given"
        ))
    })?;
    let absolute = args.get(1).is_some_and(Value::to_bool);
    let mut r = difference(&dt_state(o), &dt_state(&target));
    if absolute {
        r.invert = 0;
    }
    super::interval::from_diff(ctx, &r)
}

// ---- static factories -------------------------------------------------------------------

/// `DateTime::createFromFormat(string $format, string $datetime, ?DateTimeZone $timezone = null): static|false`
fn create_from_format_mutable(ctx: &mut Ctx, _: Option<&Object>, a: &mut [Value]) -> NativeResult {
    create_from_format(ctx, b"DateTime", "DateTime::createFromFormat", a)
}

/// `DateTimeImmutable::createFromFormat(…): static|false`
fn create_from_format_immutable(
    ctx: &mut Ctx,
    _: Option<&Object>,
    a: &mut [Value],
) -> NativeResult {
    create_from_format(
        ctx,
        b"DateTimeImmutable",
        "DateTimeImmutable::createFromFormat",
        a,
    )
}

/// The shared body of both `createFromFormat`s.
pub(crate) fn create_from_format(
    ctx: &mut Ctx,
    class: &[u8],
    who: &str,
    args: &mut [Value],
) -> NativeResult {
    let fmt = str_arg(ctx, who, 1, "format", &args[0])?;
    let text = str_arg(ctx, who, 2, "datetime", &args[1])?;
    let arg_tz = obj_arg(ctx, who, 3, "timezone", "DateTimeZone", true, args.get(2))?;
    let arg_tz = match arg_tz {
        Some(z) => Some(super::zone::zone_of(ctx, &z, who)?),
        None => None,
    };
    let scan = super::fromformat::scan(&fmt, &text);
    super::set_last_errors(scan.warnings.clone(), scan.errors.clone());
    if !scan.errors.is_empty() {
        return Ok(Value::Bool(false));
    }
    // php fills the unnamed fields from `now` in the argument's (or the
    // default) zone — its wall clock, even when the string names another
    // zone, which then only labels the result (`apply_parse`).
    let tz = arg_tz.unwrap_or_else(|| default_tz(ctx));
    let (base_ts, base_us) = now();
    let base = DtState {
        ts: base_ts,
        usec: base_us,
        tz,
    };
    let st = apply_parse(&scan.parsed, &base, base_us, None);
    new_dt(ctx, class, st)
}

/// `DateTime::createFromTimestamp(int|float $timestamp): static` (8.4). The
/// zone php gives the result is the fixed offset `+00:00`, not `UTC`.
fn create_from_timestamp_mutable(
    ctx: &mut Ctx,
    _: Option<&Object>,
    a: &mut [Value],
) -> NativeResult {
    create_from_timestamp(ctx, b"DateTime", "DateTime::createFromTimestamp", a)
}

/// `DateTimeImmutable::createFromTimestamp(int|float $timestamp): static`
fn create_from_timestamp_immutable(
    ctx: &mut Ctx,
    _: Option<&Object>,
    a: &mut [Value],
) -> NativeResult {
    create_from_timestamp(
        ctx,
        b"DateTimeImmutable",
        "DateTimeImmutable::createFromTimestamp",
        a,
    )
}

/// The shared body of both `createFromTimestamp`s.
fn create_from_timestamp(
    ctx: &mut Ctx,
    class: &[u8],
    who: &str,
    args: &mut [Value],
) -> NativeResult {
    let (ts, usec) = match &*args[0].deref() {
        Value::Float(f) => {
            let f = *f;
            let secs = f.floor();
            if !f.is_finite() || secs < -9.223_372_036_854_776e18 || secs > 9.223_372_036_854_776e18
            {
                return Err(date_throw(
                    ctx,
                    "DateRangeError",
                    format!(
                        "{who}(): Argument #1 ($timestamp) must be a finite number between \
                         -9223372036854775808 and 9223372036854775807.999999, {} given",
                        crate::output::php_gcvt(f, -1, b'e')
                    ),
                ));
            }
            (secs as i64, ((f - secs) * 1_000_000.0).round() as u32)
        }
        Value::Int(i) => (*i, 0),
        Value::Bool(b) => (i64::from(*b), 0),
        s @ Value::Str(_) if s.is_numeric() => (s.to_int(), 0),
        other => {
            return Err(Unwind::type_error(format!(
                "{who}(): Argument #1 ($timestamp) must be of type int|float, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    new_dt(
        ctx,
        class,
        DtState {
            ts,
            usec: usec.min(999_999),
            tz: Tz::Offset(0),
        },
    )
}

/// `DateTime::createFromInterface(DateTimeInterface $object): static`
fn create_from_interface_mutable(
    ctx: &mut Ctx,
    _: Option<&Object>,
    a: &mut [Value],
) -> NativeResult {
    copy_of(
        ctx,
        b"DateTime",
        "DateTime::createFromInterface",
        "object",
        a,
    )
}

/// `DateTimeImmutable::createFromInterface(DateTimeInterface $object): static`
fn create_from_interface_immutable(
    ctx: &mut Ctx,
    _: Option<&Object>,
    a: &mut [Value],
) -> NativeResult {
    copy_of(
        ctx,
        b"DateTimeImmutable",
        "DateTimeImmutable::createFromInterface",
        "object",
        a,
    )
}

/// `DateTime::createFromImmutable(DateTimeImmutable $object): static`
fn create_from_immutable(ctx: &mut Ctx, _: Option<&Object>, a: &mut [Value]) -> NativeResult {
    copy_of(
        ctx,
        b"DateTime",
        "DateTime::createFromImmutable",
        "object",
        a,
    )
}

/// `DateTimeImmutable::createFromMutable(DateTime $object): static`
fn create_from_mutable(ctx: &mut Ctx, _: Option<&Object>, a: &mut [Value]) -> NativeResult {
    copy_of(
        ctx,
        b"DateTimeImmutable",
        "DateTimeImmutable::createFromMutable",
        "object",
        a,
    )
}

/// The shared body of the four converting factories.
fn copy_of(
    ctx: &mut Ctx,
    class: &[u8],
    who: &str,
    param: &str,
    args: &mut [Value],
) -> NativeResult {
    let src = obj_arg(ctx, who, 1, param, "DateTimeInterface", false, args.first())?.ok_or_else(
        || {
            Unwind::type_error(format!(
                "{who}(): Argument #1 (${param}) must be of type DateTimeInterface, null given"
            ))
        },
    )?;
    new_dt(ctx, class, dt_state(&src))
}

// ---- serialization --------------------------------------------------------------------

/// The `[date, timezone_type, timezone]` bag every serialization form uses.
fn state_array(st: &DtState) -> Array {
    let r = format::render(&st.tz, st.ts, st.usec);
    let mut a = Array::new();
    a.set(
        ArrayKey::str(b"date"),
        Value::string(&format::format(&r, b"Y-m-d H:i:s.u")),
    );
    a.set(ArrayKey::str(b"timezone_type"), Value::Int(st.tz.type_id()));
    a.set(
        ArrayKey::str(b"timezone"),
        Value::string(st.tz.name().as_bytes()),
    );
    a
}

/// `DateTimeInterface::__serialize(): array`
fn magic_serialize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Array(state_array(&dt_state(this(o)?))))
}

/// Rebuild a date object's state from a `[date, timezone]` bag.
fn from_bag(ctx: &mut Ctx, bag: &Array, who: &str) -> Result<DtState, Unwind> {
    let name = bag
        .get_deref(&ArrayKey::str(b"timezone"))
        .map(|v| v.to_php_string())
        .unwrap_or_else(|| "UTC".to_string());
    let Some(tz) = Tz::parse(&name) else {
        return Err(date_throw(
            ctx,
            "DateInvalidTimeZoneException",
            format!("{who}(): Unknown or bad timezone ({name})"),
        ));
    };
    let text = bag
        .get_deref(&ArrayKey::str(b"date"))
        .map(|v| v.to_php_bytes())
        .unwrap_or_default();
    let p = parse::parse(&text);
    if let Some(err) = p.errors.first().copied() {
        return Err(malformed_string(ctx, Some(who), &text, err));
    }
    let base = DtState { ts: 0, usec: 0, tz };
    Ok(apply_parse(&p, &base, 0, None))
}

/// The array argument of `__unserialize` / `__set_state`.
fn bag_arg(who: &str, v: &Value) -> Result<Array, Unwind> {
    match &*v.deref() {
        Value::Array(a) => Ok(a.clone()),
        _ => Err(Unwind::type_error(format!(
            "{who}(): Argument #1 ($data) must be of type array"
        ))),
    }
}

/// `DateTimeInterface::__unserialize(array $data): void`
fn magic_unserialize(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let bag = bag_arg("DateTime::__unserialize", &args[0])?;
    let st = from_bag(ctx, &bag, "DateTime::__unserialize")?;
    put_dt(o, st);
    Ok(Value::Null)
}

/// `DateTimeInterface::__wakeup(): void` — the pre-8.2 form, where the three
/// properties are already on the object.
fn magic_wakeup(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut bag = Array::new();
    for key in [b"date".as_slice(), b"timezone"] {
        if let Some(v) = o.get_deref(key) {
            bag.set(ArrayKey::str(key), v);
        }
    }
    let st = from_bag(ctx, &bag, "DateTime::__wakeup")?;
    put_dt(o, st);
    Ok(Value::Null)
}

/// `DateTime::__set_state(array $array): DateTime`
fn set_state_mutable(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let bag = bag_arg("DateTime::__set_state", &args[0])?;
    let st = from_bag(ctx, &bag, "DateTime::__set_state")?;
    new_dt(ctx, b"DateTime", st)
}

/// `DateTimeImmutable::__set_state(array $array): DateTimeImmutable`
fn set_state_immutable(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let bag = bag_arg("DateTimeImmutable::__set_state", &args[0])?;
    let st = from_bag(ctx, &bag, "DateTimeImmutable::__set_state")?;
    new_dt(ctx, b"DateTimeImmutable", st)
}

// ---- registration -------------------------------------------------------------------------

/// `native_init`: an instance starts at the wall clock in the default zone,
/// so that an object built without its constructor still renders.
fn dt_init(interp: &mut rphp_runtime::Interp, o: &Object) -> Result<(), Unwind> {
    let mut ctx = Ctx(interp);
    let tz = default_tz(&mut ctx);
    let (ts, usec) = now();
    put_dt(o, DtState { ts, usec, tz });
    Ok(())
}

/// Register `DateTimeInterface`, `DateTime` and `DateTimeImmutable`.
pub(crate) fn register_classes(r: &mut Registry) {
    // The class constants live on the interface; both classes inherit them.
    r.interface("DateTimeInterface")
        .class_const("ATOM", Value::string(br"Y-m-d\TH:i:sP"))
        .class_const("COOKIE", Value::string(b"l, d-M-Y H:i:s T"))
        .class_const("ISO8601", Value::string(br"Y-m-d\TH:i:sO"))
        .class_const("ISO8601_EXPANDED", Value::string(br"X-m-d\TH:i:sP"))
        .class_const("RFC822", Value::string(b"D, d M y H:i:s O"))
        .class_const("RFC850", Value::string(b"l, d-M-y H:i:s T"))
        .class_const("RFC1036", Value::string(b"D, d M y H:i:s O"))
        .class_const("RFC1123", Value::string(b"D, d M Y H:i:s O"))
        .class_const("RFC7231", Value::string(br"D, d M Y H:i:s \G\M\T"))
        .class_const("RFC2822", Value::string(b"D, d M Y H:i:s O"))
        .class_const("RFC3339", Value::string(br"Y-m-d\TH:i:sP"))
        .class_const("RFC3339_EXTENDED", Value::string(br"Y-m-d\TH:i:s.vP"))
        .class_const("RSS", Value::string(b"D, d M Y H:i:s O"))
        .class_const("W3C", Value::string(br"Y-m-d\TH:i:sP"))
        .abstract_method("format", nm!(1, Some(1), format))
        .abstract_method("getTimezone", nm!(0, Some(0), get_timezone))
        .abstract_method("getOffset", nm!(0, Some(0), get_offset))
        .abstract_method("getTimestamp", nm!(0, Some(0), get_timestamp))
        .abstract_method("getMicrosecond", nm!(0, Some(0), get_microsecond))
        .abstract_method("diff", nm!(1, Some(2), diff))
        .abstract_method("__wakeup", nm!(0, Some(0), magic_wakeup))
        .abstract_method("__serialize", nm!(0, Some(0), magic_serialize))
        .abstract_method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .finish();

    shared(r.class("DateTime"))
        .method("__construct", nm!(0, Some(2), construct_mutable))
        .method("__set_state", snm(1, Some(1), set_state_mutable))
        .method(
            "createFromImmutable",
            snm(1, Some(1), create_from_immutable),
        )
        .method(
            "createFromInterface",
            snm(1, Some(1), create_from_interface_mutable),
        )
        .method(
            "createFromFormat",
            snm(2, Some(3), create_from_format_mutable),
        )
        .method(
            "createFromTimestamp",
            snm(1, Some(1), create_from_timestamp_mutable),
        )
        .finish();

    shared(r.class("DateTimeImmutable"))
        .method("__construct", nm!(0, Some(2), construct_immutable))
        .method("__set_state", snm(1, Some(1), set_state_immutable))
        .method("createFromMutable", snm(1, Some(1), create_from_mutable))
        .method(
            "createFromInterface",
            snm(1, Some(1), create_from_interface_immutable),
        )
        .method(
            "createFromFormat",
            snm(2, Some(3), create_from_format_immutable),
        )
        .method(
            "createFromTimestamp",
            snm(1, Some(1), create_from_timestamp_immutable),
        )
        .finish();
}

/// Everything `DateTime` and `DateTimeImmutable` declare identically. The
/// two differ only in their constructor, their factories and what
/// [`answer`] does, so this is where the class really lives.
fn shared(b: ClassBuilder<'_>) -> ClassBuilder<'_> {
    b.implements(&["DateTimeInterface"])
        // The three keys php's debug handler shows, in php's order.
        .prop(
            "date",
            Visibility::Public,
            Value::string(b"1970-01-01 00:00:00.000000"),
        )
        .prop("timezone_type", Visibility::Public, Value::Int(3))
        .prop("timezone", Visibility::Public, Value::string(b"UTC"))
        .native_init(dt_init)
        .payload_clone(super::dt_clone)
        .method("format", nm!(1, Some(1), format))
        .method("modify", nm!(1, Some(1), modify))
        .method("add", nm!(1, Some(1), add))
        .method("sub", nm!(1, Some(1), sub))
        .method("getTimezone", nm!(0, Some(0), get_timezone))
        .method("setTimezone", nm!(1, Some(1), set_timezone))
        .method("getOffset", nm!(0, Some(0), get_offset))
        .method("getMicrosecond", nm!(0, Some(0), get_microsecond))
        .method("setMicrosecond", nm!(1, Some(1), set_microsecond))
        .method("setTime", nm!(2, Some(4), set_time))
        .method("setDate", nm!(3, Some(3), set_date))
        .method("setISODate", nm!(2, Some(3), set_iso_date))
        .method("setTimestamp", nm!(1, Some(1), set_timestamp))
        .method("getTimestamp", nm!(0, Some(0), get_timestamp))
        .method("diff", nm!(1, Some(2), diff))
        .method("getLastErrors", snm(0, Some(0), get_last_errors))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("__wakeup", nm!(0, Some(0), magic_wakeup))
}
