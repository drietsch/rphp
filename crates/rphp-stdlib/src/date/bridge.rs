//! What another extension needs from `ext/date` without its internals:
//! the instant and zone a `DateTime` carries, the zone a `DateTimeZone`
//! carries, new objects of both, the request's default zone. ext/intl's
//! formatters and calendars take and return php's date objects.

use rphp_runtime::{Ctx, Unwind};
use rphp_value::{Object, Value};

use super::classes::{default_tz, dt_state, put_dt, put_zone, zone_state, DtState};
use super::tz::Tz;

/// A php zone as the other side sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Zone {
    /// A fixed offset in seconds east of UTC.
    Offset(i32),
    /// An abbreviation with its fixed offset.
    Abbr { name: String, offset: i32, dst: bool },
    /// An IANA identifier as the caller spelled it.
    Id(String),
}

impl From<Tz> for Zone {
    fn from(tz: Tz) -> Zone {
        match tz {
            Tz::Offset(o) => Zone::Offset(o),
            Tz::Abbr { name, offset, dst } => Zone::Abbr { name, offset, dst },
            Tz::Id(id) => Zone::Id(id),
        }
    }
}

impl From<Zone> for Tz {
    fn from(z: Zone) -> Tz {
        match z {
            Zone::Offset(o) => Tz::Offset(o),
            Zone::Abbr { name, offset, dst } => Tz::Abbr { name, offset, dst },
            Zone::Id(id) => Tz::Id(id),
        }
    }
}

/// The instant (seconds, microseconds) and zone of a `DateTimeInterface`
/// object.
pub fn datetime_instant(o: &Object) -> (i64, u32, Zone) {
    let st = dt_state(o);
    (st.ts, st.usec, st.tz.into())
}

/// The zone of a `DateTimeZone` object.
pub fn zone_of(o: &Object) -> Option<Zone> {
    zone_state(o).map(Zone::from)
}

/// `php`'s reading of a zone name (`Tz::parse`): an identifier, an
/// abbreviation or an offset; `None` when php's database has none.
pub fn parse_zone(name: &str) -> Option<Zone> {
    Tz::parse(name).map(Zone::from)
}

/// The zone's offset in seconds at `ts`.
pub fn zone_offset_at(zone: &Zone, ts: i64) -> i32 {
    Tz::from(zone.clone()).offset_at(ts)
}

/// The wall clock, in whole seconds since the epoch.
pub fn now_seconds() -> i64 {
    super::classes::now().0
}

/// The request's default zone (`date_default_timezone_get()`).
pub fn default_zone(ctx: &mut Ctx) -> Zone {
    default_tz(ctx).into()
}

/// A new `DateTimeZone` for a zone.
pub fn new_datetime_zone(ctx: &mut Ctx, zone: Zone) -> Result<Value, Unwind> {
    let cid = ctx.lookup_class_or_error(b"DateTimeZone")?;
    let obj = ctx.instantiate(cid);
    put_zone(&obj, zone.into());
    Ok(Value::Object(obj))
}

/// A new `DateTime` (or `DateTimeImmutable` when `immutable`) at an
/// instant in a zone.
pub fn new_datetime(ctx: &mut Ctx, ts: i64, usec: u32, zone: Zone, immutable: bool) -> Result<Value, Unwind> {
    let name: &[u8] = if immutable { b"DateTimeImmutable" } else { b"DateTime" };
    let cid = ctx.lookup_class_or_error(name)?;
    let obj = ctx.instantiate(cid);
    put_dt(&obj, DtState { ts, usec, tz: zone.into() });
    Ok(Value::Object(obj))
}
