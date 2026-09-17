//! php-src `ext/standard/uniqid.c`, `microtime.c`, `hrtime.c` and the
//! sleep functions of `basic_functions.c`: `uniqid`, `microtime`,
//! `gettimeofday`, `hrtime`, `sleep`, `usleep`, `time_nanosleep`. `time()`
//! belongs to `ext/date` and moves to `date.rs` with S6; it lives here until
//! then because everything calls it.
//!
//! Wall-clock values are allowlisted as `timing` in the differential
//! suite. `hrtime` is monotonic but process-relative (std exposes no raw
//! `CLOCK_MONOTONIC` reading), so only differences are meaningful — which
//! is how php documents it.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("uniqid", 0, Some(2), uniqid),
    nf!("microtime", 0, Some(1), microtime),
    nf!("gettimeofday", 0, Some(1), gettimeofday),
    nf!("hrtime", 0, Some(1), hrtime),
    nf!("time", 0, Some(0), time),
    nf!("sleep", 1, Some(1), sleep),
    nf!("usleep", 1, Some(1), usleep),
    nf!("time_nanosleep", 2, Some(2), time_nanosleep),
];

/// `gettimeofday(2)`: seconds and microseconds since the epoch.
fn now() -> (i64, i64) {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    (d.as_secs() as i64, d.subsec_micros() as i64)
}

/// `uniqid(string $prefix = "", bool $more_entropy = false): string` —
/// `%s%08x%05x` of the current second and microsecond (spinning until the
/// microsecond changes, as php does, so consecutive ids differ), plus
/// `%.8F` of a random `[0, 10)` with `$more_entropy`.
pub(crate) fn uniqid(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let prefix = args.first().map(Value::to_php_bytes).unwrap_or_default();
    let more_entropy = args.get(1).is_some_and(Value::to_bool);
    static LAST: std::sync::Mutex<(i64, i64)> = std::sync::Mutex::new((0, 0));
    let (sec, usec) = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = now();
        while t == *last {
            t = now();
        }
        *last = t;
        t
    };
    let mut out = prefix;
    out.extend_from_slice(format!("{sec:08x}{usec:05x}").as_bytes());
    if more_entropy {
        let mut buf = [0u8; 8];
        let r = match std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf)) {
            Ok(()) => u64::from_le_bytes(buf) % (1 << 53),
            Err(_) => (sec as u64).wrapping_mul(6_364_136_223_846_793_005).wrapping_add(usec as u64) % (1 << 53),
        };
        let lcg = r as f64 / (1u64 << 53) as f64;
        out.extend_from_slice(format!("{:.8}", lcg * 10.0).as_bytes());
    }
    Ok(Value::string(&out))
}

/// `microtime(bool $as_float = false): string|float` — `"0.12345600 1700000000"`
/// or the float seconds.
pub(crate) fn microtime(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (sec, usec) = now();
    if args.first().is_some_and(Value::to_bool) {
        return Ok(Value::Float(sec as f64 + usec as f64 / 1_000_000.0));
    }
    Ok(Value::string(format!("{:.8} {sec}", usec as f64 / 1_000_000.0).as_bytes()))
}

/// `gettimeofday(bool $as_float = false): array|float` — `sec`, `usec`,
/// `minuteswest`, `dsttime` (UTC: 0 / 0).
pub(crate) fn gettimeofday(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (sec, usec) = now();
    if args.first().is_some_and(Value::to_bool) {
        return Ok(Value::Float(sec as f64 + usec as f64 / 1_000_000.0));
    }
    let mut a = Array::new();
    a.set(ArrayKey::str(b"sec"), Value::Int(sec));
    a.set(ArrayKey::str(b"usec"), Value::Int(usec));
    a.set(ArrayKey::str(b"minuteswest"), Value::Int(0));
    a.set(ArrayKey::str(b"dsttime"), Value::Int(0));
    Ok(Value::Array(a))
}

/// `hrtime(bool $as_number = false): array|int|float|false` — monotonic
/// `[seconds, nanoseconds]` or total nanoseconds.
pub(crate) fn hrtime(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    static BASE: OnceLock<Instant> = OnceLock::new();
    let elapsed = BASE.get_or_init(Instant::now).elapsed();
    if args.first().is_some_and(Value::to_bool) {
        return Ok(Value::Int(elapsed.as_nanos().min(i64::MAX as u128) as i64));
    }
    let mut a = Array::new();
    a.push(Value::Int(elapsed.as_secs() as i64));
    a.push(Value::Int(elapsed.subsec_nanos() as i64));
    Ok(Value::Array(a))
}

/// `time(): int`
pub(crate) fn time(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(now().0))
}

/// `sleep(int $seconds): int` — 0 (the remaining time on interruption is
/// not modelled).
pub(crate) fn sleep(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let secs = args[0].to_int();
    if secs < 0 {
        return Err(Unwind::value_error("sleep(): Argument #1 ($seconds) must be greater than or equal to 0"));
    }
    std::thread::sleep(Duration::from_secs(secs as u64));
    Ok(Value::Int(0))
}

/// `usleep(int $microseconds): void`
pub(crate) fn usleep(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let us = args[0].to_int();
    if us < 0 {
        return Err(Unwind::value_error("usleep(): Argument #1 ($microseconds) must be greater than or equal to 0"));
    }
    std::thread::sleep(Duration::from_micros(us as u64));
    Ok(Value::Null)
}

/// `time_nanosleep(int $seconds, int $nanoseconds): array|bool` — `true`
/// after an uninterrupted sleep.
pub(crate) fn time_nanosleep(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let secs = args[0].to_int();
    let nanos = args[1].to_int();
    if secs < 0 {
        return Err(Unwind::value_error("time_nanosleep(): Argument #1 ($seconds) must be greater than or equal to 0"));
    }
    if nanos < 0 {
        return Err(Unwind::value_error("time_nanosleep(): Argument #2 ($nanoseconds) must be greater than or equal to 0"));
    }
    if nanos > 999_999_999 {
        return Err(Unwind::value_error("time_nanosleep(): Argument #2 ($nanoseconds) must be less than 1000000000"));
    }
    std::thread::sleep(Duration::new(secs as u64, nanos as u32));
    Ok(Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::call_named;

    #[test]
    fn ids_and_clocks_have_php_shapes() {
        let a = call_named(b"uniqid", &[]).to_php_bytes();
        let b = call_named(b"uniqid", &[]).to_php_bytes();
        assert_eq!(a.len(), 13);
        assert_ne!(a, b);
        assert_eq!(call_named(b"uniqid", &[Value::string(b"p_"), Value::Bool(true)]).to_php_bytes().len(), 25);
        let mt = call_named(b"microtime", &[]).to_php_string();
        let (frac, sec) = mt.split_once(' ').unwrap();
        assert_eq!(frac.len(), 10);
        assert!(sec.parse::<i64>().unwrap() > 1_600_000_000);
        assert!(matches!(call_named(b"microtime", &[Value::Bool(true)]), Value::Float(f) if f > 1.6e9));
        assert!(matches!(call_named(b"time", &[]), Value::Int(t) if t > 1_600_000_000));
        let Value::Array(hr) = call_named(b"hrtime", &[]) else { panic!() };
        assert_eq!(hr.len(), 2);
        assert!(matches!(call_named(b"hrtime", &[Value::Bool(true)]), Value::Int(_)));
    }
}
