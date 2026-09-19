//! ext/standard's address helpers (php-src `ext/standard/basic_functions.c`
//! and `network.c`): the two that convert an IPv4 address to and from its
//! 32-bit form, and the two that convert either family to and from the
//! packed bytes `inet_pton(3)` produces.
//!
//! php's `ip2long()` is `inet_pton(AF_INET)`, which is *strict*: every one
//! of the four parts must be a decimal number below 256, so `'256.1.1.1'`,
//! `'1.2.3'` and `'::1'` are all `false`. `long2ip()` takes the number
//! modulo 2^32, which is how `-1` becomes `255.255.255.255`.
//!
//! **Known divergence.** php hands `inet_pton()` to the platform's own
//! resolver, so a **scoped** IPv6 address (`fe80::1%lo0`) is whatever the C
//! library makes of it — macOS accepts it and answers `fe80:1::1`. Here the
//! parser is Rust's, which refuses the zone suffix.

use std::net::{Ipv4Addr, Ipv6Addr};

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::Value;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("ip2long", 1, Some(1), ip2long),
    nf!("long2ip", 1, Some(1), long2ip),
    nf!("inet_pton", 1, Some(1), inet_pton),
    nf!("inet_ntop", 1, Some(1), inet_ntop),
];

/// Constants this module provides: none — the address family constants
/// belong to the socket extension.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}

/// A `string` parameter.
fn str_arg(v: &Value, func: &str, name: &str) -> Result<Vec<u8>, Unwind> {
    match v.deref().as_ref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => {
            Err(Unwind::type_error(format!(
                "{func}(): Argument #1 (${name}) must be of type string, {} given",
                v.type_name()
            )))
        }
        _ => Ok(v.to_php_bytes()),
    }
}

/// Parse a dotted-quad the way `inet_pton(AF_INET)` does: four decimal
/// parts, each at most three digits and below 256, and nothing else.
fn dotted_quad(s: &[u8]) -> Option<Ipv4Addr> {
    let text = std::str::from_utf8(s).ok()?;
    let mut parts = [0u8; 4];
    let mut seen = 0;
    for (slot, part) in parts.iter_mut().zip(text.split('.')) {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse::<u8>().ok()?;
        seen += 1;
    }
    if seen != 4 || text.split('.').count() != 4 {
        return None;
    }
    Some(Ipv4Addr::from(parts))
}

/// `ip2long(string $ip): int|false`
fn ip2long(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "ip2long", "ip")?;
    Ok(match dotted_quad(&s) {
        Some(a) => Value::Int(i64::from(u32::from(a))),
        None => Value::Bool(false),
    })
}

/// `long2ip(int $ip): string` — php wraps the number into 32 bits, so a
/// negative one is its two's complement address.
fn long2ip(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let n = args[0].deref().to_int() as u32;
    Ok(Value::string(Ipv4Addr::from(n).to_string().as_bytes()))
}

/// `inet_pton(string $ip): string|false` — four packed bytes for IPv4,
/// sixteen for IPv6.
fn inet_pton(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "inet_pton", "ip")?;
    if let Some(a) = dotted_quad(&s) {
        return Ok(Value::string(&a.octets()));
    }
    if let Some(a) = std::str::from_utf8(&s)
        .ok()
        .and_then(|t| t.parse::<Ipv6Addr>().ok())
    {
        return Ok(Value::string(&a.octets()));
    }
    ctx.warn("inet_pton(): Unrecognized address")?;
    Ok(Value::Bool(false))
}

/// `inet_ntop(string $ip): string|false` — the address the packed bytes
/// spell, IPv4 for four of them and IPv6 for sixteen.
fn inet_ntop(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "inet_ntop", "ip")?;
    let text = match s.len() {
        4 => Ipv4Addr::from([s[0], s[1], s[2], s[3]]).to_string(),
        16 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(&s);
            Ipv6Addr::from(o).to_string()
        }
        _ => {
            ctx.warn("inet_ntop(): Invalid in_addr value")?;
            return Ok(Value::Bool(false));
        }
    };
    Ok(Value::string(text.as_bytes()))
}
