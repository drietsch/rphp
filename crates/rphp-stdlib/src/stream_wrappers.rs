//! User stream wrappers (php-src `main/streams/userspace.c`, the
//! `stream_wrapper_*` functions of `ext/standard/streamsfuncs.c`) and
//! `popen()`/`pclose()` (`ext/standard/file.c`).
//!
//! ## The wrapper table
//!
//! php keeps one table of url schemes. The built-in entries are there from
//! the start; `stream_wrapper_register()` adds a scheme served by a class,
//! `stream_wrapper_unregister()` takes any entry out (a built-in one too —
//! with `file` gone, plain paths stop opening) and
//! `stream_wrapper_restore()` puts a built-in back, at the end of the
//! table, which is the order `stream_get_wrappers()` lists. The table lives
//! in a per-request slot that only exists once a script has touched it, so
//! every path function asks it for nothing until then.
//!
//! ## A user stream
//!
//! Opening a url whose scheme a class serves makes an instance of that
//! class (its `$context` set, then its constructor run) and asks
//! `stream_open()`. The stream that comes back is php's buffered stream
//! over the object: reads fill a read buffer with `stream_read()` calls of
//! php's own sizes (8192 at a time, more once the buffer has grown), each
//! followed by `stream_eof()`; writes go out in 8192-byte pieces through
//! `stream_write()`; a seek inside the buffer never reaches the object, one
//! outside it is `stream_seek()` and then `stream_tell()`. The position,
//! the end-of-file flag and "this stream cannot seek" are the stream's, as
//! php keeps them, which is what makes the calls a class sees — and the
//! answers the script gets — the same as php's.
//!
//! The one-shot operations (`url_stat`, `unlink`, `rename`, `mkdir`,
//! `rmdir`, `stream_metadata`) get an instance of their own that is gone
//! (its destructor run) before the function returns; `dir_opendir()` makes
//! a directory stream whose `readdir()` is `dir_readdir()`. A missing
//! method is php's warning, word for word.
//!
//! `include` of a url a class serves reads the unit through the wrapper
//! (`include_hook`, installed as the runtime's
//! [`IncludeStreamHook`](rphp_runtime::IncludeStreamHook)).
//!
//! ## popen
//!
//! `popen()` runs the command through `/bin/sh -c` under the host's
//! spawn policy (`exec.rs`) and answers a pipe stream (`file.rs`) over the
//! child's stdout or stdin; `pclose()` (or `fclose()`) closes it and waits.
//!
//! Streams and children still open when the request ends are closed then
//! ([`request_shutdown`]); everything else here is per-request state in
//! `Interp::ext`.

use std::rc::Rc;

use rphp_runtime::{nf, Ctx, IncludeOpen, Interp, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, PhpRef, Resource, Str, Value};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("stream_wrapper_register", 2, Some(3), stream_wrapper_register),
    nf!("stream_register_wrapper", 2, Some(3), stream_wrapper_register),
    nf!("stream_wrapper_unregister", 1, Some(1), stream_wrapper_unregister),
    nf!("stream_wrapper_restore", 1, Some(1), stream_wrapper_restore),
    nf!("popen", 2, Some(2), popen),
    nf!("pclose", 1, Some(1), pclose),
];

/// No classes; the runtime's include hook is installed here.
pub(crate) fn register_classes(r: &mut Registry) {
    r.interp().include_stream_hook = Some(include_hook);
}

/// The `STREAM_*` constants user wrappers use are `file.rs`'s.
pub(crate) fn register_constants(_r: &mut Registry) {}

// ---- constants -------------------------------------------------------------------------

/// php's stream chunk size.
const CHUNK: usize = 8192;
/// `BUFSIZ` on this platform, which php passes to `stream_set_option()`
/// when a buffer option comes without a size.
const BUFSIZ: i64 = 1024;

const STREAM_USE_PATH: i64 = 1;
const STREAM_REPORT_ERRORS: i64 = 8;
const STREAM_OPEN_FOR_INCLUDE: i64 = 0x80;
const STREAM_OPEN_FOR_ZEND_STREAM: i64 = 0x10000;
const STREAM_URL_STAT_LINK: i64 = 1;
const STREAM_URL_STAT_QUIET: i64 = 2;
/// The bit php's `php_stat()` always adds (`PHP_STREAM_URL_STAT_IGNORE_OPEN_BASEDIR`).
const STREAM_URL_STAT_IGNORE_OPEN_BASEDIR: i64 = 4;
const STREAM_MKDIR_RECURSIVE: i64 = 1;

const OPTION_BLOCKING: i64 = 1;
const OPTION_READ_BUFFER: i64 = 2;
const OPTION_WRITE_BUFFER: i64 = 3;
const OPTION_READ_TIMEOUT: i64 = 4;
const BUFFER_NONE: i64 = 0;
const BUFFER_FULL: i64 = 2;
const CAST_FOR_SELECT: i64 = 3;

const META_TOUCH: i64 = 1;
const META_OWNER_NAME: i64 = 2;
const META_OWNER: i64 = 3;
const META_GROUP_NAME: i64 = 4;
const META_GROUP: i64 = 5;
const META_ACCESS: i64 = 6;

const S_IFMT: i64 = 0o170000;
const S_IFREG: i64 = 0o100000;
const S_IFDIR: i64 = 0o040000;
const S_IFLNK: i64 = 0o120000;

/// php's built-in wrappers that rphp does not list in
/// `stream_get_wrappers()` (it cannot open them), but that are taken as far
/// as registering goes: php refuses `stream_wrapper_register('phar', …)`.
const PHP_ONLY_BUILTINS: &[&str] = &["ftps", "compress.zlib", "compress.bzip2", "ftp", "phar", "zip"];

// ---- the wrapper table -----------------------------------------------------------------

/// A scheme a class serves.
pub(crate) struct UserWrapper {
    class: u32,
    /// The class's name as declared, which php's messages use.
    class_name: String,
    /// `STREAM_IS_URL`: `include` refuses it under `allow_url_include=0`.
    is_url: bool,
    /// php's `stream factory` resource for the registration.
    res: Value,
}

#[derive(Clone)]
enum Kind {
    Builtin,
    User(Rc<UserWrapper>),
}

/// The table, in php's order. Empty until first touched (see [`table`]).
#[derive(Default)]
struct Table(Vec<(Vec<u8>, Kind)>);

const TABLE_SLOT: &str = "stream_wrappers.table";

/// Every built-in scheme, `stream_get_wrappers()`'s ones first.
fn builtins() -> impl Iterator<Item = &'static str> {
    crate::socket::WRAPPERS.iter().copied().chain(PHP_ONLY_BUILTINS.iter().copied())
}

fn table<'a>(ctx: &'a mut Ctx<'_>) -> &'a mut Vec<(Vec<u8>, Kind)> {
    let t = &mut ctx.ext.slot::<Table>(TABLE_SLOT).0;
    if t.is_empty() {
        t.extend(builtins().map(|b| (b.as_bytes().to_vec(), Kind::Builtin)));
    }
    t
}

/// Whether a script has changed the table this request.
fn touched(ctx: &Ctx) -> bool {
    ctx.ext.slots.contains_key(TABLE_SLOT)
}

/// The entry registered under exactly this name.
fn find(ctx: &mut Ctx, name: &[u8]) -> Option<Kind> {
    if !touched(ctx) {
        return builtins().any(|b| b.as_bytes() == name).then_some(Kind::Builtin);
    }
    table(ctx).iter().find(|(n, _)| n == name).map(|(_, k)| k.clone())
}

/// `stream_get_wrappers()`: the table's names, the built-ins rphp cannot
/// open left out.
pub(crate) fn wrapper_names(ctx: &mut Ctx) -> Array {
    let mut out = Array::new();
    if !touched(ctx) {
        for w in crate::socket::WRAPPERS {
            out.push(Value::string(w.as_bytes()));
        }
        return out;
    }
    for (name, kind) in table(ctx).iter() {
        let hidden = matches!(kind, Kind::Builtin) && PHP_ONLY_BUILTINS.iter().any(|b| b.as_bytes() == &name[..]);
        if !hidden {
            out.push(Value::string(name));
        }
    }
    out
}

/// A `string` parameter, php's TypeError for what cannot be one.
fn str_arg(v: &Value, func: &str, n: usize, name: &str) -> Result<Vec<u8>, Unwind> {
    match &*v.deref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type string, {} given",
            rphp_runtime::value_name(&v.deref())
        ))),
        other => Ok(other.to_php_bytes().to_vec()),
    }
}

/// `stream_wrapper_register(string $protocol, string $class, int $flags = 0): bool`
fn stream_wrapper_register(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "stream_wrapper_register";
    let protocol = str_arg(&args[0], FUNC, 1, "protocol")?;
    let class = str_arg(&args[1], FUNC, 2, "class")?;
    let flags = args.get(2).map_or(0, Value::to_int);
    let Some(cid) = ctx.lookup_class(&class)? else {
        return Err(Unwind::type_error(format!(
            "{FUNC}(): Argument #2 ($class) must be a valid class name, {} given",
            String::from_utf8_lossy(&class)
        )));
    };
    let class_name = ctx.class(cid).name_str();
    // php makes the resource before it tries, so a refused registration
    // still uses up an id.
    let res = ctx.resources.add("stream factory", Box::new(()));
    let shown = String::from_utf8_lossy(&protocol).into_owned();
    let valid = protocol.iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'));
    let taken = find(ctx, &protocol).is_some();
    if !valid || taken {
        ctx.resources.close_value(&res);
        if taken {
            ctx.warn(&format!("{FUNC}(): Protocol {shown}:// is already defined."))?;
        } else {
            ctx.warn(&format!(
                "{FUNC}(): Invalid protocol scheme specified. Unable to register wrapper class {class_name} to {shown}://"
            ))?;
        }
        return Ok(Value::Bool(false));
    }
    let w = UserWrapper { class: cid, class_name, is_url: flags & 1 != 0, res };
    table(ctx).push((protocol, Kind::User(Rc::new(w))));
    Ok(Value::Bool(true))
}

/// Take an entry out of the table; a user one's resource goes with it.
fn remove(ctx: &mut Ctx, name: &[u8]) -> bool {
    let t = table(ctx);
    let Some(at) = t.iter().position(|(n, _)| n == name) else { return false };
    let (_, kind) = t.remove(at);
    if let Kind::User(w) = kind {
        ctx.resources.close_value(&w.res);
    }
    true
}

/// `stream_wrapper_unregister(string $protocol): bool`
fn stream_wrapper_unregister(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "stream_wrapper_unregister";
    let protocol = str_arg(&args[0], FUNC, 1, "protocol")?;
    if !remove(ctx, &protocol) {
        let shown = String::from_utf8_lossy(&protocol);
        ctx.warn(&format!("{FUNC}(): Unable to unregister protocol {shown}://"))?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

/// `stream_wrapper_restore(string $protocol): bool` — a built-in back, at
/// the end of the table.
fn stream_wrapper_restore(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "stream_wrapper_restore";
    let protocol = str_arg(&args[0], FUNC, 1, "protocol")?;
    let shown = String::from_utf8_lossy(&protocol).into_owned();
    if !builtins().any(|b| b.as_bytes() == &protocol[..]) {
        ctx.warn(&format!("{FUNC}(): {shown}:// never existed, nothing to restore"))?;
        return Ok(Value::Bool(false));
    }
    if matches!(find(ctx, &protocol), Some(Kind::Builtin)) {
        ctx.notice(&format!("{FUNC}(): {shown}:// was never changed, nothing to restore"))?;
        return Ok(Value::Bool(true));
    }
    remove(ctx, &protocol);
    table(ctx).push((protocol, Kind::Builtin));
    Ok(Value::Bool(true))
}

// ---- finding the wrapper for a path ----------------------------------------------------

/// Where a path goes (php's `php_stream_locate_url_wrapper`).
pub(crate) enum Route {
    /// A built-in wrapper, or a plain path: the native code's.
    Native,
    /// A class serves it; the path the wrapper opens (a `file://` url loses
    /// its scheme on the way).
    User(Rc<UserWrapper>, Vec<u8>),
    /// No wrapper will take it; the reason has been reported.
    Fail,
    /// A built-in scheme the script unregistered: php falls back to the
    /// plain-file wrapper, which fails to open the url as a file.
    Missing,
}

/// The length of the url scheme `path` starts with, php's way: at least
/// two scheme characters and `://` — or `data:`.
fn scheme_len(path: &[u8]) -> Option<usize> {
    let n = path.iter().take_while(|&&c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.')).count();
    let rest = &path[n..];
    (rest.first() == Some(&b':') && n > 1 && (rest[1..].starts_with(b"//") || (n == 4 && path.starts_with(b"data:"))))
        .then_some(n)
}

/// Where `path` goes. `report` is php's `REPORT_ERRORS`; `include` its
/// `STREAM_OPEN_FOR_INCLUDE`.
pub(crate) fn locate(ctx: &mut Ctx, path: &[u8], report: bool, include: bool, func: &str) -> Result<Route, Unwind> {
    let n = scheme_len(path);
    if n.is_none() && !touched(ctx) {
        return Ok(Route::Native);
    }
    let mut found = None;
    let mut protocol = None;
    if let Some(n) = n {
        let name = &path[..n];
        found = find(ctx, name);
        if found.is_none() {
            found = find(ctx, &name.to_ascii_lowercase());
        }
        if found.is_none() {
            let shown = String::from_utf8_lossy(&name[..n.min(31)]).into_owned();
            ctx.warn(&format!(
                "{func}(): Unable to find the wrapper \"{shown}\" - did you forget to enable it when you configured PHP?"
            ))?;
            // A built-in the script took away: php opens the url as a
            // plain file, which is not there.
            if builtins().any(|b| b.as_bytes().eq_ignore_ascii_case(name)) {
                return Ok(Route::Missing);
            }
        } else {
            protocol = Some(n);
        }
    }
    let is_file = protocol.is_none_or(|n| path[..n].eq_ignore_ascii_case(b"file"));
    if is_file {
        let mut open_path = path.to_vec();
        if let Some(n) = protocol {
            let localhost = path.len() >= 17 && path[..17].eq_ignore_ascii_case(b"file://localhost/");
            if !localhost && path.get(n + 3).is_some_and(|&c| c != b'/') {
                if report {
                    let shown = String::from_utf8_lossy(path);
                    ctx.warn(&format!("{func}(): Remote host file access not supported, {shown}"))?;
                }
                return Ok(Route::Fail);
            }
            // Past `file:`, down to the last of the slashes.
            let mut at = n + 1 + if localhost { 11 } else { 0 };
            while path.get(at + 1) == Some(&b'/') {
                at += 1;
            }
            open_path = path[at..].to_vec();
        }
        if !touched(ctx) {
            return Ok(Route::Native);
        }
        let entry = if protocol.is_some() { found } else { find(ctx, b"file") };
        let entry = entry.or_else(|| find(ctx, b"file"));
        return match entry {
            Some(Kind::Builtin) => Ok(Route::Native),
            Some(Kind::User(w)) => Ok(Route::User(w, open_path)),
            None => {
                if report {
                    ctx.warn(&format!("{func}(): file:// wrapper is disabled in the server configuration"))?;
                }
                Ok(Route::Fail)
            }
        };
    }
    match found {
        Some(Kind::User(w)) => {
            let n = protocol.unwrap_or(0);
            let fopen = ini_on(ctx, "allow_url_fopen");
            if w.is_url && (!fopen || (include && !ini_on(ctx, "allow_url_include"))) {
                if report {
                    let shown = String::from_utf8_lossy(&path[..n]);
                    let why = if fopen { "allow_url_include=0" } else { "allow_url_fopen=0" };
                    ctx.warn(&format!("{func}(): {shown}:// wrapper is disabled in the server configuration by {why}"))?;
                }
                return Ok(Route::Fail);
            }
            Ok(Route::User(w, path.to_vec()))
        }
        _ => Ok(Route::Native),
    }
}

fn ini_on(ctx: &Ctx, name: &str) -> bool {
    ctx.ini_get(name).is_some_and(rphp_runtime::parse_bool)
}

// ---- the object ------------------------------------------------------------------------

/// php's `call_method_if_exists`: `None` when the object has no public
/// method of that name and no `__call`.
fn call(ctx: &mut Ctx, obj: &Object, name: &str, args: &[Value]) -> Result<Option<Value>, Unwind> {
    let mut cb = Array::new();
    cb.push(Value::Object(obj.clone()));
    cb.push(Value::string(name.as_bytes()));
    let cb = Value::Array(cb);
    if !ctx.is_callable(&cb) {
        return Ok(None);
    }
    Ok(Some(ctx.call_value(&cb, args)?.unref()))
}

/// A new instance of the wrapper's class, `$context` set first and the
/// constructor run after, as php's `user_stream_create_object` does.
fn create_object(ctx: &mut Ctx, w: &UserWrapper, context: Value) -> Result<Object, Unwind> {
    let obj = ctx.new_object(w.class)?;
    ctx.assign_prop(&Value::Object(obj.clone()), b"context", context)?;
    if ctx.resolve_method(w.class, b"__construct").is_some() {
        ctx.call_method(&obj, b"__construct", &[])?;
    }
    Ok(obj)
}

/// The stream context a function passes on: the one given, or the default.
fn context_arg(ctx: &mut Ctx, v: Option<&Value>) -> Result<Value, Unwind> {
    match v.map(|v| v.deref().into_owned()) {
        Some(Value::Resource(r)) if r.kind() == "stream-context" => Ok(Value::Resource(r)),
        _ => ctx.call_function(b"stream_context_get_default", &[]),
    }
}

/// A one-shot operation's instance: made, asked `method`, and gone (its
/// destructor run) before the answer is looked at. `None` when the class
/// has no such method.
fn one_shot(ctx: &mut Ctx, w: &UserWrapper, context: Value, method: &str, args: &[Value]) -> Result<Option<Value>, Unwind> {
    let obj = create_object(ctx, w, context)?;
    let r = call(ctx, &obj, method, args);
    drop(obj);
    ctx.run_pending_destructors()?;
    r
}

/// php's answer for the bool-returning one-shot operations: a `bool` is
/// the answer, anything else is `false`, a missing method php's warning.
fn bool_op(ctx: &mut Ctx, w: &UserWrapper, context: Value, method: &str, args: &[Value], func: &str) -> NativeResult {
    match one_shot(ctx, w, context, method, args)? {
        Some(Value::Bool(b)) => Ok(Value::Bool(b)),
        Some(_) => Ok(Value::Bool(false)),
        None => {
            ctx.warn(&format!("{func}(): {}::{method} is not implemented!", w.class_name))?;
            Ok(Value::Bool(false))
        }
    }
}

// ---- the stream ------------------------------------------------------------------------

/// An open user stream (or directory stream): the object, and php's
/// buffered-stream state over it.
pub(crate) struct UserStream {
    obj: Object,
    wrapper: Rc<UserWrapper>,
    mode: Box<str>,
    /// The url it was opened under (`uri`); `None` for a directory.
    uri: Option<Box<str>>,
    /// php's read buffer: `readbuf.len()` is its allocated length, the
    /// bytes between `readpos` and `writepos` are unread.
    readbuf: Vec<u8>,
    readpos: usize,
    writepos: usize,
    position: i64,
    eof: bool,
    /// `PHP_STREAM_FLAG_NO_SEEK`: `stream_seek()` turned out to be missing.
    no_seek: bool,
    /// Something was written since the last flush (`PHP_STREAM_FLAG_WAS_WRITTEN`).
    written: bool,
    chunk: usize,
    dir: bool,
}

impl UserStream {
    fn new(obj: Object, wrapper: Rc<UserWrapper>, mode: &str, uri: Option<&[u8]>, dir: bool) -> UserStream {
        UserStream {
            obj,
            wrapper,
            mode: mode.into(),
            uri: uri.map(|u| String::from_utf8_lossy(u).into()),
            readbuf: Vec::new(),
            readpos: 0,
            writepos: 0,
            position: 0,
            eof: false,
            no_seek: false,
            written: false,
            chunk: CHUNK,
            dir,
        }
    }

    fn buffered(&self) -> usize {
        self.writepos - self.readpos
    }
}

/// The user stream behind a value, if it is one.
fn user_res(v: &Value) -> Option<Resource> {
    match &*v.deref() {
        Value::Resource(r) if r.kind() == "stream" => {
            r.payload().as_ref().is_some_and(|p| p.is::<UserStream>()).then(|| r.clone())
        }
        _ => None,
    }
}

/// Whether `v` is a user stream (file or directory).
pub(crate) fn is_user_stream(v: &Value) -> bool {
    user_res(v).is_some()
}

/// Whether a resource is a user directory stream.
pub(crate) fn is_user_dir(r: &Resource) -> bool {
    r.payload().as_ref().and_then(|p| p.downcast_ref::<UserStream>()).is_some_and(|u| u.dir)
}

fn us<R>(r: &Resource, f: impl FnOnce(&mut UserStream) -> R) -> R {
    r.with::<UserStream, R>(f).expect("a checked user stream")
}

fn obj_of(r: &Resource) -> (Object, Rc<UserWrapper>) {
    us(r, |u| (u.obj.clone(), u.wrapper.clone()))
}

/// php's `php_userstreamop_read`: one `stream_read($count)` and then
/// `stream_eof()`; the bytes, or `None` for php's `-1`.
fn op_read(ctx: &mut Ctx, r: &Resource, count: usize, func: &str) -> Result<Option<Vec<u8>>, Unwind> {
    let (obj, w) = obj_of(r);
    let Some(ret) = call(ctx, &obj, "stream_read", &[Value::Int(count as i64)])? else {
        return Ok(None);
    };
    if matches!(ret, Value::Bool(false)) {
        return Ok(None);
    }
    let mut data = ctx.to_string(&ret)?.as_bytes().to_vec();
    if data.len() > count {
        ctx.warn(&format!(
            "{func}(): {}::stream_read - read {} bytes more data than requested ({} read, {count} max) - excess data will be lost",
            w.class_name,
            data.len() - count,
            data.len()
        ))?;
        data.truncate(count);
    }
    match call(ctx, &obj, "stream_eof", &[])? {
        Some(v) => {
            if v.to_bool() {
                us(r, |u| u.eof = true);
            }
        }
        None => {
            ctx.warn(&format!("{func}(): {}::stream_eof is not implemented! Assuming EOF", w.class_name))?;
            us(r, |u| u.eof = true);
            return Ok(None);
        }
    }
    Ok(Some(data))
}

/// php's `_php_stream_fill_read_buffer`: make room the way php does (which
/// decides the `$count` the object is asked for) and read once. `false`
/// when the read failed.
fn fill(ctx: &mut Ctx, r: &Resource, size: usize, func: &str) -> Result<bool, Unwind> {
    let count = us(r, |u| {
        if u.buffered() >= size {
            return None;
        }
        if !u.readbuf.is_empty() && u.readbuf.len() - u.writepos < u.chunk {
            u.readbuf.copy_within(u.readpos..u.writepos, 0);
            u.writepos -= u.readpos;
            u.readpos = 0;
        }
        if u.readbuf.len() - u.writepos < u.chunk {
            let len = u.readbuf.len() + u.chunk;
            u.readbuf.resize(len, 0);
        }
        Some(u.readbuf.len() - u.writepos)
    });
    let Some(count) = count else { return Ok(true) };
    let Some(data) = op_read(ctx, r, count, func)? else { return Ok(false) };
    us(r, |u| {
        let end = u.writepos + data.len();
        u.readbuf[u.writepos..end].copy_from_slice(&data);
        u.writepos = end;
    });
    Ok(true)
}

/// Up to `n` buffered bytes off the buffer.
fn take(r: &Resource, n: usize) -> Vec<u8> {
    us(r, |u| {
        let n = n.min(u.buffered());
        let out = u.readbuf[u.readpos..u.readpos + n].to_vec();
        u.readpos += n;
        out
    })
}

/// php's `_php_stream_read` for a user stream: the buffer first, then at
/// most one fill. `None` for php's `-1`.
fn read(ctx: &mut Ctx, r: &Resource, size: usize, func: &str) -> Result<Option<Vec<u8>>, Unwind> {
    let mut out = take(r, size);
    if out.len() < size {
        if !fill(ctx, r, size - out.len(), func)? {
            if out.is_empty() {
                return Ok(None);
            }
        } else {
            let more = take(r, size - out.len());
            out.extend_from_slice(&more);
        }
    }
    us(r, |u| u.position += out.len() as i64);
    Ok(Some(out))
}

/// php's `_php_stream_get_line`: a line up to and including `\n`, at most
/// `maxlen - 1` bytes (`None`: any length). `None` when nothing was read.
fn get_line(ctx: &mut Ctx, r: &Resource, maxlen: Option<usize>, func: &str) -> Result<Option<Vec<u8>>, Unwind> {
    let mut out = Vec::new();
    let mut room = maxlen;
    loop {
        let (avail, eof) = us(r, |u| (u.buffered(), u.eof));
        if avail > 0 {
            let (cpy, done) = us(r, |u| {
                let hay = &u.readbuf[u.readpos..u.writepos];
                let (mut cpy, mut done) = match hay.iter().position(|&b| b == b'\n') {
                    Some(i) => (i + 1, true),
                    None => (hay.len(), false),
                };
                if let Some(m) = room {
                    if cpy >= m - 1 {
                        cpy = m - 1;
                        done = true;
                    }
                }
                (cpy, done)
            });
            let got = take(r, cpy);
            us(r, |u| u.position += got.len() as i64);
            out.extend_from_slice(&got);
            if let Some(m) = room.as_mut() {
                *m -= cpy;
            }
            if done {
                break;
            }
        } else if eof {
            break;
        } else {
            let toread = match room {
                None => us(r, |u| u.chunk),
                Some(m) => (m - 1).min(us(r, |u| u.chunk)),
            };
            fill(ctx, r, toread, func)?;
            if us(r, |u| u.buffered()) == 0 {
                break;
            }
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

/// php's `php_stream_get_record` (`stream_get_line()`).
fn get_record(ctx: &mut Ctx, r: &Resource, maxlen: usize, delim: &[u8], func: &str) -> Result<Option<Vec<u8>>, Unwind> {
    if maxlen == 0 {
        return Ok(None);
    }
    let search = |r: &Resource, skip: usize| -> Option<usize> {
        us(r, |u| {
            let seek = u.buffered().min(maxlen);
            let hay = &u.readbuf[u.readpos..u.readpos + seek];
            if delim.is_empty() || skip > hay.len() {
                return None;
            }
            hay[skip..].windows(delim.len()).position(|w| w == delim).map(|i| i + skip)
        })
    };
    let has_delim = !delim.is_empty();
    let mut found = if has_delim { search(r, 0) } else { None };
    let mut buffered = us(r, |u| u.buffered());
    while found.is_none() && buffered < maxlen {
        let now = (maxlen - buffered).min(us(r, |u| u.chunk));
        fill(ctx, r, buffered + now, func)?;
        let just = us(r, |u| u.buffered()) - buffered;
        if just == 0 {
            break;
        }
        if has_delim {
            let skip = buffered.saturating_sub(delim.len() - 1);
            found = search(r, skip);
            if found.is_some() {
                break;
            }
        }
        buffered += just;
    }
    let (amount, eof) = us(r, |u| (u.buffered(), u.eof));
    let len = if let Some(at) = found {
        at
    } else if !has_delim && amount >= maxlen {
        maxlen
    } else if (amount < maxlen && !eof) || (amount == 0 && eof) {
        return Ok(None);
    } else {
        amount.min(maxlen)
    };
    let out = take(r, len);
    us(r, |u| {
        u.position += out.len() as i64;
        if found.is_some() {
            u.readpos += delim.len();
            u.position += delim.len() as i64;
        }
    });
    Ok(Some(out))
}

/// php's `php_stream_eof`: data in the buffer is not the end; otherwise
/// the flag, or — while it is not set — the object's `stream_eof()`.
fn eof(ctx: &mut Ctx, r: &Resource, func: &str) -> Result<bool, Unwind> {
    let (avail, eof) = us(r, |u| (u.buffered(), u.eof));
    if avail > 0 {
        return Ok(false);
    }
    if eof {
        return Ok(true);
    }
    let (obj, w) = obj_of(r);
    let at_end = match call(ctx, &obj, "stream_eof", &[])? {
        Some(Value::Bool(b)) => b,
        _ => {
            ctx.warn(&format!("{func}(): {}::stream_eof is not implemented! Assuming EOF", w.class_name))?;
            true
        }
    };
    if at_end {
        us(r, |u| u.eof = true);
    }
    Ok(at_end)
}

/// php's `php_userstreamop_write`: the count written, or `-1`.
fn op_write(ctx: &mut Ctx, r: &Resource, data: &[u8], func: &str) -> Result<i64, Unwind> {
    let (obj, w) = obj_of(r);
    let Some(ret) = call(ctx, &obj, "stream_write", &[Value::string(data)])? else {
        ctx.warn(&format!("{func}(): {}::stream_write is not implemented!", w.class_name))?;
        return Ok(-1);
    };
    if matches!(ret, Value::Bool(false)) {
        return Ok(-1);
    }
    let n = ret.to_int();
    let count = data.len() as i64;
    if n > 0 && n > count {
        ctx.warn(&format!(
            "{func}(): {}::stream_write wrote {} bytes more data than requested ({n} written, {count} max)",
            w.class_name,
            n - count
        ))?;
        return Ok(count);
    }
    Ok(n)
}

/// php's `_php_stream_write` for a user stream: in chunk-sized pieces, the
/// read buffer given up first (and the object told where writing starts).
fn write(ctx: &mut Ctx, r: &Resource, data: &[u8], func: &str) -> Result<i64, Unwind> {
    if data.is_empty() {
        return Ok(0);
    }
    let resync = us(r, |u| {
        if !u.no_seek && u.readpos != u.writepos {
            u.readpos = 0;
            u.writepos = 0;
            Some(u.position)
        } else {
            None
        }
    });
    if let Some(pos) = resync {
        op_seek(ctx, r, pos, 0, func)?;
    }
    let chunk = us(r, |u| u.chunk);
    let mut done = 0i64;
    let mut rest = data;
    while !rest.is_empty() {
        let n = rest.len().min(chunk);
        let just = op_write(ctx, r, &rest[..n], func)?;
        if just <= 0 {
            if done == 0 {
                done = just;
            }
            break;
        }
        let just = just as usize;
        rest = &rest[just.min(rest.len())..];
        done += just as i64;
        us(r, |u| u.position += just as i64);
    }
    us(r, |u| u.written = true);
    Ok(done)
}

/// php's `php_userstreamop_seek`: `stream_seek()` and, when it agreed,
/// `stream_tell()` for the new position. `0` or `-1`.
fn op_seek(ctx: &mut Ctx, r: &Resource, offset: i64, whence: i64, func: &str) -> Result<i64, Unwind> {
    let (obj, w) = obj_of(r);
    match call(ctx, &obj, "stream_seek", &[Value::Int(offset), Value::Int(whence)])? {
        None => {
            us(r, |u| u.no_seek = true);
            return Ok(-1);
        }
        Some(v) if v.to_bool() => {}
        Some(_) => return Ok(-1),
    }
    match call(ctx, &obj, "stream_tell", &[])? {
        Some(Value::Int(n)) => {
            us(r, |u| u.position = n);
            Ok(0)
        }
        None => {
            ctx.warn(&format!("{func}(): {}::stream_tell is not implemented!", w.class_name))?;
            Ok(-1)
        }
        Some(_) => Ok(-1),
    }
}

/// php's `_php_stream_seek` for a user stream: inside the buffer without a
/// call; otherwise the object's seek (`SEEK_CUR` made absolute); a stream
/// that cannot seek emulates a forward `SEEK_CUR` by reading.
fn seek(ctx: &mut Ctx, r: &Resource, offset: i64, whence: i64, func: &str) -> Result<i64, Unwind> {
    let inside = us(r, |u| {
        let avail = u.buffered() as i64;
        let skip = match whence {
            1 if offset > 0 && offset <= avail => Some(offset),
            0 if offset > u.position && offset <= u.position + avail => Some(offset - u.position),
            _ => None,
        };
        if let Some(n) = skip {
            u.readpos += n as usize;
            u.position += n;
            u.eof = false;
        }
        skip.is_some()
    });
    if inside {
        return Ok(0);
    }
    let (mut offset, mut whence) = (offset, whence);
    if !us(r, |u| u.no_seek) {
        if whence == 1 {
            offset = us(r, |u| u.position).saturating_add(offset);
            whence = 0;
        }
        let ret = if whence == 0 && offset < 0 { -1 } else { op_seek(ctx, r, offset, whence, func)? };
        let no_seek = us(r, |u| u.no_seek);
        if !no_seek || ret == 0 {
            us(r, |u| {
                if ret == 0 {
                    u.eof = false;
                }
                u.readpos = 0;
                u.writepos = 0;
            });
            return Ok(ret);
        }
    }
    if whence == 1 && offset >= 0 {
        let mut left = offset as usize;
        while left > 0 {
            match read(ctx, r, left.min(1024), func)? {
                Some(b) if !b.is_empty() => left -= b.len(),
                _ => return Ok(-1),
            }
        }
        us(r, |u| u.eof = false);
        return Ok(0);
    }
    ctx.warn(&format!("{func}(): Stream does not support seeking"))?;
    Ok(-1)
}

/// php's `_php_stream_flush`: `stream_flush()`, `0` when it said yes.
fn flush(ctx: &mut Ctx, r: &Resource) -> Result<bool, Unwind> {
    let (obj, _) = obj_of(r);
    us(r, |u| u.written = false);
    Ok(call(ctx, &obj, "stream_flush", &[])?.is_some_and(|v| v.to_bool()))
}

/// Close a user stream: the flush a write left owing, `stream_close()` (or
/// `dir_closedir()`), then the stream and — once nothing holds it — the
/// object are gone.
fn close(ctx: &mut Ctx, r: &Resource) -> Result<(), Unwind> {
    let (written, dir) = us(r, |u| (u.written, u.dir));
    if written {
        flush(ctx, r)?;
    }
    let (obj, _) = obj_of(r);
    call(ctx, &obj, if dir { "dir_closedir" } else { "stream_close" }, &[])?;
    drop(obj);
    ctx.resources.close(r.id());
    ctx.run_pending_destructors()
}

/// php's `statbuf_from_array`: the thirteen fields a stat array names.
fn stat_fields(a: &Array) -> [i64; 13] {
    let mut out = [0i64; 13];
    for (i, key) in STAT_KEYS.iter().enumerate() {
        if let Some(v) = a.get(&ArrayKey::str(key.as_bytes())) {
            out[i] = v.deref().to_int();
        }
    }
    out
}

const STAT_KEYS: [&str; 13] = [
    "dev", "ino", "mode", "nlink", "uid", "gid", "rdev", "size", "atime", "mtime", "ctime", "blksize", "blocks",
];

/// `stream_stat()`: the fields, `None` when it did not answer an array.
fn op_stat(ctx: &mut Ctx, r: &Resource, func: &str) -> Result<Option<[i64; 13]>, Unwind> {
    let (obj, w) = obj_of(r);
    match call(ctx, &obj, "stream_stat", &[])? {
        Some(Value::Array(a)) => Ok(Some(stat_fields(&a))),
        Some(_) => Ok(None),
        None => {
            ctx.warn(&format!("{func}(): {}::stream_stat is not implemented!", w.class_name))?;
            Ok(None)
        }
    }
}

/// php's `PHP_STREAM_OPTION_RETURN_*`.
const OPT_OK: i64 = 0;
const OPT_ERR: i64 = -1;

/// `stream_set_option($option, $arg1, $arg2)`.
fn op_set_option(ctx: &mut Ctx, r: &Resource, option: i64, a1: Value, a2: Value, func: &str) -> Result<i64, Unwind> {
    let (obj, w) = obj_of(r);
    match call(ctx, &obj, "stream_set_option", &[Value::Int(option), a1, a2])? {
        None => {
            ctx.warn(&format!("{func}(): {}::stream_set_option is not implemented!", w.class_name))?;
            Ok(OPT_ERR)
        }
        Some(v) if v.to_bool() => Ok(OPT_OK),
        Some(_) => Ok(OPT_ERR),
    }
}

/// `stream_lock($operation)`: `0` for a lock taken, non-zero otherwise.
fn op_lock(ctx: &mut Ctx, r: &Resource, operation: i64, func: &str) -> Result<i64, Unwind> {
    let (obj, w) = obj_of(r);
    match call(ctx, &obj, "stream_lock", &[Value::Int(operation)])? {
        Some(Value::Bool(b)) => Ok(i64::from(!b)),
        Some(_) => Ok(-2),
        None if operation == 0 => Ok(0),
        None => {
            ctx.warn(&format!("{func}(): {}::stream_lock is not implemented!", w.class_name))?;
            Ok(OPT_ERR)
        }
    }
}

/// `stream_truncate($size)`: whether it cut the stream.
fn op_truncate(ctx: &mut Ctx, r: &Resource, size: i64, func: &str) -> Result<bool, Unwind> {
    let (obj, w) = obj_of(r);
    match call(ctx, &obj, "stream_truncate", &[Value::Int(size)])? {
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => {
            ctx.warn(&format!("{func}(): {}::stream_truncate did not return a boolean!", w.class_name))?;
            Ok(false)
        }
        None => {
            ctx.warn(&format!("{func}(): {}::stream_truncate is not implemented!", w.class_name))?;
            Ok(false)
        }
    }
}

/// php's `_php_stream_copy_to_mem`: what is left, or up to `maxlen`.
fn copy_to_mem(ctx: &mut Ctx, r: &Resource, maxlen: Option<usize>, func: &str) -> Result<Vec<u8>, Unwind> {
    let mut out = Vec::new();
    if maxlen == Some(0) {
        return Ok(out);
    }
    if let Some(max) = maxlen.filter(|m| *m < 4 * CHUNK) {
        while out.len() < max && !eof(ctx, r, func)? {
            match read(ctx, r, max - out.len(), func)? {
                Some(b) if !b.is_empty() => out.extend_from_slice(&b),
                _ => break,
            }
        }
        return Ok(out);
    }
    let pos = us(r, |u| u.position);
    let mut cap = match op_stat(ctx, r, func)? {
        Some(s) if s[7] > 0 => (s[7] - pos).max(0) as usize + CHUNK,
        _ => CHUNK,
    };
    loop {
        let want = cap - out.len();
        let want = maxlen.map_or(want, |m| want.min(m - out.len()));
        if want == 0 {
            break;
        }
        match read(ctx, r, want, func)? {
            Some(b) if !b.is_empty() => {
                out.extend_from_slice(&b);
                if out.len() + CHUNK / 4 >= cap {
                    cap += CHUNK;
                }
            }
            _ => break,
        }
    }
    Ok(out)
}

// ---- opening ---------------------------------------------------------------------------

/// A stream `open` made, and the path its object reported.
type Opened = (Value, Option<Vec<u8>>);

/// A user wrapper and the path it opens.
type Target = (Rc<UserWrapper>, Vec<u8>);

/// Open a user stream: an instance, `stream_open()`, and on success the
/// stream resource with the path the object reported (`$opened_path`).
/// `None` after php's warning (when `report`) if it refused.
#[allow(clippy::too_many_arguments)]
fn open(
    ctx: &mut Ctx,
    w: &Rc<UserWrapper>,
    path: &[u8],
    shown: &[u8],
    mode: &str,
    options: i64,
    context: Value,
    func: &str,
) -> Result<Option<Opened>, Unwind> {
    let obj = create_object(ctx, w, context)?;
    let opened = PhpRef::new(Value::Null);
    let args = [
        Value::string(path),
        Value::string(mode.as_bytes()),
        Value::Int(options & !STREAM_REPORT_ERRORS),
        Value::Ref(opened.clone()),
    ];
    let why = match call(ctx, &obj, "stream_open", &args)? {
        Some(v) if v.to_bool() => None,
        Some(_) => Some("call failed"),
        None => Some("is not implemented"),
    };
    if let Some(why) = why {
        drop(obj);
        ctx.run_pending_destructors()?;
        let shown = String::from_utf8_lossy(shown);
        ctx.warn(&format!(
            "{func}({shown}): Failed to open stream: \"{}::stream_open\" {why}",
            w.class_name
        ))?;
        return Ok(None);
    }
    let opened_path = match opened.get() {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    };
    let stream = UserStream::new(obj, w.clone(), mode, Some(shown), false);
    let res = ctx.resources.add("stream", Box::new(stream));
    // An append stream learns where it starts (php's `SEEK_CUR` by 0).
    if mode.contains('a') {
        let r = user_res(&res).expect("just made");
        op_seek(ctx, &r, 0, 1, func)?;
    }
    Ok(Some((res, opened_path)))
}

/// An open through a user wrapper for a whole-file function
/// (`file_get_contents()`, `file()`, `readfile()`, `file_put_contents()`):
/// the stream, or the `false` to answer.
fn open_for(
    ctx: &mut Ctx,
    route: Route,
    args: &[Value],
    func: &str,
    mode: &str,
    flags_at: usize,
    context_at: usize,
) -> Result<Result<Resource, Value>, Unwind> {
    let path = args[0].to_php_bytes().to_vec();
    let use_path = args.get(flags_at).is_some_and(|v| v.to_int() & 1 != 0);
    let Some((w, p)) = user_route(ctx, route, &path, func)? else {
        return Ok(Err(Value::Bool(false)));
    };
    let context = context_arg(ctx, args.get(context_at))?;
    let options = if use_path { STREAM_USE_PATH } else { 0 } | STREAM_REPORT_ERRORS;
    match open(ctx, &w, &p, &path, mode, options, context, func)? {
        Some((res, _)) => Ok(Ok(user_res(&res).expect("just made"))),
        None => Ok(Err(Value::Bool(false))),
    }
}

/// The wrapper an opener goes to, or php's "Failed to open stream" for a
/// route that goes nowhere.
fn user_route(ctx: &mut Ctx, route: Route, path: &[u8], func: &str) -> Result<Option<Target>, Unwind> {
    match route {
        Route::User(w, p) => Ok(Some((w, p))),
        Route::Missing | Route::Native => {
            fail_open(ctx, func, path, "No such file or directory")?;
            Ok(None)
        }
        Route::Fail => {
            fail_open(ctx, func, path, "no suitable wrapper could be found")?;
            Ok(None)
        }
    }
}

fn fail_open(ctx: &mut Ctx, func: &str, path: &[u8], why: &str) -> Result<(), Unwind> {
    let shown = String::from_utf8_lossy(path);
    ctx.warn(&format!("{func}({shown}): Failed to open stream: {why}"))
}

/// Where a path function's path goes, asked once — php's lookup warns as
/// it looks. `None` when the native code takes it.
fn route_of(ctx: &mut Ctx, path: &[u8], report: bool, func: &str) -> Result<Option<Route>, Unwind> {
    if scheme_len(path).is_none() && !touched(ctx) {
        return Ok(None);
    }
    Ok(match locate(ctx, path, report, false, func)? {
        Route::Native => None,
        r => Some(r),
    })
}

// ---- handle functions ------------------------------------------------------------------

/// A registered user wrapper's stream takes the call: the hook the stream
/// functions in `file.rs`/`file2.rs`/`socket.rs` make first. `None` when
/// `args[0]` is not a user stream (nor, for `fclose()`, a `popen()` one).
pub(crate) fn handle_op(ctx: &mut Ctx, func: &str, args: &mut [Value]) -> Result<Option<Value>, Unwind> {
    let Some(r) = user_res(&args[0]) else {
        if func == "fclose" {
            return close_popen(ctx, &args[0]);
        }
        return Ok(None);
    };
    let int_arg = |args: &[Value], i: usize| args.get(i).map(|v| v.deref().into_owned()).filter(|v| !matches!(v, Value::Null | Value::Uninit)).map(|v| v.to_int());
    Ok(Some(match func {
        "fclose" => {
            close(ctx, &r)?;
            Value::Bool(true)
        }
        "fwrite" | "fputs" => {
            let mut data = ctx.to_string(&args[1])?.as_bytes().to_vec();
            if let Some(len) = int_arg(args, 2) {
                data.truncate(len.max(0) as usize);
            }
            match write(ctx, &r, &data, func)? {
                n if n < 0 => Value::Bool(false),
                n => Value::Int(n),
            }
        }
        "fread" => {
            let len = args[1].to_int();
            if len <= 0 {
                return Err(Unwind::value_error("fread(): Argument #2 ($length) must be greater than 0"));
            }
            match read(ctx, &r, len as usize, func)? {
                Some(b) => Value::Str(Str::from_vec(b)),
                None => Value::Bool(false),
            }
        }
        "fgets" => {
            let len = match int_arg(args, 1) {
                None => None,
                Some(n) if n <= 0 => {
                    return Err(Unwind::value_error("fgets(): Argument #2 ($length) must be greater than 0"))
                }
                Some(n) => Some(n as usize),
            };
            match get_line(ctx, &r, len, func)? {
                Some(b) => Value::Str(Str::from_vec(b)),
                None => Value::Bool(false),
            }
        }
        "fgetc" => match read(ctx, &r, 1, func)? {
            Some(b) if !b.is_empty() => Value::Str(Str::from_vec(b)),
            _ => Value::Bool(false),
        },
        "feof" => Value::Bool(eof(ctx, &r, func)?),
        "ftell" => Value::Int(us(&r, |u| u.position)),
        "fseek" => {
            let whence = args.get(2).map_or(0, Value::to_int);
            Value::Int(seek(ctx, &r, args[1].to_int(), whence, func)?)
        }
        "rewind" => Value::Bool(seek(ctx, &r, 0, 0, func)? == 0),
        "fflush" => Value::Bool(flush(ctx, &r)?),
        "stream_get_contents" => {
            let max = match int_arg(args, 1) {
                None | Some(-1) => None,
                Some(n) if n < 0 => {
                    return Err(Unwind::value_error(
                        "stream_get_contents(): Argument #2 ($length) must be greater than or equal to -1",
                    ))
                }
                Some(n) => Some(n as usize),
            };
            let offset = args.get(2).map_or(-1, Value::to_int);
            if offset >= 0 {
                let pos = us(&r, |u| u.position);
                let res = if offset > pos {
                    seek(ctx, &r, offset - pos, 1, func)?
                } else if offset < pos {
                    seek(ctx, &r, offset, 0, func)?
                } else {
                    0
                };
                if res != 0 {
                    ctx.warn(&format!("{func}(): Failed to seek to position {offset} in the stream"))?;
                    return Ok(Some(Value::Bool(false)));
                }
            }
            Value::Str(Str::from_vec(copy_to_mem(ctx, &r, max, func)?))
        }
        "stream_get_line" => {
            let max = args[1].to_int();
            if max < 0 {
                return Err(Unwind::value_error(
                    "stream_get_line(): Argument #2 ($length) must be greater than or equal to 0",
                ));
            }
            let max = if max == 0 { CHUNK } else { max as usize };
            let ending = args.get(2).map(|v| v.to_php_bytes().to_vec()).unwrap_or_default();
            match get_record(ctx, &r, max, &ending, func)? {
                Some(b) => Value::Str(Str::from_vec(b)),
                None => Value::Bool(false),
            }
        }
        "stream_get_meta_data" => meta_data(ctx, &r, func)?,
        "stream_set_blocking" => {
            let on = i64::from(args[1].to_bool());
            Value::Bool(op_set_option(ctx, &r, OPTION_BLOCKING, Value::Int(on), Value::Null, func)? != OPT_ERR)
        }
        "stream_set_timeout" => {
            let mut secs = args[1].to_int();
            let usecs = match int_arg(args, 2) {
                Some(us) => {
                    secs += us / 1_000_000;
                    us % 1_000_000
                }
                None => 0,
            };
            let r = op_set_option(ctx, &r, OPTION_READ_TIMEOUT, Value::Int(secs), Value::Int(usecs), func)?;
            Value::Bool(r == OPT_OK)
        }
        "stream_set_write_buffer" | "stream_set_read_buffer" => {
            let option = if func == "stream_set_write_buffer" { OPTION_WRITE_BUFFER } else { OPTION_READ_BUFFER };
            let size = args[1].to_int();
            let (mode, size) = if size == 0 { (BUFFER_NONE, BUFSIZ) } else { (BUFFER_FULL, size) };
            let r = op_set_option(ctx, &r, option, Value::Int(mode), Value::Int(size), func)?;
            Value::Int(if r == OPT_OK { 0 } else { -1 })
        }
        "stream_set_chunk_size" => {
            let size = args[1].to_int() as usize;
            Value::Int(us(&r, |u| std::mem::replace(&mut u.chunk, size)) as i64)
        }
        "flock" => {
            let operation = args[1].to_int();
            if args.len() > 2 {
                args[2] = Value::Int(0);
            }
            Value::Bool(op_lock(ctx, &r, (operation & 3) | (operation & 4), func)? == 0)
        }
        "ftruncate" => {
            let (obj, _) = obj_of(&r);
            if call_exists(ctx, &obj, "stream_truncate") {
                Value::Bool(op_truncate(ctx, &r, args[1].to_int(), func)?)
            } else {
                ctx.warn(&format!("{func}(): Can't truncate this stream!"))?;
                Value::Bool(false)
            }
        }
        "fstat" => match op_stat(ctx, &r, func)? {
            Some(s) => Value::Array(crate::file2::stat_array(&s)),
            None => Value::Bool(false),
        },
        "fpassthru" | "readfile" => {
            let mut n = 0i64;
            while let Some(b) = read(ctx, &r, CHUNK, func)? {
                if b.is_empty() {
                    break;
                }
                n += b.len() as i64;
                ctx.echo(&b);
            }
            Value::Int(n)
        }
        "fsync" | "fdatasync" => {
            ctx.warn(&format!("{func}(): Can't fsync this stream!"))?;
            Value::Bool(false)
        }
        "stream_is_local" => Value::Bool(!us(&r, |u| u.wrapper.is_url)),
        "stream_isatty" => Value::Bool(false),
        _ => return Ok(None),
    }))
}

/// Whether the object answers `name` (php's `zend_is_callable_ex`).
fn call_exists(ctx: &mut Ctx, obj: &Object, name: &str) -> bool {
    let mut cb = Array::new();
    cb.push(Value::Object(obj.clone()));
    cb.push(Value::string(name.as_bytes()));
    ctx.is_callable(&Value::Array(cb))
}

/// `stream_get_meta_data()` of a user stream, php's keys in php's order.
fn meta_data(ctx: &mut Ctx, r: &Resource, func: &str) -> NativeResult {
    // A directory stream has no `stream_eof()` to ask.
    let eof = if us(r, |u| u.dir) { us(r, |u| u.eof) } else { eof(ctx, r, func)? };
    us(r, |u| {
        let mut out = Array::new();
        let mut set = |k: &str, v: Value| out.set(ArrayKey::str(k.as_bytes()), v);
        set("timed_out", Value::Bool(false));
        set("blocked", Value::Bool(true));
        set("eof", Value::Bool(eof));
        set("wrapper_data", Value::Object(u.obj.clone()));
        set("wrapper_type", Value::string(b"user-space"));
        set("stream_type", Value::string(if u.dir { b"user-space-dir".as_slice() } else { b"user-space" }));
        set("mode", Value::string(u.mode.as_bytes()));
        set("unread_bytes", Value::Int(u.buffered() as i64));
        set("seekable", Value::Bool(!u.no_seek));
        if let Some(uri) = &u.uri {
            set("uri", Value::string(uri.as_bytes()));
        }
        Ok(Value::Array(out))
    })
}

/// `stream_copy_to_stream()` when either end is a user stream: read the
/// source through its stream, write through the destination's.
pub(crate) fn copy_streams(ctx: &mut Ctx, args: &mut [Value]) -> Result<Option<Value>, Unwind> {
    let (from, to) = (user_res(&args[0]), user_res(&args[1]));
    if from.is_none() && to.is_none() {
        return Ok(None);
    }
    const FUNC: &str = "stream_copy_to_stream";
    let max = args.get(2).map(|v| v.deref().into_owned()).filter(|v| !matches!(v, Value::Null | Value::Uninit)).map(|v| v.to_int()).filter(|n| *n >= 0);
    let offset = args.get(3).map_or(0, Value::to_int);
    let data = match &from {
        Some(r) => {
            if offset > 0 && seek(ctx, r, offset, 0, FUNC)? != 0 {
                ctx.warn(&format!("{FUNC}(): Failed to seek to position {offset} in the stream"))?;
                return Ok(Some(Value::Bool(false)));
            }
            // A chunk read, then written, as php's copy loop goes.
            let mut total = 0usize;
            loop {
                let want = max.map_or(CHUNK, |m| (m as usize - total).min(CHUNK));
                if want == 0 {
                    break;
                }
                let chunk = match read(ctx, r, want, FUNC)? {
                    Some(b) if !b.is_empty() => b,
                    _ => break,
                };
                total += chunk.len();
                let n = match &to {
                    Some(t) => write(ctx, t, &chunk, FUNC)?,
                    None => ctx.call_function(b"fwrite", &[args[1].clone(), Value::string(&chunk)])?.to_int(),
                };
                if n != chunk.len() as i64 {
                    return Ok(Some(Value::Bool(false)));
                }
            }
            return Ok(Some(Value::Int(total as i64)));
        }
        None => {
            let a = [args[0].clone(), max.map_or(Value::Null, Value::Int), Value::Int(if offset > 0 { offset } else { -1 })];
            ctx.call_function(b"stream_get_contents", &a)?.to_php_bytes().to_vec()
        }
    };
    let n = match &to {
        Some(r) => write(ctx, r, &data, FUNC)?,
        None => ctx.call_function(b"fwrite", &[args[1].clone(), Value::string(&data)])?.to_int(),
    };
    Ok(Some(if n < 0 { Value::Bool(false) } else { Value::Int(n) }))
}

/// `stream_select()` meets a user stream: `stream_cast(STREAM_CAST_FOR_SELECT)`.
/// `None` when `v` is not a user stream; `Some(None)` when it cannot be
/// selected on (php's warnings said so and it drops out of the set);
/// otherwise the stream to poll in its place.
pub(crate) fn select_target(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Option<Option<Value>>, Unwind> {
    let Some(r) = user_res(v) else { return Ok(None) };
    let (obj, w) = obj_of(&r);
    let target = match call(ctx, &obj, "stream_cast", &[Value::Int(CAST_FOR_SELECT)])? {
        None => {
            ctx.warn(&format!("{func}(): {}::stream_cast is not implemented!", w.class_name))?;
            None
        }
        Some(ret) if !ret.to_bool() => None,
        Some(ret) => match &ret {
            Value::Resource(t) if t.kind() == "stream" => {
                if t.ptr_eq(&r) {
                    ctx.warn(&format!("{func}(): {}::stream_cast must not return itself", w.class_name))?;
                    None
                } else if is_user_stream(&ret) {
                    None
                } else {
                    Some(ret.clone())
                }
            }
            _ => {
                ctx.warn(&format!("{func}(): {}::stream_cast must return a stream resource", w.class_name))?;
                None
            }
        },
    };
    if target.is_none() {
        ctx.warn(&format!("{func}(): Cannot represent a stream of type user-space as a select()able descriptor"))?;
    }
    Ok(Some(target))
}

// ---- path functions --------------------------------------------------------------------

/// The per-request stat cache for user urls: php's one `stat` and one
/// `lstat` slot (`BG(CurrentStatFile)`/`BG(CurrentLStatFile)`), which
/// `clearstatcache()` and every change a plain path sees empty.
#[derive(Default)]
struct StatCache {
    stat: Option<(Vec<u8>, [i64; 13])>,
    lstat: Option<(Vec<u8>, [i64; 13])>,
}

const STAT_SLOT: &str = "stream_wrappers.stat";

/// Empty the user-url stat cache (`filestat::clear_stat_cache`).
pub(crate) fn clear_stat_cache(ctx: &mut Ctx) {
    if let Some(c) = ctx.ext.slots.get_mut(STAT_SLOT).and_then(|b| b.downcast_mut::<StatCache>()) {
        *c = StatCache::default();
    }
}

/// `url_stat($path, $flags)`: the fields, `None` when it did not answer an
/// array.
fn url_stat(ctx: &mut Ctx, w: &UserWrapper, path: &[u8], flags: i64, func: &str) -> Result<Option<[i64; 13]>, Unwind> {
    match one_shot(ctx, w, Value::Null, "url_stat", &[Value::string(path), Value::Int(flags)])? {
        Some(Value::Array(a)) => Ok(Some(stat_fields(&a))),
        Some(_) => Ok(None),
        None => {
            ctx.warn(&format!("{func}(): {}::url_stat is not implemented!", w.class_name))?;
            Ok(None)
        }
    }
}

/// The stat-based functions (php's `php_stat`) over a user url.
fn php_stat(ctx: &mut Ctx, w: &UserWrapper, path: &[u8], shown: &[u8], func: &str) -> NativeResult {
    let link = matches!(func, "filetype" | "is_link" | "lstat");
    let exists = matches!(func, "file_exists" | "is_writable" | "is_writeable" | "is_readable" | "is_executable" | "is_file" | "is_dir" | "is_link");
    let cached = {
        let c = ctx.ext.slot::<StatCache>(STAT_SLOT);
        let slot = if link { &c.lstat } else { &c.stat };
        slot.as_ref().filter(|(p, _)| p == shown).map(|(_, s)| *s)
    };
    let st = match cached {
        Some(s) => Some(s),
        None => {
            let mut flags = STREAM_URL_STAT_IGNORE_OPEN_BASEDIR;
            if link {
                flags |= STREAM_URL_STAT_LINK;
            }
            if exists {
                flags |= STREAM_URL_STAT_QUIET;
            }
            let st = url_stat(ctx, w, path, flags, func)?;
            if let Some(s) = st {
                ctx.ext.stat_cache = None;
                let c = ctx.ext.slot::<StatCache>(STAT_SLOT);
                if link {
                    c.lstat = Some((shown.to_vec(), s));
                    if s[2] & S_IFMT != S_IFLNK {
                        c.stat = Some((shown.to_vec(), s));
                    }
                } else {
                    c.stat = Some((shown.to_vec(), s));
                }
            }
            st
        }
    };
    let Some(s) = st else {
        if !exists {
            let l = if link { "L" } else { "" };
            ctx.warn(&format!("{func}(): {l}stat failed for {}", String::from_utf8_lossy(shown)))?;
        }
        return Ok(Value::Bool(false));
    };
    let mode = s[2];
    let (mut rmask, mut wmask, mut xmask) = (0o004, 0o002, 0o001);
    if matches!(func, "is_readable" | "is_writable" | "is_writeable" | "is_executable") {
        let uid = i64::from(rustix::process::getuid().as_raw());
        let gid = i64::from(rustix::process::getgid().as_raw());
        let in_group = s[5] == gid
            || rustix::process::getgroups().is_ok_and(|g| g.iter().any(|g| i64::from(g.as_raw()) == s[5]));
        if s[4] == uid {
            (rmask, wmask, xmask) = (0o400, 0o200, 0o100);
        } else if in_group {
            (rmask, wmask, xmask) = (0o040, 0o020, 0o010);
        }
    }
    Ok(match func {
        "file_exists" => Value::Bool(true),
        "is_file" => Value::Bool(mode & S_IFMT == S_IFREG),
        "is_dir" => Value::Bool(mode & S_IFMT == S_IFDIR),
        "is_link" => Value::Bool(mode & S_IFMT == S_IFLNK),
        "is_readable" => Value::Bool(mode & rmask != 0),
        "is_writable" | "is_writeable" => Value::Bool(mode & wmask != 0),
        "is_executable" => Value::Bool(mode & xmask != 0),
        "fileperms" => Value::Int(mode),
        "fileinode" => Value::Int(s[1]),
        "filesize" => Value::Int(s[7]),
        "fileowner" => Value::Int(s[4]),
        "filegroup" => Value::Int(s[5]),
        "fileatime" => Value::Int(s[8]),
        "filemtime" => Value::Int(s[9]),
        "filectime" => Value::Int(s[10]),
        "filetype" => {
            let t: &[u8] = match mode & S_IFMT {
                S_IFLNK => b"link",
                0o010000 => b"fifo",
                0o020000 => b"char",
                S_IFDIR => b"dir",
                0o060000 => b"block",
                S_IFREG => b"file",
                0o140000 => b"socket",
                other => {
                    ctx.notice(&format!("{func}(): Unknown file type ({other})"))?;
                    b"unknown"
                }
            };
            Value::string(t)
        }
        _ => Value::Array(crate::file2::stat_array(&s)),
    })
}

/// A registered user wrapper takes the url: the hook the path functions
/// in `file.rs`/`file2.rs`/`filestat.rs`/`dir.rs` make first. `None` when
/// the path is the native code's.
pub(crate) fn path_op(ctx: &mut Ctx, func: &str, args: &mut [Value]) -> Result<Option<Value>, Unwind> {
    let path = args[0].to_php_bytes().to_vec();
    let opener = matches!(func, "fopen" | "file_get_contents" | "file_put_contents" | "file" | "readfile");
    // A plain path only goes anywhere but the file system once the table
    // has changed; and `file_put_contents()` refuses `LOCK_EX` on a url
    // before it looks for a wrapper.
    if func == "file_put_contents" && scheme_len(&path).is_some() {
        let flags = args.get(2).map_or(0, Value::to_int);
        let file_url = path.len() >= 7 && path[..7].eq_ignore_ascii_case(b"file://");
        if flags & 8 == 0 && flags & 2 != 0 && !file_url && route_of_quiet(ctx, &path) {
            ctx.warn("file_put_contents(): Exclusive locks may only be set for regular files")?;
            return Ok(Some(Value::Bool(false)));
        }
    }
    let Some(route) = route_of(ctx, &path, opener, func)? else {
        return Ok(None);
    };
    match func {
        "fopen" => fopen(ctx, route, args, &path).map(Some),
        "file_get_contents" => file_get_contents(ctx, route, args).map(Some),
        "file_put_contents" => file_put_contents(ctx, route, args, &path).map(Some),
        "file" => file(ctx, route, args).map(Some),
        "readfile" => match open_for(ctx, route, args, func, "rb", 1, 2)? {
            Ok(r) => {
                let v = handle_op(ctx, "readfile", &mut [Value::Resource(r.clone())])?;
                close(ctx, &r)?;
                Ok(v)
            }
            Err(v) => Ok(Some(v)),
        },
        "unlink" | "rmdir" | "mkdir" => {
            let Route::User(w, _) = route else {
                if func == "unlink" {
                    ctx.warn("unlink(): Unable to locate stream wrapper")?;
                }
                return Ok(Some(Value::Bool(false)));
            };
            let at = match func {
                "unlink" => 1,
                "rmdir" => 1,
                _ => 3,
            };
            let context = context_arg(ctx, args.get(at))?;
            let call_args = match func {
                "unlink" => vec![Value::string(&path)],
                "rmdir" => vec![Value::string(&path), Value::Int(STREAM_REPORT_ERRORS)],
                _ => {
                    let perms = args.get(1).map_or(0o777, Value::to_int);
                    let recursive = args.get(2).is_some_and(Value::to_bool);
                    let options = STREAM_REPORT_ERRORS | if recursive { STREAM_MKDIR_RECURSIVE } else { 0 };
                    vec![Value::string(&path), Value::Int(perms), Value::Int(options)]
                }
            };
            bool_op(ctx, &w, context, func, &call_args, func).map(Some)
        }
        "stat" | "lstat" | "file_exists" | "is_file" | "is_dir" | "is_link" | "is_readable" | "is_writable"
        | "is_writeable" | "is_executable" | "filesize" | "filemtime" | "fileatime" | "filectime" | "filetype"
        | "fileperms" | "fileinode" | "fileowner" | "filegroup" => {
            match route {
                Route::User(w, p) => php_stat(ctx, &w, &p, &path, func).map(Some),
                _ => {
                    let exists = matches!(func, "file_exists" | "is_file" | "is_dir" | "is_link" | "is_readable" | "is_writable" | "is_writeable" | "is_executable");
                    if !exists {
                        let l = if matches!(func, "filetype" | "lstat") { "L" } else { "" };
                        ctx.warn(&format!("{func}(): {l}stat failed for {}", String::from_utf8_lossy(&path)))?;
                    }
                    Ok(Some(Value::Bool(false)))
                }
            }
        }
        "touch" | "chmod" | "chown" | "chgrp" => metadata(ctx, route, args, &path, func).map(Some),
        "stream_is_local" => Ok(Some(Value::Bool(!matches!(&route, Route::User(w, _) if w.is_url)))),
        _ => Ok(None),
    }
}

/// Whether a url goes to a user wrapper, asked without a word.
fn route_of_quiet(ctx: &mut Ctx, path: &[u8]) -> bool {
    ctx.silence += 1;
    let r = locate(ctx, path, false, false, "");
    ctx.silence -= 1;
    matches!(r, Ok(Route::User(..)))
}

/// `fopen()` through a user wrapper.
fn fopen(ctx: &mut Ctx, route: Route, args: &[Value], path: &[u8]) -> NativeResult {
    const FUNC: &str = "fopen";
    let mode = String::from_utf8_lossy(&args[1].to_php_bytes()).into_owned();
    let use_path = args.get(2).is_some_and(Value::to_bool);
    let Some((w, p)) = user_route(ctx, route, path, FUNC)? else {
        return Ok(Value::Bool(false));
    };
    let context = context_arg(ctx, args.get(3))?;
    let options = if use_path { STREAM_USE_PATH } else { 0 } | STREAM_REPORT_ERRORS;
    Ok(match open(ctx, &w, &p, path, &mode, options, context, FUNC)? {
        Some((res, _)) => res,
        None => Value::Bool(false),
    })
}

/// `file_get_contents()` through a user wrapper.
fn file_get_contents(ctx: &mut Ctx, route: Route, args: &[Value]) -> NativeResult {
    const FUNC: &str = "file_get_contents";
    let max = match args.get(4).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) if v.to_int() < 0 => {
            return Err(Unwind::value_error("file_get_contents(): Argument #5 ($length) must be greater than or equal to 0"))
        }
        Some(v) => Some(v.to_int() as usize),
    };
    let offset = args.get(3).map_or(0, Value::to_int);
    let r = match open_for(ctx, route, args, FUNC, "rb", 1, 2)? {
        Ok(r) => r,
        Err(v) => return Ok(v),
    };
    if offset != 0 && seek(ctx, &r, offset, if offset > 0 { 0 } else { 2 }, FUNC)? < 0 {
        ctx.warn(&format!("{FUNC}(): Failed to seek to position {offset} in the stream"))?;
        close(ctx, &r)?;
        return Ok(Value::Bool(false));
    }
    let data = copy_to_mem(ctx, &r, max, FUNC)?;
    close(ctx, &r)?;
    Ok(Value::Str(Str::from_vec(data)))
}

/// `file()` through a user wrapper.
fn file(ctx: &mut Ctx, route: Route, args: &[Value]) -> NativeResult {
    const FUNC: &str = "file";
    let flags = args.get(1).map_or(0, Value::to_int);
    if !(0..=(1 | 2 | 4 | 16)).contains(&flags) {
        return Err(Unwind::value_error("file(): Argument #2 ($flags) must be a valid flag value"));
    }
    let r = match open_for(ctx, route, args, FUNC, "rb", 1, 2)? {
        Ok(r) => r,
        Err(v) => return Ok(v),
    };
    let data = copy_to_mem(ctx, &r, None, FUNC)?;
    close(ctx, &r)?;
    let mut a = Array::new();
    for line in crate::file::split_lines(&data, flags) {
        a.push(Value::Str(Str::from_vec(line)));
    }
    Ok(Value::Array(a))
}

/// `file_put_contents()` through a user wrapper.
fn file_put_contents(ctx: &mut Ctx, route: Route, args: &[Value], path: &[u8]) -> NativeResult {
    const FUNC: &str = "file_put_contents";
    const LOCK_EX: i64 = 2;
    const FILE_APPEND: i64 = 8;
    let flags = args.get(2).map_or(0, Value::to_int);
    let mut mode = if flags & FILE_APPEND != 0 { "ab" } else { "wb" };
    if flags & FILE_APPEND == 0 && flags & LOCK_EX != 0 {
        mode = "cb";
    }
    let r = match open_for(ctx, route, args, FUNC, mode, 2, 3)? {
        Ok(r) => r,
        Err(v) => return Ok(v),
    };
    if flags & LOCK_EX != 0 && (op_lock(ctx, &r, 0, FUNC)? != 0 || op_lock(ctx, &r, 2, FUNC)? != 0) {
        close(ctx, &r)?;
        ctx.warn(&format!("{FUNC}(): Exclusive locks are not supported for this stream"))?;
        return Ok(Value::Bool(false));
    }
    if mode == "cb" {
        let (obj, _) = obj_of(&r);
        if call_exists(ctx, &obj, "stream_truncate") {
            op_truncate(ctx, &r, 0, FUNC)?;
        }
    }
    let data = args[1].deref().into_owned();
    let mut total: i64 = 0;
    match &data {
        Value::Array(a) => {
            for (_, v) in a.iter() {
                let s = ctx.to_string(v)?.as_bytes().to_vec();
                if s.is_empty() {
                    continue;
                }
                total += s.len() as i64;
                let n = write(ctx, &r, &s, FUNC)?;
                if n != s.len() as i64 {
                    ctx.warn(&format!("{FUNC}(): Failed to write {} bytes to {}", s.len(), String::from_utf8_lossy(path)))?;
                    total = -1;
                    break;
                }
            }
        }
        Value::Resource(_) => {
            let s = ctx.call_function(b"stream_get_contents", std::slice::from_ref(&data))?.to_php_bytes().to_vec();
            total = write(ctx, &r, &s, FUNC)?;
        }
        Value::Closure(_) => total = -1,
        other => {
            let s = ctx.to_string(other)?.as_bytes().to_vec();
            if !s.is_empty() {
                total = write(ctx, &r, &s, FUNC)?;
                if total != -1 && total != s.len() as i64 {
                    ctx.warn(&format!(
                        "{FUNC}(): Only {total} of {} bytes written, possibly out of free disk space",
                        s.len()
                    ))?;
                    total = -1;
                }
            }
        }
    }
    close(ctx, &r)?;
    Ok(if total < 0 { Value::Bool(false) } else { Value::Int(total) })
}

/// `touch()`/`chmod()`/`chown()`/`chgrp()`: `stream_metadata()`.
fn metadata(ctx: &mut Ctx, route: Route, args: &[Value], path: &[u8], func: &str) -> NativeResult {
    let (option, value) = match func {
        "touch" => {
            let some = |i: usize| args.get(i).map(|v| v.deref().into_owned()).filter(|v| !matches!(v, Value::Null | Value::Uninit)).map(|v| v.to_int());
            let (mtime, atime) = (some(1), some(2));
            let mut a = Array::new();
            match (mtime, atime) {
                (None, None) => {}
                (Some(m), None) => {
                    a.push(Value::Int(m));
                    a.push(Value::Int(m));
                }
                (None, Some(_)) => {
                    return Err(Unwind::value_error(
                        "touch(): Argument #2 ($mtime) cannot be null when argument #3 ($atime) is an integer",
                    ))
                }
                (Some(m), Some(at)) => {
                    a.push(Value::Int(m));
                    a.push(Value::Int(at));
                }
            }
            (META_TOUCH, Value::Array(a))
        }
        "chmod" => (META_ACCESS, Value::Int(args[1].to_int())),
        _ => {
            let user = func == "chown";
            match &*args[1].deref() {
                Value::Int(n) => (if user { META_OWNER } else { META_GROUP }, Value::Int(*n)),
                other => (if user { META_OWNER_NAME } else { META_GROUP_NAME }, Value::string(&other.to_php_bytes())),
            }
        }
    };
    let Route::User(w, _) = route else {
        return Ok(Value::Bool(false));
    };
    bool_op(ctx, &w, Value::Null, "stream_metadata", &[Value::string(path), Value::Int(option), value], func)
}

/// `rename()` when either end is a user url.
pub(crate) fn rename(ctx: &mut Ctx, args: &mut [Value]) -> Result<Option<Value>, Unwind> {
    const FUNC: &str = "rename";
    let from = args[0].to_php_bytes().to_vec();
    let to = args[1].to_php_bytes().to_vec();
    let src = route_of(ctx, &from, false, FUNC)?;
    let dst = route_of(ctx, &to, false, FUNC)?;
    if !matches!(src, Some(Route::User(..))) && !matches!(dst, Some(Route::User(..))) {
        return Ok(None);
    }
    let w = match src {
        Some(Route::User(w, _)) => w,
        None => {
            ctx.warn(&format!("{FUNC}(): Cannot rename a file across wrapper types"))?;
            return Ok(Some(Value::Bool(false)));
        }
        Some(_) => {
            ctx.warn(&format!("{FUNC}(): Unable to locate stream wrapper"))?;
            return Ok(Some(Value::Bool(false)));
        }
    };
    if !matches!(&dst, Some(Route::User(w2, _)) if Rc::ptr_eq(&w, w2)) {
        ctx.warn(&format!("{FUNC}(): Cannot rename a file across wrapper types"))?;
        return Ok(Some(Value::Bool(false)));
    }
    let context = context_arg(ctx, args.get(2))?;
    bool_op(ctx, &w, context, "rename", &[Value::string(&from), Value::string(&to)], FUNC).map(Some)
}

/// `copy()` when either end is a user url (php's `php_copy_file_ctx`).
pub(crate) fn copy(ctx: &mut Ctx, args: &mut [Value]) -> Result<Option<Value>, Unwind> {
    const FUNC: &str = "copy";
    let from = args[0].to_php_bytes().to_vec();
    let to = args[1].to_php_bytes().to_vec();
    let src = route_of(ctx, &from, false, FUNC)?;
    let dst = route_of(ctx, &to, false, FUNC)?;
    if !matches!(src, Some(Route::User(..))) && !matches!(dst, Some(Route::User(..))) {
        return Ok(None);
    }
    // The source's stat (not quiet) and the destination's (quiet): a
    // directory at either end refuses.
    let stat_of = |ctx: &mut Ctx, route: &Option<Route>, p: &[u8], flags: i64| -> Result<Option<[i64; 13]>, Unwind> {
        match route {
            Some(Route::User(w, open)) => url_stat(ctx, w, open, flags, FUNC),
            Some(_) => Ok(None),
            None => {
                let v = Value::string(p);
                let path = crate::filestat::arg_path(ctx, &v);
                Ok(std::fs::metadata(path).ok().map(|m| {
                    use std::os::unix::fs::MetadataExt;
                    let mut s = [0i64; 13];
                    s[2] = i64::from(m.mode());
                    s[1] = m.ino() as i64;
                    s[0] = m.dev() as i64;
                    s
                }))
            }
        }
    };
    if let Some(s) = stat_of(ctx, &src, &from, 0)? {
        if s[2] & S_IFMT == S_IFDIR {
            ctx.warn(&format!("{FUNC}(): The first argument to copy() function cannot be a directory"))?;
            return Ok(Some(Value::Bool(false)));
        }
        if let Some(d) = stat_of(ctx, &dst, &to, STREAM_URL_STAT_QUIET)? {
            if d[2] & S_IFMT == S_IFDIR {
                ctx.warn(&format!("{FUNC}(): The second argument to copy() function cannot be a directory"))?;
                return Ok(Some(Value::Bool(false)));
            }
        }
    }
    let context = args.get(2).cloned();
    let Some(src) = open_any(ctx, src, &from, "rb", context.as_ref(), FUNC)? else {
        return Ok(Some(Value::Bool(false)));
    };
    let Some(dst) = open_any(ctx, dst, &to, "wb", context.as_ref(), FUNC)? else {
        ctx.call_function(b"fclose", &[src])?;
        return Ok(Some(Value::Bool(false)));
    };
    let copied = ctx.call_function(b"stream_copy_to_stream", &[src.clone(), dst.clone()])?;
    ctx.call_function(b"fclose", &[src])?;
    ctx.call_function(b"fclose", &[dst])?;
    Ok(Some(Value::Bool(!matches!(copied, Value::Bool(false)))))
}

/// Open either kind of path for `copy()`, its failures reported under
/// `func`'s name.
fn open_any(ctx: &mut Ctx, route: Option<Route>, path: &[u8], mode: &str, context: Option<&Value>, func: &str) -> Result<Option<Value>, Unwind> {
    match route {
        None => {
            ctx.silence += 1;
            let r = ctx.call_function(b"fopen", &[Value::string(path), Value::string(mode.as_bytes())]);
            ctx.silence -= 1;
            let r = r?;
            if matches!(r, Value::Resource(_)) {
                return Ok(Some(r));
            }
            fail_open(ctx, func, path, "No such file or directory")?;
            Ok(None)
        }
        Some(route) => {
            let Some((w, p)) = user_route(ctx, route, path, func)? else { return Ok(None) };
            let context = context_arg(ctx, context)?;
            Ok(open(ctx, &w, &p, path, mode, STREAM_REPORT_ERRORS, context, func)?.map(|(r, _)| r))
        }
    }
}

// ---- directories -----------------------------------------------------------------------

/// `opendir()`/`dir()`/`scandir()` of a user url: `dir_opendir()`. `None`
/// when the path is the native code's; `Some(None)` after php's warning.
pub(crate) fn open_dir(ctx: &mut Ctx, func: &str, raw: &Value, context: Option<&Value>) -> Result<Option<Option<Value>>, Unwind> {
    let path = raw.to_php_bytes().to_vec();
    let Some(route) = route_of(ctx, &path, true, func)? else { return Ok(None) };
    let (w, p) = match route {
        Route::User(w, p) => (w, p),
        _ => {
            let shown = String::from_utf8_lossy(&path);
            ctx.warn(&format!("{func}({shown}): Failed to open directory: no suitable wrapper could be found"))?;
            return Ok(Some(None));
        }
    };
    let context = match func {
        "scandir" => match context.map(|v| v.deref().into_owned()) {
            Some(v @ Value::Resource(_)) => v,
            _ => Value::Null,
        },
        _ => context_arg(ctx, context)?,
    };
    let obj = create_object(ctx, &w, context)?;
    let why = match call(ctx, &obj, "dir_opendir", &[Value::string(&p), Value::Int(0)])? {
        Some(v) if v.to_bool() => None,
        Some(_) => Some("call failed"),
        None => Some("is not implemented"),
    };
    if let Some(why) = why {
        drop(obj);
        ctx.run_pending_destructors()?;
        let shown = String::from_utf8_lossy(&path);
        ctx.warn(&format!(
            "{func}({shown}): Failed to open directory: \"{}::dir_opendir\" {why}",
            w.class_name
        ))?;
        return Ok(Some(None));
    }
    let stream = UserStream::new(obj, w, "r", None, true);
    Ok(Some(Some(ctx.resources.add("stream", Box::new(stream)))))
}

/// `readdir()`/`rewinddir()`/`closedir()` (and `Directory`'s methods) on a
/// user directory stream. `None` when `r` is not one.
pub(crate) fn dir_op(ctx: &mut Ctx, func: &str, r: &Resource) -> Result<Option<Value>, Unwind> {
    if !is_user_dir(r) {
        return Ok(None);
    }
    let (obj, w) = obj_of(r);
    Ok(Some(match func {
        "readdir" => match call(ctx, &obj, "dir_readdir", &[])? {
            None => {
                ctx.warn(&format!("{func}(): {}::dir_readdir is not implemented!", w.class_name))?;
                Value::Bool(false)
            }
            Some(Value::Bool(_)) => Value::Bool(false),
            Some(v) => Value::Str(ctx.to_string(&v)?),
        },
        "rewinddir" => {
            call(ctx, &obj, "dir_rewinddir", &[])?;
            Value::Null
        }
        _ => {
            drop(obj);
            close(ctx, r)?;
            Value::Null
        }
    }))
}

/// `scandir()` of a user url: every entry `dir_readdir()` gives, sorted.
pub(crate) fn scandir(ctx: &mut Ctx, args: &mut [Value]) -> Result<Option<Value>, Unwind> {
    const FUNC: &str = "scandir";
    let Some(h) = open_dir(ctx, FUNC, &args[0], args.get(2))? else { return Ok(None) };
    let Some(h) = h else {
        ctx.warn(&format!("{FUNC}(): (errno 0): Undefined error: 0"))?;
        return Ok(Some(Value::Bool(false)));
    };
    let r = user_res(&h).expect("just made");
    let mut names: Vec<Vec<u8>> = Vec::new();
    while let Some(Value::Str(s)) = dir_op(ctx, "readdir", &r)? {
        names.push(s.as_bytes().to_vec());
    }
    close(ctx, &r)?;
    match args.get(1).map_or(0, Value::to_int) {
        0 => names.sort(),
        2 => {}
        _ => {
            names.sort();
            names.reverse();
        }
    }
    let mut a = Array::new();
    for n in names {
        a.push(Value::Str(Str::from_vec(n)));
    }
    Ok(Some(Value::Array(a)))
}

// ---- include ---------------------------------------------------------------------------

/// The runtime's [`IncludeStreamHook`](rphp_runtime::IncludeStreamHook):
/// an `include` of a url a class serves opens it through the wrapper
/// (`rb`, php's include options), turns its read buffer off, asks its size
/// and reads the unit — or, for an `_once` of a unit already included,
/// just opens and closes it, as php does.
fn include_hook(it: &mut Interp, path: &[u8], keyword: &str, once: bool) -> Option<Result<IncludeOpen, Unwind>> {
    let mut ctx = Ctx(it);
    if scheme_len(path).is_none() && !touched(&ctx) {
        return None;
    }
    include_through(&mut ctx, path, keyword, once)
}

fn include_through(ctx: &mut Ctx, path: &[u8], keyword: &str, once: bool) -> Option<Result<IncludeOpen, Unwind>> {
    let route = match locate(ctx, path, true, true, keyword) {
        Ok(r) => r,
        Err(u) => return Some(Err(u)),
    };
    let (w, p) = match route {
        Route::User(w, p) => (w, p),
        Route::Native | Route::Missing => return None,
        Route::Fail => {
            return Some(fail_open(ctx, keyword, path, "no suitable wrapper could be found").map(|()| IncludeOpen::Failed))
        }
    };
    Some(read_unit(ctx, &w, &p, path, keyword, once))
}

fn read_unit(ctx: &mut Ctx, w: &Rc<UserWrapper>, p: &[u8], path: &[u8], keyword: &str, once: bool) -> Result<IncludeOpen, Unwind> {
    let options = STREAM_USE_PATH | STREAM_REPORT_ERRORS | STREAM_OPEN_FOR_INCLUDE | STREAM_OPEN_FOR_ZEND_STREAM;
    let Some((res, opened)) = open(ctx, w, p, path, "rb", options, Value::Null, keyword)? else {
        return Ok(IncludeOpen::Failed);
    };
    let r = user_res(&res).expect("just made");
    op_set_option(ctx, &r, OPTION_READ_BUFFER, Value::Int(BUFFER_NONE), Value::Int(BUFSIZ), keyword)?;
    let name = String::from_utf8_lossy(opened.as_deref().unwrap_or(path)).into_owned();
    if once && ctx.is_included(&name) {
        close(ctx, &r)?;
        return Ok(IncludeOpen::Included);
    }
    let size = op_stat(ctx, &r, keyword)?.map_or(0, |s| s[7].max(0) as usize);
    let mut bytes = Vec::new();
    if size > 0 {
        while bytes.len() < size {
            match read(ctx, &r, size - bytes.len(), keyword)? {
                Some(b) if !b.is_empty() => bytes.extend_from_slice(&b),
                _ => break,
            }
        }
    } else {
        let mut remain = 4096usize;
        while let Some(b) = read(ctx, &r, remain, keyword)? {
            if b.is_empty() {
                break;
            }
            bytes.extend_from_slice(&b);
            remain -= b.len();
            if remain == 0 {
                remain = bytes.len();
            }
        }
    }
    close(ctx, &r)?;
    Ok(IncludeOpen::Source { name, bytes })
}

// ---- popen -----------------------------------------------------------------------------

/// The children `popen()` started, by the id of their stream.
#[derive(Default)]
struct Children(Vec<(u32, std::process::Child)>);

const CHILDREN_SLOT: &str = "stream_wrappers.popen";

/// `popen(string $command, string $mode): resource|false`
fn popen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "popen";
    let command = str_arg(&args[0], FUNC, 1, "command")?;
    let mode = str_arg(&args[1], FUNC, 2, "mode")?;
    if command.contains(&0) {
        return Err(Unwind::value_error("popen(): Argument #1 ($command) must not contain any null bytes"));
    }
    let invalid = mode.len() > 2
        || (mode.len() == 1 && mode[0] != b'r' && mode[0] != b'w')
        || (mode.len() == 2 && mode != b"rb" && mode != b"wb");
    if invalid {
        return Err(Unwind::value_error("popen(): Argument #2 ($mode) must be one of \"r\", \"rb\", \"w\", or \"wb\""));
    }
    let shown = String::from_utf8_lossy(&command).into_owned();
    if mode.is_empty() {
        ctx.warn(&format!("{FUNC}({shown},): Invalid argument"))?;
        return Ok(Value::Bool(false));
    }
    let reading = mode[0] == b'r';
    let argv = crate::exec::shell_argv(&command);
    if !crate::exec::permitted(ctx, &crate::exec::SpawnRequest { function: FUNC, argv: &argv, cwd: None })? {
        return Ok(Value::Bool(false));
    }
    let mut c = crate::exec::command_for(&argv);
    c.current_dir(&ctx.cwd);
    if reading {
        c.stdout(std::process::Stdio::piped());
    } else {
        c.stdin(std::process::Stdio::piped());
    }
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let m = String::from_utf8_lossy(&mode);
            ctx.warn(&format!("{FUNC}({shown},{m}): {}", crate::filestat::io_text(&e)))?;
            return Ok(Value::Bool(false));
        }
    };
    let fd: std::os::fd::OwnedFd = if reading {
        child.stdout.take().expect("piped").into()
    } else {
        child.stdin.take().expect("piped").into()
    };
    let res = crate::file::popen_resource(ctx, std::fs::File::from(fd), reading, &String::from_utf8_lossy(&mode));
    if let Value::Resource(r) = &res {
        ctx.ext.slot::<Children>(CHILDREN_SLOT).0.push((r.id(), child));
    }
    Ok(res)
}

/// The child behind a `popen()` stream, taken out of the table.
fn take_child(ctx: &mut Ctx, v: &Value) -> Option<std::process::Child> {
    let Value::Resource(r) = &*v.deref() else { return None };
    let id = r.id();
    let kids = ctx.ext.slots.get_mut(CHILDREN_SLOT)?.downcast_mut::<Children>()?;
    let at = kids.0.iter().position(|(i, _)| *i == id)?;
    Some(kids.0.remove(at).1)
}

/// php's `pclose(3)` answer: the exit code, or the raw wait status of a
/// child a signal ended.
fn wait_status(mut child: std::process::Child) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    match child.wait() {
        Ok(s) => s.code().map_or_else(|| i64::from(s.into_raw()), i64::from),
        Err(_) => -1,
    }
}

/// `fclose()` of a `popen()` stream waits for the child, as php's does.
fn close_popen(ctx: &mut Ctx, v: &Value) -> Result<Option<Value>, Unwind> {
    let Some(child) = take_child(ctx, v) else { return Ok(None) };
    let r = ctx.call_function(b"fclose", std::slice::from_ref(v))?;
    wait_status(child);
    Ok(Some(r))
}

/// `pclose(resource $handle): int`
fn pclose(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "pclose";
    match &*args[0].deref() {
        Value::Resource(r) if r.kind() == "stream" => {}
        Value::Resource(_) => {
            return Err(Unwind::type_error(format!("{FUNC}(): Argument #1 ($handle) must be an open stream resource")))
        }
        other => {
            return Err(Unwind::type_error(format!(
                "{FUNC}(): Argument #1 ($handle) must be of type resource, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    }
    let child = take_child(ctx, &args[0]);
    ctx.call_function(b"fclose", &[args[0].clone()])?;
    Ok(Value::Int(child.map_or(0, wait_status)))
}

// ---- the end of the request ------------------------------------------------------------

/// php closes every stream when the request ends: a user stream gets its
/// owed flush and `stream_close()` (`dir_closedir()`), a `popen()` child
/// its end of the pipe closed and is waited for. Runs from
/// `file::request_shutdown`.
pub(crate) fn request_shutdown(ctx: &mut Ctx) {
    let open: Vec<Resource> = ctx
        .resources
        .iter()
        .filter(|(_, r)| r.kind() == "stream" && r.payload().as_ref().is_some_and(|p| p.is::<UserStream>()))
        .map(|(_, r)| r.clone())
        .collect();
    for r in open {
        if let Err(u) = close(ctx, &r) {
            ctx.handle_top_level_unwind(u);
        }
    }
    let kids = ctx.ext.slots.get_mut(CHILDREN_SLOT).and_then(|b| b.downcast_mut::<Children>()).map(|k| std::mem::take(&mut k.0));
    for (id, child) in kids.unwrap_or_default() {
        ctx.resources.close(id);
        wait_status(child);
    }
}
