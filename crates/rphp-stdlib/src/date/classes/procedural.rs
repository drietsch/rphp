//! The procedural aliases php keeps beside the date classes.
//!
//! Every one of them is the method with the receiver moved into argument
//! one, so each body here is a coercion followed by a delegation. The two
//! places they genuinely differ are worth the code: the `*_create` family
//! and `timezone_open()` predate exceptions and still answer `false`
//! (`timezone_open()` with a warning as well) where the constructor throws.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Object, Value};

use super::{dt, interval, obj_arg, str_arg, zone};

/// This module's contribution to `date`'s function table. `date/funcs.rs`
/// owns `date_parse`, `date_default_timezone_get`/`set` and the two
/// timezone listings, so none of them appear here.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("date_create", 0, Some(2), date_create),
    nf!("date_create_immutable", 0, Some(2), date_create_immutable),
    nf!(
        "date_create_from_format",
        2,
        Some(3),
        date_create_from_format
    ),
    nf!(
        "date_create_immutable_from_format",
        2,
        Some(3),
        date_create_immutable_from_format
    ),
    nf!("date_get_last_errors", 0, Some(0), date_get_last_errors),
    nf!("date_format", 2, Some(2), date_format),
    nf!("date_modify", 2, Some(2), date_modify),
    nf!("date_add", 2, Some(2), date_add),
    nf!("date_sub", 2, Some(2), date_sub),
    nf!("date_diff", 2, Some(3), date_diff),
    nf!("date_timezone_get", 1, Some(1), date_timezone_get),
    nf!("date_timezone_set", 2, Some(2), date_timezone_set),
    nf!("date_offset_get", 1, Some(1), date_offset_get),
    nf!("date_date_set", 4, Some(4), date_date_set),
    nf!("date_isodate_set", 3, Some(4), date_isodate_set),
    nf!("date_time_set", 3, Some(5), date_time_set),
    nf!("date_timestamp_set", 2, Some(2), date_timestamp_set),
    nf!("date_timestamp_get", 1, Some(1), date_timestamp_get),
    nf!("timezone_open", 1, Some(1), timezone_open),
    nf!("timezone_name_get", 1, Some(1), timezone_name_get),
    nf!(
        "timezone_name_from_abbr",
        1,
        Some(3),
        timezone_name_from_abbr
    ),
    nf!("timezone_offset_get", 2, Some(2), timezone_offset_get),
    nf!(
        "timezone_transitions_get",
        1,
        Some(3),
        timezone_transitions_get
    ),
    nf!(
        "date_interval_create_from_date_string",
        1,
        Some(1),
        date_interval_create_from_date_string
    ),
    nf!("date_interval_format", 2, Some(2), date_interval_format),
];

/// The object an alias's first argument names.
fn recv(ctx: &mut Ctx, who: &str, class: &str, v: &Value) -> Result<Object, Unwind> {
    named_recv(ctx, who, "object", class, v)
}

/// [`recv`] for the two aliases whose first parameter is not `$object`.
fn named_recv(
    ctx: &mut Ctx,
    who: &str,
    param: &str,
    class: &str,
    v: &Value,
) -> Result<Object, Unwind> {
    obj_arg(ctx, who, 1, param, class, false, Some(v))?.ok_or_else(|| {
        Unwind::type_error(format!(
            "{who}(): Argument #1 (${param}) must be of type {class}, null given"
        ))
    })
}

/// The pre-exception answer: `false` for the one exception class the old
/// procedural surface used to report by return value.
fn soften(r: NativeResult, class: &str) -> NativeResult {
    match r {
        Err(u) if u.class_name().as_deref() == Some(class) => Ok(Value::Bool(false)),
        other => other,
    }
}

/// Build an object of `class` by running its constructor with `args`.
fn construct(ctx: &mut Ctx, class: &[u8], args: &[Value]) -> NativeResult {
    let cid = ctx.class_by_name(class).ok_or_else(|| {
        Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(class)
        ))
    })?;
    let o = ctx.new_object(cid)?;
    ctx.call_method(&o, b"__construct", args)?;
    Ok(Value::Object(o))
}

/// `date_create(string $datetime = "now", ?DateTimeZone $timezone = null): DateTime|false`
fn date_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let owned = args.to_vec();
    soften(
        construct(ctx, b"DateTime", &owned),
        "DateMalformedStringException",
    )
}

/// `date_create_immutable(…): DateTimeImmutable|false`
fn date_create_immutable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let owned = args.to_vec();
    soften(
        construct(ctx, b"DateTimeImmutable", &owned),
        "DateMalformedStringException",
    )
}

/// `date_create_from_format(string $format, string $datetime, ?DateTimeZone $timezone = null): DateTime|false`
fn date_create_from_format(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    dt::create_from_format(ctx, b"DateTime", "date_create_from_format", args)
}

/// `date_create_immutable_from_format(…): DateTimeImmutable|false`
fn date_create_immutable_from_format(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    dt::create_from_format(
        ctx,
        b"DateTimeImmutable",
        "date_create_immutable_from_format",
        args,
    )
}

/// `date_get_last_errors(): array|false`
fn date_get_last_errors(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(super::last_errors())
}

/// `date_format(DateTimeInterface $object, string $format): string`
fn date_format(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_format", "DateTimeInterface", &args[0])?;
    dt::format(ctx, Some(&o), &mut args[1..])
}

/// `date_modify(DateTime $object, string $modifier): DateTime|false`
fn date_modify(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_modify", "DateTime", &args[0])?;
    soften(
        dt::modify(ctx, Some(&o), &mut args[1..]),
        "DateMalformedStringException",
    )
}

/// `date_add(DateTime $object, DateInterval $interval): DateTime`
fn date_add(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_add", "DateTime", &args[0])?;
    dt::add(ctx, Some(&o), &mut args[1..])
}

/// `date_sub(DateTime $object, DateInterval $interval): DateTime`
fn date_sub(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_sub", "DateTime", &args[0])?;
    dt::sub(ctx, Some(&o), &mut args[1..])
}

/// `date_diff(DateTimeInterface $baseObject, DateTimeInterface $targetObject, bool $absolute = false): DateInterval`
fn date_diff(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = named_recv(
        ctx,
        "date_diff",
        "baseObject",
        "DateTimeInterface",
        &args[0],
    )?;
    dt::diff(ctx, Some(&o), &mut args[1..])
}

/// `date_timezone_get(DateTimeInterface $object): DateTimeZone|false`
fn date_timezone_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_timezone_get", "DateTimeInterface", &args[0])?;
    dt::get_timezone(ctx, Some(&o), &mut [])
}

/// `date_timezone_set(DateTime $object, DateTimeZone $timezone): DateTime`
fn date_timezone_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_timezone_set", "DateTime", &args[0])?;
    dt::set_timezone(ctx, Some(&o), &mut args[1..])
}

/// `date_offset_get(DateTimeInterface $object): int`
fn date_offset_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_offset_get", "DateTimeInterface", &args[0])?;
    dt::get_offset(ctx, Some(&o), &mut [])
}

/// `date_date_set(DateTime $object, int $year, int $month, int $day): DateTime`
fn date_date_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_date_set", "DateTime", &args[0])?;
    dt::set_date(ctx, Some(&o), &mut args[1..])
}

/// `date_isodate_set(DateTime $object, int $year, int $week, int $dayOfWeek = 1): DateTime`
fn date_isodate_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_isodate_set", "DateTime", &args[0])?;
    dt::set_iso_date(ctx, Some(&o), &mut args[1..])
}

/// `date_time_set(DateTime $object, int $hour, int $minute, int $second = 0, int $microsecond = 0): DateTime`
fn date_time_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_time_set", "DateTime", &args[0])?;
    dt::set_time(ctx, Some(&o), &mut args[1..])
}

/// `date_timestamp_set(DateTime $object, int $timestamp): DateTime`
fn date_timestamp_set(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_timestamp_set", "DateTime", &args[0])?;
    dt::set_timestamp(ctx, Some(&o), &mut args[1..])
}

/// `date_timestamp_get(DateTimeInterface $object): int`
fn date_timestamp_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_timestamp_get", "DateTimeInterface", &args[0])?;
    dt::get_timestamp(ctx, Some(&o), &mut [])
}

/// `timezone_open(string $timezone): DateTimeZone|false` — the one alias
/// that still warns instead of throwing.
fn timezone_open(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = str_arg(ctx, "timezone_open", 1, "timezone", &args[0])?;
    match construct(ctx, b"DateTimeZone", &[Value::string(&name)]) {
        Ok(v) => Ok(v),
        Err(u) if u.class_name().as_deref() == Some("DateInvalidTimeZoneException") => {
            ctx.warn(&format!(
                "timezone_open(): Unknown or bad timezone ({})",
                String::from_utf8_lossy(&name)
            ))?;
            Ok(Value::Bool(false))
        }
        Err(u) => Err(u),
    }
}

/// `timezone_name_get(DateTimeZone $object): string`
fn timezone_name_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "timezone_name_get", "DateTimeZone", &args[0])?;
    zone::get_name(ctx, Some(&o), &mut [])
}

/// `timezone_name_from_abbr(string $abbr, int $utcOffset = -1, int $isDST = -1): string|false`
fn timezone_name_from_abbr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    zone::name_from_abbr(ctx, args)
}

/// `timezone_offset_get(DateTimeZone $object, DateTimeInterface $datetime): int`
fn timezone_offset_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "timezone_offset_get", "DateTimeZone", &args[0])?;
    zone::get_offset(ctx, Some(&o), &mut args[1..])
}

/// `timezone_transitions_get(DateTimeZone $object, int $timestampBegin = PHP_INT_MIN, int $timestampEnd = PHP_INT_MAX): array|false`
fn timezone_transitions_get(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "timezone_transitions_get", "DateTimeZone", &args[0])?;
    zone::get_transitions(ctx, Some(&o), &mut args[1..])
}

/// `date_interval_create_from_date_string(string $datetime): DateInterval|false`
/// — the second alias that warns instead of throwing, with the exception's
/// own message behind the function name.
fn date_interval_create_from_date_string(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "date_interval_create_from_date_string";
    match interval::create_from_date_string(ctx, None, args) {
        Err(u) if u.class_name().as_deref() == Some("DateMalformedIntervalStringException") => {
            let msg = u.message_string().unwrap_or_default();
            ctx.warn(&format!("{who}(): {msg}"))?;
            Ok(Value::Bool(false))
        }
        other => other,
    }
}

/// `date_interval_format(DateInterval $object, string $format): string`
fn date_interval_format(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = recv(ctx, "date_interval_format", "DateInterval", &args[0])?;
    interval::format(ctx, Some(&o), &mut args[1..])
}
