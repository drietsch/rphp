//! Image metadata (ext/standard `image.c` and `iptc.c`): `getimagesize()`
//! and `getimagesizefromstring()` over every format php recognises by its
//! signature, `image_type_to_mime_type()` / `image_type_to_extension()`, the
//! `IMAGETYPE_*` constants, and the IPTC block functions `iptcparse()` /
//! `iptcembed()`.
//!
//! The detectors read the input the way php's stream code does — a cursor
//! that reads, `getc`s and seeks relative to where the signature sniffing
//! left it — so short, truncated and corrupt inputs fail the way they do in
//! php. AVIF and HEIF go through a port of libavifinfo's box walk (the
//! parser php bundles); SVG, which php 8.5 recognises through ext/libxml,
//! through a small well-formedness scanner that sees what libxml's reader
//! has parsed when the root element arrives (512-byte chunks).

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf_ref!("getimagesize", 1, Some(2), 0b10, getimagesize),
    nf_ref!("getimagesizefromstring", 1, Some(2), 0b10, getimagesizefromstring),
    nf!("image_type_to_mime_type", 1, Some(1), image_type_to_mime_type),
    nf!("image_type_to_extension", 1, Some(2), image_type_to_extension),
    nf!("iptcparse", 1, Some(1), iptcparse),
    nf!("iptcembed", 2, Some(3), iptcembed),
];

// ---- image types -------------------------------------------------------------

const T_UNKNOWN: i64 = 0;
const T_GIF: i64 = 1;
const T_JPEG: i64 = 2;
const T_PNG: i64 = 3;
const T_SWF: i64 = 4;
const T_PSD: i64 = 5;
const T_BMP: i64 = 6;
const T_TIFF_II: i64 = 7;
const T_TIFF_MM: i64 = 8;
const T_JPC: i64 = 9;
const T_JP2: i64 = 10;
const T_JPX: i64 = 11;
const T_JB2: i64 = 12;
const T_SWC: i64 = 13;
const T_IFF: i64 = 14;
const T_WBMP: i64 = 15;
const T_XBM: i64 = 16;
const T_ICO: i64 = 17;
const T_WEBP: i64 = 18;
const T_AVIF: i64 = 19;
const T_HEIF: i64 = 20;
const T_SVG: i64 = 21;
const T_COUNT: i64 = 22;

pub(crate) fn register_constants(r: &mut Registry) {
    // In php's registration order.
    for (name, v) in [
        ("IMAGETYPE_GIF", T_GIF),
        ("IMAGETYPE_JPEG", T_JPEG),
        ("IMAGETYPE_PNG", T_PNG),
        ("IMAGETYPE_SWF", T_SWF),
        ("IMAGETYPE_PSD", T_PSD),
        ("IMAGETYPE_BMP", T_BMP),
        ("IMAGETYPE_TIFF_II", T_TIFF_II),
        ("IMAGETYPE_TIFF_MM", T_TIFF_MM),
        ("IMAGETYPE_JPC", T_JPC),
        ("IMAGETYPE_JP2", T_JP2),
        ("IMAGETYPE_JPX", T_JPX),
        ("IMAGETYPE_JB2", T_JB2),
        ("IMAGETYPE_SWC", T_SWC),
        ("IMAGETYPE_IFF", T_IFF),
        ("IMAGETYPE_WBMP", T_WBMP),
        ("IMAGETYPE_JPEG2000", T_JPC),
        ("IMAGETYPE_XBM", T_XBM),
        ("IMAGETYPE_ICO", T_ICO),
        ("IMAGETYPE_WEBP", T_WEBP),
        ("IMAGETYPE_AVIF", T_AVIF),
        ("IMAGETYPE_HEIF", T_HEIF),
        ("IMAGETYPE_UNKNOWN", T_UNKNOWN),
        ("IMAGETYPE_COUNT", T_COUNT),
        ("IMAGETYPE_SVG", T_SVG),
    ] {
        r.constant(name, Value::Int(v));
    }
}

#[allow(dead_code)]
pub(crate) fn register_classes(_r: &mut Registry) {}

/// php's `php_image_type_to_mime_type()`.
fn mime_of(t: i64) -> &'static str {
    match t {
        T_GIF => "image/gif",
        T_JPEG => "image/jpeg",
        T_PNG => "image/png",
        T_SWF | T_SWC => "application/x-shockwave-flash",
        T_PSD => "image/psd",
        T_BMP => "image/bmp",
        T_TIFF_II | T_TIFF_MM => "image/tiff",
        T_JP2 => "image/jp2",
        T_IFF => "image/iff",
        T_WBMP => "image/vnd.wap.wbmp",
        T_XBM => "image/xbm",
        T_ICO => "image/vnd.microsoft.icon",
        T_WEBP => "image/webp",
        T_AVIF => "image/avif",
        T_HEIF => "image/heif",
        T_SVG => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// The extension `image_type_to_extension()` names, without the dot.
fn extension_of(t: i64) -> Option<&'static str> {
    Some(match t {
        T_GIF => "gif",
        T_JPEG => "jpeg",
        T_PNG => "png",
        T_SWF | T_SWC => "swf",
        T_PSD => "psd",
        T_BMP | T_WBMP => "bmp",
        T_TIFF_II | T_TIFF_MM => "tiff",
        T_IFF => "iff",
        T_JPC => "jpc",
        T_JP2 => "jp2",
        T_JPX => "jpx",
        T_JB2 => "jb2",
        T_XBM => "xbm",
        T_ICO => "ico",
        T_WEBP => "webp",
        T_AVIF => "avif",
        T_HEIF => "heif",
        T_SVG => "svg",
        _ => return None,
    })
}

/// `image_type_to_mime_type(int $image_type): string`
fn image_type_to_mime_type(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::string(mime_of(args[0].to_int()).as_bytes()))
}

/// `image_type_to_extension(int $image_type, bool $include_dot = true): string|false`
fn image_type_to_extension(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let dot = args.get(1).is_none_or(Value::to_bool);
    Ok(match extension_of(args[0].to_int()) {
        Some(e) if dot => Value::string(format!(".{e}").as_bytes()),
        Some(e) => Value::string(e.as_bytes()),
        None => Value::Bool(false),
    })
}

// ---- the input cursor --------------------------------------------------------

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// A php stream as the detectors see it: a plain file, or (for
/// `getimagesizefromstring`) a memory stream, whose seeks past the end fail.
struct Src {
    r: Box<dyn ReadSeek>,
    /// The length of a memory stream.
    mem_len: Option<u64>,
    eof: bool,
}

impl Src {
    fn memory(data: Vec<u8>) -> Src {
        Src { mem_len: Some(data.len() as u64), r: Box::new(Cursor::new(data)), eof: false }
    }

    /// Up to `n` bytes.
    fn read(&mut self, n: usize) -> Vec<u8> {
        let mut buf = vec![0u8; n];
        let mut got = 0;
        while got < n {
            match self.r.read(&mut buf[got..]) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.eof = true;
                    break;
                }
            }
        }
        buf.truncate(got);
        buf
    }

    /// Exactly `n` bytes, or `None` on a short read.
    fn read_n(&mut self, n: usize) -> Option<Vec<u8>> {
        let b = self.read(n);
        (b.len() == n).then_some(b)
    }

    /// `php_stream_getc`: a byte, or -1 at the end.
    fn getc(&mut self) -> i32 {
        self.read(1).first().map_or(-1, |&c| i32::from(c))
    }

    /// `php_read2`: big-endian, 0 on a short read.
    fn read2(&mut self) -> u32 {
        self.read_n(2).map_or(0, |a| u32::from(a[0]) << 8 | u32::from(a[1]))
    }

    /// `php_read4`: big-endian, 0 on a short read.
    fn read4(&mut self) -> u32 {
        self.read_n(4).map_or(0, |a| u32::from_be_bytes([a[0], a[1], a[2], a[3]]))
    }

    fn pos(&mut self) -> u64 {
        self.r.stream_position().unwrap_or(0)
    }

    /// `php_stream_seek(…, SEEK_CUR)`; `true` on success.
    fn seek_cur(&mut self, off: i64) -> bool {
        let target = self.pos() as i64 + off;
        self.seek_to(target)
    }

    fn seek_to(&mut self, target: i64) -> bool {
        if target < 0 {
            return false;
        }
        let mut target = target as u64;
        let mut ok = true;
        if let Some(len) = self.mem_len {
            if target > len {
                target = len;
                ok = false;
            }
        }
        self.eof = false;
        self.r.seek(SeekFrom::Start(target)).is_ok() && ok
    }

    fn rewind(&mut self) -> bool {
        self.seek_to(0)
    }

    /// Everything from the start.
    fn all(&mut self) -> Vec<u8> {
        self.rewind();
        let mut out = Vec::new();
        let _ = self.r.read_to_end(&mut out);
        out
    }
}

// ---- getimagesize ------------------------------------------------------------

/// What a detector found (php's `struct gfxinfo`).
struct Gfx {
    width: u32,
    height: u32,
    bits: u32,
    channels: u32,
    units: Option<(String, String)>,
}

impl Gfx {
    fn new(width: u32, height: u32, bits: u32, channels: u32) -> Gfx {
        Gfx { width, height, bits, channels, units: None }
    }
}

/// `getimagesize(string $filename, &$image_info = null): array|false`
fn getimagesize(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const F: &str = "getimagesize";
    let name = args[0].to_php_bytes().to_vec();
    if name.contains(&0) {
        return Err(Unwind::value_error(
            "getimagesize(): Argument #1 ($filename) must not contain any null bytes",
        ));
    }
    let want_info = args.len() > 1;
    if want_info {
        args[1] = Value::Array(Array::new());
    }
    if name.is_empty() {
        return Err(Unwind::value_error("Path must not be empty"));
    }
    let shown = String::from_utf8_lossy(&name).into_owned();
    let mut src = if name.windows(3).any(|w| w == b"://") && !name.starts_with(b"file://") {
        // Another wrapper (`data:`, `http:`, …): read it through the stream layer.
        match ctx.call_function(b"file_get_contents", &[Value::Str(Str::from_vec(name.clone()))])? {
            Value::Str(s) => Src::memory(s.as_bytes().to_vec()),
            _ => return Ok(Value::Bool(false)),
        }
    } else {
        let plain = name.strip_prefix(b"file://").unwrap_or(&name);
        let p = crate::filestat::arg_path(ctx, &Value::Str(Str::from_vec(plain.to_vec())));
        match std::fs::File::open(&p) {
            Ok(f) => {
                if f.metadata().is_ok_and(|m| m.is_dir()) {
                    ctx.notice(&format!("{F}(): Read of 8192 bytes failed with errno=21 Is a directory"))?;
                    Src::memory(Vec::new())
                } else {
                    Src { r: Box::new(std::io::BufReader::new(f)), mem_len: None, eof: false }
                }
            }
            Err(e) => {
                ctx.warn(&format!(
                    "{F}({shown}): Failed to open stream: {}",
                    crate::filestat::io_text(&e)
                ))?;
                return Ok(Value::Bool(false));
            }
        }
    };
    let mut info = Array::new();
    let r = image_size(ctx, F, &mut src, &shown, want_info.then_some(&mut info))?;
    if want_info {
        args[1] = Value::Array(info);
    }
    Ok(r)
}

/// `getimagesizefromstring(string $string, &$image_info = null): array|false`
fn getimagesizefromstring(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let data = args[0].to_php_bytes().to_vec();
    let want_info = args.len() > 1;
    // php formats the input with `%s`: it ends at the first NUL.
    let shown = &data[..data.iter().position(|&c| c == 0).unwrap_or(data.len())];
    let shown = String::from_utf8_lossy(shown).into_owned();
    let mut src = Src::memory(data);
    let mut info = Array::new();
    let r = image_size(ctx, "getimagesizefromstring", &mut src, &shown, want_info.then_some(&mut info))?;
    if want_info {
        args[1] = Value::Array(info);
    }
    Ok(r)
}

/// php's `php_getimagesize_from_stream()`: sniff the type, run its
/// detector, build the result array.
fn image_size(
    ctx: &mut Ctx,
    func: &str,
    src: &mut Src,
    input: &str,
    info: Option<&mut Array>,
) -> NativeResult {
    let (itype, svg) = image_type(ctx, func, src, input)?;
    let gfx = match itype {
        T_GIF => handle_gif(src),
        T_JPEG => handle_jpeg(ctx, func, src, info)?,
        T_PNG => handle_png(src),
        T_SWF => handle_swf(src),
        T_SWC => handle_swc(src),
        T_PSD => handle_psd(src),
        T_BMP => handle_bmp(src),
        T_JPC => handle_jpc(ctx, func, src)?,
        T_JP2 => handle_jp2(ctx, func, src)?,
        T_TIFF_II => handle_tiff(src, false),
        T_TIFF_MM => handle_tiff(src, true),
        T_IFF => handle_iff(src),
        T_WBMP => get_wbmp(src).map(|(w, h)| Gfx::new(w, h, 0, 0)),
        T_XBM => get_xbm(src).map(|(w, h)| Gfx::new(w, h, 0, 0)),
        T_ICO => handle_ico(src),
        T_WEBP => handle_webp(src),
        T_AVIF => avif::features(src, true),
        T_HEIF => avif::features(src, false),
        T_SVG => svg,
        _ => None,
    };
    let Some(g) = gfx else {
        return Ok(Value::Bool(false));
    };
    let (wu, hu) = g.units.unwrap_or_else(|| ("px".to_owned(), "px".to_owned()));
    let mut a = Array::new();
    a.set(ArrayKey::Int(0), Value::Int(i64::from(g.width)));
    a.set(ArrayKey::Int(1), Value::Int(i64::from(g.height)));
    a.set(ArrayKey::Int(2), Value::Int(itype));
    if wu == "px" && hu == "px" {
        let attr = format!("width=\"{}\" height=\"{}\"", g.width as i32, g.height as i32);
        a.set(ArrayKey::Int(3), Value::string(attr.as_bytes()));
    }
    if g.bits != 0 {
        a.set(ArrayKey::str(b"bits"), Value::Int(i64::from(g.bits)));
    }
    if g.channels != 0 {
        a.set(ArrayKey::str(b"channels"), Value::Int(i64::from(g.channels)));
    }
    a.set(ArrayKey::str(b"mime"), Value::string(mime_of(itype).as_bytes()));
    a.set(ArrayKey::str(b"width_unit"), Value::string(wu.as_bytes()));
    a.set(ArrayKey::str(b"height_unit"), Value::string(hu.as_bytes()));
    Ok(Value::Array(a))
}

const SIG_PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
const SIG_JP2: &[u8] = b"\x00\x00\x00\x0cjP  \x0d\x0a\x87\x0a";

/// php's `php_getimagetype()`: the type the leading bytes announce. An
/// SVG's result comes along, since recognising it means parsing it.
fn image_type(ctx: &mut Ctx, func: &str, src: &mut Src, input: &str) -> Result<(i64, Option<Gfx>), Unwind> {
    let unreadable = |ctx: &mut Ctx| ctx.notice(&format!("{func}(): Error reading from {input}!"));
    let mut ft = src.read(3);
    if ft.len() != 3 {
        unreadable(ctx)?;
        return Ok((T_UNKNOWN, None));
    }
    let t = match &ft[..] {
        b"GIF" => T_GIF,
        [0xff, 0xd8, 0xff] => T_JPEG,
        [0x89, b'P', b'N'] => {
            let more = src.read(5);
            if more.len() != 5 {
                unreadable(ctx)?;
                return Ok((T_UNKNOWN, None));
            }
            ft.extend(more);
            if ft == SIG_PNG {
                T_PNG
            } else {
                ctx.warn(&format!("{func}(): PNG file corrupted by ASCII conversion"))?;
                return Ok((T_UNKNOWN, None));
            }
        }
        b"FWS" => T_SWF,
        b"CWS" => T_SWC,
        b"8BP" => T_PSD,
        [b'B', b'M', _] => T_BMP,
        [0xff, 0x4f, 0xff] => T_JPC,
        b"RIF" => {
            let more = src.read(9);
            if more.len() != 9 {
                unreadable(ctx)?;
                return Ok((T_UNKNOWN, None));
            }
            ft.extend(more);
            return Ok((if &ft[8..12] == b"WEBP" { T_WEBP } else { T_UNKNOWN }, None));
        }
        _ => T_UNKNOWN,
    };
    if t != T_UNKNOWN {
        return Ok((t, None));
    }
    let more = src.read(1);
    if more.len() != 1 {
        unreadable(ctx)?;
        return Ok((T_UNKNOWN, None));
    }
    ft.extend(more);
    let t = match &ft[..] {
        b"II\x2a\x00" => T_TIFF_II,
        b"MM\x00\x2a" => T_TIFF_MM,
        b"FORM" => T_IFF,
        b"\x00\x00\x01\x00" => T_ICO,
        _ => T_UNKNOWN,
    };
    if t != T_UNKNOWN {
        return Ok((t, None));
    }
    let more = src.read(8);
    let twelve = more.len() == 8;
    ft.extend(more);
    if twelve {
        if ft == SIG_JP2 {
            return Ok((T_JP2, None));
        }
        if avif::identify(src) {
            return Ok((T_AVIF, None));
        }
        if &ft[4..8] == b"ftyp" && matches!(&ft[8..12], b"mif1" | b"heic" | b"heix") {
            return Ok((T_HEIF, None));
        }
    }
    if get_wbmp(src).is_some() {
        return Ok((T_WBMP, None));
    }
    if !twelve {
        unreadable(ctx)?;
        return Ok((T_UNKNOWN, None));
    }
    if get_xbm(src).is_some() {
        return Ok((T_XBM, None));
    }
    let data = src.all();
    if let Some(g) = svg::size(&data) {
        return Ok((T_SVG, Some(g)));
    }
    Ok((T_UNKNOWN, None))
}

fn handle_gif(src: &mut Src) -> Option<Gfx> {
    if !src.seek_cur(3) {
        return None;
    }
    let d = src.read_n(5)?;
    let bits = if d[4] & 0x80 != 0 { u32::from(d[4] & 0x07) + 1 } else { 0 };
    Some(Gfx::new(u32::from(d[0]) | u32::from(d[1]) << 8, u32::from(d[2]) | u32::from(d[3]) << 8, bits, 3))
}

fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

fn le32(d: &[u8]) -> u32 {
    u32::from_le_bytes([d[0], d[1], d[2], d[3]])
}

fn handle_psd(src: &mut Src) -> Option<Gfx> {
    if !src.seek_cur(11) {
        return None;
    }
    let d = src.read_n(8)?;
    Some(Gfx::new(be32(&d[4..]), be32(&d[..4]), 0, 0))
}

fn handle_bmp(src: &mut Src) -> Option<Gfx> {
    if !src.seek_cur(11) {
        return None;
    }
    let d = src.read_n(16)?;
    let size = le32(&d);
    if size == 12 {
        let w = u32::from(d[5]) << 8 | u32::from(d[4]);
        let h = u32::from(d[7]) << 8 | u32::from(d[6]);
        Some(Gfx::new(w, h, u32::from(d[11]), 0))
    } else if size > 12 && (size <= 64 || size == 108 || size == 124) {
        let w = le32(&d[4..]);
        let h = (le32(&d[8..]) as i32).unsigned_abs();
        let bits = u32::from(d[15]) << 8 | u32::from(d[14]);
        Some(Gfx::new(w, h, bits, 0))
    } else {
        None
    }
}

fn handle_png(src: &mut Src) -> Option<Gfx> {
    if !src.seek_cur(8) {
        return None;
    }
    let d = src.read_n(9)?;
    Some(Gfx::new(be32(&d), be32(&d[4..]), u32::from(d[8]), 0))
}

/// php's `php_swf_get_bits()`: `count` bits from bit `pos`, MSB first.
fn swf_bits(b: &[u8], pos: usize, count: usize) -> u64 {
    let mut r: u64 = 0;
    for bit in pos..pos + count {
        let v = u64::from((b.get(bit / 8).copied().unwrap_or(0) >> (7 - (bit % 8))) & 1);
        r = r.wrapping_add(v << (count - (bit - pos) - 1));
    }
    r
}

/// The frame rectangle at the start of an SWF body, in twips.
fn swf_rect(b: &[u8]) -> Gfx {
    let bits = swf_bits(b, 0, 5) as usize;
    let w = swf_bits(b, 5 + bits, bits).wrapping_sub(swf_bits(b, 5, bits)) / 20;
    let h = swf_bits(b, 5 + 3 * bits, bits).wrapping_sub(swf_bits(b, 5 + 2 * bits, bits)) / 20;
    Gfx::new(w as u32, h as u32, 0, 0)
}

fn handle_swf(src: &mut Src) -> Option<Gfx> {
    if !src.seek_cur(5) {
        return None;
    }
    let a = src.read_n(32)?;
    Some(swf_rect(&a))
}

/// A zlib-compressed SWF: the body after the 8-byte header must inflate
/// to its end; the frame rectangle is in the first 64 bytes.
fn handle_swc(src: &mut Src) -> Option<Gfx> {
    if !src.seek_cur(5) {
        return None;
    }
    src.read_n(64)?;
    if !src.seek_to(8) {
        return None;
    }
    let mut body = Vec::new();
    let _ = src.r.read_to_end(&mut body);
    let mut z = flate2::Decompress::new(true);
    let mut head: Vec<u8> = Vec::with_capacity(64);
    let mut buf = vec![0u8; 16384];
    loop {
        let before_in = z.total_in() as usize;
        let before_out = z.total_out();
        let st = z.decompress(&body[before_in..], &mut buf, flate2::FlushDecompress::Finish);
        let produced = (z.total_out() - before_out) as usize;
        let take = produced.min(64 - head.len());
        head.extend_from_slice(&buf[..take]);
        match st {
            Ok(flate2::Status::StreamEnd) => break,
            Ok(_) if produced > 0 || (z.total_in() as usize) > before_in => continue,
            _ => return None,
        }
    }
    head.resize(65, 0);
    Some(swf_rect(&head))
}

// JPEG markers.
const M_SOF0: i32 = 0xC0;
const M_SOF15: i32 = 0xCF;
const M_DHT: i32 = 0xC4;
const M_JPG: i32 = 0xC8;
const M_DAC: i32 = 0xCC;
const M_SOS: i32 = 0xDA;
const M_EOI: i32 = 0xD9;
const M_APP0: i32 = 0xE0;
const M_APP1: i32 = 0xE1;
const M_APP13: i32 = 0xED;
const M_APP15: i32 = 0xEF;

/// php's `php_next_marker()`.
fn next_marker(ctx: &mut Ctx, func: &str, src: &mut Src, ff_read: bool) -> Result<i32, Unwind> {
    if !ff_read {
        let mut extraneous = 0usize;
        loop {
            let m = src.getc();
            if m == 0xff {
                break;
            }
            if m < 0 {
                return Ok(M_EOI);
            }
            extraneous += 1;
        }
        if extraneous > 0 {
            ctx.warn(&format!("{func}(): Corrupt JPEG data: {extraneous} extraneous bytes before marker"))?;
        }
    }
    loop {
        let m = src.getc();
        if m < 0 {
            return Ok(M_EOI);
        }
        if m != 0xff {
            return Ok(m);
        }
    }
}

/// php's `php_skip_variable()`: `false` for a length below 2.
fn skip_variable(src: &mut Src) -> bool {
    let len = src.read2();
    if len < 2 {
        return false;
    }
    src.seek_cur(i64::from(len - 2));
    true
}

fn handle_jpeg(ctx: &mut Ctx, func: &str, src: &mut Src, mut info: Option<&mut Array>) -> Result<Option<Gfx>, Unwind> {
    let mut result: Option<Gfx> = None;
    let mut ff_read = true;
    loop {
        let marker = next_marker(ctx, func, src, ff_read)?;
        ff_read = false;
        match marker {
            M_SOF0..=M_SOF15 if marker != M_DHT && marker != M_JPG && marker != M_DAC => {
                if result.is_none() {
                    let length = src.read2();
                    let bits = src.getc() as u32;
                    let height = src.read2();
                    let width = src.read2();
                    let channels = src.getc() as u32;
                    result = Some(Gfx::new(width, height, bits, channels));
                    if info.is_none() || length < 8 {
                        return Ok(result);
                    }
                    if !src.seek_cur(i64::from(length) - 8) {
                        return Ok(result);
                    }
                } else if !skip_variable(src) {
                    return Ok(result);
                }
            }
            M_APP0..=M_APP15 => {
                if let Some(info) = info.as_deref_mut() {
                    // php's `php_read_APP()`.
                    let length = src.read2();
                    if length < 2 {
                        return Ok(result);
                    }
                    let Some(data) = src.read_n(length as usize - 2) else {
                        return Ok(result);
                    };
                    let key = format!("APP{}", marker - M_APP0);
                    let key = ArrayKey::str(key.as_bytes());
                    if !info.contains_key(&key) {
                        info.set(key, Value::Str(Str::from_vec(data)));
                    }
                } else if !skip_variable(src) {
                    return Ok(result);
                }
            }
            M_SOS | M_EOI => return Ok(result),
            _ => {
                if !skip_variable(src) {
                    return Ok(result);
                }
            }
        }
    }
}

/// A JPEG 2000 codestream, positioned after `FF 4F FF`.
fn handle_jpc(ctx: &mut Ctx, func: &str, src: &mut Src) -> Result<Option<Gfx>, Unwind> {
    if src.getc() != 0x51 {
        ctx.warn(&format!(
            "{func}(): JPEG2000 codestream corrupt(Expected SIZ marker not found after SOC)"
        ))?;
        return Ok(None);
    }
    src.read2(); // Lsiz
    src.read2(); // Rsiz
    let width = src.read4();
    let height = src.read4();
    if !src.seek_cur(24) {
        return Ok(None);
    }
    let channels = src.read2();
    if (channels == 0 && src.eof) || channels > 256 {
        return Ok(None);
    }
    let mut highest = 0u32;
    for _ in 0..channels {
        let depth = (src.getc() + 1) as u32;
        highest = highest.max(depth);
        src.getc();
        src.getc();
    }
    Ok(Some(Gfx::new(width, height, highest, channels)))
}

/// A JP2 file: the codestream of the first `jp2c` box at the top level.
fn handle_jp2(ctx: &mut Ctx, func: &str, src: &mut Src) -> Result<Option<Gfx>, Unwind> {
    let mut result = None;
    loop {
        let len = src.read4();
        let Some(ty) = src.read_n(4) else { break };
        if len == 1 {
            return Ok(None);
        }
        if ty == b"jp2c" {
            src.seek_cur(3);
            result = handle_jpc(ctx, func, src)?;
            break;
        }
        if (len as i32) <= 0 {
            break;
        }
        if !src.seek_cur(i64::from(len) - 8) {
            break;
        }
    }
    if result.is_none() {
        ctx.warn(&format!("{func}(): JP2 file has no codestreams at root level"))?;
    }
    Ok(result)
}

fn handle_tiff(src: &mut Src, motorola: bool) -> Option<Gfx> {
    let g16 = |d: &[u8]| -> u32 {
        if motorola {
            u32::from(d[0]) << 8 | u32::from(d[1])
        } else {
            u32::from(d[1]) << 8 | u32::from(d[0])
        }
    };
    let g32 = |d: &[u8]| -> u32 { if motorola { be32(d) } else { le32(d) } };
    let ptr = src.read_n(4)?;
    let ifd = g32(&ptr);
    if !src.seek_cur(i64::from(ifd) - 8) {
        return None;
    }
    let n = src.read_n(2)?;
    let entries = g16(&n) as usize;
    let dir = src.read_n(12 * entries + 4)?;
    let (mut width, mut height) = (0u64, 0u64);
    for i in 0..entries {
        let e = &dir[i * 12..i * 12 + 12];
        let tag = g16(e);
        let value: u64 = match g16(&e[2..]) {
            1 | 6 => u64::from(e[8]),
            3 => u64::from(g16(&e[8..])),
            8 => i64::from(g16(&e[8..]) as u16 as i16) as u64,
            4 => u64::from(g32(&e[8..])),
            9 => i64::from(g32(&e[8..]) as i32) as u64,
            _ => continue,
        };
        match tag {
            0x100 | 0xA002 => width = value,
            0x101 | 0xA003 => height = value,
            _ => {}
        }
    }
    (width != 0 && height != 0).then(|| Gfx::new(width as u32, height as u32, 0, 0))
}

fn handle_iff(src: &mut Src) -> Option<Gfx> {
    let a = src.read_n(8)?;
    if &a[4..8] != b"ILBM" && &a[4..8] != b"PBM " {
        return None;
    }
    loop {
        let a = src.read_n(8)?;
        let id = be32(&a);
        let mut size = be32(&a[4..]) as i32;
        if size < 0 {
            return None;
        }
        if size & 1 == 1 {
            size += 1;
        }
        if id == 0x424d_4844 {
            if size < 9 {
                return None;
            }
            let b = src.read_n(9)?;
            let w = i16::from_be_bytes([b[0], b[1]]);
            let h = i16::from_be_bytes([b[2], b[3]]);
            let bits = i16::from(b[8]);
            if w > 0 && h > 0 && bits > 0 && bits < 33 {
                return Some(Gfx::new(w as u32, h as u32, bits as u32, 0));
            }
        } else if !src.seek_cur(i64::from(size)) {
            return None;
        }
    }
}

fn handle_ico(src: &mut Src) -> Option<Gfx> {
    let d = src.read_n(2)?;
    let mut n = u32::from(d[1]) << 8 | u32::from(d[0]);
    if !(1..=255).contains(&n) {
        return None;
    }
    let mut g = Gfx::new(0, 0, 0, 0);
    while n > 0 {
        let Some(d) = src.read_n(16) else { break };
        let bits = u32::from(d[7]) << 8 | u32::from(d[6]);
        if bits >= g.bits {
            g.width = u32::from(d[0]);
            g.height = u32::from(d[1]);
            g.bits = bits;
        }
        n -= 1;
    }
    if g.width == 0 {
        g.width = 256;
    }
    if g.height == 0 {
        g.height = 256;
    }
    Some(g)
}

fn handle_webp(src: &mut Src) -> Option<Gfx> {
    let b = src.read_n(18)?;
    if &b[..3] != b"VP8" {
        return None;
    }
    let u = |i: usize| u32::from(b[i]);
    let (w, h) = match b[3] {
        b' ' => (u(14) + ((u(15) & 0x3F) << 8), u(16) + ((u(17) & 0x3F) << 8)),
        b'L' => (
            u(9) + ((u(10) & 0x3F) << 8) + 1,
            (u(10) >> 6) + (u(11) << 2) + ((u(12) & 0xF) << 10) + 1,
        ),
        b'X' => (u(12) + (u(13) << 8) + (u(14) << 16) + 1, u(15) + (u(16) << 8) + (u(17) << 16) + 1),
        _ => return None,
    };
    Some(Gfx::new(w, h, 8, 0))
}

/// php's `php_get_wbmp()`: type 0, a header, then width and height as
/// multi-byte integers (at most 2048 each, neither 0).
fn get_wbmp(src: &mut Src) -> Option<(u32, u32)> {
    if !src.rewind() || src.getc() != 0 {
        return None;
    }
    loop {
        let i = src.getc();
        if i < 0 {
            return None;
        }
        if i & 0x80 == 0 {
            break;
        }
    }
    let mut dim = [0u32; 2];
    for d in &mut dim {
        loop {
            let i = src.getc();
            if i < 0 {
                return None;
            }
            *d = (*d << 7) | (i as u32 & 0x7f);
            if *d > 2048 {
                return None;
            }
            if i & 0x80 == 0 {
                break;
            }
        }
    }
    (dim[0] != 0 && dim[1] != 0).then_some((dim[0], dim[1]))
}

/// `sscanf(line, "#define %s %d")`: the name and the number.
fn scan_define(line: &[u8]) -> Option<(&[u8], i32)> {
    let rest = line.strip_prefix(b"#define")?;
    let ws = |c: &u8| c.is_ascii_whitespace();
    let start = rest.iter().position(|c| !ws(c))?;
    let rest = &rest[start..];
    let end = rest.iter().position(ws).unwrap_or(rest.len());
    let (name, rest) = rest.split_at(end);
    let start = rest.iter().position(|c| !ws(c))?;
    let mut rest = &rest[start..];
    let neg = match rest.first() {
        Some(b'-') => {
            rest = &rest[1..];
            true
        }
        Some(b'+') => {
            rest = &rest[1..];
            false
        }
        _ => false,
    };
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let mut v: i64 = 0;
    for &c in &rest[..digits] {
        v = v.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    Some((name, if neg { v.wrapping_neg() as i32 } else { v as i32 }))
}

/// php's `php_get_xbm()`: `#define <name>_width N` and `…_height N` lines.
fn get_xbm(src: &mut Src) -> Option<(u32, u32)> {
    let data = src.all();
    let (mut width, mut height) = (0u32, 0u32);
    for line in data.split_inclusive(|&c| c == b'\n') {
        let line = &line[..line.iter().position(|&c| c == 0).unwrap_or(line.len())];
        let Some((name, value)) = scan_define(line) else { continue };
        let ty = match name.iter().rposition(|&c| c == b'_') {
            Some(i) => &name[i + 1..],
            None => name,
        };
        if ty == b"width" {
            width = value as u32;
            if height != 0 {
                break;
            }
        }
        if ty == b"height" {
            height = value as u32;
            if width != 0 {
                break;
            }
        }
    }
    (width != 0 && height != 0).then_some((width, height))
}

// ---- AVIF / HEIF ---------------------------------------------------------------

/// A port of libavifinfo (the parser php bundles for AVIF, and reuses for
/// HEIF): walk the ISOBMFF boxes to the primary item's `ispe` (size) and
/// `pixi`/`av1C` (depth, channels), plus one channel for an alpha `auxC`.
mod avif {
    use super::{Gfx, Src};

    const MAX_PROPS: usize = 32;
    const MAX_FEATURES: usize = 8;
    const MAX_TILES: usize = 16;
    const MAX_VALUE: u32 = 255;
    const MAX_BOXES: u32 = 4096;

    /// Any status but "found" or "not found yet" (php treats them alike).
    struct Fail;

    type R<T> = Result<T, Fail>;

    #[derive(Default)]
    struct Features {
        has_primary: bool,
        has_alpha: bool,
        primary: u32,
        width: u32,
        height: u32,
        bit_depth: u32,
        num_channels: u32,
        has_gainmap: bool,
        tone_mapped: u32,
        iinf_parsed: bool,
        iref_parsed: bool,
        /// (tile item, parent item, dimg index)
        tiles: Vec<(u32, u32, u32)>,
        /// (property index, item)
        props: Vec<(u32, u32)>,
        dim_props: Vec<(u32, u32, u32)>,
        chan_props: Vec<(u32, u32, u32)>,
        boxes: u32,
    }

    struct BoxHdr {
        size: u64,
        ty: [u8; 4],
        version: u8,
        flags: u32,
        content: u64,
    }

    fn read(src: &mut Src, n: usize) -> R<Vec<u8>> {
        src.read_n(n).ok_or(Fail)
    }

    fn read_uint(src: &mut Src, n: usize) -> R<u32> {
        Ok(read(src, n)?.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)))
    }

    fn skip(src: &mut Src, n: u64) -> R<()> {
        if src.seek_cur(n as i64) { Ok(()) } else { Err(Fail) }
    }

    fn parse_box(src: &mut Src, f: &mut Features, remaining: u64) -> R<BoxHdr> {
        let h = read(src, 8)?;
        let mut header: u64 = 8;
        let mut size = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
        let mut ty = [h[4], h[5], h[6], h[7]];
        if size == 1 {
            let l = read(src, 8)?;
            header += 8;
            size = u64::from_be_bytes([l[0], l[1], l[2], l[3], l[4], l[5], l[6], l[7]]);
        } else if size == 0 {
            // The box extends to the end of the file: unusable here.
            return Err(Fail);
        }
        if &ty == b"uuid" {
            read(src, 16)?;
            header += 16;
        }
        let full = matches!(&ty, b"meta" | b"pitm" | b"ipma" | b"ispe" | b"pixi" | b"iref" | b"auxC" | b"iinf" | b"infe");
        if full {
            header += 4;
        }
        if size < header || size > remaining {
            return Err(Fail);
        }
        f.boxes += 1;
        if f.boxes >= MAX_BOXES {
            return Err(Fail);
        }
        let (mut version, mut flags) = (0u8, 0u32);
        if full {
            let d = read(src, 4)?;
            version = d[0];
            flags = u32::from(d[1]) << 16 | u32::from(d[2]) << 8 | u32::from(d[3]);
            let parsable = match &ty {
                b"meta" | b"ispe" | b"pixi" | b"auxC" => version == 0,
                b"pitm" | b"ipma" | b"iref" | b"iinf" => version <= 1,
                b"infe" => (2..=3).contains(&version),
                _ => true,
            };
            if !parsable {
                ty = *b"skip";
            }
        }
        Ok(BoxHdr { size, ty, version, flags, content: size - header })
    }

    fn item_features(f: &mut Features, target: u32, depth: u32) -> bool {
        for pi in 0..f.props.len() {
            let (index, item) = f.props[pi];
            if item != target {
                continue;
            }
            if target == f.primary && (f.width == 0 || f.height == 0) {
                if let Some(&(_, w, h)) = f.dim_props.iter().find(|d| d.0 == index) {
                    f.width = w;
                    f.height = h;
                    if f.bit_depth != 0 && f.num_channels != 0 {
                        return true;
                    }
                }
            }
            if f.bit_depth == 0 || f.num_channels == 0 {
                if let Some(&(_, d, c)) = f.chan_props.iter().find(|d| d.0 == index) {
                    f.bit_depth = d;
                    f.num_channels = c;
                    if f.width != 0 && f.height != 0 {
                        return true;
                    }
                }
            }
        }
        if depth < 3 {
            let tiles: Vec<u32> = f.tiles.iter().filter(|t| t.1 == target).map(|t| t.0).collect();
            for t in tiles {
                if item_features(f, t, depth + 1) {
                    return true;
                }
            }
        }
        false
    }

    fn primary_features(f: &mut Features) -> bool {
        if !f.has_primary || f.dim_props.is_empty() || f.chan_props.is_empty() {
            return false;
        }
        if f.tone_mapped != 0 && f.tiles.iter().any(|t| t.1 == f.tone_mapped && t.2 == 1) {
            f.has_gainmap = true;
        }
        if !f.has_gainmap && (!f.iinf_parsed || (f.tone_mapped != 0 && !f.iref_parsed)) {
            return false;
        }
        let primary = f.primary;
        if !item_features(f, primary, 0) {
            return false;
        }
        if f.has_alpha {
            f.num_channels += 1;
        }
        true
    }

    fn parse_ipco(src: &mut Src, f: &mut Features, mut remaining: u64) -> R<()> {
        let mut index: u32 = 1;
        while remaining > 0 {
            let b = parse_box(src, f, remaining)?;
            match &b.ty {
                b"ispe" => {
                    if b.content < 8 {
                        return Err(Fail);
                    }
                    let w = read_uint(src, 4)?;
                    let h = read_uint(src, 4)?;
                    if w == 0 || h == 0 {
                        return Err(Fail);
                    }
                    if f.dim_props.len() < MAX_FEATURES && index <= MAX_VALUE {
                        f.dim_props.push((index, w, h));
                    }
                    skip(src, b.content - 8)?;
                }
                b"pixi" => {
                    if b.content < 1 {
                        return Err(Fail);
                    }
                    let n = read_uint(src, 1)?;
                    if n < 1 || b.content < 1 + u64::from(n) {
                        return Err(Fail);
                    }
                    let depth = read_uint(src, 1)?;
                    if depth < 1 {
                        return Err(Fail);
                    }
                    for i in 1..n {
                        if read_uint(src, 1)? != depth || i > 32 {
                            return Err(Fail);
                        }
                    }
                    if f.chan_props.len() < MAX_FEATURES && index <= MAX_VALUE {
                        f.chan_props.push((index, depth, n));
                    }
                    skip(src, b.content - 1 - u64::from(n))?;
                }
                b"av1C" => {
                    if b.content < 3 {
                        return Err(Fail);
                    }
                    let d = read(src, 3)?;
                    let high = d[2] & 0x40 != 0;
                    let twelve = d[2] & 0x20 != 0;
                    let mono = d[2] & 0x10 != 0;
                    if twelve && !high {
                        return Err(Fail);
                    }
                    if f.chan_props.len() < MAX_FEATURES && index <= MAX_VALUE {
                        let depth = if high { if twelve { 12 } else { 10 } } else { 8 };
                        f.chan_props.push((index, depth, if mono { 1 } else { 3 }));
                    }
                    skip(src, b.content - 3)?;
                }
                b"auxC" => {
                    const ALPHA: &[u8] = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0";
                    if b.content >= ALPHA.len() as u64 {
                        if read(src, ALPHA.len())? == ALPHA {
                            f.has_alpha = true;
                        }
                        skip(src, b.content - ALPHA.len() as u64)?;
                    } else {
                        skip(src, b.content)?;
                    }
                }
                _ => skip(src, b.content)?,
            }
            index += 1;
            remaining -= b.size;
        }
        Ok(())
    }

    fn parse_iprp(src: &mut Src, f: &mut Features, mut remaining: u64) -> R<bool> {
        while remaining > 0 {
            let b = parse_box(src, f, remaining)?;
            match &b.ty {
                b"ipco" => parse_ipco(src, f, b.content)?,
                b"ipma" => {
                    let mut used: u64 = 4;
                    if used > b.content {
                        return Err(Fail);
                    }
                    let count = read_uint(src, 4)?;
                    for _ in 0..count {
                        let id_len = if b.version < 1 { 2 } else { 4 };
                        used += id_len as u64 + 1;
                        if used > b.content {
                            return Err(Fail);
                        }
                        let item = read_uint(src, id_len)?;
                        let n = read_uint(src, 1)?;
                        for _ in 0..n {
                            let idx = if b.flags & 1 != 0 {
                                used += 2;
                                read_uint(src, 2)? & 0x7fff
                            } else {
                                used += 1;
                                read_uint(src, 1)? & 0x7f
                            };
                            if used > b.content {
                                return Err(Fail);
                            }
                            if item <= MAX_VALUE && idx <= MAX_VALUE && f.props.len() < MAX_PROPS {
                                f.props.push((idx, item));
                            }
                        }
                        if primary_features(f) {
                            return Ok(true);
                        }
                    }
                    skip(src, b.content - used)?;
                }
                _ => skip(src, b.content)?,
            }
            remaining -= b.size;
        }
        Ok(false)
    }

    fn parse_iref(src: &mut Src, f: &mut Features, mut remaining: u64, version: u8) -> R<bool> {
        let id_len = if version == 0 { 2 } else { 4 };
        while remaining > 0 {
            let b = parse_box(src, f, remaining)?;
            if &b.ty == b"dimg" {
                let mut used = id_len as u64 + 2;
                if used > b.content {
                    return Err(Fail);
                }
                let from = read_uint(src, id_len)?;
                let n = read_uint(src, 2)?;
                for i in 0..n {
                    used += id_len as u64;
                    if used > b.content {
                        return Err(Fail);
                    }
                    let to = read_uint(src, id_len)?;
                    if from <= MAX_VALUE && to <= MAX_VALUE && f.tiles.len() < MAX_TILES {
                        f.tiles.push((to, from, i));
                    }
                }
                if primary_features(f) {
                    return Ok(true);
                }
                skip(src, b.content - used)?;
            } else {
                skip(src, b.content)?;
            }
            remaining -= b.size;
        }
        f.iref_parsed = true;
        Ok(primary_features(f))
    }

    fn parse_iinf(src: &mut Src, f: &mut Features, content: u64, version: u8) -> R<bool> {
        let n = if version == 0 { 2 } else { 4 };
        if n > content {
            return Err(Fail);
        }
        let count = read_uint(src, n as usize)?;
        let mut remaining = content - n;
        for _ in 0..count {
            if remaining == 0 {
                break;
            }
            let b = parse_box(src, f, remaining)?;
            if &b.ty == b"infe" {
                let id_len = if b.version == 2 { 2 } else { 4 };
                if b.content < id_len + 6 {
                    return Err(Fail);
                }
                let id = read_uint(src, id_len as usize)?;
                read(src, 2)?;
                let ty = read(src, 4)?;
                if ty == b"tmap" && id <= MAX_VALUE {
                    f.tone_mapped = id;
                }
                skip(src, b.content - id_len - 6)?;
            } else {
                skip(src, b.content)?;
            }
            remaining -= b.size;
        }
        skip(src, remaining)?;
        f.iinf_parsed = true;
        Ok(primary_features(f))
    }

    fn parse_meta(src: &mut Src, f: &mut Features, mut remaining: u64) -> R<bool> {
        while remaining > 0 {
            let b = parse_box(src, f, remaining)?;
            match &b.ty {
                b"pitm" => {
                    let n = if b.version == 0 { 2 } else { 4 };
                    if n > b.content {
                        return Err(Fail);
                    }
                    let id = read_uint(src, n as usize)?;
                    if id > MAX_VALUE {
                        return Err(Fail);
                    }
                    f.has_primary = true;
                    f.primary = id;
                    skip(src, b.content - n)?;
                }
                b"iprp" => {
                    if parse_iprp(src, f, b.content)? {
                        return Ok(true);
                    }
                }
                b"iref" => {
                    if parse_iref(src, f, b.content, b.version)? {
                        return Ok(true);
                    }
                }
                b"iinf" => {
                    if parse_iinf(src, f, b.content, b.version)? {
                        return Ok(true);
                    }
                }
                _ => skip(src, b.content)?,
            }
            remaining -= b.size;
        }
        Err(Fail)
    }

    /// The `ftyp` box; with `brand`, one of its brands must be AVIF's.
    fn parse_ftyp(src: &mut Src, f: &mut Features, brand: bool) -> R<bool> {
        let b = parse_box(src, f, u64::MAX)?;
        if &b.ty != b"ftyp" || b.content < 8 {
            return Err(Fail);
        }
        if !brand {
            skip(src, b.content)?;
            return Ok(true);
        }
        let mut i = 0u64;
        while i + 4 <= b.content {
            let d = read(src, 4)?;
            if i != 4 && (d == b"avif" || d == b"avis") {
                skip(src, b.content - (i + 4))?;
                return Ok(true);
            }
            if i > 32 * 4 {
                return Err(Fail);
            }
            i += 4;
        }
        Ok(false)
    }

    /// `AvifInfoIdentifyStream()`: an `ftyp` naming AVIF.
    pub(super) fn identify(src: &mut Src) -> bool {
        src.rewind();
        let mut f = Features::default();
        matches!(parse_ftyp(src, &mut f, true), Ok(true))
    }

    /// `AvifInfoGetFeaturesStream()` from the start of the input.
    pub(super) fn features(src: &mut Src, brand: bool) -> Option<Gfx> {
        src.rewind();
        let mut f = Features::default();
        if !matches!(parse_ftyp(src, &mut f, brand), Ok(true)) {
            return None;
        }
        loop {
            let b = parse_box(src, &mut f, u64::MAX).ok()?;
            if &b.ty == b"meta" {
                return match parse_meta(src, &mut f, b.content) {
                    Ok(true) => Some(Gfx::new(f.width, f.height, f.bit_depth, f.num_channels)),
                    _ => None,
                };
            }
            skip(src, b.content).ok()?;
        }
    }
}

// ---- SVG -----------------------------------------------------------------------

/// SVG, as php 8.5 sees it through libxml's reader: the first element must
/// be `svg` with integer `width` and `height` attributes (a unit of ASCII
/// letters may follow), and what the parser has consumed by then — whole
/// 512-byte chunks — must be well-formed. Input that merely ends early is
/// fine: the reader never gets to complain about it.
mod svg {
    use super::{Gfx, HashMap};

    const CHUNK: usize = 512;

    enum Stop {
        /// Ran out of input: tolerated.
        End,
        /// Not well-formed.
        Bad,
    }

    type R<T> = Result<T, Stop>;

    /// A start tag's attributes: (name, value) pairs.
    type Attrs = Vec<(Vec<u8>, Vec<u8>)>;

    struct P<'a> {
        d: &'a [u8],
        i: usize,
        end: usize,
        latin1: bool,
        ents: HashMap<Vec<u8>, Vec<u8>>,
    }

    fn is_name_start(c: u8) -> bool {
        c.is_ascii_alphabetic() || c == b'_' || c == b':' || c >= 0x80
    }

    fn is_name(c: u8) -> bool {
        is_name_start(c) || c.is_ascii_digit() || c == b'-' || c == b'.'
    }

    fn is_ws(c: u8) -> bool {
        matches!(c, b' ' | b'\t' | b'\n' | b'\r')
    }

    fn xml_char(c: u32) -> bool {
        matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
    }

    impl P<'_> {
        fn peek(&self) -> R<u8> {
            if self.i < self.end { Ok(self.d[self.i]) } else { Err(Stop::End) }
        }

        fn starts(&self, s: &[u8]) -> bool {
            self.d[self.i..self.end].starts_with(s)
        }

        /// Whether the rest could still begin with `s` (it is a prefix of it).
        fn maybe(&self, s: &[u8]) -> bool {
            let rest = &self.d[self.i..self.end];
            rest.len() < s.len() && s.starts_with(rest)
        }

        fn ws(&mut self) -> usize {
            let s = self.i;
            while self.i < self.end && is_ws(self.d[self.i]) {
                self.i += 1;
            }
            self.i - s
        }

        fn name(&mut self) -> R<Vec<u8>> {
            let c = self.peek()?;
            if !is_name_start(c) {
                return Err(Stop::Bad);
            }
            let s = self.i;
            while self.i < self.end && is_name(self.d[self.i]) {
                self.i += 1;
            }
            if self.i >= self.end {
                return Err(Stop::End);
            }
            Ok(self.d[s..self.i].to_vec())
        }

        /// One character of text, checked.
        fn char(&mut self) -> R<()> {
            let c = self.peek()?;
            if c < 0x80 || self.latin1 {
                if !xml_char(u32::from(c)) {
                    return Err(Stop::Bad);
                }
                self.i += 1;
                return Ok(());
            }
            let n = match c {
                0xC2..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF4 => 4,
                _ => return Err(Stop::Bad),
            };
            if self.i + n > self.end {
                return Err(Stop::End);
            }
            match std::str::from_utf8(&self.d[self.i..self.i + n]) {
                Ok(s) if s.chars().all(|ch| xml_char(ch as u32)) => {
                    self.i += n;
                    Ok(())
                }
                _ => Err(Stop::Bad),
            }
        }

        /// `&…;` after the `&`: its replacement text.
        fn reference(&mut self) -> R<Vec<u8>> {
            if self.peek()? == b'#' {
                self.i += 1;
                let hex = self.peek()? == b'x';
                if hex {
                    self.i += 1;
                }
                let s = self.i;
                while self.peek()? != b';' {
                    let c = self.d[self.i];
                    if !(if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }) {
                        return Err(Stop::Bad);
                    }
                    self.i += 1;
                }
                let digits = std::str::from_utf8(&self.d[s..self.i]).unwrap_or("");
                self.i += 1;
                let v = u32::from_str_radix(digits, if hex { 16 } else { 10 }).map_err(|_| Stop::Bad)?;
                let ch = char::from_u32(v).filter(|_| xml_char(v)).ok_or(Stop::Bad)?;
                return Ok(ch.to_string().into_bytes());
            }
            let n = self.name()?;
            if self.peek()? != b';' {
                return Err(Stop::Bad);
            }
            self.i += 1;
            Ok(match &n[..] {
                b"lt" => b"<".to_vec(),
                b"gt" => b">".to_vec(),
                b"amp" => b"&".to_vec(),
                b"quot" => b"\"".to_vec(),
                b"apos" => b"'".to_vec(),
                _ => self.ents.get(&n).cloned().ok_or(Stop::Bad)?,
            })
        }

        /// A quoted literal, raw.
        fn literal(&mut self) -> R<Vec<u8>> {
            let q = self.peek()?;
            if q != b'"' && q != b'\'' {
                return Err(Stop::Bad);
            }
            self.i += 1;
            let s = self.i;
            while self.peek()? != q {
                self.i += 1;
            }
            self.i += 1;
            Ok(self.d[s..self.i - 1].to_vec())
        }

        /// Attributes up to `>` / `/>`: (name, value) pairs and whether the
        /// tag closed itself.
        fn attributes(&mut self) -> R<(Attrs, bool)> {
            let mut attrs: Attrs = Vec::new();
            loop {
                let spaced = self.ws() > 0;
                match self.peek()? {
                    b'>' => {
                        self.i += 1;
                        return Ok((attrs, false));
                    }
                    b'/' => {
                        self.i += 1;
                        if self.peek()? != b'>' {
                            return Err(Stop::Bad);
                        }
                        self.i += 1;
                        return Ok((attrs, true));
                    }
                    _ if !spaced => return Err(Stop::Bad),
                    _ => {}
                }
                let n = self.name()?;
                self.ws();
                if self.peek()? != b'=' {
                    return Err(Stop::Bad);
                }
                self.i += 1;
                self.ws();
                let q = self.peek()?;
                if q != b'"' && q != b'\'' {
                    return Err(Stop::Bad);
                }
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    let c = self.peek()?;
                    if c == q {
                        self.i += 1;
                        break;
                    }
                    match c {
                        b'<' => return Err(Stop::Bad),
                        b'&' => {
                            self.i += 1;
                            v.extend(self.reference()?);
                        }
                        b'\t' | b'\n' | b'\r' => {
                            v.push(b' ');
                            self.i += 1;
                        }
                        _ => {
                            let s = self.i;
                            self.char()?;
                            v.extend_from_slice(&self.d[s..self.i]);
                        }
                    }
                }
                if attrs.iter().any(|(a, _)| *a == n) {
                    return Err(Stop::Bad);
                }
                attrs.push((n, v));
            }
        }

        /// `<!--` … `-->`, positioned after `<!--`.
        fn comment(&mut self) -> R<()> {
            loop {
                if self.starts(b"--") {
                    if self.starts(b"-->") {
                        self.i += 3;
                        return Ok(());
                    }
                    if self.maybe(b"-->") {
                        return Err(Stop::End);
                    }
                    return Err(Stop::Bad);
                }
                self.char()?;
            }
        }

        /// A processing instruction, positioned after `<?`.
        fn pi(&mut self) -> R<()> {
            let t = self.name()?;
            if t.eq_ignore_ascii_case(b"xml") {
                return Err(Stop::Bad);
            }
            if !self.starts(b"?>") && self.ws() == 0 {
                self.peek()?;
                return Err(Stop::Bad);
            }
            while !self.starts(b"?>") {
                self.char()?;
            }
            self.i += 2;
            Ok(())
        }

        /// The XML declaration, positioned at `<?xml`.
        fn xml_decl(&mut self) -> R<()> {
            self.i += 5;
            let mut seen_version = false;
            loop {
                let spaced = self.ws() > 0;
                if self.starts(b"?>") {
                    self.i += 2;
                    return if seen_version { Ok(()) } else { Err(Stop::Bad) };
                }
                self.peek()?;
                if !spaced {
                    return Err(Stop::Bad);
                }
                let n = self.name()?;
                self.ws();
                if self.peek()? != b'=' {
                    return Err(Stop::Bad);
                }
                self.i += 1;
                self.ws();
                let v = self.literal()?;
                match &n[..] {
                    b"version" if !seen_version => seen_version = true,
                    b"encoding" if seen_version => {
                        let e = String::from_utf8_lossy(&v).to_ascii_lowercase();
                        self.latin1 = matches!(
                            e.as_str(),
                            "iso-8859-1" | "latin1" | "latin-1" | "iso_8859-1" | "windows-1252" | "cp1252"
                        );
                    }
                    b"standalone" if seen_version => {}
                    _ => return Err(Stop::Bad),
                }
            }
        }

        /// `<!DOCTYPE …>`, positioned after `<!DOCTYPE`; internal general
        /// entities are remembered.
        fn doctype(&mut self) -> R<()> {
            if self.ws() == 0 {
                return Err(Stop::Bad);
            }
            self.name()?;
            self.ws();
            if self.starts(b"SYSTEM") {
                self.i += 6;
                self.ws();
                self.literal()?;
            } else if self.starts(b"PUBLIC") {
                self.i += 6;
                self.ws();
                self.literal()?;
                self.ws();
                self.literal()?;
            }
            self.ws();
            if self.peek()? == b'[' {
                self.i += 1;
                loop {
                    self.ws();
                    match self.peek()? {
                        b']' => {
                            self.i += 1;
                            break;
                        }
                        b'%' => {
                            self.i += 1;
                            self.name()?;
                            if self.peek()? != b';' {
                                return Err(Stop::Bad);
                            }
                            self.i += 1;
                        }
                        b'<' if self.starts(b"<!--") => {
                            self.i += 4;
                            self.comment()?;
                        }
                        b'<' if self.starts(b"<?") => {
                            self.i += 2;
                            self.pi()?;
                        }
                        b'<' if self.starts(b"<!ENTITY") => {
                            self.i += 8;
                            if self.ws() == 0 {
                                return Err(Stop::Bad);
                            }
                            let param = self.peek()? == b'%';
                            if param {
                                self.i += 1;
                                self.ws();
                            }
                            let n = self.name()?;
                            self.ws();
                            let q = self.peek()?;
                            if q == b'"' || q == b'\'' {
                                let v = self.literal()?;
                                if !param {
                                    self.ents.entry(n).or_insert(v);
                                }
                            }
                            self.decl_rest()?;
                        }
                        b'<' if self.starts(b"<!") => {
                            self.i += 2;
                            self.decl_rest()?;
                        }
                        _ => return Err(Stop::Bad),
                    }
                }
                self.ws();
            }
            if self.peek()? != b'>' {
                return Err(Stop::Bad);
            }
            self.i += 1;
            Ok(())
        }

        /// The rest of a markup declaration, through its `>`.
        fn decl_rest(&mut self) -> R<()> {
            loop {
                match self.peek()? {
                    b'>' => {
                        self.i += 1;
                        return Ok(());
                    }
                    b'"' | b'\'' => {
                        self.literal()?;
                    }
                    _ => self.i += 1,
                }
            }
        }

        /// Comments, PIs and white space (the `Misc` production).
        fn misc(&mut self) -> R<bool> {
            self.ws();
            if self.starts(b"<!--") {
                self.i += 4;
                self.comment()?;
                return Ok(true);
            }
            if self.starts(b"<?") {
                self.i += 2;
                self.pi()?;
                return Ok(true);
            }
            Ok(false)
        }

        /// Element content after the root's start tag, to the end of the
        /// consumed input.
        fn content(&mut self, mut stack: Vec<Vec<u8>>) -> R<()> {
            loop {
                if stack.is_empty() {
                    while self.misc()? {}
                    self.peek()?;
                    return Err(Stop::Bad);
                }
                let c = self.peek()?;
                if c == b'<' {
                    if self.starts(b"</") {
                        self.i += 2;
                        let n = self.name()?;
                        self.ws();
                        if self.peek()? != b'>' {
                            return Err(Stop::Bad);
                        }
                        self.i += 1;
                        if stack.last().is_none_or(|top| *top != n) {
                            return Err(Stop::Bad);
                        }
                        stack.pop();
                    } else if self.starts(b"<!--") {
                        self.i += 4;
                        self.comment()?;
                    } else if self.starts(b"<![CDATA[") {
                        self.i += 9;
                        while !self.starts(b"]]>") {
                            self.char()?;
                        }
                        self.i += 3;
                    } else if self.starts(b"<?") {
                        self.i += 2;
                        self.pi()?;
                    } else if self.maybe(b"<![CDATA[") || self.maybe(b"<!--") {
                        return Err(Stop::End);
                    } else if self.starts(b"<!") {
                        return Err(Stop::Bad);
                    } else {
                        self.i += 1;
                        let n = self.name()?;
                        // An unbound prefix is a namespace error, which
                        // libxml reports without stopping.
                        let (_, empty) = self.attributes()?;
                        if !empty {
                            stack.push(n);
                        }
                    }
                } else if c == b'&' {
                    self.i += 1;
                    self.reference()?;
                } else {
                    if self.starts(b"]]>") {
                        return Err(Stop::Bad);
                    }
                    self.char()?;
                }
            }
        }
    }

    /// An SVG length: digits, then an optional unit of ASCII letters.
    fn dimension(v: &[u8]) -> Option<(u32, String)> {
        let digits = v.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 || !v[digits..].iter().all(u8::is_ascii_alphabetic) {
            return None;
        }
        let mut n: i64 = 0;
        for &c in &v[..digits] {
            n = n.saturating_mul(10).saturating_add(i64::from(c - b'0'));
        }
        let unit = if digits == v.len() { "px".to_owned() } else { String::from_utf8_lossy(&v[digits..]).into_owned() };
        Some((n as u32, unit))
    }

    pub(super) fn size(d: &[u8]) -> Option<Gfx> {
        if d.first() != Some(&b'<') {
            return None;
        }
        let mut p = P { d, i: 0, end: d.len(), latin1: false, ents: HashMap::new() };
        let parsed = (|| -> R<Option<Gfx>> {
            if p.starts(b"<?xml") && p.d.get(5).is_some_and(|&c| is_ws(c) || c == b'?') {
                p.xml_decl()?;
            }
            let mut doctype = false;
            loop {
                if p.misc()? {
                    continue;
                }
                if !doctype && p.starts(b"<!DOCTYPE") {
                    p.i += 9;
                    p.doctype()?;
                    doctype = true;
                    continue;
                }
                break;
            }
            if p.peek()? != b'<' {
                return Err(Stop::Bad);
            }
            p.i += 1;
            let name = p.name()?;
            let (attrs, empty) = p.attributes()?;
            // The local name: after a bound prefix; with an unbound one
            // libxml keeps the whole qualified name.
            let local = match name.iter().position(|&b| b == b':') {
                Some(c) if attrs.iter().any(|(a, _)| a.strip_prefix(b"xmlns:") == Some(&name[..c])) => &name[c + 1..],
                _ => &name[..],
            };
            if !local.eq_ignore_ascii_case(b"svg") {
                return Ok(None);
            }
            let attr = |k: &[u8]| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            let g = match (attr(b"width"), attr(b"height")) {
                (Some(w), Some(h)) => match (dimension(&w), dimension(&h)) {
                    (Some((w, wu)), Some((h, hu))) => {
                        let mut g = Gfx::new(w, h, 0, 0);
                        g.units = Some((wu, hu));
                        Some(g)
                    }
                    _ => None,
                },
                _ => None,
            };
            // What the reader had parsed when the root arrived.
            p.end = d.len().min(p.i.div_ceil(CHUNK) * CHUNK);
            let stack = if empty { Vec::new() } else { vec![name.clone()] };
            match p.content(stack) {
                Ok(()) | Err(Stop::End) => Ok(g),
                Err(Stop::Bad) => Err(Stop::Bad),
            }
        })();
        parsed.ok().flatten()
    }
}

// ---- IPTC ----------------------------------------------------------------------

/// `iptcparse(string $iptc_block): array|false`
fn iptcparse(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let b = args[0].to_php_bytes();
    let n = b.len();
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while i < n {
        if at(i) == 0x1c && (at(i + 1) == 0x01 || at(i + 1) == 0x02) {
            break;
        }
        i += 1;
    }
    let mut out = Array::new();
    let mut found = 0;
    while i < n {
        if at(i) != 0x1c {
            break;
        }
        i += 1;
        if i + 4 >= n {
            break;
        }
        let dataset = at(i);
        let recnum = at(i + 1);
        i += 2;
        let len = if at(i) & 0x80 != 0 {
            if i + 6 >= n {
                break;
            }
            let l = u32::from_be_bytes([at(i + 2), at(i + 3), at(i + 4), at(i + 5)]) as usize;
            i += 6;
            l
        } else {
            let l = usize::from(at(i)) << 8 | usize::from(at(i + 1));
            i += 2;
            l
        };
        if len > n || i + len > n {
            break;
        }
        let key = format!("{dataset}#{recnum:03}");
        let key = ArrayKey::str(key.as_bytes());
        let mut list = match out.get(&key) {
            Some(Value::Array(a)) => a.clone(),
            _ => Array::new(),
        };
        list.push(Value::Str(Str::from_vec(b[i..i + len].to_vec())));
        out.set(key, Value::Array(list));
        i += len;
        found += 1;
    }
    Ok(if found == 0 { Value::Bool(false) } else { Value::Array(out) })
}

/// The Photoshop APP13 header `iptcembed()` writes before the IPTC data.
const PS_HEADER: &[u8; 28] = b"\xFF\xED\0\0Photoshop 3.0\x008BIM\x04\x04\0\0\0\0";

/// `iptcembed()`'s copy loop: the JPEG read byte by byte, echoed when
/// spooling, copied into the result buffer below spool 2.
struct Embed {
    data: Vec<u8>,
    pos: usize,
    spool: i64,
    echo: Vec<u8>,
    buf: Option<Vec<u8>>,
}

impl Embed {
    fn put1(&mut self, c: u8) {
        if self.spool > 0 {
            self.echo.push(c);
        }
        if let Some(b) = self.buf.as_mut() {
            b.push(c);
        }
    }

    fn raw(&mut self) -> i32 {
        let c = self.data.get(self.pos).map_or(-1, |&c| i32::from(c));
        if c >= 0 {
            self.pos += 1;
        }
        c
    }

    fn get1(&mut self, copy: bool) -> i32 {
        let c = self.raw();
        if c >= 0 && copy {
            self.put1(c as u8);
        }
        c
    }

    fn read_remaining(&mut self) {
        while self.get1(true) >= 0 {}
    }

    fn skip_variable(&mut self, copy: bool) {
        let c1 = self.get1(copy);
        if c1 < 0 {
            return;
        }
        let c2 = self.get1(copy);
        if c2 < 0 {
            return;
        }
        let mut len = ((c1 as u32) << 8).wrapping_add(c2 as u32).wrapping_sub(2);
        while len > 0 {
            len -= 1;
            if self.get1(copy) < 0 {
                return;
            }
        }
    }

    fn next_marker(&mut self) -> i32 {
        let mut c = self.get1(true);
        if c < 0 {
            return M_EOI;
        }
        while c != 0xff {
            c = self.get1(true);
            if c < 0 {
                return M_EOI;
            }
        }
        loop {
            c = self.raw();
            if c < 0 {
                return M_EOI;
            }
            if c != 0xff {
                return c;
            }
            self.put1(0xff);
        }
    }
}

/// `iptcembed(string $iptc_data, string $filename, int $spool = 0): string|bool`
fn iptcembed(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut iptc = args[0].to_php_bytes().to_vec();
    let name = args[1].to_php_bytes().to_vec();
    if name.contains(&0) {
        return Err(Unwind::value_error(
            "iptcembed(): Argument #2 ($filename) must not contain any null bytes",
        ));
    }
    let spool = args.get(2).map_or(0, Value::to_int);
    let p = crate::filestat::arg_path(ctx, &args[1]);
    let data = match std::fs::File::open(&p) {
        Ok(mut f) => {
            let mut d = Vec::new();
            let _ = f.read_to_end(&mut d);
            d
        }
        Err(_) => {
            ctx.warn(&format!("iptcembed(): Unable to open {}", String::from_utf8_lossy(&name)))?;
            return Ok(Value::Bool(false));
        }
    };
    let mut e = Embed { data, pos: 0, spool, echo: Vec::new(), buf: (spool < 2).then(Vec::new) };
    let ok = e.get1(true) == 0xff && e.get1(true) == 0xd8;
    if !ok {
        if !e.echo.is_empty() {
            ctx.echo(&e.echo);
        }
        return Ok(Value::Bool(false));
    }
    let mut written = false;
    loop {
        let marker = e.next_marker();
        if marker == M_EOI {
            break;
        }
        if marker != M_APP13 {
            e.put1(marker as u8);
        }
        match marker {
            M_APP13 => {
                e.skip_variable(false);
                e.raw();
                e.read_remaining();
                break;
            }
            M_APP0 | M_APP1 => {
                if written {
                    continue;
                }
                written = true;
                e.skip_variable(true);
                if iptc.len() & 1 == 1 {
                    iptc.push(0);
                }
                let total = iptc.len() + 28;
                let mut hdr = *PS_HEADER;
                hdr[2] = (total >> 8) as u8;
                hdr[3] = total as u8;
                for c in hdr {
                    e.put1(c);
                }
                e.put1((iptc.len() >> 8) as u8);
                e.put1(iptc.len() as u8);
                for &c in &iptc {
                    e.put1(c);
                }
            }
            M_SOS => {
                e.read_remaining();
                break;
            }
            _ => e.skip_variable(true),
        }
    }
    if !e.echo.is_empty() {
        ctx.echo(&e.echo);
    }
    Ok(match e.buf {
        Some(b) => Value::Str(Str::from_vec(b)),
        None => Value::Bool(true),
    })
}
