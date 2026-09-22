//! The `file` half of ext/standard: path arithmetic and whole-file I/O.
//!
//! The path functions (`dirname`, `basename`, `pathinfo`) live in
//! `string2.rs`, where php also groups them — they are pure string
//! arithmetic and never touch the filesystem.
//!
//! Reads and writes here go through the stat cache in `filestat.rs`, which
//! they empty whenever they change a path.

use std::fs;
use std::io::Write;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

use crate::filestat::{arg_path, clear_stat_cache};

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
    nf!("stream_get_line", 2, Some(3), stream_get_line),
    nf!("stream_get_meta_data", 1, Some(1), stream_get_meta_data),
    nf!("stream_set_blocking", 2, Some(2), stream_set_blocking),
    nf!("stream_context_create", 0, Some(2), stream_context_create),
    nf!("stream_context_get_options", 1, Some(1), stream_context_get_options),
    nf!("stream_context_set_option", 2, Some(4), stream_context_set_option),
    nf!("stream_context_set_options", 2, Some(2), stream_context_set_options),
    nf!("stream_context_get_params", 1, Some(1), stream_context_get_params),
    nf!("stream_context_set_params", 2, Some(2), stream_context_set_params),
    nf!("stream_context_get_default", 0, Some(1), stream_context_get_default),
    nf!("stream_context_set_default", 1, Some(1), stream_context_set_default),
    rphp_runtime::nf_ref!("stream_select", 4, Some(5), 0b111, stream_select),
    nf!("stream_set_chunk_size", 2, Some(2), stream_set_chunk_size),
    nf!("stream_set_write_buffer", 2, Some(2), stream_set_write_buffer),
    nf!("stream_set_read_buffer", 2, Some(2), stream_set_read_buffer),
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
/// the whole implementation one code path — except a file opened to append
/// only (`a`, a log), whose writes go straight to the file: reading a
/// growing log in whole to append a line made every request O(log size),
/// and two writers would have overwritten each other's lines. `php://stdout`/
/// `stderr`/`output` forward writes to the engine's output channel instead.
pub(crate) struct Stream {
    buf: Vec<u8>,
    pos: usize,
    /// An append-only file (`fopen(…, 'a')`): every write lands in it at
    /// once, as php's `O_APPEND` handle does; `pos` counts the bytes written.
    file: Option<fs::File>,
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
    /// `stream_set_chunk_size()`: what the function reports back; the
    /// buffer-backed reads here do not chunk.
    chunk_size: i64,
    /// A live descriptor (a `proc_open` pipe, a socket): reads and writes go
    /// straight to it, nothing is buffered, and the cursor means nothing.
    pipe: Option<Pipe>,
    /// What a socket stream knows about itself beyond its descriptor
    /// (`socket.rs`); `None` for everything that is not a socket.
    pub(crate) sock: Option<Sock>,
}

/// The descriptor behind a pipe or socket stream.
pub(crate) struct Pipe {
    pub(crate) conn: Conn,
    /// `stream_set_blocking(false)`: `O_NONBLOCK` is set on the descriptor.
    pub(crate) nonblocking: bool,
    /// Bytes already taken off the descriptor that reads must serve before
    /// they go back to it — the body the http wrapper read while it was
    /// looking for the end of the headers. php counts what is left of this
    /// as a stream's `unread_bytes`.
    pub(crate) prefix: Vec<u8>,
}

/// What a live descriptor actually is.
///
/// Everything here is a descriptor, so one `read(2)`/`write(2)`/`poll(2)`
/// path serves all of it; the variants are kept apart only because the
/// socket calls that are *not* reads and writes — the peer's name, a
/// half-close, a datagram's address, a receive timeout — are std's, and
/// reaching them through a raw `sockaddr` would buy nothing but `unsafe`.
pub(crate) enum Conn {
    /// A `proc_open` pipe, or any other plain descriptor.
    File(fs::File),
    Tcp(std::net::TcpStream),
    TcpListen(std::net::TcpListener),
    Udp(std::net::UdpSocket),
    Unix(std::os::unix::net::UnixStream),
    UnixListen(std::os::unix::net::UnixListener),
    UnixDgram(std::os::unix::net::UnixDatagram),
}

impl Conn {
    /// The descriptor, borrowed — what `poll(2)` and `fcntl(2)` want.
    pub(crate) fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        match self {
            Conn::File(f) => f.as_fd(),
            Conn::Tcp(s) => s.as_fd(),
            Conn::TcpListen(s) => s.as_fd(),
            Conn::Udp(s) => s.as_fd(),
            Conn::Unix(s) => s.as_fd(),
            Conn::UnixListen(s) => s.as_fd(),
            Conn::UnixDgram(s) => s.as_fd(),
        }
    }

    /// A second handle on the same descriptor (`dup(2)`).
    pub(crate) fn try_clone(&self) -> std::io::Result<Conn> {
        Ok(match self {
            Conn::File(f) => Conn::File(f.try_clone()?),
            Conn::Tcp(s) => Conn::Tcp(s.try_clone()?),
            Conn::TcpListen(s) => Conn::TcpListen(s.try_clone()?),
            Conn::Udp(s) => Conn::Udp(s.try_clone()?),
            Conn::Unix(s) => Conn::Unix(s.try_clone()?),
            Conn::UnixListen(s) => Conn::UnixListen(s.try_clone()?),
            Conn::UnixDgram(s) => Conn::UnixDgram(s.try_clone()?),
        })
    }

    /// One `read(2)`. A listening socket has nothing to read, which is what
    /// php's `fread()` on one comes back with too.
    pub(crate) fn read_once(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::Read;
        match self {
            Conn::File(f) => f.read(buf),
            Conn::Tcp(s) => s.read(buf),
            Conn::Udp(s) => s.recv(buf),
            Conn::Unix(s) => s.read(buf),
            Conn::UnixDgram(s) => s.recv(buf),
            Conn::TcpListen(_) | Conn::UnixListen(_) => Ok(0),
        }
    }

    /// One `write(2)`, repeated until everything is gone.
    pub(crate) fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        match self {
            Conn::File(f) => f.write_all(data),
            Conn::Tcp(s) => s.write_all(data),
            Conn::Udp(s) => s.send(data).map(|_| ()),
            Conn::Unix(s) => s.write_all(data),
            Conn::UnixDgram(s) => s.send(data).map(|_| ()),
            Conn::TcpListen(_) | Conn::UnixListen(_) => {
                Err(std::io::Error::from(std::io::ErrorKind::InvalidInput))
            }
        }
    }
}

impl Pipe {
    /// Set or clear `O_NONBLOCK`.
    fn set_blocking(&mut self, blocking: bool) -> bool {
        let fd = self.conn.as_fd();
        let Ok(mut flags) = rustix::fs::fcntl_getfl(fd) else { return false };
        flags.set(rustix::fs::OFlags::NONBLOCK, !blocking);
        if rustix::fs::fcntl_setfl(fd, flags).is_err() {
            return false;
        }
        self.nonblocking = !blocking;
        true
    }

    /// One `read(2)` of up to `len` bytes: `Ok(None)` when nothing is there
    /// yet — a non-blocking descriptor with no data, or a blocking one whose
    /// receive timeout ran out — and `Ok(Some(empty))` at end of stream.
    fn read(&mut self, len: usize) -> std::io::Result<Option<Vec<u8>>> {
        if !self.prefix.is_empty() {
            let n = len.min(self.prefix.len());
            return Ok(Some(self.prefix.drain(..n).collect()));
        }
        let mut buf = vec![0u8; len.max(1)];
        loop {
            match self.conn.read_once(&mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    return Ok(Some(buf));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                // macOS reports an expired `SO_RCVTIMEO` as `EAGAIN`, Linux
                // as `ETIMEDOUT`; both mean "nothing, and not the end".
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }
}

/// What a socket stream knows beyond its descriptor.
pub(crate) struct Sock {
    /// Which `stream_type` `stream_get_meta_data()` reports.
    pub(crate) kind: SockKind,
    /// The address the stream was opened under, as given — php reports it
    /// back as `uri`, unresolved. A `stream_socket_pair()` end has none, and
    /// php's metadata then has no `uri` key at all.
    pub(crate) uri: Option<Box<str>>,
    /// `stream_set_timeout()`: how long a blocking read waits before it
    /// gives up. `None` is php's default, which is `default_socket_timeout`.
    pub(crate) timeout: Option<std::time::Duration>,
    /// Set by a read that ran that timeout out, reported by
    /// `stream_get_meta_data()` and cleared by the next successful read,
    /// exactly as php's `EOF`-independent flag behaves.
    pub(crate) timed_out: bool,
    /// The wrapper that opened this socket, when one did: its name, and the
    /// response's header lines. php reports them as `wrapper_type` and
    /// `wrapper_data`, which is where Symfony's `NativeHttpClient` reads a
    /// response's headers from.
    pub(crate) wrapper: Option<(Box<str>, Array)>,
}

/// php's `stream_type` for a socket, which names the transport rather than
/// the wrapper. `tcp_socket/ssl` is what a plain tcp stream reports too:
/// php names the transport by the module that registered it, and openssl
/// registers over tcp.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SockKind {
    Tcp,
    Udp,
    Unix,
    UnixDgram,
    /// A `stream_socket_pair()` end, which php leaves generic.
    Generic,
}

impl SockKind {
    pub(crate) fn stream_type(self) -> &'static str {
        match self {
            SockKind::Tcp => "tcp_socket/ssl",
            SockKind::Udp => "udp_socket",
            SockKind::Unix => "unix_socket",
            SockKind::UnixDgram => "udg_socket",
            SockKind::Generic => "generic_socket",
        }
    }
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
        if let Some(f) = &mut self.file {
            use std::io::Write;
            if f.write_all(data).is_err() {
                return 0;
            }
            self.pos += data.len();
            return data.len();
        }
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
    if let Some(url) = wrapper_url(&args[0]) {
        // `file_get_contents($f, $use_include_path, $context, $offset, $length)`
        let context = args.get(2).cloned();
        return Ok(match crate::http::read_all(ctx, &url, context.as_ref(), "file_get_contents")? {
            Some(b) => Value::Str(Str::from_vec(b)),
            None => Value::Bool(false),
        });
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
    // The output wrappers: `php://stdout`/`output` is the engine's output
    // channel, `php://stderr` the process's.
    match args[0].to_php_bytes().as_slice() {
        b"php://stdout" | b"php://output" => {
            ctx.echo(&data);
            return Ok(Value::Int(data.len() as i64));
        }
        b"php://stderr" => {
            use std::io::Write;
            let _ = std::io::stderr().write_all(&data);
            return Ok(Value::Int(data.len() as i64));
        }
        _ => {}
    }
    let p = arg_path(ctx, &args[0]);
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
    clear_stat_cache(ctx);
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
    let flags = args.get(1).map_or(0, Value::to_int);
    if let Some(url) = wrapper_url(&args[0]) {
        let context = args.get(2).cloned();
        return Ok(match crate::http::read_all(ctx, &url, context.as_ref(), "file")? {
            Some(b) => {
                let mut a = Array::new();
                for line in split_lines(&b, flags) {
                    a.push(Value::Str(Str::from_vec(line)));
                }
                Value::Array(a)
            }
            None => Value::Bool(false),
        });
    }
    let p = arg_path(ctx, &args[0]);
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
    if let Some(url) = wrapper_url(&args[0]) {
        let context = args.get(2).cloned();
        return Ok(match crate::http::read_all(ctx, &url, context.as_ref(), "readfile")? {
            Some(b) => {
                let n = b.len() as i64;
                ctx.echo(&b);
                Value::Int(n)
            }
            None => Value::Bool(false),
        });
    }
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
    clear_stat_cache(ctx);
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
    clear_stat_cache(ctx);
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
    clear_stat_cache(ctx);
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
    clear_stat_cache(ctx);
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
    // `streamsfuncs.c`'s `STREAM_*` constants, php's values on macOS
    // (`STREAM_PF_INET6` is the one platform-dependent value: 30 here, 10
    // on Linux — cataloged).
    for (name, v) in [
        ("STREAM_NOTIFY_CONNECT", 2),
        ("STREAM_NOTIFY_AUTH_REQUIRED", 3),
        ("STREAM_NOTIFY_AUTH_RESULT", 10),
        ("STREAM_NOTIFY_MIME_TYPE_IS", 4),
        ("STREAM_NOTIFY_FILE_SIZE_IS", 5),
        ("STREAM_NOTIFY_REDIRECTED", 6),
        ("STREAM_NOTIFY_PROGRESS", 7),
        ("STREAM_NOTIFY_FAILURE", 9),
        ("STREAM_NOTIFY_COMPLETED", 8),
        ("STREAM_NOTIFY_RESOLVE", 1),
        ("STREAM_NOTIFY_SEVERITY_INFO", 0),
        ("STREAM_NOTIFY_SEVERITY_WARN", 1),
        ("STREAM_NOTIFY_SEVERITY_ERR", 2),
        ("STREAM_FILTER_READ", 1),
        ("STREAM_FILTER_WRITE", 2),
        ("STREAM_FILTER_ALL", 3),
        ("STREAM_CLIENT_PERSISTENT", 1),
        ("STREAM_CLIENT_ASYNC_CONNECT", 2),
        ("STREAM_CLIENT_CONNECT", 4),
        ("STREAM_CRYPTO_METHOD_ANY_CLIENT", 127),
        ("STREAM_CRYPTO_METHOD_SSLv2_CLIENT", 3),
        ("STREAM_CRYPTO_METHOD_SSLv3_CLIENT", 5),
        ("STREAM_CRYPTO_METHOD_SSLv23_CLIENT", 57),
        ("STREAM_CRYPTO_METHOD_TLS_CLIENT", 121),
        ("STREAM_CRYPTO_METHOD_TLSv1_0_CLIENT", 9),
        ("STREAM_CRYPTO_METHOD_TLSv1_1_CLIENT", 17),
        ("STREAM_CRYPTO_METHOD_TLSv1_2_CLIENT", 33),
        ("STREAM_CRYPTO_METHOD_TLSv1_3_CLIENT", 65),
        ("STREAM_CRYPTO_METHOD_ANY_SERVER", 126),
        ("STREAM_CRYPTO_METHOD_SSLv2_SERVER", 2),
        ("STREAM_CRYPTO_METHOD_SSLv3_SERVER", 4),
        ("STREAM_CRYPTO_METHOD_SSLv23_SERVER", 120),
        ("STREAM_CRYPTO_METHOD_TLS_SERVER", 120),
        ("STREAM_CRYPTO_METHOD_TLSv1_0_SERVER", 8),
        ("STREAM_CRYPTO_METHOD_TLSv1_1_SERVER", 16),
        ("STREAM_CRYPTO_METHOD_TLSv1_2_SERVER", 32),
        ("STREAM_CRYPTO_METHOD_TLSv1_3_SERVER", 64),
        ("STREAM_CRYPTO_PROTO_SSLv3", 4),
        ("STREAM_CRYPTO_PROTO_TLSv1_0", 8),
        ("STREAM_CRYPTO_PROTO_TLSv1_1", 16),
        ("STREAM_CRYPTO_PROTO_TLSv1_2", 32),
        ("STREAM_CRYPTO_PROTO_TLSv1_3", 64),
        ("STREAM_SHUT_RD", 0),
        ("STREAM_SHUT_WR", 1),
        ("STREAM_SHUT_RDWR", 2),
        ("STREAM_PF_INET", 2),
        ("STREAM_PF_INET6", 30),
        ("STREAM_PF_UNIX", 1),
        ("STREAM_IPPROTO_IP", 0),
        ("STREAM_IPPROTO_TCP", 6),
        ("STREAM_IPPROTO_UDP", 17),
        ("STREAM_IPPROTO_ICMP", 1),
        ("STREAM_IPPROTO_RAW", 255),
        ("STREAM_SOCK_STREAM", 1),
        ("STREAM_SOCK_DGRAM", 2),
        ("STREAM_SOCK_RAW", 3),
        ("STREAM_SOCK_SEQPACKET", 5),
        ("STREAM_SOCK_RDM", 4),
        ("STREAM_PEEK", 2),
        ("STREAM_OOB", 1),
        ("STREAM_SERVER_BIND", 4),
        ("STREAM_SERVER_LISTEN", 8),
        ("STREAM_USE_PATH", 1),
        ("STREAM_IGNORE_URL", 2),
        ("STREAM_REPORT_ERRORS", 8),
        ("STREAM_MUST_SEEK", 16),
        ("STREAM_URL_STAT_LINK", 1),
        ("STREAM_URL_STAT_QUIET", 2),
        ("STREAM_MKDIR_RECURSIVE", 1),
        ("STREAM_IS_URL", 1),
        ("STREAM_OPTION_BLOCKING", 1),
        ("STREAM_OPTION_READ_TIMEOUT", 4),
        ("STREAM_OPTION_READ_BUFFER", 2),
        ("STREAM_OPTION_WRITE_BUFFER", 3),
        ("STREAM_BUFFER_NONE", 0),
        ("STREAM_BUFFER_LINE", 1),
        ("STREAM_BUFFER_FULL", 2),
        ("STREAM_CAST_AS_STREAM", 0),
        ("STREAM_CAST_FOR_SELECT", 3),
        ("STREAM_META_TOUCH", 1),
        ("STREAM_META_OWNER", 3),
        ("STREAM_META_OWNER_NAME", 2),
        ("STREAM_META_GROUP", 5),
        ("STREAM_META_GROUP_NAME", 4),
        ("STREAM_META_ACCESS", 6),
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
                rphp_runtime::value_name(&other)
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
    // The network wrappers open a connection rather than a path.
    if crate::http::is_http_url(&path_str) {
        let context = args.get(3).cloned();
        return Ok(crate::http::open(ctx, &path_str, context.as_ref(), "fopen")?
            .unwrap_or(Value::Bool(false)));
    }
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
            file: None,
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
            chunk_size: 8192,
            pipe: None,
            sock: None,
        }
    } else {
        let p = arg_path(ctx, &args[0]);
        // Append-only: the file itself, nothing read.
        let file = if append && !readable {
            match fs::OpenOptions::new().append(true).create(true).open(&p) {
                Ok(f) => Some(f),
                Err(_) => {
                    ctx.warn(&format!(
                        "fopen({path_str}): Failed to open stream: No such file or directory"
                    ))?;
                    return Ok(Value::Bool(false));
                }
            }
        } else {
            None
        };
        let existing = if truncate || file.is_some() { Ok(Vec::new()) } else { fs::read(&p) };
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
        // php starts an `a+` handle's read position at the beginning; a
        // write moves it to the end.
        let pos = 0;
        let dirty = writable && (truncate || append) && file.is_none();
        Stream {
            buf,
            pos,
            file,
            path: Some(p.clone()),
            append,
            sink: Sink::Buffer,
            readable,
            writable,
            eof: false,
            // A `w`/`a+` open creates the file even with nothing written.
            dirty,
            mode: m.clone().into(),
            uri: p.to_string_lossy().into_owned().into(),
            fill_end: pos,
            chunk_size: 8192,
            pipe: None,
            sock: None,
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
        file: None,
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
        chunk_size: 8192,
        pipe: None,
        sock: None,
    };
    ctx.resources.add("stream", Box::new(stream))
}

/// A stream over one end of a pipe (`proc_open`): `readable` for the
/// child's stdout/stderr, writable for its stdin. Blocking to begin with,
/// as php's are.
pub(crate) fn pipe_resource(ctx: &mut Ctx, file: fs::File, readable: bool) -> Value {
    let stream = Stream {
        buf: Vec::new(),
        pos: 0,
        file: None,
        path: None,
        append: false,
        sink: Sink::Buffer,
        readable,
        writable: !readable,
        eof: false,
        dirty: false,
        mode: if readable { "r".into() } else { "w".into() },
        uri: "php://stdio".into(),
        fill_end: 0,
        chunk_size: 8192,
        pipe: Some(Pipe { conn: Conn::File(file), nonblocking: false, prefix: Vec::new() }),
        sock: None,
    };
    ctx.resources.add("stream", Box::new(stream))
}

/// Whether the first argument names a url the http wrapper handles, and
/// the url if so. The wrappers that read from the network are the only
/// place a path is not a path.
fn wrapper_url(v: &Value) -> Option<String> {
    let bytes = v.to_php_bytes();
    let url = String::from_utf8_lossy(&bytes);
    crate::http::is_http_url(&url).then(|| url.into_owned())
}

/// A stream context's options, for a wrapper to read its own section out
/// of. Anything that is not a context resource simply has none.
pub(crate) fn context_options_of(_ctx: &mut Ctx, v: &Value) -> Option<Array> {
    let Value::Resource(r) = &*v.deref() else { return None };
    if r.kind() != "stream-context" {
        return None;
    }
    let mut payload = r.payload_mut();
    let c = payload.as_mut()?.downcast_mut::<StreamContext>()?;
    Some(c.options.clone())
}

/// Read a stream to its end — what `file_get_contents()` does with a
/// wrapper's handle once the headers are off it.
pub(crate) fn drain_stream(ctx: &mut Ctx, v: &Value) -> Vec<u8> {
    match pipe_read(ctx, v, "file_get_contents", PipeRead::All(None)) {
        Ok(Some(b)) => b,
        _ => Vec::new(),
    }
}

/// A stream over a socket, which is what every `stream_socket_*` function
/// and `fsockopen()` hands back (`socket.rs`). php opens a transport `r+`
/// and blocking, and its cursor means nothing.
pub(crate) fn socket_resource(
    ctx: &mut Ctx,
    conn: Conn,
    kind: SockKind,
    uri: Option<String>,
) -> Value {
    socket_resource_with(ctx, conn, kind, uri, Vec::new(), None)
}

/// `socket_resource` for a socket a wrapper opened: `prefix` is what it
/// already read off the descriptor, and `wrapper` what php reports as
/// `wrapper_type`/`wrapper_data`.
pub(crate) fn socket_resource_with(
    ctx: &mut Ctx,
    conn: Conn,
    kind: SockKind,
    uri: Option<String>,
    prefix: Vec<u8>,
    wrapper: Option<(Box<str>, Array)>,
) -> Value {
    // A stream a wrapper opened is read-only; a bare socket is `r+`.
    let mode = if wrapper.is_some() { "r" } else { "r+" };
    let stream = Stream {
        buf: Vec::new(),
        pos: 0,
        file: None,
        path: None,
        append: false,
        sink: Sink::Buffer,
        readable: true,
        writable: true,
        eof: false,
        dirty: false,
        mode: mode.into(),
        uri: uri.clone().unwrap_or_default().into(),
        fill_end: 0,
        chunk_size: 8192,
        pipe: Some(Pipe { conn, nonblocking: false, prefix }),
        sock: Some(Sock {
            kind,
            uri: uri.map(Into::into),
            timeout: None,
            timed_out: false,
            wrapper,
        }),
    };
    ctx.resources.add("stream", Box::new(stream))
}

/// Run `f` over the socket behind a resource; `Ok(None)` when the resource
/// is a stream but not a socket, which is php's "not a socket" answer for
/// every `stream_socket_*` function.
pub(crate) fn with_socket<R>(
    ctx: &mut Ctx,
    v: &Value,
    func: &str,
    f: impl FnOnce(&mut Conn, &mut Sock) -> R,
) -> Result<Option<R>, Unwind> {
    with_stream(ctx, v, func, |s| {
        let (Some(p), Some(sock)) = (s.pipe.as_mut(), s.sock.as_mut()) else {
            return None;
        };
        Some(f(&mut p.conn, sock))
    })
}

/// A duplicate of the descriptor behind a *pipe* stream, for handing it on
/// to a child: `None` for anything else, sockets included, since a child
/// inherits a pipe and reopens everything else.
pub(crate) fn pipe_dup(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Option<fs::File>, Unwind> {
    with_stream(ctx, v, func, |s| match s.pipe.as_ref().map(|p| &p.conn) {
        Some(Conn::File(f)) => f.try_clone().ok(),
        _ => None,
    })
}

/// A duplicate of the descriptor behind *any* live stream — a pipe or a
/// socket — for `stream_select` to poll.
fn conn_dup(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Option<Conn>, Unwind> {
    with_stream(ctx, v, func, |s| s.pipe.as_ref().and_then(|p| p.conn.try_clone().ok()))
}

/// Whether a stream is one of the process's output handles (`php://stdout`,
/// `php://stderr`), which a child inherits rather than receives a copy of.
pub(crate) fn std_sink(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Option<bool>, Unwind> {
    with_stream(ctx, v, func, |s| match s.sink {
        Sink::Stdout => Some(false),
        Sink::Stderr => Some(true),
        Sink::Buffer => None,
    })
}

/// The path behind a real-file stream, for a child to reopen.
pub(crate) fn stream_path(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Option<std::path::PathBuf>, Unwind> {
    with_stream(ctx, v, func, |s| s.path.clone())
}

/// A `php://` handle for one of the process's standard streams.
fn std_stream(sink: Sink, uri: &str, mode: &str) -> Stream {
    Stream {
        buf: Vec::new(),
        pos: 0,
        file: None,
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
        chunk_size: 8192,
        pipe: None,
        sock: None,
    }
}

/// Write a stream's buffer back to its file, if it has one.
fn flush_stream(s: &Stream) -> Option<std::path::PathBuf> {
    match (&s.path, s.dirty) {
        (Some(p), true) => {
            let _ = fs::write(p, &s.buf);
            // The file on disk just changed: whoever asked about this path
            // before the write must not be answered from the stat cache.
            Some(p.clone())
        }
        _ => None,
    }
}

/// `fclose(resource $stream): bool`
fn fclose(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let written = with_stream(ctx, &args[0].clone(), "fclose", |s| {
        let written = flush_stream(s);
        // The child sees EOF on its stdin the moment the pipe closes.
        s.pipe = None;
        written
    })?;
    if written.is_some() {
        clear_stat_cache(ctx);
    }
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
    // A pipe: the bytes go to the descriptor now; a closed reader (EPIPE)
    // is php's `false` with a notice.
    let piped = with_stream(ctx, &stream, "fwrite", |s| {
        let p = s.pipe.as_mut()?;
        if !s.writable {
            return Some(Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        }
        Some(p.conn.write_all(&data).map(|()| data.len()))
    })?;
    if let Some(r) = piped {
        return match r {
            Ok(n) => Ok(Value::Int(n as i64)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                ctx.notice(&format!("fwrite(): Write of {} bytes failed with errno=9 Bad file descriptor", data.len()))?;
                Ok(Value::Bool(false))
            }
            Err(_) => {
                ctx.notice(&format!("fwrite(): Write of {} bytes failed with errno=32 Broken pipe", data.len()))?;
                Ok(Value::Bool(false))
            }
        };
    }
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

/// What a read function does with a pipe stream, when the stream is one.
enum PipeRead {
    /// Up to this many bytes, one `read(2)`.
    Some(usize),
    /// A line up to and including `\n`.
    Line,
    /// Everything up to end of stream (or, non-blocking, up to "nothing yet").
    All(Option<usize>),
}

/// Read from a pipe stream; `Ok(None)` when the stream is not a pipe. A
/// read that comes back empty at end of stream sets `eof`; a non-blocking
/// read with nothing yet answers the empty string without setting it.
fn pipe_read(ctx: &mut Ctx, v: &Value, func: &str, how: PipeRead) -> Result<Option<Vec<u8>>, Unwind> {
    with_stream(ctx, v, func, |s| {
        let p = s.pipe.as_mut()?;
        // A *blocking* descriptor that comes back with nothing did not find
        // the stream empty — it ran its receive timeout out. That is all
        // php's `timed_out` means, and a read that gets somewhere clears it.
        let blocking = !p.nonblocking;
        let mut starved = false;
        let mut out = Vec::new();
        match how {
            PipeRead::Some(len) => match p.read(len) {
                Ok(Some(b)) => {
                    if b.is_empty() && len > 0 {
                        s.eof = true;
                    }
                    out = b;
                }
                Ok(None) => starved = true,
                Err(_) => s.eof = true,
            },
            PipeRead::Line => loop {
                match p.read(1) {
                    Ok(Some(b)) if !b.is_empty() => {
                        out.push(b[0]);
                        if b[0] == b'\n' {
                            break;
                        }
                    }
                    Ok(Some(_)) | Err(_) => {
                        s.eof = true;
                        break;
                    }
                    Ok(None) => {
                        starved = true;
                        break;
                    }
                }
            },
            PipeRead::All(max) => loop {
                let want = match max {
                    Some(m) if out.len() >= m => break,
                    Some(m) => (m - out.len()).min(CHUNK),
                    None => CHUNK,
                };
                match p.read(want) {
                    Ok(Some(b)) if !b.is_empty() => out.extend_from_slice(&b),
                    Ok(Some(_)) | Err(_) => {
                        s.eof = true;
                        break;
                    }
                    Ok(None) => {
                        starved = true;
                        break;
                    }
                }
            },
        }
        if let Some(sock) = s.sock.as_mut() {
            sock.timed_out = starved && blocking && out.is_empty();
        }
        Some(out)
    })
}

fn fread(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let len = args[1].to_int().max(0) as usize;
    let stream = args[0].clone();
    if read_denied(ctx, &stream, "fread")? {
        return Ok(Value::Bool(false));
    }
    if let Some(b) = pipe_read(ctx, &stream, "fread", PipeRead::Some(len.min(CHUNK)))? {
        return Ok(Value::Str(Str::from_vec(b)));
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
    if let Some(b) = pipe_read(ctx, &stream, "fgets", PipeRead::Line)? {
        return Ok(if b.is_empty() { Value::Bool(false) } else { Value::Str(Str::from_vec(b)) });
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

/// `stream_get_line(resource $stream, int $length, string $ending = ""): string|false`
///
/// `fgets()` with a delimiter of any length, which — unlike `fgets()`'s
/// newline — is *not* part of what comes back. A length of `0` means php's
/// default chunk. `false` only when there was nothing left at all.
fn stream_get_line(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let max = args[1].to_int();
    if max < 0 {
        return Err(Unwind::value_error(
            "stream_get_line(): Argument #2 ($length) must be greater than or equal to 0",
        ));
    }
    let ending = args.get(2).map(|v| v.to_php_bytes().to_vec()).unwrap_or_default();
    let ending = if ending.is_empty() { b"\n".to_vec() } else { ending };
    let cap = if max == 0 { CHUNK } else { max as usize };
    if read_denied(ctx, &stream, "stream_get_line")? {
        return Ok(Value::Bool(false));
    }
    // A live descriptor has to be read up to the delimiter and no further:
    // anything taken past it belongs to the next call.
    let piped = with_stream(ctx, &stream, "stream_get_line", |s| s.pipe.is_some())?;
    if piped {
        let mut out: Vec<u8> = Vec::new();
        while out.len() < cap {
            let Some(b) = pipe_read(ctx, &stream, "stream_get_line", PipeRead::Some(1))? else {
                break;
            };
            if b.is_empty() {
                break;
            }
            out.push(b[0]);
            if out.ends_with(&ending) {
                out.truncate(out.len() - ending.len());
                return Ok(Value::Str(Str::from_vec(out)));
            }
        }
        return Ok(if out.is_empty() {
            Value::Bool(false)
        } else {
            Value::Str(Str::from_vec(out))
        });
    }
    with_stream(ctx, &stream, "stream_get_line", |s| {
        record_fill(s);
        if s.pos >= s.buf.len() {
            s.eof = true;
            return Value::Bool(false);
        }
        let start = s.pos;
        let limit = (start + cap).min(s.buf.len());
        let hay = &s.buf[start..limit];
        match hay
            .windows(ending.len())
            .position(|w| w == &ending[..])
        {
            Some(i) => {
                s.pos = start + i + ending.len();
                Value::Str(Str::from_vec(hay[..i].to_vec()))
            }
            None => {
                s.pos = limit;
                s.eof = limit == s.buf.len();
                Value::Str(Str::from_vec(hay.to_vec()))
            }
        }
    })
}

/// `fgetc(resource $stream): string|false`
fn fgetc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    if read_denied(ctx, &stream, "fgetc")? {
        return Ok(Value::Bool(false));
    }
    if let Some(b) = pipe_read(ctx, &stream, "fgetc", PipeRead::Some(1))? {
        return Ok(if b.is_empty() { Value::Bool(false) } else { Value::Str(Str::from_vec(b)) });
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
    with_stream(ctx, &stream, "feof", |s| {
        // A socket is asked, not remembered: php checks the peer is still
        // there (`socket::socket_eof`) rather than waiting for a read to
        // come back short, so `feof()` is true as soon as the other end
        // closes. Every other stream keeps the flag its reads set.
        if !s.eof && s.sock.is_some() {
            if let Some(p) = s.pipe.as_ref() {
                s.eof = crate::socket::socket_eof(&p.conn);
            }
        }
        Value::Bool(s.eof)
    })
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
    let written = with_stream(ctx, &stream, "fflush", |s| flush_stream(s))?;
    if written.is_some() {
        clear_stat_cache(ctx);
    }
    Ok(Value::Bool(true))
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
    if let Some(b) = pipe_read(ctx, &stream, "stream_get_contents", PipeRead::All(max.filter(|n| *n >= 0).map(|n| n as usize)))? {
        return Ok(Value::Str(Str::from_vec(b)));
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
        // A socket's metadata is php's *without* a `wrapper_type`: a
        // transport is registered as a transport, not as a wrapper, so
        // `_php_stream_populate_meta_data` never fills that key. A pair has
        // no address either, so it has no `uri` key.
        if let Some(sock) = &s.sock {
            let mut out = Array::new();
            let mut set = |k: &str, v: Value| out.set(ArrayKey::str(k.as_bytes()), v);
            set("timed_out", Value::Bool(sock.timed_out));
            set("blocked", Value::Bool(s.pipe.as_ref().is_none_or(|p| !p.nonblocking)));
            set("eof", Value::Bool(s.eof));
            if let Some((name, data)) = &sock.wrapper {
                set("wrapper_data", Value::Array(data.clone()));
                set("wrapper_type", Value::string(name.as_bytes()));
            }
            set("stream_type", Value::string(sock.kind.stream_type().as_bytes()));
            set("mode", Value::string(s.mode.as_bytes()));
            // What a wrapper read ahead and has not handed out yet.
            let pending = s.pipe.as_ref().map_or(0, |p| p.prefix.len());
            set("unread_bytes", Value::Int(pending as i64));
            set("seekable", Value::Bool(false));
            if let Some(uri) = &sock.uri {
                set("uri", Value::string(uri.as_bytes()));
            }
            return Value::Array(out);
        }
        let php_stream = s.path.is_none();
        let (wrapper, kind, seekable) = if s.pipe.is_some() {
            ("PHP", "STDIO".to_string(), false)
        } else if !php_stream {
            ("plainfile", "STDIO".to_string(), true)
        } else if s.sink != Sink::Buffer {
            ("PHP", "STDIO".to_string(), false)
        } else if s.uri.ends_with("temp") || s.uri.starts_with("php://temp/") {
            ("PHP", "TEMP".to_string(), true)
        } else {
            ("PHP", "MEMORY".to_string(), true)
        };
        let mut out = Array::new();
        let mut set = |k: &str, v: Value| out.set(ArrayKey::str(k.as_bytes()), v);
        set("timed_out", Value::Bool(false));
        set("blocked", Value::Bool(s.pipe.as_ref().is_none_or(|p| !p.nonblocking)));
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
    let blocking = args[1].to_bool();
    with_stream(ctx, &stream, "stream_set_blocking", |s| match s.pipe.as_mut() {
        Some(p) => Value::Bool(p.set_blocking(blocking)),
        None => Value::Bool(true),
    })
}

/// `stream_select(?array &$read, ?array &$write, ?array &$except, ?int $seconds, ?int $microseconds = null): int|false`
///
/// `poll(2)` over the pipe streams named; each array is narrowed to the
/// streams that are ready, keys kept. A buffer stream (a file, a memory
/// stream) is always ready, as its descriptor would be. `except` is
/// accepted and never fires.
fn stream_select(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    use rustix::event::{poll, PollFd, PollFlags};
    let timeout = match (&args[3], args.get(4)) {
        (Value::Null, _) => None,
        (secs, usecs) => {
            let s = secs.to_int().max(0);
            let us = usecs.filter(|v| !matches!(v, Value::Null)).map_or(0, Value::to_int).max(0);
            Some(rustix::fs::Timespec { tv_sec: s + us / 1_000_000, tv_nsec: (us % 1_000_000) * 1000 })
        }
    };
    // (array index, key, dup of the descriptor) for every pipe stream;
    // ready flags for the rest.
    let mut sets: [Vec<(ArrayKey, Value, bool)>; 2] = [Vec::new(), Vec::new()];
    let mut fds: Vec<(usize, usize, Conn)> = Vec::new();
    for (which, arg) in args.iter().take(2).enumerate() {
        let Value::Array(a) = &*arg.deref() else { continue };
        for (k, v) in a.iter() {
            let v = v.deref().into_owned();
            match conn_dup(ctx, &v, "stream_select")? {
                Some(f) => {
                    fds.push((which, sets[which].len(), f));
                    sets[which].push((k.clone(), v, false));
                }
                None => sets[which].push((k.clone(), v, true)),
            }
        }
    }
    let mut ready = sets.iter().map(|s| s.iter().filter(|e| e.2).count()).sum::<usize>();
    if !fds.is_empty() {
        let mut polls: Vec<PollFd<'_>> = fds
            .iter()
            .map(|(which, _, c)| {
                let want = if *which == 0 { PollFlags::IN | PollFlags::HUP } else { PollFlags::OUT };
                PollFd::from_borrowed_fd(c.as_fd(), want)
            })
            .collect();
        // Everything else being ready already, a poll with descriptors only
        // asks whether they are too.
        let wait = if ready > 0 { Some(rustix::fs::Timespec { tv_sec: 0, tv_nsec: 0 }) } else { timeout };
        loop {
            match poll(&mut polls, wait.as_ref()) {
                Ok(_) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(_) => {
                    ctx.warn("stream_select(): Unable to select")?;
                    return Ok(Value::Bool(false));
                }
            }
        }
        for (i, p) in polls.iter().enumerate() {
            let (which, at, _) = &fds[i];
            let hit = !(p.revents() & (PollFlags::IN | PollFlags::OUT | PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL)).is_empty();
            if hit {
                sets[*which][*at].2 = true;
                ready += 1;
            }
        }
    }
    for (which, set) in sets.iter().enumerate() {
        if !matches!(args[which], Value::Array(_)) {
            continue;
        }
        let mut out = Array::new();
        for (k, v, r) in set {
            if *r {
                out.set(k.clone(), v.clone());
            }
        }
        args[which] = Value::Array(out);
    }
    if matches!(args[2], Value::Array(_)) {
        args[2] = Value::Array(Array::new());
    }
    Ok(Value::Int(ready as i64))
}

/// `stream_set_chunk_size(resource $stream, int $size): int` — the previous
/// chunk size (8192 to begin with); php refuses a size below 1 or above
/// `INT_MAX`.
fn stream_set_chunk_size(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let size = args[1].to_int();
    if size <= 0 {
        return Err(Unwind::value_error(
            "stream_set_chunk_size(): Argument #2 ($size) must be greater than 0",
        ));
    }
    if size > i64::from(i32::MAX) {
        return Err(Unwind::value_error(
            "stream_set_chunk_size(): Argument #2 ($size) is too large",
        ));
    }
    with_stream(ctx, &stream, "stream_set_chunk_size", |s| {
        Value::Int(std::mem::replace(&mut s.chunk_size, size))
    })
}

/// `stream_set_write_buffer(resource $stream, int $size): int` — `0` when
/// the stream took the setting, `-1` when it has no write buffer to set.
/// php's plain-file streams answer `-1` unless they sit on a `FILE*`,
/// which only the process's standard handles do; memory streams answer
/// `-1` too.
fn stream_set_write_buffer(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let sink = with_stream(ctx, &stream, "stream_set_write_buffer", |s| s.sink)?;
    Ok(Value::Int(match sink {
        Sink::Stdout | Sink::Stderr => 0,
        Sink::Buffer => -1,
    }))
}

/// `stream_set_read_buffer(resource $stream, int $size): int` — the read
/// buffer is the stream layer's own, so every stream answers `0`.
fn stream_set_read_buffer(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    with_stream(ctx, &stream, "stream_set_read_buffer", |_| Value::Int(0))
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

/// A stream context: per-wrapper options and the parameters (`notification`,
/// `options`) `stream_context_get_params()` reports.
///
/// The local wrappers this engine serves take no options — a context is
/// accepted everywhere php accepts one and changes nothing — but scripts
/// build and inspect contexts constantly (Symfony's `Filesystem::copy` makes
/// one for every copy), so they round-trip exactly.
#[derive(Default, Clone)]
pub(crate) struct StreamContext {
    /// `wrapper => option => value`, in insertion order.
    options: Array,
    /// The parameters other than `options`; `notification` is the only one
    /// php defines.
    params: Array,
}

/// The `Interp::ext` slot the default context lives in.
const DEFAULT_CONTEXT_SLOT: &str = "stream.default-context";

#[derive(Default)]
struct DefaultContext(Option<Value>);

/// `$options` as php validates it: `wrapper => [option => value]`, every
/// wrapper an array.
fn context_options(ctx: &mut Ctx, func: &str, arg: Option<&Value>) -> Result<Array, Unwind> {
    let Some(v) = arg.filter(|v| !matches!(**v, Value::Null)) else { return Ok(Array::new()) };
    let _ = ctx;
    let Value::Array(a) = &*v.deref() else {
        return Err(Unwind::type_error(format!("{func}(): Argument #1 ($options) must be of type ?array, {} given", rphp_runtime::value_name(&v.deref()))));
    };
    for (_, wrapper) in a.iter() {
        if !matches!(&*wrapper.deref(), Value::Array(_)) {
            return Err(Unwind::type_error(format!("{func}(): Argument #1 ($options) must be an array with valid callbacks as values, no array or string given")));
        }
    }
    Ok(a.clone())
}

/// `$params`: php validates `notification` as a callable and refuses the
/// whole call — with the *options* message — when it is not one.
fn context_params(ctx: &mut Ctx, func: &str, arg: Option<&Value>) -> Result<Array, Unwind> {
    let Some(v) = arg.filter(|v| !matches!(**v, Value::Null)) else { return Ok(Array::new()) };
    let Value::Array(a) = &*v.deref() else {
        return Err(Unwind::type_error(format!("{func}(): Argument #2 ($params) must be of type ?array, {} given", rphp_runtime::value_name(&v.deref()))));
    };
    if let Some(notification) = a.get_deref(&ArrayKey::str(b"notification")) {
        if !ctx.is_callable(&notification) {
            let shown = String::from_utf8_lossy(&notification.to_php_bytes()).into_owned();
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #1 ($options) must be an array with valid callbacks as values, function \"{shown}\" not found or invalid function name"
            )));
        }
    }
    Ok(a.clone())
}

/// Run `f` over the context behind a value — a context resource, or the
/// context a stream was opened with (always the empty one here).
fn with_context<R>(_ctx: &mut Ctx, v: &Value, func: &str, f: impl FnOnce(&mut StreamContext) -> R) -> Result<R, Unwind> {
    let resource = match &*v.deref() {
        Value::Resource(r) if r.kind() == "stream-context" => r.clone(),
        Value::Resource(r) if r.kind() == "stream" => {
            // A stream carries no context here: answer from a scratch one.
            let _ = r;
            return Ok(f(&mut StreamContext::default()));
        }
        other => {
            return Err(Unwind::type_error(format!(
                "{func}(): supplied resource is not a valid stream/context resource, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let mut payload = resource.payload_mut();
    let Some(c) = payload.as_mut().and_then(|a| a.downcast_mut::<StreamContext>()) else {
        return Err(Unwind::type_error(format!("{func}(): supplied resource is not a valid stream/context resource")));
    };
    Ok(f(c))
}

/// `stream_context_create(?array $options = null, ?array $params = null): resource`
fn stream_context_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let options = context_options(ctx, "stream_context_create", args.first())?;
    let params = context_params(ctx, "stream_context_create", args.get(1))?;
    Ok(ctx.resources.add("stream-context", Box::new(StreamContext { options, params })))
}

/// `stream_context_get_options(resource $stream_or_context): array`
fn stream_context_get_options(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v = args[0].clone();
    with_context(ctx, &v, "stream_context_get_options", |c| Value::Array(c.options.clone()))
}

/// Set `wrapper[option] = value` on a context.
fn set_option(c: &mut StreamContext, wrapper: &[u8], option: &[u8], value: Value) {
    let key = ArrayKey::str(wrapper);
    let mut inner = match c.options.get_deref(&key) {
        Some(Value::Array(a)) => a,
        _ => Array::new(),
    };
    inner.set(ArrayKey::str(option), value);
    c.options.set(key, Value::Array(inner));
}

/// Merge a whole `wrapper => [option => value]` map into a context.
fn set_options(c: &mut StreamContext, options: &Array) {
    for (wrapper, entries) in options.iter() {
        let ArrayKey::Str(name) = wrapper else { continue };
        if let Value::Array(entries) = &*entries.deref() {
            for (option, value) in entries.iter() {
                let option = match option {
                    ArrayKey::Str(s) => s.as_bytes().to_vec(),
                    ArrayKey::Int(i) => i.to_string().into_bytes(),
                };
                set_option(c, name.as_bytes(), &option, value.deref().into_owned());
            }
        }
    }
}

/// `stream_context_set_option(resource $context, array|string $wrapper_or_options, ?string $option_name = null, mixed $value = null): true`
///
/// The two-argument form (a whole options array) is deprecated in php 8.3 in
/// favour of `stream_context_set_options()`, and says so.
fn stream_context_set_option(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let target = args[0].clone();
    match &*args[1].deref() {
        Value::Array(options) => {
            let options = options.clone();
            ctx.deprecated("Calling stream_context_set_option() with 2 arguments is deprecated, use stream_context_set_options() instead")?;
            with_context(ctx, &target, "stream_context_set_option", |c| set_options(c, &options))?;
        }
        other => {
            let wrapper = other.to_php_bytes();
            let Some(option) = args.get(2).filter(|v| !matches!(**v, Value::Null)) else {
                return Err(Unwind::value_error("stream_context_set_option(): Argument #3 ($option_name) cannot be null when argument #2 ($wrapper_or_options) is a string"));
            };
            let option = option.to_php_bytes();
            let value = args.get(3).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
            with_context(ctx, &target, "stream_context_set_option", |c| set_option(c, &wrapper, &option, value))?;
        }
    }
    Ok(Value::Bool(true))
}

/// `stream_context_set_options(resource $context, array $options): true`
fn stream_context_set_options(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let target = args[0].clone();
    let Value::Array(options) = &*args[1].deref() else {
        return Err(Unwind::type_error("stream_context_set_options(): Argument #2 ($options) must be of type array"));
    };
    let options = options.clone();
    with_context(ctx, &target, "stream_context_set_options", |c| set_options(c, &options))?;
    Ok(Value::Bool(true))
}

/// `stream_context_get_params(resource $context): array` — the parameters
/// plus the options under the `options` key, as php reports them.
fn stream_context_get_params(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v = args[0].clone();
    with_context(ctx, &v, "stream_context_get_params", |c| {
        let mut out = c.params.clone();
        out.set(ArrayKey::str(b"options"), Value::Array(c.options.clone()));
        Value::Array(out)
    })
}

/// `stream_context_set_params(resource $context, array $params): true`
fn stream_context_set_params(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let target = args[0].clone();
    let params = context_params(ctx, "stream_context_set_params", args.get(1))?;
    with_context(ctx, &target, "stream_context_set_params", |c| {
        for (k, v) in params.iter() {
            c.params.set(k.clone(), v.deref().into_owned());
        }
    })?;
    Ok(Value::Bool(true))
}

/// `stream_context_get_default(?array $options = null): resource` — the
/// request's default context, created on first use; `$options` is merged in.
fn stream_context_get_default(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let options = context_options(ctx, "stream_context_get_default", args.first())?;
    let existing = ctx.ext.slot::<DefaultContext>(DEFAULT_CONTEXT_SLOT).0.clone();
    let context = match existing {
        Some(v) => v,
        None => {
            let v = ctx.resources.add("stream-context", Box::new(StreamContext::default()));
            ctx.ext.slot::<DefaultContext>(DEFAULT_CONTEXT_SLOT).0 = Some(v.clone());
            v
        }
    };
    if !options.is_empty() {
        with_context(ctx, &context, "stream_context_get_default", |c| set_options(c, &options))?;
    }
    Ok(context)
}

/// `stream_context_set_default(array $options): resource`
fn stream_context_set_default(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let options = context_options(ctx, "stream_context_set_default", args.first())?;
    let context = stream_context_get_default(ctx, &mut [])?;
    with_context(ctx, &context, "stream_context_set_default", |c| {
        c.options = Array::new();
        set_options(c, &options);
    })?;
    Ok(context)
}
