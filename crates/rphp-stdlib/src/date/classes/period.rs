//! `DatePeriod` (php-src `ext/date/php_date.c`, the `date_period_*` half).
//!
//! The class is a plan, not a list: the seven properties php shows are the
//! plan, and iteration replays it by stepping the start date with
//! [`dt::advance`] in its *wall-clock* mode. That mode is not what
//! `DateTime::add()` does — a `PT24H` period across a spring-forward keeps
//! 12:00 where `add()` moves it to 13:00 — and reproducing the difference
//! is the reason the two share one entry point with a flag.
//!
//! **The `recurrences` property is not the argument.** php stores the
//! recurrence count *plus* the two boundary flags, so `new DatePeriod($s,
//! $i, 2)` shows `recurrences => 3` while `getRecurrences()` still answers
//! `2`, and the end-date form shows `1` (or `2` with `INCLUDE_END_DATE`)
//! while `getRecurrences()` answers `null`. The stored number is exactly
//! how many dates the iteration yields.
//!
//! **Known divergences.** `getIterator()` answers an `ArrayIterator` over
//! the dates rather than php's `InternalIterator`, and the `current`
//! property therefore stays `null` throughout the loop where php updates it
//! on every step.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, ArrayKey, Object, Value};

use super::super::tz::Tz;
use super::zone::snm;
use super::{date_throw, dt, dt_state, int_arg, put_dt, str_arg, this, DtState};

/// `DatePeriod::EXCLUDE_START_DATE`
const EXCLUDE_START_DATE: i64 = 1;
/// `DatePeriod::INCLUDE_END_DATE`
const INCLUDE_END_DATE: i64 = 2;

/// php's message for a constructor call that matches none of the three
/// signatures.
const SIGNATURES: &str = "DatePeriod::__construct() accepts (DateTimeInterface, DateInterval, int [, int]), or (DateTimeInterface, DateInterval, DateTime [, int]), or (string [, int]) as arguments";

/// Whether a value is an instance of `class`.
fn is_a(ctx: &Ctx, v: &Value, class: &[u8]) -> Option<Object> {
    let v = v.deref();
    let Value::Object(o) = &*v else { return None };
    let cid = ctx.class_by_name(class)?;
    ctx.object_instanceof(o, cid).then(|| o.clone())
}

/// Write the seven properties php shows, in php's order.
#[allow(clippy::too_many_arguments)]
fn seed(
    o: &Object,
    start: Object,
    end: Option<Object>,
    interval: Object,
    recurrences: i64,
    include_start: bool,
    include_end: bool,
) {
    o.set(b"start", Value::Object(start));
    o.set(b"current", Value::Null);
    o.set(b"end", end.map_or(Value::Null, Value::Object));
    o.set(b"interval", Value::Object(interval));
    o.set(b"recurrences", Value::Int(recurrences));
    o.set(b"include_start_date", Value::Bool(include_start));
    o.set(b"include_end_date", Value::Bool(include_end));
}

/// `DatePeriod::__construct()` in all three of php's shapes.
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    // The ISO form is a single string; php deprecated it in 8.2 in favour
    // of the named factory but still accepts it.
    if args.len() <= 2 && !matches!(&*args[0].deref(), Value::Object(_)) {
        ctx.deprecated(
            "Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead",
        )?;
        let spec = str_arg(ctx, "DatePeriod::__construct", 1, "isostr", &args[0])?;
        let options = match args.get(1) {
            Some(v) => int_arg(ctx, "DatePeriod::__construct", 2, "options", v)?,
            None => 0,
        };
        return from_iso(ctx, o, &spec, options, "DatePeriod::__construct");
    }
    if args.len() < 3 {
        return Err(Unwind::type_error(SIGNATURES));
    }
    let Some(start) = is_a(ctx, &args[0], b"DateTimeInterface") else {
        return Err(Unwind::type_error(SIGNATURES));
    };
    let Some(interval) = is_a(ctx, &args[1], b"DateInterval") else {
        return Err(Unwind::type_error(SIGNATURES));
    };
    let options = match args.get(3) {
        Some(v) => int_arg(ctx, "DatePeriod::__construct", 4, "options", v)?,
        None => 0,
    };
    let include_start = options & EXCLUDE_START_DATE == 0;
    let include_end = options & INCLUDE_END_DATE != 0;
    let extra = i64::from(include_start) + i64::from(include_end);
    match is_a(ctx, &args[2], b"DateTimeInterface") {
        Some(end) => {
            seed(
                o,
                start,
                Some(end),
                interval,
                extra,
                include_start,
                include_end,
            );
            Ok(Value::Null)
        }
        None => {
            let n = int_arg(ctx, "DatePeriod::__construct", 3, "recurrences", &args[2])?;
            if !(1..2_147_483_640).contains(&n) {
                return Err(date_throw(
                    ctx,
                    "DateMalformedPeriodStringException",
                    "DatePeriod::__construct(): Recurrence count must be greater or equal to 1 and lower than 2147483640".to_string(),
                ));
            }
            seed(
                o,
                start,
                None,
                interval,
                n + extra,
                include_start,
                include_end,
            );
            Ok(Value::Null)
        }
    }
}

/// `DatePeriod::createFromISO8601String(string $specification, int $options = 0): static`
fn create_from_iso(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = "DatePeriod::createFromISO8601String";
    let spec = str_arg(ctx, who, 1, "specification", &args[0])?;
    let options = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "options", v)?,
        None => 0,
    };
    let cid = ctx
        .class_by_name(b"DatePeriod")
        .ok_or_else(|| Unwind::error("Class \"DatePeriod\" not found"))?;
    let o = ctx.instantiate(cid);
    from_iso(ctx, &o, &spec, options, who)?;
    Ok(Value::Object(o))
}

/// `YYYY-MM-DDTHH:MM:SSZ` or `YYYYMMDDTHHMMSSZ`, and nothing else — the
/// only two spellings php's interval scanner accepts for the start of a
/// repeating interval.
fn iso_instant(s: &[u8]) -> Option<[i64; 6]> {
    let body = s.strip_suffix(b"Z")?;
    let (layout, positions): (&[(usize, u8)], [(usize, usize); 6]) = match body.len() {
        19 => (
            &[(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')],
            [(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19)],
        ),
        15 => (
            &[(8, b'T')],
            [(0, 4), (4, 6), (6, 8), (9, 11), (11, 13), (13, 15)],
        ),
        _ => return None,
    };
    for &(at, ch) in layout {
        if body.get(at) != Some(&ch) {
            return None;
        }
    }
    let mut out = [0i64; 6];
    for (slot, (from, to)) in out.iter_mut().zip(positions) {
        let part = &body[from..to];
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *slot = std::str::from_utf8(part).ok()?.parse().ok()?;
    }
    Some(out)
}

/// php's `Unknown or bad format (…)` for a specification it cannot read.
fn bad_format(ctx: &mut Ctx, text: &str) -> Unwind {
    date_throw(
        ctx,
        "DateMalformedPeriodStringException",
        format!("Unknown or bad format ({text})"),
    )
}

/// php's ISO 8601 repeating-interval grammar, `R<n>/<start>/<duration>`.
///
/// The scanner is **not** positional in the way the spelling suggests: php
/// classifies every `/`-separated component on its own — `R<n>`, an
/// instant, or a duration — so `<start>/R2/<duration>` is accepted and an
/// empty component is ignored. What position decides is only *which* date a
/// date is: one that arrives before any duration is the start, and one
/// after it is the end, which is what makes `R2/P7D/<date>` the ISO
/// "duration then end date" form and therefore a specification with **no
/// start date**.
///
/// The three semantic errors are then reported in php's order — start date,
/// interval, recurrence count — which is why `<start>/<end>` (two dates, no
/// duration) reports the missing interval and not the missing count.
fn from_iso(ctx: &mut Ctx, o: &Object, spec: &[u8], options: i64, who: &str) -> NativeResult {
    let text = String::from_utf8_lossy(spec).into_owned();
    let mut recurrences = 0i64;
    let mut begin: Option<[i64; 6]> = None;
    let mut end: Option<[i64; 6]> = None;
    let mut duration: Option<&[u8]> = None;
    let mut components = 0usize;
    for part in spec.split(|&c| c == b'/') {
        if part.is_empty() {
            continue;
        }
        components += 1;
        if let Some(digits) = part.strip_prefix(b"R") {
            if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                return Err(bad_format(ctx, &text));
            }
            // php's scanner reads at most nine digits, so `R99999999999` is
            // 999 999 999 repetitions and the upper bound below is out of
            // reach from a string.
            let head = &digits[..digits.len().min(9)];
            let Ok(n) = std::str::from_utf8(head).unwrap_or("").parse::<i64>() else {
                return Err(bad_format(ctx, &text));
            };
            recurrences = n;
            continue;
        }
        if part.starts_with(b"P") {
            if super::interval::parse_duration(part).is_none() {
                return Err(bad_format(ctx, &text));
            }
            duration = Some(part);
            continue;
        }
        // The start is the strict ISO form only: a full date and time with
        // the `Z` suffix, extended or compact. A local time, a numeric
        // offset and a fraction are all refused, and the result is the
        // fixed offset +00:00 on an immutable object.
        let Some(fields) = iso_instant(part) else {
            return Err(bad_format(ctx, &text));
        };
        // timelib: a date after any date or duration is the end date (the
        // last one wins), the first one otherwise is the start.
        if begin.is_none() && end.is_none() && duration.is_none() {
            begin = Some(fields);
        } else {
            end = Some(fields);
        }
    }
    if components == 0 {
        return Err(bad_format(ctx, &text));
    }
    let Some(fields) = begin else {
        return Err(date_throw(
            ctx,
            "DateMalformedPeriodStringException",
            format!("{who}(): ISO interval must contain a start date, \"{text}\" given"),
        ));
    };
    let Some(duration) = duration else {
        return Err(date_throw(
            ctx,
            "DateMalformedPeriodStringException",
            format!("{who}(): ISO interval must contain an interval, \"{text}\" given"),
        ));
    };
    // 8.5.10: without an end date the count must be given (8.5.0 compared
    // the pointer and fell through to the range check below).
    if end.is_none() && recurrences == 0 {
        return Err(date_throw(
            ctx,
            "DateMalformedPeriodStringException",
            format!("{who}(): ISO interval must contain an end date or a recurrence count, \"{text}\" given"),
        ));
    }
    if end.is_none() && !(1..2_147_483_640).contains(&recurrences) {
        return Err(date_throw(
            ctx,
            "DateMalformedPeriodStringException",
            format!(
                "{who}(): Recurrence count must be greater or equal to 1 and lower than 2147483640"
            ),
        ));
    }
    let instant = |ctx: &mut Ctx, fields: [i64; 6]| -> Result<Object, Unwind> {
        let tz = Tz::Offset(0);
        let cid = ctx
            .class_by_name(b"DateTimeImmutable")
            .ok_or_else(|| Unwind::error("Class \"DateTimeImmutable\" not found"))?;
        let inst = ctx.instantiate(cid);
        let ts = super::super::civil::days_from_civil(fields[0], fields[1], fields[2])
            .saturating_mul(86_400)
            + fields[3] * 3600
            + fields[4] * 60
            + fields[5];
        put_dt(&inst, DtState { ts, usec: 0, tz });
        Ok(inst)
    };
    let start = instant(ctx, fields)?;
    let end = match end {
        Some(fields) => Some(instant(ctx, fields)?),
        None => None,
    };
    let interval = {
        let cid = ctx
            .class_by_name(b"DateInterval")
            .ok_or_else(|| Unwind::error("Class \"DateInterval\" not found"))?;
        let inst = ctx.new_object(cid)?;
        ctx.call_method(&inst, b"__construct", &[Value::string(duration)])?;
        inst
    };
    let include_start = options & EXCLUDE_START_DATE == 0;
    let include_end = options & INCLUDE_END_DATE != 0;
    let extra = i64::from(include_start) + i64::from(include_end);
    seed(
        o,
        start,
        end,
        interval,
        recurrences + extra,
        include_start,
        include_end,
    );
    Ok(Value::Null)
}

// ---- reading ----------------------------------------------------------------------

/// A declared object property.
fn obj_prop(o: &Object, name: &[u8]) -> Option<Object> {
    match o.get_deref(name) {
        Some(Value::Object(x)) => Some(x),
        _ => None,
    }
}

/// `DatePeriod::getStartDate(): DateTimeInterface`
fn get_start_date(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(obj_prop(this(o)?, b"start").map_or(Value::Null, Value::Object))
}

/// `DatePeriod::getEndDate(): ?DateTimeInterface`
fn get_end_date(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(obj_prop(this(o)?, b"end").map_or(Value::Null, Value::Object))
}

/// `DatePeriod::getDateInterval(): DateInterval`
fn get_date_interval(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(obj_prop(this(o)?, b"interval").map_or(Value::Null, Value::Object))
}

/// `DatePeriod::getRecurrences(): ?int` — the count the caller gave, which
/// the end-date form never had.
fn get_recurrences(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    if o.get_deref(b"end")
        .is_some_and(|v| !matches!(v, Value::Null))
    {
        return Ok(Value::Null);
    }
    let stored = o.get_deref(b"recurrences").map_or(0, |v| v.to_int());
    let include_start = o
        .get_deref(b"include_start_date")
        .is_some_and(|v| v.to_bool());
    let include_end = o
        .get_deref(b"include_end_date")
        .is_some_and(|v| v.to_bool());
    Ok(Value::Int(
        stored - i64::from(include_start) - i64::from(include_end),
    ))
}

/// Replay the plan: every date the period yields, in order.
fn dates(ctx: &mut Ctx, o: &Object) -> Result<Vec<Object>, Unwind> {
    let Some(start) = obj_prop(o, b"start") else {
        return Ok(Vec::new());
    };
    let Some(interval) = obj_prop(o, b"interval") else {
        return Ok(Vec::new());
    };
    let end = obj_prop(o, b"end");
    let include_start = o
        .get_deref(b"include_start_date")
        .is_some_and(|v| v.to_bool());
    let include_end = o
        .get_deref(b"include_end_date")
        .is_some_and(|v| v.to_bool());
    let limit = o.get_deref(b"recurrences").map_or(0, |v| v.to_int());
    let class = start.class_id();
    let mut st = dt_state(&start);
    if !include_start {
        st = dt::advance(ctx, &st, &interval, false, true)?;
    }
    let mut out = Vec::new();
    match end {
        Some(end) => {
            let stop = dt_state(&end);
            // php's own guard against an interval that never moves.
            let mut guard = 0u32;
            while (st.ts, st.usec) < (stop.ts, stop.usec)
                || (include_end && (st.ts, st.usec) == (stop.ts, stop.usec))
            {
                out.push(make(ctx, class, st.clone()));
                let next = dt::advance(ctx, &st, &interval, false, true)?;
                if (next.ts, next.usec) == (st.ts, st.usec) {
                    break;
                }
                st = next;
                guard += 1;
                if guard == u32::MAX {
                    break;
                }
            }
        }
        None => {
            for _ in 0..limit.max(0) {
                out.push(make(ctx, class, st.clone()));
                st = dt::advance(ctx, &st, &interval, false, true)?;
            }
        }
    }
    Ok(out)
}

/// One date of the period, of the same class as the start date.
fn make(ctx: &mut Ctx, class: u32, st: DtState) -> Object {
    let o = ctx.instantiate(class);
    put_dt(&o, st);
    o
}

/// `DatePeriod::getIterator(): Iterator`
fn get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let list = dates(ctx, o)?;
    let mut a = Array::new();
    for d in list {
        a.push(Value::Object(d));
    }
    let cid = ctx
        .class_by_name(b"ArrayIterator")
        .ok_or_else(|| Unwind::error("Class \"ArrayIterator\" not found"))?;
    let it = ctx.new_object(cid)?;
    ctx.call_method(&it, b"__construct", &[Value::Array(a)])?;
    Ok(Value::Object(it))
}

// ---- serialization ------------------------------------------------------------------

/// The seven keys php serializes, in php's order.
const KEYS: [&[u8]; 7] = [
    b"start",
    b"current",
    b"end",
    b"interval",
    b"recurrences",
    b"include_start_date",
    b"include_end_date",
];

/// `DatePeriod::__serialize(): array`
fn magic_serialize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut a = Array::new();
    for key in KEYS {
        a.set(ArrayKey::str(key), o.get_deref(key).unwrap_or(Value::Null));
    }
    Ok(Value::Array(a))
}

/// Copy a property bag onto an instance.
fn restore(o: &Object, bag: &Array) {
    for key in KEYS {
        if let Some(v) = bag.get_deref(&ArrayKey::str(key)) {
            o.set(key, v);
        }
    }
}

/// `DatePeriod::__unserialize(array $data): void`
fn magic_unserialize(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Value::Array(bag) = &*args[0].deref() else {
        return Err(Unwind::type_error(
            "DatePeriod::__unserialize(): Argument #1 ($data) must be of type array",
        ));
    };
    restore(o, bag);
    Ok(Value::Null)
}

/// `DatePeriod::__wakeup(): void` — the properties are the state.
fn magic_wakeup(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Null)
}

/// `DatePeriod::__set_state(array $array): DatePeriod`
fn set_state(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Value::Array(bag) = &*args[0].deref() else {
        return Err(Unwind::type_error(
            "DatePeriod::__set_state(): Argument #1 ($array) must be of type array",
        ));
    };
    let bag = bag.clone();
    let cid = ctx
        .class_by_name(b"DatePeriod")
        .ok_or_else(|| Unwind::error("Class \"DatePeriod\" not found"))?;
    let o = ctx.instantiate(cid);
    restore(&o, &bag);
    Ok(Value::Object(o))
}

/// Register `DatePeriod`.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("DatePeriod")
        .implements(&["IteratorAggregate"])
        .class_const("EXCLUDE_START_DATE", Value::Int(EXCLUDE_START_DATE))
        .class_const("INCLUDE_END_DATE", Value::Int(INCLUDE_END_DATE))
        // php makes these seven readonly and virtual; they are plain public
        // slots here (see the module header).
        .prop("start", Visibility::Public, Value::Null)
        .prop("current", Visibility::Public, Value::Null)
        .prop("end", Visibility::Public, Value::Null)
        .prop("interval", Visibility::Public, Value::Null)
        .prop("recurrences", Visibility::Public, Value::Int(0))
        .prop("include_start_date", Visibility::Public, Value::Bool(true))
        .prop("include_end_date", Visibility::Public, Value::Bool(false))
        .method("__construct", nm!(1, Some(4), construct))
        .method("createFromISO8601String", snm(1, Some(2), create_from_iso))
        .method("getStartDate", nm!(0, Some(0), get_start_date))
        .method("getEndDate", nm!(0, Some(0), get_end_date))
        .method("getDateInterval", nm!(0, Some(0), get_date_interval))
        .method("getRecurrences", nm!(0, Some(0), get_recurrences))
        .method("getIterator", nm!(0, Some(0), get_iterator))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("__wakeup", nm!(0, Some(0), magic_wakeup))
        .method("__set_state", snm(1, Some(1), set_state))
        .finish();
}
