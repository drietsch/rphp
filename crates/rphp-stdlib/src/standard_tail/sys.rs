//! System and network lookups of ext/standard: services and protocols
//! (`basic_functions.c`), `nl_langinfo` (`string.c`), `mail()` (`mail.c`),
//! resource usage and priority (`microtime.c`, `proc_open.c`), `ftok`,
//! `lchown`/`lchgrp` (`filestat.c`), `net_get_interfaces` (`net.c`) and
//! the DNS functions (`dns.c`).
//!
//! * The services and protocols databases are read from `/etc/services`
//!   and `/etc/protocols` the way the C library's file backend walks them:
//!   first match wins, names and aliases compare case-sensitively, an empty
//!   protocol matches nothing.
//! * The DNS functions carry their own stub resolver in place of
//!   `res_nsearch(3)`: `/etc/resolv.conf`'s `nameserver`, `search`/`domain`
//!   and `options ndots/timeout/attempts`, the BIND search order, UDP with
//!   a TCP retry on truncation, and `dn_expand`'s name spelling. A name
//!   that cannot be encoded (an empty or over-long label) is "host not
//!   found" without a query, as macOS reports it.
//! * `nl_langinfo()` knows the C locale only — the one locale rphp's
//!   `setlocale()` models (`CODESET` follows `LC_CTYPE`: `C.UTF-8` is
//!   `UTF-8`, `C`/`POSIX` is `US-ASCII`).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

use crate::filestat::{arg_path, clear_stat_cache, io_text};

fn s(b: &[u8]) -> Value {
    Value::Str(Str::new(b))
}

fn key(k: &str) -> ArrayKey {
    ArrayKey::str(k.as_bytes())
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ---- services and protocols -------------------------------------------------

/// The fields of one database line, the comment cut off.
fn db_lines(path: &str) -> Vec<Vec<Vec<u8>>> {
    let Ok(data) = std::fs::read(path) else { return Vec::new() };
    data.split(|&b| b == b'\n')
        .map(|line| {
            let line = match line.iter().position(|&b| b == b'#') {
                Some(i) => &line[..i],
                None => line,
            };
            line.split(|b| b.is_ascii_whitespace()).filter(|f| !f.is_empty()).map(<[u8]>::to_vec).collect()
        })
        .filter(|f: &Vec<Vec<u8>>| f.len() >= 2)
        .collect()
}

/// A services line: name, port, protocol, aliases.
type Service = (Vec<u8>, u16, Vec<u8>, Vec<Vec<u8>>);

/// The services database, in file order.
fn services() -> Vec<Service> {
    db_lines("/etc/services")
        .into_iter()
        .filter_map(|mut f| {
            let pp = f[1].clone();
            let slash = pp.iter().position(|&b| b == b'/')?;
            let port: u16 = std::str::from_utf8(&pp[..slash]).ok()?.parse().ok()?;
            let aliases = f.split_off(2);
            Some((f.swap_remove(0), port, pp[slash + 1..].to_vec(), aliases))
        })
        .collect()
}

/// `getservbyname(string $service, string $protocol): int|false`
pub(super) fn getservbyname(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    let proto = args[1].to_php_bytes();
    if proto.is_empty() {
        return Ok(Value::Bool(false));
    }
    for (n, port, p, aliases) in services() {
        if p == proto && (n == name || aliases.contains(&name)) {
            return Ok(Value::Int(i64::from(port)));
        }
    }
    Ok(Value::Bool(false))
}

/// `getservbyport(int $port, string $protocol): string|false` — the port
/// is taken modulo 2^16, as the C call's `htons((unsigned short) port)`.
pub(super) fn getservbyport(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let port = args[0].to_int() as u16;
    let proto = args[1].to_php_bytes();
    if proto.is_empty() {
        return Ok(Value::Bool(false));
    }
    for (n, p_port, p, _) in services() {
        if p_port == port && p == proto {
            return Ok(s(&n));
        }
    }
    Ok(Value::Bool(false))
}

/// A protocols line as `(name, number, aliases)`.
fn protocols() -> Vec<(Vec<u8>, i64, Vec<Vec<u8>>)> {
    db_lines("/etc/protocols")
        .into_iter()
        .filter_map(|mut f| {
            let num: i64 = std::str::from_utf8(&f[1]).ok()?.parse().ok()?;
            let aliases = f.split_off(2);
            Some((f.swap_remove(0), num, aliases))
        })
        .collect()
}

/// `getprotobyname(string $protocol): int|false`
pub(super) fn getprotobyname(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    for (n, num, aliases) in protocols() {
        if n == name || aliases.contains(&name) {
            return Ok(Value::Int(num));
        }
    }
    Ok(Value::Bool(false))
}

/// `getprotobynumber(int $protocol): string|false`
pub(super) fn getprotobynumber(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let want = args[0].to_int();
    for (n, num, _) in protocols() {
        if num == want {
            return Ok(s(&n));
        }
    }
    Ok(Value::Bool(false))
}

// ---- nl_langinfo ---------------------------------------------------------------

/// The `nl_item` constants of macOS `<langinfo.h>` php exports, with the
/// C locale's answer (`CODESET` is decided at call time).
const LANGINFO: &[(&str, i64, &str)] = &[
    ("CODESET", 0, ""),
    ("D_T_FMT", 1, "%a %b %e %H:%M:%S %Y"),
    ("D_FMT", 2, "%m/%d/%y"),
    ("T_FMT", 3, "%H:%M:%S"),
    ("T_FMT_AMPM", 4, "%I:%M:%S %p"),
    ("AM_STR", 5, "AM"),
    ("PM_STR", 6, "PM"),
    ("DAY_1", 7, "Sunday"),
    ("DAY_2", 8, "Monday"),
    ("DAY_3", 9, "Tuesday"),
    ("DAY_4", 10, "Wednesday"),
    ("DAY_5", 11, "Thursday"),
    ("DAY_6", 12, "Friday"),
    ("DAY_7", 13, "Saturday"),
    ("ABDAY_1", 14, "Sun"),
    ("ABDAY_2", 15, "Mon"),
    ("ABDAY_3", 16, "Tue"),
    ("ABDAY_4", 17, "Wed"),
    ("ABDAY_5", 18, "Thu"),
    ("ABDAY_6", 19, "Fri"),
    ("ABDAY_7", 20, "Sat"),
    ("MON_1", 21, "January"),
    ("MON_2", 22, "February"),
    ("MON_3", 23, "March"),
    ("MON_4", 24, "April"),
    ("MON_5", 25, "May"),
    ("MON_6", 26, "June"),
    ("MON_7", 27, "July"),
    ("MON_8", 28, "August"),
    ("MON_9", 29, "September"),
    ("MON_10", 30, "October"),
    ("MON_11", 31, "November"),
    ("MON_12", 32, "December"),
    ("ABMON_1", 33, "Jan"),
    ("ABMON_2", 34, "Feb"),
    ("ABMON_3", 35, "Mar"),
    ("ABMON_4", 36, "Apr"),
    ("ABMON_5", 37, "May"),
    ("ABMON_6", 38, "Jun"),
    ("ABMON_7", 39, "Jul"),
    ("ABMON_8", 40, "Aug"),
    ("ABMON_9", 41, "Sep"),
    ("ABMON_10", 42, "Oct"),
    ("ABMON_11", 43, "Nov"),
    ("ABMON_12", 44, "Dec"),
    ("ERA", 45, ""),
    ("ERA_D_FMT", 46, ""),
    ("ERA_D_T_FMT", 47, ""),
    ("ERA_T_FMT", 48, ""),
    ("ALT_DIGITS", 49, ""),
    ("RADIXCHAR", 50, "."),
    ("THOUSEP", 51, ""),
    ("YESEXPR", 52, "^[yY]"),
    ("NOEXPR", 53, "^[nN]"),
    ("YESSTR", 54, "yes"),
    ("NOSTR", 55, "no"),
    ("CRNCYSTR", 56, ""),
];

/// `nl_langinfo(int $item): string|false`
pub(super) fn nl_langinfo(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let item = args[0].to_int();
    let Some(&(name, _, text)) = LANGINFO.iter().find(|(_, v, _)| *v == item) else {
        ctx.warn(&format!("nl_langinfo(): Item '{item}' is not valid"))?;
        return Ok(Value::Bool(false));
    };
    if name == "CODESET" {
        let ctype = ctx.call_function(b"setlocale", &[Value::Int(2), Value::string(b"0")])?;
        let utf8 = ctype.to_php_bytes().to_ascii_uppercase().ends_with(b"UTF-8");
        return Ok(s(if utf8 { b"UTF-8" } else { b"US-ASCII" }));
    }
    Ok(s(text.as_bytes()))
}

// ---- process resources ----------------------------------------------------------

/// `getrusage(int $mode = 0): array|false` — `1` is the children's usage.
pub(super) fn getrusage(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    use nix::sys::resource::{getrusage as ru, UsageWho};
    let who = if args.first().map_or(0, Value::to_int) == 1 {
        UsageWho::RUSAGE_CHILDREN
    } else {
        UsageWho::RUSAGE_SELF
    };
    let Ok(u) = ru(who) else { return Ok(Value::Bool(false)) };
    let mut a = Array::new();
    let mut put = |k: &str, v: i64| a.set(key(k), Value::Int(v));
    put("ru_oublock", u.block_writes());
    put("ru_inblock", u.block_reads());
    put("ru_msgsnd", u.ipc_sends());
    put("ru_msgrcv", u.ipc_receives());
    put("ru_maxrss", u.max_rss());
    put("ru_ixrss", u.shared_integral());
    put("ru_idrss", u.unshared_data_integral());
    put("ru_minflt", u.minor_page_faults());
    put("ru_majflt", u.major_page_faults());
    put("ru_nsignals", u.signals());
    put("ru_nvcsw", u.voluntary_context_switches());
    put("ru_nivcsw", u.involuntary_context_switches());
    put("ru_nswap", u.full_swaps());
    let (ut, st) = (u.user_time(), u.system_time());
    put("ru_utime.tv_usec", ut.tv_usec() as i64);
    put("ru_utime.tv_sec", ut.tv_sec());
    put("ru_stime.tv_usec", st.tv_usec() as i64);
    put("ru_stime.tv_sec", st.tv_sec());
    Ok(Value::Array(a))
}

/// `proc_nice(int $priority): bool` — `nice(3)`: the increment is taken as
/// a C `int` and the result clamped to the scheduler's range.
pub(super) fn proc_nice(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    use rustix::process::{getpriority_process, setpriority_process};
    let inc = args[0].to_int() as i32;
    let cur = getpriority_process(None).unwrap_or(0);
    let want = cur.saturating_add(inc).clamp(-20, 19);
    match setpriority_process(None, want) {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) if e == rustix::io::Errno::PERM || e == rustix::io::Errno::ACCESS => {
            ctx.warn("proc_nice(): Only a super user may attempt to increase the priority of a process")?;
            Ok(Value::Bool(false))
        }
        Err(e) => {
            ctx.warn(&format!("proc_nice(): Cannot change process priority (errno {})", e.raw_os_error()))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `time_sleep_until(float $timestamp): bool`
pub(super) fn time_sleep_until(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let target = args[0].to_float();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs() as f64 + f64::from(d.subsec_micros()) / 1e6);
    if target < now {
        ctx.warn("time_sleep_until(): Argument #1 ($timestamp) must be greater than or equal to the current time")?;
        return Ok(Value::Bool(false));
    }
    std::thread::sleep(Duration::from_secs_f64(target - now));
    Ok(Value::Bool(true))
}

// ---- files -------------------------------------------------------------------------

/// `ftok(string $filename, string $project_id): int` — the BSD formula,
/// `id << 24 | (st_dev & 0xff) << 16 | (st_ino & 0xffff)`, as a `key_t`.
pub(super) fn ftok(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    use std::os::unix::fs::MetadataExt;
    let name = args[0].to_php_bytes();
    let proj = args[1].to_php_bytes();
    if name.is_empty() {
        return Err(Unwind::value_error("ftok(): Argument #1 ($filename) must not be empty"));
    }
    if proj.len() != 1 {
        return Err(Unwind::value_error("ftok(): Argument #2 ($project_id) must be a single character"));
    }
    let path = arg_path(ctx, &args[0]);
    match std::fs::metadata(&path) {
        Ok(m) => {
            let id = i32::from(proj[0] as i8);
            let k = (id << 24) | (((m.dev() & 0xff) as i32) << 16) | ((m.ino() & 0xffff) as i32);
            Ok(Value::Int(i64::from(k)))
        }
        Err(e) => {
            ctx.warn(&format!("ftok(): ftok() failed - {}", io_text(&e)))?;
            Ok(Value::Int(-1))
        }
    }
}

/// `lchown(string $filename, string|int $user): bool`
pub(super) fn lchown(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_link_owner(ctx, args, true)
}

/// `lchgrp(string $filename, string|int $group): bool`
pub(super) fn lchgrp(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_link_owner(ctx, args, false)
}

/// `chown`'s rules (a string is a name, an int an id) without following a
/// final symlink.
fn set_link_owner(ctx: &mut Ctx, args: &mut [Value], user: bool) -> NativeResult {
    let func = if user { "lchown" } else { "lchgrp" };
    let id = if matches!(&*args[1].deref(), Value::Str(_)) {
        let name = args[1].to_php_bytes();
        match crate::file2::lookup_id(&name, user) {
            Some(id) => id,
            None => {
                let what = if user { "uid" } else { "gid" };
                ctx.warn(&format!("{func}(): Unable to find {what} for {}", lossy(&name)))?;
                return Ok(Value::Bool(false));
            }
        }
    } else {
        args[1].to_int() as u32
    };
    let path = arg_path(ctx, &args[0]);
    let r = if user {
        std::os::unix::fs::lchown(&path, Some(id), None)
    } else {
        std::os::unix::fs::lchown(&path, None, Some(id))
    };
    clear_stat_cache(ctx);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            ctx.warn(&format!("{func}(): {}", io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

// ---- net_get_interfaces -------------------------------------------------------------

/// `net_get_interfaces(): array|false` — `getifaddrs(3)` grouped by
/// interface: every address under `unicast` with its flags and family, the
/// IPv4/IPv6 ones spelled out, and `up`.
pub(super) fn net_get_interfaces(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    use nix::net::if_::InterfaceFlags;
    use nix::sys::socket::{SockaddrLike, SockaddrStorage};
    let Ok(addrs) = nix::ifaddrs::getifaddrs() else { return Ok(Value::Bool(false)) };
    fn ntop(a: Option<&SockaddrStorage>) -> Option<String> {
        let a = a?;
        if let Some(v4) = a.as_sockaddr_in() {
            return Some(v4.ip().to_string());
        }
        a.as_sockaddr_in6().map(|v6| v6.ip().to_string())
    }
    let mut out = Array::new();
    let mut order: Vec<String> = Vec::new();
    let mut ifaces: std::collections::HashMap<String, (Array, bool)> = std::collections::HashMap::new();
    for ifa in addrs {
        let flags = ifa.flags;
        let entry = ifaces.entry(ifa.interface_name.clone()).or_insert_with(|| {
            order.push(ifa.interface_name.clone());
            (Array::new(), false)
        });
        let mut u = Array::new();
        u.set(key("flags"), Value::Int(i64::from(flags.bits())));
        if let Some(addr) = ifa.address.as_ref() {
            u.set(key("family"), Value::Int(i64::from(addr.family().map_or(0, |f| f as i32))));
            let bcast = if flags.contains(InterfaceFlags::IFF_BROADCAST) { ifa.broadcast.as_ref() } else { None };
            let ptp = if flags.contains(InterfaceFlags::IFF_POINTOPOINT) { ifa.destination.as_ref() } else { None };
            for (k, v) in [
                ("address", ntop(Some(addr))),
                ("netmask", ntop(ifa.netmask.as_ref())),
                ("broadcast", ntop(bcast)),
                ("ptp", ntop(ptp)),
            ] {
                if let Some(v) = v {
                    u.set(key(k), s(v.as_bytes()));
                }
            }
        }
        entry.0.push(Value::Array(u));
        entry.1 = flags.contains(InterfaceFlags::IFF_UP);
    }
    for name in order {
        let (unicast, up) = ifaces.remove(&name).unwrap_or_default();
        let mut i = Array::new();
        i.set(key("unicast"), Value::Array(unicast));
        i.set(key("up"), Value::Bool(up));
        out.set(key(&name), Value::Array(i));
    }
    Ok(Value::Array(out))
}

// ---- mail -----------------------------------------------------------------------------

/// The `mail.*` / sendmail directives with their `php -n` defaults.
const MAIL_INI: &[(&str, &str)] = &[
    ("SMTP", "localhost"),
    ("smtp_port", "25"),
    ("sendmail_from", ""),
    ("sendmail_path", "/usr/sbin/sendmail -t -i"),
    ("mail.add_x_header", "0"),
    ("mail.mixed_lf_and_crlf", "0"),
    ("mail.log", ""),
    ("mail.force_extra_parameters", ""),
];

/// The type name php's header messages use (`zend_zval_value_name`).
fn zval_type_name(v: &Value) -> String {
    rphp_runtime::value_name(v)
}

/// php's field-name check: printable ASCII, no `:`.
fn header_name_ok(name: &[u8]) -> bool {
    name.iter().all(|&c| (33..=126).contains(&c) && c != b':')
}

/// php's field-value check: a CRLF only as folding (followed by a space
/// or tab), no bare CR, LF or NUL.
fn check_header_value(key: &str, v: &[u8]) -> Result<(), Unwind> {
    let mut i = 0;
    while i < v.len() {
        match v[i] {
            b'\r' => {
                if v.get(i + 1) == Some(&b'\n') {
                    if matches!(v.get(i + 2), Some(b' ' | b'\t')) {
                        i += 3;
                        continue;
                    }
                    return Err(Unwind::value_error(format!(
                        "Header \"{key}\" contains CRLF characters that are used as a line separator and are not allowed in the header"
                    )));
                }
                return Err(Unwind::value_error(format!(
                    "Header \"{key}\" contains CR character that is not allowed in the header"
                )));
            }
            b'\n' => {
                return Err(Unwind::value_error(format!(
                    "Header \"{key}\" contains LF character that is not allowed in the header"
                )))
            }
            0 => {
                return Err(Unwind::value_error(format!(
                    "Header \"{key}\" contains NULL character that is not allowed in the header"
                )))
            }
            _ => i += 1,
        }
    }
    Ok(())
}

/// One `Name: value\r\n` (a string) or one per element (a list).
fn header_elem(out: &mut Vec<u8>, name: &[u8], v: &Value) -> Result<(), Unwind> {
    let k = lossy(name);
    match v {
        Value::Str(val) => {
            if !header_name_ok(name) {
                return Err(Unwind::value_error(format!("Header name \"{k}\" contains invalid characters")));
            }
            check_header_value(&k, val.as_bytes())?;
            out.extend_from_slice(name);
            out.extend_from_slice(b": ");
            out.extend_from_slice(val.as_bytes());
            out.extend_from_slice(b"\r\n");
            Ok(())
        }
        Value::Array(a) => header_elems(out, name, a),
        other => Err(Unwind::type_error(format!(
            "Header \"{k}\" must be of type array|string, {} given",
            zval_type_name(other)
        ))),
    }
}

fn header_elems(out: &mut Vec<u8>, name: &[u8], a: &Array) -> Result<(), Unwind> {
    let k = lossy(name);
    for (ik, iv) in a.iter() {
        if let ArrayKey::Str(sk) = ik {
            return Err(Unwind::type_error(format!(
                "Header \"{k}\" must only contain numeric keys, \"{}\" found",
                lossy(sk.as_bytes())
            )));
        }
        let iv = iv.deref().into_owned();
        if !matches!(iv, Value::Str(_)) {
            return Err(Unwind::type_error(format!(
                "Header \"{k}\" must only contain values of type string, {} found",
                zval_type_name(&iv)
            )));
        }
        header_elem(out, name, &iv)?;
    }
    Ok(())
}

/// `php_mail_build_headers`: an array of headers as one block.
fn build_headers(headers: &Array) -> Result<Vec<u8>, Unwind> {
    const SINGLE: &[&str] = &[
        "orig-date", "from", "sender", "reply-to", "cc", "bcc", "message-id", "in-reply-to", "references",
    ];
    let mut out = Vec::new();
    for (k, v) in headers.iter() {
        let name = match k {
            ArrayKey::Int(i) => return Err(Unwind::type_error(format!("Header name cannot be numeric, {i} given"))),
            ArrayKey::Str(s) => s.as_bytes().to_vec(),
        };
        let v = v.deref().into_owned();
        let lower = name.to_ascii_lowercase();
        if lower == b"to" {
            return Err(Unwind::value_error("The additional headers cannot contain the \"To\" header"));
        }
        if lower == b"subject" {
            return Err(Unwind::value_error("The additional headers cannot contain the \"Subject\" header"));
        }
        if let Some(target) = SINGLE.iter().find(|t| t.as_bytes() == lower.as_slice()) {
            match &v {
                Value::Str(_) => header_elem(&mut out, &name, &v)?,
                Value::Array(_) => {
                    return Err(Unwind::type_error(format!("Header \"{target}\" must be of type string, array given")))
                }
                other => {
                    return Err(Unwind::type_error(format!(
                        "Header \"{}\" must be of type array|string, {} given",
                        lossy(&name),
                        zval_type_name(other)
                    )))
                }
            }
        } else {
            header_elem(&mut out, &name, &v)?;
        }
    }
    Ok(out)
}

/// `To:` and `Subject:`: trailing whitespace off, control characters
/// spaced out except a folding CRLF.
fn sanitize_line(v: &[u8]) -> Vec<u8> {
    let mut r = v.to_vec();
    while r.last().is_some_and(|c| c.is_ascii_whitespace() || *c == 0x0b) {
        r.pop();
    }
    let mut i = 0;
    while i < r.len() {
        if r[i] < 0x20 || r[i] == 0x7f {
            if r[i] == b'\r' && r.get(i + 1) == Some(&b'\n') && matches!(r.get(i + 2), Some(b' ' | b'\t')) {
                i += 2;
                while matches!(r.get(i + 1), Some(b' ' | b'\t')) {
                    i += 1;
                }
                i += 1;
                continue;
            }
            r[i] = b' ';
        }
        i += 1;
    }
    r
}

/// `php_mail_detect_multiple_crlf`: a header block that starts badly or
/// holds an empty line.
fn malformed_headers(h: &[u8]) -> bool {
    if h.is_empty() {
        return false;
    }
    if h[0] < 33 || h[0] > 126 || h[0] == b':' {
        return true;
    }
    let at = |i: usize| h.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while i < h.len() {
        match h[i] {
            0 => break,
            b'\r' => {
                let n = at(i + 1);
                if n == 0 || n == b'\r' || (n == b'\n' && matches!(at(i + 2), 0 | b'\n' | b'\r')) {
                    return true;
                }
                i += 2;
            }
            b'\n' => {
                if matches!(at(i + 1), 0 | b'\r' | b'\n') {
                    return true;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    false
}

/// `mail(string $to, string $subject, string $message, array|string $additional_headers = [], string $additional_params = ""): bool`
pub(super) fn mail(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let to = sanitize_line(&args[0].to_php_bytes());
    let subject = sanitize_line(&args[1].to_php_bytes());
    let message = args[2].to_php_bytes();
    let headers: Option<Vec<u8>> = match args.get(3).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => {
            let mut h = build_headers(&a)?;
            while h.last().is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0 | 0x0b)) {
                h.pop();
            }
            Some(h)
        }
        Some(Value::Str(st)) => {
            let mut h = st.as_bytes().to_vec();
            while h.last().is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0 | 0x0b)) {
                h.pop();
            }
            Some(h)
        }
        Some(v @ (Value::Int(_) | Value::Float(_) | Value::Bool(_))) => Some(v.to_php_bytes().to_vec()),
        Some(other) => {
            return Err(Unwind::type_error(format!(
                "mail(): Argument #4 ($additional_headers) must be of type array|string, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let headers = headers.filter(|h| !h.is_empty());
    let force = ctx.ini_get("mail.force_extra_parameters").unwrap_or("").to_string();
    let extra = if !force.is_empty() {
        Some(ctx.call_function(b"escapeshellcmd", &[Value::string(force.as_bytes())])?.to_php_bytes().to_vec())
    } else {
        match args.get(4).map(Value::to_php_bytes) {
            Some(p) if !p.is_empty() => Some(ctx.call_function(b"escapeshellcmd", &[s(&p)])?.to_php_bytes().to_vec()),
            _ => None,
        }
    };
    let crlf = ctx.ini_get("mail.mixed_lf_and_crlf").is_some_and(rphp_runtime::parse_bool);
    let sep: &[u8] = if crlf { b"\n" } else { b"\r\n" };

    // mail.log
    let log = ctx.ini_get("mail.log").unwrap_or("").to_string();
    if !log.is_empty() && log != "syslog" {
        let file = ctx.current_file();
        let line = ctx.current_line();
        let date = ctx.call_function(b"date", &[Value::string(b"d-M-Y H:i:s e")])?.to_php_bytes().to_vec();
        let hdr: Vec<u8> = headers
            .as_deref()
            .unwrap_or(b"")
            .iter()
            .map(|&c| if c == b'\r' || c == b'\n' { b' ' } else { c })
            .collect();
        let mut entry = format!("[{}] mail() on [{file}:{line}]: To: ", lossy(&date)).into_bytes();
        entry.extend_from_slice(&to);
        entry.extend_from_slice(b" -- Headers: ");
        entry.extend_from_slice(&hdr);
        entry.extend_from_slice(b" -- Subject: ");
        entry.extend_from_slice(&subject);
        entry.push(b'\n');
        let p = arg_path(ctx, &Value::string(log.as_bytes()));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(&entry);
        }
        clear_stat_cache(ctx);
    }

    let mut hdr = headers;
    if ctx.ini_get("mail.add_x_header").is_some_and(rphp_runtime::parse_bool) {
        let file = ctx.current_file();
        let base = file.rsplit('/').next().unwrap_or(&file).to_string();
        let uid = ctx.call_function(b"getmyuid", &[])?.to_int();
        let mut x = format!("X-PHP-Originating-Script: {uid}:{base}").into_bytes();
        if let Some(h) = &hdr {
            x.extend_from_slice(sep);
            x.extend_from_slice(h);
        }
        hdr = Some(x);
    }
    if hdr.as_deref().is_some_and(malformed_headers) {
        ctx.warn("mail(): Multiple or malformed newlines found in additional_header")?;
        return Ok(Value::Bool(false));
    }
    let path = ctx.ini_get("sendmail_path").unwrap_or("").as_bytes().to_vec();
    let mut cmd = path.clone();
    if let Some(e) = &extra {
        cmd.push(b' ');
        cmd.extend_from_slice(e);
    }
    let argv = vec![b"/bin/sh".to_vec(), b"-c".to_vec(), cmd];
    let req = crate::exec::SpawnRequest { function: "mail", argv: &argv, cwd: None };
    if !crate::exec::permitted(ctx, &req)? {
        return Ok(Value::Bool(false));
    }
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new("/bin/sh");
    c.arg0("sh")
        .arg("-c")
        .arg(std::ffi::OsStr::from_bytes(&argv[2]))
        .current_dir(&ctx.cwd)
        .stdin(std::process::Stdio::piped());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(_) => {
            ctx.warn(&format!("mail(): Could not execute mail delivery program '{}'", lossy(&path)))?;
            return Ok(Value::Bool(false));
        }
    };
    let mut body = Vec::new();
    body.extend_from_slice(b"To: ");
    body.extend_from_slice(&to);
    body.extend_from_slice(sep);
    body.extend_from_slice(b"Subject: ");
    body.extend_from_slice(&subject);
    body.extend_from_slice(sep);
    if let Some(h) = &hdr {
        body.extend_from_slice(h);
        body.extend_from_slice(sep);
    }
    body.extend_from_slice(sep);
    body.extend_from_slice(&message);
    body.extend_from_slice(sep);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&body);
    }
    let status = child.wait();
    clear_stat_cache(ctx);
    match status.ok().and_then(|st| st.code()) {
        Some(0) => Ok(Value::Bool(true)),
        Some(code) => {
            ctx.warn(&format!("mail(): Sendmail exited with non-zero exit code {code}"))?;
            Ok(Value::Bool(false))
        }
        None => Ok(Value::Bool(false)),
    }
}

// ---- DNS ------------------------------------------------------------------------------

const T_A: u16 = 1;
const T_NS: u16 = 2;
const T_CNAME: u16 = 5;
const T_SOA: u16 = 6;
const T_PTR: u16 = 12;
const T_HINFO: u16 = 13;
const T_MX: u16 = 15;
const T_TXT: u16 = 16;
const T_AAAA: u16 = 28;
const T_SRV: u16 = 33;
const T_NAPTR: u16 = 35;
const T_A6: u16 = 38;
const T_ANY: u16 = 255;
const T_CAA: u16 = 257;

const DNS_A: i64 = 1;
const DNS_NS: i64 = 2;
const DNS_CNAME: i64 = 16;
const DNS_SOA: i64 = 32;
const DNS_PTR: i64 = 2048;
const DNS_HINFO: i64 = 4096;
const DNS_CAA: i64 = 8192;
const DNS_MX: i64 = 16384;
const DNS_TXT: i64 = 32768;
const DNS_A6: i64 = 16777216;
const DNS_SRV: i64 = 33554432;
const DNS_NAPTR: i64 = 67108864;
const DNS_AAAA: i64 = 134217728;
const DNS_ANY: i64 = 268435456;
const DNS_ALL: i64 = DNS_A
    | DNS_NS
    | DNS_CNAME
    | DNS_SOA
    | DNS_PTR
    | DNS_HINFO
    | DNS_CAA
    | DNS_MX
    | DNS_TXT
    | DNS_A6
    | DNS_SRV
    | DNS_NAPTR
    | DNS_AAAA;

/// `h_errno` after a failed search.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DnsErr {
    HostNotFound,
    TryAgain,
    NoRecovery,
    NoData,
}

/// What `res_ninit` reads from `/etc/resolv.conf`.
struct Resolver {
    servers: Vec<SocketAddr>,
    search: Vec<Vec<u8>>,
    ndots: usize,
    timeout: Duration,
    attempts: u32,
}

fn resolver() -> Resolver {
    let mut r = Resolver { servers: Vec::new(), search: Vec::new(), ndots: 1, timeout: Duration::from_secs(5), attempts: 2 };
    let data = std::fs::read("/etc/resolv.conf").unwrap_or_default();
    for line in data.split(|&b| b == b'\n') {
        let line = lossy(line);
        let mut f = line.split_whitespace();
        match f.next() {
            Some("nameserver") => {
                if let Some(ip) = f.next().and_then(|a| a.split('%').next()?.parse::<IpAddr>().ok()) {
                    if r.servers.len() < 3 {
                        r.servers.push(SocketAddr::new(ip, 53));
                    }
                }
            }
            Some("domain") => {
                if let Some(d) = f.next() {
                    r.search = vec![d.as_bytes().to_vec()];
                }
            }
            Some("search") => r.search = f.map(|d| d.as_bytes().to_vec()).collect(),
            Some("options") => {
                for o in f {
                    if let Some(n) = o.strip_prefix("ndots:").and_then(|n| n.parse::<usize>().ok()) {
                        r.ndots = n.min(15);
                    } else if let Some(n) = o.strip_prefix("timeout:").and_then(|n| n.parse::<u64>().ok()) {
                        r.timeout = Duration::from_secs(n.clamp(1, 30));
                    } else if let Some(n) = o.strip_prefix("attempts:").and_then(|n| n.parse::<u32>().ok()) {
                        r.attempts = n.clamp(1, 5);
                    }
                }
            }
            _ => {}
        }
    }
    if r.servers.is_empty() {
        r.servers.push(SocketAddr::from(([127, 0, 0, 1], 53)));
    }
    r
}

/// A query id that differs call to call.
fn query_id() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish() as u16
}

/// `res_mkquery`: `None` for a name that cannot be encoded.
fn make_query(id: u16, name: &[u8], qtype: u16) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(32 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    let name = name.strip_suffix(b".").unwrap_or(name);
    if !name.is_empty() {
        for label in name.split(|&b| b == b'.') {
            if label.is_empty() || label.len() > 63 {
                return None;
            }
            q.push(label.len() as u8);
            q.extend_from_slice(label);
        }
    }
    q.push(0);
    if q.len() - 12 > 255 {
        return None;
    }
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes());
    Some(q)
}

/// `res_send`: the answer packet, or `None` when no server replied.
fn send(r: &Resolver, q: &[u8]) -> Option<Vec<u8>> {
    let id = &q[..2];
    for _ in 0..r.attempts {
        for &srv in &r.servers {
            let bind: SocketAddr = if srv.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { (std::net::Ipv6Addr::UNSPECIFIED, 0).into() };
            let Ok(sock) = UdpSocket::bind(bind) else { continue };
            if sock.connect(srv).is_err() || sock.set_read_timeout(Some(r.timeout)).is_err() {
                continue;
            }
            if sock.send(q).is_err() {
                continue;
            }
            let mut buf = vec![0u8; 65536];
            let reply = loop {
                match sock.recv(&mut buf) {
                    Ok(n) if n >= 12 && &buf[..2] == id => break Some(buf[..n].to_vec()),
                    Ok(_) => continue,
                    Err(_) => break None,
                }
            };
            let Some(reply) = reply else { continue };
            if reply[2] & 0x02 != 0 {
                if let Some(full) = send_tcp(srv, q, r.timeout) {
                    return Some(full);
                }
            }
            return Some(reply);
        }
    }
    None
}

/// The query over TCP (a truncated UDP answer's retry).
fn send_tcp(srv: SocketAddr, q: &[u8], timeout: Duration) -> Option<Vec<u8>> {
    let mut t = TcpStream::connect_timeout(&srv, timeout).ok()?;
    t.set_read_timeout(Some(timeout)).ok()?;
    let mut msg = (q.len() as u16).to_be_bytes().to_vec();
    msg.extend_from_slice(q);
    t.write_all(&msg).ok()?;
    let mut len = [0u8; 2];
    t.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; usize::from(u16::from_be_bytes(len))];
    t.read_exact(&mut buf).ok()?;
    (buf.len() >= 12 && buf[..2] == q[..2]).then_some(buf)
}

/// `res_nquery`: a reply with answers, or the `h_errno` it maps to.
fn query(r: &Resolver, name: &[u8], qtype: u16) -> Result<Vec<u8>, DnsErr> {
    let Some(q) = make_query(query_id(), name, qtype) else { return Err(DnsErr::HostNotFound) };
    let Some(reply) = send(r, &q) else { return Err(DnsErr::TryAgain) };
    let rcode = reply[3] & 0x0f;
    let ancount = u16::from_be_bytes([reply[6], reply[7]]);
    match rcode {
        0 if ancount > 0 => Ok(reply),
        0 => Err(DnsErr::NoData),
        3 => Err(DnsErr::HostNotFound),
        2 => Err(DnsErr::TryAgain),
        _ => Err(DnsErr::NoRecovery),
    }
}

/// `res_nsearch`: the BIND search order over the `search` list.
fn search(r: &Resolver, name: &[u8], qtype: u16) -> Result<Vec<u8>, DnsErr> {
    let dots = name.iter().filter(|&&b| b == b'.').count();
    let trailing = name.last() == Some(&b'.');
    let mut saved: Option<DnsErr> = None;
    let mut tried_as_is = false;
    let (mut got_nodata, mut got_servfail) = (false, false);
    if dots >= r.ndots || trailing {
        match query(r, name, qtype) {
            Ok(a) => return Ok(a),
            Err(e) if trailing => return Err(e),
            Err(e) => saved = Some(e),
        }
        tried_as_is = true;
    }
    if !trailing {
        for d in &r.search {
            let mut full = name.to_vec();
            full.push(b'.');
            full.extend_from_slice(d);
            match query(r, &full, qtype) {
                Ok(a) => return Ok(a),
                Err(DnsErr::NoData) => got_nodata = true,
                Err(DnsErr::HostNotFound) => {}
                Err(DnsErr::TryAgain) => got_servfail = true,
                Err(_) => break,
            }
        }
    }
    if !tried_as_is && !trailing {
        match query(r, name, qtype) {
            Ok(a) => return Ok(a),
            Err(e) => {
                if saved.is_none() && !got_nodata && !got_servfail {
                    return Err(e);
                }
            }
        }
    }
    Err(saved.unwrap_or(if got_nodata {
        DnsErr::NoData
    } else if got_servfail {
        DnsErr::TryAgain
    } else {
        DnsErr::HostNotFound
    }))
}

/// `dn_expand`: the name at `pos` (following compression pointers) as
/// `ns_name_ntop` spells it (the root as the empty string), and the bytes it takes at `pos`.
fn dn_expand(msg: &[u8], pos: usize) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut p = pos;
    let mut used: Option<usize> = None;
    let mut hops = 0;
    loop {
        let len = *msg.get(p)?;
        match len & 0xc0 {
            0xc0 => {
                let target = (usize::from(len & 0x3f) << 8) | usize::from(*msg.get(p + 1)?);
                if used.is_none() {
                    used = Some(p + 2 - pos);
                }
                hops += 1;
                if hops > 128 || target >= msg.len() {
                    return None;
                }
                p = target;
            }
            0x00 => {
                if len == 0 {
                    // The root is "" (macOS `dn_expand`), not ".".
                    return Some((out, used.unwrap_or(p + 1 - pos)));
                }
                let label = msg.get(p + 1..p + 1 + usize::from(len))?;
                if !out.is_empty() {
                    out.push(b'.');
                }
                for &c in label {
                    match c {
                        b'.' | b'"' | b';' | b'\\' | b'(' | b')' | b'@' | b'$' => {
                            out.push(b'\\');
                            out.push(c);
                        }
                        0x21..=0x7e => out.push(c),
                        _ => out.extend_from_slice(format!("\\{c:03}").as_bytes()),
                    }
                }
                if out.len() > 1024 {
                    return None;
                }
                p += 1 + usize::from(len);
            }
            _ => return None,
        }
    }
}

/// `dn_skipname`: the bytes of the name at `pos`.
fn dn_skip(msg: &[u8], pos: usize) -> Option<usize> {
    let mut p = pos;
    loop {
        let len = *msg.get(p)?;
        match len & 0xc0 {
            0xc0 => return (p + 2 <= msg.len()).then_some(p + 2 - pos),
            0x00 if len == 0 => return Some(p + 1 - pos),
            0x00 => p += 1 + usize::from(len),
            _ => return None,
        }
    }
}

fn u16_at(m: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*m.get(p)?, *m.get(p + 1)?]))
}

fn u32_at(m: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*m.get(p)?, *m.get(p + 1)?, *m.get(p + 2)?, *m.get(p + 3)?]))
}

/// php's own IPv6 spelling (`php_parserr`'s loop: the first zero run
/// becomes `::`, later zero groups are `0`).
fn php_ipv6(groups: [u16; 8]) -> String {
    let mut out = String::new();
    let (mut have_break, mut in_break) = (false, false);
    for g in groups {
        if g != 0 {
            if !out.is_empty() {
                in_break = false;
                out.push(':');
            }
            out.push_str(&format!("{g:x}"));
        } else if !have_break {
            have_break = true;
            in_break = true;
            out.push(':');
        } else if !in_break {
            out.push_str(":0");
        }
    }
    if have_break && in_break {
        out.push(':');
    }
    out
}

/// `php_parserr`: one resource record at `cp`. Returns the record (when
/// it is kept) and where the next begins (`None` stops the walk).
fn parse_rr(msg: &[u8], cp: usize, want: u16, store: bool, raw: bool) -> (Option<Array>, Option<usize>) {
    let Some((name, n)) = dn_expand(msg, cp) else { return (None, None) };
    let mut cp = cp + n;
    if cp + 10 > msg.len() {
        return (None, None);
    }
    let rtype = u16_at(msg, cp).unwrap_or(0);
    let ttl = u32_at(msg, cp + 4).unwrap_or(0);
    let dlen = usize::from(u16_at(msg, cp + 8).unwrap_or(0));
    cp += 10;
    if cp + dlen > msg.len() || dlen == 0 {
        return (None, None);
    }
    let end = cp + dlen;
    if (want != T_ANY && rtype != want) || !store {
        return (None, Some(end));
    }
    let mut a = Array::new();
    a.set(key("host"), s(&name));
    a.set(key("class"), s(b"IN"));
    a.set(key("ttl"), Value::Int(i64::from(ttl)));
    if raw {
        a.set(key("type"), Value::Int(i64::from(rtype)));
        a.set(key("data"), s(&msg[cp..end]));
        return (Some(a), Some(end));
    }
    let d = &msg[cp..end];
    let set_type = |a: &mut Array, t: &[u8]| a.set(key("type"), s(t));
    let int = |a: &mut Array, k: &str, v: i64| a.set(key(k), Value::Int(v));
    let fail = (None, None);
    match rtype {
        T_A => {
            if d.len() < 4 {
                return fail;
            }
            set_type(&mut a, b"A");
            a.set(key("ip"), s(format!("{}.{}.{}.{}", d[0], d[1], d[2], d[3]).as_bytes()));
            (Some(a), Some(end))
        }
        T_MX | T_CNAME | T_NS | T_PTR => {
            let mut p = cp;
            if rtype == T_MX {
                let Some(pri) = u16_at(msg, p) else { return fail };
                set_type(&mut a, b"MX");
                int(&mut a, "pri", i64::from(pri));
                p += 2;
            }
            match rtype {
                T_CNAME => set_type(&mut a, b"CNAME"),
                T_NS => set_type(&mut a, b"NS"),
                T_PTR => set_type(&mut a, b"PTR"),
                _ => {}
            }
            let Some((target, n)) = dn_expand(msg, p) else { return fail };
            a.set(key("target"), s(&target));
            (Some(a), Some(p + n))
        }
        T_HINFO => {
            set_type(&mut a, b"HINFO");
            let mut p = cp;
            for k in ["cpu", "os"] {
                let Some(&n) = msg.get(p) else { return fail };
                let Some(v) = msg.get(p + 1..p + 1 + usize::from(n)) else { return fail };
                a.set(key(k), s(v));
                p += 1 + usize::from(n);
            }
            (Some(a), Some(p))
        }
        T_CAA => {
            set_type(&mut a, b"CAA");
            let Some(&flags) = msg.get(cp) else { return fail };
            int(&mut a, "flags", i64::from(flags));
            let Some(&n) = msg.get(cp + 1) else { return fail };
            let n = usize::from(n);
            let Some(tag) = msg.get(cp + 2..cp + 2 + n) else { return fail };
            a.set(key("tag"), s(tag));
            if dlen < n + 2 {
                return fail;
            }
            let vlen = dlen - n - 2;
            let Some(v) = msg.get(cp + 2 + n..cp + 2 + n + vlen) else { return fail };
            a.set(key("value"), s(v));
            (Some(a), Some(cp + 2 + n + vlen))
        }
        T_TXT => {
            set_type(&mut a, b"TXT");
            let mut whole = Vec::new();
            let mut entries = Array::new();
            let mut l1 = 0usize;
            while l1 < dlen {
                let mut n = usize::from(d[l1]);
                if l1 + n >= dlen {
                    n = dlen - (l1 + 1);
                }
                if n > 0 {
                    whole.extend_from_slice(&d[l1 + 1..l1 + 1 + n]);
                    entries.push(s(&d[l1 + 1..l1 + 1 + n]));
                }
                l1 += n + 1;
            }
            a.set(key("txt"), s(&whole));
            a.set(key("entries"), Value::Array(entries));
            (Some(a), Some(end))
        }
        T_SOA => {
            set_type(&mut a, b"SOA");
            let Some((mname, n)) = dn_expand(msg, cp) else { return fail };
            let p = cp + n;
            let Some((rname, n)) = dn_expand(msg, p) else { return fail };
            let p = p + n;
            a.set(key("mname"), s(&mname));
            a.set(key("rname"), s(&rname));
            if p + 20 > msg.len() {
                return fail;
            }
            for (i, k) in ["serial", "refresh", "retry", "expire", "minimum-ttl"].iter().enumerate() {
                int(&mut a, k, i64::from(u32_at(msg, p + 4 * i).unwrap_or(0)));
            }
            (Some(a), Some(p + 20))
        }
        T_AAAA => {
            if d.len() < 16 {
                return fail;
            }
            let mut g = [0u16; 8];
            for (i, gi) in g.iter_mut().enumerate() {
                *gi = u16::from_be_bytes([d[2 * i], d[2 * i + 1]]);
            }
            set_type(&mut a, b"AAAA");
            a.set(key("ipv6"), s(php_ipv6(g).as_bytes()));
            (Some(a), Some(cp + 16))
        }
        T_SRV => {
            if d.len() < 6 {
                return fail;
            }
            set_type(&mut a, b"SRV");
            int(&mut a, "pri", i64::from(u16_at(msg, cp).unwrap_or(0)));
            int(&mut a, "weight", i64::from(u16_at(msg, cp + 2).unwrap_or(0)));
            int(&mut a, "port", i64::from(u16_at(msg, cp + 4).unwrap_or(0)));
            let Some((target, n)) = dn_expand(msg, cp + 6) else { return fail };
            a.set(key("target"), s(&target));
            (Some(a), Some(cp + 6 + n))
        }
        T_NAPTR => {
            if d.len() < 4 {
                return fail;
            }
            set_type(&mut a, b"NAPTR");
            int(&mut a, "order", i64::from(u16_at(msg, cp).unwrap_or(0)));
            int(&mut a, "pref", i64::from(u16_at(msg, cp + 2).unwrap_or(0)));
            let mut p = cp + 4;
            for k in ["flags", "services", "regex"] {
                let Some(&n) = msg.get(p) else { return fail };
                let Some(v) = msg.get(p + 1..p + 1 + usize::from(n)) else { return fail };
                a.set(key(k), s(v));
                p += 1 + usize::from(n);
            }
            let Some((rep, n)) = dn_expand(msg, p) else { return fail };
            a.set(key("replacement"), s(&rep));
            (Some(a), Some(p + n))
        }
        // A6 (obsolete) and every other type are dropped, as php drops
        // the types it does not know.
        _ => (None, Some(end)),
    }
}

/// The `h_errno` warning `dns_get_record()` gives before `false`.
fn dns_failure(ctx: &mut Ctx, e: DnsErr) -> NativeResult {
    ctx.warn(match e {
        DnsErr::NoRecovery => "dns_get_record(): An unexpected server failure occurred.",
        DnsErr::TryAgain => "dns_get_record(): A temporary server error occurred.",
        _ => "dns_get_record(): DNS Query failed",
    })?;
    Ok(Value::Bool(false))
}

/// `dns_get_record(string $hostname, int $type = DNS_ANY, &$authoritative_name_servers = null, &$additional_records = null, bool $raw = false): array|false`
pub(super) fn dns_get_record(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let host = args[0].to_php_bytes();
    let type_param = args.get(1).map_or(DNS_ANY, Value::to_int);
    let want_ns = args.len() > 2;
    let want_ar = args.len() > 3;
    let raw = args.get(4).is_some_and(Value::to_bool);
    if !raw {
        if (type_param & !DNS_ALL) != 0 && type_param != DNS_ANY {
            return Err(Unwind::value_error("dns_get_record(): Argument #2 ($type) must be a DNS_* constant"));
        }
    } else if !(1..=0xffff).contains(&type_param) {
        return Err(Unwind::value_error(
            "dns_get_record(): Argument #2 ($type) must be between 1 and 65535 when argument #5 ($raw) is true",
        ));
    }
    let mut authns = Array::new();
    let mut addtl = Array::new();
    let mut result = Array::new();
    if want_ns {
        args[2] = Value::Array(Array::new());
    }
    if want_ar {
        args[3] = Value::Array(Array::new());
    }
    // (qtype, store answers)
    let mut steps: Vec<(u16, bool)> = Vec::new();
    if raw {
        steps.push((type_param as u16, true));
        if want_ar {
            steps.push((T_ANY, false));
        }
    } else if type_param == DNS_ANY {
        steps.push((T_ANY, true));
    } else {
        for (bit, t) in [
            (DNS_A, T_A),
            (DNS_NS, T_NS),
            (DNS_CNAME, T_CNAME),
            (DNS_SOA, T_SOA),
            (DNS_PTR, T_PTR),
            (DNS_HINFO, T_HINFO),
            (DNS_CAA, T_CAA),
            (DNS_MX, T_MX),
            (DNS_TXT, T_TXT),
            (DNS_A6, T_A6),
            (DNS_SRV, T_SRV),
            (DNS_NAPTR, T_NAPTR),
            (DNS_AAAA, T_AAAA),
        ] {
            if type_param & bit != 0 {
                steps.push((t, true));
            }
        }
        if want_ar {
            steps.push((T_ANY, false));
        }
    }
    if steps.is_empty() {
        return Ok(Value::Array(result));
    }
    let r = resolver();
    for (qtype, store) in steps {
        let msg = match search(&r, &host, qtype) {
            Ok(m) => m,
            Err(DnsErr::NoData | DnsErr::HostNotFound) => continue,
            Err(e) => return dns_failure(ctx, e),
        };
        let qd = u16_at(&msg, 4).unwrap_or(0);
        let an = u16_at(&msg, 6).unwrap_or(0);
        let ns = u16_at(&msg, 8).unwrap_or(0);
        let ar = u16_at(&msg, 10).unwrap_or(0);
        let mut cp = Some(12usize);
        for _ in 0..qd {
            let Some(n) = cp.and_then(|c| dn_skip(&msg, c)) else {
                ctx.warn("dns_get_record(): Unable to parse DNS data received")?;
                return Ok(Value::Bool(false));
            };
            cp = cp.map(|c| c + n + 4);
        }
        for _ in 0..an {
            let Some(c) = cp.filter(|&c| c < msg.len()) else { break };
            let (rec, next) = parse_rr(&msg, c, qtype, store, raw);
            if let Some(rec) = rec {
                result.push(Value::Array(rec));
            }
            cp = next;
        }
        if want_ns || want_ar {
            for _ in 0..ns {
                let Some(c) = cp.filter(|&c| c < msg.len()) else { break };
                let (rec, next) = parse_rr(&msg, c, T_ANY, want_ns, raw);
                if let Some(rec) = rec {
                    authns.push(Value::Array(rec));
                }
                cp = next;
            }
        }
        if want_ar {
            for _ in 0..ar {
                let Some(c) = cp.filter(|&c| c < msg.len()) else { break };
                let (rec, next) = parse_rr(&msg, c, T_ANY, true, raw);
                if let Some(rec) = rec {
                    addtl.push(Value::Array(rec));
                }
                cp = next;
            }
        }
    }
    if want_ns {
        args[2] = Value::Array(authns);
    }
    if want_ar {
        args[3] = Value::Array(addtl);
    }
    Ok(Value::Array(result))
}

/// The record type a `checkdnsrr()` type string names.
fn rr_type(func: &str, t: &[u8]) -> Result<u16, Unwind> {
    let t = t.to_ascii_uppercase();
    Ok(match t.as_slice() {
        b"A" => T_A,
        b"MX" => T_MX,
        b"NS" => T_NS,
        b"PTR" => T_PTR,
        b"ANY" => T_ANY,
        b"SOA" => T_SOA,
        b"CAA" => T_CAA,
        b"TXT" => T_TXT,
        b"CNAME" => T_CNAME,
        b"AAAA" => T_AAAA,
        b"SRV" => T_SRV,
        b"NAPTR" => T_NAPTR,
        b"A6" => T_A6,
        _ => {
            return Err(Unwind::value_error(format!(
                "{func}(): Argument #2 ($type) must be a valid DNS record type"
            )))
        }
    })
}

fn check_record(args: &mut [Value], func: &str) -> NativeResult {
    let host = args[0].to_php_bytes();
    if host.is_empty() {
        return Err(Unwind::value_error(format!("{func}(): Argument #1 ($hostname) must not be empty")));
    }
    let t = match args.get(1) {
        Some(v) => rr_type(func, &v.to_php_bytes())?,
        None => T_MX,
    };
    Ok(Value::Bool(search(&resolver(), &host, t).is_ok()))
}

/// `checkdnsrr(string $hostname, string $type = "MX"): bool`
pub(super) fn checkdnsrr(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    check_record(args, "checkdnsrr")
}

/// `dns_check_record()` — the alias, which names itself in its errors.
pub(super) fn dns_check_record(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    check_record(args, "dns_check_record")
}

/// `getmxrr(string $hostname, &$hosts, &$weights = null): bool`
pub(super) fn getmxrr(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let host = args[0].to_php_bytes();
    let want_w = args.len() > 2;
    args[1] = Value::Array(Array::new());
    if want_w {
        args[2] = Value::Array(Array::new());
    }
    let Ok(msg) = search(&resolver(), &host, T_MX) else { return Ok(Value::Bool(false)) };
    let mut hosts = Array::new();
    let mut weights = Array::new();
    let mut cp = 12usize;
    for _ in 0..u16_at(&msg, 4).unwrap_or(0) {
        let Some(n) = dn_skip(&msg, cp) else { return Ok(Value::Bool(false)) };
        cp += n + 4;
    }
    for _ in 0..u16_at(&msg, 6).unwrap_or(0) {
        if cp >= msg.len() {
            break;
        }
        let Some(n) = dn_skip(&msg, cp) else { return Ok(Value::Bool(false)) };
        cp += n;
        let (Some(rtype), Some(dlen)) = (u16_at(&msg, cp), u16_at(&msg, cp + 8)) else { break };
        cp += 10;
        if rtype != T_MX {
            cp += usize::from(dlen);
            continue;
        }
        let Some(weight) = u16_at(&msg, cp) else { break };
        cp += 2;
        let Some((name, n)) = dn_expand(&msg, cp) else { return Ok(Value::Bool(false)) };
        cp += n;
        hosts.push(s(&name));
        weights.push(Value::Int(i64::from(weight)));
    }
    let found = !hosts.is_empty();
    args[1] = Value::Array(hosts);
    if want_w {
        args[2] = Value::Array(weights);
    }
    Ok(Value::Bool(found))
}

pub(super) fn register_constants(r: &mut Registry) {
    for &(name, v, _) in LANGINFO {
        r.constant(name, Value::Int(v));
    }
    for (name, v) in [
        ("DNS_A", DNS_A),
        ("DNS_NS", DNS_NS),
        ("DNS_CNAME", DNS_CNAME),
        ("DNS_SOA", DNS_SOA),
        ("DNS_PTR", DNS_PTR),
        ("DNS_HINFO", DNS_HINFO),
        ("DNS_CAA", DNS_CAA),
        ("DNS_MX", DNS_MX),
        ("DNS_TXT", DNS_TXT),
        ("DNS_A6", DNS_A6),
        ("DNS_SRV", DNS_SRV),
        ("DNS_NAPTR", DNS_NAPTR),
        ("DNS_AAAA", DNS_AAAA),
        ("DNS_ANY", DNS_ANY),
        ("DNS_ALL", DNS_ALL),
    ] {
        r.constant(name, Value::Int(v));
    }
    for &(k, v) in MAIL_INI {
        r.interp().ini.register(k, v);
    }
}
