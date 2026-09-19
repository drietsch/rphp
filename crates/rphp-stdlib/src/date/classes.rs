//! php-src `ext/date`'s object half: the `DateTime` family, `DateTimeZone`,
//! `DateInterval`, `DatePeriod` and the nine exception classes they throw.
//!
//! **Where the state lives.** php's date objects have no real properties at
//! all — `ext/date` installs a `get_properties` handler that *synthesizes*
//! the array `var_dump`, `serialize` and `var_export` see out of the C
//! struct. Two consequences are observable and are reproduced here in the
//! two different ways the object model allows:
//!
//! * `DateTime`, `DateTimeImmutable` and `DateTimeZone` always show the same
//!   keys, so those are real declared slots kept in step with the instant
//!   behind them ([`sync_dt`], [`sync_zone`]). The instant itself — the
//!   timestamp, the microseconds and the resolved [`Tz`] — is the instance's
//!   native [`Payload`].
//! * `DateInterval`'s key *set* depends on how it was built: ten keys for a
//!   parsed duration, two (`from_string`, `date_string`) for one made by
//!   `createFromDateString`. A declared layout cannot express that, so a
//!   `DateInterval`'s fields are ordinary *dynamic* properties written at
//!   construction. That also makes `$i->d = 5` work the way it does in php,
//!   where the write lands on the C struct and `format()` reads it back.
//!
//! **Civil, not absolute.** Every calendar operation goes through
//! [`super::parse`]'s resolution pipeline: the fields are moved, normalized
//! and converted to an instant exactly once, at the end. That is why
//! `$d->add(new DateInterval('P1D'))` across a spring-forward is 23 real
//! hours while `PT24H` is 24, and why `modify()` on an overlapping wall
//! clock keeps the offset the object already carried (`Tz::resolve`'s
//! `prefer`).
//!
//! **`diff()`.** php's is neither a wall-clock nor an absolute difference:
//! it is a field-wise subtraction of the two *local* civil times, normalized
//! with timelib's borrow rules (the month before the day, which is why
//! `2021-01-31` to `2021-03-01` is 29 days and not one month one day), with
//! two corrections. Between two *different* zones the offset difference is
//! taken out of the hour and minute fields before normalizing; within one
//! zone across a DST transition the correction is the difference between the
//! offset at the end and the offset the calendar part lands on, applied
//! *after* normalizing and without re-normalizing — which is how php can
//! answer `h=6, i=-29`. The rule here was fitted against stock php over
//! ~50 000 random pairs across fifteen zones; see the known divergences.
//!
//! **Known divergences.**
//!
//! * A static factory (`createFromFormat`, `createFromInterface`,
//!   `createFromTimestamp`, `__set_state`) built on a *user subclass* of
//!   `DateTime` answers with a `DateTime`, because the native ABI hands a
//!   static method no late-static-bound class.
//! * `get_object_vars()` on a `DateTime` yields the three keys here and
//!   nothing in php, which answers `get_properties_for(GET_OBJECT_VARS)`
//!   with an empty set and only fills the debug view. Reflection likewise
//!   reports three declared properties where php reports none.
//! * `DatePeriod`'s seven properties are plain public slots; php makes them
//!   `readonly` (and virtual), so php raises `Error: Cannot modify readonly
//!   property DatePeriod::$start` on a write and this does not.
//! * `DatePeriod::getIterator()` answers an `ArrayIterator` over the
//!   generated dates; php answers its own internal iterator. The values and
//!   keys a `foreach` sees are the same.
//! * `<` / `>` between two date objects compares the declared slots, so it
//!   is correct within one timezone and wrong across two; php compares the
//!   instants.
//! * `DateTimeZone::getLocation()` and `timezone_location_get()` are not
//!   registered: the latitude, longitude and comment columns of php's
//!   `zone.tab` snapshot are not in `tz.rs`'s tables.
//! * `DateTime::createFromFormat()` implements php's format characters but
//!   not its `getLastErrors()` *positions*: an error is reported at the
//!   offset the scanner stopped at rather than at php's per-character one.

use std::cell::RefCell;

use rphp_runtime::{Ctx, NativeFn, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Payload, Value};

use super::format;
use super::tz::Tz;

mod dt;
mod fromformat;
mod interval;
mod period;
mod procedural;
mod zone;

// ---- the hidden state ---------------------------------------------------------

/// What a `DateTime` / `DateTimeImmutable` keeps out of sight: the instant,
/// its microseconds, and the zone the three visible slots are rendered in.
#[derive(Clone)]
pub(crate) struct DtState {
    pub(crate) ts: i64,
    pub(crate) usec: u32,
    pub(crate) tz: Tz,
}

/// What a `DateTimeZone` keeps out of sight: the parsed zone. The two
/// visible slots are its `timezone_type` and its name.
#[derive(Clone)]
pub(crate) struct ZoneState {
    pub(crate) tz: Option<Tz>,
}

/// Rewrite the three slots php shows for a date object.
pub(crate) fn sync_dt(o: &Object, st: &DtState) {
    let r = format::render(&st.tz, st.ts, st.usec);
    o.set(
        b"date",
        Value::string(&format::format(&r, b"Y-m-d H:i:s.u")),
    );
    o.set(b"timezone_type", Value::Int(st.tz.type_id()));
    o.set(b"timezone", Value::string(st.tz.name().as_bytes()));
}

/// Install a date object's state and its visible slots in one step.
pub(crate) fn put_dt(o: &Object, st: DtState) {
    sync_dt(o, &st);
    o.set_payload(Payload::Native(Box::new(st)));
}

/// A date object's state. An instance the engine built without running
/// `native_init` — `unserialize`, an `(object)` cast — has no payload, so
/// the two visible slots are re-read instead.
pub(crate) fn dt_state(o: &Object) -> DtState {
    if let Some(st) = o.with_payload::<DtState, _>(|s| s.clone()) {
        return st;
    }
    let tz = o
        .get_deref(b"timezone")
        .map(|v| v.to_php_string())
        .and_then(|n| Tz::parse(&n))
        .unwrap_or_else(Tz::utc);
    let text = o
        .get_deref(b"date")
        .map(|v| v.to_php_bytes())
        .unwrap_or_default();
    let p = super::parse::parse(&text);
    let (ts, usec) = super::parse::resolve(&p, 0, &tz);
    DtState { ts, usec, tz }
}

/// Rewrite the two slots php shows for a `DateTimeZone`.
pub(crate) fn sync_zone(o: &Object, tz: &Tz) {
    o.set(b"timezone_type", Value::Int(tz.type_id()));
    o.set(b"timezone", Value::string(tz.name().as_bytes()));
}

/// Install a zone object's state and its visible slots.
pub(crate) fn put_zone(o: &Object, tz: Tz) {
    sync_zone(o, &tz);
    o.set_payload(Payload::Native(Box::new(ZoneState { tz: Some(tz) })));
}

/// The zone a `DateTimeZone` instance carries, falling back to its visible
/// name for an instance the engine built without the constructor.
pub(crate) fn zone_state(o: &Object) -> Option<Tz> {
    if let Some(Some(tz)) = o.with_payload::<ZoneState, _>(|s| s.tz.clone()) {
        return Some(tz);
    }
    let name = o.get_deref(b"timezone").map(|v| v.to_php_string())?;
    Tz::parse(&name)
}

/// `clone` of a date object.
fn dt_clone(_: &mut rphp_runtime::Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    dst.set_payload(Payload::Native(Box::new(dt_state(src))));
    Ok(())
}

/// `clone` of a `DateTimeZone`.
fn zone_clone(_: &mut rphp_runtime::Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    dst.set_payload(Payload::Native(Box::new(ZoneState {
        tz: zone_state(src),
    })));
    Ok(())
}

// ---- diagnostics ---------------------------------------------------------------

/// The receiver of an instance method.
pub(crate) fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// `throw new <class>(message)` for one of `ext/date`'s own classes, as a
/// real object when the class is registered.
pub(crate) fn date_throw(ctx: &mut Ctx, class: &'static str, message: String) -> Unwind {
    match ctx.class_by_name(class.as_bytes()) {
        Some(cid) => Unwind::Throw(ctx.create_throwable(cid, &message, 0, None, None)),
        None => Unwind::exception(class, message),
    }
}

/// php's `Failed to parse time string (…) at position N (c): message`, with
/// the caller's name in front for every entry point but the constructors.
pub(crate) fn malformed_string(
    ctx: &mut Ctx,
    who: Option<&str>,
    text: &[u8],
    err: (usize, &'static str),
) -> Unwind {
    let (pos, msg) = err;
    let ch = text.get(pos).copied().unwrap_or(b' ') as char;
    let prefix = who.map(|w| format!("{w}(): ")).unwrap_or_default();
    date_throw(
        ctx,
        "DateMalformedStringException",
        format!(
            "{prefix}Failed to parse time string ({}) at position {pos} ({ch}): {msg}",
            String::from_utf8_lossy(text)
        ),
    )
}

// ---- `getLastErrors()` ------------------------------------------------------------

thread_local! {
    /// What the last `createFromFormat` / constructor parse reported, as
    /// `DateTime::getLastErrors()` returns it. php keeps one slot for the
    /// whole request and answers `false` while nothing has been parsed.
    static LAST_ERRORS: RefCell<Option<(Vec<(usize, String)>, Vec<(usize, String)>)>> =
        const { RefCell::new(None) };
}

/// Record a parse's diagnostics for `getLastErrors()`.
pub(crate) fn set_last_errors(warnings: Vec<(usize, String)>, errors: Vec<(usize, String)>) {
    LAST_ERRORS.with(|s| *s.borrow_mut() = Some((warnings, errors)));
}

/// `DateTime::getLastErrors(): array|false`
pub(crate) fn last_errors() -> Value {
    LAST_ERRORS.with(|s| match &*s.borrow() {
        None => Value::Bool(false),
        Some((warnings, errors)) => {
            let mut a = Array::new();
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
            Value::Array(a)
        }
    })
}

/// php's `[position => message]` diagnostic map: a second error at the same
/// position overwrites the first, which is why `error_count` can exceed the
/// number of entries.
fn diagnostics(list: &[(usize, String)]) -> Value {
    let mut a = Array::new();
    for (pos, msg) in list {
        a.set(ArrayKey::Int(*pos as i64), Value::string(msg.as_bytes()));
    }
    Value::Array(a)
}

// ---- argument coercion ----------------------------------------------------------

/// php's weak `string` parameter coercion for a method, with php's
/// diagnostics.
pub(crate) fn str_arg(
    ctx: &mut Ctx,
    who: &str,
    pos: u32,
    name: &str,
    v: &Value,
) -> Result<Vec<u8>, Unwind> {
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

/// php's weak `int` parameter coercion for a method.
pub(crate) fn int_arg(
    ctx: &mut Ctx,
    who: &str,
    pos: u32,
    name: &str,
    v: &Value,
) -> Result<i64, Unwind> {
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

/// An `?Object of class $class` parameter: `None` for `null` or an omitted
/// argument, php's `TypeError` for anything else.
pub(crate) fn obj_arg(
    ctx: &mut Ctx,
    who: &str,
    pos: u32,
    name: &str,
    class: &str,
    nullable: bool,
    v: Option<&Value>,
) -> Result<Option<Object>, Unwind> {
    let Some(v) = v else { return Ok(None) };
    let v = v.deref();
    let want = ctx.class_by_name(class.as_bytes());
    match (&*v, want) {
        (Value::Null | Value::Uninit, _) if nullable => Ok(None),
        (Value::Object(o), Some(cid)) if ctx.object_instanceof(o, cid) => Ok(Some(o.clone())),
        (other, _) => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type {}{class}, {} given",
            if nullable { "?" } else { "" },
            rphp_runtime::value_name(other)
        ))),
    }
}

/// The timezone every entry point without an explicit one works in. The
/// default lives in `funcs.rs`'s request slot, which only
/// `date_default_timezone_get()` can read from here.
pub(crate) fn default_tz(ctx: &mut Ctx) -> Tz {
    match super::funcs::date_default_timezone_get(ctx, &mut []) {
        Ok(v) => Tz::Id(v.to_php_string()),
        Err(_) => Tz::utc(),
    }
}

/// The wall clock `"now"` is read from. php's date objects carry
/// microseconds, so the fractional part is kept.
pub(crate) fn now() -> (i64, u32) {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_micros()),
        Err(e) => {
            let d = e.duration();
            (-(d.as_secs() as i64), 0)
        }
    }
}

// ---- registration ----------------------------------------------------------------

/// Procedural aliases this module provides. `date/funcs.rs` owns the rest of
/// `ext/date`'s function surface, and no name is registered twice.
pub(crate) static FUNCTIONS: &[NativeFn] = procedural::FUNCTIONS;

/// Register the whole object surface. The exception classes come first
/// because everything else throws them, `DateTimeZone` before the date
/// classes because their constructors take one, and `DateInterval` before
/// `DatePeriod`.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.0.class_by_name(b"DateTimeZone").is_some() {
        return;
    }
    register_exceptions(r);
    zone::register_classes(r);
    dt::register_classes(r);
    interval::register_classes(r);
    period::register_classes(r);
}

/// `ext/date`'s exception tree. The two roots are deliberately different:
/// the `DateError` side extends `Error` because it reports a programming
/// mistake, the `DateException` side extends `Exception` because it reports
/// bad *input*.
fn register_exceptions(r: &mut Registry) {
    r.class("DateError").extends("Error").finish();
    r.class("DateObjectError").extends("DateError").finish();
    r.class("DateRangeError").extends("DateError").finish();
    r.class("DateException").extends("Exception").finish();
    for name in [
        "DateInvalidTimeZoneException",
        "DateInvalidOperationException",
        "DateMalformedStringException",
        "DateMalformedIntervalStringException",
        "DateMalformedPeriodStringException",
    ] {
        r.class(name).extends("DateException").finish();
    }
}
