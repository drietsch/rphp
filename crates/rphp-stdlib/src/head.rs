//! The response-header surface (php-src `ext/standard/head.c`).
//!
//! The head lives in [`rphp_runtime::ResponseHead`], shared between the
//! interpreter and the SAPI's sink: the natives here fill it, the sink sends
//! it ahead of the first output byte. Two things decide what a call does:
//!
//! * **Whether the head is gone.** Under a web SAPI that is the moment the
//!   sink wrote it — the first bytes that left the output buffers, or an
//!   explicit `flush()`, which sends the head even while `output_buffering`
//!   still holds every byte. After that every call here is php's `Cannot
//!   modify header information - headers already sent` (naming where the
//!   output started, when it was output) and does nothing.
//! * **Which SAPI.** The CLI has nowhere to send a header, and php's CLI
//!   header handler drops the line rather than keep it: `headers_list()` is
//!   always empty there, while the refusal above still fires. The status
//!   code is kept everywhere (`http_response_code()` answers it).

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, SapiKind, Unwind};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("header", 1, Some(3), header),
    nf!("header_remove", 0, Some(1), header_remove),
    nf!("headers_list", 0, Some(0), headers_list),
    nf_ref!("headers_sent", 0, Some(2), 0b11, headers_sent),
    nf!("http_response_code", 0, Some(1), http_response_code),
    nf!("setcookie", 1, Some(7), setcookie),
    nf!("setrawcookie", 1, Some(7), setrawcookie),
];

/// The functions only a web SAPI defines (`php -S` and Apache have them,
/// the CLI does not).
static SERVER_FUNCTIONS: &[NativeFn] = &[
    nf!("getallheaders", 0, Some(0), getallheaders),
    nf!("apache_request_headers", 0, Some(0), getallheaders),
    nf!("apache_response_headers", 0, Some(0), apache_response_headers),
];

/// The FastCGI SAPI's own function.
static FCGI_FUNCTIONS: &[NativeFn] = &[nf!("fastcgi_finish_request", 0, Some(0), fastcgi_finish_request)];

/// Register the web-only functions when the SAPI is one.
pub(crate) fn register_server_functions(r: &mut Registry) {
    if r.interp().sapi.is_web() {
        r.functions(SERVER_FUNCTIONS);
    }
    if r.interp().sapi == SapiKind::Fcgi {
        r.functions(FCGI_FUNCTIONS);
    }
}

/// `fastcgi_finish_request(): bool` — flush every output buffer and the
/// head, end the response, and go on running with output discarded.
fn fastcgi_finish_request(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.finish_output();
    Ok(Value::Bool(ctx.out.finish_request()))
}

/// php's complaint, with the place the output started when there was any
/// (a head sent by `flush()` has no such place).
fn already_sent(ctx: &Ctx) -> String {
    match &ctx.output_started {
        Some((file, line)) => format!(" by (output started at {file}:{line})"),
        None => String::new(),
    }
}

/// Whether the head is gone: bytes reached the SAPI, or it sent the head
/// on its own.
pub(crate) fn head_sent(ctx: &mut Ctx) -> bool {
    ctx.note_output();
    ctx.out.sent() || ctx.head.lock().map(|h| h.sent).unwrap_or(false)
}

/// Whether the headers are gone, warning the way `header()` does.
fn refuse(ctx: &mut Ctx) -> Result<bool, Unwind> {
    if !head_sent(ctx) {
        return Ok(false);
    }
    let msg = format!(
        "Cannot modify header information - headers already sent{}",
        already_sent(ctx)
    );
    ctx.warn(&msg)?;
    Ok(true)
}

/// Whether the SAPI keeps header lines at all.
fn keeps_headers(ctx: &Ctx) -> bool {
    ctx.sapi != SapiKind::Cli
}

/// php's `sapi_header_op(SAPI_HEADER_ADD | SAPI_HEADER_REPLACE)`: the
/// status-line and `Content-Type`/`Location` rules, then the list.
pub(crate) fn add_header(ctx: &mut Ctx, line: &str, replace: bool) {
    // `HTTP/1.1 404 Not Found` is a status line, not a field: php reads the
    // code out of it and keeps the line as the response's own status line.
    if let Some(rest) = line.strip_prefix("HTTP/") {
        if let Some(code) = rest.split_whitespace().nth(1).and_then(|c| c.parse().ok()) {
            let mut head = ctx.head.lock().unwrap();
            head.code = code;
            head.status_line = Some(line.to_string());
        }
        return;
    }
    let Some((name, value)) = line.split_once(':') else {
        return;
    };
    let name = name.trim();
    let mut line = line.to_string();
    if name.eq_ignore_ascii_case("Content-Type") {
        // A `text/*` type without a charset gets the default one, and php
        // respells the field while it is at it.
        let mime = value.trim();
        let charset = ctx.ini.get("default_charset").unwrap_or("").to_string();
        if !charset.is_empty()
            && mime.len() >= 5
            && mime[..5].eq_ignore_ascii_case("text/")
            && !mime.to_ascii_lowercase().contains("charset=")
        {
            line = format!("Content-type: {mime};charset={charset}");
        }
        ctx.head.lock().unwrap().has_content_type = true;
    } else if name.eq_ignore_ascii_case("Location") {
        // A redirect, unless the script chose a status of its own.
        let mut head = ctx.head.lock().unwrap();
        if head.code == 0 || head.code == 200 {
            head.code = 302;
        }
    }
    if !keeps_headers(ctx) {
        return;
    }
    let mut head = ctx.head.lock().unwrap();
    if replace {
        head.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }
    head.headers.push((name.to_string(), line));
}

/// `header(string $header, bool $replace = true, int $response_code = 0): void`
fn header(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if refuse(ctx)? {
        return Ok(Value::Null);
    }
    let line = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let replace = args.get(1).is_none_or(|v| v.deref().to_bool());
    let code = args.get(2).map_or(0, |v| v.deref().to_int());
    add_header(ctx, &line, replace);
    if code > 0 {
        ctx.head.lock().unwrap().code = code;
    }
    Ok(Value::Null)
}

/// `header_remove(?string $name = null): void` — one field, or all of them.
fn header_remove(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if refuse(ctx)? {
        return Ok(Value::Null);
    }
    let mut head = ctx.head.lock().unwrap();
    match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Str(s)) => {
            let name = String::from_utf8_lossy(s.as_bytes()).into_owned();
            head.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
            if name.eq_ignore_ascii_case("Content-Type") {
                head.has_content_type = false;
            }
        }
        _ => {
            head.headers.clear();
            head.has_content_type = false;
        }
    }
    Ok(Value::Null)
}

/// php's `php_session_remove_cookie`: drop every `Set-Cookie: <name>=`
/// line already in the list, so the session cookie is sent once.
pub(crate) fn remove_cookie_lines(ctx: &mut Ctx, name: &str) {
    let prefix = format!("Set-Cookie: {name}=");
    ctx.head.lock().unwrap().headers.retain(|(_, line)| !line.starts_with(&prefix));
}

/// `headers_list(): array` — the field lines, in the order they were set.
fn headers_list(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (_, line) in &ctx.head.lock().unwrap().headers {
        out.push(Value::string(line.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `headers_sent(&$filename = null, &$line = null): bool` — and where the
/// output that sent them started.
fn headers_sent(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let sent = head_sent(ctx);
    let (file, line) = ctx
        .output_started
        .clone()
        .unwrap_or_else(|| (String::new(), 0));
    if let Some(slot) = args.first_mut() {
        Value::assign(slot, Value::string(file.as_bytes()));
    }
    if let Some(slot) = args.get_mut(1) {
        Value::assign(slot, Value::Int(i64::from(line)));
    }
    Ok(Value::Bool(sent))
}

/// `http_response_code(int $response_code = 0): int|bool` — the previous code,
/// or `false` when nothing has set one (which in the CLI is the normal
/// state; a web SAPI starts at 200).
fn http_response_code(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let previous = ctx.head.lock().unwrap().code;
    let previous_value = || {
        if previous == 0 {
            Value::Bool(false)
        } else {
            Value::Int(previous)
        }
    };
    let Some(code) = args.first().map(|v| v.deref().to_int()) else {
        return Ok(previous_value());
    };
    if head_sent(ctx) {
        let msg = format!(
            "http_response_code(): Cannot set response code - headers already sent{}",
            already_sent(ctx)
        );
        ctx.warn(&msg)?;
        return Ok(Value::Bool(false));
    }
    ctx.head.lock().unwrap().code = code;
    Ok(if previous == 0 {
        Value::Bool(true)
    } else {
        Value::Int(previous)
    })
}

// ---- cookies -------------------------------------------------------------------

/// The characters php refuses in a cookie name, value (raw), path and
/// domain — and how it names them.
const COOKIE_BAD: &[u8] = b",; \t\r\n\x0b\x0c";
const COOKIE_BAD_TEXT: &str = r#"",", ";", " ", "\t", "\r", "\n", "\013", or "\014""#;

/// The `$options` array / positional arguments of `setcookie()`.
#[derive(Default)]
struct CookieOptions {
    expires: i64,
    path: String,
    domain: String,
    secure: bool,
    httponly: bool,
    samesite: String,
    partitioned: bool,
}

/// Read the cookie arguments: `(name, value, expires_or_options, path,
/// domain, secure, httponly)`, with php's checks and error texts.
fn cookie_args(who: &str, args: &[Value]) -> Result<(Vec<u8>, Vec<u8>, CookieOptions), Unwind> {
    let name = args[0].to_php_bytes();
    if name.is_empty() {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($name) must not be empty"
        )));
    }
    if name.iter().any(|b| *b == b'=' || COOKIE_BAD.contains(b)) {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($name) cannot contain \"=\", {COOKIE_BAD_TEXT}"
        )));
    }
    let value = args.get(1).map(|v| v.to_php_bytes()).unwrap_or_default();
    let mut o = CookieOptions::default();
    match args.get(2).map(|v| v.deref().into_owned()) {
        Some(Value::Array(opts)) => {
            if args.len() > 3 {
                return Err(Unwind::error(format!(
                    "{who}(): Argument #3 ($expires_or_options) cannot be an array when the other arguments are provided"
                )));
            }
            for (k, v) in opts.iter() {
                let ArrayKey::Str(k) = k else {
                    return Err(Unwind::value_error(format!("{who}(): option array cannot have numeric keys")));
                };
                let text = || String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
                match k.as_ref() {
                    b"expires" => o.expires = v.deref().to_int(),
                    b"path" => o.path = text(),
                    b"domain" => o.domain = text(),
                    b"secure" => o.secure = v.deref().to_bool(),
                    b"httponly" => o.httponly = v.deref().to_bool(),
                    b"samesite" => o.samesite = text(),
                    b"partitioned" => o.partitioned = v.deref().to_bool(),
                    other => {
                        return Err(Unwind::value_error(format!(
                            "{who}(): option \"{}\" is invalid",
                            String::from_utf8_lossy(other)
                        )))
                    }
                }
            }
        }
        Some(v) => {
            o.expires = v.to_int();
            o.path = args.get(3).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned()).unwrap_or_default();
            o.domain = args.get(4).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned()).unwrap_or_default();
            o.secure = args.get(5).is_some_and(|v| v.deref().to_bool());
            o.httponly = args.get(6).is_some_and(|v| v.deref().to_bool());
        }
        None => {}
    }
    if o.path.bytes().any(|b| COOKIE_BAD.contains(&b)) {
        return Err(Unwind::value_error(format!(
            "{who}(): \"path\" option cannot contain {COOKIE_BAD_TEXT}"
        )));
    }
    if o.domain.bytes().any(|b| COOKIE_BAD.contains(&b)) {
        return Err(Unwind::value_error(format!(
            "{who}(): \"domain\" option cannot contain {COOKIE_BAD_TEXT}"
        )));
    }
    Ok((name, value, o))
}

/// php's `php_setcookie`: the `Set-Cookie` line.
fn cookie_line(who: &str, name: &[u8], value: &[u8], raw: bool, o: &CookieOptions, now: i64) -> Result<String, Unwind> {
    let mut line = b"Set-Cookie: ".to_vec();
    line.extend_from_slice(name);
    line.push(b'=');
    if value.is_empty() {
        // Deleting a cookie: a marker value and a date at the dawn of time.
        line.extend_from_slice(b"deleted; expires=Thu, 01 Jan 1970 00:00:01 GMT; Max-Age=0");
    } else {
        if raw {
            if value.iter().any(|b| COOKIE_BAD.contains(b)) {
                return Err(Unwind::value_error(format!(
                    "{who}(): Argument #2 ($value) cannot contain {COOKIE_BAD_TEXT}"
                )));
            }
            line.extend_from_slice(value);
        } else {
            line.extend_from_slice(&crate::url::encode(value, true));
        }
        if o.expires > 0 {
            let (y, _, _) = crate::date::civil::civil_from_days(o.expires.div_euclid(86_400));
            if y > 9999 {
                return Err(Unwind::value_error(format!(
                    "{who}(): \"expires\" option cannot have a year greater than 9999"
                )));
            }
            line.extend_from_slice(b"; expires=");
            line.extend_from_slice(http_date(o.expires).as_bytes());
            line.extend_from_slice(format!("; Max-Age={}", (o.expires - now).max(0)).as_bytes());
        }
    }
    if !o.path.is_empty() {
        line.extend_from_slice(b"; path=");
        line.extend_from_slice(o.path.as_bytes());
    }
    if !o.domain.is_empty() {
        line.extend_from_slice(b"; domain=");
        line.extend_from_slice(o.domain.as_bytes());
    }
    if o.secure {
        line.extend_from_slice(b"; secure");
    }
    if o.httponly {
        line.extend_from_slice(b"; HttpOnly");
    }
    if !o.samesite.is_empty() {
        line.extend_from_slice(b"; SameSite=");
        line.extend_from_slice(o.samesite.as_bytes());
    }
    if o.partitioned {
        line.extend_from_slice(b"; Partitioned");
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// `Thu, 01 Jan 1970 00:00:01 GMT` — the `D, d M Y H:i:s GMT` of a unix
/// timestamp.
pub(crate) fn http_date(ts: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = crate::date::civil::civil_from_days(days);
    let wd = crate::date::civil::weekday(days) as usize;
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[wd],
        d,
        MONTHS[(m - 1) as usize],
        y,
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// The request's start time in seconds (`$_SERVER['REQUEST_TIME']`), or the
/// clock.
fn now(ctx: &Ctx) -> i64 {
    ctx.request_time
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
        }) as i64
}

/// `setcookie()` / `setrawcookie()`.
fn set_cookie(ctx: &mut Ctx, args: &[Value], raw: bool) -> NativeResult {
    let who = if raw { "setrawcookie" } else { "setcookie" };
    let (name, value, opts) = cookie_args(who, args)?;
    let line = cookie_line(who, &name, &value, raw, &opts, now(ctx))?;
    if refuse(ctx)? {
        return Ok(Value::Bool(false));
    }
    // A cookie never replaces an earlier one: every call is another line.
    if keeps_headers(ctx) {
        ctx.head.lock().unwrap().headers.push(("Set-Cookie".to_string(), line));
    }
    Ok(Value::Bool(true))
}

/// `setcookie(string $name, string $value = "", int|array $expires_or_options = 0, string $path = "", string $domain = "", bool $secure = false, bool $httponly = false): bool`
fn setcookie(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_cookie(ctx, args, false)
}

/// `setrawcookie(...)`: the value goes out as given.
fn setrawcookie(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_cookie(ctx, args, true)
}

// ---- request headers (web SAPIs) --------------------------------------------------

/// `getallheaders(): array` — the request's header fields as received, in
/// order and with their original spelling.
fn getallheaders(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (name, value) in &ctx.request_headers {
        out.set(ArrayKey::str(name.as_bytes()), Value::string(value.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `apache_response_headers(): array` — the response fields set so far,
/// as `name => value`.
fn apache_response_headers(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (name, line) in &ctx.head.lock().unwrap().headers {
        let value = line.split_once(':').map_or("", |(_, v)| v.trim());
        out.set(ArrayKey::str(name.as_bytes()), Value::string(value.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// No constants: the `HTTP_*` names belong to other extensions.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_date_matches_php() {
        assert_eq!(http_date(1), "Thu, 01 Jan 1970 00:00:01 GMT");
        assert_eq!(http_date(1_700_000_000), "Tue, 14 Nov 2023 22:13:20 GMT");
        assert_eq!(http_date(1_800_000_000), "Fri, 15 Jan 2027 08:00:00 GMT");
    }

    #[test]
    fn cookie_lines_match_php() {
        let o = |expires: i64, path: &str, domain: &str, secure: bool, httponly: bool, samesite: &str| CookieOptions {
            expires,
            path: path.into(),
            domain: domain.into(),
            secure,
            httponly,
            samesite: samesite.into(),
            partitioned: false,
        };
        let now = 1_789_898_169;
        assert_eq!(
            cookie_line("setcookie", b"a", b"b c,d;e", false, &o(0, "", "", false, false, ""), now).unwrap(),
            "Set-Cookie: a=b%20c%2Cd%3Be"
        );
        assert_eq!(
            cookie_line("setcookie", b"d", b"e", false, &o(1_800_000_000, "/p", "example.com", true, true, ""), now).unwrap(),
            "Set-Cookie: d=e; expires=Fri, 15 Jan 2027 08:00:00 GMT; Max-Age=10101831; path=/p; domain=example.com; secure; HttpOnly"
        );
        assert_eq!(
            cookie_line("setcookie", b"h", b"", false, &o(0, "", "", false, false, ""), now).unwrap(),
            "Set-Cookie: h=deleted; expires=Thu, 01 Jan 1970 00:00:01 GMT; Max-Age=0"
        );
        assert_eq!(
            cookie_line("setcookie", b"n", b"v", false, &o(1_700_000_000, "", "", false, false, ""), now).unwrap(),
            "Set-Cookie: n=v; expires=Tue, 14 Nov 2023 22:13:20 GMT; Max-Age=0"
        );
        assert_eq!(
            cookie_line("setcookie", b"t", "ü+&=".as_bytes(), false, &o(-1, "", "", false, false, ""), now).unwrap(),
            "Set-Cookie: t=%C3%BC%2B%26%3D"
        );
        assert!(cookie_line("setrawcookie", b"j", b"k l", true, &o(0, "", "", false, false, ""), now).is_err());
    }
}
