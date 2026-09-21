//! php-src `ext/standard/url.c` + `http.c` (+ `parse_str` from `string.c`,
//! which is the same query-string machinery): `parse_url`, `urlencode`,
//! `urldecode`, `rawurlencode`, `rawurldecode`, `http_build_query`,
//! `parse_str`. `get_headers` needs the network stream layer (S12) and is
//! cataloged.
//!
//! `parse_url` is a transcription of `php_url_parse_ex2` — its scheme /
//! port / `//host` heuristics, control-character replacement and the
//! order components are reported in — and is exercised against php-src's
//! `ext/standard/tests/url` corpus. `parse_str` follows
//! `php_default_treat_data` + `php_register_variable_ex` (`[]` / `[k]`
//! nesting, `.`/space → `_` outside brackets, `max_input_vars`).

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{array_key, Array, ArrayKey, Str, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("parse_url", 1, Some(2), parse_url),
    nf!("urlencode", 1, Some(1), urlencode),
    nf!("urldecode", 1, Some(1), urldecode),
    nf!("rawurlencode", 1, Some(1), rawurlencode),
    nf!("rawurldecode", 1, Some(1), rawurldecode),
    nf!("http_build_query", 1, Some(4), http_build_query),
    nf_ref!("parse_str", 2, Some(2), 0b10, parse_str),
];

/// `PHP_URL_*` and `PHP_QUERY_*`.
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("PHP_URL_SCHEME", 0),
        ("PHP_URL_HOST", 1),
        ("PHP_URL_PORT", 2),
        ("PHP_URL_USER", 3),
        ("PHP_URL_PASS", 4),
        ("PHP_URL_PATH", 5),
        ("PHP_URL_QUERY", 6),
        ("PHP_URL_FRAGMENT", 7),
        ("PHP_QUERY_RFC1738", 1),
        ("PHP_QUERY_RFC3986", 2),
    ] {
        r.constant(name, Value::Int(v));
    }
}

// ---- parse_url --------------------------------------------------------------

/// The components `php_url_parse_ex2` fills in.
#[derive(Default, Debug, PartialEq)]
struct Url {
    scheme: Option<Vec<u8>>,
    host: Option<Vec<u8>>,
    port: Option<u16>,
    user: Option<Vec<u8>>,
    pass: Option<Vec<u8>>,
    path: Option<Vec<u8>>,
    query: Option<Vec<u8>>,
    fragment: Option<Vec<u8>>,
}

/// `php_replace_controlchars_ex`: control characters become `_`.
fn clean(s: &[u8]) -> Vec<u8> {
    s.iter().map(|&b| if b.is_ascii_control() { b'_' } else { b }).collect()
}

/// `strtol`-style port parse: `[+-]?digits`, garbage after the digits is
/// ignored; `None` when no digits were read. Compared to `0..=65535`.
fn parse_port(s: &[u8]) -> Option<i64> {
    let mut i = 0;
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut v: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((s[i] - b'0') as i64);
        i += 1;
    }
    (i > start).then_some(if neg { -v } else { v })
}

/// `php_url_parse_ex2`. `None` is php's `false` (a severely malformed URL).
fn url_parse(str_: &[u8]) -> Option<Url> {
    let ue = str_.len();
    let mut ret = Url::default();
    let mut s = 0usize;
    let mut e;

    // Labels of the C routine, modelled as a small state machine.
    enum Next {
        ParsePort,
        ParseHost,
        JustPath,
    }

    let colon = str_.iter().position(|&b| b == b':');
    let next = if let Some(c) = colon.filter(|&c| c != 0) {
        e = c;
        // Validate the scheme: `1*[ alpha | digit | "+" | "-" | "." ]`.
        let bad = str_[s..e]
            .iter()
            .any(|&b| !(b.is_ascii_alphanumeric() || b == b'+' || b == b'.' || b == b'-'));
        if bad {
            let qh = str_.iter().position(|&b| b == b'?' || b == b'#').unwrap_or(ue);
            if e + 1 < ue && e < qh {
                Next::ParsePort
            } else if s + 1 < ue && str_[s] == b'/' && str_[s + 1] == b'/' {
                // Relative-scheme URL.
                s += 2;
                Next::ParseHost
            } else {
                Next::JustPath
            }
        } else if e + 1 == ue {
            // Only the scheme is present.
            ret.scheme = Some(clean(&str_[s..e]));
            return Some(ret);
        } else if str_[e + 1] != b'/' {
            // `mailto:` / `zlib:` style — unless what follows the colon is
            // a port (`a.com:80`).
            let mut p = e + 1;
            while p < ue && str_[p].is_ascii_digit() {
                p += 1;
            }
            if (p == ue || str_[p] == b'/') && (p - e) < 7 {
                Next::ParsePort
            } else {
                ret.scheme = Some(clean(&str_[s..e]));
                s = e + 1;
                Next::JustPath
            }
        } else {
            ret.scheme = Some(clean(&str_[s..e]));
            if e + 2 < ue && str_[e + 2] == b'/' {
                s = e + 3;
                if str_[..e].eq_ignore_ascii_case(b"file") && e + 3 < ue && str_[e + 3] == b'/' {
                    // `file:///c:/dir` keeps a Windows drive letter in the path.
                    if e + 5 < ue && str_[e + 5] == b':' {
                        s = e + 4;
                    }
                    Next::JustPath
                } else {
                    Next::ParseHost
                }
            } else {
                s = e + 1;
                Next::JustPath
            }
        }
    } else if colon.is_some() {
        Next::ParsePort
    } else if s + 1 < ue && str_[s] == b'/' && str_[s + 1] == b'/' {
        s += 2;
        Next::ParseHost
    } else {
        Next::JustPath
    };

    let next = match next {
        Next::ParsePort => {
            // No scheme; the colon may introduce a port.
            e = colon.expect("parse_port is reached with a colon");
            let p = e + 1;
            let mut pp = p;
            while pp < ue && pp - p < 6 && str_[pp].is_ascii_digit() {
                pp += 1;
            }
            if pp - p > 0 && pp - p < 6 && (pp == ue || str_[pp] == b'/') {
                let port = parse_port(&str_[p..pp])?;
                if !(0..=65535).contains(&port) {
                    return None;
                }
                ret.port = Some(port as u16);
                if s + 1 < ue && str_[s] == b'/' && str_[s + 1] == b'/' {
                    s += 2;
                }
                Next::ParseHost
            } else if p == pp && pp == ue {
                return None;
            } else if s + 1 < ue && str_[s] == b'/' && str_[s + 1] == b'/' {
                s += 2;
                Next::ParseHost
            } else {
                Next::JustPath
            }
        }
        other => other,
    };

    if let Next::ParseHost = next {
        e = s + str_[s..].iter().position(|&b| b == b'/' || b == b'?' || b == b'#').unwrap_or(ue - s);
        // Login and password.
        if let Some(at) = str_[s..e].iter().rposition(|&b| b == b'@') {
            let p = s + at;
            if let Some(c) = str_[s..p].iter().position(|&b| b == b':') {
                let pp = s + c;
                ret.user = Some(clean(&str_[s..pp]));
                ret.pass = Some(clean(&str_[pp + 1..p]));
            } else {
                ret.user = Some(clean(&str_[s..p]));
            }
            s = p + 1;
        }
        // Port — skipped for a bracketed IPv6 literal.
        let p = if s < ue && str_[s] == b'[' && e > 0 && str_[e - 1] == b']' {
            None
        } else {
            str_[s..e].iter().rposition(|&b| b == b':').map(|c| s + c)
        };
        let host_end = match p {
            Some(p) => {
                if ret.port.is_none() {
                    let pstart = p + 1;
                    if e - pstart > 5 {
                        return None;
                    } else if e - pstart > 0 {
                        let port = parse_port(&str_[pstart..e])?;
                        if !(0..=65535).contains(&port) {
                            return None;
                        }
                        ret.port = Some(port as u16);
                    }
                }
                p
            }
            None => e,
        };
        if host_end <= s {
            return None;
        }
        ret.host = Some(clean(&str_[s..host_end]));
        if e == ue {
            return Some(ret);
        }
        s = e;
    }

    // just_path:
    e = ue;
    if let Some(h) = str_[s..e].iter().position(|&b| b == b'#') {
        let p = s + h + 1;
        ret.fragment = Some(clean(&str_[p..e]));
        e = p - 1;
    }
    if let Some(q) = str_[s..e].iter().position(|&b| b == b'?') {
        let p = s + q + 1;
        ret.query = Some(clean(&str_[p..e]));
        e = p - 1;
    }
    if s < e || s == ue {
        ret.path = Some(clean(&str_[s..e]));
    }
    Some(ret)
}

/// `parse_url(string $url, int $component = -1): int|string|array|null|false`
pub(crate) fn parse_url(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let url = args[0].to_php_bytes();
    let component = args.get(1).map_or(-1, Value::to_int);
    if !(-1..=7).contains(&component) {
        return Err(Unwind::value_error(format!(
            "parse_url(): Argument #2 ($component) must be a valid URL component identifier, {component} given"
        )));
    }
    let Some(u) = url_parse(&url) else {
        return Ok(Value::Bool(false));
    };
    let str_or_null = |v: &Option<Vec<u8>>| v.as_ref().map_or(Value::Null, |b| Value::string(b));
    if component >= 0 {
        return Ok(match component {
            0 => str_or_null(&u.scheme),
            1 => str_or_null(&u.host),
            2 => u.port.map_or(Value::Null, |p| Value::Int(p as i64)),
            3 => str_or_null(&u.user),
            4 => str_or_null(&u.pass),
            5 => str_or_null(&u.path),
            6 => str_or_null(&u.query),
            _ => str_or_null(&u.fragment),
        });
    }
    let mut a = Array::new();
    let put = |a: &mut Array, k: &[u8], v: &Option<Vec<u8>>| {
        if let Some(b) = v {
            a.set(ArrayKey::str(k), Value::string(b));
        }
    };
    put(&mut a, b"scheme", &u.scheme);
    put(&mut a, b"host", &u.host);
    if let Some(p) = u.port {
        a.set(ArrayKey::str(b"port"), Value::Int(p as i64));
    }
    put(&mut a, b"user", &u.user);
    put(&mut a, b"pass", &u.pass);
    put(&mut a, b"path", &u.path);
    put(&mut a, b"query", &u.query);
    put(&mut a, b"fragment", &u.fragment);
    Ok(Value::Array(a))
}

// ---- encoding ---------------------------------------------------------------

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// `php_url_encode` (`raw = false`: space → `+`, `~` escaped) /
/// `php_raw_url_encode` (`raw = true`: RFC 3986, `~` kept, space → `%20`).
pub(crate) fn encode(s: &[u8], raw: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &b in s {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' || (raw && b == b'~') {
            out.push(b);
        } else if b == b' ' && !raw {
            out.push(b'+');
        } else {
            out.push(b'%');
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 15) as usize]);
        }
    }
    out
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `php_url_decode` (`plus = true`: `+` → space) / `php_raw_url_decode`. An
/// incomplete or invalid `%xx` is copied through untouched.
fn decode(s: &[u8], plus: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        if b == b'+' && plus {
            out.push(b' ');
        } else if b == b'%' && i + 2 < s.len() {
            match (hexval(s[i + 1]), hexval(s[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h << 4) | l);
                    i += 2;
                }
                _ => out.push(b),
            }
        } else {
            out.push(b);
        }
        i += 1;
    }
    out
}

/// `urlencode(string $string): string`
pub(crate) fn urlencode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Str(Str::from_vec(encode(&args[0].to_php_bytes(), false))))
}

/// `rawurlencode(string $string): string`
pub(crate) fn rawurlencode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Str(Str::from_vec(encode(&args[0].to_php_bytes(), true))))
}

/// `urldecode(string $string): string`
pub(crate) fn urldecode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Str(Str::from_vec(decode(&args[0].to_php_bytes(), true))))
}

/// `rawurldecode(string $string): string`
pub(crate) fn rawurldecode(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Str(Str::from_vec(decode(&args[0].to_php_bytes(), false))))
}

// ---- http_build_query -------------------------------------------------------

/// `http_build_query(array|object $data, string $numeric_prefix = "", ?string $arg_separator = null, int $encoding_type = PHP_QUERY_RFC1738): string`
pub(crate) fn http_build_query(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let prefix = args.get(1).map(Value::to_php_bytes).unwrap_or_default();
    // A null separator means the `arg_separator.output` ini value (an empty
    // string is used as-is).
    let sep = match args.get(2) {
        Some(v) if !matches!(*v.deref(), Value::Null) => v.to_php_bytes(),
        _ => ctx.ini_get("arg_separator.output").unwrap_or("&").as_bytes().to_vec(),
    };
    // Anything but PHP_QUERY_RFC3986 (2) encodes as RFC 1738.
    let raw = args.get(3).map_or(1, Value::to_int) == 2;
    let mut out = Vec::new();
    let data = args[0].deref().into_owned();
    match data {
        Value::Array(a) => build_query(&mut out, &a, &prefix, &sep, raw, None),
        Value::Object(o) => {
            // Public properties, as an array.
            let mut a = Array::new();
            for (name, v, vis) in o.props_snapshot() {
                if vis == rphp_value::Vis::Public {
                    a.set(ArrayKey::Str(name.into()), v);
                }
            }
            build_query(&mut out, &a, &prefix, &sep, raw, None)
        }
        other => {
            return Err(Unwind::type_error(format!(
                "http_build_query(): Argument #1 ($data) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    }
    Ok(Value::Str(Str::from_vec(out)))
}

/// `php_url_encode_hash_ex`: one level of `$data`; `key_prefix` is the
/// already-encoded enclosing key (`a%5Bb%5D`) for nested arrays.
fn build_query(out: &mut Vec<u8>, a: &Array, num_prefix: &[u8], sep: &[u8], raw: bool, key_prefix: Option<&[u8]>) {
    for (k, v) in a.iter() {
        let v = v.deref().into_owned();
        if matches!(v, Value::Null) {
            continue;
        }
        let mut key = Vec::new();
        match key_prefix {
            Some(kp) => {
                key.extend_from_slice(kp);
                key.extend_from_slice(b"%5B");
                match k {
                    ArrayKey::Int(i) => key.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(s) => key.extend_from_slice(&encode(s, raw)),
                }
                key.extend_from_slice(b"%5D");
            }
            None => match k {
                ArrayKey::Int(i) => {
                    // The numeric prefix is spliced in verbatim.
                    key.extend_from_slice(num_prefix);
                    key.extend_from_slice(i.to_string().as_bytes());
                }
                ArrayKey::Str(s) => key.extend_from_slice(&encode(s, raw)),
            },
        }
        match v {
            Value::Array(inner) => build_query(out, &inner, num_prefix, sep, raw, Some(&key)),
            Value::Object(o) => {
                let mut inner = Array::new();
                for (name, pv, vis) in o.props_snapshot() {
                    if vis == rphp_value::Vis::Public {
                        inner.set(ArrayKey::Str(name.into()), pv);
                    }
                }
                build_query(out, &inner, num_prefix, sep, raw, Some(&key));
            }
            scalar => {
                if !out.is_empty() {
                    out.extend_from_slice(sep);
                }
                out.extend_from_slice(&key);
                out.push(b'=');
                let text = match scalar {
                    Value::Bool(b) => vec![if b { b'1' } else { b'0' }],
                    other => other.to_php_bytes(),
                };
                out.extend_from_slice(&encode(&text, raw));
            }
        }
    }
}

// ---- parse_str --------------------------------------------------------------

/// `parse_str(string $string, array &$result): void`
pub(crate) fn parse_str(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let input = args[0].to_php_bytes();
    let separators = ctx.ini_get("arg_separator.input").unwrap_or("&").as_bytes().to_vec();
    let max_vars = ctx.ini.int("max_input_vars");
    let (result, exceeded) = parse_query(&input, &separators, max_vars);
    if exceeded {
        ctx.warn(&format!(
            "parse_str(): Input variables exceeded {max_vars}. To increase the limit change max_input_vars in php.ini."
        ))?;
    }
    Value::assign(&mut args[1], Value::Array(result));
    Ok(Value::Null)
}

/// php's `php_default_treat_data`: a query string (or urlencoded body) as
/// the array `$_GET`/`$_POST`/`parse_str()` build from it. Answers whether
/// `max_input_vars` (when > 0) cut it short.
pub fn parse_query(input: &[u8], separators: &[u8], max_vars: i64) -> (Array, bool) {
    let mut result = Array::new();
    let mut count = 0i64;
    for pair in input.split(|b| separators.contains(b)) {
        if pair.is_empty() {
            continue;
        }
        count += 1;
        if max_vars > 0 && count > max_vars {
            return (result, true);
        }
        let (name, value) = match pair.iter().position(|&b| b == b'=') {
            Some(eq) => (decode(&pair[..eq], true), decode(&pair[eq + 1..], true)),
            None => (decode(pair, true), Vec::new()),
        };
        register_variable(&mut result, &name, Value::Str(Str::from_vec(value)));
    }
    (result, false)
}

/// `php_register_variable_ex`: place `val` under the (possibly nested)
/// variable name `var_name` in `table` — `a[b][]=` and the rest of php's
/// request-variable grammar.
pub fn register_variable(table: &mut Array, var_name: &[u8], val: Value) {
    // Leading spaces are ignored; `.` and space become `_` up to the first
    // `[`, which starts the index list.
    let mut var: Vec<u8> = var_name.iter().skip_while(|&&b| b == b' ').copied().collect();
    let mut bracket = None;
    for (i, b) in var.iter_mut().enumerate() {
        match *b {
            b' ' | b'.' => *b = b'_',
            b'[' => {
                bracket = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(ip) = bracket else {
        if var.is_empty() {
            return;
        }
        set_var(table, Some(&var), val);
        return;
    };
    if ip == 0 {
        return;
    }
    let mut index: Option<Vec<u8>> = Some(var[..ip].to_vec());
    let mut cur = ip; // at '['
    // Each iteration descends one `[...]` level.
    let mut path: Vec<Option<Vec<u8>>> = Vec::new();
    loop {
        cur += 1; // past '['
        let index_s = cur;
        let mut probe = cur;
        if probe < var.len() && is_c_space(var[probe]) {
            probe += 1;
        }
        let new_index: Option<Vec<u8>>;
        if probe < var.len() && var[probe] == b']' {
            new_index = None;
            cur = probe;
        } else {
            match var[index_s..].iter().position(|&b| b == b']') {
                Some(off) => {
                    new_index = Some(var[index_s..index_s + off].to_vec());
                    cur = index_s + off;
                }
                None => {
                    // Not an index after all: the name up to here, with the
                    // `[` and everything after it flattened to `_`.
                    let mut flat = var[..index_s - 1].to_vec();
                    flat.push(b'_');
                    flat.extend(var[index_s..].iter().map(|&b| match b {
                        b' ' | b'.' | b'[' => b'_',
                        other => other,
                    }));
                    // Only reachable at the first level (deeper levels come
                    // from a full `[..]` match), so `index` is the base name.
                    if path.is_empty() {
                        set_var(table, Some(&flat), val);
                    } else {
                        let base = index.clone();
                        set_nested(table, &path, base.as_deref(), val);
                    }
                    return;
                }
            }
        }
        path.push(index.take());
        index = new_index;
        cur += 1; // past ']'
        if cur < var.len() && var[cur] == b'[' {
            continue;
        }
        break;
    }
    set_nested(table, &path, index.as_deref(), val);
}

/// Insert under `index` (`None` = append).
fn set_var(table: &mut Array, index: Option<&[u8]>, val: Value) {
    match index {
        None => table.push(val),
        Some(k) => table.set(array_key(&Value::string(k)).expect("string key"), val),
    }
}

/// Descend `path` (creating / replacing non-arrays with arrays as php does)
/// and insert `val` under `leaf`.
fn set_nested(table: &mut Array, path: &[Option<Vec<u8>>], leaf: Option<&[u8]>, val: Value) {
    let Some((first, rest)) = path.split_first() else {
        set_var(table, leaf, val);
        return;
    };
    let key = match first {
        None => ArrayKey::Int(table.next_free_index()),
        Some(k) => array_key(&Value::string(k)).expect("string key"),
    };
    let mut inner = match table.get_deref(&key) {
        Some(Value::Array(a)) => a,
        _ => Array::new(),
    };
    set_nested(&mut inner, rest, leaf, val);
    table.set(key, Value::Array(inner));
}

/// C `isspace` in the C locale: space, `\t`, `\n`, `\v`, `\f`, `\r`
/// (Rust's `is_ascii_whitespace` leaves out the vertical tab).
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[cfg(test)]
mod tests {
    use rphp_value::{ArrayKey, Value};

    use crate::tests::{call_named, interp};

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    fn parsed(url: &str) -> String {
        let v = call_named(b"parse_url", &[s(url)]);
        match v {
            Value::Array(a) => a
                .iter()
                .map(|(k, v)| format!("{}={}", String::from_utf8_lossy(&k.to_value().to_php_bytes()), v.to_php_string()))
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_php_string(),
        }
    }

    #[test]
    fn parse_url_matches_php_quirks() {
        assert_eq!(parsed("http://user:pw@host:8080/p/a?q=1#f"), "scheme=http host=host port=8080 user=user pass=pw path=/p/a query=q=1 fragment=f");
        assert_eq!(parsed("64.246.30.37:80/"), "host=64.246.30.37 port=80 path=/");
        assert_eq!(parsed("mailto:me@x.com"), "scheme=mailto path=me@x.com");
        assert_eq!(parsed("//x"), "host=x");
        assert_eq!(parsed("host:"), "scheme=host");
        assert_eq!(parsed("http://x/?"), "scheme=http host=x path=/ query=");
        assert_eq!(parsed("http://x/#"), "scheme=http host=x path=/ fragment=");
        assert_eq!(parsed(""), "path=");
        assert_eq!(parsed("http://[::1]:80/"), "scheme=http host=[::1] port=80 path=/");
        assert_eq!(parsed("a:b:c"), "scheme=a path=b:c");
        assert_eq!(parsed("file:///c:/dir"), "scheme=file path=c:/dir");
        assert_eq!(parsed("http://host:1a"), "scheme=http host=host port=1");
        assert_eq!(parsed("http://a\x01b/c"), "scheme=http host=a_b path=/c");
        for bad in ["http:///blah.com", "http://:80", "http://blah.com:123456", "http://blah.com:abcdef", "http://:", "http://@/", "http://?"] {
            assert_eq!(call_named(b"parse_url", &[s(bad)]), Value::Bool(false), "{bad}");
        }
        assert_eq!(call_named(b"parse_url", &[s("http://x/p"), Value::Int(5)]), s("/p"));
        assert_eq!(call_named(b"parse_url", &[s("http://x/p"), Value::Int(2)]), Value::Null);
    }

    #[test]
    fn encoders_match_php() {
        assert_eq!(call_named(b"urlencode", &[s("a b+c~d\0")]), s("a+b%2Bc%7Ed%00"));
        assert_eq!(call_named(b"rawurlencode", &[s("a b+c~d")]), s("a%20b%2Bc~d"));
        assert_eq!(call_named(b"urldecode", &[s("a+b%20c%2Fd%zz%2")]), s("a b c/d%zz%2"));
        assert_eq!(call_named(b"rawurldecode", &[s("a+b%41")]), s("a+bA"));
    }

    #[test]
    fn http_build_query_nests_and_encodes() {
        let mut a = rphp_value::Array::new();
        a.set(ArrayKey::str(b"a"), Value::Int(1));
        a.set(ArrayKey::str(b"b"), s("x y"));
        let mut c = rphp_value::Array::new();
        c.push(Value::Int(1));
        c.set(ArrayKey::str(b"k"), Value::Bool(true));
        a.set(ArrayKey::str(b"c"), Value::Array(c));
        a.set(ArrayKey::str(b"d"), Value::Null);
        a.set(ArrayKey::str(b"e"), Value::Bool(false));
        assert_eq!(call_named(b"http_build_query", &[Value::Array(a.clone())]), s("a=1&b=x+y&c%5B0%5D=1&c%5Bk%5D=1&e=0"));
        assert_eq!(
            call_named(b"http_build_query", &[Value::Array(a), s(""), s(";"), Value::Int(2)]),
            s("a=1;b=x%20y;c%5B0%5D=1;c%5Bk%5D=1;e=0")
        );
        let mut l = rphp_value::Array::new();
        l.push(Value::Int(1));
        l.push(Value::Int(2));
        assert_eq!(call_named(b"http_build_query", &[Value::Array(l), s("p_")]), s("p_0=1&p_1=2"));
    }

    #[test]
    fn parse_str_builds_nested_arrays() {
        let mut it = interp();
        let mut args = [s("a=1&b[]=2&b[]=3&c[k]=v&d.e=f&j[k][l]=m&n&p[=q&r]=s&t[a]b=u&x[1]=a&x[]=b&&=z"), Value::Null];
        it.call_native(it.native_by_name(b"parse_str").unwrap(), &mut args).unwrap();
        let Value::Array(r) = &args[1] else { panic!() };
        let keys: Vec<String> = r.keys().map(|k| String::from_utf8_lossy(&k.to_value().to_php_bytes()).into_owned()).collect();
        assert_eq!(keys, ["a", "b", "c", "d_e", "j", "n", "p_", "r]", "t", "x"]);
        let Value::Array(b) = r.get_deref(&ArrayKey::str(b"b")).unwrap() else { panic!() };
        assert_eq!(b.len(), 2);
        let Value::Array(j) = r.get_deref(&ArrayKey::str(b"j")).unwrap() else { panic!() };
        let Value::Array(jk) = j.get_deref(&ArrayKey::str(b"k")).unwrap() else { panic!() };
        assert_eq!(jk.get_deref(&ArrayKey::str(b"l")).unwrap(), s("m"));
        let Value::Array(x) = r.get_deref(&ArrayKey::str(b"x")).unwrap() else { panic!() };
        assert_eq!(x.get_deref(&ArrayKey::Int(2)).unwrap(), s("b"));
        assert_eq!(r.get_deref(&ArrayKey::str(b"n")).unwrap(), s(""));
    }
}
