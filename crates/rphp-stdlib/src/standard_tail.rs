//! The rest of ext/standard: the functions no other module carries —
//! `fnmatch`, `getopt`, `md5_file`/`sha1_file`, `str_increment`,
//! `forward_static_call` and friends. The system and network lookups
//! (`getservbyname`, DNS, `mail`, `getrusage`, …) live in [`sys`].

use rphp_runtime::{nf, nf_ref, Callable, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

mod strptime;
mod sys;

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    // ---- misc (this file) ---------------------------------------------------
    nf!("strchr", 2, Some(3), strchr),
    nf!("fnmatch", 2, Some(3), fnmatch),
    nf_ref!("getopt", 1, Some(3), 0b100, getopt),
    nf!("md5_file", 1, Some(2), md5_file),
    nf!("sha1_file", 1, Some(2), sha1_file),
    nf!("str_increment", 1, Some(1), str_increment),
    nf!("str_decrement", 1, Some(1), str_decrement),
    nf!("ini_parse_quantity", 1, Some(1), ini_parse_quantity),
    nf!("socket_get_status", 1, Some(1), socket_get_status),
    nf!("socket_set_blocking", 2, Some(2), socket_set_blocking),
    nf!("set_file_buffer", 2, Some(2), set_file_buffer),
    nf!("stream_supports_lock", 1, Some(1), stream_supports_lock),
    nf!("forward_static_call", 1, None, forward_static_call),
    nf!("forward_static_call_array", 2, Some(2), forward_static_call_array),
    nf!("assert_options", 1, Some(2), assert_options),
    nf!("realpath_cache_size", 0, Some(0), realpath_cache_size),
    nf!("realpath_cache_get", 0, Some(0), realpath_cache_get),
    nf!("get_browser", 0, Some(2), get_browser),
    nf!("output_add_rewrite_var", 2, Some(2), output_add_rewrite_var),
    nf!("output_reset_rewrite_vars", 0, Some(0), output_reset_rewrite_vars),
    nf!("request_parse_body", 0, Some(1), request_parse_body),
    nf!("register_tick_function", 1, None, register_tick_function),
    nf!("unregister_tick_function", 1, Some(1), unregister_tick_function),
    nf!("php_strip_whitespace", 1, Some(1), php_strip_whitespace),
    nf!("get_meta_tags", 1, Some(2), get_meta_tags),
    nf!("hebrev", 1, Some(2), hebrev),
    nf!("strptime", 2, Some(2), strptime),
    nf!("dl", 1, Some(1), dl),
    nf!("sys_getloadavg", 0, Some(0), sys_getloadavg),
    nf!("cli_set_process_title", 1, Some(1), cli_set_process_title),
    nf!("cli_get_process_title", 0, Some(0), cli_get_process_title),
    // ---- end misc -----------------------------------------------------------
    //
    //
    //
    // ---- system & network (sys.rs) ------------------------------------------
    nf!("getservbyname", 2, Some(2), sys::getservbyname),
    nf!("getservbyport", 2, Some(2), sys::getservbyport),
    nf!("getprotobyname", 1, Some(1), sys::getprotobyname),
    nf!("getprotobynumber", 1, Some(1), sys::getprotobynumber),
    nf!("nl_langinfo", 1, Some(1), sys::nl_langinfo),
    nf!("getrusage", 0, Some(1), sys::getrusage),
    nf!("proc_nice", 1, Some(1), sys::proc_nice),
    nf!("time_sleep_until", 1, Some(1), sys::time_sleep_until),
    nf!("ftok", 2, Some(2), sys::ftok),
    nf!("lchown", 2, Some(2), sys::lchown),
    nf!("lchgrp", 2, Some(2), sys::lchgrp),
    nf!("net_get_interfaces", 0, Some(0), sys::net_get_interfaces),
    nf!("mail", 3, Some(5), sys::mail),
    nf_ref!("dns_get_record", 1, Some(5), 0b1100, sys::dns_get_record),
    nf!("checkdnsrr", 1, Some(2), sys::checkdnsrr),
    nf!("dns_check_record", 1, Some(2), sys::dns_check_record),
    nf_ref!("getmxrr", 2, Some(3), 0b110, sys::getmxrr),
    nf_ref!("dns_get_mx", 2, Some(3), 0b110, sys::getmxrr),
    // ---- end system & network -----------------------------------------------
];

/// The message php's `smart_str_append_escaped` makes of `bytes`: `\n`,
/// `\r`, `\t`, `\f`, `\v`, `\\`, `\e` spelled out, other control and
/// high bytes as `\xHH`.
fn escaped(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &c in bytes {
        if c < 32 || c == b'\\' || c > 126 {
            s.push('\\');
            match c {
                b'\n' => s.push('n'),
                b'\r' => s.push('r'),
                b'\t' => s.push('t'),
                0x0c => s.push('f'),
                0x0b => s.push('v'),
                b'\\' => s.push('\\'),
                0x1b => s.push('e'),
                _ => s.push_str(&format!("x{c:02X}")),
            }
        } else {
            s.push(c as char);
        }
    }
    s
}

// ---- aliases -----------------------------------------------------------------

/// `strchr()` — php's alias of `strstr()`.
fn strchr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.call_function(b"strstr", args)
}

/// The open stream `$stream` of `func`, or php's `TypeError`.
fn stream_arg(func: &str, v: &Value) -> Result<rphp_value::Resource, Unwind> {
    match &*v.deref() {
        Value::Resource(r) if r.kind() == "stream" => Ok(r.clone()),
        Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be an open stream resource"
        ))),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// Call `target` for its alias `alias`: php reports errors under the name
/// the script called.
fn alias(ctx: &mut Ctx, alias: &str, target: &str, args: &mut [Value]) -> NativeResult {
    stream_arg(alias, &args[0])?;
    match ctx.call_function(target.as_bytes(), args) {
        Err(Unwind::Pending(mut p)) => {
            if let Some(rest) = p.message.strip_prefix(target) {
                p.message = format!("{alias}{rest}");
            }
            Err(Unwind::Pending(p))
        }
        r => r,
    }
}

/// `socket_get_status()` — php's alias of `stream_get_meta_data()`.
fn socket_get_status(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    alias(ctx, "socket_get_status", "stream_get_meta_data", args)
}

/// `socket_set_blocking()` — php's alias of `stream_set_blocking()`.
fn socket_set_blocking(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    alias(ctx, "socket_set_blocking", "stream_set_blocking", args)
}

/// `set_file_buffer()` — php's alias of `stream_set_write_buffer()`.
fn set_file_buffer(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    alias(ctx, "set_file_buffer", "stream_set_write_buffer", args)
}

/// `stream_supports_lock(resource $stream): bool` — whether `flock()` can
/// work on the stream: a file descriptor stream can; memory, temp,
/// sockets and directory handles cannot.
fn stream_supports_lock(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg("stream_supports_lock", &args[0])?;
    let v = args[0].deref().into_owned();
    // A directory handle has no stream metadata here, and cannot lock.
    let Ok(Value::Array(meta)) = ctx.call_function(b"stream_get_meta_data", &[v]) else {
        return Ok(Value::Bool(false));
    };
    let field = |k: &str| meta.get(&ArrayKey::str(k.as_bytes())).map(|v| v.to_php_bytes()).unwrap_or_default();
    // Every stdio-backed stream (files, pipes, the std handles) can lock.
    Ok(Value::Bool(field("stream_type") == b"STDIO"))
}

// ---- fnmatch -------------------------------------------------------------------

const FNM_NOESCAPE: i64 = 1;
const FNM_PATHNAME: i64 = 2;
const FNM_PERIOD: i64 = 4;
const FNM_LEADING_DIR: i64 = 8;
const FNM_CASEFOLD: i64 = 16;

/// php's `MAXPATHLEN`: the longest pattern `fnmatch()` accepts.
const MAXPATHLEN: usize = 1024;

/// `fnmatch(string $pattern, string $filename, int $flags = 0): bool`
fn fnmatch(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let pattern = args[0].to_php_bytes();
    let name = args[1].to_php_bytes();
    let flags = args.get(2).map_or(0, Value::to_int);
    if pattern.contains(&0) {
        return Err(Unwind::value_error("fnmatch(): Argument #1 ($pattern) must not contain any null bytes"));
    }
    if name.contains(&0) {
        return Err(Unwind::value_error("fnmatch(): Argument #2 ($filename) must not contain any null bytes"));
    }
    if pattern.len() >= MAXPATHLEN {
        ctx.warn(&format!(
            "fnmatch(): Pattern exceeds the maximum allowed length of {MAXPATHLEN} characters"
        ))?;
        return Ok(Value::Bool(false));
    }
    // In the C locale a byte above 0x7f is no character (`mbrtowc` fails
    // on it), and libc's matcher gives up on the pattern.
    if pattern.iter().any(|&b| b >= 0x80) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(fn_match(&pattern, &name, flags)))
}

/// The byte at `i`, or NUL past the end (C string semantics).
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

fn fold(c: u8, flags: i64) -> u8 {
    if flags & FNM_CASEFOLD != 0 {
        c.to_ascii_lowercase()
    } else {
        c
    }
}

/// The C library's `fnmatch(3)` (the BSD one macOS ships): `*`, `?`,
/// bracket expressions with ranges, negation and `[:class:]`, `\`
/// escapes, and the `FNM_*` flags.
pub(crate) fn fn_match(pattern: &[u8], string: &[u8], flags: i64) -> bool {
    let (mut p, mut s) = (0usize, 0usize);
    let mut bt: Option<(usize, usize)> = None;
    let period_at = |s: usize| {
        at(string, s) == b'.'
            && flags & FNM_PERIOD != 0
            && (s == 0 || (flags & FNM_PATHNAME != 0 && string[s - 1] == b'/'))
    };
    loop {
        let pc = at(pattern, p);
        p += 1;
        let sc = at(string, s);
        // `Some(true)`: matched, go on; `Some(false)`: mismatch, backtrack.
        let step: bool = match pc {
            0 => {
                if flags & FNM_LEADING_DIR != 0 && sc == b'/' {
                    return true;
                }
                if sc == 0 {
                    return true;
                }
                false
            }
            b'?' => {
                if sc == 0 {
                    return false;
                }
                if (sc == b'/' && flags & FNM_PATHNAME != 0) || period_at(s) {
                    false
                } else {
                    s += 1;
                    true
                }
            }
            b'*' => {
                while at(pattern, p) == b'*' {
                    p += 1;
                }
                let c = at(pattern, p);
                if period_at(s) {
                    false
                } else if c == 0 {
                    return if flags & FNM_PATHNAME != 0 {
                        flags & FNM_LEADING_DIR != 0 || !string[s..].contains(&b'/')
                    } else {
                        true
                    };
                } else if c == b'/' && flags & FNM_PATHNAME != 0 {
                    match string[s.min(string.len())..].iter().position(|&b| b == b'/') {
                        Some(off) => {
                            s += off;
                            true
                        }
                        None => return false,
                    }
                } else {
                    bt = Some((p, s));
                    true
                }
            }
            b'[' => {
                if sc == 0 {
                    return false;
                }
                if (sc == b'/' && flags & FNM_PATHNAME != 0) || period_at(s) {
                    false
                } else {
                    match range_match(pattern, p, sc, flags) {
                        Range::Error => return false,
                        Range::NoMatch => false,
                        Range::Match(np) => {
                            p = np;
                            s += 1;
                            true
                        }
                    }
                }
            }
            _ => {
                let mut pc = pc;
                if pc == b'\\' && flags & FNM_NOESCAPE == 0 {
                    pc = at(pattern, p);
                    if pc == 0 {
                        return false;
                    }
                    p += 1;
                }
                if sc != 0 && (pc == sc || fold(pc, flags) == fold(sc, flags)) {
                    s += 1;
                    true
                } else {
                    false
                }
            }
        };
        if !step {
            // Back to the last `*`, which now swallows one more byte.
            let Some((bp, bs)) = bt else { return false };
            let c = at(string, bs);
            if c == 0 || (c == b'/' && flags & FNM_PATHNAME != 0) {
                return false;
            }
            bt = Some((bp, bs + 1));
            p = bp;
            s = bs + 1;
        }
    }
}

enum Range {
    Match(usize),
    NoMatch,
    Error,
}

/// A `[:name:]` class test.
fn in_class(name: &[u8], c: u8) -> Option<bool> {
    Some(match name {
        b"alnum" => c.is_ascii_alphanumeric(),
        b"alpha" => c.is_ascii_alphabetic(),
        b"blank" => c == b' ' || c == b'\t',
        b"cntrl" => c.is_ascii_control(),
        b"digit" => c.is_ascii_digit(),
        b"graph" => c.is_ascii_graphic(),
        b"lower" => c.is_ascii_lowercase(),
        b"print" => c.is_ascii_graphic() || c == b' ',
        b"punct" => c.is_ascii_punctuation(),
        b"space" => matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c),
        b"upper" => c.is_ascii_uppercase(),
        b"xdigit" => c.is_ascii_hexdigit(),
        _ => return None,
    })
}

/// A bracket expression starting at `p` (just past the `[`) against `test`.
fn range_match(pattern: &[u8], mut p: usize, test: u8, flags: i64) -> Range {
    let negate = matches!(at(pattern, p), b'!' | b'^');
    if negate {
        p += 1;
    }
    let test = fold(test, flags);
    let origin = p;
    let mut ok = false;
    loop {
        let mut c = at(pattern, p);
        if c == b']' && p > origin {
            p += 1;
            break;
        }
        if c == 0 {
            return Range::Error;
        }
        if c == b'/' && flags & FNM_PATHNAME != 0 {
            return Range::NoMatch;
        }
        if c == b'[' && matches!(at(pattern, p + 1), b':' | b'.' | b'=') {
            let kind = at(pattern, p + 1);
            let start = p + 2;
            let Some(len) = pattern[start.min(pattern.len())..]
                .windows(2)
                .position(|w| w[0] == kind && w[1] == b']')
            else {
                return Range::Error;
            };
            let body = &pattern[start..start + len];
            p = start + len + 2;
            match kind {
                b':' => match in_class(body, test) {
                    Some(hit) => {
                        ok |= hit;
                        continue;
                    }
                    None => return Range::Error,
                },
                _ => {
                    // A collating symbol / equivalence class of one byte
                    // is that byte; it may start a range.
                    if body.len() != 1 {
                        return Range::Error;
                    }
                    c = body[0];
                }
            }
        } else {
            if c == b'\\' && flags & FNM_NOESCAPE == 0 {
                p += 1;
                c = at(pattern, p);
                if c == 0 {
                    return Range::Error;
                }
            }
            p += 1;
        }
        let c = fold(c, flags);
        if at(pattern, p) == b'-' && at(pattern, p + 1) != 0 && at(pattern, p + 1) != b']' {
            p += 1;
            if at(pattern, p) == b'\\' && flags & FNM_NOESCAPE == 0 {
                p += 1;
            }
            let c2 = at(pattern, p);
            if c2 == 0 {
                return Range::Error;
            }
            p += 1;
            let c2 = fold(c2, flags);
            if c <= test && test <= c2 {
                ok = true;
            }
        } else if c == test {
            ok = true;
        }
    }
    if ok == negate {
        Range::NoMatch
    } else {
        Range::Match(p)
    }
}

// ---- getopt --------------------------------------------------------------------

/// One option `getopt()` recognises: a short letter or a long name, and
/// whether it takes a value (0 no, 1 required, 2 optional).
struct Opt {
    ch: u8,
    name: Option<Vec<u8>>,
    need: u8,
}

/// The option table php's `parse_opts` builds from the short-options
/// string: letters and digits, each optionally followed by `:` or `::`;
/// the scan stops at the first other byte.
fn short_opts(spec: &[u8]) -> Vec<Opt> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spec.len() && spec[i].is_ascii_alphanumeric() {
        let ch = spec[i];
        i += 1;
        let mut need = 0;
        if at(spec, i) == b':' {
            need = 1;
            i += 1;
            if at(spec, i) == b':' {
                need = 2;
                i += 1;
            }
        }
        out.push(Opt { ch, name: None, need });
    }
    out
}

/// What one step of php's `php_getopt()` found.
enum OptHit {
    /// Option `idx` of the table, with its value.
    Opt(usize, Option<Vec<u8>>),
    /// Something unrecognised (skipped).
    Invalid,
    /// The end of the options.
    End,
}

/// php's `php_getopt()` state machine (`main/getopt.c`), over `argv`.
struct GetoptState {
    optind: usize,
    optchr: usize,
    dash: bool,
}

impl GetoptState {
    fn next(&mut self, argv: &[Vec<u8>], opts: &[Opt]) -> OptHit {
        let argc = argv.len();
        if self.optind >= argc {
            return OptHit::End;
        }
        let arg = &argv[self.optind];
        if !self.dash && (at(arg, 0) != b'-' || at(arg, 1) == 0) {
            return OptHit::End;
        }
        let mut idx: Option<usize> = None;
        let mut arg_start;
        if at(arg, 0) == b'-' && at(arg, 1) == b'-' {
            // A long option, `--name`, `--name=value`.
            if at(arg, 2) == 0 {
                self.optind += 1;
                return OptHit::End;
            }
            arg_start = 2;
            let body = &arg[2..];
            // php looks for the `=` short of the last byte: `--opt=` is
            // the (unknown) name `opt=`.
            let name_len = match body[..body.len() - 1].iter().position(|&b| b == b'=') {
                Some(eq) => {
                    arg_start += 1;
                    eq
                }
                None => body.len(),
            };
            let found = opts
                .iter()
                .position(|o| o.name.as_deref().is_some_and(|n| n == &body[..name_len]));
            let Some(found) = found else {
                self.optind += 1;
                return OptHit::Invalid;
            };
            idx = Some(found);
            self.optchr = 0;
            self.dash = false;
            arg_start += name_len;
        } else {
            if !self.dash {
                self.dash = true;
                self.optchr = 1;
            }
            if at(arg, self.optchr) == b':' {
                self.dash = false;
                self.optind += 1;
                return OptHit::Invalid;
            }
            arg_start = 1 + self.optchr;
        }
        let idx = match idx {
            Some(i) => i,
            None => {
                let c = at(arg, self.optchr);
                match opts.iter().position(|o| o.name.is_none() && o.ch == c && c != 0) {
                    Some(i) => i,
                    None => {
                        if at(arg, self.optchr + 1) == 0 {
                            self.dash = false;
                            self.optind += 1;
                        } else {
                            self.optchr += 1;
                        }
                        return OptHit::Invalid;
                    }
                }
            }
        };
        let need = opts[idx].need;
        if need > 0 {
            self.dash = false;
            if at(arg, arg_start) == 0 {
                self.optind += 1;
                if self.optind == argc {
                    if need == 1 {
                        return OptHit::Invalid;
                    }
                } else if need == 1 {
                    let v = argv[self.optind].clone();
                    self.optind += 1;
                    return OptHit::Opt(idx, Some(v));
                }
                return OptHit::Opt(idx, None);
            }
            if at(arg, arg_start) == b'=' {
                arg_start += 1;
            }
            let v = arg[arg_start..].to_vec();
            self.optind += 1;
            return OptHit::Opt(idx, Some(v));
        }
        // Several short options in one word (`-ab`).
        if arg_start >= 2 && !(at(arg, 0) == b'-' && at(arg, 1) == b'-') {
            if at(arg, self.optchr + 1) == 0 {
                self.dash = false;
                self.optind += 1;
            } else {
                self.optchr += 1;
            }
        } else {
            self.optind += 1;
        }
        OptHit::Opt(idx, None)
    }
}

/// The script's `argv`: `$_SERVER['argv']`, else the global `$argv`.
fn script_argv(ctx: &Ctx) -> Option<Value> {
    if let Some(Value::Array(server)) = ctx.globals.get(b"_SERVER").map(|c| c.get()) {
        if let Some(v) = server.get(&ArrayKey::str(b"argv")) {
            return Some(v.deref().into_owned());
        }
    }
    ctx.globals.get(b"argv").map(|c| c.get().deref().into_owned())
}

/// `getopt(string $short_options, array $long_options = [], &$rest_index = null): array|false`
fn getopt(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let spec = args[0].to_php_bytes();
    if args.len() > 2 {
        args[2] = Value::Int(1);
    }
    let Some(argv_v) = script_argv(ctx) else {
        return Ok(Value::Bool(false));
    };
    let Value::Array(argv_a) = argv_v else {
        return Ok(Value::Bool(false));
    };
    let mut argv: Vec<Vec<u8>> = Vec::new();
    for (_, v) in argv_a.iter() {
        argv.push(zval_string(ctx, &v.deref())?);
    }
    let mut opts = short_opts(&spec);
    if let Some(Value::Array(longs)) = args.get(1).map(|v| v.deref().into_owned()) {
        for (_, v) in longs.iter() {
            let mut name = zval_string(ctx, &v.deref())?;
            // A C string: anything past a NUL is not part of the name.
            if let Some(nul) = name.iter().position(|&b| b == 0) {
                name.truncate(nul);
            }
            let mut need = 0;
            if name.last() == Some(&b':') {
                need = 1;
                name.pop();
                if name.last() == Some(&b':') {
                    need = 2;
                    name.pop();
                }
            }
            opts.push(Opt { ch: 0, name: Some(name), need });
        }
    }
    let mut st = GetoptState { optind: 1, optchr: 0, dash: false };
    let mut out = Array::new();
    loop {
        let (idx, val) = match st.next(&argv, &opts) {
            OptHit::End => break,
            OptHit::Invalid => continue,
            OptHit::Opt(i, v) => (i, v),
        };
        let o = &opts[idx];
        let name: Vec<u8> = match &o.name {
            Some(n) => n.clone(),
            None => vec![o.ch],
        };
        let val = match val {
            // php keeps the value as a C string.
            Some(mut b) => {
                if let Some(nul) = b.iter().position(|&c| c == 0) {
                    b.truncate(nul);
                }
                Value::Str(Str::from_vec(b))
            }
            None => Value::Bool(false),
        };
        let numeric = !(name.len() > 1 && name[0] == b'0')
            && matches!(rphp_value::numeric_string(&name), Some(Value::Int(_)))
            && name.iter().all(|b| b.is_ascii_digit() || *b == b'-' || *b == b'+');
        let key = if numeric {
            ArrayKey::Int(atoi(&name))
        } else {
            ArrayKey::Str(Str::new(&name))
        };
        match out.get(&key).cloned() {
            Some(Value::Array(mut list)) => {
                list.push(val);
                out.set(key, Value::Array(list));
            }
            Some(prev) => {
                let mut list = Array::new();
                list.push(prev);
                list.push(val);
                out.set(key, Value::Array(list));
            }
            None => out.set(key, val),
        }
    }
    if args.len() > 2 {
        args[2] = Value::Int(st.optind as i64);
    }
    Ok(Value::Array(out))
}

/// C's `atoi()` of a numeric option name (an `int`, so it wraps).
fn atoi(s: &[u8]) -> i64 {
    let (neg, digits) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i64 = 0;
    for &d in digits.iter().take_while(|d| d.is_ascii_digit()) {
        n = n.wrapping_mul(10).wrapping_add(i64::from(d - b'0'));
    }
    let n = if neg { n.wrapping_neg() } else { n };
    i64::from(n as i32)
}

// ---- md5_file / sha1_file ------------------------------------------------------

/// The bytes of `path` for a `*_file()` digest, with php's diagnostics:
/// `None` after the warning for a file that cannot be opened.
fn read_input(ctx: &mut Ctx, func: &str, v: &Value) -> Result<Option<Vec<u8>>, Unwind> {
    let raw = v.to_php_bytes();
    if raw.is_empty() {
        return Err(Unwind::value_error("Path must not be empty"));
    }
    if raw.contains(&0) {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #1 ($filename) must not contain any null bytes"
        )));
    }
    let is_url = raw.windows(3).any(|w| w == b"://") || raw.starts_with(b"data:");
    if is_url && !raw.starts_with(b"file://") {
        // Every other wrapper goes through the stream layer.
        let h = ctx.call_function(b"fopen", &[v.clone(), Value::string(b"rb")])?;
        if !matches!(h, Value::Resource(_)) {
            return Ok(None);
        }
        let r = ctx.call_function(b"stream_get_contents", std::slice::from_ref(&h))?;
        ctx.call_function(b"fclose", &[h])?;
        return Ok(match r {
            Value::Str(s) => Some(s.as_bytes().to_vec()),
            _ => None,
        });
    }
    let raw = raw.strip_prefix(b"file://").map(<[u8]>::to_vec).unwrap_or(raw);
    let path = crate::filestat::arg_path(ctx, &Value::Str(Str::from_vec(raw)));
    if path.is_dir() {
        // php opens a directory fine, then its first read fails.
        ctx.notice(&format!("{func}(): Read of 8192 bytes failed with errno=21 Is a directory"))?;
        return Ok(Some(Vec::new()));
    }
    match std::fs::read(&path) {
        Ok(b) => Ok(Some(b)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "{func}({shown}): Failed to open stream: {}",
                crate::filestat::io_text(&e)
            ))?;
            Ok(None)
        }
    }
}

/// `md5_file(string $filename, bool $binary = false): string|false`
fn md5_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(data) = read_input(ctx, "md5_file", &args[0])? else {
        return Ok(Value::Bool(false));
    };
    let mut a = [Value::Str(Str::from_vec(data)), args.get(1).cloned().unwrap_or(Value::Bool(false))];
    crate::hash::md5(ctx, &mut a)
}

/// `sha1_file(string $filename, bool $binary = false): string|false`
fn sha1_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(data) = read_input(ctx, "sha1_file", &args[0])? else {
        return Ok(Value::Bool(false));
    };
    let mut a = [Value::Str(Str::from_vec(data)), args.get(1).cloned().unwrap_or(Value::Bool(false))];
    crate::hash::sha1(ctx, &mut a)
}

// ---- str_increment / str_decrement ---------------------------------------------

fn check_alnum(func: &str, s: &[u8]) -> Result<(), Unwind> {
    if s.is_empty() {
        return Err(Unwind::value_error(format!("{func}(): Argument #1 ($string) must not be empty")));
    }
    if !s.iter().all(u8::is_ascii_alphanumeric) {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #1 ($string) must be composed only of alphanumeric ASCII characters"
        )));
    }
    Ok(())
}

/// `str_increment(string $string): string` (8.3) — php's alphanumeric
/// increment of `$s++`, without the numeric-string special case.
fn str_increment(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut s = args[0].to_php_bytes();
    check_alnum("str_increment", &s)?;
    let mut i = s.len();
    let mut carry = true;
    while carry && i > 0 {
        i -= 1;
        let (next, c) = match s[i] {
            b'z' => (b'a', true),
            b'Z' => (b'A', true),
            b'9' => (b'0', true),
            c => (c + 1, false),
        };
        s[i] = next;
        carry = c;
    }
    if carry {
        let first = match s[0] {
            b'0' => b'1',
            b'a' => b'a',
            _ => b'A',
        };
        s.insert(0, first);
    }
    Ok(Value::Str(Str::from_vec(s)))
}

/// `str_decrement(string $string): string` (8.3).
fn str_decrement(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let orig = args[0].to_php_bytes();
    check_alnum("str_decrement", &orig)?;
    let out_of_range = || {
        Unwind::value_error(format!(
            "str_decrement(): Argument #1 ($string) \"{}\" is out of decrement range",
            String::from_utf8_lossy(&orig)
        ))
    };
    if orig.len() > 1 && orig[0] == b'0' {
        return Err(out_of_range());
    }
    let mut s = orig.clone();
    let mut i = s.len();
    let mut carry = true;
    while carry && i > 0 {
        i -= 1;
        let (next, c) = match s[i] {
            b'a' => (b'z', true),
            b'A' => (b'Z', true),
            b'0' => (b'9', true),
            c => (c - 1, false),
        };
        s[i] = next;
        carry = c;
    }
    if carry || (s[0] == b'0' && s.len() > 1) {
        if s.len() == 1 {
            return Err(out_of_range());
        }
        s.remove(0);
    }
    Ok(Value::Str(Str::from_vec(s)))
}

// ---- ini_parse_quantity ----------------------------------------------------------

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// C's `strtoull(s, &end, base)` from `s[0]` (which the caller made sure
/// is not whitespace or a sign): the value, the end offset, overflow.
fn strtoull(s: &[u8], base: u32) -> (u64, usize, bool) {
    let mut i = 0;
    let mut base = base;
    if base == 0 {
        if at(s, 0) == b'0' {
            if matches!(at(s, 1), b'x' | b'X') && (at(s, 2) as char).is_ascii_hexdigit() {
                base = 16;
                i = 2;
            } else {
                base = 8;
            }
        } else {
            base = 10;
        }
    } else if base == 16 && at(s, 0) == b'0' && matches!(at(s, 1), b'x' | b'X') && (at(s, 2) as char).is_ascii_hexdigit() {
        i = 2;
    }
    let start = i;
    let mut v: u64 = 0;
    let mut over = false;
    while let Some(d) = s.get(i).and_then(|&c| (c as char).to_digit(base)) {
        match v.checked_mul(u64::from(base)).and_then(|x| x.checked_add(u64::from(d))) {
            Some(n) => v = n,
            None => over = true,
        }
        i += 1;
    }
    if i == start {
        // No digits: nothing consumed (a lone `0x` counts as the `0`).
        return (0, 0, false);
    }
    if over {
        v = u64::MAX;
    }
    (v, i, over)
}

/// php's `zend_ini_parse_quantity()` (signed): the value, and the warning
/// text when the input is not a clean quantity.
pub(crate) fn parse_quantity(value: &[u8]) -> (i64, Option<String>) {
    let str_start = 0usize;
    let mut digits = 0usize;
    let mut end = value.len();
    while digits < end && is_ws(value[digits]) {
        digits += 1;
    }
    while digits < end && is_ws(value[end - 1]) {
        end -= 1;
    }
    if digits == end {
        return (0, None);
    }
    let invalid = escaped(value);
    let no_digits = || {
        (
            0,
            Some(format!(
                "Invalid quantity \"{invalid}\": no valid leading digits, interpreting as \"0\" for backwards compatibility"
            )),
        )
    };
    let mut negative = false;
    if value[digits] == b'+' {
        digits += 1;
    } else if value[digits] == b'-' {
        negative = true;
        digits += 1;
    }
    let s = &value[..end];
    if !at(s, digits).is_ascii_digit() {
        return no_digits();
    }
    let mut base = 0;
    if at(s, digits) == b'0' && !at(s, digits + 1).is_ascii_digit() {
        if digits + 1 == end {
            return (0, None);
        }
        match s[digits + 1] {
            b'g' | b'G' | b'm' | b'M' | b'k' | b'K' => {}
            c => {
                base = match c {
                    b'x' | b'X' => 16,
                    b'o' | b'O' => 8,
                    b'b' | b'B' => 2,
                    _ => {
                        return (
                            0,
                            Some(format!(
                                "Invalid prefix \"0{}\", interpreting as \"0\" for backwards compatibility",
                                c as char
                            )),
                        )
                    }
                };
                digits += 2;
                // strtoull would quietly take whitespace, a sign or a
                // second prefix here.
                let bad = digits == end
                    || is_ws(s[digits])
                    || s[digits] == b'+'
                    || s[digits] == b'-'
                    || (base == 16 && s[digits] == b'0' && matches!(at(s, digits + 1), b'x' | b'X'));
                if bad {
                    return (
                        0,
                        Some(format!(
                            "Invalid quantity \"{invalid}\": no digits after base prefix, interpreting as \"0\" for backwards compatibility"
                        )),
                    );
                }
            }
        }
    }
    let (mut retval, len, erange) = strtoull(&s[digits..], base);
    let mut digits_end = digits + len;
    let mut overflow = false;
    if erange {
        overflow = true;
    } else if negative && retval == (i64::MAX as u64) + 1 {
        retval = 0u64.wrapping_sub(retval);
    } else if (retval as i64) < 0 {
        overflow = true;
    } else if negative {
        retval = 0u64.wrapping_sub(retval);
    }
    if digits_end == digits {
        return no_digits();
    }
    while digits_end < end && is_ws(s[digits_end]) {
        digits_end += 1;
    }
    let overflow_msg = || {
        format!("Invalid quantity \"{invalid}\": value is out of range, using overflow result for backwards compatibility")
    };
    if digits_end == end {
        return (retval as i64, overflow.then(overflow_msg));
    }
    let interpreted = escaped(&value[str_start..digits_end]);
    let suffix = s[end - 1];
    let factor: u64 = match suffix {
        b'g' | b'G' => 1 << 30,
        b'm' | b'M' => 1 << 20,
        b'k' | b'K' => 1 << 10,
        _ => {
            return (
                retval as i64,
                Some(format!(
                    "Invalid quantity \"{invalid}\": unknown multiplier \"{}\", interpreting as \"{interpreted}\" for backwards compatibility",
                    escaped(&[suffix])
                )),
            )
        }
    };
    if !overflow {
        let sv = retval as i64;
        let f = factor as i64;
        overflow = if sv > 0 { sv > i64::MAX / f } else { sv < i64::MIN / f };
    }
    retval = retval.wrapping_mul(factor);
    if digits_end != end - 1 {
        return (
            retval as i64,
            Some(format!(
                "Invalid quantity \"{invalid}\", interpreting as \"{interpreted}{}\" for backwards compatibility",
                escaped(&[suffix])
            )),
        );
    }
    (retval as i64, overflow.then(overflow_msg))
}

/// `ini_parse_quantity(string $shorthand): int` (8.2)
fn ini_parse_quantity(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (v, err) = parse_quantity(&args[0].to_php_bytes());
    if let Some(e) = err {
        ctx.warn(&e)?;
    }
    Ok(Value::Int(v))
}

// ---- forward_static_call -------------------------------------------------------

/// php's `TypeError` for a `$callback` argument that does not resolve.
pub(crate) fn callback_error(ctx: &Ctx, func: &str, pos: usize, name: &str, cb: &Value) -> Unwind {
    let what = match &*cb.deref() {
        Value::Str(s) => {
            let b = s.as_bytes();
            match b.windows(2).position(|w| w == b"::") {
                Some(i) => {
                    let (class, method) = (&b[..i], &b[i + 2..]);
                    match ctx.class_by_name(class) {
                        None => format!("class \"{}\" not found", String::from_utf8_lossy(class)),
                        Some(cid) => format!(
                            "class {} does not have a method \"{}\"",
                            ctx.class(cid).name_str(),
                            String::from_utf8_lossy(method)
                        ),
                    }
                }
                None => format!("function \"{}\" not found or invalid function name", s.to_string_lossy()),
            }
        }
        Value::Array(a) if a.len() == 2 => {
            let first = a.get(&ArrayKey::Int(0)).map(|v| v.deref().into_owned());
            let method = a.get(&ArrayKey::Int(1)).map(|v| v.to_php_bytes()).unwrap_or_default();
            match first {
                Some(Value::Str(c)) => match ctx.class_by_name(c.as_bytes()) {
                    None => format!("class \"{}\" not found", c.to_string_lossy()),
                    Some(cid) => format!(
                        "class {} does not have a method \"{}\"",
                        ctx.class(cid).name_str(),
                        String::from_utf8_lossy(&method)
                    ),
                },
                Some(Value::Object(o)) => format!(
                    "class {} does not have a method \"{}\"",
                    ctx.class_name_of(&o),
                    String::from_utf8_lossy(&method)
                ),
                _ => "first array member is not a valid class name or object".to_string(),
            }
        }
        Value::Array(_) => "array callback must have exactly two members".to_string(),
        _ => "no array or string given".to_string(),
    };
    Unwind::type_error(format!("{func}(): Argument #{pos} (${name}) must be a valid callback, {what}"))
}

/// The shared body of the two `forward_static_call*()`: call `cb` with the
/// caller's called scope (`static::`) kept when the callee's class is an
/// ancestor of it.
fn forward_call(
    ctx: &mut Ctx,
    func: &str,
    cb: &Value,
    args: &[Value],
    named: Vec<(Box<[u8]>, Value)>,
) -> NativeResult {
    ctx.autoload_callable(cb)?;
    let callable = match ctx.resolve_callable(cb) {
        Ok(c) => c,
        Err(_) => return Err(callback_error(ctx, func, 1, "callback", cb)),
    };
    let (scope, called) = match ctx.current_user_frame() {
        Some(f) => (f.scope, f.static_class.or(f.scope)),
        None => (None, None),
    };
    if scope.is_none() {
        return Err(Unwind::error(format!("Cannot call {func}() when no class scope is active")));
    }
    let callable = match callable {
        Callable::User { func: f, this, scope: s, static_class, closure } => {
            let static_class = match (called, static_class) {
                (Some(c), Some(target)) if ctx.is_subclass_or_eq(c, target) => Some(c),
                _ => static_class,
            };
            Callable::User { func: f, this, scope: s, static_class, closure }
        }
        other => other,
    };
    ctx.call_resolved_named(callable, args, named)
}

/// `forward_static_call(callable $callback, mixed ...$args): mixed`
fn forward_static_call(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let named = ctx.take_extra_named();
    let cb = args[0].clone();
    forward_call(ctx, "forward_static_call", &cb, &args[1..], named)
}

/// `forward_static_call_array(callable $callback, array $args): mixed`
fn forward_static_call_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let cb = args[0].clone();
    let Value::Array(arr) = args[1].deref().into_owned() else {
        return Err(Unwind::type_error(format!(
            "forward_static_call_array(): Argument #2 ($args) must be of type array, {} given",
            rphp_runtime::value_name(&args[1])
        )));
    };
    let mut positional = Vec::new();
    let mut named: Vec<(Box<[u8]>, Value)> = Vec::new();
    for (k, v) in arr.iter() {
        match k {
            ArrayKey::Int(_) => {
                if !named.is_empty() {
                    return Err(Unwind::error(
                        "Cannot use positional argument after named argument during unpacking",
                    ));
                }
                positional.push(v.clone());
            }
            ArrayKey::Str(s) => named.push((Box::from(s.as_bytes()), v.clone())),
        }
    }
    forward_call(ctx, "forward_static_call_array", &cb, &positional, named)
}

// ---- assert_options --------------------------------------------------------------

const ASSERT_ACTIVE: i64 = 1;
const ASSERT_CALLBACK: i64 = 2;
const ASSERT_BAIL: i64 = 3;
const ASSERT_WARNING: i64 = 4;
const ASSERT_EXCEPTION: i64 = 5;

/// `assert_options()`'s own state: the settings the ini table does not
/// carry, and the callback (a value, not an ini string).
#[derive(Default)]
struct AssertState {
    bail: Option<bool>,
    warning: Option<bool>,
    callback: Option<Value>,
}

const ASSERT_SLOT: &str = "standard.assert-options";

/// An ini boolean as php's `OnUpdateBool` reads it.
fn ini_bool(s: &[u8]) -> bool {
    let t = s.to_ascii_lowercase();
    match t.as_slice() {
        b"true" | b"on" | b"yes" => true,
        _ => rphp_value::Value::Str(Str::new(s)).to_int() != 0,
    }
}

/// `assert_options(int $option, mixed $value = UNKNOWN): mixed` — deprecated
/// since 8.3.
fn assert_options(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function assert_options() is deprecated since 8.3")?;
    let what = args[0].to_int();
    let value = args.get(1).map(|v| v.deref().into_owned());
    let as_text = |ctx: &mut Ctx, v: &Value| zval_string(ctx, v);
    match what {
        ASSERT_ACTIVE | ASSERT_EXCEPTION => {
            let key = if what == ASSERT_ACTIVE { "assert.active" } else { "assert.exception" };
            let old = ini_bool(ctx.ini_get(key).unwrap_or("1").as_bytes());
            if let Some(v) = value {
                let t = as_text(ctx, &v)?;
                ctx.ini_set(key, &String::from_utf8_lossy(&t));
            }
            Ok(Value::Int(i64::from(old)))
        }
        ASSERT_BAIL | ASSERT_WARNING => {
            let st = ctx.ext.slot::<AssertState>(ASSERT_SLOT);
            let cur = if what == ASSERT_BAIL { st.bail.unwrap_or(false) } else { st.warning.unwrap_or(true) };
            if let Some(v) = value {
                let t = as_text(ctx, &v)?;
                let st = ctx.ext.slot::<AssertState>(ASSERT_SLOT);
                let b = Some(ini_bool(&t));
                if what == ASSERT_BAIL {
                    st.bail = b;
                } else {
                    st.warning = b;
                }
            }
            Ok(Value::Int(i64::from(cur)))
        }
        ASSERT_CALLBACK => {
            let st = ctx.ext.slot::<AssertState>(ASSERT_SLOT);
            let old = st.callback.clone().unwrap_or(Value::Null);
            if let Some(v) = value {
                st.callback = if matches!(v, Value::Null) { None } else { Some(v) };
            }
            Ok(old)
        }
        _ => Err(Unwind::value_error("assert_options(): Argument #1 ($option) must be an ASSERT_* constant")),
    }
}

// ---- things a CLI request does not have ----------------------------------------------

/// `realpath_cache_size(): int` — rphp keeps no realpath cache, so it
/// holds nothing.
fn realpath_cache_size(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(0))
}

/// `realpath_cache_get(): array` — the (empty) realpath cache.
fn realpath_cache_get(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Array(Array::new()))
}

/// `get_browser(?string $user_agent = null, bool $return_array = false): object|array|false`
/// — without a `browscap` ini file there is nothing to look in.
fn get_browser(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    ctx.warn("get_browser(): browscap ini directive not set")?;
    Ok(Value::Bool(false))
}

/// The URL-rewriter variables `output_add_rewrite_var()` collected.
#[derive(Default)]
struct RewriteVars(Vec<(Vec<u8>, Vec<u8>)>);

const REWRITE_SLOT: &str = "standard.url-rewrite-vars";

/// `output_add_rewrite_var(string $name, string $value): bool`
fn output_add_rewrite_var(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    let value = args[1].to_php_bytes();
    ctx.ext.slot::<RewriteVars>(REWRITE_SLOT).0.push((name, value));
    Ok(Value::Bool(true))
}

/// `output_reset_rewrite_vars(): bool`
fn output_reset_rewrite_vars(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    ctx.ext.slot::<RewriteVars>(REWRITE_SLOT).0.clear();
    Ok(Value::Bool(true))
}

/// The `$options` keys `request_parse_body()` accepts.
const BODY_OPTIONS: &[&str] = &[
    "post_max_size",
    "max_input_vars",
    "max_multipart_body_parts",
    "max_file_uploads",
    "upload_max_filesize",
];

/// `request_parse_body(?array $options = null): array` (8.4) — parse the
/// request body the way php does for POST; outside a request with a
/// content type there is none to parse.
fn request_parse_body(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if let Some(Value::Array(opts)) = args.first().map(|v| v.deref().into_owned()) {
        for (k, v) in opts.iter() {
            let key = match k {
                ArrayKey::Str(s) => s.as_bytes().to_vec(),
                ArrayKey::Int(i) => i.to_string().into_bytes(),
            };
            let Some(name) = BODY_OPTIONS.iter().find(|n| n.as_bytes().eq_ignore_ascii_case(&key)) else {
                return Err(Unwind::value_error(format!(
                    "Invalid key \"{}\" in $options argument",
                    String::from_utf8_lossy(&key)
                )));
            };
            let text = zval_string(ctx, &v.deref())?;
            let (_, err) = parse_quantity(&text);
            if let Some(e) = err {
                ctx.warn(&e)?;
            }
            let _ = name;
        }
    }
    let content_type = match ctx.globals.get(b"_SERVER").map(|c| c.get()) {
        Some(Value::Array(s)) => s.get(&ArrayKey::str(b"CONTENT_TYPE")).map(|v| v.to_php_bytes()),
        _ => None,
    };
    if content_type.is_none() || ctx.request_body.is_none() {
        return Err(Unwind::exception("RequestParseBodyException", "Request does not provide a content type"));
    }
    Err(Unwind::exception(
        "RequestParseBodyException",
        format!(
            "Content-Type \"{}\" is not supported",
            String::from_utf8_lossy(&content_type.unwrap_or_default())
        ),
    ))
}

#[allow(dead_code)]
pub(crate) fn register_classes(_r: &mut Registry) {}

pub(crate) fn register_constants(r: &mut Registry) {
    r.constant("FNM_NOESCAPE", Value::Int(FNM_NOESCAPE));
    r.constant("FNM_PATHNAME", Value::Int(FNM_PATHNAME));
    r.constant("FNM_PERIOD", Value::Int(FNM_PERIOD));
    r.constant("FNM_CASEFOLD", Value::Int(FNM_CASEFOLD));
    const NOTE: &str = " since 8.3, as assert_options() is deprecated";
    r.deprecated_constant("ASSERT_ACTIVE", Value::Int(ASSERT_ACTIVE), NOTE);
    r.deprecated_constant("ASSERT_CALLBACK", Value::Int(ASSERT_CALLBACK), NOTE);
    r.deprecated_constant("ASSERT_BAIL", Value::Int(ASSERT_BAIL), NOTE);
    r.deprecated_constant("ASSERT_WARNING", Value::Int(ASSERT_WARNING), NOTE);
    r.deprecated_constant("ASSERT_EXCEPTION", Value::Int(ASSERT_EXCEPTION), NOTE);
    sys::register_constants(r);
}

/// A value as php's `zval_get_string()` makes it a string (objects via
/// `__toString()`, arrays with the conversion warning).
fn zval_string(ctx: &mut Ctx, v: &Value) -> Result<Vec<u8>, Unwind> {
    Ok(ctx.to_string(v)?.as_bytes().to_vec())
}

// ---- tick functions ------------------------------------------------------------

/// The functions `register_tick_function()` collected, with their extra
/// arguments. rphp does not compile `declare(ticks=N)` (a script using it
/// is refused), so — exactly as in php for code without that declaration
/// — they are never called; the registry is kept so `unregister` works.
#[derive(Default)]
struct TickFunctions(Vec<(Value, Vec<Value>)>);

const TICK_SLOT: &str = "standard.tick-functions";

/// A callable argument, resolved or refused with php's `TypeError`.
fn callable_arg(ctx: &mut Ctx, func: &str, cb: &Value) -> Result<(), Unwind> {
    ctx.autoload_callable(cb)?;
    match ctx.resolve_callable(cb) {
        Ok(_) => Ok(()),
        Err(_) => Err(callback_error(ctx, func, 1, "callback", cb)),
    }
}

/// `register_tick_function(callable $callback, mixed ...$args): bool`
fn register_tick_function(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    callable_arg(ctx, "register_tick_function", &args[0])?;
    let entry = (args[0].deref().into_owned(), args[1..].to_vec());
    ctx.ext.slot::<TickFunctions>(TICK_SLOT).0.push(entry);
    Ok(Value::Bool(true))
}

/// `unregister_tick_function(callable $callback): void`
fn unregister_tick_function(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    callable_arg(ctx, "unregister_tick_function", &args[0])?;
    let cb = args[0].deref().into_owned();
    let list = &mut ctx.ext.slot::<TickFunctions>(TICK_SLOT).0;
    if let Some(i) = list.iter().position(|(f, _)| f.loose_eq(&cb)) {
        list.remove(i);
    }
    Ok(Value::Null)
}

// ---- php_strip_whitespace ------------------------------------------------------

/// php's `zend_strip()`: the source with comments dropped and each run of
/// whitespace squeezed to one space; a heredoc's closing label keeps the
/// newline after it.
fn strip_source(ctx: &Ctx, src: &[u8]) -> Vec<u8> {
    use rphp_tokenizer::ids;
    let toks = rphp_tokenizer::tokenize(src, rphp_tokenizer::Options { short_open_tag: ctx.ini.bool("short_open_tag") });
    let mut out = Vec::with_capacity(src.len());
    let mut prev_space = false;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        i += 1;
        match t.id {
            ids::T_WHITESPACE => {
                if !prev_space {
                    out.push(b' ');
                    prev_space = true;
                }
                continue;
            }
            ids::T_COMMENT | ids::T_DOC_COMMENT => continue,
            ids::T_END_HEREDOC => {
                out.extend_from_slice(t.text(src));
                // The byte after the label: a newline (dropped) or `;`.
                if let Some(n) = toks.get(i) {
                    i += 1;
                    if n.id != ids::T_WHITESPACE {
                        out.extend_from_slice(n.text(src));
                    }
                }
                out.push(b'\n');
                prev_space = true;
                continue;
            }
            _ => out.extend_from_slice(t.text(src)),
        }
        prev_space = false;
    }
    out
}

/// `php_strip_whitespace(string $filename): string`
fn php_strip_whitespace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(src) = read_input(ctx, "php_strip_whitespace", &args[0])? else {
        return Ok(Value::string(b""));
    };
    Ok(Value::Str(Str::from_vec(strip_source(ctx, &src))))
}

// ---- get_meta_tags -------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetaTok {
    Eof,
    OpenTag,
    CloseTag,
    Slash,
    Equal,
    Space,
    Id,
    Str,
    Other,
}

/// php's `php_next_meta_token()` over the file's bytes, with the stream
/// semantics it leans on: EOF is only seen after a read past the end,
/// `getc()` then yields -1, and a NUL byte ends a scan loop.
struct MetaLexer<'a> {
    data: &'a [u8],
    pos: usize,
    eof: bool,
    ulc: bool,
    lc: i32,
    in_meta: bool,
    token: Vec<u8>,
}

const META_BUFSIZE: usize = 8192;

impl MetaLexer<'_> {
    fn getc(&mut self) -> i32 {
        match self.data.get(self.pos) {
            Some(&c) => {
                self.pos += 1;
                i32::from(c)
            }
            None => {
                self.eof = true;
                -1
            }
        }
    }

    fn is_alnum(c: i32) -> bool {
        (0..128).contains(&c) && (c as u8).is_ascii_alphanumeric()
    }

    fn next(&mut self) -> MetaTok {
        let mut ch: i32 = 0;
        loop {
            if !self.ulc {
                if self.eof {
                    break;
                }
                ch = self.getc();
                if ch == 0 {
                    break;
                }
            }
            if self.eof {
                break;
            }
            if self.ulc {
                ch = self.lc;
                self.ulc = false;
            }
            match ch {
                0x3c => return MetaTok::OpenTag,
                0x3e => return MetaTok::CloseTag,
                0x3d => return MetaTok::Equal,
                0x2f => return MetaTok::Slash,
                0x27 | 0x22 => {
                    let quote = ch;
                    self.token.clear();
                    while !self.eof {
                        ch = self.getc();
                        if ch == 0 || ch == quote || ch == 0x3c || ch == 0x3e {
                            break;
                        }
                        self.token.push(ch as u8);
                        if self.token.len() == META_BUFSIZE {
                            break;
                        }
                    }
                    if ch == 0x3c || ch == 0x3e {
                        // Was just an apostrophe.
                        self.ulc = true;
                        self.lc = ch;
                    }
                    return MetaTok::Str;
                }
                0x0a | 0x0d | 0x09 => {}
                0x20 => return MetaTok::Space,
                _ => {
                    if !Self::is_alnum(ch) {
                        return MetaTok::Other;
                    }
                    self.token.clear();
                    self.token.push(ch as u8);
                    while !self.eof {
                        ch = self.getc();
                        if ch == 0 || !(Self::is_alnum(ch) || matches!(ch, 0x2d | 0x5f | 0x2e | 0x3a)) {
                            break;
                        }
                        self.token.push(ch as u8);
                        if self.token.len() == META_BUFSIZE {
                            break;
                        }
                    }
                    let alpha = (0..128).contains(&ch) && (ch as u8).is_ascii_alphabetic();
                    if !alpha && !self.token.is_empty() {
                        self.ulc = true;
                        self.lc = ch;
                    }
                    return MetaTok::Id;
                }
            }
        }
        MetaTok::Eof
    }
}

/// The bytes php's `PHP_META_UNSAFE` replaces with `_` in a meta name.
const META_UNSAFE: &[u8] = b".\\+*?[^]$() ";

fn meta_name(raw: &[u8]) -> Vec<u8> {
    raw.iter().map(|c| if META_UNSAFE.contains(c) { b'_' } else { *c }).collect()
}

/// `get_meta_tags(string $filename, bool $use_include_path = false): array|false`
fn get_meta_tags(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(data) = read_input(ctx, "get_meta_tags", &args[0])? else {
        return Ok(Value::Bool(false));
    };
    let mut md = MetaLexer { data: &data, pos: 0, eof: false, ulc: false, lc: 0, in_meta: false, token: Vec::new() };
    let mut out = Array::new();
    let (mut in_tag, mut looking_for_val) = (false, false);
    let (mut have_name, mut have_content, mut saw_name, mut saw_content) = (false, false, false, false);
    let mut name: Option<Vec<u8>> = None;
    let mut value: Option<Vec<u8>> = None;
    let mut last = MetaTok::Eof;
    loop {
        let tok = md.next();
        if tok == MetaTok::Eof {
            break;
        }
        let take_value = |md: &MetaLexer, name: &mut Option<Vec<u8>>, value: &mut Option<Vec<u8>>, hn: &mut bool, hc: &mut bool| {
            // A C string: the value ends at a NUL.
            let t = match md.token.iter().position(|&b| b == 0) {
                Some(n) => md.token[..n].to_vec(),
                None => md.token.clone(),
            };
            if saw_name {
                *name = Some(meta_name(&t));
                *hn = true;
            } else if saw_content {
                *value = Some(t);
                *hc = true;
            }
        };
        match tok {
            MetaTok::Id => {
                if last == MetaTok::OpenTag {
                    md.in_meta = md.token.eq_ignore_ascii_case(b"meta");
                } else if last == MetaTok::Slash && in_tag {
                    if md.token.eq_ignore_ascii_case(b"head") {
                        break;
                    }
                } else if last == MetaTok::Equal && looking_for_val {
                    take_value(&md, &mut name, &mut value, &mut have_name, &mut have_content);
                    looking_for_val = false;
                } else if md.in_meta {
                    if md.token.eq_ignore_ascii_case(b"name") {
                        (saw_name, saw_content, looking_for_val) = (true, false, true);
                    } else if md.token.eq_ignore_ascii_case(b"content") {
                        (saw_name, saw_content, looking_for_val) = (false, true, true);
                    }
                }
            }
            MetaTok::Str if last == MetaTok::Equal && looking_for_val => {
                take_value(&md, &mut name, &mut value, &mut have_name, &mut have_content);
                looking_for_val = false;
            }
            MetaTok::OpenTag => {
                if looking_for_val {
                    looking_for_val = false;
                    (have_name, saw_name, have_content, saw_content) = (false, false, false, false);
                }
                in_tag = true;
            }
            MetaTok::CloseTag => {
                if have_name {
                    let key = name.take().unwrap_or_default().to_ascii_lowercase();
                    let v = if have_content { value.take().unwrap_or_default() } else { Vec::new() };
                    let key = rphp_value::array_key(&Value::Str(Str::from_vec(key))).unwrap_or(ArrayKey::Int(0));
                    out.set(key, Value::Str(Str::from_vec(v)));
                }
                name = None;
                value = None;
                (in_tag, looking_for_val) = (false, false);
                (have_name, saw_name, have_content, saw_content) = (false, false, false, false);
                md.in_meta = false;
            }
            _ => {}
        }
        last = tok;
    }
    Ok(Value::Array(out))
}

// ---- dl ------------------------------------------------------------------------

/// `dl(string $extension_filename): bool` — rphp has no shared-object
/// extensions to load, so every attempt ends in php's own failure paths:
/// dl disabled, a name with a path in it, or the library not loadable
/// from `extension_dir` (both the name and `name.so` are tried).
fn dl(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    if ctx.ini_get("enable_dl").is_some_and(|v| !ini_bool(v.as_bytes())) {
        ctx.warn("dl(): Dynamically loaded extensions aren't enabled")?;
        return Ok(Value::Bool(false));
    }
    if name.contains(&b'/') {
        ctx.warn("dl(): Temporary module name should contain only filename")?;
        return Ok(Value::Bool(false));
    }
    let dir = ctx.ini_get("extension_dir").unwrap_or("").to_owned();
    let shown = String::from_utf8_lossy(&name).into_owned();
    let why = |path: &str| {
        if std::path::Path::new(path).is_file() {
            format!("{path} (rphp cannot load native extensions)")
        } else {
            format!("{path} (dlopen({path}, 0x0009): tried: '{path}' (no such file))")
        }
    };
    let (a, b) = (format!("{dir}/{shown}"), format!("{dir}/{shown}.so"));
    ctx.warn(&format!(
        "dl(): Unable to load dynamic library '{shown}' (tried: {}, {})",
        why(&a),
        why(&b)
    ))?;
    Ok(Value::Bool(false))
}

// ---- load average, process title -------------------------------------------------

/// The three load averages `getloadavg(3)` reports: `/proc/loadavg` on
/// Linux; elsewhere the kernel's raw `vm.loadavg` (fixed-point counts and
/// their scale, as `sysctl -b` prints the `struct loadavg`), which gives
/// the exact values php shows.
fn load_averages() -> Option<[f64; 3]> {
    if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
        let mut it = text.split_whitespace().map(|w| w.parse::<f64>().ok());
        return Some([it.next()??, it.next()??, it.next()??]);
    }
    let out = std::process::Command::new("/usr/sbin/sysctl").args(["-b", "vm.loadavg"]).output().ok()?;
    let b = out.stdout;
    if !out.status.success() || b.len() < 24 {
        return None;
    }
    let u32_at = |i: usize| f64::from(u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]));
    let mut scale = [0u8; 8];
    scale.copy_from_slice(&b[16..24]);
    let scale = i64::from_le_bytes(scale) as f64;
    if scale <= 0.0 {
        return None;
    }
    Some([u32_at(0) / scale, u32_at(4) / scale, u32_at(8) / scale])
}

/// `sys_getloadavg(): array|false`
fn sys_getloadavg(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let Some(avg) = load_averages() else {
        return Ok(Value::Bool(false));
    };
    let mut a = Array::new();
    for v in avg {
        a.push(Value::Float(v));
    }
    Ok(Value::Array(a))
}

/// The title `cli_set_process_title()` last set. php writes it over the
/// process's argv area, which `ps` then shows; rphp cannot rewrite that
/// memory safely, so only the title the script reads back is kept.
#[derive(Default)]
struct ProcessTitle(Vec<u8>);

const TITLE_SLOT: &str = "standard.process-title";

/// `cli_set_process_title(string $title): bool`
fn cli_set_process_title(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let title = args[0].to_php_bytes();
    ctx.ext.slot::<ProcessTitle>(TITLE_SLOT).0 = title;
    Ok(Value::Bool(true))
}

/// `cli_get_process_title(): ?string`
fn cli_get_process_title(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Str(Str::from_vec(ctx.ext.slot::<ProcessTitle>(TITLE_SLOT).0.clone())))
}

// ---- strptime ------------------------------------------------------------------

/// `strptime(string $timestamp, string $format): array|false` — deprecated
/// since 8.2.
fn strptime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated(
        "Function strptime() is deprecated since 8.2, use date_parse_from_format() (for locale-independent parsing), \
         or IntlDateFormatter::parse() (for locale-dependent parsing) instead",
    )?;
    let ts = args[0].to_php_bytes();
    let format = args[1].to_php_bytes();
    let Some((tm, rest)) = strptime::strptime(&ts, &format) else {
        return Ok(Value::Bool(false));
    };
    let mut a = Array::new();
    for (k, v) in [
        ("tm_sec", tm.sec),
        ("tm_min", tm.min),
        ("tm_hour", tm.hour),
        ("tm_mday", tm.mday),
        ("tm_mon", tm.mon),
        ("tm_year", tm.year),
        ("tm_wday", tm.wday),
        ("tm_yday", tm.yday),
    ] {
        a.set(ArrayKey::str(k.as_bytes()), Value::Int(v));
    }
    a.set(ArrayKey::str(b"unparsed"), Value::Str(Str::from_vec(rest)));
    Ok(Value::Array(a))
}

// ---- hebrev --------------------------------------------------------------------

fn is_heb(c: u8) -> bool {
    (224..=250).contains(&c)
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn is_newline(c: u8) -> bool {
    c == b'\n' || c == b'\r'
}

/// `hebrev(string $string, int $max_chars_per_line = 0): string` — php's
/// visual reordering of logical Hebrew (ISO-8859-8) text, a port of
/// `ext/standard/string.c`.
fn hebrev(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let src = args[0].to_php_bytes();
    let max_chars = args.get(1).map_or(0, Value::to_int);
    if src.is_empty() {
        return Ok(Value::string(b""));
    }
    let len = src.len();
    // The C string, NUL-terminated: `str[len]` is read as a sentinel.
    let s = |i: usize| at(&src, i);
    let punct = |c: u8| c.is_ascii_punctuation();
    let mut heb = vec![0u8; len];
    let mut target = len as isize - 1;
    let mut tmp = 0usize;
    let (mut block_start, mut block_end) = (0usize, 0usize);
    let mut hebrew = is_heb(s(0));
    loop {
        if hebrew {
            while (is_heb(s(tmp + 1)) || is_blank(s(tmp + 1)) || punct(s(tmp + 1)) || s(tmp + 1) == b'\n')
                && block_end < len - 1
            {
                tmp += 1;
                block_end += 1;
            }
            for i in block_start + 1..=block_end + 1 {
                let c = match src[i - 1] {
                    b'(' => b')',
                    b')' => b'(',
                    b'[' => b']',
                    b']' => b'[',
                    b'{' => b'}',
                    b'}' => b'{',
                    b'<' => b'>',
                    b'>' => b'<',
                    b'\\' => b'/',
                    b'/' => b'\\',
                    c => c,
                };
                if target >= 0 {
                    heb[target as usize] = c;
                }
                target -= 1;
            }
            hebrew = false;
        } else {
            while !is_heb(s(tmp + 1)) && s(tmp + 1) != b'\n' && block_end < len - 1 {
                tmp += 1;
                block_end += 1;
            }
            while (is_blank(s(tmp)) || punct(s(tmp))) && s(tmp) != b'/' && s(tmp) != b'-' && block_end > block_start {
                tmp -= 1;
                block_end -= 1;
            }
            let mut i = block_end + 1;
            while i > block_start {
                if target >= 0 {
                    heb[target as usize] = src[i - 1];
                }
                target -= 1;
                i -= 1;
            }
            hebrew = true;
        }
        block_start = block_end + 1;
        if block_end >= len - 1 {
            break;
        }
    }

    let mut out: Vec<u8> = Vec::with_capacity(len);
    let (mut begin, mut end) = (len - 1, len - 1);
    loop {
        let mut char_count: i64 = 0;
        while (max_chars == 0 || (max_chars > 0 && char_count < max_chars)) && begin > 0 {
            char_count += 1;
            begin -= 1;
            if is_newline(heb[begin]) {
                while begin > 0 && is_newline(heb[begin - 1]) {
                    begin -= 1;
                    char_count += 1;
                }
                break;
            }
        }
        if max_chars >= 0 && char_count == max_chars {
            // Try to avoid breaking words.
            let (mut n, mut nb) = (char_count, begin);
            while n > 0 {
                if is_blank(heb[nb]) || is_newline(heb[nb]) {
                    break;
                }
                nb += 1;
                n -= 1;
            }
            if n > 0 {
                begin = nb;
            }
        }
        let orig_begin = begin;
        if is_blank(heb[begin]) {
            heb[begin] = b'\n';
        }
        while begin <= end && is_newline(heb[begin]) {
            begin += 1;
        }
        for &c in heb.iter().take(end + 1).skip(begin) {
            out.push(c);
        }
        let mut i = orig_begin;
        while i <= end && is_newline(heb[i]) {
            out.push(heb[i]);
            i += 1;
        }
        begin = orig_begin;
        if begin == 0 {
            break;
        }
        begin -= 1;
        end = begin;
    }
    Ok(Value::Str(Str::from_vec(out)))
}
