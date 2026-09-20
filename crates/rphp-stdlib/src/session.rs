//! php-src `ext/session`: the session lifecycle, the `php` serialization
//! handler, the `files` save handler and the four classes a user handler
//! plugs into.
//!
//! # What a session is here
//!
//! One per request, so the whole state is a thread-local [`State`]: the
//! status, the id, whether the id came from the caller, and the save
//! handler in force. `$_SESSION` is **not** part of it — it is the global
//! of that name, exactly as php has it: `session_start()` fills it,
//! `session_write_close()` reads it back out and encodes it, and what is
//! left in it after a close is still the caller's array.
//!
//! # The two encodings
//!
//! `session.serialize_handler` is `php` by default, and that is the one
//! implemented: each entry is `name` `|` `serialize($value)`, concatenated.
//! A name containing `|` cannot be written and php refuses the whole
//! encode; an integer key cannot be written either (`session_encode()`
//! answers `false` after a notice). `php_serialize` and `php_binary` are
//! cataloged, not faked — an ini set to one of them is refused at
//! `session_start()`.
//!
//! # Where the data goes
//!
//! The `files` handler writes `<save_path>/sess_<id>`, with the system
//! temporary directory standing in for an empty `session.save_path`, which
//! is what php's own default does. `SessionHandler` exposes that handler as
//! a class, so `class MyHandler extends SessionHandler` works the way the
//! manual describes: the parent methods are the files handler.
//!
//! # Known divergences (ADR-004)
//!
//! * **No cookie is ever sent.** php emits `Set-Cookie` through the SAPI
//!   when `session.use_cookies` is on; the CLI has nowhere to send one, and
//!   the engine's header list is CLI-only, so the id is never published.
//!   `session_start()` still refuses to run after output, with php's
//!   `Session cannot be started after headers have already been sent`.
//! * The session id is never read from `$_COOKIE`/`$_GET`: with no request
//!   there is nothing to read it from. `session_id($id)` before the start
//!   is the way to resume one, as it is under the CLI in php.
//! * Garbage collection runs only when `session_gc()` asks for it; php also
//!   rolls `session.gc_probability`/`gc_divisor` at start time.
//! * `session.upload_progress.*` is registered (the ini entries are
//!   observable) but nothing acts on it — there are no uploads without a
//!   web SAPI.

use std::cell::RefCell;
use std::path::PathBuf;

use rphp_runtime::{nf, nm, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("session_status", 0, Some(0), session_status),
    nf!("session_start", 0, Some(1), session_start),
    nf!("session_id", 0, Some(1), session_id),
    nf!("session_name", 0, Some(1), session_name),
    nf!("session_save_path", 0, Some(1), session_save_path),
    nf!("session_module_name", 0, Some(1), session_module_name),
    nf!("session_create_id", 0, Some(1), session_create_id),
    nf!("session_regenerate_id", 0, Some(1), session_regenerate_id),
    nf!("session_write_close", 0, Some(0), session_write_close),
    nf!("session_commit", 0, Some(0), session_write_close),
    nf!("session_abort", 0, Some(0), session_abort),
    nf!("session_reset", 0, Some(0), session_reset),
    nf!("session_unset", 0, Some(0), session_unset),
    nf!("session_destroy", 0, Some(0), session_destroy),
    nf!("session_encode", 0, Some(0), session_encode),
    nf!("session_decode", 1, Some(1), session_decode),
    nf!("session_gc", 0, Some(0), session_gc),
    nf!("session_get_cookie_params", 0, Some(0), session_get_cookie_params),
    nf!("session_set_cookie_params", 1, Some(5), session_set_cookie_params),
    nf!("session_cache_limiter", 0, Some(1), session_cache_limiter),
    nf!("session_cache_expire", 0, Some(1), session_cache_expire),
    nf!("session_set_save_handler", 1, Some(7), session_set_save_handler),
    nf!("session_register_shutdown", 0, Some(0), session_register_shutdown),
];

/// `PHP_SESSION_DISABLED`.
const DISABLED: i64 = 0;
/// `PHP_SESSION_NONE`: the extension is there and no session is running.
const NONE: i64 = 1;
/// `PHP_SESSION_ACTIVE`.
const ACTIVE: i64 = 2;

/// The three status constants and php's `session.*` ini entries with their
/// defaults.
pub(crate) fn register_constants(r: &mut Registry) {
    r.constant("PHP_SESSION_DISABLED", Value::Int(DISABLED));
    r.constant("PHP_SESSION_NONE", Value::Int(NONE));
    r.constant("PHP_SESSION_ACTIVE", Value::Int(ACTIVE));
    let ini = &mut r.interp().ini;
    for (name, default) in [
        ("session.auto_start", "0"),
        ("session.cache_expire", "180"),
        ("session.cache_limiter", "nocache"),
        ("session.cookie_domain", ""),
        ("session.cookie_httponly", ""),
        ("session.cookie_lifetime", "0"),
        ("session.cookie_partitioned", "0"),
        ("session.cookie_path", "/"),
        ("session.cookie_samesite", ""),
        ("session.cookie_secure", "0"),
        ("session.gc_divisor", "1000"),
        ("session.gc_maxlifetime", "1440"),
        ("session.gc_probability", "1"),
        ("session.lazy_write", "1"),
        ("session.name", "PHPSESSID"),
        ("session.referer_check", ""),
        ("session.save_handler", "files"),
        ("session.save_path", ""),
        ("session.serialize_handler", "php"),
        ("session.sid_bits_per_character", "4"),
        ("session.sid_length", "32"),
        ("session.upload_progress.cleanup", "1"),
        ("session.upload_progress.enabled", "1"),
        ("session.upload_progress.freq", "1%"),
        ("session.upload_progress.min_freq", "1"),
        ("session.upload_progress.name", "PHP_SESSION_UPLOAD_PROGRESS"),
        ("session.upload_progress.prefix", "upload_progress_"),
        ("session.use_cookies", "1"),
        ("session.use_only_cookies", "1"),
        ("session.use_strict_mode", "0"),
        ("session.use_trans_sid", "0"),
    ] {
        ini.register(name, default);
    }
}

// ---- the request's session ---------------------------------------------------------

/// The save handler in force.
#[derive(Clone, Default)]
enum Handler {
    /// php's `files`.
    #[default]
    Files,
    /// A `SessionHandlerInterface` instance.
    Object(Object),
    /// The six-callable form of `session_set_save_handler()`, in php's
    /// order: open, close, read, write, destroy, gc.
    Callables(Box<[Value; 6]>),
}

/// Everything a request knows about its session.
#[derive(Default)]
struct State {
    status: Status,
    id: Vec<u8>,
    /// Where `session_start()` ran, which php quotes back when a second
    /// call meets the session it opened.
    started_at: Option<(String, u32)>,
    handler: Handler,
    /// Set by `session_set_cookie_params()`, which php keeps apart from the
    /// ini defaults only in that it writes them all at once.
    cookie: Option<Array>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Status {
    #[default]
    None,
    Active,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Forget the running session (tests, and a fresh request).
#[cfg(test)]
pub(crate) fn reset_state() {
    with_state(|s| *s = State::default());
}

fn active() -> bool {
    with_state(|s| s.status == Status::Active)
}

// ---- ini and arguments -------------------------------------------------------------

fn ini(ctx: &Ctx, key: &str) -> String {
    ctx.ini_get(key).unwrap_or("").to_string()
}

/// A `?string` parameter: absent or `null` means "read, do not write".
fn opt_str(args: &[Value], i: usize, func: &str, name: &str) -> Result<Option<Vec<u8>>, Unwind> {
    match args.get(i) {
        None => Ok(None),
        Some(v) => match &*v.deref() {
            Value::Null => Ok(None),
            Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => {
                Err(Unwind::type_error(format!(
                    "{func}(): Argument #{} (${name}) must be of type ?string, {} given",
                    i + 1,
                    v.type_name()
                )))
            }
            _ => Ok(Some(v.to_php_bytes().to_vec())),
        },
    }
}

/// php's guard on every setter: a running session refuses it.
fn refuse_while_active(ctx: &mut Ctx, who: &str, what: &str) -> Result<bool, Unwind> {
    if active() {
        ctx.warn(&format!("{who}(): Session {what} cannot be changed when a session is active"))?;
        return Ok(true);
    }
    Ok(false)
}

// ---- ids ---------------------------------------------------------------------------

/// The alphabet `session.sid_bits_per_character` selects, php's own.
fn sid_alphabet(bits: i64) -> &'static [u8] {
    match bits {
        5 => b"0123456789abcdefghijklmnopqrstuv",
        6 => b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-,",
        _ => b"0123456789abcdef",
    }
}

/// A fresh id of `session.sid_length` characters from the CSPRNG.
fn new_id(ctx: &mut Ctx) -> Result<Vec<u8>, Unwind> {
    let len = ctx.ini.int("session.sid_length").clamp(22, 256) as usize;
    let bits = ctx.ini.int("session.sid_bits_per_character");
    let alphabet = sid_alphabet(bits);
    let mut args = [Value::Int(len as i64)];
    let bytes = crate::random::random_bytes(ctx, &mut args)?.to_php_bytes().to_vec();
    Ok(bytes
        .iter()
        .map(|b| alphabet[usize::from(*b) % alphabet.len()])
        .collect())
}

/// Whether every character of `id` is one php would have written, which is
/// what `session.use_strict_mode` and `session_id()` both check.
fn valid_id(id: &[u8]) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id.iter().all(|b| {
            b.is_ascii_alphanumeric() || *b == b'-' || *b == b','
        })
}

// ---- the `php` serialization handler -----------------------------------------------

/// Encode `$_SESSION` the way `session.serialize_handler = php` does:
/// `name|serialize(value)` for each entry. `None` after php's notice for a
/// key it cannot write.
fn encode(ctx: &mut Ctx, data: &Array, who: &str) -> Result<Option<Vec<u8>>, Unwind> {
    let mut out = Vec::new();
    for (key, value) in data.iter() {
        let name = match &key {
            ArrayKey::Str(s) => s.to_vec(),
            ArrayKey::Int(n) => {
                ctx.notice(&format!("{who}(): Skipping numeric key {n}"))?;
                continue;
            }
        };
        if name.contains(&b'|') {
            ctx.warn(&format!(
                "{who}(): Failed to encode session object. Session has been destroyed"
            ))?;
            return Ok(None);
        }
        out.extend_from_slice(&name);
        out.push(b'|');
        let mut args = [value.clone()];
        out.extend_from_slice(&crate::var::serialize(ctx, &mut args)?.to_php_bytes());
    }
    Ok(Some(out))
}

/// Read back what [`encode`] wrote, into `$_SESSION`. `false` is php's
/// answer for data it cannot read.
fn decode(ctx: &mut Ctx, data: &[u8]) -> Result<Option<Array>, Unwind> {
    let mut out = Array::new();
    let mut rest = data;
    while !rest.is_empty() {
        let Some(bar) = rest.iter().position(|b| *b == b'|') else {
            return Ok(None);
        };
        let name = rest[..bar].to_vec();
        let payload = &rest[bar + 1..];
        // `unserialize()` reads one value and stops; the length it consumed
        // is what the next entry starts after, so the value is measured by
        // re-serializing is not an option — php's reader tracks the cursor.
        let Some(used) = serialized_len(payload) else {
            return Ok(None);
        };
        let mut args = [Value::string(&payload[..used])];
        let value = crate::var::unserialize(ctx, &mut args)?;
        out.set(ArrayKey::str(&name), value);
        rest = &payload[used..];
    }
    Ok(Some(out))
}

/// The length of the one serialized value at the head of `data`, or `None`
/// when it is not a complete value. This cursor is what php's session
/// reader keeps as it walks `name|value` pairs; `unserialize()` itself only
/// answers the value.
fn serialized_len(data: &[u8]) -> Option<usize> {
    scan_value(data, 0)
}

/// The index just past the `;` of a `b:…;` / `i:…;` / `d:…;` / `r:…;`.
fn scan_scalar(data: &[u8], at: usize) -> Option<usize> {
    let end = data.get(at..)?.iter().position(|b| *b == b';')?;
    Some(at + end + 1)
}

/// The `<digits>:` at `at`, as a number and the index of its colon.
fn length_prefix(data: &[u8], at: usize) -> Option<(usize, usize)> {
    let rest = data.get(at..)?;
    let end = rest.iter().position(|b| *b == b':')?;
    let n: usize = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    Some((n, at + end))
}

/// A `:"<len bytes>"` starting at the colon `at`, answering the index just
/// past the closing quote.
fn scan_quoted(data: &[u8], at: usize, len: usize) -> Option<usize> {
    if data.get(at) != Some(&b':') || data.get(at + 1) != Some(&b'"') {
        return None;
    }
    let close = at + 2 + len;
    if data.get(close) != Some(&b'"') {
        return None;
    }
    Some(close + 1)
}

/// The index just past the one serialized value at `at`, or `None`.
fn scan_value(data: &[u8], at: usize) -> Option<usize> {
    match *data.get(at)? {
        // `N;`
        b'N' => (data.get(at + 1) == Some(&b';')).then_some(at + 2),
        b'b' | b'i' | b'd' | b'r' | b'R' => scan_scalar(data, at),
        // `s:<len>:"<bytes>";` and the enum form `E:<len>:"Enum:Case";`
        b's' | b'E' => {
            let (len, colon) = length_prefix(data, at + 2)?;
            let after = scan_quoted(data, colon, len)?;
            (data.get(after) == Some(&b';')).then_some(after + 1)
        }
        // `a:<count>:{<key><value>…}`
        b'a' => {
            let (count, colon) = length_prefix(data, at + 2)?;
            scan_members(data, colon + 1, count)
        }
        // `O:<len>:"<class>":<count>:{…}` — and `o:`, the same shape.
        b'O' | b'o' => {
            let (len, colon) = length_prefix(data, at + 2)?;
            let after_name = scan_quoted(data, colon, len)?;
            let (count, colon2) = length_prefix(data, after_name + 1)?;
            scan_members(data, colon2 + 1, count)
        }
        // `C:<len>:"<class>":<datalen>:{<opaque>}`
        b'C' => {
            let (len, colon) = length_prefix(data, at + 2)?;
            let after_name = scan_quoted(data, colon, len)?;
            let (datalen, colon2) = length_prefix(data, after_name + 1)?;
            let close = colon2 + 2 + datalen;
            if data.get(colon2 + 1) != Some(&b'{') || data.get(close) != Some(&b'}') {
                return None;
            }
            Some(close + 1)
        }
        _ => None,
    }
}

/// `{` then `n` key/value pairs then `}` — an object's properties are pairs
/// too, which is why one function serves both.
fn scan_members(data: &[u8], start: usize, n: usize) -> Option<usize> {
    if data.get(start) != Some(&b'{') {
        return None;
    }
    let mut cur = start + 1;
    for _ in 0..n {
        cur = scan_value(data, cur)?;
        cur = scan_value(data, cur)?;
    }
    (data.get(cur) == Some(&b'}')).then_some(cur + 1)
}

// ---- the save handlers -------------------------------------------------------------

/// The directory the `files` handler writes to: `session.save_path`, or the
/// system temporary directory when it is empty, which is php's default.
fn save_dir(ctx: &Ctx) -> PathBuf {
    let path = ini(ctx, "session.save_path");
    if path.is_empty() {
        std::env::temp_dir()
    } else {
        // php's `N;/path` form selects a directory depth; only the path
        // after the last `;` is the directory.
        PathBuf::from(path.rsplit(';').next().unwrap_or("").to_string())
    }
}

fn session_file(ctx: &Ctx, id: &[u8]) -> PathBuf {
    save_dir(ctx).join(format!("sess_{}", String::from_utf8_lossy(id)))
}

/// Call one method of the user handler, whichever form it was registered in.
fn call_handler(ctx: &mut Ctx, method: &str, args: &[Value]) -> Result<Option<Value>, Unwind> {
    let handler = with_state(|s| s.handler.clone());
    match handler {
        Handler::Files => Ok(None),
        Handler::Object(o) => Ok(Some(ctx.call_method(&o, method.as_bytes(), args)?)),
        Handler::Callables(cbs) => {
            let slot = match method {
                "open" => 0,
                "close" => 1,
                "read" => 2,
                "write" => 3,
                "destroy" => 4,
                "gc" => 5,
                _ => return Ok(None),
            };
            Ok(Some(ctx.call_value(&cbs[slot], args)?))
        }
    }
}

/// Read the stored data for `id`.
fn handler_read(ctx: &mut Ctx, id: &[u8]) -> Result<Vec<u8>, Unwind> {
    if let Some(v) = call_handler(ctx, "read", &[Value::string(id)])? {
        return Ok(v.to_php_bytes().to_vec());
    }
    Ok(std::fs::read(session_file(ctx, id)).unwrap_or_default())
}

/// Store `data` under `id`.
fn handler_write(ctx: &mut Ctx, id: &[u8], data: &[u8]) -> Result<(), Unwind> {
    if call_handler(ctx, "write", &[Value::string(id), Value::string(data)])?.is_some() {
        return Ok(());
    }
    let path = session_file(ctx, id);
    let _ = std::fs::write(&path, data);
    crate::filestat::invalidate(ctx, &path);
    Ok(())
}

/// Drop the stored data for `id`.
fn handler_destroy(ctx: &mut Ctx, id: &[u8]) -> Result<(), Unwind> {
    if call_handler(ctx, "destroy", &[Value::string(id)])?.is_some() {
        return Ok(());
    }
    let path = session_file(ctx, id);
    let _ = std::fs::remove_file(&path);
    crate::filestat::invalidate(ctx, &path);
    Ok(())
}

/// The `files` handler's collection: every `sess_*` older than `maxlifetime`.
fn files_gc(ctx: &mut Ctx, maxlifetime: i64) -> i64 {
    let dir = save_dir(ctx);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    let mut stale_paths = Vec::new();
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with("sess_") {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d.as_secs() as i64 > maxlifetime);
        if stale && std::fs::remove_file(e.path()).is_ok() {
            stale_paths.push(e.path());
            removed += 1;
        }
    }
    for path in stale_paths {
        crate::filestat::invalidate(ctx, &path);
    }
    removed
}

// ---- `$_SESSION` -------------------------------------------------------------------

/// The `$_SESSION` global's array, empty when it holds anything else.
fn session_array(ctx: &mut Ctx) -> Array {
    match ctx.globals.get(b"_SESSION").map(|c| c.get()) {
        Some(Value::Array(a)) => a,
        _ => Array::new(),
    }
}

fn set_session_array(ctx: &mut Ctx, a: Array) {
    ctx.globals.get_or_create(b"_SESSION").set(Value::Array(a));
}

// ---- the lifecycle -----------------------------------------------------------------

/// `session_status(): int`
fn session_status(_: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(if active() { ACTIVE } else { NONE }))
}

/// `session_start(array $options = []): bool`
fn session_start(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_start";
    if active() {
        let (file, line) = with_state(|s| s.started_at.clone())
            .unwrap_or_else(|| ("unknown".to_string(), 0));
        ctx.notice(&format!(
            "{who}(): Ignoring session_start() because a session is already active (started from {file} on line {line})"
        ))?;
        return Ok(Value::Bool(true));
    }
    // Options are ini entries for the length of the request.
    if let Some(v) = args.first() {
        if let Value::Array(opts) = &*v.deref() {
            for (k, value) in opts.iter() {
                let ArrayKey::Str(name) = &k else { continue };
                let name = String::from_utf8_lossy(name).into_owned();
                let key = if name.starts_with("session.") {
                    name
                } else {
                    format!("session.{name}")
                };
                ctx.ini_set(&key, &String::from_utf8_lossy(&value.to_php_bytes()));
            }
        }
    }
    // The engine records where the first bytes that reached the SAPI came
    // from only once something asks, the way `header()` does.
    ctx.note_output();
    if ctx.out.sent() {
        let (file, line) = ctx
            .output_started
            .clone()
            .unwrap_or_else(|| ("unknown".to_string(), 0));
        ctx.warn(&format!(
            "{who}(): Session cannot be started after headers have already been sent (sent from {file} on line {line})"
        ))?;
        return Ok(Value::Bool(false));
    }
    let handler = ini(ctx, "session.serialize_handler");
    if handler != "php" {
        ctx.warn(&format!(
            "{who}(): Cannot find serialization handler '{handler}' - session startup failed"
        ))?;
        return Ok(Value::Bool(false));
    }
    // The id: set ahead by `session_id()`, else the request's cookie, else
    // a fresh one. php publishes it (`Set-Cookie`) unless it came from the
    // cookie.
    let id = with_state(|s| s.id.clone());
    let mut from_cookie = false;
    let id = if !id.is_empty() {
        id
    } else if let Some(c) = cookie_id(ctx) {
        from_cookie = true;
        c
    } else {
        new_id(ctx)?
    };
    call_handler(
        ctx,
        "open",
        &[
            Value::string(save_dir(ctx).to_string_lossy().as_bytes()),
            Value::string(&ini(ctx, "session.name").into_bytes()),
        ],
    )?;
    let data = handler_read(ctx, &id)?;
    let decoded = if data.is_empty() {
        Some(Array::new())
    } else {
        decode(ctx, &data)?
    };
    let Some(decoded) = decoded else {
        ctx.warn(&format!("{who}(): Failed to decode session object. Session has been destroyed"))?;
        set_session_array(ctx, Array::new());
        let site = (ctx.current_file(), ctx.current_line());
        with_state(|s| {
            s.id = id;
            s.status = Status::Active;
            s.started_at = Some(site);
        });
        return Ok(Value::Bool(true));
    };
    set_session_array(ctx, decoded);
    let site = (ctx.current_file(), ctx.current_line());
    with_state(|s| {
        s.id = id.clone();
        s.status = Status::Active;
        s.started_at = Some(site);
    });
    if !from_cookie {
        send_cookie(ctx, &id)?;
    }
    send_cache_limiter(ctx);
    Ok(Value::Bool(true))
}

/// The id the request's cookie carries (`session.use_cookies`), if any.
fn cookie_id(ctx: &mut Ctx) -> Option<Vec<u8>> {
    if !ctx.ini.bool("session.use_cookies") {
        return None;
    }
    let name = ini(ctx, "session.name");
    let cookies = ctx.globals.get(b"_COOKIE")?;
    let v = cookies.borrow().clone();
    let Value::Array(a) = v else { return None };
    let id = a.get(&ArrayKey::str(name.as_bytes()))?.to_php_bytes();
    if id.is_empty() || !id.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b',') {
        return None;
    }
    Some(id)
}

/// `Set-Cookie: <session.name>=<id>; …` from the `session.cookie_*`
/// settings (php's `php_session_send_cookie`).
fn send_cookie(ctx: &mut Ctx, id: &[u8]) -> Result<(), Unwind> {
    if !ctx.ini.bool("session.use_cookies") {
        return Ok(());
    }
    let name = ini(ctx, "session.name");
    let lifetime = ctx.ini.int("session.cookie_lifetime");
    let mut opts = Array::new();
    if lifetime > 0 {
        let now = ctx.request_time.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
        }) as i64;
        opts.set(ArrayKey::str(b"expires"), Value::Int(now + lifetime));
    }
    for (key, ini_name) in [
        ("path", "session.cookie_path"),
        ("domain", "session.cookie_domain"),
        ("samesite", "session.cookie_samesite"),
    ] {
        let v = ini(ctx, ini_name);
        if !v.is_empty() {
            opts.set(ArrayKey::str(key.as_bytes()), Value::string(v.as_bytes()));
        }
    }
    for (key, ini_name) in [
        ("secure", "session.cookie_secure"),
        ("httponly", "session.cookie_httponly"),
        ("partitioned", "session.cookie_partitioned"),
    ] {
        if ctx.ini.bool(ini_name) {
            opts.set(ArrayKey::str(key.as_bytes()), Value::Bool(true));
        }
    }
    ctx.call_function(
        b"setcookie",
        &[Value::string(name.as_bytes()), Value::string(id), Value::Array(opts)],
    )?;
    Ok(())
}

/// The `session.cache_limiter` headers (`php_session_cache_limiter`).
fn send_cache_limiter(ctx: &mut Ctx) {
    let limiter = ini(ctx, "session.cache_limiter");
    let expire = ctx.ini.int("session.cache_expire") * 60;
    let now = ctx.request_time.unwrap_or(0.0) as i64;
    let lines: Vec<String> = match limiter.as_str() {
        "nocache" => vec![
            "Expires: Thu, 19 Nov 1981 08:52:00 GMT".to_string(),
            "Cache-Control: no-store, no-cache, must-revalidate".to_string(),
            "Pragma: no-cache".to_string(),
        ],
        "public" => vec![
            format!("Expires: {}", crate::head::http_date(now + expire)),
            format!("Cache-Control: public, max-age={expire}"),
            format!("Last-Modified: {}", crate::head::http_date(now)),
        ],
        "private" => vec![
            "Expires: Thu, 19 Nov 1981 08:52:00 GMT".to_string(),
            format!("Cache-Control: private, max-age={expire}"),
            format!("Last-Modified: {}", crate::head::http_date(now)),
        ],
        "private_no_expire" => vec![
            format!("Cache-Control: private, max-age={expire}"),
            format!("Last-Modified: {}", crate::head::http_date(now)),
        ],
        _ => Vec::new(),
    };
    for line in lines {
        crate::head::add_header(ctx, &line, true);
    }
}

/// `session_write_close(): bool` (and its `session_commit` alias)
fn session_write_close(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    // Nothing to close is `false`, quietly — the one status answer that is
    // not a warning.
    if !active() {
        return Ok(Value::Bool(false));
    }
    let data = session_array(ctx);
    let id = with_state(|s| s.id.clone());
    if let Some(bytes) = encode(ctx, &data, "session_write_close")? {
        handler_write(ctx, &id, &bytes)?;
    }
    call_handler(ctx, "close", &[])?;
    with_state(|s| s.status = Status::None);
    Ok(Value::Bool(true))
}

/// `session_abort(): bool` — close without writing.
fn session_abort(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if !active() {
        return Ok(Value::Bool(false));
    }
    call_handler(ctx, "close", &[])?;
    with_state(|s| s.status = Status::None);
    Ok(Value::Bool(true))
}

/// `session_reset(): bool` — re-read the stored data, dropping the changes.
fn session_reset(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if !active() {
        return Ok(Value::Bool(false));
    }
    let id = with_state(|s| s.id.clone());
    let data = handler_read(ctx, &id)?;
    let decoded = if data.is_empty() {
        Some(Array::new())
    } else {
        decode(ctx, &data)?
    };
    set_session_array(ctx, decoded.unwrap_or_default());
    Ok(Value::Bool(true))
}

/// `session_unset(): bool` — empty `$_SESSION`, keeping the session open.
fn session_unset(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if !active() {
        return Ok(Value::Bool(false));
    }
    set_session_array(ctx, Array::new());
    Ok(Value::Bool(true))
}

/// `session_destroy(): bool`
fn session_destroy(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if !active() {
        ctx.warn("session_destroy(): Trying to destroy uninitialized session")?;
        return Ok(Value::Bool(false));
    }
    let id = with_state(|s| s.id.clone());
    handler_destroy(ctx, &id)?;
    call_handler(ctx, "close", &[])?;
    with_state(|s| s.status = Status::None);
    Ok(Value::Bool(true))
}

/// `session_regenerate_id(bool $delete_old_session = false): bool`
fn session_regenerate_id(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_regenerate_id";
    if !active() {
        ctx.warn(&format!("{who}(): Session ID cannot be regenerated when there is no active session"))?;
        return Ok(Value::Bool(false));
    }
    let delete_old = args.first().is_some_and(|v| v.deref().to_bool());
    let old = with_state(|s| s.id.clone());
    // php writes the current data under the *old* id first, so a handler
    // that keeps a transaction sees it closed.
    let data = session_array(ctx);
    if let Some(bytes) = encode(ctx, &data, who)? {
        handler_write(ctx, &old, &bytes)?;
    }
    if delete_old {
        handler_destroy(ctx, &old)?;
    }
    let id = new_id(ctx)?;
    with_state(|s| s.id = id.clone());
    send_cookie(ctx, &id)?;
    let data = session_array(ctx);
    if let Some(bytes) = encode(ctx, &data, who)? {
        handler_write(ctx, &id, &bytes)?;
    }
    Ok(Value::Bool(true))
}

/// `session_gc(): int|false`
fn session_gc(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if !active() {
        ctx.warn("session_gc(): Session cannot be garbage collected when there is no active session")?;
        return Ok(Value::Bool(false));
    }
    let maxlifetime = ctx.ini.int("session.gc_maxlifetime");
    if let Some(v) = call_handler(ctx, "gc", &[Value::Int(maxlifetime)])? {
        return Ok(Value::Int(v.to_int()));
    }
    Ok(Value::Int(files_gc(ctx, maxlifetime)))
}

// ---- the settings ------------------------------------------------------------------

/// `session_id(?string $id = null): string|false`
fn session_id(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_id";
    let new = opt_str(args, 0, who, "id")?;
    let current = with_state(|s| s.id.clone());
    if let Some(id) = new {
        if active() {
            ctx.warn(&format!("{who}(): Session ID cannot be changed when a session is active"))?;
            return Ok(Value::Bool(false));
        }
        with_state(|s| s.id = id);
    }
    Ok(Value::string(&current))
}

/// `session_name(?string $name = null): string|false`
fn session_name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_name";
    let new = opt_str(args, 0, who, "name")?;
    let current = ini(ctx, "session.name");
    if let Some(name) = new {
        if refuse_while_active(ctx, who, "name")? {
            return Ok(Value::Bool(false));
        }
        if name.is_empty() {
            ctx.warn(&format!("{who}(): Argument #1 ($name) cannot be empty"))?;
            return Ok(Value::Bool(false));
        }
        ctx.ini_set("session.name", &String::from_utf8_lossy(&name));
    }
    Ok(Value::string(current.as_bytes()))
}

/// `session_save_path(?string $path = null): string|false`
fn session_save_path(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_save_path";
    let new = opt_str(args, 0, who, "path")?;
    let current = ini(ctx, "session.save_path");
    if let Some(path) = new {
        if refuse_while_active(ctx, who, "save path")? {
            return Ok(Value::Bool(false));
        }
        ctx.ini_set("session.save_path", &String::from_utf8_lossy(&path));
    }
    Ok(Value::string(current.as_bytes()))
}

/// `session_module_name(?string $module = null): string|false`
fn session_module_name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_module_name";
    let new = opt_str(args, 0, who, "module")?;
    let current = ini(ctx, "session.save_handler");
    if let Some(module) = new {
        if refuse_while_active(ctx, who, "save handler module")? {
            return Ok(Value::Bool(false));
        }
        if module != b"files" && module != b"user" {
            ctx.warn(&format!(
                "{who}(): Session handler module \"{}\" cannot be found",
                String::from_utf8_lossy(&module)
            ))?;
            return Ok(Value::Bool(false));
        }
        ctx.ini_set("session.save_handler", &String::from_utf8_lossy(&module));
    }
    Ok(Value::string(current.as_bytes()))
}

/// `session_cache_limiter(?string $value = null): string|false`
fn session_cache_limiter(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_cache_limiter";
    let new = opt_str(args, 0, who, "value")?;
    let current = ini(ctx, "session.cache_limiter");
    if let Some(v) = new {
        if refuse_while_active(ctx, who, "cache limiter")? {
            return Ok(Value::Bool(false));
        }
        ctx.ini_set("session.cache_limiter", &String::from_utf8_lossy(&v));
    }
    Ok(Value::string(current.as_bytes()))
}

/// `session_cache_expire(?int $value = null): int|false`
fn session_cache_expire(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_cache_expire";
    let current = ctx.ini.int("session.cache_expire");
    let new = match args.first() {
        None => None,
        Some(v) => match &*v.deref() {
            Value::Null => None,
            other => Some(other.to_int()),
        },
    };
    if let Some(v) = new {
        if refuse_while_active(ctx, who, "cache expire")? {
            return Ok(Value::Bool(false));
        }
        ctx.ini_set("session.cache_expire", &v.to_string());
    }
    Ok(Value::Int(current))
}

/// `session_create_id(string $prefix = ""): string|false`
fn session_create_id(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let prefix = match args.first() {
        None => Vec::new(),
        Some(v) => v.to_php_bytes().to_vec(),
    };
    if !prefix.is_empty() && !valid_id(&prefix) {
        ctx.warn("session_create_id(): Prefix cannot contain special characters. Only the A-Z, a-z, 0-9, \"-\", and \",\" characters are allowed")?;
        return Ok(Value::Bool(false));
    }
    let mut id = prefix;
    id.extend_from_slice(&new_id(ctx)?);
    Ok(Value::string(&id))
}

/// `session_get_cookie_params(): array`
fn session_get_cookie_params(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if let Some(a) = with_state(|s| s.cookie.clone()) {
        return Ok(Value::Array(a));
    }
    let mut a = Array::new();
    a.set(ArrayKey::str(b"lifetime"), Value::Int(ctx.ini.int("session.cookie_lifetime")));
    a.set(
        ArrayKey::str(b"path"),
        Value::string(ini(ctx, "session.cookie_path").as_bytes()),
    );
    a.set(
        ArrayKey::str(b"domain"),
        Value::string(ini(ctx, "session.cookie_domain").as_bytes()),
    );
    a.set(ArrayKey::str(b"secure"), Value::Bool(ctx.ini.bool("session.cookie_secure")));
    a.set(
        ArrayKey::str(b"partitioned"),
        Value::Bool(ctx.ini.bool("session.cookie_partitioned")),
    );
    a.set(
        ArrayKey::str(b"httponly"),
        Value::Bool(ctx.ini.bool("session.cookie_httponly")),
    );
    a.set(
        ArrayKey::str(b"samesite"),
        Value::string(ini(ctx, "session.cookie_samesite").as_bytes()),
    );
    Ok(Value::Array(a))
}

/// `session_set_cookie_params(array|int $lifetime_or_options, ?string $path = null, ?string $domain = null, ?bool $secure = null, ?bool $httponly = null): bool`
fn session_set_cookie_params(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_set_cookie_params";
    if refuse_while_active(ctx, who, "cookie parameters")? {
        return Ok(Value::Bool(false));
    }
    let mut params = match session_get_cookie_params(ctx, &mut [])? {
        Value::Array(a) => a,
        _ => Array::new(),
    };
    let set = |params: &mut Array, key: &[u8], v: Value| params.set(ArrayKey::str(key), v);
    match &*args[0].deref() {
        Value::Array(opts) => {
            for (k, v) in opts.iter() {
                let ArrayKey::Str(name) = &k else { continue };
                match name.as_ref() {
                    b"lifetime" => set(&mut params, b"lifetime", Value::Int(v.to_int())),
                    b"path" | b"domain" | b"samesite" => {
                        set(&mut params, name, Value::string(&v.to_php_bytes()))
                    }
                    b"secure" | b"httponly" | b"partitioned" => {
                        set(&mut params, name, Value::Bool(v.to_bool()))
                    }
                    other => {
                        ctx.warn(&format!(
                            "{who}(): Argument #1 ($lifetime_or_options) contains an unrecognized key \"{}\"",
                            String::from_utf8_lossy(other)
                        ))?;
                    }
                }
            }
        }
        v => {
            set(&mut params, b"lifetime", Value::Int(v.to_int()));
            for (i, key) in [(1usize, b"path".as_slice()), (2, b"domain")] {
                if let Some(a) = args.get(i) {
                    if !matches!(&*a.deref(), Value::Null) {
                        set(&mut params, key, Value::string(&a.to_php_bytes()));
                    }
                }
            }
            for (i, key) in [(3usize, b"secure".as_slice()), (4, b"httponly")] {
                if let Some(a) = args.get(i) {
                    if !matches!(&*a.deref(), Value::Null) {
                        set(&mut params, key, Value::Bool(a.deref().to_bool()));
                    }
                }
            }
        }
    }
    // php writes them into the ini table, where `ini_get()` sees them.
    for (key, ini_key) in [
        (b"lifetime".as_slice(), "session.cookie_lifetime"),
        (b"path", "session.cookie_path"),
        (b"domain", "session.cookie_domain"),
        (b"samesite", "session.cookie_samesite"),
    ] {
        if let Some(v) = params.get_deref(&ArrayKey::str(key)) {
            ctx.ini_set(ini_key, &String::from_utf8_lossy(&v.to_php_bytes()));
        }
    }
    for (key, ini_key) in [
        (b"secure".as_slice(), "session.cookie_secure"),
        (b"httponly", "session.cookie_httponly"),
        (b"partitioned", "session.cookie_partitioned"),
    ] {
        if let Some(v) = params.get_deref(&ArrayKey::str(key)) {
            ctx.ini_set(ini_key, if v.to_bool() { "1" } else { "0" });
        }
    }
    with_state(|s| s.cookie = Some(params));
    Ok(Value::Bool(true))
}

// ---- encode / decode ---------------------------------------------------------------

/// `session_encode(): string|false`
fn session_encode(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    if !active() {
        ctx.warn("session_encode(): Cannot encode non-existent session")?;
        return Ok(Value::Bool(false));
    }
    let data = session_array(ctx);
    Ok(match encode(ctx, &data, "session_encode")? {
        Some(bytes) => Value::string(&bytes),
        None => Value::Bool(false),
    })
}

/// `session_decode(string $data): bool` — merges into `$_SESSION`.
fn session_decode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if !active() {
        ctx.warn("session_decode(): Session data cannot be decoded when there is no active session")?;
        return Ok(Value::Bool(false));
    }
    let data = args[0].to_php_bytes().to_vec();
    let Some(decoded) = decode(ctx, &data)? else {
        // php means it: the running session is gone, its stored data with
        // it, and `$_SESSION` is empty.
        ctx.warn("session_decode(): Failed to decode session object. Session has been destroyed")?;
        let id = with_state(|s| s.id.clone());
        handler_destroy(ctx, &id)?;
        call_handler(ctx, "close", &[])?;
        set_session_array(ctx, Array::new());
        with_state(|s| {
            s.status = Status::None;
            s.id = Vec::new();
        });
        return Ok(Value::Bool(false));
    };
    let mut current = session_array(ctx);
    for (k, v) in decoded.iter() {
        current.set(k.clone(), v.clone());
    }
    set_session_array(ctx, current);
    Ok(Value::Bool(true))
}

// ---- the save handler ---------------------------------------------------------------

/// `session_set_save_handler(SessionHandlerInterface $handler, bool $register_shutdown = true): bool`,
/// or the six-callable form php kept from before there were interfaces.
fn session_set_save_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "session_set_save_handler";
    if active() {
        ctx.warn(&format!("{who}(): Cannot change save handler when a session is active"))?;
        return Ok(Value::Bool(false));
    }
    if let Value::Object(o) = &*args[0].deref() {
        with_state(|s| s.handler = Handler::Object(o.clone()));
        ctx.ini_set("session.save_handler", "user");
        return Ok(Value::Bool(true));
    }
    if args.len() < 6 {
        return Err(Unwind::argument_count_error(format!(
            "{who}() expects exactly 6 arguments, {} given",
            args.len()
        )));
    }
    ctx.deprecated(&format!(
        "{who}(): Providing individual callbacks instead of an object implementing SessionHandlerInterface is deprecated"
    ))?;
    let cbs: [Value; 6] = std::array::from_fn(|i| args[i].deref().into_owned());
    with_state(|s| s.handler = Handler::Callables(Box::new(cbs)));
    ctx.ini_set("session.save_handler", "user");
    Ok(Value::Bool(true))
}

/// `session_register_shutdown(): void` — the shutdown function php
/// registers so an uncaught fatal still writes the session.
fn session_register_shutdown(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut args = [Value::string(b"session_write_close")];
    crate::basic_functions::register_shutdown_function(ctx, &mut args)?;
    Ok(Value::Null)
}

// ---- the classes --------------------------------------------------------------------

/// Register `SessionHandlerInterface`, `SessionIdInterface`,
/// `SessionUpdateTimestampHandlerInterface` and `SessionHandler`.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.interp().class_by_name(b"SessionHandlerInterface").is_some() {
        return;
    }
    r.interface("SessionHandlerInterface")
        .abstract_method("open", nm!(2, Some(2), unreachable_method))
        .abstract_method("close", nm!(0, Some(0), unreachable_method))
        .abstract_method("read", nm!(1, Some(1), unreachable_method))
        .abstract_method("write", nm!(2, Some(2), unreachable_method))
        .abstract_method("destroy", nm!(1, Some(1), unreachable_method))
        .abstract_method("gc", nm!(1, Some(1), unreachable_method))
        .finish();
    r.interface("SessionIdInterface")
        .abstract_method("create_sid", nm!(0, Some(0), unreachable_method))
        .finish();
    r.interface("SessionUpdateTimestampHandlerInterface")
        .abstract_method("validateId", nm!(1, Some(1), unreachable_method))
        .abstract_method("updateTimestamp", nm!(2, Some(2), unreachable_method))
        .finish();
    // php's `files` handler as a class: a subclass may override one method
    // and call `parent::` for the rest, which is the documented way to add
    // encryption or a different directory.
    // php declares it against the first two interfaces only: the strict /
    // timestamp behaviour is a *user* subclass's to add (Symfony's
    // `StrictSessionHandler` refuses to wrap one that already has it).
    r.class("SessionHandler")
        .implements(&["SessionHandlerInterface", "SessionIdInterface"])
        .method("open", nm!(2, Some(2), handler_open))
        .method("close", nm!(0, Some(0), handler_close))
        .method("read", nm!(1, Some(1), handler_read_method))
        .method("write", nm!(2, Some(2), handler_write_method))
        .method("destroy", nm!(1, Some(1), handler_destroy_method))
        .method("gc", nm!(1, Some(1), handler_gc_method))
        .method("create_sid", nm!(0, Some(0), handler_create_sid))
        .finish();
}

/// An interface's method body is never reached: the class that implements
/// it supplies one.
fn unreachable_method(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "Cannot call abstract method SessionHandlerInterface::method()",
    ))
}

/// `SessionHandler::open(string $path, string $name): bool`
fn handler_open(_: &mut Ctx, _: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

/// `SessionHandler::close(): bool`
fn handler_close(_: &mut Ctx, _: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

/// `SessionHandler::read(string $id): string|false`
fn handler_read_method(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let id = args[0].to_php_bytes().to_vec();
    Ok(Value::string(
        &std::fs::read(session_file(ctx, &id)).unwrap_or_default(),
    ))
}

/// `SessionHandler::write(string $id, string $data): bool`
fn handler_write_method(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let id = args[0].to_php_bytes().to_vec();
    let data = args[1].to_php_bytes().to_vec();
    let path = session_file(ctx, &id);
    let ok = std::fs::write(&path, data).is_ok();
    crate::filestat::invalidate(ctx, &path);
    Ok(Value::Bool(ok))
}

/// `SessionHandler::destroy(string $id): bool`
fn handler_destroy_method(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let id = args[0].to_php_bytes().to_vec();
    let path = session_file(ctx, &id);
    let _ = std::fs::remove_file(&path);
    crate::filestat::invalidate(ctx, &path);
    Ok(Value::Bool(true))
}

/// `SessionHandler::gc(int $max_lifetime): int|false`
fn handler_gc_method(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let max = args[0].deref().to_int();
    Ok(Value::Int(files_gc(ctx, max)))
}

/// `SessionHandler::create_sid(): string`
fn handler_create_sid(ctx: &mut Ctx, _: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    Ok(Value::string(&new_id(ctx)?))
}

