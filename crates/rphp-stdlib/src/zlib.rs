//! `ext/zlib`: the `gz*`/`zlib_*` functions, the incremental
//! `deflate_*`/`inflate_*` contexts, `gzopen` and `compress.zlib://`.
//!
//! **Compression** is `zlib/deflate.rs`, a port of zlib 1.2.12's
//! compressor — the libz php links (`ZLIB_VERSION` is `1.2.12`) — because
//! php passes zlib parameters `flate2` cannot: memLevel 9 for the one-shot
//! functions and the `zlib.deflate` filter, and a script-chosen memory
//! level and strategy for `deflate_init()` and `gzopen()`. Every caller
//! below drives it with the same `deflate()` calls and output-buffer sizes
//! php's C does, since stored (level 0) blocks are sized by the room left
//! in the output buffer.
//!
//! **Decompression** is `flate2` over the system libz: inflate's output is
//! fixed by its input, so only the call sequence matters, and that is
//! php's — `php_zlib_inflate_rounds()` with its 100-round limit and its
//! growing buffer for the one-shot functions (which also hands zlib the
//! string's terminating NUL, as php does), `inflate_add()`'s 8K chunks.
//!
//! **Files.** A `gzopen()` handle is an ordinary `stream` resource
//! (`zlib/gzfile.rs`): a file opened for reading is decompressed whole
//! into the stream's buffer — or passed through as it is when it is not
//! gzip — and one opened for writing compresses each write into the file
//! as zlib's `gzwrite()` does, a member per open. `compress.zlib://` is
//! the same handle behind `fopen()` and the whole-file functions.
//!
//! **Not here** (catalogued in COVERAGE.md): `ob_gzhandler()` and
//! `zlib.output_compression` — the ini entries are registered with php's
//! defaults, but output is never compressed, which is also what php's CLI
//! does without an `Accept-Encoding` request header.

use rphp_runtime::{nf, nm, ClassFlags, Ctx, NativeFn, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, ArrayKey, Object, Payload, Str, Value};

mod deflate;
mod filter;
mod gzfile;

use deflate::{Deflate, Z_BUF_ERROR, Z_FINISH, Z_OK, Z_STREAM_END};

#[allow(unused_imports)]
pub(crate) use filter::ZlibFilter;
pub(crate) use gzfile::{compress_zlib_path, open_compress_zlib, read_compress_zlib, write_compress_zlib};
pub(crate) use gzfile::GzStream;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("zlib_get_coding_type", 0, Some(0), zlib_get_coding_type),
    nf!("gzfile", 1, Some(2), gzfile::gzfile),
    nf!("gzopen", 2, Some(3), gzfile::gzopen),
    nf!("readgzfile", 1, Some(2), gzfile::readgzfile),
    nf!("zlib_encode", 2, Some(3), zlib_encode),
    nf!("zlib_decode", 1, Some(2), zlib_decode),
    nf!("gzdeflate", 1, Some(3), gzdeflate),
    nf!("gzencode", 1, Some(3), gzencode),
    nf!("gzcompress", 1, Some(3), gzcompress),
    nf!("gzinflate", 1, Some(2), gzinflate),
    nf!("gzdecode", 1, Some(2), gzdecode),
    nf!("gzuncompress", 1, Some(2), gzuncompress),
    nf!("gzwrite", 2, Some(3), gzfile::gzwrite),
    nf!("gzputs", 2, Some(3), gzfile::gzputs),
    nf!("gzrewind", 1, Some(1), gzfile::gzrewind),
    nf!("gzclose", 1, Some(1), gzfile::gzclose),
    nf!("gzeof", 1, Some(1), gzfile::gzeof),
    nf!("gzgetc", 1, Some(1), gzfile::gzgetc),
    nf!("gzpassthru", 1, Some(1), gzfile::gzpassthru),
    nf!("gzseek", 2, Some(3), gzfile::gzseek),
    nf!("gztell", 1, Some(1), gzfile::gztell),
    nf!("gzread", 2, Some(2), gzfile::gzread),
    nf!("gzgets", 1, Some(2), gzfile::gzgets),
    nf!("deflate_init", 1, Some(2), deflate_init),
    nf!("deflate_add", 2, Some(3), deflate_add),
    nf!("inflate_init", 1, Some(2), inflate_init),
    nf!("inflate_add", 2, Some(3), inflate_add),
    nf!("inflate_get_status", 1, Some(1), inflate_get_status),
    nf!("inflate_get_read_len", 1, Some(1), inflate_get_read_len),
];

// ---- constants ------------------------------------------------------------

/// `ZLIB_ENCODING_RAW` / `GZIP` / `DEFLATE`: the window bits php passes
/// to zlib for each wrapper.
const ENCODING_RAW: i64 = -15;
const ENCODING_GZIP: i64 = 31;
const ENCODING_DEFLATE: i64 = 15;
/// php's `PHP_ZLIB_ENCODING_ANY`: inflate that detects zlib or gzip.
const ENCODING_ANY: i64 = 0x2f;

/// zlib's `MAX_MEM_LEVEL`, which php's one-shot functions use.
const MAX_MEM_LEVEL: i32 = 9;

/// `ZLIB_VERSION` / `ZLIB_VERNUM`: the zlib this compressor is.
const ZLIB_VERSION: &str = "1.2.12";
const ZLIB_VERNUM: i64 = 0x12c0;

pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("FORCE_GZIP", ENCODING_GZIP),
        ("FORCE_DEFLATE", ENCODING_DEFLATE),
        ("ZLIB_ENCODING_RAW", ENCODING_RAW),
        ("ZLIB_ENCODING_GZIP", ENCODING_GZIP),
        ("ZLIB_ENCODING_DEFLATE", ENCODING_DEFLATE),
        ("ZLIB_NO_FLUSH", 0),
        ("ZLIB_PARTIAL_FLUSH", 1),
        ("ZLIB_SYNC_FLUSH", 2),
        ("ZLIB_FULL_FLUSH", 3),
        ("ZLIB_BLOCK", 5),
        ("ZLIB_FINISH", 4),
        ("ZLIB_FILTERED", 1),
        ("ZLIB_HUFFMAN_ONLY", 2),
        ("ZLIB_RLE", 3),
        ("ZLIB_FIXED", 4),
        ("ZLIB_DEFAULT_STRATEGY", 0),
    ] {
        r.constant(name, Value::Int(v));
    }
    r.constant("ZLIB_VERSION", Value::string(ZLIB_VERSION.as_bytes()));
    r.constant("ZLIB_VERNUM", Value::Int(ZLIB_VERNUM));
    for (name, v) in [
        ("ZLIB_OK", 0),
        ("ZLIB_STREAM_END", 1),
        ("ZLIB_NEED_DICT", 2),
        ("ZLIB_ERRNO", -1),
        ("ZLIB_STREAM_ERROR", -2),
        ("ZLIB_DATA_ERROR", -3),
        ("ZLIB_MEM_ERROR", -4),
        ("ZLIB_BUF_ERROR", -5),
        ("ZLIB_VERSION_ERROR", -6),
    ] {
        r.constant(name, Value::Int(v));
    }
    // The ini entries, with php's defaults.
    for (k, v) in [
        ("zlib.output_compression", "0"),
        ("zlib.output_compression_level", "-1"),
        ("zlib.output_handler", ""),
    ] {
        r.interp().ini.register(k, v);
    }
}

// ---- argument helpers -----------------------------------------------------

fn bytes(v: &Value) -> Vec<u8> {
    v.deref().to_php_bytes().to_vec()
}

fn string_value(b: Vec<u8>) -> Value {
    Value::Str(Str::from_vec(b))
}

/// php's `PHP_ZLIB_BUFFER_SIZE_GUESS()`: the output buffer the one-shot
/// compressors and `deflate_add()` start from.
fn buffer_size_guess(in_len: usize) -> usize {
    ((in_len as f64) * 1.015) as usize + 10 + 8 + 4 + 1
}

/// The level and encoding checks every one-shot compressor makes.
fn check_level(func: &str, level: i64, arg: u32) -> Result<(), Unwind> {
    if !(-1..=9).contains(&level) {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #{arg} ($level) must be between -1 and 9"
        )));
    }
    Ok(())
}

fn check_encoding(func: &str, encoding: i64, arg: u32) -> Result<(), Unwind> {
    match encoding {
        ENCODING_RAW | ENCODING_GZIP | ENCODING_DEFLATE => Ok(()),
        _ => Err(Unwind::value_error(format!(
            "{func}(): Argument #{arg} ($encoding) must be one of ZLIB_ENCODING_RAW, ZLIB_ENCODING_GZIP, or ZLIB_ENCODING_DEFLATE"
        ))),
    }
}

// ---- one-shot compression -------------------------------------------------

/// `php_zlib_encode()`: one `deflate(Z_FINISH)` into a buffer of php's
/// guessed size, memLevel 9. `Err` carries zlib's status for the warning.
fn encode(data: &[u8], encoding: i64, level: i64) -> Result<Vec<u8>, i32> {
    let Some(mut z) = Deflate::new(level as i32, encoding as i32, MAX_MEM_LEVEL, 0) else {
        return Err(deflate::Z_STREAM_ERROR);
    };
    let mut out = vec![0u8; buffer_size_guess(data.len())];
    let (status, _, made) = z.deflate(data, &mut out, Z_FINISH);
    if status == Z_STREAM_END {
        out.truncate(made);
        Ok(out)
    } else {
        Err(status)
    }
}

fn encode_result(ctx: &mut Ctx, func: &str, data: &[u8], encoding: i64, level: i64) -> NativeResult {
    match encode(data, encoding, level) {
        Ok(out) => Ok(string_value(out)),
        Err(status) => {
            ctx.warn(&format!("{func}(): {}", deflate::z_error(status)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// The three `gz*` compressors: `(data, level = -1, encoding = <own>)`.
fn gz_encode_fn(ctx: &mut Ctx, args: &mut [Value], func: &str, default_encoding: i64) -> NativeResult {
    let data = bytes(&args[0]);
    let level = args.get(1).map_or(-1, |v| v.deref().to_int());
    let encoding = args.get(2).map_or(default_encoding, |v| v.deref().to_int());
    check_level(func, level, 2)?;
    check_encoding(func, encoding, 3)?;
    encode_result(ctx, func, &data, encoding, level)
}

/// `gzcompress(string $data, int $level = -1, int $encoding = ZLIB_ENCODING_DEFLATE): string|false`
fn gzcompress(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_encode_fn(ctx, args, "gzcompress", ENCODING_DEFLATE)
}

/// `gzdeflate(string $data, int $level = -1, int $encoding = ZLIB_ENCODING_RAW): string|false`
fn gzdeflate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_encode_fn(ctx, args, "gzdeflate", ENCODING_RAW)
}

/// `gzencode(string $data, int $level = -1, int $encoding = ZLIB_ENCODING_GZIP): string|false`
fn gzencode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_encode_fn(ctx, args, "gzencode", ENCODING_GZIP)
}

/// `zlib_encode(string $data, int $encoding, int $level = -1): string|false`
fn zlib_encode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let data = bytes(&args[0]);
    let encoding = args[1].deref().to_int();
    let level = args.get(2).map_or(-1, |v| v.deref().to_int());
    check_encoding("zlib_encode", encoding, 2)?;
    check_level("zlib_encode", level, 3)?;
    encode_result(ctx, "zlib_encode", &data, encoding, level)
}

// ---- one-shot decompression -----------------------------------------------

/// zlib's inflate status codes, as `flate2` reports them.
const Z_NEED_DICT: i32 = 2;
const Z_DATA_ERROR: i32 = -3;
const Z_MEM_ERROR: i32 = -4;

/// A decompressor for php's window-bits encoding (`-8..-15` raw, `8..15`
/// zlib, `24..31` gzip); `first` is the input, for the automatic
/// zlib-or-gzip detection of `ENCODING_ANY` (window bits 32 and up),
/// which looks at the first two bytes exactly as zlib's inflate does.
pub(crate) fn new_inflater(encoding: i64, first: &[u8]) -> flate2::Decompress {
    use flate2::Decompress;
    // flate2 takes 9..=15; zlib treats an 8-bit window as a 9-bit one when
    // inflating a zlib stream, and a raw one never needs less than 9.
    let bits = |w: i64| w.clamp(9, 15) as u8;
    if encoding < 0 {
        Decompress::new_with_window_bits(false, bits(-encoding))
    } else if encoding >= 32 {
        let w = if encoding - 32 == 0 { 15 } else { encoding - 32 };
        if first.len() >= 2 && first[0] == 0x1f && first[1] == 0x8b {
            Decompress::new_gzip(bits(w))
        } else {
            Decompress::new_with_window_bits(true, bits(w))
        }
    } else if encoding >= 16 {
        Decompress::new_gzip(bits(encoding - 16))
    } else if encoding == 0 {
        Decompress::new_with_window_bits(true, 15)
    } else {
        Decompress::new_with_window_bits(true, bits(encoding))
    }
}

/// One `inflate()` call through flate2: status, consumed, produced.
pub(crate) fn inflate_step(
    z: &mut flate2::Decompress,
    input: &[u8],
    out: &mut [u8],
    finish: bool,
) -> (i32, usize, usize) {
    let (bi, bo) = (z.total_in(), z.total_out());
    let flush = if finish { flate2::FlushDecompress::Finish } else { flate2::FlushDecompress::Sync };
    let r = z.decompress(input, out, flush);
    let used = (z.total_in() - bi) as usize;
    let made = (z.total_out() - bo) as usize;
    let status = match r {
        Ok(flate2::Status::Ok) => Z_OK,
        Ok(flate2::Status::BufError) => Z_BUF_ERROR,
        Ok(flate2::Status::StreamEnd) => Z_STREAM_END,
        Err(e) if e.needs_dictionary().is_some() => Z_NEED_DICT,
        Err(_) => Z_DATA_ERROR,
    };
    (status, used, made)
}

/// `php_zlib_inflate_rounds()`: inflate into a buffer that starts at the
/// input's size (or `max`) and grows by an eighth per round, for at most
/// 100 rounds; running past `max` is `Z_MEM_ERROR`.
fn inflate_rounds(z: &mut flate2::Decompress, input: &[u8], max: usize) -> Result<Vec<u8>, i32> {
    let mut size = if max != 0 && max < input.len() { max } else { input.len() };
    let mut buf: Vec<u8> = Vec::new();
    let mut used = 0usize;
    let mut in_pos = 0usize;
    let mut round = 0;
    let mut status;
    loop {
        if max != 0 && max <= used {
            status = Z_MEM_ERROR;
        } else {
            buf.resize(size, 0);
            // php's inflate() call here is Z_NO_FLUSH; flate2's Sync is the
            // same for inflate (the flush mode only matters to Z_FINISH/Z_BLOCK).
            let (st, u, made) = inflate_step(z, &input[in_pos..], &mut buf[used..size], false);
            status = st;
            in_pos += u;
            used += made;
            size += (size >> 3) + 1;
        }
        round += 1;
        let again = status == Z_BUF_ERROR || (status == Z_OK && in_pos < input.len());
        if !(again && round < 100) {
            break;
        }
    }
    if status == Z_STREAM_END {
        buf.truncate(used);
        Ok(buf)
    } else if status == Z_OK {
        Err(Z_DATA_ERROR)
    } else {
        Err(status)
    }
}

/// `php_zlib_decode()`: php hands zlib the string *with* its terminating
/// NUL, and `ENCODING_ANY` falls back to raw deflate on a data error.
fn decode(data: &[u8], encoding: i64, max: usize) -> Result<Vec<u8>, i32> {
    if data.is_empty() {
        return Err(Z_DATA_ERROR);
    }
    let mut input = data.to_vec();
    input.push(0);
    let mut encoding = encoding;
    loop {
        let mut z = new_inflater(encoding, &input);
        match inflate_rounds(&mut z, &input, max) {
            Ok(out) => return Ok(out),
            Err(Z_DATA_ERROR) if encoding == ENCODING_ANY => encoding = ENCODING_RAW,
            Err(e) => return Err(e),
        }
    }
}

fn gz_decode_fn(ctx: &mut Ctx, args: &mut [Value], func: &str, encoding: i64) -> NativeResult {
    let data = bytes(&args[0]);
    let max = args.get(1).map_or(0, |v| v.deref().to_int());
    if max < 0 {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #2 ($max_length) must be greater than or equal to 0"
        )));
    }
    match decode(&data, encoding, max as usize) {
        Ok(out) => Ok(string_value(out)),
        Err(status) => {
            ctx.warn(&format!("{func}(): {}", deflate::z_error(status)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `gzuncompress(string $data, int $max_length = 0): string|false`
fn gzuncompress(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_decode_fn(ctx, args, "gzuncompress", ENCODING_DEFLATE)
}

/// `gzinflate(string $data, int $max_length = 0): string|false`
fn gzinflate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_decode_fn(ctx, args, "gzinflate", ENCODING_RAW)
}

/// `gzdecode(string $data, int $max_length = 0): string|false`
fn gzdecode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_decode_fn(ctx, args, "gzdecode", ENCODING_GZIP)
}

/// `zlib_decode(string $data, int $max_length = 0): string|false` — zlib
/// or gzip, detected, else raw deflate.
fn zlib_decode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    gz_decode_fn(ctx, args, "zlib_decode", ENCODING_ANY)
}

/// `zlib_get_coding_type(): string|false` — the encoding output
/// compression chose for this request. Output is never compressed here
/// (see the module docs), so there is none.
fn zlib_get_coding_type(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

// ---- incremental contexts -------------------------------------------------

/// What a `DeflateContext` holds.
struct DeflateState {
    z: Deflate,
}

/// What an `InflateContext` holds: the stream, php's saved status, and
/// the dictionary still to be offered when the stream asks for one.
struct InflateState {
    z: flate2::Decompress,
    encoding: i64,
    status: i32,
    /// `inflate_get_read_len()`: `total_in` since the last reset.
    read_len: u64,
    dict: Option<Vec<u8>>,
}

/// php's `zlib_create_dictionary_string()`: a string as it is, or an
/// array of non-empty NUL-free strings each followed by a NUL.
fn dictionary_option(func: &str, options: Option<&Array>) -> Result<Option<Vec<u8>>, Unwind> {
    let Some(v) = options.and_then(|o| o.get_deref(&ArrayKey::str(b"dictionary"))) else {
        return Ok(None);
    };
    match &v {
        Value::Str(s) => Ok(Some(s.as_bytes().to_vec())),
        Value::Array(a) => {
            let mut dict = Vec::new();
            for (_, item) in a.iter() {
                let b = item.deref().to_php_bytes().to_vec();
                if b.is_empty() {
                    return Err(Unwind::value_error(format!(
                        "{func}(): Argument #2 ($options) must not contain empty strings"
                    )));
                }
                if b.contains(&0) {
                    return Err(Unwind::value_error(format!(
                        "{func}(): Argument #2 ($options) must not contain strings with null bytes"
                    )));
                }
                dict.extend_from_slice(&b);
                dict.push(0);
            }
            Ok(if a.is_empty() { None } else { Some(dict) })
        }
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #2 ($options) must be of type zero-terminated string or array, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

fn options_arg(func: &str, args: &[Value]) -> Result<Option<Array>, Unwind> {
    match args.get(1).map(|v| v.deref().into_owned()) {
        None => Ok(None),
        Some(Value::Array(a)) => Ok(Some(a)),
        Some(Value::Object(o)) => {
            // php's `H` accepts an object's property table.
            let mut a = Array::new();
            for (k, v, _) in o.props_snapshot() {
                a.set(ArrayKey::str(&k), v);
            }
            Ok(Some(a))
        }
        Some(other) => Err(Unwind::type_error(format!(
            "{func}(): Argument #2 ($options) must be of type array, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

fn opt_int(options: Option<&Array>, key: &str, default: i64) -> i64 {
    options
        .and_then(|o| o.get_deref(&ArrayKey::str(key.as_bytes())))
        .map_or(default, |v| v.to_int())
}

/// The window bits php hands zlib for an encoding and a `window` option.
fn window_encoding(encoding: i64, window: i64) -> i64 {
    if encoding < 0 {
        encoding + (15 - window)
    } else {
        encoding - (15 - window)
    }
}

fn new_context(ctx: &mut Ctx, class: &[u8], payload: Box<dyn std::any::Any>) -> Result<Value, Unwind> {
    let cid = ctx.class_by_name(class).ok_or_else(|| {
        Unwind::error(format!("Class \"{}\" not found", String::from_utf8_lossy(class)))
    })?;
    let o = ctx.instantiate(cid);
    o.set_payload(Payload::Native(payload));
    Ok(Value::Object(o))
}

/// `deflate_init(int $encoding, array $options = []): DeflateContext|false`
fn deflate_init(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "deflate_init";
    let encoding = args[0].deref().to_int();
    let options = options_arg(F, args)?;
    let o = options.as_ref();
    let level = opt_int(o, "level", -1);
    if !(-1..=9).contains(&level) {
        return Err(Unwind::value_error("deflate_init(): \"level\" option must be between -1 and 9"));
    }
    let memory = opt_int(o, "memory", 8);
    if !(1..=9).contains(&memory) {
        return Err(Unwind::value_error("deflate_init(): \"memory\" option must be between 1 and 9"));
    }
    let window = opt_int(o, "window", 15);
    if !(8..=15).contains(&window) {
        return Err(Unwind::value_error("deflate_init(): \"window\" option must be between 8 and 15"));
    }
    let strategy = opt_int(o, "strategy", 0);
    if !(0..=4).contains(&strategy) {
        return Err(Unwind::value_error(
            "deflate_init(): \"strategy\" option must be one of ZLIB_FILTERED, ZLIB_HUFFMAN_ONLY, ZLIB_RLE, ZLIB_FIXED, or ZLIB_DEFAULT_STRATEGY",
        ));
    }
    let dict = dictionary_option(F, o)?;
    check_encoding(F, encoding, 1)?;
    let bits = window_encoding(encoding, window);
    let Some(mut z) = Deflate::new(level as i32, bits as i32, memory as i32, strategy as i32) else {
        ctx.warn("deflate_init(): Failed allocating zlib.deflate context")?;
        return Ok(Value::Bool(false));
    };
    if let Some(d) = dict {
        // php asserts success and ignores the answer: a gzip stream
        // refuses a dictionary, and php then simply goes on without one.
        let _ = z.set_dictionary(&d);
    }
    new_context(ctx, b"DeflateContext", Box::new(DeflateState { z }))
}

fn context_arg(ctx: &mut Ctx, v: &Value, func: &str, class: &str) -> Result<Object, Unwind> {
    match &*v.deref() {
        Value::Object(o) if ctx.class_name_of(o) == class => Ok(o.clone()),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($context) must be of type {class}, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

fn check_flush(func: &str, flush: i64) -> Result<(), Unwind> {
    if !(0..=5).contains(&flush) {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #3 ($flush_mode) must be one of ZLIB_NO_FLUSH, ZLIB_PARTIAL_FLUSH, ZLIB_SYNC_FLUSH, ZLIB_FULL_FLUSH, ZLIB_BLOCK, or ZLIB_FINISH"
        )));
    }
    Ok(())
}

/// `deflate_add(DeflateContext $context, string $data, int $flush_mode = ZLIB_SYNC_FLUSH): string|false`
fn deflate_add(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "deflate_add";
    let o = context_arg(ctx, &args[0], F, "DeflateContext")?;
    let data = bytes(&args[1]);
    let flush = args.get(2).map_or(2, |v| v.deref().to_int());
    check_flush(F, flush)?;
    if data.is_empty() && flush != Z_FINISH as i64 {
        return Ok(Value::string(b""));
    }
    let r = o.with_payload::<DeflateState, _>(|st| {
        // php's loop: a buffer of the guessed size, grown 64 bytes at a
        // time while deflate() keeps filling it.
        let mut out = vec![0u8; buffer_size_guess(data.len()).max(64)];
        let mut used = 0usize;
        let mut in_pos = 0usize;
        let mut status;
        loop {
            if used == out.len() {
                out.resize(out.len() + 64, 0);
            }
            let (st, u, made) = st.z.deflate(&data[in_pos..], &mut out[used..], flush as i32);
            status = st;
            in_pos += u;
            used += made;
            if !(status == Z_OK && used == out.len()) {
                break;
            }
        }
        match status {
            Z_OK => {
                out.truncate(used);
                Ok(out)
            }
            Z_STREAM_END => {
                out.truncate(used);
                st.z.reset();
                Ok(out)
            }
            e => Err(e),
        }
    });
    match r {
        Some(Ok(out)) => Ok(string_value(out)),
        Some(Err(e)) => {
            ctx.warn(&format!("deflate_add(): zlib error ({})", deflate::z_error(e)))?;
            Ok(Value::Bool(false))
        }
        None => Err(Unwind::type_error(format!(
            "{F}(): Argument #1 ($context) must be of type DeflateContext, DeflateContext given"
        ))),
    }
}

/// `inflate_init(int $encoding, array $options = []): InflateContext|false`
fn inflate_init(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "inflate_init";
    let encoding = args[0].deref().to_int();
    let options = options_arg(F, args)?;
    let o = options.as_ref();
    let window = opt_int(o, "window", 15);
    if !(8..=15).contains(&window) {
        return Err(Unwind::value_error(format!(
            "zlib window size (logarithm) ({window}) must be within 8..15"
        )));
    }
    let dict = dictionary_option(F, o)?;
    match encoding {
        ENCODING_RAW | ENCODING_GZIP | ENCODING_DEFLATE => {}
        _ => {
            return Err(Unwind::value_error(
                "Encoding mode must be ZLIB_ENCODING_RAW, ZLIB_ENCODING_GZIP or ZLIB_ENCODING_DEFLATE",
            ))
        }
    }
    let bits = window_encoding(encoding, window);
    let mut z = new_inflater(bits, &[]);
    let mut dict = dict;
    // A raw stream never asks for its dictionary, so php sets it up front.
    if bits == ENCODING_RAW {
        if let Some(d) = dict.take() {
            if z.set_dictionary(&d).is_err() {
                ctx.warn("inflate_init(): Dictionary does not match expected dictionary (incorrect adler32 hash)")?;
            }
        }
    }
    let state = InflateState { z, encoding: bits, status: Z_OK, read_len: 0, dict };
    new_context(ctx, b"InflateContext", Box::new(state))
}

/// What one `inflate_add()` came to.
enum InflateOutcome {
    Data(Vec<u8>),
    Warn(String),
}

/// `inflate_add(InflateContext $context, string $data, int $flush_mode = ZLIB_SYNC_FLUSH): string|false`
fn inflate_add(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "inflate_add";
    const CHUNK: usize = 8192;
    let o = context_arg(ctx, &args[0], F, "InflateContext")?;
    let data = bytes(&args[1]);
    let flush = args.get(2).map_or(2, |v| v.deref().to_int());
    check_flush(F, flush)?;
    let finish = flush == Z_FINISH as i64;
    let r = o.with_payload::<InflateState, _>(|st| {
        // php resets a finished stream lazily, so `inflate_get_read_len()`
        // still answers for it until the next call.
        if st.status == Z_STREAM_END {
            st.status = Z_OK;
            st.z = new_inflater(st.encoding, &[]);
            st.read_len = 0;
        }
        if data.is_empty() && !finish {
            return InflateOutcome::Data(Vec::new());
        }
        let mut out = vec![0u8; data.len().max(CHUNK)];
        let mut used = 0usize;
        let mut in_pos = 0usize;
        loop {
            let (status, u, made) = inflate_step(&mut st.z, &data[in_pos..], &mut out[used..], finish);
            in_pos += u;
            used += made;
            st.read_len += u as u64;
            st.status = status;
            match status {
                Z_OK => {
                    if used == out.len() {
                        out.resize(out.len() + CHUNK, 0);
                        continue;
                    }
                    break;
                }
                Z_STREAM_END | Z_BUF_ERROR => break,
                Z_NEED_DICT => match st.dict.take() {
                    Some(d) => {
                        if st.z.set_dictionary(&d).is_err() {
                            return InflateOutcome::Warn(
                                "Dictionary does not match expected dictionary (incorrect adler32 hash)".into(),
                            );
                        }
                    }
                    None => {
                        return InflateOutcome::Warn(
                            "Inflating this data requires a preset dictionary, please specify it in the options array of inflate_init()".into(),
                        )
                    }
                },
                e => return InflateOutcome::Warn(deflate::z_error(e).into()),
            }
        }
        out.truncate(used);
        InflateOutcome::Data(out)
    });
    match r {
        Some(InflateOutcome::Data(out)) => Ok(string_value(out)),
        Some(InflateOutcome::Warn(msg)) => {
            ctx.warn(&format!("{F}(): {msg}"))?;
            Ok(Value::Bool(false))
        }
        None => Ok(Value::Bool(false)),
    }
}

/// `inflate_get_status(InflateContext $context): int`
fn inflate_get_status(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = context_arg(ctx, &args[0], "inflate_get_status", "InflateContext")?;
    Ok(Value::Int(o.with_payload::<InflateState, _>(|st| st.status as i64).unwrap_or(0)))
}

/// `inflate_get_read_len(InflateContext $context): int`
fn inflate_get_read_len(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = context_arg(ctx, &args[0], "inflate_get_read_len", "InflateContext")?;
    Ok(Value::Int(o.with_payload::<InflateState, _>(|st| st.read_len as i64).unwrap_or(0)))
}

// ---- the context classes --------------------------------------------------

/// `final class DeflateContext` / `final class InflateContext`: made only
/// by their `*_init()` function, never cloned, never serialized.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("InflateContext")
        .flags(ClassFlags::FINAL)
        .native_init(inflate_not_constructible)
        .method_vis("__serialize", Visibility::Private, nm!(0, Some(0), inflate_serialize))
        .method_vis("__unserialize", Visibility::Private, nm!(1, Some(1), inflate_unserialize))
        .uncloneable()
        .finish();
    r.class("DeflateContext")
        .flags(ClassFlags::FINAL)
        .native_init(deflate_not_constructible)
        .method_vis("__serialize", Visibility::Private, nm!(0, Some(0), deflate_serialize))
        .method_vis("__unserialize", Visibility::Private, nm!(1, Some(1), deflate_unserialize))
        .uncloneable()
        .finish();
}

fn inflate_not_constructible(_: &mut rphp_runtime::Interp, _: &Object) -> Result<(), Unwind> {
    Err(Unwind::error("Cannot directly construct InflateContext, use inflate_init() instead"))
}

fn deflate_not_constructible(_: &mut rphp_runtime::Interp, _: &Object) -> Result<(), Unwind> {
    Err(Unwind::error("Cannot directly construct DeflateContext, use deflate_init() instead"))
}

fn inflate_serialize(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception("Exception", "Serialization of 'InflateContext' is not allowed"))
}

fn inflate_unserialize(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception("Exception", "Unserialization of 'InflateContext' is not allowed"))
}

fn deflate_serialize(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception("Exception", "Serialization of 'DeflateContext' is not allowed"))
}

fn deflate_unserialize(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception("Exception", "Unserialization of 'DeflateContext' is not allowed"))
}
