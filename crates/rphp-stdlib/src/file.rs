//! The `file` half of ext/standard: path arithmetic and whole-file I/O.
//!
//! The path functions (`dirname`, `basename`, `pathinfo`) live in
//! `string2.rs`, where php also groups them — they are pure string
//! arithmetic and never touch the filesystem.
//!
//! Reads and writes here go through the stat cache in `filestat.rs`, which
//! they invalidate whenever they change a path.

use std::fs;
use std::io::Write;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

use crate::filestat::{arg_path, invalidate};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("file_get_contents", 1, Some(5), file_get_contents),
    nf!("file_put_contents", 2, Some(4), file_put_contents),
    nf!("file", 1, Some(3), file),
    nf!("readfile", 1, Some(3), readfile),
    nf!("unlink", 1, Some(2), unlink),
    nf!("is_uploaded_file", 1, Some(1), is_uploaded_file),
    nf!("move_uploaded_file", 2, Some(2), move_uploaded_file),
    nf!("copy", 2, Some(3), copy),
    nf!("rename", 2, Some(3), rename),
    nf!("stream_resolve_include_path", 1, Some(1), stream_resolve_include_path),
    nf!("fopen", 2, Some(4), fopen),
    nf!("fclose", 1, Some(1), fclose),
    nf!("fwrite", 2, Some(3), fwrite),
    nf!("fputs", 2, Some(3), fwrite),
    nf!("fread", 2, Some(2), fread),
    nf!("fgets", 1, Some(2), fgets),
    nf!("fgetc", 1, Some(1), fgetc),
    nf!("feof", 1, Some(1), feof),
    nf!("ftell", 1, Some(1), ftell),
    nf!("fseek", 2, Some(3), fseek),
    nf!("rewind", 1, Some(1), rewind),
    nf!("fflush", 1, Some(1), fflush),
    nf!("stream_get_contents", 1, Some(3), stream_get_contents),
    nf!("stream_get_meta_data", 1, Some(1), stream_get_meta_data),
    nf!("stream_set_blocking", 2, Some(2), stream_set_blocking),
    nf!("stream_isatty", 1, Some(1), stream_isatty),
    nf!("stream_copy_to_stream", 2, Some(4), stream_copy_to_stream),
    nf!("stream_is_local", 1, Some(1), stream_is_local),
];

/// The wrappers php marks `is_url`: everything else — `file://`, `php://`,
/// `compress.*://`, `phar://`, a plain path — is local. `data:` is the one
/// that does not spell its scheme with `://`.
const REMOTE_WRAPPERS: &[&str] = &[
    "http", "https", "ftp", "ftps", "ssh2.shell", "ssh2.exec", "ssh2.tunnel", "ssh2.ftp",
    "ssh2.scp", "ssh2.sftp", "ogg", "expect",
];

/// An open stream: a byte buffer plus a cursor, and where writes ultimately
/// go. php's `php://memory` and `php://temp` are pure buffers; a real file is
/// read into the buffer on open and written back on flush/close, which keeps
/// the whole implementation one code path. `php://stdout`/`stderr`/`output`
/// forward writes to the engine's output channel instead.
pub(crate) struct Stream {
    buf: Vec<u8>,
    pos: usize,
    /// The file to write back to on flush, when this is a real file opened
    /// for writing.
    path: Option<std::path::PathBuf>,
    /// Appending: writes always go to the end and the file is not truncated.
    append: bool,
    /// Where writes go when this is a php:// output stream.
    sink: Sink,
    readable: bool,
    writable: bool,
    /// php's end-of-file flag, which is **not** "the cursor is at the end":
    /// it is set by a read that came back short of what it asked for, and
    /// cleared by a seek. A fresh handle on an empty file reports `false`
    /// until something tries to read it.
    eof: bool,
    dirty: bool,
    /// The mode string `fopen` was called with, which
    /// `stream_get_meta_data()` reports back.
    mode: Box<str>,
    /// The name the stream was opened under (`uri`): a real path, or the
    /// `php://…` spelling.
    uri: Box<str>,
    /// How far php's read buffer was filled by the last read. `unread_bytes`
    /// is what is left of that fill, so it is `0` on a fresh handle and after
    /// a seek, and up to one 8192-byte chunk after a read.
    fill_end: usize,
}

/// Where a stream's writes end up.
#[derive(PartialEq, Clone, Copy)]
pub(crate) enum Sink {
    /// A buffer (memory streams and real files).
    Buffer,
    /// The engine's output channel (`php://stdout`, `php://output`).
    Stdout,
    /// The process's stderr (`php://stderr`).
    Stderr,
}

impl Stream {
    /// `ftruncate`: cut the buffer to `size`, or extend it with NUL bytes.
    /// The cursor stays where it was, as php leaves it.
    pub(crate) fn truncate_to(&mut self, size: usize) {
        self.buf.resize(size, 0);
        self.dirty = true;
        self.fill_end = self.fill_end.min(self.buf.len());
    }

    /// php's `SEEK_*` applied to the cursor.
    fn seek(&mut self, offset: i64, whence: i64) -> i64 {
        let base = match whence {
            1 => self.pos as i64,          // SEEK_CUR
            2 => self.buf.len() as i64,    // SEEK_END
            _ => 0,                        // SEEK_SET
        };
        self.pos = (base + offset).max(0) as usize;
        self.eof = false;
        // A seek throws php's read buffer away.
        self.fill_end = self.pos;
        0
    }

    /// Write `data` at the cursor, extending the buffer as php does.
    fn write(&mut self, data: &[u8]) -> usize {
        if self.append {
            self.pos = self.buf.len();
        }
        let end = self.pos + data.len();
        if self.buf.len() < end {
            self.buf.resize(end, 0);
        }
        self.buf[self.pos..end].copy_from_slice(data);
        self.pos = end;
        self.dirty = true;
        data.len()
    }
}

/// php's `FILE_*` flags that these functions honour.
const FILE_IGNORE_NEW_LINES: i64 = 2;
const FILE_SKIP_EMPTY_LINES: i64 = 4;
const FILE_APPEND: i64 = 8;

/// `file_get_contents(string $filename, ...): string|false`
fn file_get_contents(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // `php://input` is the request body, readable any number of times.
    if args[0].to_php_bytes().as_slice() == b"php://input" {
        let body = ctx.request_body.as_deref().map(<[u8]>::to_vec).unwrap_or_default();
        return Ok(Value::Str(Str::from_vec(body)));
    }
    let p = arg_path(ctx, &args[0]);
    match fs::read(&p) {
        Ok(b) => Ok(Value::Str(Str::from_vec(b))),
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "file_get_contents({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `file_put_contents(string $filename, mixed $data, int $flags = 0, ...): int|false`
///
/// Returns the **byte count**, not a bool.
fn file_put_contents(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    // An array argument is concatenated, as php does for `file()` output.
    let data: Vec<u8> = match &*args[1].deref() {
        Value::Array(a) => {
            let mut out = Vec::new();
            for (_, v) in a.iter() {
                out.extend_from_slice(&v.deref().to_php_bytes());
            }
            out
        }
        other => other.to_php_bytes().to_vec(),
    };
    let flags = args.get(2).map_or(0, Value::to_int);
    let r = if flags & FILE_APPEND != 0 {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .and_then(|mut f| f.write_all(&data))
    } else {
        fs::write(&p, &data)
    };
    invalidate(ctx, &p);
    match r {
        Ok(()) => Ok(Value::Int(data.len() as i64)),
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "file_put_contents({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// Split file contents into php's `file()` lines: the newline stays on the
/// line unless `FILE_IGNORE_NEW_LINES`.
fn split_lines(data: &[u8], flags: i64) -> Vec<Vec<u8>> {
    let keep_nl = flags & FILE_IGNORE_NEW_LINES == 0;
    let skip_empty = flags & FILE_SKIP_EMPTY_LINES != 0;
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            let end = if keep_nl { i + 1 } else { i };
            let mut line = data[start..end].to_vec();
            if !keep_nl && line.last() == Some(&b'\r') {
                line.pop();
            }
            if !(skip_empty && line.is_empty()) {
                out.push(line);
            }
            start = i + 1;
        }
    }
    if start < data.len() {
        let line = data[start..].to_vec();
        if !(skip_empty && line.is_empty()) {
            out.push(line);
        }
    }
    out
}

/// `file(string $filename, int $flags = 0, ...): array|false`
fn file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let flags = args.get(1).map_or(0, Value::to_int);
    match fs::read(&p) {
        Ok(b) => {
            let mut a = Array::new();
            for line in split_lines(&b, flags) {
                a.push(Value::Str(Str::from_vec(line)));
            }
            Ok(Value::Array(a))
        }
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "file({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `readfile(string $filename, ...): int|false` — write the file to output.
fn readfile(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match fs::read(&p) {
        Ok(b) => {
            let n = b.len() as i64;
            ctx.echo(&b);
            Ok(Value::Int(n))
        }
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "readfile({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `unlink(string $filename, ...): bool`
fn unlink(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let r = fs::remove_file(&p);
    invalidate(ctx, &p);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!("unlink({shown}): {}", crate::filestat::io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `is_uploaded_file(string $filename): bool` — whether the path is one
/// of this request's uploads, spelled exactly as `$_FILES` spells it.
fn is_uploaded_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    Ok(Value::Bool(ctx.uploaded_files.iter().any(|f| f.to_string_lossy() == name)))
}

/// `move_uploaded_file(string $from, string $to): bool` — a rename (a copy
/// and unlink across devices) of an upload, which is then no longer one;
/// `false` without a word for anything that is not an upload.
fn move_uploaded_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from_name = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let Some(i) = ctx.uploaded_files.iter().position(|f| f.to_string_lossy() == from_name) else {
        return Ok(Value::Bool(false));
    };
    let from = arg_path(ctx, &args[0]);
    let to = arg_path(ctx, &args[1]);
    let moved = fs::rename(&from, &to).or_else(|_| fs::copy(&from, &to).and_then(|_| fs::remove_file(&from)));
    invalidate(ctx, &from);
    invalidate(ctx, &to);
    match moved {
        Ok(()) => {
            ctx.uploaded_files.remove(i);
            Ok(Value::Bool(true))
        }
        Err(_) => {
            let to_shown = String::from_utf8_lossy(&args[1].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "move_uploaded_file(): Unable to move \"{from_name}\" to \"{to_shown}\""
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `copy(string $from, string $to, ...): bool`
fn copy(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from = arg_path(ctx, &args[0]);
    let to = arg_path(ctx, &args[1]);
    let r = fs::copy(&from, &to);
    invalidate(ctx, &to);
    match r {
        Ok(_) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "copy({shown}): Failed to open stream: {}",
                crate::filestat::io_text(&e)
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `rename(string $from, string $to, ...): bool`
fn rename(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from = arg_path(ctx, &args[0]);
    let to = arg_path(ctx, &args[1]);
    let r = fs::rename(&from, &to);
    invalidate(ctx, &from);
    invalidate(ctx, &to);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            // php names both paths here.
            let src = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            let dst = String::from_utf8_lossy(&args[1].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "rename({src},{dst}): {}",
                crate::filestat::io_text(&e)
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `stream_resolve_include_path(string $filename): string|false` — the
/// canonical path when the file is reachable, `false` otherwise.
fn stream_resolve_include_path(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(match fs::canonicalize(&p) {
        Ok(c) => Value::string(c.to_string_lossy().as_bytes()),
        Err(_) => Value::Bool(false),
    })
}

/// php's `FILE_*` and `PATHINFO_*` constants.
pub(crate) fn register_constants(r: &mut rphp_runtime::Registry) {
    for (name, v) in [
        ("FILE_USE_INCLUDE_PATH", 1i64),
        ("FILE_IGNORE_NEW_LINES", FILE_IGNORE_NEW_LINES),
        ("FILE_SKIP_EMPTY_LINES", FILE_SKIP_EMPTY_LINES),
        ("FILE_APPEND", FILE_APPEND),
        ("FILE_NO_DEFAULT_CONTEXT", 16),
        ("LOCK_SH", 1),
        ("LOCK_EX", 2),
        ("LOCK_UN", 3),
        ("PATHINFO_DIRNAME", 1),
        ("PATHINFO_BASENAME", 2),
        ("PATHINFO_EXTENSION", 4),
        ("PATHINFO_FILENAME", 8),
        ("SEEK_SET", 0),
        ("SEEK_CUR", 1),
        ("SEEK_END", 2),
    ] {
        r.constant(name, Value::Int(v));
    }
    // The CLI SAPI's three standard handles, in php's order — which is why
    // they are resources 1, 2 and 3 and the first `fopen()` of a script is 5.
    for (name, sink, uri, mode) in [
        ("STDIN", Sink::Buffer, "php://stdin", "r"),
        ("STDOUT", Sink::Stdout, "php://stdout", "w"),
        ("STDERR", Sink::Stderr, "php://stderr", "w"),
    ] {
        let res = r
            .interp()
            .resources
            .add("stream", Box::new(std_stream(sink, uri, mode)));
        r.constant(name, res);
    }
    // php's CLI holds a fourth resource of its own, so a script's first
    // `fopen()` is id 5 and every dumped handle counts from there. Reserving
    // one keeps `var_dump($handle)` identical.
    r.interp().resources.reserve_id();
}

// ---- streams ---------------------------------------------------------------

/// Resolve a stream argument to its resource, or php's TypeError.
fn stream_arg(ctx: &mut Ctx, v: &Value, func: &str) -> Result<rphp_value::Resource, Unwind> {
    match &*v.deref() {
        Value::Resource(r) if r.kind() == "stream" => Ok(r.clone()),
        other => {
            let _ = ctx;
            Err(Unwind::type_error(format!(
                "{func}(): Argument #1 ($stream) must be of type resource, {} given",
                other.type_name()
            )))
        }
    }
}

/// Run `f` over the stream behind a resource.
fn with_stream<R>(
    ctx: &mut Ctx,
    v: &Value,
    func: &str,
    f: impl FnOnce(&mut Stream) -> R,
) -> Result<R, Unwind> {
    let r = stream_arg(ctx, v, func)?;
    let mut payload = r.payload_mut();
    let Some(any) = payload.as_mut() else {
        return Err(Unwind::error(format!("{func}(): supplied resource is not a valid stream")));
    };
    let Some(s) = any.downcast_mut::<Stream>() else {
        return Err(Unwind::error(format!("{func}(): supplied resource is not a valid stream")));
    };
    Ok(f(s))
}

/// `fopen(string $filename, string $mode, ...): resource|false`
fn fopen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes().to_vec();
    let mode = args[1].to_php_bytes().to_vec();
    let m = String::from_utf8_lossy(&mode).into_owned();
    let readable = m.contains('r') || m.contains('+');
    let writable = m.contains('w') || m.contains('a') || m.contains('x') || m.contains('c') || m.contains('+');
    let append = m.starts_with('a');
    let truncate = m.starts_with('w');

    let path_str = String::from_utf8_lossy(&name).into_owned();
    // php:// wrappers: memory and temp are buffers, the rest are output.
    let stream = if let Some(rest) = path_str.strip_prefix("php://") {
        let sink = match rest {
            "stdout" | "output" => Sink::Stdout,
            "stderr" => Sink::Stderr,
            _ => Sink::Buffer,
        };
        // `php://input` opens on a copy of the request body.
        let input = rest == "input";
        let buf = if input {
            ctx.request_body.as_deref().map(<[u8]>::to_vec).unwrap_or_default()
        } else {
            Vec::new()
        };
        Stream {
            buf,
            pos: 0,
            path: None,
            append,
            sink,
            readable: sink == Sink::Buffer,
            writable: !input && (sink != Sink::Buffer || writable),
            eof: false,
            dirty: false,
            // php's memory streams are always binary.
            mode: if sink == Sink::Buffer && !m.contains('b') {
                format!("{m}b").into()
            } else {
                m.clone().into()
            },
            uri: path_str.clone().into(),
            fill_end: 0,
        }
    } else {
        let p = arg_path(ctx, &args[0]);
        let existing = if truncate { Ok(Vec::new()) } else { fs::read(&p) };
        let buf = match existing {
            Ok(b) => b,
            Err(_) if writable => Vec::new(),
            Err(_) => {
                ctx.warn(&format!(
                    "fopen({path_str}): Failed to open stream: No such file or directory"
                ))?;
                return Ok(Value::Bool(false));
            }
        };
        let pos = if append { buf.len() } else { 0 };
        Stream {
            buf,
            pos,
            path: Some(p.clone()),
            append,
            sink: Sink::Buffer,
            readable,
            writable,
            eof: false,
            // A `w`/`a` open creates the file even with nothing written.
            dirty: writable && (truncate || append),
            mode: m.clone().into(),
            uri: p.to_string_lossy().into_owned().into(),
            fill_end: pos,
        }
    };
    Ok(ctx.resources.add("stream", Box::new(stream)))
}

/// Run `f` on the stream behind `v` — the same access `file.rs`'s own
/// functions have, for the handle functions that live in `file2.rs`.
pub(crate) fn with_stream_mut<R>(
    ctx: &mut Ctx,
    v: &Value,
    func: &str,
    f: impl FnOnce(&mut Stream) -> R,
) -> Result<R, Unwind> {
    with_stream(ctx, v, func, f)
}

/// Open `path` the way `fopen` would and hand back the resource — what
/// `tmpfile()` and the CLI's standard handles are built from.
pub(crate) fn open_resource(ctx: &mut Ctx, path: &std::path::Path, mode: &str) -> Value {
    let stream = Stream {
        buf: Vec::new(),
        pos: 0,
        path: Some(path.to_path_buf()),
        append: false,
        sink: Sink::Buffer,
        readable: true,
        writable: true,
        eof: false,
        dirty: true,
        mode: mode.into(),
        uri: path.to_string_lossy().into_owned().into(),
        fill_end: 0,
    };
    ctx.resources.add("stream", Box::new(stream))
}

/// A `php://` handle for one of the process's standard streams.
fn std_stream(sink: Sink, uri: &str, mode: &str) -> Stream {
    Stream {
        buf: Vec::new(),
        pos: 0,
        path: None,
        append: false,
        sink,
        readable: sink == Sink::Buffer,
        writable: sink != Sink::Buffer,
        eof: false,
        dirty: false,
        mode: mode.into(),
        uri: uri.into(),
        fill_end: 0,
    }
}

/// Write a stream's buffer back to its file, if it has one.
fn flush_stream(s: &Stream) {
    if let (Some(p), true) = (&s.path, s.dirty) {
        let _ = fs::write(p, &s.buf);
    }
}

/// `fclose(resource $stream): bool`
fn fclose(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    with_stream(ctx, &args[0].clone(), "fclose", |s| flush_stream(s))?;
    ctx.resources.close_value(&args[0]);
    Ok(Value::Bool(true))
}

/// `fwrite(resource $stream, string $data, ?int $length = null): int|false`
fn fwrite(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut data = args[1].to_php_bytes().to_vec();
    if let Some(len) = args.get(2).filter(|v| !matches!(v, Value::Null)).map(Value::to_int) {
        data.truncate(len.max(0) as usize);
    }
    let stream = args[0].clone();
    let sink = with_stream(ctx, &stream, "fwrite", |s| {
        if !s.writable {
            return Err((s.path.is_some(), data.len()));
        }
        if s.sink == Sink::Buffer {
            s.write(&data);
        }
        Ok(s.sink)
    })?;
    match sink {
        // A write of nothing never reaches the fd, so php answers `0` without
        // a diagnostic even on a read-only handle.
        Err((_, 0)) => Ok(Value::Int(0)),
        Err((real, n)) => {
            if real {
                ctx.notice(&format!(
                    "fwrite(): Write of {n} bytes failed with errno=9 Bad file descriptor"
                ))?;
            }
            Ok(Value::Bool(false))
        }
        Ok(Sink::Stdout) => {
            ctx.echo(&data);
            Ok(Value::Int(data.len() as i64))
        }
        Ok(Sink::Stderr) => {
            eprint!("{}", String::from_utf8_lossy(&data));
            Ok(Value::Int(data.len() as i64))
        }
        Ok(Sink::Buffer) => Ok(Value::Int(data.len() as i64)),
    }
}

/// `fread(resource $stream, int $length): string|false`
/// php's read buffer is filled a chunk (8192 bytes) at a time from the
/// current position; `unread_bytes` reports what is left of that fill. A
/// memory stream has no buffer and always answers `0`.
const CHUNK: usize = 8192;

fn record_fill(s: &mut Stream) {
    if s.path.is_some() {
        s.fill_end = (s.pos + CHUNK).min(s.buf.len());
    }
}

/// php reads through the fd in `chunk_size` (8192) pieces, so a handle opened
/// write-only fails there and every read function reports the same notice —
/// naming 8192 bytes whatever length was asked for. A memory stream has no fd
/// and just answers nothing.
fn read_denied(ctx: &mut Ctx, v: &Value, func: &str) -> Result<bool, Unwind> {
    let denied = with_stream(ctx, v, func, |s| !s.readable && s.path.is_some())?;
    if denied {
        ctx.notice(&format!(
            "{func}(): Read of 8192 bytes failed with errno=9 Bad file descriptor"
        ))?;
    }
    Ok(denied)
}

fn fread(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let len = args[1].to_int().max(0) as usize;
    let stream = args[0].clone();
    if read_denied(ctx, &stream, "fread")? {
        return Ok(Value::Bool(false));
    }
    with_stream(ctx, &stream, "fread", |s| {
        record_fill(s);
        let end = (s.pos + len).min(s.buf.len());
        let out = s.buf[s.pos.min(s.buf.len())..end].to_vec();
        s.pos = end;
        // Short of the length asked for means the read hit the end.
        s.eof = out.len() < len;
        Value::Str(Str::from_vec(out))
    })
}

/// `fgets(resource $stream, ?int $length = null): string|false` — a line
/// *including* its newline, `false` at end of stream.
fn fgets(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    if read_denied(ctx, &stream, "fgets")? {
        return Ok(Value::Bool(false));
    }
    with_stream(ctx, &stream, "fgets", |s| {
        record_fill(s);
        if s.pos >= s.buf.len() {
            s.eof = true;
            return Value::Bool(false);
        }
        let start = s.pos;
        // A line that ends at the end of the data, rather than at a newline,
        // only got there by reading past the end — php flags that.
        let end = match s.buf[start..].iter().position(|&b| b == b'\n') {
            Some(i) => start + i + 1,
            None => {
                s.eof = true;
                s.buf.len()
            }
        };
        s.pos = end;
        Value::Str(Str::from_vec(s.buf[start..end].to_vec()))
    })
}

/// `fgetc(resource $stream): string|false`
fn fgetc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    if read_denied(ctx, &stream, "fgetc")? {
        return Ok(Value::Bool(false));
    }
    with_stream(ctx, &stream, "fgetc", |s| {
        record_fill(s);
        if s.pos >= s.buf.len() {
            s.eof = true;
            return Value::Bool(false);
        }
        let b = s.buf[s.pos];
        s.pos += 1;
        Value::Str(Str::from_vec(vec![b]))
    })
}

/// `feof(resource $stream): bool`
fn feof(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "feof", |s| Value::Bool(s.eof))
}

/// `ftell(resource $stream): int|false`
fn ftell(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "ftell", |s| Value::Int(s.pos as i64))
}

/// `fseek(resource $stream, int $offset, int $whence = SEEK_SET): int`
fn fseek(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let offset = args[1].to_int();
    let whence = args.get(2).map_or(0, Value::to_int);
    let stream = args[0].clone();
    with_stream(ctx, &stream, "fseek", |s| Value::Int(s.seek(offset, whence)))
}

/// `rewind(resource $stream): bool`
fn rewind(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "rewind", |s| {
        s.seek(0, 0);
        Value::Bool(true)
    })
}

/// `fflush(resource $stream): bool`
fn fflush(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "fflush", |s| {
        flush_stream(s);
        Value::Bool(true)
    })
}

/// `stream_get_contents(resource $stream, ?int $maxLength = null, int $offset = -1): string|false`
///
/// Reads from the **current position** unless `$offset` is given.
fn stream_get_contents(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let max = args.get(1).filter(|v| !matches!(v, Value::Null)).map(Value::to_int);
    let offset = args.get(2).map_or(-1, Value::to_int);
    let stream = args[0].clone();
    if read_denied(ctx, &stream, "stream_get_contents")? {
        return Ok(Value::Str(Str::new(b"")));
    }
    with_stream(ctx, &stream, "stream_get_contents", |s| {
        record_fill(s);
        if offset >= 0 {
            s.pos = (offset as usize).min(s.buf.len());
        }
        let start = s.pos.min(s.buf.len());
        let end = match max {
            Some(n) if n >= 0 => (start + n as usize).min(s.buf.len()),
            _ => s.buf.len(),
        };
        s.pos = end;
        // Without a length it reads until the end; with one, only a short
        // answer means the end was reached.
        s.eof = match max {
            Some(n) if n >= 0 => end - start < n as usize,
            _ => true,
        };
        Value::Str(Str::from_vec(s.buf[start..end].to_vec()))
    })
}

/// `stream_get_meta_data(resource $stream): array` — php's nine keys, in
/// php's order. `stream_type`/`wrapper_type` name the implementation behind
/// the handle: a real file is `plainfile`/`STDIO`, `php://memory` is
/// `PHP`/`MEMORY`, `php://temp` `PHP`/`TEMP`, and the three output handles
/// `PHP`/`STDIO` (and are not seekable).
fn stream_get_meta_data(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "stream_get_meta_data", |s| {
        let php_stream = s.path.is_none();
        let (wrapper, kind, seekable) = if !php_stream {
            ("plainfile", "STDIO".to_string(), true)
        } else if s.sink != Sink::Buffer {
            ("PHP", "STDIO".to_string(), false)
        } else if s.uri.ends_with("temp") || s.uri.starts_with("php://temp/") {
            ("PHP", "TEMP".to_string(), true)
        } else {
            ("PHP", "MEMORY".to_string(), true)
        };
        let mut out = Array::new();
        let mut set = |k: &str, v: Value| out.set(ArrayKey::Str(Box::from(k.as_bytes())), v);
        set("timed_out", Value::Bool(false));
        set("blocked", Value::Bool(true));
        set("eof", Value::Bool(s.eof));
        set("wrapper_type", Value::string(wrapper.as_bytes()));
        set("stream_type", Value::string(kind.as_bytes()));
        set("mode", Value::string(s.mode.as_bytes()));
        set(
            "unread_bytes",
            Value::Int(s.fill_end.saturating_sub(s.pos) as i64),
        );
        set("seekable", Value::Bool(seekable));
        set("uri", Value::string(s.uri.as_bytes()));
        Value::Array(out)
    })
}

/// `stream_set_blocking(resource $stream, bool $enable): bool` — every stream
/// rphp has is a buffer or an ordinary file, so it is always blocking and the
/// call only has to report success.
fn stream_set_blocking(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "stream_set_blocking", |_| Value::Bool(true))
}

/// `stream_isatty(resource $stream): bool` — true only for the process's own
/// standard handles, and only when they really are a terminal.
fn stream_isatty(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let sink = with_stream(ctx, &stream, "stream_isatty", |s| s.sink)?;
    Ok(Value::Bool(match sink {
        Sink::Buffer => false,
        Sink::Stdout => rustix::termios::isatty(std::io::stdout()),
        Sink::Stderr => rustix::termios::isatty(std::io::stderr()),
    }))
}

/// `stream_is_local(resource|string $stream): bool`
///
/// php asks the *wrapper*, not the filesystem: a path with no scheme is
/// local, and so is one whose scheme is a local wrapper, whatever is
/// actually there. An open handle is local in the engine by construction —
/// there is no remote wrapper to open one with.
fn stream_is_local(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if matches!(&*args[0].deref(), Value::Resource(_)) {
        return with_stream(ctx, &args[0].clone(), "stream_is_local", |_| Value::Bool(true));
    }
    // The parameter is `resource|string`, and php's coercion — not a type
    // error — is what an array meets here.
    if matches!(&*args[0].deref(), Value::Array(_)) {
        ctx.warn("Array to string conversion")?;
    }
    let name = args[0].to_php_bytes();
    let lower = name.to_ascii_lowercase();
    let scheme = match lower.windows(3).position(|w| w == b"://") {
        Some(at) => &lower[..at],
        // php's wrapper scan also matches `data:`, whose payload follows the
        // colon directly.
        None if lower.starts_with(b"data:") => b"data".as_slice(),
        None => return Ok(Value::Bool(true)),
    };
    let remote = REMOTE_WRAPPERS
        .iter()
        .any(|w| w.as_bytes() == scheme)
        || scheme == b"data";
    Ok(Value::Bool(!remote))
}

/// `stream_copy_to_stream(resource $from, resource $to, ?int $length = null, int $offset = 0): int|false`
fn stream_copy_to_stream(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let length = args
        .get(2)
        .map(|v| v.deref().into_owned())
        .filter(|v| !matches!(v, Value::Null | Value::Uninit))
        .map(|v| v.to_int());
    let offset = args.get(3).map_or(0, Value::to_int);
    let from = args[0].clone();
    let data = with_stream(ctx, &from, "stream_copy_to_stream", |s| {
        if offset > 0 {
            s.seek(offset, 0);
        }
        let start = s.pos.min(s.buf.len());
        let end = match length {
            Some(n) if n >= 0 => (start + n as usize).min(s.buf.len()),
            _ => s.buf.len(),
        };
        s.pos = end;
        s.eof = length.is_none_or(|n| end - start < n as usize);
        s.buf[start..end].to_vec()
    })?;
    let to = args[1].clone();
    let sink = with_stream(ctx, &to, "stream_copy_to_stream", |s| {
        if s.sink == Sink::Buffer {
            s.write(&data);
        }
        s.sink
    })?;
    match sink {
        Sink::Stdout => ctx.echo(&data),
        Sink::Stderr => eprint!("{}", String::from_utf8_lossy(&data)),
        Sink::Buffer => {}
    }
    Ok(Value::Int(data.len() as i64))
}
