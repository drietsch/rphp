//! The `zlib.deflate` and `zlib.inflate` stream filters (php-src
//! `ext/zlib/zlib_filter.c`), for `filters.rs` to attach.
//!
//! Both work through 32K input and output buffers, as php's do: each
//! bucket is fed to zlib in pieces of at most 32K, and output leaves in
//! pieces of at most 32K. `zlib.deflate` compresses with `Z_NO_FLUSH`,
//! answers `fflush()` with `Z_SYNC_FLUSH` and finishes the stream with
//! `Z_FINISH` when the filter is removed or its stream closed; its
//! defaults are php's — raw deflate (window -15), level -1 and **memLevel
//! 9**, which is why this uses the ported compressor. `zlib.inflate`
//! defaults to raw inflate too, stops at the end of the compressed stream
//! (anything after it is swallowed), and reports a broken stream as
//! php's notice `zlib: data error`.
//!
//! **For the integrator.** Pass `Value::Uninit` as the parameters when
//! the script gave none: an explicit `null` is, to php, a parameter of the
//! wrong type (`zlib.deflate` warns "Invalid filter parameter, ignored").
//! [`ZlibFilter::new`] answers `None` for a name
//! that is not one of the two, and `Err` when php would refuse to create
//! the filter — each line of the error is one warning php prints, the last
//! `Unable to create or locate filter "…"`. A filter that was created may
//! still have warnings about ignored parameters ([`ZlibFilter::take_warnings`],
//! printed by `stream_filter_append()`). After [`ZlibFilter::push`], a
//! non-empty [`ZlibFilter::take_notices`] is php's `PSFS_ERR_FATAL`: print
//! each as an `E_NOTICE` from the calling function and fail the write.

use rphp_value::{ArrayKey, Value};

use super::deflate::{Deflate, Z_BUF_ERROR, Z_FINISH, Z_NO_FLUSH, Z_OK, Z_STREAM_END, Z_SYNC_FLUSH};

/// php's `inbuf_len` / `outbuf_len`.
const BUF: usize = 0x8000;

enum Engine {
    Deflate(Box<Deflate>),
    /// `None` until the first bytes arrive when the window asks for
    /// zlib-or-gzip detection (window bits 32 and up), which looks at them.
    Inflate { z: Option<flate2::Decompress>, window: i64 },
}

/// One attached `zlib.*` filter.
pub(crate) struct ZlibFilter {
    engine: Engine,
    /// php's `data->finished`.
    finished: bool,
    warnings: Vec<String>,
    notices: Vec<String>,
}

fn param(params: &Value, key: &str) -> Option<Value> {
    match &*params.deref() {
        Value::Array(a) => a.get_deref(&ArrayKey::str(key.as_bytes())),
        _ => None,
    }
}

impl ZlibFilter {
    /// Create the filter php's factory would for `name` with `params`.
    #[allow(dead_code)]
    pub(crate) fn new(name: &str, params: &Value) -> Option<Result<ZlibFilter, String>> {
        let lower = name.to_ascii_lowercase();
        let mut warnings = Vec::new();
        let engine = match lower.as_str() {
            "zlib.inflate" => {
                let mut window: i64 = -15;
                if let Some(v) = param(params, "window") {
                    let tmp = v.to_int();
                    if !(-15..=15 + 32).contains(&tmp) {
                        warnings.push(format!("Invalid parameter given for window size ({tmp})"));
                    } else {
                        window = tmp;
                    }
                }
                // inflateInit2()'s own check: -8..-15, 8..15, 24..31, 40..47
                // or 0 (the window from the header).
                let base = if window < 0 { -window } else if window >= 32 { window - 32 } else if window >= 16 { window - 16 } else { window };
                if !(window == 0 || (8..=15).contains(&base) || (window >= 32 && base == 0)) {
                    warnings.push("Unable to create or locate filter \"zlib.inflate\"".to_string());
                    return Some(Err(warnings.join("\n")));
                }
                let z = if window >= 32 { None } else { Some(super::new_inflater(window, &[])) };
                Engine::Inflate { z, window }
            }
            "zlib.deflate" => {
                let mut level: i64 = -1;
                let mut window: i64 = -15;
                let mut mem: i64 = 9;
                let mut level_param = None;
                match &*params.deref() {
                    Value::Uninit => {}
                    Value::Array(_) => {
                        if let Some(v) = param(params, "memory") {
                            let tmp = v.to_int();
                            if !(1..=9).contains(&tmp) {
                                warnings.push(format!("Invalid parameter given for memory level ({tmp})"));
                            } else {
                                mem = tmp;
                            }
                        }
                        if let Some(v) = param(params, "window") {
                            let tmp = v.to_int();
                            if !(-15..=15 + 16).contains(&tmp) {
                                warnings.push(format!("Invalid parameter given for window size ({tmp})"));
                            } else {
                                window = tmp;
                            }
                        }
                        level_param = param(params, "level").map(|v| v.to_int());
                    }
                    Value::Str(_) | Value::Float(_) | Value::Int(_) => level_param = Some(params.deref().to_int()),
                    _ => warnings.push("Invalid filter parameter, ignored".to_string()),
                }
                if let Some(tmp) = level_param {
                    if !(-1..=9).contains(&tmp) {
                        warnings.push(format!("Invalid compression level specified. ({tmp})"));
                    } else {
                        level = tmp;
                    }
                }
                match Deflate::new(level as i32, window as i32, mem as i32, 0) {
                    Some(z) => Engine::Deflate(Box::new(z)),
                    None => {
                        warnings.push("Unable to create or locate filter \"zlib.deflate\"".to_string());
                        return Some(Err(warnings.join("\n")));
                    }
                }
            }
            _ => return None,
        };
        Some(Ok(ZlibFilter { engine, finished: false, warnings, notices: Vec::new() }))
    }

    /// Warnings about parameters php ignored while creating the filter.
    #[allow(dead_code)]
    pub(crate) fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// The notices the last call raised; any means the write failed.
    #[allow(dead_code)]
    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// Filter a bucket: php's call without flush flags.
    #[allow(dead_code)]
    pub(crate) fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        self.run(input, out, Flags::None);
    }

    /// `fflush()` on the stream: `PSFS_FLAG_FLUSH_INC`.
    #[allow(dead_code)]
    pub(crate) fn flush(&mut self, out: &mut Vec<u8>) {
        self.run(&[], out, Flags::Inc);
    }

    /// The filter's end — removed, or its stream closed:
    /// `PSFS_FLAG_FLUSH_CLOSE`.
    #[allow(dead_code)]
    pub(crate) fn finish(&mut self, out: &mut Vec<u8>) {
        self.run(&[], out, Flags::Close);
    }

    fn run(&mut self, input: &[u8], out: &mut Vec<u8>, flags: Flags) {
        match &mut self.engine {
            Engine::Deflate(z) => deflate_filter(z, &mut self.finished, input, out, flags),
            Engine::Inflate { z, window } => {
                if z.is_none() {
                    if input.is_empty() {
                        return;
                    }
                    *z = Some(super::new_inflater(*window, input));
                }
                let z = z.as_mut().expect("created above");
                if let Err(msg) = inflate_filter(z, &mut self.finished, input, out, flags) {
                    self.notices.push(format!("zlib: {msg}"));
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Flags {
    None,
    Inc,
    Close,
}

fn deflate_filter(z: &mut Deflate, finished: &mut bool, input: &[u8], out: &mut Vec<u8>, flags: Flags) {
    let mut outbuf = vec![0u8; BUF];
    let mut bin = 0;
    while bin < input.len() && !*finished {
        let desired = (input.len() - bin).min(BUF);
        let mode = match flags {
            Flags::Close => super::deflate::Z_FULL_FLUSH,
            Flags::Inc => Z_SYNC_FLUSH,
            Flags::None => Z_NO_FLUSH,
        };
        *finished = mode != Z_NO_FLUSH;
        let (status, used, made) = z.deflate(&input[bin..bin + desired], &mut outbuf, mode);
        if status != Z_OK {
            return;
        }
        bin += used;
        out.extend_from_slice(&outbuf[..made]);
    }
    if flags == Flags::Close || (flags == Flags::Inc && !*finished) {
        loop {
            let mode = if flags == Flags::Close { Z_FINISH } else { Z_SYNC_FLUSH };
            let (status, _, made) = z.deflate(&[], &mut outbuf, mode);
            *finished = flags == Flags::Close;
            out.extend_from_slice(&outbuf[..made]);
            if status != Z_OK {
                break;
            }
        }
    }
}

fn inflate_filter(
    z: &mut flate2::Decompress,
    finished: &mut bool,
    input: &[u8],
    out: &mut Vec<u8>,
    flags: Flags,
) -> Result<(), &'static str> {
    let mut outbuf = vec![0u8; BUF];
    let mut bin = 0;
    let finish = flags == Flags::Close;
    while bin < input.len() && !*finished {
        let desired = (input.len() - bin).min(BUF);
        let (status, used, made) = super::inflate_step(z, &input[bin..bin + desired], &mut outbuf, finish);
        if status == Z_STREAM_END {
            *finished = true;
        } else if status != Z_OK && status != Z_BUF_ERROR {
            return Err(super::deflate::z_error(status));
        }
        bin += used;
        out.extend_from_slice(&outbuf[..made]);
        if used == 0 && made == 0 && status != Z_STREAM_END {
            // No progress is possible with what is here.
            break;
        }
    }
    if !*finished && finish {
        loop {
            let (status, _, made) = super::inflate_step(z, &[], &mut outbuf, true);
            out.extend_from_slice(&outbuf[..made]);
            if status != Z_OK {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// `stream_filter_append($fp, 'zlib.deflate', STREAM_FILTER_WRITE, 6)`,
    /// then `fwrite($fp, "hello hello hello")` and removal. Expected bytes
    /// from `php -n` (8.5.10, zlib 1.2.12).
    #[test]
    fn deflate_filter_matches_php_level_6() {
        let mut f = ZlibFilter::new("zlib.deflate", &Value::Int(6)).unwrap().unwrap();
        let mut out = Vec::new();
        f.push(b"hello hello hello", &mut out);
        f.finish(&mut out);
        assert_eq!(hex(&out), "cb48cdc9c957c8409000");
    }

    /// Writes, an `fflush()` between them, then removal: php gives
    /// `4a4c4a06000000ffff4b494d0300`.
    #[test]
    fn deflate_filter_flush_is_a_sync_flush() {
        let mut f = ZlibFilter::new("zlib.deflate", &Value::Int(6)).unwrap().unwrap();
        let mut out = Vec::new();
        f.push(b"abc", &mut out);
        f.flush(&mut out);
        f.push(b"def", &mut out);
        f.finish(&mut out);
        assert_eq!(hex(&out), "4a4c4a06000000ffff4b494d0300");
    }

    /// php's default is memLevel 9: for this input the filter's output is
    /// `deflate_init(ZLIB_ENCODING_RAW, ['memory' => 9])`'s, which the
    /// one-shot `gzdeflate()` (also memLevel 9) reproduces.
    #[test]
    fn deflate_filter_default_is_memlevel_9() {
        let mut data = Vec::new();
        let mut x: u32 = 12345;
        for _ in 0..200_000 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            data.push(32 + ((x >> 16) % 59) as u8);
        }
        let mut f = ZlibFilter::new("zlib.deflate", &Value::Uninit).unwrap().unwrap();
        let mut out = Vec::new();
        for chunk in data.chunks(7000) {
            f.push(chunk, &mut out);
        }
        f.finish(&mut out);
        let one_shot = crate::zlib::encode(&data, -15, -1).unwrap();
        assert!(out == one_shot);
    }

    #[test]
    fn inflate_filter_round_trips_and_stops_at_the_end() {
        let packed = crate::zlib::encode(b"hello world", -15, -1).unwrap();
        let mut f = ZlibFilter::new("zlib.inflate", &Value::Uninit).unwrap().unwrap();
        let mut out = Vec::new();
        f.push(&packed, &mut out);
        f.push(b"trailing", &mut out);
        f.finish(&mut out);
        assert_eq!(out, b"hello world");
        assert!(f.take_notices().is_empty());
    }

    #[test]
    fn inflate_filter_reports_garbage() {
        let mut f = ZlibFilter::new("zlib.inflate", &Value::Uninit).unwrap().unwrap();
        let mut out = Vec::new();
        f.push(b"garbage!", &mut out);
        assert_eq!(f.take_notices(), vec!["zlib: data error".to_string()]);
    }

    #[test]
    fn refusals_and_warnings() {
        assert!(ZlibFilter::new("string.rot13", &Value::Uninit).is_none());
        let mut a = rphp_value::Array::new();
        a.set(ArrayKey::str(b"window"), Value::Int(3));
        let err = ZlibFilter::new("zlib.deflate", &Value::Array(a)).unwrap().err().unwrap();
        assert_eq!(err, "Unable to create or locate filter \"zlib.deflate\"");
        let mut f = ZlibFilter::new("zlib.deflate", &Value::Bool(true)).unwrap().unwrap();
        assert_eq!(f.take_warnings(), vec!["Invalid filter parameter, ignored".to_string()]);
    }
}
