//! `SplFileObject` and `SplTempFileObject` (php-src
//! `ext/spl/spl_directory.c`): an `SplFileInfo` with an open handle and a
//! one-line read cache.
//!
//! # Reaching the stream
//!
//! `file.rs` owns `Stream` and keeps its fields private, so — like
//! `file2.rs` before it — everything here reaches the handle by calling the
//! registered natives (`fopen`, `fgets`, `fseek`, `fgetcsv`, …) back
//! through the interpreter. That runs the very code a PHP script would, so
//! the stream model is respected rather than re-implemented. The cost is
//! that a diagnostic raised *inside* one of those calls would name the
//! inner function, so every method that can fail on the handle first
//! re-derives readability and writability from the mode the handle reports
//! — exactly as `fopen` derives them — and raises php's notice under its
//! own name.
//!
//! # The one-line cache, and why `key()` moves the way it does
//!
//! php keeps `current_line`, `current_zval` (the parsed CSV row) and
//! `current_line_num`. Every read frees the cache, refills it, and then
//! adds to the line number — **by one if the cache had held something, by
//! zero if it was empty**. `fgets()` is the exception: it always adds one.
//! That single rule explains the whole family of observations:
//!
//! ```text
//! $f = new SplFileObject($p);   // key() === 0
//! $f->current();                // reads line 0, key() still 0
//! $f->next();                   // key() === 1
//! $g = new SplFileObject($p);
//! $g->getCurrentLine();         // reads line 0, key() === 1
//! ```
//!
//! and it is why `seek(100)` on a five-line file leaves `key()` at 4.
//!
//! `SKIP_EMPTY` retries around a line of **length zero** — so it drops a
//! blank line only together with `DROP_NEW_LINE`, because `"\n"` on its own
//! is one byte long and php's emptiness test is a length test.
//!
//! # Deliberately absent (ADR-004)
//!
//! * `ftruncate()`. It has to shorten the `Stream`'s buffer, and no
//!   registered native exposes that — the same blocker `file2.rs` records
//!   for the procedural `ftruncate()`. Registering it would mean answering
//!   `true` and changing nothing, so it is left out.
//! * php's "overloaded `getCurrentLine`" path: when a subclass overrides
//!   `getCurrentLine()` or `current()`, php's internal line reader calls the
//!   override instead of reading the stream (`func_getCurr->common.scope !=
//!   spl_ce_SplFileObject`). Here the reader always reads the stream, so a
//!   subclass's override is visible to callers but not to `foreach`.
//! * `$context` is accepted and ignored: the engine has no stream contexts.
//! * `READ_CSV` parses the one line it read with `str_getcsv()`. php hands
//!   `php_fgetcsv()` the *stream* as well, so a field that carries a
//!   newline inside its enclosure keeps reading; here such a record is cut
//!   at the line break.
//! * `fputcsv()`, `fscanf()`, `fpassthru()`, `flock()` and `fstat()` are
//!   forwarded whole to the procedural native, so a notice raised inside
//!   one — the `Write of N bytes failed with errno=9` an unwritable handle
//!   produces — names `fputcsv()` where php names
//!   `SplFileObject::fputcsv()`. The reads and `fwrite()`, whose byte
//!   counts are known here, raise php's notice under their own name.
//! * The modes that can fail before a byte moves (`r` on a missing file,
//!   `x` on one that exists) are pre-checked so `fopen`'s own warning never
//!   leaks; what `file.rs`'s `fopen` does *not* do — fail a `w`/`a`/`c`
//!   open under a missing directory, honour `x`'s `O_EXCL` atomically — it
//!   still does not do from here.
//! * A write is flushed to disk immediately rather than at close, because
//!   the object has no destructor hook here; the file therefore always
//!   holds what has been written, which is what php's buffered handle
//!   settles on anyway.

use rphp_runtime::{
    nm, Ctx, NativeMethod, NativeMethodHandler, NativeResult, Registry, Unwind, Visibility,
};
use rphp_value::{Object, Str, Value};

use super::common::{
    as_path, debug_array, int_arg, set_file_name, str_arg, sync, this, with_fs, File, Fs, Kind,
};
use super::iterators::no_clone;
use crate::filestat::{invalidate, io_text};

/// `SplFileObject::DROP_NEW_LINE`
const DROP_NEW_LINE: i64 = 1;
/// `SplFileObject::READ_AHEAD`
const READ_AHEAD: i64 = 2;
/// `SplFileObject::SKIP_EMPTY`
const SKIP_EMPTY: i64 = 4;
/// `SplFileObject::READ_CSV`
const READ_CSV: i64 = 8;
/// The bits `getFlags()` reports.
const FILE_FLAGS_MASK: i64 = DROP_NEW_LINE | READ_AHEAD | SKIP_EMPTY | READ_CSV;

/// A method descriptor with by-reference parameters, which the `nm!` macro
/// cannot express. `flock`'s `$wouldBlock` and `fscanf`'s `$vars` are the
/// only two in this module.
const fn nm_ref(
    min: u8,
    max: Option<u8>,
    by_ref: u32,
    handler: NativeMethodHandler,
) -> NativeMethod {
    NativeMethod {
        handler,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref,
        is_static: false,
        is_final: false,
    }
}

// ---- opening ---------------------------------------------------------------

/// Whether the name addresses a `php://` wrapper rather than the real
/// filesystem.
fn is_php_wrapper(name: &[u8]) -> bool {
    name.len() >= 6 && name[..6].eq_ignore_ascii_case(b"php://")
}

/// Whether the `php://` name is one of the two that carry a buffer, as
/// against `stdout`/`stderr`/`output`, which have nothing to read back.
/// `file.rs`'s `fopen` makes the same split.
fn is_php_buffer(name: &[u8]) -> bool {
    let rest = &name[6..];
    !(rest.eq_ignore_ascii_case(b"stdout")
        || rest.eq_ignore_ascii_case(b"stderr")
        || rest.eq_ignore_ascii_case(b"output"))
}

/// Whether a read can reach the handle, derived from the mode exactly as
/// `fopen` derives it: a `php://` buffer ignores the mode, which is why an
/// `SplTempFileObject` opened `wb` still reads back.
fn readable(name: &[u8], mode: &[u8]) -> bool {
    if is_php_wrapper(name) {
        is_php_buffer(name)
    } else {
        mode.contains(&b'r') || mode.contains(&b'+')
    }
}

/// Whether a write can reach the handle.
fn writable(name: &[u8], mode: &[u8]) -> bool {
    if is_php_wrapper(name) && !is_php_buffer(name) {
        return true;
    }
    mode.iter()
        .any(|c| matches!(c, b'w' | b'a' | b'x' | b'c' | b'+'))
}

/// php's `SplFileObject::__construct(/x): Failed to open stream: …`, the
/// shape the SPL error handler gives `php_stream_open_wrapper`'s warning.
fn open_failed(who: &str, name: &[u8], why: &str) -> Unwind {
    Unwind::exception(
        "RuntimeException",
        format!(
            "{who}({}): Failed to open stream: {why}",
            String::from_utf8_lossy(name)
        ),
    )
}

/// Open `name` and install the file state on `o`. `who` names the function
/// the failure is reported under, which is `SplFileInfo::openFile` when the
/// object was spawned from an info object and `SplFileObject::__construct`
/// otherwise — php picks the same two names for the same reason.
pub(super) fn open_into(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    name: &[u8],
    mode: &[u8],
    use_include_path: bool,
) -> Result<(), Unwind> {
    // An empty path never reaches the wrapper layer, and a directory is
    // refused before the stream is opened.
    if name.is_empty() {
        return Err(Unwind::value_error("Path must not be empty"));
    }
    if !is_php_wrapper(name) && std::fs::metadata(as_path(ctx, name)).is_ok_and(|m| m.is_dir()) {
        return Err(Unwind::exception(
            "LogicException",
            "Cannot use SplFileObject with directories",
        ));
    }
    // php parses the mode before it touches the filesystem, and only the
    // first character decides; anything else is not a mode at all.
    if !matches!(mode.first(), Some(b'r' | b'w' | b'a' | b'x' | b'c')) {
        return Err(open_failed(
            who,
            name,
            &format!(
                "`{}' is not a valid mode for fopen",
                String::from_utf8_lossy(mode)
            ),
        ));
    }
    // `fopen` in `file.rs` reports a failed open through the warning
    // channel under its own name, so the two modes that can fail on a
    // missing or already-present file are checked here first.
    if !is_php_wrapper(name) {
        let path = as_path(ctx, name);
        match mode[0] {
            b'r' => {
                if let Err(e) = std::fs::metadata(&path) {
                    return Err(open_failed(who, name, io_text(&e)));
                }
            }
            b'x' => {
                if path.exists() {
                    return Err(open_failed(who, name, "File exists"));
                }
            }
            _ => {}
        }
    }
    let id = ctx.native_by_name(b"fopen").expect("fopen is registered");
    let mut argv = [
        Value::Str(Str::from_vec(name.to_vec())),
        Value::Str(Str::from_vec(mode.to_vec())),
        Value::Bool(use_include_path),
    ];
    let stream = ctx.call_native(id, &mut argv)?;
    if matches!(stream, Value::Bool(false)) {
        return Err(open_failed(who, name, "No such file or directory"));
    }
    let st = with_fs(o, |s| {
        s.kind = Kind::File;
        set_file_name(s, name);
        s.file = Some(File {
            stream,
            open_mode: mode.to_vec(),
            flags: 0,
            max_line_len: 0,
            delimiter: b',',
            enclosure: b'"',
            escape: vec![b'\\'],
            escape_given: false,
            current_line: None,
            current_zval: None,
            line_num: 0,
        });
        s.clone()
    });
    sync(o, &st);
    Ok(())
}

/// Build the `SplFileObject` (or subclass) an `SplFileInfo::openFile()`
/// asks for. A class that declares its own constructor gets it called with
/// php's four arguments; one that inherits php's own is opened inline,
/// which is why a failure is reported as `SplFileInfo::openFile(…)` rather
/// than under the child's constructor.
pub(super) fn spawn(
    ctx: &mut Ctx,
    src: &Fs,
    who: &str,
    name: &[u8],
    mode: &[u8],
    use_include_path: bool,
) -> NativeResult {
    let class = src
        .file_class
        .clone()
        .unwrap_or_else(|| b"SplFileObject".to_vec());
    let cid = ctx.lookup_class(&class)?.ok_or_else(|| {
        Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(&class)
        ))
    })?;
    let obj = ctx.new_object(cid)?;
    let base = ctx.class_by_name(b"SplFileObject").expect("registered");
    let own_ctor = ctx
        .resolve_method(cid, b"__construct")
        .is_some_and(|m| m.decl != base);
    if own_ctor {
        ctx.call_method(
            &obj,
            b"__construct",
            &[
                Value::Str(Str::from_vec(name.to_vec())),
                Value::Str(Str::from_vec(mode.to_vec())),
                Value::Bool(use_include_path),
                Value::Null,
            ],
        )?;
    } else {
        open_into(ctx, &obj, who, name, mode, use_include_path)?;
        with_fs(&obj, |s| {
            s.info_class = src.info_class.clone();
            s.file_class = src.file_class.clone();
        });
    }
    Ok(Value::Object(obj))
}

// ---- reaching the handle ---------------------------------------------------

/// The state a handle operation needs, read out of the payload before
/// anything re-enters the VM.
struct Handle {
    stream: Value,
    name: Vec<u8>,
    mode: Vec<u8>,
}

/// The open handle, or php's `Error` for an `SplFileObject` whose
/// constructor never ran.
fn handle(o: &Object) -> Result<Handle, Unwind> {
    with_fs(o, |s| match (&s.file, &s.file_name) {
        (Some(f), name) => Ok(Handle {
            stream: f.stream.clone(),
            name: name.clone().unwrap_or_default(),
            mode: f.open_mode.clone(),
        }),
        _ => Err(Unwind::error("Object not initialized")),
    })
}

/// Call a registered native with the handle in slot 0.
fn on_stream(ctx: &mut Ctx, h: &Handle, func: &[u8], rest: &[Value]) -> NativeResult {
    let id = ctx
        .native_by_name(func)
        .unwrap_or_else(|| panic!("{} is registered", String::from_utf8_lossy(func)));
    let mut argv: Vec<Value> = Vec::with_capacity(rest.len() + 1);
    argv.push(h.stream.clone());
    argv.extend_from_slice(rest);
    ctx.call_native(id, &mut argv)
}

/// php's notice for a read that cannot reach the descriptor, raised under
/// this class's own method name.
fn deny_read(ctx: &mut Ctx, method: &str) -> Result<(), Unwind> {
    ctx.notice(&format!(
        "SplFileObject::{method}(): Read of 8192 bytes failed with errno=9 Bad file descriptor"
    ))
}

/// The same for a write, which names the number of bytes it tried.
fn deny_write(ctx: &mut Ctx, method: &str, len: usize) -> Result<(), Unwind> {
    ctx.notice(&format!(
        "SplFileObject::{method}(): Write of {len} bytes failed with errno=9 Bad file descriptor"
    ))
}

/// `feof()` on the handle.
fn at_eof(ctx: &mut Ctx, h: &Handle) -> Result<bool, Unwind> {
    Ok(on_stream(ctx, h, b"feof", &[])?.to_bool())
}

/// One line off the handle, honouring `setMaxLineLen()`. `file.rs`'s
/// `fgets()` ignores its `$length`, so a bounded read takes the whole line
/// and seeks the remainder back — which also clears the end-of-file flag
/// the over-long read set, as a real bounded read would leave it.
fn raw_line(ctx: &mut Ctx, h: &Handle, max: i64) -> Result<Option<Vec<u8>>, Unwind> {
    let before = if max > 0 {
        on_stream(ctx, h, b"ftell", &[])?.to_int()
    } else {
        0
    };
    match on_stream(ctx, h, b"fgets", &[])? {
        Value::Bool(false) => Ok(None),
        v => {
            let mut bytes = v.to_php_bytes();
            if max > 0 && bytes.len() as i64 > max {
                bytes.truncate(max as usize);
                on_stream(ctx, h, b"fseek", &[Value::Int(before + max), Value::Int(0)])?;
            }
            Ok(Some(bytes))
        }
    }
}

// ---- php's line cache ------------------------------------------------------

/// Read one record into the cache, php's `spl_filesystem_file_read_ex`.
///
/// `line_add` is the caller's: the iteration methods pass "one if the cache
/// already held something", `fgets()` always passes one. A read that comes
/// back with nothing still counts as a record and caches the empty string;
/// only a read attempted *at* end of file fails, and then `silent` decides
/// between `false` and `RuntimeException: Cannot read from file /x`.
fn read_once(ctx: &mut Ctx, o: &Object, silent: bool, line_add: i64) -> Result<bool, Unwind> {
    let h = handle(o)?;
    let (max, drop_nl, csv, delim, enc, esc) = with_fs(o, |s| {
        let f = s.file.as_ref().expect("checked in handle()");
        (
            f.max_line_len,
            f.flags & DROP_NEW_LINE != 0,
            f.flags & READ_CSV != 0,
            f.delimiter,
            f.enclosure,
            f.escape.clone(),
        )
    });
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.current_line = None;
            f.current_zval = None;
        }
    });
    if at_eof(ctx, &h)? {
        if !silent {
            return Err(Unwind::exception(
                "RuntimeException",
                format!("Cannot read from file {}", String::from_utf8_lossy(&h.name)),
            ));
        }
        return Ok(false);
    }
    if !readable(&h.name, &h.mode) {
        deny_read(ctx, "fgets")?;
        with_fs(o, |s| {
            if let Some(f) = &mut s.file {
                f.current_line = Some(Vec::new());
                f.line_num += line_add;
            }
        });
        return Ok(true);
    }
    let mut line = raw_line(ctx, &h, max)?.unwrap_or_default();
    if drop_nl {
        if let Some(i) = line.iter().position(|&b| b == b'\r' || b == b'\n') {
            line.truncate(i);
        }
    }
    // Under `READ_CSV` the same bytes are also parsed into a row, which is
    // what `current()` then answers.
    let row = if csv {
        let id = ctx
            .native_by_name(b"str_getcsv")
            .expect("str_getcsv is registered");
        let mut argv = [
            Value::Str(Str::from_vec(line.clone())),
            Value::Str(Str::from_vec(vec![delim])),
            Value::Str(Str::from_vec(vec![enc])),
            Value::Str(Str::from_vec(esc)),
        ];
        Some(ctx.call_native(id, &mut argv)?)
    } else {
        None
    };
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.current_line = Some(line);
            f.current_zval = row;
            f.line_num += line_add;
        }
    });
    Ok(true)
}

/// php's emptiness test, which is a **length** test: `"\n"` is one byte and
/// therefore not empty, so `SKIP_EMPTY` on its own drops nothing from a
/// text file and only bites together with `DROP_NEW_LINE`.
fn is_empty_record(o: &Object) -> bool {
    with_fs(o, |s| {
        let Some(f) = &s.file else { return true };
        if let Some(l) = &f.current_line {
            return l.is_empty();
        }
        match &f.current_zval {
            None | Some(Value::Null) | Some(Value::Uninit) => true,
            Some(Value::Str(v)) => v.is_empty(),
            Some(Value::Array(a)) => {
                f.flags & READ_CSV == 0
                    && a.len() == 1
                    && a.values()
                        .next()
                        .is_some_and(|v| matches!(&*v.deref(), Value::Str(x) if x.is_empty()))
            }
            Some(_) => false,
        }
    })
}

/// php's `spl_filesystem_file_read_line`: one record, then keep going while
/// `SKIP_EMPTY` is on and the record is empty. Each retry starts from a
/// freed cache, so it adds nothing to the line number.
fn read_record(ctx: &mut Ctx, o: &Object, silent: bool) -> Result<bool, Unwind> {
    let add = with_fs(o, |s| {
        s.file
            .as_ref()
            .is_some_and(|f| f.current_line.is_some() || f.current_zval.is_some())
    });
    let mut ok = read_once(ctx, o, silent, i64::from(add))?;
    let skip_empty = with_fs(o, |s| {
        s.file.as_ref().is_some_and(|f| f.flags & SKIP_EMPTY != 0)
    });
    while skip_empty && ok && is_empty_record(o) {
        with_fs(o, |s| {
            if let Some(f) = &mut s.file {
                f.current_line = None;
                f.current_zval = None;
            }
        });
        ok = read_once(ctx, o, silent, 0)?;
    }
    Ok(ok)
}

// ---- the iterator ----------------------------------------------------------

/// `rewind(): void` — seek to the start, drop the cache, reset the line
/// number, and pre-read when `READ_AHEAD` is on.
fn fo_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    on_stream(ctx, &h, b"rewind", &[])?;
    let ahead = with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.current_line = None;
            f.current_zval = None;
            f.line_num = 0;
            f.flags & READ_AHEAD != 0
        } else {
            false
        }
    });
    if ahead {
        read_record(ctx, o, true)?;
    }
    Ok(Value::Null)
}

/// `valid(): bool` — under `READ_AHEAD` the cache answers, otherwise it is
/// a live "not at end of file".
fn fo_valid(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (ahead, cached) = with_fs(o, |s| {
        let Some(f) = &s.file else {
            return (false, false);
        };
        (
            f.flags & READ_AHEAD != 0,
            f.current_line.is_some() || f.current_zval.is_some(),
        )
    });
    if ahead {
        return Ok(Value::Bool(cached));
    }
    let h = handle(o)?;
    Ok(Value::Bool(!at_eof(ctx, &h)?))
}

/// The cached record as php's `current()` shapes it.
fn cached_current(o: &Object) -> Value {
    with_fs(o, |s| {
        let Some(f) = &s.file else {
            return Value::Bool(false);
        };
        if let Some(l) = &f.current_line {
            if f.flags & READ_CSV == 0 || f.current_zval.is_none() {
                return Value::Str(Str::from_vec(l.clone()));
            }
        }
        match &f.current_zval {
            Some(v) => v.clone(),
            None => Value::Bool(false),
        }
    })
}

/// `current(): string|array|false` — reads on demand, so a plain
/// `foreach` never has to call anything else.
fn fo_current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let empty = with_fs(o, |s| {
        s.file
            .as_ref()
            .is_none_or(|f| f.current_line.is_none() && f.current_zval.is_none())
    });
    if empty {
        read_record(ctx, o, true)?;
    }
    Ok(cached_current(o))
}

/// `key(): int` — php's line number.
fn fo_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_fs(o, |s| {
        s.file.as_ref().map_or(0, |f| f.line_num)
    })))
}

/// `next(): void` — drop the cache, pre-read under `READ_AHEAD`, then bump
/// the line number. The pre-read adds nothing of its own, because the cache
/// it starts from is empty.
fn fo_next(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let ahead = with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.current_line = None;
            f.current_zval = None;
            f.flags & READ_AHEAD != 0
        } else {
            false
        }
    });
    if ahead {
        read_record(ctx, o, true)?;
    }
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.line_num += 1;
        }
    });
    Ok(Value::Null)
}

/// `seek(int $line): void` — rewind and read forward. Running out of lines
/// is not an error: the cursor simply stops at the last one it reached,
/// which is why `seek(100)` on a five-line file leaves `key()` at 4.
fn fo_seek(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let target = int_arg(ctx, "SplFileObject::seek", 1, "line", &args[0])?;
    if target < 0 {
        return Err(Unwind::value_error(
            "SplFileObject::seek(): Argument #1 ($line) must be greater than or equal to 0",
        ));
    }
    fo_rewind(ctx, Some(o), &mut [])?;
    let num = |o: &Object| with_fs(o, |s| s.file.as_ref().map_or(0, |f| f.line_num));
    while num(o) < target {
        if !read_record(ctx, o, true)? {
            return Ok(Value::Null);
        }
    }
    if target > 0 {
        with_fs(o, |s| {
            if let Some(f) = &mut s.file {
                f.line_num = target;
            }
        });
    }
    Ok(Value::Null)
}

/// `__toString(): string` — the current line, read if need be, and this
/// time *not* silently: at end of file it is `RuntimeException: Cannot read
/// from file /x`.
fn fo_to_string(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let empty = with_fs(o, |s| {
        s.file
            .as_ref()
            .is_none_or(|f| f.current_line.is_none() && f.current_zval.is_none())
    });
    if empty {
        read_record(ctx, o, false)?;
    }
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| {
        s.file
            .as_ref()
            .and_then(|f| f.current_line.clone())
            .unwrap_or_default()
    }))))
}

/// `hasChildren(): false` — an `SplFileObject` is a `RecursiveIterator`
/// that never recurses, which is what lets it sit under a
/// `RecursiveIteratorIterator` unharmed.
fn fo_has_children(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

/// `getChildren(): null`
fn fo_get_children(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

// ---- the handle operations -------------------------------------------------

/// `eof(): bool`
fn fo_eof(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    Ok(Value::Bool(at_eof(ctx, &h)?))
}

/// `fgets(): string` / `getCurrentLine(): string` — the one read that
/// always advances the line number, and the one that throws at end of file.
fn fo_fgets(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    if !read_once(ctx, o, false, 1)? {
        return Ok(Value::string(b""));
    }
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| {
        s.file
            .as_ref()
            .and_then(|f| f.current_line.clone())
            .unwrap_or_default()
    }))))
}

/// `fgetc(): string|false` — drops the cache and counts a newline as a
/// line, which is php's own bookkeeping.
fn fo_fgetc(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.current_line = None;
            f.current_zval = None;
        }
    });
    if !readable(&h.name, &h.mode) {
        deny_read(ctx, "fgetc")?;
        return Ok(Value::Bool(false));
    }
    let r = on_stream(ctx, &h, b"fgetc", &[])?;
    if r.to_php_bytes() == b"\n" {
        with_fs(o, |s| {
            if let Some(f) = &mut s.file {
                f.line_num += 1;
            }
        });
    }
    Ok(r)
}

/// `fread(int $length): string|false`
fn fo_fread(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let len = int_arg(ctx, "SplFileObject::fread", 1, "length", &args[0])?;
    if len <= 0 {
        return Err(Unwind::value_error(
            "SplFileObject::fread(): Argument #1 ($length) must be greater than 0",
        ));
    }
    let h = handle(o)?;
    if !readable(&h.name, &h.mode) {
        deny_read(ctx, "fread")?;
        return Ok(Value::Bool(false));
    }
    on_stream(ctx, &h, b"fread", &[Value::Int(len)])
}

/// Write `data` back to disk immediately (see the module header).
fn flush_now(ctx: &mut Ctx, h: &Handle) -> Result<(), Unwind> {
    on_stream(ctx, h, b"fflush", &[])?;
    if !is_php_wrapper(&h.name) {
        let p = as_path(ctx, &h.name);
        invalidate(ctx, &p);
    }
    Ok(())
}

/// `fwrite(string $data, ?int $length = null): int|false`
fn fo_fwrite(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mut data = str_arg(ctx, "SplFileObject::fwrite", 1, "data", &args[0])?;
    if let Some(v) = args.get(1) {
        if !matches!(&*v.deref(), Value::Null | Value::Uninit) {
            let n = int_arg(ctx, "SplFileObject::fwrite", 2, "length", v)?;
            let n = n.max(0) as usize;
            if n < data.len() {
                data.truncate(n);
            }
        }
    }
    if data.is_empty() {
        return Ok(Value::Int(0));
    }
    let h = handle(o)?;
    if !writable(&h.name, &h.mode) {
        deny_write(ctx, "fwrite", data.len())?;
        return Ok(Value::Bool(false));
    }
    let n = on_stream(ctx, &h, b"fwrite", &[Value::Str(Str::from_vec(data))])?;
    flush_now(ctx, &h)?;
    Ok(n)
}

/// `ftell(): int|false`
fn fo_ftell(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    on_stream(ctx, &h, b"ftell", &[])
}

/// `fseek(int $offset, int $whence = SEEK_SET): int` — drops the line
/// cache, as php does, but leaves the line number alone.
fn fo_fseek(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let offset = int_arg(ctx, "SplFileObject::fseek", 1, "offset", &args[0])?;
    let whence = match args.get(1) {
        Some(v) => int_arg(ctx, "SplFileObject::fseek", 2, "whence", v)?,
        None => 0,
    };
    let h = handle(o)?;
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.current_line = None;
            f.current_zval = None;
        }
    });
    on_stream(ctx, &h, b"fseek", &[Value::Int(offset), Value::Int(whence)])
}

/// `fflush(): bool`
fn fo_fflush(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    flush_now(ctx, &h)?;
    Ok(Value::Bool(true))
}

/// `fpassthru(): int`
fn fo_fpassthru(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    on_stream(ctx, &h, b"fpassthru", &[])
}

/// `fstat(): array`
fn fo_fstat(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    on_stream(ctx, &h, b"fstat", &[])
}

/// `flock(int $operation, &$wouldBlock = null): bool`
fn fo_flock(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    let id = ctx.native_by_name(b"flock").expect("flock is registered");
    let mut argv = vec![h.stream.clone(), args[0].deref().into_owned(), Value::Null];
    let r = ctx.call_native(id, &mut argv)?;
    if args.len() > 1 {
        args[1] = argv[2].clone();
    }
    Ok(r)
}

/// `fscanf(string $format, mixed &...$vars): array|int|false|null` — php
/// reads a line of its own first, which is why `fscanf()` moves `key()`.
fn fo_fscanf(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    if !read_once(ctx, o, true, 1)? {
        return Ok(Value::Bool(false));
    }
    let line = with_fs(o, |s| {
        s.file
            .as_ref()
            .and_then(|f| f.current_line.clone())
            .unwrap_or_default()
    });
    let mut scan: Vec<Value> = Vec::with_capacity(args.len() + 1);
    scan.push(Value::Str(Str::from_vec(line)));
    scan.extend(args.iter().cloned());
    let result = crate::string2::sscanf(ctx, &mut scan)?;
    // `sscanf` wrote its captures into the slots it was handed; the subject
    // took slot 0, so everything after `$format` shifts by one.
    for (i, v) in scan.into_iter().enumerate().skip(2) {
        args[i - 1] = v;
    }
    Ok(result)
}

// ---- CSV -------------------------------------------------------------------

/// The CSV trio, defaulting to the object's own control characters — which
/// is why `setCsvControl()` changes what a bare `fgetcsv()` does.
///
/// php deprecates a missing `$escape` unless `setCsvControl()` supplied
/// one, and says so in a longer sentence than the procedural functions use.
fn csv_control(
    ctx: &mut Ctx,
    o: &Object,
    method: &str,
    args: &[Value],
    base: usize,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), Unwind> {
    let (mut delim, mut enc, mut esc, given) = with_fs(o, |s| {
        let f = s.file.as_ref().expect("checked by the caller");
        (
            vec![f.delimiter],
            vec![f.enclosure],
            f.escape.clone(),
            f.escape_given,
        )
    });
    if let Some(v) = args.get(base) {
        delim = str_arg(
            ctx,
            &format!("SplFileObject::{method}"),
            base as u32 + 1,
            "separator",
            v,
        )?;
        if delim.len() != 1 {
            return Err(Unwind::value_error(format!(
                "SplFileObject::{method}(): Argument #{} ($separator) must be a single character",
                base + 1
            )));
        }
    }
    if let Some(v) = args.get(base + 1) {
        enc = str_arg(
            ctx,
            &format!("SplFileObject::{method}"),
            base as u32 + 2,
            "enclosure",
            v,
        )?;
        if enc.len() != 1 {
            return Err(Unwind::value_error(format!(
                "SplFileObject::{method}(): Argument #{} ($enclosure) must be a single character",
                base + 2
            )));
        }
    }
    match args.get(base + 2) {
        Some(v) => {
            esc = str_arg(
                ctx,
                &format!("SplFileObject::{method}"),
                base as u32 + 3,
                "escape",
                v,
            )?;
            if esc.len() > 1 {
                return Err(Unwind::value_error(format!(
                    "SplFileObject::{method}(): Argument #{} ($escape) must be empty or a \
                     single character",
                    base + 3
                )));
            }
        }
        None if !given => {
            ctx.deprecated(&format!(
                "SplFileObject::{method}(): the $escape parameter must be provided, as its \
                 default value will change, either explicitly or via SplFileObject::setCsvControl()"
            ))?;
        }
        None => {}
    }
    Ok((delim, enc, esc))
}

/// `fgetcsv(string $separator = ',', string $enclosure = '"', string $escape = '\\'): array|false`
fn fo_fgetcsv(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    let (delim, enc, esc) = csv_control(ctx, o, "fgetcsv", args, 0)?;
    if !readable(&h.name, &h.mode) {
        deny_read(ctx, "fgetcsv")?;
        return Ok(Value::Bool(false));
    }
    on_stream(
        ctx,
        &h,
        b"fgetcsv",
        &[
            Value::Int(0),
            Value::Str(Str::from_vec(delim)),
            Value::Str(Str::from_vec(enc)),
            Value::Str(Str::from_vec(esc)),
        ],
    )
}

/// `fputcsv(array $fields, string $separator = ',', string $enclosure = '"',
/// string $escape = '\\', string $eol = "\n"): int|false`
fn fo_fputcsv(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let h = handle(o)?;
    let fields = args[0].deref().into_owned();
    if !matches!(fields, Value::Array(_)) {
        return Err(Unwind::type_error(format!(
            "SplFileObject::fputcsv(): Argument #1 ($fields) must be of type array, {} given",
            rphp_runtime::value_name(&fields)
        )));
    }
    let (delim, enc, esc) = csv_control(ctx, o, "fputcsv", args, 1)?;
    let eol = match args.get(4) {
        Some(v) => str_arg(ctx, "SplFileObject::fputcsv", 5, "eol", v)?,
        None => b"\n".to_vec(),
    };
    let n = on_stream(
        ctx,
        &h,
        b"fputcsv",
        &[
            fields,
            Value::Str(Str::from_vec(delim)),
            Value::Str(Str::from_vec(enc)),
            Value::Str(Str::from_vec(esc)),
            Value::Str(Str::from_vec(eol)),
        ],
    )?;
    flush_now(ctx, &h)?;
    Ok(n)
}

/// `getCsvControl(): array`
fn fo_get_csv_control(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(with_fs(o, |s| {
        let mut a = rphp_value::Array::new();
        if let Some(f) = &s.file {
            a.push(Value::Str(Str::from_vec(vec![f.delimiter])));
            a.push(Value::Str(Str::from_vec(vec![f.enclosure])));
            a.push(Value::Str(Str::from_vec(f.escape.clone())));
        }
        a
    })))
}

/// `setCsvControl(string $separator = ',', string $enclosure = '"', string $escape = '\\'): void`
fn fo_set_csv_control(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "SplFileObject::setCsvControl";
    let delim = match args.first() {
        Some(v) => str_arg(ctx, who, 1, "separator", v)?,
        None => b",".to_vec(),
    };
    if delim.len() != 1 {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($separator) must be a single character"
        )));
    }
    let enc = match args.get(1) {
        Some(v) => str_arg(ctx, who, 2, "enclosure", v)?,
        None => b"\"".to_vec(),
    };
    if enc.len() != 1 {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($enclosure) must be a single character"
        )));
    }
    let esc = match args.get(2) {
        Some(v) => str_arg(ctx, who, 3, "escape", v)?,
        None => b"\\".to_vec(),
    };
    if esc.len() > 1 {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #3 ($escape) must be empty or a single character"
        )));
    }
    let given = args.len() >= 3;
    let st = with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.delimiter = delim[0];
            f.enclosure = enc[0];
            f.escape = esc;
            // php remembers that an escape was named, which is what turns
            // the `fputcsv()`/`fgetcsv()` deprecation off.
            f.escape_given = f.escape_given || given;
        }
        s.clone()
    });
    sync(o, &st);
    Ok(Value::Null)
}

// ---- flags -----------------------------------------------------------------

/// `getFlags(): int`
fn fo_get_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_fs(o, |s| {
        s.file.as_ref().map_or(0, |f| f.flags) & FILE_FLAGS_MASK
    })))
}

/// `setFlags(int $flags): void`
fn fo_set_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let new = int_arg(ctx, "SplFileObject::setFlags", 1, "flags", &args[0])?;
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.flags = new;
        }
    });
    Ok(Value::Null)
}

/// `getMaxLineLen(): int`
fn fo_get_max_line_len(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_fs(o, |s| {
        s.file.as_ref().map_or(0, |f| f.max_line_len)
    })))
}

/// `setMaxLineLen(int $maxLength): void`
fn fo_set_max_line_len(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let n = int_arg(
        ctx,
        "SplFileObject::setMaxLineLen",
        1,
        "maxLength",
        &args[0],
    )?;
    if n < 0 {
        return Err(Unwind::value_error(
            "SplFileObject::setMaxLineLen(): Argument #1 ($maxLength) must be greater than \
             or equal to 0",
        ));
    }
    with_fs(o, |s| {
        if let Some(f) = &mut s.file {
            f.max_line_len = n;
        }
    });
    Ok(Value::Null)
}

/// `__debugInfo(): array`
fn fo_debug_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(with_fs(o, |s| debug_array(s))))
}

// ---- constructors ----------------------------------------------------------

/// `SplFileObject::__construct(string $filename, string $mode = 'r',
/// bool $useIncludePath = false, ?resource $context = null)`
fn fo_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "SplFileObject::__construct";
    let name = str_arg(ctx, who, 1, "filename", &args[0])?;
    let mode = match args.get(1) {
        Some(v) => str_arg(ctx, who, 2, "mode", v)?,
        None => b"r".to_vec(),
    };
    let use_include_path = args.get(2).is_some_and(|v| v.deref().to_bool());
    open_into(ctx, o, who, &name, &mode, use_include_path)?;
    Ok(Value::Null)
}

/// `SplTempFileObject::__construct(int $maxMemory = 2097152)` — a negative
/// budget means `php://memory`, an explicit one
/// `php://temp/maxmemory:<n>`, and no argument at all the bare
/// `php://temp`. php stores that name **without** deriving a path from it,
/// which is why `getFilename()` answers the whole `php://temp` where an
/// `SplFileObject` on `php://memory` answers just `memory`.
fn temp_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name: Vec<u8> = match args.first() {
        Some(v) => {
            let n = int_arg(ctx, "SplTempFileObject::__construct", 1, "maxMemory", v)?;
            if n < 0 {
                b"php://memory".to_vec()
            } else {
                format!("php://temp/maxmemory:{n}").into_bytes()
            }
        }
        None => b"php://temp".to_vec(),
    };
    open_into(
        ctx,
        o,
        "SplTempFileObject::__construct",
        &name,
        b"wb",
        false,
    )?;
    // php assigns `file_name` directly here, leaving `_path` unset.
    let st = with_fs(o, |s| {
        s.path = Vec::new();
        s.file_name = Some(name);
        s.clone()
    });
    sync(o, &st);
    Ok(Value::Null)
}

// ---- registration ----------------------------------------------------------

/// Register `SplFileObject` and `SplTempFileObject`. `openMode`,
/// `delimiter` and `enclosure` are real private slots because php's debug
/// view shows them.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("SplFileObject")
        .extends("SplFileInfo")
        .implements(&["RecursiveIterator", "SeekableIterator"])
        .class_const("DROP_NEW_LINE", Value::Int(DROP_NEW_LINE))
        .class_const("READ_AHEAD", Value::Int(READ_AHEAD))
        .class_const("SKIP_EMPTY", Value::Int(SKIP_EMPTY))
        .class_const("READ_CSV", Value::Int(READ_CSV))
        .prop("openMode", Visibility::Private, Value::Uninit)
        .prop("delimiter", Visibility::Private, Value::Uninit)
        .prop("enclosure", Visibility::Private, Value::Uninit)
        .method("__construct", nm!(1, Some(4), fo_construct))
        .method("rewind", nm!(0, Some(0), fo_rewind))
        .method("eof", nm!(0, Some(0), fo_eof))
        .method("valid", nm!(0, Some(0), fo_valid))
        .method("fgets", nm!(0, Some(0), fo_fgets))
        .method("fread", nm!(1, Some(1), fo_fread))
        .method("fgetcsv", nm!(0, Some(3), fo_fgetcsv))
        .method("fputcsv", nm!(1, Some(5), fo_fputcsv))
        .method("setCsvControl", nm!(0, Some(3), fo_set_csv_control))
        .method("getCsvControl", nm!(0, Some(0), fo_get_csv_control))
        .method("flock", nm_ref(1, Some(2), 0b10, fo_flock))
        .method("fflush", nm!(0, Some(0), fo_fflush))
        .method("ftell", nm!(0, Some(0), fo_ftell))
        .method("fseek", nm!(1, Some(2), fo_fseek))
        .method("fgetc", nm!(0, Some(0), fo_fgetc))
        .method("fpassthru", nm!(0, Some(0), fo_fpassthru))
        .method("fscanf", nm_ref(1, None, 0xFFFF_FFFE, fo_fscanf))
        .method("fwrite", nm!(1, Some(2), fo_fwrite))
        .method("fstat", nm!(0, Some(0), fo_fstat))
        .method("current", nm!(0, Some(0), fo_current))
        .method("key", nm!(0, Some(0), fo_key))
        .method("next", nm!(0, Some(0), fo_next))
        .method("setFlags", nm!(1, Some(1), fo_set_flags))
        .method("getFlags", nm!(0, Some(0), fo_get_flags))
        .method("setMaxLineLen", nm!(1, Some(1), fo_set_max_line_len))
        .method("getMaxLineLen", nm!(0, Some(0), fo_get_max_line_len))
        .method("hasChildren", nm!(0, Some(0), fo_has_children))
        .method("getChildren", nm!(0, Some(0), fo_get_children))
        .method("seek", nm!(1, Some(1), fo_seek))
        .method("getCurrentLine", nm!(0, Some(0), fo_fgets))
        .method("__toString", nm!(0, Some(0), fo_to_string))
        .method("__debugInfo", nm!(0, Some(0), fo_debug_info))
        .method("__clone", nm!(0, Some(0), no_clone))
        .finish();

    r.class("SplTempFileObject")
        .extends("SplFileObject")
        .method("__construct", nm!(0, Some(1), temp_construct))
        .finish();
}
