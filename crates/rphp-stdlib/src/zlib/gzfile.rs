//! `gzopen()` and the handle functions, `gzfile()`, `readgzfile()` and
//! the `compress.zlib://` wrapper.
//!
//! A gz handle is a `stream` resource like any other (`file.rs`), whose
//! [`GzStream`] says what is special about it:
//!
//! * **reading**, the file is decompressed whole on open — every gzip
//!   member in it, trailing garbage ignored — or, when it does not start
//!   with the gzip magic, handed through as it is, which is what zlib's
//!   `gzread()` does with a plain file. The stream then reads from that
//!   buffer; what is gz about it is only `feof()` (zlib notices the end
//!   as soon as a read *reaches* it, so the idiomatic `while
//!   (!gzeof($h)) gzgets($h)` loop stops without an extra empty read),
//!   the metadata, and `SEEK_END`, which zlib refuses.
//! * **writing**, [`GzWriter`] is zlib's `gzwrite()` layer: an 8K input
//!   buffer, an 8K output buffer, `deflate()` over them with the level and
//!   strategy from the mode string (`"wb9"`, `"wh"`, …), a forward seek
//!   that becomes zeros, and at close php's flush (`Z_SYNC_FLUSH`, only
//!   when something was written since the last one) followed by zlib's
//!   `Z_FINISH`. The buffer sizes matter for level 0, whose stored blocks
//!   are cut by them.
//!
//! php's gz functions are aliases of the stream functions (`gzread` is
//! `fread`, …), so the handle functions here check their argument under
//! their own name — which is what php's messages say — and hand over to
//! `file.rs`.

use std::fs;
use std::io::{Read, Write};

use rphp_runtime::{Ctx, NativeResult, Unwind};
use rphp_value::{Array, Str, Value};

use super::deflate::{Deflate, Z_FINISH, Z_NO_FLUSH, Z_STREAM_END, Z_SYNC_FLUSH};

/// zlib's `GZBUFSIZE`: the size of `gzwrite()`'s input and output buffers.
const GZBUFSIZE: usize = 8192;

/// What makes a stream a gz stream.
pub(crate) enum GzStream {
    /// A read handle over the decompressed bytes. `past` is zlib's flag
    /// for "a read went past the end", which php turns into the stream's
    /// end-of-file flag.
    Read { past: bool, wrapper: bool },
    Write(Box<GzWriter>),
}

impl GzStream {
    /// Whether this handle was opened through `compress.zlib://` (php
    /// then reports a `wrapper_type` and a `uri`).
    pub(crate) fn via_wrapper(&self) -> bool {
        match self {
            GzStream::Read { wrapper, .. } => *wrapper,
            GzStream::Write(w) => w.wrapper,
        }
    }

    /// `past`, for a read handle.
    pub(crate) fn past(&self) -> bool {
        matches!(self, GzStream::Read { past: true, .. })
    }

    /// A buffered read got to within a chunk of the end: zlib's
    /// `gzread()` of that chunk went past it.
    pub(crate) fn note_fill(&mut self, pos: usize, len: usize) {
        if let GzStream::Read { past, .. } = self {
            if pos + GZBUFSIZE > len {
                *past = true;
            }
        }
    }

    /// A seek clears the flag.
    pub(crate) fn clear_past(&mut self) {
        if let GzStream::Read { past, .. } = self {
            *past = false;
        }
    }

    pub(crate) fn writer(&mut self) -> Option<&mut GzWriter> {
        match self {
            GzStream::Write(w) => Some(w),
            GzStream::Read { .. } => None,
        }
    }
}

/// zlib's write state (`gz_state` in `GZ_WRITE` mode) over a file.
pub(crate) struct GzWriter {
    file: Option<fs::File>,
    level: i32,
    strategy: i32,
    /// `'T'` in the mode: no compression at all.
    direct: bool,
    wrapper: bool,
    z: Option<Deflate>,
    /// `state->in`: bytes waiting for `deflate()`.
    pending_in: Vec<u8>,
    /// `state->out`, `strm->next_out` and `state->x.next`.
    out: Vec<u8>,
    next_out: usize,
    x_next: usize,
    /// A finished member: the next data starts another one.
    reset: bool,
    /// `state->x.pos`: bytes written, zeros included.
    pos: u64,
    /// A pending forward seek (`state->seek`/`state->skip`).
    skip: Option<u64>,
    /// php's `PHP_STREAM_FLAG_WAS_WRITTEN`: whether closing flushes first.
    written: bool,
    closed: bool,
}

impl GzWriter {
    fn new(file: fs::File, level: i32, strategy: i32, direct: bool, wrapper: bool) -> GzWriter {
        GzWriter {
            file: Some(file),
            level,
            strategy,
            direct,
            wrapper,
            z: None,
            pending_in: Vec::new(),
            out: Vec::new(),
            next_out: 0,
            x_next: 0,
            reset: false,
            pos: 0,
            skip: None,
            written: false,
            closed: false,
        }
    }

    /// `gztell()`.
    pub(crate) fn tell(&self) -> u64 {
        self.pos + self.skip.unwrap_or(0)
    }

    fn write_file(&mut self, data: &[u8]) {
        if let Some(f) = self.file.as_mut() {
            // zlib records a failed write and gives up on the stream; php
            // reports nothing for it, so neither does this.
            let _ = f.write_all(data);
        }
    }

    /// `gz_init()` for writing.
    fn init(&mut self) {
        if self.z.is_some() || (self.direct && !self.out.is_empty()) {
            return;
        }
        if !self.direct {
            self.z = Deflate::new(self.level, 15 + 16, 8, self.strategy);
        }
        self.out = vec![0; GZBUFSIZE];
        self.next_out = 0;
        self.x_next = 0;
    }

    /// `gz_comp()`: run `deflate()` over `input` until it stops producing.
    fn comp(&mut self, mut input: &[u8], flush: i32) {
        self.init();
        if self.direct {
            let data = input.to_vec();
            self.write_file(&data);
            return;
        }
        if self.reset {
            if input.is_empty() {
                return;
            }
            if let Some(z) = self.z.as_mut() {
                z.reset();
            }
            self.reset = false;
        }
        let mut ret = super::deflate::Z_OK;
        loop {
            let avail_out = self.out.len() - self.next_out;
            if avail_out == 0 || (flush != Z_NO_FLUSH && (flush != Z_FINISH || ret == Z_STREAM_END)) {
                if self.next_out > self.x_next {
                    let chunk = self.out[self.x_next..self.next_out].to_vec();
                    self.write_file(&chunk);
                    self.x_next = self.next_out;
                }
                if avail_out == 0 {
                    self.next_out = 0;
                    self.x_next = 0;
                }
            }
            let Some(z) = self.z.as_mut() else { return };
            let (st, used, made) = z.deflate(input, &mut self.out[self.next_out..], flush);
            ret = st;
            input = &input[used..];
            self.next_out += made;
            if made == 0 {
                break;
            }
        }
        if flush == Z_FINISH {
            self.reset = true;
        }
    }

    /// `gz_zero()`: a pending seek becomes that many zeros.
    fn zero(&mut self, mut len: u64) {
        if !self.pending_in.is_empty() {
            let p = std::mem::take(&mut self.pending_in);
            self.comp(&p, Z_NO_FLUSH);
        }
        let zeros = vec![0u8; GZBUFSIZE];
        while len > 0 {
            let n = (GZBUFSIZE as u64).min(len) as usize;
            self.pos += n as u64;
            self.comp(&zeros[..n], Z_NO_FLUSH);
            len -= n as u64;
        }
    }

    fn take_skip(&mut self) {
        if let Some(n) = self.skip.take() {
            self.zero(n);
        }
    }

    /// `gzwrite()`: answers the bytes taken.
    pub(crate) fn write(&mut self, data: &[u8]) -> usize {
        if self.closed || data.is_empty() {
            return 0;
        }
        self.written = true;
        self.init();
        self.take_skip();
        if data.len() < GZBUFSIZE {
            let mut buf = data;
            loop {
                let copy = (GZBUFSIZE - self.pending_in.len()).min(buf.len());
                self.pending_in.extend_from_slice(&buf[..copy]);
                self.pos += copy as u64;
                buf = &buf[copy..];
                if buf.is_empty() {
                    break;
                }
                let p = std::mem::take(&mut self.pending_in);
                self.comp(&p, Z_NO_FLUSH);
            }
        } else {
            if !self.pending_in.is_empty() {
                let p = std::mem::take(&mut self.pending_in);
                self.comp(&p, Z_NO_FLUSH);
            }
            self.pos += data.len() as u64;
            self.comp(data, Z_NO_FLUSH);
        }
        data.len()
    }

    /// `gzseek()` on a write handle: forward only, as zeros; answers the
    /// new position or -1.
    pub(crate) fn seek(&mut self, offset: i64, whence: i64) -> i64 {
        let mut offset = offset;
        if whence == 0 {
            offset -= self.pos as i64;
        } else if let Some(n) = self.skip {
            offset += n as i64;
        }
        self.skip = None;
        if offset < 0 {
            return -1;
        }
        if offset > 0 {
            self.skip = Some(offset as u64);
        }
        (self.pos + offset as u64) as i64
    }

    /// `fflush()`: php's `gzflush(Z_SYNC_FLUSH)`.
    pub(crate) fn flush(&mut self) {
        if self.closed {
            return;
        }
        self.written = false;
        self.init();
        self.take_skip();
        let p = std::mem::take(&mut self.pending_in);
        self.comp(&p, Z_SYNC_FLUSH);
    }

    /// `fclose()`: php flushes a handle written since its last flush,
    /// then zlib finishes the member.
    pub(crate) fn close(&mut self) {
        if self.closed {
            return;
        }
        if self.written {
            self.flush();
        }
        self.take_skip();
        let p = std::mem::take(&mut self.pending_in);
        self.comp(&p, Z_FINISH);
        self.closed = true;
        self.file = None;
    }
}

impl Drop for GzWriter {
    /// A handle nobody closed still ends as php leaves it: complete.
    fn drop(&mut self) {
        self.close();
    }
}

// ---- reading --------------------------------------------------------------

/// What zlib's `gzread()` makes of a file: every gzip member decoded in
/// turn, stopping at anything that is not another member; a file that
/// does not start with the gzip magic comes through unchanged.
fn gunzip_file(data: &[u8]) -> Vec<u8> {
    let is_gzip = |d: &[u8]| d.len() > 1 && d[0] == 0x1f && d[1] == 0x8b;
    if !is_gzip(data) {
        return data.to_vec();
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while is_gzip(&data[pos..]) {
        let mut z = flate2::Decompress::new_gzip(15);
        let mut buf = vec![0u8; 64 * 1024];
        let mut ended = false;
        loop {
            let (st, used, made) = super::inflate_step(&mut z, &data[pos..], &mut buf, false);
            out.extend_from_slice(&buf[..made]);
            pos += used;
            if st == Z_STREAM_END {
                ended = true;
                break;
            }
            // A data error, or the file ending mid-member: zlib keeps what
            // it had and reports the end.
            if st != super::deflate::Z_OK || (used == 0 && made == 0) {
                break;
            }
        }
        if !ended {
            break;
        }
    }
    out
}

// ---- opening --------------------------------------------------------------

/// The parts of a gz mode string zlib cares about.
struct GzMode {
    write: bool,
    append: bool,
    level: i32,
    strategy: i32,
    direct: bool,
}

/// zlib's `gz_open()` mode parsing: the last of `r`/`w`/`a` wins, a digit
/// is the level, `f`/`h`/`R`/`F` a strategy, `T` transparent writing.
fn parse_gz_mode(mode: &str) -> Option<GzMode> {
    let mut m = None;
    let mut level = -1;
    let mut strategy = 0;
    let mut direct = false;
    for c in mode.chars() {
        match c {
            '0'..='9' => level = c as i32 - '0' as i32,
            'r' => m = Some('r'),
            'w' => m = Some('w'),
            'a' => m = Some('a'),
            'f' => strategy = super::deflate::Z_FILTERED,
            'h' => strategy = super::deflate::Z_HUFFMAN_ONLY,
            'R' => strategy = super::deflate::Z_RLE,
            'F' => strategy = super::deflate::Z_FIXED,
            'T' => direct = true,
            _ => {}
        }
    }
    let m = m?;
    // zlib cannot force a transparent read.
    if m == 'r' && direct {
        return None;
    }
    Some(GzMode { write: m != 'r', append: m == 'a', level, strategy, direct })
}

/// Open the file under a gz handle the way php's plain wrapper opens its
/// inner stream: by the mode's first letter.
fn open_inner(path: &std::path::Path, mode: &str) -> Result<fs::File, String> {
    let mut o = fs::OpenOptions::new();
    match mode.chars().next() {
        Some('r') => o.read(true),
        Some('w') => o.write(true).create(true).truncate(true),
        Some('a') => o.append(true).create(true),
        Some('x') => o.write(true).create_new(true),
        Some('c') => o.write(true).create(true),
        _ => return Err(format!("`{mode}' is not a valid mode for fopen")),
    };
    o.open(path).map_err(|e| crate::filestat::io_text(&e).to_string())
}

/// Why a gz open failed, for the caller to word.
enum OpenError {
    /// php's "Cannot open a zlib stream for reading and writing".
    ReadWrite,
    /// The inner open failed, with its reason.
    Inner(String),
    /// zlib refused the mode.
    GzFailed,
}

/// Open `path` as a gz handle; the `stream` resource on success.
fn open_gz(ctx: &mut Ctx, path_bytes: &[u8], mode: &str, wrapper: Option<&str>) -> Result<Value, OpenError> {
    if mode.contains('+') {
        return Err(OpenError::ReadWrite);
    }
    let path = crate::filestat::arg_path(ctx, &Value::string(path_bytes));
    let file = open_inner(&path, mode).map_err(OpenError::Inner)?;
    if !mode.starts_with('r') {
        crate::filestat::clear_stat_cache(ctx);
    }
    // php's inner plain-file stream holds a resource id of its own — also
    // when zlib then refuses the mode.
    ctx.resources.reserve_id();
    let Some(gz) = parse_gz_mode(mode) else {
        return Err(OpenError::GzFailed);
    };
    let uri = wrapper.map(str::to_string);
    let shown_uri = uri.clone().unwrap_or_else(|| path.to_string_lossy().into_owned());
    if gz.write {
        let _ = gz.append;
        let w = GzWriter::new(file, gz.level, gz.strategy, gz.direct, wrapper.is_some());
        Ok(crate::file::gz_resource(ctx, GzStream::Write(Box::new(w)), Vec::new(), None, mode, &shown_uri))
    } else {
        let mut file = file;
        let mut raw = Vec::new();
        // A descriptor that cannot be read (a directory, a write-only
        // open) reads as nothing, as zlib's gzread() error does.
        let _ = file.read_to_end(&mut raw);
        let data = gunzip_file(&raw);
        let st = GzStream::Read { past: false, wrapper: wrapper.is_some() };
        Ok(crate::file::gz_resource(ctx, st, data, Some(path), mode, &shown_uri))
    }
}

/// `gzopen(string $filename, string $mode, int $use_include_path = 0): resource|false`
pub(crate) fn gzopen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].deref().to_php_bytes().to_vec();
    let mode = String::from_utf8_lossy(&args[1].deref().to_php_bytes()).into_owned();
    open_gz_reporting(ctx, "gzopen", &name, &mode)
}

fn open_gz_reporting(ctx: &mut Ctx, func: &str, name: &[u8], mode: &str) -> NativeResult {
    let shown = String::from_utf8_lossy(name).into_owned();
    match open_gz(ctx, name, mode, None) {
        Ok(v) => Ok(v),
        Err(OpenError::ReadWrite) => {
            ctx.warn(&format!("{func}(): Cannot open a zlib stream for reading and writing at the same time!"))?;
            Ok(Value::Bool(false))
        }
        Err(OpenError::Inner(why)) => {
            ctx.warn(&format!("{func}({shown}): Failed to open stream: {why}"))?;
            Ok(Value::Bool(false))
        }
        Err(OpenError::GzFailed) => {
            ctx.warn(&format!("{func}(): gzopen failed"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// The path behind a `compress.zlib://` url (php also takes `zlib:`).
pub(crate) fn compress_zlib_path(url: &[u8]) -> Option<&[u8]> {
    if url.len() >= 16 && url[..16].eq_ignore_ascii_case(b"compress.zlib://") {
        Some(&url[16..])
    } else {
        None
    }
}

/// `fopen("compress.zlib://…", $mode)`: a gz handle, or php's one-line
/// "operation failed" (the wrapper opens its file without reporting).
pub(crate) fn open_compress_zlib(ctx: &mut Ctx, func: &str, url: &[u8], mode: &str) -> NativeResult {
    let path = compress_zlib_path(url).unwrap_or(url);
    let shown = String::from_utf8_lossy(url).into_owned();
    match open_gz(ctx, path, mode, Some(&shown)) {
        Ok(v) => Ok(v),
        Err(_) => {
            ctx.warn(&format!("{func}({shown}): Failed to open stream: operation failed"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// The whole decompressed contents behind a `compress.zlib://` url, for
/// `file_get_contents()`, `file()` and `readfile()`; `None` after the
/// warning.
pub(crate) fn read_compress_zlib(ctx: &mut Ctx, func: &str, url: &[u8]) -> Result<Option<Vec<u8>>, Unwind> {
    let path = compress_zlib_path(url).unwrap_or(url);
    let p = crate::filestat::arg_path(ctx, &Value::string(path));
    match fs::File::open(&p) {
        Ok(mut f) => {
            let mut raw = Vec::new();
            let _ = f.read_to_end(&mut raw);
            ctx.resources.reserve_id();
            ctx.resources.reserve_id();
            Ok(Some(gunzip_file(&raw)))
        }
        Err(_) => {
            let shown = String::from_utf8_lossy(url).into_owned();
            ctx.warn(&format!("{func}({shown}): Failed to open stream: operation failed"))?;
            Ok(None)
        }
    }
}

/// `file_put_contents("compress.zlib://…")`: one gz member written (or
/// appended) in a single write. The byte count, or `None` after the
/// warning.
pub(crate) fn write_compress_zlib(
    ctx: &mut Ctx,
    func: &str,
    url: &[u8],
    data: &[u8],
    append: bool,
) -> Result<Option<usize>, Unwind> {
    let path = compress_zlib_path(url).unwrap_or(url);
    let p = crate::filestat::arg_path(ctx, &Value::string(path));
    let file = open_inner(&p, if append { "ab" } else { "wb" });
    crate::filestat::clear_stat_cache(ctx);
    match file {
        Ok(f) => {
            ctx.resources.reserve_id();
            ctx.resources.reserve_id();
            let mut w = GzWriter::new(f, -1, 0, false, true);
            let n = w.write(data);
            w.close();
            Ok(Some(n))
        }
        Err(_) => {
            let shown = String::from_utf8_lossy(url).into_owned();
            ctx.warn(&format!("{func}({shown}): Failed to open stream: operation failed"))?;
            Ok(None)
        }
    }
}

// ---- the handle functions -------------------------------------------------

/// The argument check php's aliases make under their own name.
fn stream_arg(v: &Value, func: &str) -> Result<(), Unwind> {
    match &*v.deref() {
        Value::Resource(_) => Ok(()),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// Call the stream function a gz function is an alias of.
fn call_stream_fn(ctx: &mut Ctx, name: &str, args: &mut [Value]) -> NativeResult {
    let f = crate::file::FUNCTIONS
        .iter()
        .chain(crate::file2::FUNCTIONS.iter())
        .find(|f| f.name == name)
        .ok_or_else(|| Unwind::error(format!("Call to undefined function {name}()")))?;
    (f.handler)(ctx, args)
}

/// Whether the handle is a gz handle open for writing (`Some(true)`),
/// reading (`Some(false)`), or not a gz handle at all.
fn gz_kind(ctx: &mut Ctx, v: &Value) -> Option<bool> {
    crate::file::gz_mode_of(ctx, v)
}

pub(crate) fn gzwrite(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzwrite")?;
    if gz_kind(ctx, &args[0]) == Some(false) {
        return Ok(Value::Int(0));
    }
    call_stream_fn(ctx, "fwrite", args)
}

pub(crate) fn gzputs(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzputs")?;
    if gz_kind(ctx, &args[0]) == Some(false) {
        return Ok(Value::Int(0));
    }
    call_stream_fn(ctx, "fwrite", args)
}

pub(crate) fn gzrewind(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzrewind")?;
    if gz_kind(ctx, &args[0]) == Some(true) {
        // zlib cannot seek a write handle backwards.
        let r = crate::file::gz_seek(ctx, &args[0], 0, 0);
        return Ok(Value::Bool(r >= 0));
    }
    call_stream_fn(ctx, "rewind", args)
}

pub(crate) fn gzclose(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzclose")?;
    call_stream_fn(ctx, "fclose", args)
}

pub(crate) fn gzeof(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzeof")?;
    call_stream_fn(ctx, "feof", args)
}

pub(crate) fn gzgetc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzgetc")?;
    if gz_kind(ctx, &args[0]) == Some(true) {
        return Ok(Value::Bool(false));
    }
    call_stream_fn(ctx, "fgetc", args)
}

pub(crate) fn gzpassthru(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzpassthru")?;
    match gz_kind(ctx, &args[0]) {
        Some(true) => Ok(Value::Int(-1)),
        // `fpassthru()`: the rest of the stream, echoed.
        Some(false) => {
            let data = ctx.call_function(b"stream_get_contents", &[args[0].clone()])?.to_php_bytes().to_vec();
            ctx.echo(&data);
            Ok(Value::Int(data.len() as i64))
        }
        None => call_stream_fn(ctx, "fpassthru", args),
    }
}

pub(crate) fn gzseek(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzseek")?;
    let whence = args.get(2).map_or(0, |v| v.deref().to_int());
    if gz_kind(ctx, &args[0]).is_some() {
        if whence == 2 {
            ctx.warn("gzseek(): SEEK_END is not supported")?;
            return Ok(Value::Int(-1));
        }
        let offset = args[1].deref().to_int();
        let r = crate::file::gz_seek(ctx, &args[0], offset, whence);
        return Ok(Value::Int(if r < 0 { -1 } else { 0 }));
    }
    call_stream_fn(ctx, "fseek", args)
}

pub(crate) fn gztell(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gztell")?;
    call_stream_fn(ctx, "ftell", args)
}

pub(crate) fn gzread(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzread")?;
    if gz_kind(ctx, &args[0]) == Some(true) {
        return Ok(Value::Bool(false));
    }
    call_stream_fn(ctx, "fread", args)
}

pub(crate) fn gzgets(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stream_arg(&args[0], "gzgets")?;
    let kind = gz_kind(ctx, &args[0]);
    if kind == Some(true) {
        return Ok(Value::Bool(false));
    }
    let len = args.get(1).filter(|v| !matches!(&*v.deref(), Value::Null)).map(|v| v.deref().to_int());
    let (Some(len), Some(false)) = (len, kind) else {
        return call_stream_fn(ctx, "fgets", args);
    };
    if len <= 0 {
        return Err(Unwind::value_error("gzgets(): Argument #2 ($length) must be greater than 0"));
    }
    // php's line read stops after `$length - 1` bytes or the newline.
    let mut line = Vec::new();
    while (line.len() as i64) < len - 1 {
        let mut a = [args[0].clone()];
        let c = call_stream_fn(ctx, "fgetc", &mut a)?;
        let Value::Str(c) = c else { break };
        line.extend_from_slice(c.as_bytes());
        if c.as_bytes() == b"\n" {
            break;
        }
    }
    Ok(if line.is_empty() { Value::Bool(false) } else { Value::Str(Str::from_vec(line)) })
}

/// The decompressed contents of a file for `gzfile()`/`readgzfile()`.
fn read_whole(ctx: &mut Ctx, func: &str, name: &[u8]) -> Result<Option<Vec<u8>>, Unwind> {
    let p = crate::filestat::arg_path(ctx, &Value::string(name));
    match fs::File::open(&p) {
        Ok(mut f) => {
            let mut raw = Vec::new();
            let _ = f.read_to_end(&mut raw);
            // php opens the file and the gz stream over it.
            ctx.resources.reserve_id();
            ctx.resources.reserve_id();
            Ok(Some(gunzip_file(&raw)))
        }
        Err(e) => {
            let shown = String::from_utf8_lossy(name).into_owned();
            ctx.warn(&format!(
                "{func}({shown}): Failed to open stream: {}",
                crate::filestat::io_text(&e)
            ))?;
            Ok(None)
        }
    }
}

/// `gzfile(string $filename, int $use_include_path = 0): array|false`
pub(crate) fn gzfile(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].deref().to_php_bytes().to_vec();
    let Some(data) = read_whole(ctx, "gzfile", &name)? else {
        return Ok(Value::Bool(false));
    };
    let mut out = Array::new();
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            out.push(Value::Str(Str::from_vec(data[start..=i].to_vec())));
            start = i + 1;
        }
    }
    if start < data.len() {
        out.push(Value::Str(Str::from_vec(data[start..].to_vec())));
    }
    Ok(Value::Array(out))
}

/// `readgzfile(string $filename, int $use_include_path = 0): int|false`
pub(crate) fn readgzfile(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].deref().to_php_bytes().to_vec();
    let Some(data) = read_whole(ctx, "readgzfile", &name)? else {
        return Ok(Value::Bool(false));
    };
    ctx.echo(&data);
    Ok(Value::Int(data.len() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parsing_follows_zlib() {
        let m = parse_gz_mode("wb9").unwrap();
        assert!(m.write && m.level == 9);
        assert!(parse_gz_mode("rT").is_none());
        assert!(parse_gz_mode("x").is_none());
        let m = parse_gz_mode("rw").unwrap();
        assert!(m.write);
        let m = parse_gz_mode("w10").unwrap();
        assert_eq!(m.level, 0);
    }

    #[test]
    fn gunzip_passes_plain_files_through_and_joins_members() {
        assert_eq!(gunzip_file(b"plain"), b"plain");
        let a = crate::zlib::encode(b"one ", 31, -1).unwrap();
        let b = crate::zlib::encode(b"two", 31, -1).unwrap();
        let mut both = a.clone();
        both.extend_from_slice(&b);
        both.extend_from_slice(b"junk");
        assert_eq!(gunzip_file(&both), b"one two");
    }
}
