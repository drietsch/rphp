//! `date_sun_info()`, `date_sunrise()` and `date_sunset()`: php-src
//! `ext/date/lib/astro.c` (timelib's port of Paul Schlyter's `sunriset.c`)
//! and the two PHP functions in `php_date.c` over it.
//!
//! The arithmetic is kept in php's order, constant for constant, so the
//! doubles come out bit-identical: `date_sunset(…, SUNFUNCS_RET_DOUBLE, …)`
//! prints all seventeen digits.

use rphp_runtime::{Ctx, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use super::civil;
use super::format;
use super::funcs::default_tz;
use super::tz::Tz;

// astro.c spells PI as 3.1415926535897932384, which is this double.
use std::f64::consts::PI;
const RADEG: f64 = 180.0 / PI;
const DEGRAD: f64 = PI / 180.0;
const INV360: f64 = 1.0 / 360.0;

fn sind(x: f64) -> f64 {
    (x * DEGRAD).sin()
}

fn cosd(x: f64) -> f64 {
    (x * DEGRAD).cos()
}

fn atan2d(y: f64, x: f64) -> f64 {
    RADEG * y.atan2(x)
}

fn acosd(x: f64) -> f64 {
    RADEG * x.acos()
}

/// An angle reduced to `[0, 360)`.
fn revolution(x: f64) -> f64 {
    x - 360.0 * (x * INV360).floor()
}

/// An angle reduced to `[-180, 180)`.
fn rev180(x: f64) -> f64 {
    x - 360.0 * (x * INV360 + 0.5).floor()
}

/// Greenwich mean sidereal time at 0h UT, in degrees.
fn gmst0(d: f64) -> f64 {
    revolution((180.0 + 356.0470 + 282.9404) + (0.9856002585 + 4.70935E-5) * d)
}

/// The Sun's ecliptic longitude and distance at day `d`.
fn sunpos(d: f64) -> (f64, f64) {
    let m = revolution(356.0470 + 0.9856002585 * d);
    let w = 282.9404 + 4.70935E-5 * d;
    let e = 0.016709 - 1.151E-9 * d;
    let ea = m + e * RADEG * sind(m) * (1.0 + e * cosd(m));
    let x = cosd(ea) - e;
    let y = (1.0 - e * e).sqrt() * sind(ea);
    let r = (x * x + y * y).sqrt();
    let v = atan2d(y, x);
    let mut lon = v + w;
    if lon >= 360.0 {
        lon -= 360.0;
    }
    (lon, r)
}

/// The Sun's right ascension, declination and distance at day `d`.
fn sun_ra_dec(d: f64) -> (f64, f64, f64) {
    let (lon, r) = sunpos(d);
    let x = r * cosd(lon);
    let y = r * sind(lon);
    let obl_ecl = 23.4393 - 3.563E-7 * d;
    let z = y * sind(obl_ecl);
    let y = y * cosd(obl_ecl);
    (atan2d(y, x), atan2d(z, (x * x + y * y).sqrt()), r)
}

/// What `timelib_astro_rise_set_altitude` answers.
struct RiseSet {
    /// -1: the Sun never reaches the altitude; +1: it never drops below it.
    rc: i32,
    h_rise: f64,
    h_set: f64,
    ts_rise: i64,
    ts_set: i64,
    ts_transit: i64,
}

/// `timelib_astro_rise_set_altitude()` for the local day `ts` falls on in
/// `zone`.
fn rise_set_altitude(zone: &Tz, ts: i64, lon: f64, lat: f64, mut altit: f64, upper_limb: bool) -> RiseSet {
    let local = format::render(zone, ts, 0);
    let days = civil::days_from_civil(local.year, local.month as i64, local.day as i64);
    // The local day at 12:00, and the same date at 00:00 UTC.
    let loc_noon = zone.resolve(days * 86_400 + 43_200, None);
    let utc0 = days * 86_400;
    // `timelib_ts_to_j2000(utc0) + 2 - lon / 360`: 12h local mean solar time.
    let julian = utc0 as f64 / 86_400.0 + 2_440_587.5;
    let d = (julian - 2_451_545.0) + 2.0 - lon / 360.0;
    let sidtime = revolution(gmst0(d) + 180.0 + lon);
    let (sra, sdec, sr) = sun_ra_dec(d);
    let tsouth = 12.0 - rev180(sidtime - sra) / 15.0;
    let sradius = 0.2666 / sr;
    if upper_limb {
        altit -= sradius;
    }
    let cost = (sind(altit) - sind(lat) * sind(sdec)) / (cosd(lat) * cosd(sdec));
    // C's double → `timelib_sll` conversions truncate toward zero.
    let ts_transit = (utc0 as f64 + tsouth * 3600.0) as i64;
    let (rc, t, ts_rise, ts_set) = if cost >= 1.0 {
        (-1, 0.0, ts_transit, ts_transit)
    } else if cost <= -1.0 {
        (1, 12.0, loc_noon - 12 * 3600, loc_noon + 12 * 3600)
    } else {
        let t = acosd(cost) / 15.0;
        (
            0,
            t,
            ((tsouth - t) * 3600.0 + utc0 as f64) as i64,
            ((tsouth + t) * 3600.0 + utc0 as f64) as i64,
        )
    };
    RiseSet {
        rc,
        h_rise: tsouth - t,
        h_set: tsouth + t,
        ts_rise,
        ts_set,
        ts_transit,
    }
}

/// `date_sun_info(int $timestamp, float $latitude, float $longitude): array`
pub(crate) fn date_sun_info(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ts = args[0].to_int();
    let lat = args[1].to_float();
    let lon = args[2].to_float();
    let zone = default_tz(ctx);
    let mut out = Array::new();
    let set = |out: &mut Array, key: &str, v: Value| out.set(ArrayKey::str(key.as_bytes()), v);
    let r = rise_set_altitude(&zone, ts, lon, lat, -35.0 / 60.0, true);
    match r.rc {
        -1 => {
            set(&mut out, "sunrise", Value::Bool(false));
            set(&mut out, "sunset", Value::Bool(false));
        }
        1 => {
            set(&mut out, "sunrise", Value::Bool(true));
            set(&mut out, "sunset", Value::Bool(true));
        }
        _ => {
            set(&mut out, "sunrise", Value::Int(r.ts_rise));
            set(&mut out, "sunset", Value::Int(r.ts_set));
        }
    }
    set(&mut out, "transit", Value::Int(r.ts_transit));
    for (altitude, begin, end) in [
        (-6.0, "civil_twilight_begin", "civil_twilight_end"),
        (-12.0, "nautical_twilight_begin", "nautical_twilight_end"),
        (-18.0, "astronomical_twilight_begin", "astronomical_twilight_end"),
    ] {
        let r = rise_set_altitude(&zone, ts, lon, lat, altitude, false);
        let (b, e) = match r.rc {
            -1 => (Value::Bool(false), Value::Bool(false)),
            1 => (Value::Bool(true), Value::Bool(true)),
            _ => (Value::Int(r.ts_rise), Value::Int(r.ts_set)),
        };
        set(&mut out, begin, b);
        set(&mut out, end, e);
    }
    Ok(Value::Array(out))
}

/// `date_sunrise(int $timestamp, int $returnFormat = SUNFUNCS_RET_STRING, ?float $latitude = null, ?float $longitude = null, ?float $zenith = null, ?float $utcOffset = null): string|int|float|false`
pub(crate) fn date_sunrise(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function date_sunrise() is deprecated since 8.1, use date_sun_info() instead")?;
    sunrise_sunset(ctx, args, false)
}

/// `date_sunset(…)`: [`date_sunrise`]'s twin.
pub(crate) fn date_sunset(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function date_sunset() is deprecated since 8.1, use date_sun_info() instead")?;
    sunrise_sunset(ctx, args, true)
}

/// A `date.*` float directive, read the way php's `INI_FLT` does.
fn ini_float(ctx: &Ctx, name: &str) -> f64 {
    Value::string(ctx.ini_get(name).unwrap_or("0").as_bytes()).to_float()
}

/// php's `php_do_date_sunrise_sunset`.
fn sunrise_sunset(ctx: &mut Ctx, args: &mut [Value], sunset: bool) -> NativeResult {
    let who = if sunset { "date_sunset" } else { "date_sunrise" };
    let opt = |i: usize| match args.get(i).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_float()),
    };
    let ts = args[0].to_int();
    let format = args.get(1).map_or(1, Value::to_int);
    let lat = opt(2).unwrap_or_else(|| ini_float(ctx, "date.default_latitude"));
    let lon = opt(3).unwrap_or_else(|| ini_float(ctx, "date.default_longitude"));
    let zenith = opt(4).unwrap_or_else(|| {
        ini_float(ctx, if sunset { "date.sunset_zenith" } else { "date.sunrise_zenith" })
    });
    let offset = opt(5);
    if !(0..=2).contains(&format) {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($returnFormat) must be one of SUNFUNCS_RET_TIMESTAMP, SUNFUNCS_RET_STRING, or SUNFUNCS_RET_DOUBLE"
        )));
    }
    let altitude = 90.0 - zenith;
    let zone = default_tz(ctx);
    // php reads the zone's offset off a fresh `timelib_time`, whose
    // timestamp is 0: the offset in force at the epoch, not at `$timestamp`.
    let gmt_offset = offset.unwrap_or_else(|| f64::from(zone.offset_at(0)) / 3600.0);
    let r = rise_set_altitude(&zone, ts, lon, lat, altitude, true);
    if r.rc != 0 {
        return Ok(Value::Bool(false));
    }
    if format == 0 {
        return Ok(Value::Int(if sunset { r.ts_set } else { r.ts_rise }));
    }
    let mut n = if sunset { r.h_set } else { r.h_rise } + gmt_offset;
    if !(0.0..=24.0).contains(&n) {
        n -= (n / 24.0).floor() * 24.0;
    }
    if format == 2 {
        return Ok(Value::Float(n));
    }
    let hours = n as i32;
    let minutes = (60.0 * (n - f64::from(hours))) as i32;
    Ok(Value::string(format!("{hours:02}:{minutes:02}").as_bytes()))
}
