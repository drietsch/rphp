//! The `http://` and `https://` stream wrapper (php-src
//! `ext/standard/http_fopen_wrapper.c`).
//!
//! `fopen()`, `file_get_contents()`, `file()` and `readfile()` reach the
//! network through this: a tcp connection from `socket.rs`, a request built
//! from the `http` stream-context options, and a response whose headers are
//! parsed off the front. What is left of the socket stays *live*, so the
//! body is read as it arrives rather than buffered whole — php's
//! `unread_bytes` is exactly the part of it already pulled in while looking
//! for the end of the headers.
//!
//! The response's header lines are reported three ways, all of which php
//! fills: the `wrapper_data` key of `stream_get_meta_data()` (which is
//! where Symfony's `NativeHttpClient` reads them), the function
//! `http_get_last_response_headers()`, and the `$http_response_header`
//! variable php drops into the calling scope.
//!
//! **Redirects** are followed by default, up to `max_redirects` (20), and
//! the headers of *every* hop accumulate in that order, as php's do.
//!
//! **Status** is not an error by default only for 2xx and 3xx: php refuses
//! to open the stream on a 4xx or 5xx unless `ignore_errors` is set, and
//! the warning it gives quotes the status line.
//!
//! **Known divergence.** php 8.5 deprecates `$http_response_header`, and
//! does it in the *compiler*: a scope that reads the variable without ever
//! writing it — the shape that can only have come from this wrapper — gets
//! the notice once, before the script runs. That is a compiler change for
//! a diagnostic Symfony never triggers (it reads `wrapper_data`), so the
//! variable is populated here and the notice is not emitted.

use std::time::Duration;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use crate::file::Conn;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("http_get_last_response_headers", 0, Some(0), http_get_last_response_headers),
    nf!("http_clear_last_response_headers", 0, Some(0), http_clear_last_response_headers),
    nf!("get_headers", 1, Some(3), get_headers),
];

thread_local! {
    /// What `http_get_last_response_headers()` answers: the header lines of
    /// the last response *this request* received. A server's worker thread
    /// outlives a request, so `request_shutdown` clears it.
    static LAST_HEADERS: std::cell::RefCell<Option<Array>> = const {
        std::cell::RefCell::new(None)
    };
}

/// php's `RSHUTDOWN` for this module (see `lib.rs::request_shutdown`).
pub(crate) fn request_shutdown() {
    LAST_HEADERS.with(|h| *h.borrow_mut() = None);
}

/// `http_get_last_response_headers(): ?array`
fn http_get_last_response_headers(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(LAST_HEADERS.with(|h| match &*h.borrow() {
        Some(a) => Value::Array(a.clone()),
        None => Value::Null,
    }))
}

/// `http_clear_last_response_headers(): void`
fn http_clear_last_response_headers(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    request_shutdown();
    Ok(Value::Null)
}

// ---- the request -------------------------------------------------------------------

/// Whether `allow_url_fopen` lets this wrapper run at all. php refuses
/// before it looks at the url, and says so twice: once about the wrapper
/// being switched off, once about there being no wrapper left to use.
///
/// `Ok(())` when it may proceed; `Err` carries the tail of the second
/// warning, the first having been emitted already.
fn url_fopen_allowed(ctx: &mut Ctx, url: &str, func: &str) -> Result<bool, Unwind> {
    if ctx.ini_get("allow_url_fopen").is_none_or(|v| v != "0") {
        return Ok(true);
    }
    let scheme = url.split(':').next().unwrap_or("http").to_ascii_lowercase();
    ctx.warn(&format!(
        "{func}(): {scheme}:// wrapper is disabled in the server configuration by allow_url_fopen=0"
    ))?;
    Ok(false)
}

/// Whether a path names this wrapper at all.
///
/// Every `fopen()` and `file_get_contents()` in a request asks this, almost
/// always about a local path, so it looks at the bytes rather than folding
/// a copy of the whole path to lower case.
pub(crate) fn is_http_url(url: &str) -> bool {
    let b = url.as_bytes();
    b.len() > 7 && b[..7].eq_ignore_ascii_case(b"http://")
        || b.len() > 8 && b[..8].eq_ignore_ascii_case(b"https://")
}

/// The `http` options of a stream context, flattened into what the request
/// builder needs.
#[derive(Clone)]
struct Options {
    method: String,
    headers: Vec<String>,
    content: Vec<u8>,
    user_agent: Option<String>,
    protocol_version: String,
    follow_location: bool,
    max_redirects: i64,
    ignore_errors: bool,
    timeout: Option<f64>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            method: "GET".into(),
            headers: Vec::new(),
            content: Vec::new(),
            user_agent: None,
            // php's wrapper has spoken HTTP/1.1 since 8.0. It still sends
            // `Connection: close`, so a server has no reason to chunk, but
            // one that does anyway is decoded below.
            protocol_version: "1.1".into(),
            follow_location: true,
            max_redirects: 20,
            ignore_errors: false,
            timeout: None,
        }
    }
}

/// Read the `http` section of a stream context.
fn options_of(ctx: &mut Ctx, context: Option<&Value>) -> Result<Options, Unwind> {
    let mut o = Options::default();
    let Some(v) = context else { return Ok(o) };
    let Some(all) = crate::file::context_options_of(ctx, v) else { return Ok(o) };
    let Some(http) = all.get(&ArrayKey::str(b"http")) else { return Ok(o) };
    let Value::Array(http) = &*http.deref() else { return Ok(o) };
    let text = |v: &Value| String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
    for (k, v) in http.iter() {
        let ArrayKey::Str(name) = k else { continue };
        let v = v.deref().into_owned();
        match name.as_ref() {
            b"method" => o.method = text(&v).to_ascii_uppercase(),
            b"content" => o.content = v.to_php_bytes().to_vec(),
            b"user_agent" => o.user_agent = Some(text(&v)),
            b"protocol_version" => {
                // A float option: `1.1` must not become `1`.
                o.protocol_version = match &v {
                    Value::Float(f) => format!("{f:.1}"),
                    other => text(other),
                };
            }
            b"follow_location" => o.follow_location = v.to_int() != 0,
            b"max_redirects" => o.max_redirects = v.to_int(),
            b"ignore_errors" => o.ignore_errors = v.to_bool(),
            b"timeout" => o.timeout = Some(v.to_float()),
            b"header" => match &v {
                // php takes either one string of `\r\n`-joined lines or an
                // array of them.
                Value::Array(a) => {
                    for (_, line) in a.iter() {
                        o.headers.push(String::from_utf8_lossy(&line.to_php_bytes()).into_owned());
                    }
                }
                other => {
                    for line in text(other).split("\r\n") {
                        if !line.trim().is_empty() {
                            o.headers.push(line.to_string());
                        }
                    }
                }
            },
            _ => {}
        }
    }
    Ok(o)
}

/// A url split the way the request needs it.
struct Url {
    host: String,
    port: u16,
    /// `path?query`, always at least `/`.
    target: String,
    secure: bool,
}

fn parse_url(url: &str) -> Option<Url> {
    let (scheme, rest) = url.split_once("://")?;
    let secure = scheme.eq_ignore_ascii_case("https");
    let (authority, target) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    // Credentials in the authority are not part of the host.
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.strip_prefix('[') {
        // A bracketed IPv6 literal.
        Some(_) => {
            let end = authority.find(']')?;
            let host = authority[1..end].to_string();
            let port = authority[end + 1..].strip_prefix(':').and_then(|p| p.parse().ok());
            (host, port)
        }
        None => match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()),
            None => (authority.to_string(), None),
        },
    };
    if host.is_empty() {
        return None;
    }
    Some(Url {
        host,
        port: port.unwrap_or(if secure { 443 } else { 80 }),
        target,
        secure,
    })
}

/// The `Host:` header value, which carries the port only when it is not the
/// scheme's default.
fn host_header(u: &Url) -> String {
    let default = if u.secure { 443 } else { 80 };
    if u.port == default {
        u.host.clone()
    } else {
        format!("{}:{}", u.host, u.port)
    }
}

/// One response, as far as the wrapper cares.
struct Response {
    status: u16,
    /// The status line and every header line, in the order received.
    lines: Vec<String>,
    /// The body already read while looking for the end of the headers.
    prefix: Vec<u8>,
    conn: Conn,
    /// Where a redirect points, when this is one.
    location: Option<String>,
}

/// Everything that can stop a fetch.
enum FetchError {
    /// A failure, worded as php words the tail of its
    /// `Failed to open stream: …` warning.
    Failed(String),
    /// A user error handler threw out of a diagnostic the fetch emitted.
    Unwound(Unwind),
}

impl From<Unwind> for FetchError {
    fn from(u: Unwind) -> FetchError {
        FetchError::Unwound(u)
    }
}

/// Send one request and read its head.
fn fetch_once(
    url: &Url,
    o: &Options,
    timeout: Duration,
    ssl: &crate::tls::SslOptions,
) -> Result<Response, FetchError> {
    let mut conn =
        crate::socket::connect_for_wrapper(&url.host, url.port, url.secure, timeout, ssl)
            .map_err(|(_, text)| FetchError::Failed(text))?;
    // php's `timeout` option bounds the reads too, not just the connect, so
    // a server that accepts and then says nothing cannot hang the request.
    crate::socket::set_read_timeout_on(&conn, Some(timeout));

    // php sends the caller's headers as given, and supplies Host and
    // Connection only when the caller did not.
    let named = |name: &str| {
        o.headers
            .iter()
            .any(|h| h.to_ascii_lowercase().starts_with(&format!("{}:", name.to_ascii_lowercase())))
    };
    let mut req = format!("{} {} HTTP/{}\r\n", o.method, url.target, o.protocol_version);
    if !named("host") {
        req.push_str(&format!("Host: {}\r\n", host_header(url)));
    }
    if let Some(ua) = &o.user_agent {
        if !named("user-agent") {
            req.push_str(&format!("User-Agent: {ua}\r\n"));
        }
    }
    if !o.content.is_empty() {
        if !named("content-length") {
            req.push_str(&format!("Content-Length: {}\r\n", o.content.len()));
        }
        // php supplies a content type for a body that came without one,
        // and says so.
        if !named("content-type") {
            req.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
        }
    }
    for h in &o.headers {
        req.push_str(h);
        req.push_str("\r\n");
    }
    if !named("connection") {
        req.push_str("Connection: close\r\n");
    }
    req.push_str("\r\n");

    let mut out = req.into_bytes();
    out.extend_from_slice(&o.content);
    conn.write_all(&out).map_err(|e| FetchError::Failed(io_text(&e)))?;

    read_head(conn)
}

/// Read up to the blank line that ends the headers, keeping whatever of the
/// body came with it.
fn read_head(mut conn: Conn) -> Result<Response, FetchError> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let head_end = loop {
        if let Some(i) = find_head_end(&buf) {
            break i;
        }
        let mut chunk = [0u8; 4096];
        match conn.read_once(&mut chunk) {
            Ok(0) => {
                // The peer closed before any blank line: php treats a
                // response it cannot read as no response at all.
                return Err(FetchError::Failed("HTTP request failed! ".into()));
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FetchError::Failed(io_text(&e))),
        }
    };
    let (head, rest) = buf.split_at(head_end);
    let text = String::from_utf8_lossy(head).into_owned();
    let mut lines: Vec<String> = Vec::new();
    for line in text.split("\r\n").flat_map(|l| l.split('\n')) {
        if !line.is_empty() {
            lines.push(line.trim_end_matches('\r').to_string());
        }
    }
    if lines.is_empty() {
        return Err(FetchError::Failed("HTTP request failed! ".into()));
    }
    let status = status_of(&lines[0]).unwrap_or(0);
    let location = header_value(&lines, "location");
    Ok(Response {
        status,
        lines,
        prefix: rest.to_vec(),
        conn,
        location,
    })
}

/// The offset just past the blank line that ends a response's head, in
/// either line ending, since a server may use bare `\n`.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The number in `HTTP/1.1 200 OK`.
fn status_of(line: &str) -> Option<u16> {
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// The value of the last header with this name.
fn header_value(lines: &[String], name: &str) -> Option<String> {
    let want = format!("{}:", name.to_ascii_lowercase());
    lines
        .iter()
        .skip(1)
        .filter(|l| l.to_ascii_lowercase().starts_with(&want))
        .next_back()
        .map(|l| l[want.len()..].trim().to_string())
}

/// The text php shows for a failed read or write, without Rust's trailing
/// `(os error N)`.
fn io_text(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// A relative `Location:` resolved against the url it came from.
fn resolve_location(base: &Url, location: &str) -> Option<Url> {
    if location.contains("://") {
        return parse_url(location);
    }
    let scheme = if base.secure { "https" } else { "http" };
    let target = if location.starts_with('/') {
        location.to_string()
    } else {
        let dir = match base.target.rfind('/') {
            Some(i) => &base.target[..=i],
            None => "/",
        };
        format!("{dir}{location}")
    };
    parse_url(&format!("{scheme}://{}{target}", host_header(base)))
}

/// Fetch a url, following redirects, and hand back the final response with
/// every hop's header lines in front of it.
fn fetch(
    ctx: &mut Ctx,
    url: &str,
    o: &Options,
    func: &str,
    ssl: &crate::tls::SslOptions,
) -> Result<Response, FetchError> {
    let timeout = crate::socket::wrapper_timeout(ctx, o.timeout);
    let mut current = parse_url(url).ok_or_else(|| FetchError::Failed("Invalid url".into()))?;
    let mut o = o.clone();
    let mut all_lines: Vec<String> = Vec::new();
    let mut hops = 0;
    loop {
        // php runs this check per request, not per call: a 307 that repeats
        // the body says it again, while a 302 that dropped the body does
        // not.
        content_type_notice(ctx, &o, func)?;
        let mut r = fetch_once(&current, &o, timeout, ssl)?;
        all_lines.extend(r.lines.iter().cloned());
        let redirect = matches!(r.status, 301 | 302 | 303 | 307 | 308);
        if !(redirect && o.follow_location && hops < o.max_redirects) {
            r.lines = all_lines;
            return Ok(r);
        }
        let Some(next) = r.location.as_deref().and_then(|l| resolve_location(&current, l)) else {
            r.lines = all_lines;
            return Ok(r);
        };
        // php follows 301, 302 and 303 the way a browser does — as a `GET`
        // with the body and the headers that described it dropped — and
        // follows 307 and 308 with the request untouched.
        if matches!(r.status, 301 | 302 | 303) {
            o.method = "GET".into();
            o.content.clear();
            o.headers.retain(|h| {
                let l = h.to_ascii_lowercase();
                !l.starts_with("content-type:") && !l.starts_with("content-length:")
            });
        }
        current = next;
        hops += 1;
    }
}

// ---- opening a stream --------------------------------------------------------------

/// Open `url` as a stream, or hand back the tail of php's
/// `Failed to open stream: …` warning.
///
/// `func` names the caller for the warning php emits about a status it
/// will not open.
/// Open `url` as a stream. `Ok(None)` means php's `false`: the warning
/// explaining it has already been emitted, here, so an error handler that
/// throws unwinds from where it was called rather than being discarded.
pub(crate) fn open(
    ctx: &mut Ctx,
    url: &str,
    context: Option<&Value>,
    func: &str,
) -> Result<Option<Value>, Unwind> {
    if !url_fopen_allowed(ctx, url, func)? {
        return failed(ctx, func, url, "no suitable wrapper could be found");
    }
    let o = options_of(ctx, context)?;
    let ssl = crate::socket::ssl_options_of(ctx, context);
    let r = match fetch(ctx, url, &o, func, &ssl) {
        Ok(r) => r,
        Err(FetchError::Failed(text)) => return failed(ctx, func, url, &text),
        Err(FetchError::Unwound(u)) => return Err(u),
    };

    // The header lines reach the script three ways, all of them php's.
    let mut data = Array::new();
    for (i, line) in r.lines.iter().enumerate() {
        data.set(ArrayKey::Int(i as i64), Value::string(line.as_bytes()));
    }
    LAST_HEADERS.with(|h| *h.borrow_mut() = Some(data.clone()));
    if let Some(symtab) = ctx.current_symtab() {
        let cell = symtab.get_or_create(b"http_response_header");
        cell.set(Value::Array(data.clone()));
    }

    // php opens 2xx and 3xx; anything else needs `ignore_errors`, and the
    // warning quotes the status line with the newline php leaves on it.
    if !o.ignore_errors && !(200..400).contains(&r.status) {
        // php quotes the status line *as it came off the wire*, `\r` and
        // all, though the copy it keeps in `wrapper_data` is trimmed.
        let reason = format!("HTTP request failed! {}\r\n", r.lines[0]);
        return failed(ctx, func, url, &reason);
    }

    Ok(Some(crate::file::socket_resource_with(
        ctx,
        r.conn,
        crate::file::SockKind::Tcp,
        Some(url.to_string()),
        r.prefix,
        Some(("http".into(), data)),
    )))
}

/// php's `Failed to open stream` warning, and the `false` that follows it.
fn failed(ctx: &mut Ctx, func: &str, url: &str, reason: &str) -> Result<Option<Value>, Unwind> {
    ctx.warn(&format!("{func}({url}): Failed to open stream: {reason}"))?;
    Ok(None)
}

/// Read a url whole, for `file_get_contents()` and its neighbours.
pub(crate) fn read_all(
    ctx: &mut Ctx,
    url: &str,
    context: Option<&Value>,
    func: &str,
) -> Result<Option<Vec<u8>>, Unwind> {
    let Some(handle) = open(ctx, url, context, func)? else {
        return Ok(None);
    };
    let body = crate::file::drain_stream(ctx, &handle);
    ctx.resources.close_value(&handle);
    Ok(Some(body))
}

/// `get_headers(string $url, bool $associative = false, ?resource $context = null): array|false`
fn get_headers(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let url = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let associative = args.get(1).is_some_and(Value::to_bool);
    if !is_http_url(&url) {
        ctx.warn(&format!(
            "get_headers(): Argument #1 ($url) must be a valid URL, \"{url}\" given"
        ))?;
        return Ok(Value::Bool(false));
    }
    if !url_fopen_allowed(ctx, &url, "get_headers")? {
        ctx.warn("get_headers(): This function may only be used against URLs")?;
        return Ok(Value::Bool(false));
    }
    // php asks for the head with the ordinary wrapper, ignoring the status.
    let mut o = options_of(ctx, args.get(2))?;
    o.ignore_errors = true;
    let ssl = crate::socket::ssl_options_of(ctx, args.get(2));
    let r = match fetch(ctx, &url, &o, "get_headers", &ssl) {
        Ok(r) => r,
        Err(FetchError::Failed(text)) => {
            ctx.warn(&format!("get_headers(): Request failed: {text}"))?;
            return Ok(Value::Bool(false));
        }
        Err(FetchError::Unwound(u)) => return Err(u),
    };
    let mut out = Array::new();
    if !associative {
        for (i, line) in r.lines.iter().enumerate() {
            out.set(ArrayKey::Int(i as i64), Value::string(line.as_bytes()));
        }
        return Ok(Value::Array(out));
    }
    // Associative: the status line keeps its numeric slot, and a header
    // seen twice becomes an array of its values.
    let mut status_at = 0i64;
    for line in &r.lines {
        match line.split_once(':') {
            None => {
                out.set(ArrayKey::Int(status_at), Value::string(line.as_bytes()));
                status_at += 1;
            }
            Some((name, value)) => {
                let key = ArrayKey::str(name.trim().as_bytes());
                let v = Value::string(value.trim().as_bytes());
                match out.get(&key).map(|e| e.deref().into_owned()) {
                    None => out.set(key, v),
                    Some(Value::Array(mut prev)) => {
                        let n = prev.len() as i64;
                        prev.set(ArrayKey::Int(n), v);
                        out.set(key, Value::Array(prev));
                    }
                    Some(first) => {
                        let mut both = Array::new();
                        both.set(ArrayKey::Int(0), first);
                        both.set(ArrayKey::Int(1), v);
                        out.set(key, Value::Array(both));
                    }
                }
            }
        }
    }
    Ok(Value::Array(out))
}

/// php supplies a content type for a body that came without one, and says
/// so, naming the function that is opening the stream.
fn content_type_notice(ctx: &mut Ctx, o: &Options, func: &str) -> Result<(), Unwind> {
    if o.content.is_empty()
        || o.headers.iter().any(|h| h.to_ascii_lowercase().starts_with("content-type:"))
    {
        return Ok(());
    }
    ctx.notice(&format!(
        "{func}(): Content-type not specified assuming application/x-www-form-urlencoded"
    ))
}
