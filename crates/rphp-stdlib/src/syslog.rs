//! php-src `ext/standard/syslog.c`: `openlog`, `syslog`, `closelog` and the
//! `LOG_*` constants.
//!
//! php hands these to `syslog(3)`; this crate forbids `unsafe`, so the
//! functions speak the wire form themselves — `<pri>ident[pid]: message`
//! over a datagram to the system logger's socket (`/var/run/syslog` on
//! macOS, `/dev/log` elsewhere), which is what libc does underneath. A
//! logger that is not listening loses the line, as it would for php; the
//! functions never fail (php's return `true`).

use std::os::unix::net::UnixDatagram;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry};
use rphp_value::Value;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("openlog", 3, Some(3), openlog),
    nf!("syslog", 2, Some(2), syslog),
    nf!("closelog", 0, Some(0), closelog),
];

const LOG_PID: i64 = 1;
const LOG_PERROR: i64 = 32;
const LOG_USER: i64 = 8;

/// `(name, value)` as php defines them on every platform it builds on.
const CONSTANTS: &[(&str, i64)] = &[
    ("LOG_EMERG", 0),
    ("LOG_ALERT", 1),
    ("LOG_CRIT", 2),
    ("LOG_ERR", 3),
    ("LOG_WARNING", 4),
    ("LOG_NOTICE", 5),
    ("LOG_INFO", 6),
    ("LOG_DEBUG", 7),
    ("LOG_KERN", 0),
    ("LOG_USER", LOG_USER),
    ("LOG_MAIL", 16),
    ("LOG_DAEMON", 24),
    ("LOG_AUTH", 32),
    ("LOG_SYSLOG", 40),
    ("LOG_LPR", 48),
    ("LOG_NEWS", 56),
    ("LOG_UUCP", 64),
    ("LOG_CRON", 72),
    ("LOG_AUTHPRIV", 80),
    ("LOG_LOCAL0", 128),
    ("LOG_LOCAL1", 136),
    ("LOG_LOCAL2", 144),
    ("LOG_LOCAL3", 152),
    ("LOG_LOCAL4", 160),
    ("LOG_LOCAL5", 168),
    ("LOG_LOCAL6", 176),
    ("LOG_LOCAL7", 184),
    ("LOG_PID", LOG_PID),
    ("LOG_CONS", 2),
    ("LOG_ODELAY", 4),
    ("LOG_NDELAY", 8),
    ("LOG_NOWAIT", 16),
    ("LOG_PERROR", LOG_PERROR),
];

pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in CONSTANTS {
        r.constant(name, Value::Int(*v));
    }
}

/// `openlog(string $prefix, int $flags, int $facility): true`
fn openlog(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.ext.syslog = (Some(args[0].to_php_bytes()), args[1].to_int(), args[2].to_int());
    Ok(Value::Bool(true))
}

/// `closelog(): true`
fn closelog(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.ext.syslog = (None, 0, 0);
    Ok(Value::Bool(true))
}

/// `syslog(int $priority, string $message): true`
fn syslog(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let priority = args[0].to_int();
    let message = args[1].to_php_bytes();
    let (ident, flags, facility) = ctx.ext.syslog.clone();
    // A priority without facility bits takes the one `openlog()` chose
    // (`LOG_USER` before any), as `syslog(3)` does.
    let pri = if priority & !7 == 0 {
        priority | if facility != 0 { facility } else { LOG_USER }
    } else {
        priority
    };
    let mut line = format!("<{pri}>").into_bytes();
    if let Some(ident) = &ident {
        line.extend_from_slice(ident);
        if flags & LOG_PID != 0 {
            line.extend_from_slice(format!("[{}]", std::process::id()).as_bytes());
        }
        line.extend_from_slice(b": ");
    }
    line.extend_from_slice(&message);
    if flags & LOG_PERROR != 0 {
        let mut err = std::io::stderr();
        let _ = std::io::Write::write_all(&mut err, &message);
        let _ = std::io::Write::write_all(&mut err, b"\n");
    }
    if let Ok(sock) = UnixDatagram::unbound() {
        for path in ["/var/run/syslog", "/dev/log"] {
            if sock.send_to(&line, path).is_ok() {
                break;
            }
        }
    }
    Ok(Value::Bool(true))
}
