//! `UConverter`: ICU's converter API over ICU's own registry and tables,
//! dumped from php (`data.rs`: the 232 converters `getAvailable()` lists,
//! every alias with the converter ICU opens for it and whether ICU calls
//! it ambiguous, the substitution bytes, and the complete mapping tables
//! of the single-byte converters), with the Unicode forms (UTF-8, CESU-8,
//! UTF-16/32 in both byte orders and with a BOM), US-ASCII and ISO-8859-1
//! implemented algorithmically.
//!
//! A conversion runs the way `php_converter_do_convert` drives ICU: bytes
//! to UTF-16 (`toUnicode`), UTF-16 to bytes (`fromUnicode`), each pass run
//! twice (ICU's preflight, then the real one) and each started with a
//! `REASON_RESET` callback; a subclass's `toUCallback()` /
//! `fromUCallback()` sees every illegal, truncated or unassigned
//! sequence with ICU's arguments and error codes, and what it returns is
//! written in its place. The default is ICU's substitute callback: U+FFFD
//! going to Unicode, the converter's substitution bytes coming from it.
//!
//! Not implemented (catalogued): the multi-byte table converters
//! (Shift-JIS, EUC-*, GBK/GB18030, Big5, the EBCDIC DBCS ones), ISO-2022,
//! HZ, SCSU, BOCU-1, UTF-7, IMAP-mailbox-name, LMBCS, ISCII; opening one
//! is an `Error` naming it.

mod data;

use std::collections::HashMap;
use std::sync::OnceLock;

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, PhpRef, Payload, Value};

use crate::shape::{register_class, MethodImpl};
use crate::state::{self, IntlError};
use crate::{generated, str_arg};

/// A converter as ICU lists it.
pub struct Conv {
    name: &'static str,
    ty: i64,
    subst: &'static [u8],
    subst_lens: &'static [usize],
    aliases: &'static [&'static str],
    sbcs: Option<&'static Sbcs>,
}

/// A single-byte converter's tables.
pub struct Sbcs {
    decode: [u32; 256],
    /// Code points whose encoding is not the first byte decoding to them
    /// (`None`: no encoding).
    overrides: &'static [(u32, Option<u8>)],
}

/// An unassigned byte.
const UNA: u32 = 0xFFFF_FFFF;
/// An illegal byte.
const ILL: u32 = 0xFFFF_FFFE;

const REASON_UNASSIGNED: i64 = 0;
const REASON_ILLEGAL: i64 = 1;
const REASON_RESET: i64 = 3;

const U_INVALID_CHAR_FOUND: i64 = 10;
const U_TRUNCATED_CHAR_FOUND: i64 = 11;
const U_ILLEGAL_CHAR_FOUND: i64 = 12;
const U_FILE_ACCESS_ERROR: i64 = 4;
const U_ILLEGAL_ARGUMENT_ERROR: i64 = 1;
const U_AMBIGUOUS_ALIAS_WARNING: i64 = -122;

// ---- the registry -------------------------------------------------------------------------

/// `ucnv_io_stripASCIIForCompare`: letters lowercased, digits kept (a
/// leading zero before another digit dropped), everything else ignored.
fn strip(name: &str) -> String {
    let b = name.as_bytes();
    let mut out = String::new();
    let mut after_digit = false;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'0' => {
                if !after_digit && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    continue;
                }
                out.push('0');
            }
            b'1'..=b'9' => {
                after_digit = true;
                out.push(c as char);
            }
            c if c.is_ascii_alphabetic() => {
                after_digit = false;
                out.push(c.to_ascii_lowercase() as char);
            }
            _ => after_digit = false,
        }
    }
    out
}

fn alias_map() -> &'static HashMap<String, (usize, bool)> {
    static MAP: OnceLock<HashMap<String, (usize, bool)>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for (alias, idx, ambiguous) in data::ALIASES {
            m.entry(strip(alias)).or_insert((*idx, *ambiguous));
        }
        m
    })
}

/// `ucnv_open(name)`: the converter and whether the alias is ambiguous.
fn lookup(name: &str) -> Option<(usize, bool)> {
    alias_map().get(&strip(name)).copied()
}

/// The algorithm behind a converter, `None` when rphp has none.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Utf8,
    Cesu8,
    Utf16(Endian),
    Utf32(Endian),
    Ascii,
    Latin1,
    Sbcs(&'static Sbcs),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Endian {
    Big,
    Little,
    /// Read with BOM detection (the given order without one), written
    /// with a BOM in the given order (`UTF-16` itself writes the platform
    /// order of php's ICU, little-endian).
    Bom { read_le: bool, write_le: bool },
}

const BOM: Endian = Endian::Bom { read_le: false, write_le: true };

impl PartialEq for &'static Sbcs {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(*self, *other)
    }
}
impl Eq for &'static Sbcs {}

fn codec(c: &Conv) -> Option<Codec> {
    if let Some(t) = c.sbcs {
        // GSM 03.38 escapes into a second table: not single-byte after all.
        return (!c.name.starts_with("gsm-")).then_some(Codec::Sbcs(t));
    }
    Some(match c.name {
        "UTF-8" => Codec::Utf8,
        "CESU-8" => Codec::Cesu8,
        "UTF-16" => Codec::Utf16(BOM),
        "UTF-16,version=2" | "UTF-16BE,version=1" => Codec::Utf16(Endian::Bom { read_le: false, write_le: false }),
        "UTF-16LE,version=1" => Codec::Utf16(Endian::Bom { read_le: true, write_le: true }),
        "UTF-16BE" => Codec::Utf16(Endian::Big),
        "UTF-16LE" => Codec::Utf16(Endian::Little),
        "UTF-32" => Codec::Utf32(BOM),
        "UTF-32BE" => Codec::Utf32(Endian::Big),
        "UTF-32LE" => Codec::Utf32(Endian::Little),
        "US-ASCII" => Codec::Ascii,
        "ISO-8859-1" => Codec::Latin1,
        _ => return None,
    })
}

/// An open converter: which one, its substitution bytes.
#[derive(Clone)]
struct Cnv {
    idx: usize,
    subst: Vec<u8>,
}

impl Cnv {
    fn conv(&self) -> &'static Conv {
        &data::CONVERTERS[self.idx]
    }

    fn codec(&self) -> Codec {
        codec(self.conv()).unwrap_or(Codec::Utf8)
    }
}

// ---- conversion -----------------------------------------------------------------------------

/// A callback's view of one error: reason, the rest of the source, the
/// offending units, ICU's error code.
enum Fault<'a> {
    ToU { reason: i64, rest: &'a [u8], units: &'a [u8], err: i64 },
    FromU { reason: i64, units: Vec<u32>, cp: u32, err: i64 },
}

/// What handling a fault produced: the replacement and the error code
/// left (non-zero stops the conversion).
type Handler<'h> = dyn FnMut(Fault<'_>) -> Result<(Replacement, i64), Unwind> + 'h;

enum Replacement {
    Units(Vec<u16>),
    Bytes(Vec<u8>),
}

fn push_cp(out: &mut Vec<u16>, cp: u32) {
    if cp > 0xFFFF {
        let c = cp - 0x10000;
        out.push(0xD800 | (c >> 10) as u16);
        out.push(0xDC00 | (c & 0x3FF) as u16);
    } else {
        out.push(cp as u16);
    }
}

/// One UTF-8 sequence at `b[i..]` under ICU's rules: `Ok((cp, len))`, or
/// `Err((len, truncated))` for the maximal ill-formed subpart. `cesu`
/// admits surrogates (CESU-8's encoded pairs).
fn utf8_at(b: &[u8], i: usize, cesu: bool) -> Result<(u32, usize), (usize, bool)> {
    let c0 = b[i];
    if c0 < 0x80 {
        return Ok((c0 as u32, 1));
    }
    let (n, lo, hi) = match c0 {
        0xC2..=0xDF => (2, 0x80, 0xBF),
        0xE0 => (3, 0xA0, 0xBF),
        0xED if !cesu => (3, 0x80, 0x9F),
        0xE1..=0xEF => (3, 0x80, 0xBF),
        0xF0 if !cesu => (4, 0x90, 0xBF),
        0xF1..=0xF3 if !cesu => (4, 0x80, 0xBF),
        0xF4 if !cesu => (4, 0x80, 0x8F),
        _ => return Err((1, false)),
    };
    let mut cp = (c0 as u32) & (0x7F >> n);
    for k in 1..n {
        let Some(&c) = b.get(i + k) else {
            return Err((k, true));
        };
        let (l, h) = if k == 1 { (lo, hi) } else { (0x80, 0xBF) };
        if c < l || c > h {
            return Err((k, false));
        }
        cp = (cp << 6) | (c as u32 & 0x3F);
    }
    Ok((cp, n))
}

/// Bytes to UTF-16 units; `Ok(Err(code))` when a callback leaves an error.
fn to_unicode(c: &Cnv, b: &[u8], h: &mut Option<&mut Handler<'_>>) -> Result<Result<Vec<u16>, i64>, Unwind> {
    let mut out: Vec<u16> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    let codec = c.codec();
    // The byte order a BOM form settles on.
    let mut order = match codec {
        Codec::Utf16(Endian::Little) | Codec::Utf32(Endian::Little) => Endian::Little,
        Codec::Utf16(Endian::Bom { read_le: true, .. }) | Codec::Utf32(Endian::Bom { read_le: true, .. }) => Endian::Little,
        _ => Endian::Big,
    };
    if let Codec::Utf16(Endian::Bom { .. }) = codec {
        if b.starts_with(&[0xFE, 0xFF]) {
            i = 2;
            order = Endian::Big;
        } else if b.starts_with(&[0xFF, 0xFE]) {
            i = 2;
            order = Endian::Little;
        }
    }
    if let Codec::Utf32(Endian::Bom { .. }) = codec {
        if b.starts_with(&[0, 0, 0xFE, 0xFF]) {
            i = 4;
            order = Endian::Big;
        } else if b.starts_with(&[0xFF, 0xFE, 0, 0]) {
            i = 4;
            order = Endian::Little;
        }
    }
    while i < b.len() {
        // (reason, length, error) of a fault at i, or the code point.
        let step: Result<(u32, usize), (i64, usize, i64)> = match codec {
            Codec::Utf8 | Codec::Cesu8 => match utf8_at(b, i, codec == Codec::Cesu8) {
                Ok(v) => Ok(v),
                Err((n, true)) => Err((REASON_ILLEGAL, n, U_TRUNCATED_CHAR_FOUND)),
                Err((n, false)) => Err((REASON_ILLEGAL, n, U_ILLEGAL_CHAR_FOUND)),
            },
            Codec::Ascii => {
                if b[i] < 0x80 {
                    Ok((b[i] as u32, 1))
                } else {
                    Err((REASON_ILLEGAL, 1, U_ILLEGAL_CHAR_FOUND))
                }
            }
            Codec::Latin1 => Ok((b[i] as u32, 1)),
            Codec::Sbcs(t) => match t.decode[b[i] as usize] {
                UNA => Err((REASON_UNASSIGNED, 1, U_INVALID_CHAR_FOUND)),
                ILL => Err((REASON_ILLEGAL, 1, U_ILLEGAL_CHAR_FOUND)),
                cp => Ok((cp, 1)),
            },
            Codec::Utf16(_) => {
                let unit = |k: usize| -> Option<u16> {
                    let p = b.get(k..k + 2)?;
                    Some(if order == Endian::Little { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) })
                };
                match unit(i) {
                    None => Err((REASON_ILLEGAL, b.len() - i, U_TRUNCATED_CHAR_FOUND)),
                    Some(u) if (0xD800..0xDC00).contains(&u) => match unit(i + 2) {
                        Some(l) if (0xDC00..0xE000).contains(&l) => {
                            Ok((0x10000 + (((u as u32) - 0xD800) << 10) + (l as u32 - 0xDC00), 4))
                        }
                        None if b.len() > i + 2 || b.len() == i + 2 => Err((REASON_ILLEGAL, b.len() - i, U_TRUNCATED_CHAR_FOUND)),
                        _ => Err((REASON_ILLEGAL, 2, U_ILLEGAL_CHAR_FOUND)),
                    },
                    Some(u) if (0xDC00..0xE000).contains(&u) => Err((REASON_ILLEGAL, 2, U_ILLEGAL_CHAR_FOUND)),
                    Some(u) => Ok((u as u32, 2)),
                }
            }
            Codec::Utf32(_) => match b.get(i..i + 4) {
                None => Err((REASON_ILLEGAL, b.len() - i, U_TRUNCATED_CHAR_FOUND)),
                Some(p) => {
                    let v = if order == Endian::Little {
                        u32::from_le_bytes([p[0], p[1], p[2], p[3]])
                    } else {
                        u32::from_be_bytes([p[0], p[1], p[2], p[3]])
                    };
                    if v > 0x10FFFF || (0xD800..0xE000).contains(&v) {
                        Err((REASON_ILLEGAL, 4, U_ILLEGAL_CHAR_FOUND))
                    } else {
                        Ok((v, 4))
                    }
                }
            },
        };
        match step {
            Ok((cp, n)) => {
                // CESU-8 hands surrogates through as units.
                if cp <= 0xFFFF {
                    out.push(cp as u16);
                } else {
                    push_cp(&mut out, cp);
                }
                i += n;
            }
            Err((reason, n, err)) => {
                let units = &b[i..i + n];
                i += n;
                match h {
                    None => out.push(0xFFFD),
                    Some(h) => {
                        let (rep, left) = h(Fault::ToU { reason, rest: &b[i..], units, err })?;
                        if let Replacement::Units(u) = rep {
                            out.extend(u);
                        }
                        if left > 0 {
                            return Ok(Err(left));
                        }
                    }
                }
            }
        }
    }
    Ok(Ok(out))
}

/// Whether a form writes big-endian.
fn big(e: Endian) -> bool {
    matches!(e, Endian::Big | Endian::Bom { write_le: false, .. })
}

/// ICU's `IS_DEFAULT_IGNORABLE_CODE_POINT` (ucnv_cb.c).
fn ignorable(c: u32) -> bool {
    matches!(c,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F | 0x200B..=0x200F
        | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFF8
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// UTF-16 units to bytes.
fn from_unicode(c: &Cnv, u: &[u16], h: &mut Option<&mut Handler<'_>>) -> Result<Result<Vec<u8>, i64>, Unwind> {
    let codec = c.codec();
    let mut out: Vec<u8> = Vec::with_capacity(u.len());
    if !u.is_empty() {
        match codec {
            Codec::Utf16(Endian::Bom { write_le: true, .. }) => out.extend_from_slice(&[0xFF, 0xFE]),
            Codec::Utf16(Endian::Bom { write_le: false, .. }) => out.extend_from_slice(&[0xFE, 0xFF]),
            Codec::Utf32(Endian::Bom { write_le: true, .. }) => out.extend_from_slice(&[0xFF, 0xFE, 0, 0]),
            Codec::Utf32(Endian::Bom { write_le: false, .. }) => out.extend_from_slice(&[0, 0, 0xFE, 0xFF]),
            _ => {}
        }
    }
    let mut i = 0;
    while i < u.len() {
        let (cp, n, lone) = match u[i] {
            hi @ 0xD800..=0xDBFF => match u.get(i + 1) {
                Some(&lo) if (0xDC00..0xE000).contains(&lo) => (0x10000 + (((hi as u32) - 0xD800) << 10) + (lo as u32 - 0xDC00), 2, false),
                _ => (hi as u32, 1, true),
            },
            lo @ 0xDC00..=0xDFFF => (lo as u32, 1, true),
            x => (x as u32, 1, false),
        };
        i += n;
        let encoded: Option<Vec<u8>> = if lone && codec != Codec::Cesu8 {
            None
        } else {
            match codec {
                Codec::Utf8 => char::from_u32(cp).map(|ch| ch.to_string().into_bytes()),
                Codec::Cesu8 => {
                    let mut v = Vec::new();
                    let mut units = Vec::new();
                    push_cp(&mut units, cp);
                    for unit in units {
                        let x = unit as u32;
                        if x < 0x80 {
                            v.push(x as u8);
                        } else if x < 0x800 {
                            v.extend([0xC0 | (x >> 6) as u8, 0x80 | (x & 0x3F) as u8]);
                        } else {
                            v.extend([0xE0 | (x >> 12) as u8, 0x80 | ((x >> 6) & 0x3F) as u8, 0x80 | (x & 0x3F) as u8]);
                        }
                    }
                    Some(v)
                }
                Codec::Utf16(e) => {
                    let mut units = Vec::new();
                    push_cp(&mut units, cp);
                    Some(
                        units
                            .iter()
                            .flat_map(|x| if big(e) { x.to_be_bytes() } else { x.to_le_bytes() })
                            .collect(),
                    )
                }
                Codec::Utf32(e) => Some(if big(e) { cp.to_be_bytes().to_vec() } else { cp.to_le_bytes().to_vec() }),
                Codec::Ascii => (cp < 0x80).then(|| vec![cp as u8]),
                Codec::Latin1 => (cp < 0x100).then(|| vec![cp as u8]),
                Codec::Sbcs(t) => match t.overrides.binary_search_by_key(&cp, |o| o.0) {
                    Ok(k) => t.overrides[k].1.map(|b| vec![b]),
                    Err(_) => t.decode.iter().position(|&d| d == cp).map(|b| vec![b as u8]),
                },
            }
        };
        match encoded {
            Some(bytes) => out.extend(bytes),
            None => {
                let (reason, err) = if lone { (REASON_ILLEGAL, U_ILLEGAL_CHAR_FOUND) } else { (REASON_UNASSIGNED, U_INVALID_CHAR_FOUND) };
                match h {
                    // ICU's substitute callback skips an unmappable
                    // default-ignorable code point.
                    None if !lone && ignorable(cp) => {}
                    None => out.extend_from_slice(&c.subst),
                    Some(h) => {
                        let (rep, left) = h(Fault::FromU { reason, units: vec![cp], cp, err })?;
                        if let Replacement::Bytes(b) = rep {
                            out.extend(b);
                        }
                        if left > 0 {
                            return Ok(Err(left));
                        }
                    }
                }
            }
        }
    }
    Ok(Ok(out))
}

// ---- the object -----------------------------------------------------------------------------

/// The converter pair and the object's last error.
#[derive(Clone)]
pub struct ConvState {
    src: Option<Cnv>,
    dest: Option<Cnv>,
    err: IntlError,
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn with<R>(o: &Object, f: impl FnOnce(&mut ConvState) -> R) -> R {
    if o.with_payload::<ConvState, _>(|_| ()).is_none() {
        o.set_payload(Payload::Native(Box::new(ConvState { src: None, dest: None, err: IntlError::default() })));
    }
    o.with_payload::<ConvState, _>(f).expect("payload installed")
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = src.with_payload::<ConvState, _>(|s| s.clone()) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

/// `THROW_UFAILURE`'s message.
fn failure(code: i64) -> String {
    format!("returned error {code}: {}", state::error_name(code))
}

/// `intl_errors_set(obj, code, msg)`.
fn obj_error(ctx: &mut Ctx, o: Option<&Object>, code: i64, msg: &str) -> Result<(), Unwind> {
    let who = ctx.active_function_name();
    let mut err = o.map(|o| with(o, |s| s.err.clone())).unwrap_or_default();
    state::set_both(ctx, &mut err, &who, code, msg)?;
    if let Some(o) = o {
        with(o, |s| s.err = err);
    }
    Ok(())
}

/// `php_converter_set_encoding`: `Ok(None)` after the error is recorded.
/// `ctor` makes every error an `IntlException`, as the constructor does.
fn open(ctx: &mut Ctx, o: Option<&Object>, name: &str, ctor: bool) -> Result<Option<Cnv>, Unwind> {
    let who = ctx.active_function_name();
    let Some((idx, ambiguous)) = lookup(name) else {
        if ctor {
            let msg = format!("{who}(): {}", failure(U_FILE_ACCESS_ERROR));
            state::set_global_code(ctx, U_FILE_ACCESS_ERROR);
            return Err(Unwind::exception("IntlException", msg));
        }
        match o {
            Some(_) => obj_error(ctx, o, U_FILE_ACCESS_ERROR, &failure(U_FILE_ACCESS_ERROR))?,
            None => state::set_global(
                ctx,
                &who,
                U_FILE_ACCESS_ERROR,
                &format!("Error setting encoding: {U_FILE_ACCESS_ERROR} - {}", state::error_name(U_FILE_ACCESS_ERROR)),
            )?,
        }
        return Ok(None);
    };
    let conv = &data::CONVERTERS[idx];
    if ambiguous {
        let msg = format!("Ambiguous encoding specified, using {}", conv.name);
        if ctor {
            state::set_global_code(ctx, U_AMBIGUOUS_ALIAS_WARNING);
            return Err(Unwind::exception("IntlException", format!("{who}(): {msg}")));
        }
        state::set_global(ctx, &who, U_AMBIGUOUS_ALIAS_WARNING, &msg)?;
    }
    if codec(conv).is_none() {
        return Err(Unwind::error(format!("{who}(): the {} converter is not implemented by rphp's intl yet", conv.name)));
    }
    Ok(Some(Cnv { idx, subst: conv.subst.to_vec() }))
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    state::reset_global(ctx);
    let arg = |i: usize| crate::opt_arg(args, i).map_or_else(|| "utf-8".to_string(), |v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    let (dest, src) = (arg(0), arg(1));
    with(o, |_| ());
    let s = open(ctx, Some(o), &src, true)?;
    with(o, |st| st.src = s);
    let d = open(ctx, Some(o), &dest, true)?;
    with(o, |st| st.dest = d);
    Ok(Value::Null)
}

/// The callbacks of a subclass, or `None` for `UConverter` itself.
fn user_handler<'a, 'c>(ctx: &'a mut Ctx<'c>, o: &'a Object) -> Option<impl FnMut(Fault<'_>) -> Result<(Replacement, i64), Unwind> + use<'a, 'c>> {
    let cid = ctx.class_by_name(b"UConverter")?;
    if o.class_id() == cid {
        return None;
    }
    Some(move |f: Fault<'_>| -> Result<(Replacement, i64), Unwind> {
        let err = PhpRef::new(Value::Int(match &f {
            Fault::ToU { err, .. } | Fault::FromU { err, .. } => *err,
        }));
        let (method, args, to_u): (&[u8], Vec<Value>, bool) = match &f {
            Fault::ToU { reason, rest, units, .. } => (
                b"toUCallback",
                vec![Value::Int(*reason), Value::string(rest), Value::string(units), Value::Ref(err.clone())],
                true,
            ),
            Fault::FromU { reason, units, cp, .. } => {
                let mut a = Array::new();
                for u in units {
                    a.push(Value::Int(*u as i64));
                }
                (b"fromUCallback", vec![Value::Int(*reason), Value::Array(a), Value::Int(*cp as i64), Value::Ref(err.clone())], false)
            }
        };
        let ret = ctx.call_method(o, method, &args)?;
        let rep = if to_u {
            let mut units = Vec::new();
            append_to_unicode(ctx, o, &ret, &mut units)?;
            Replacement::Units(units)
        } else {
            let mut bytes = Vec::new();
            append_from_unicode(ctx, o, &ret, &mut bytes)?;
            Replacement::Bytes(bytes)
        };
        let left = match err.get() {
            Value::Int(i) => i,
            _ => match &f {
                Fault::ToU { err, .. } | Fault::FromU { err, .. } => *err,
            },
        };
        Ok((rep, left))
    })
}

/// `php_converter_append_toUnicode_target`.
fn append_to_unicode(ctx: &mut Ctx, o: &Object, v: &Value, out: &mut Vec<u16>) -> Result<(), Unwind> {
    match &*v.deref() {
        Value::Null => {}
        Value::Int(cp) => {
            if !(0..=0x10FFFF).contains(cp) {
                obj_error(ctx, Some(o), U_ILLEGAL_ARGUMENT_ERROR, &format!("Invalid codepoint U+{cp:04x}"))?;
                return Ok(());
            }
            push_cp(out, *cp as u32);
        }
        Value::Str(s) => {
            for c in String::from_utf8_lossy(s.as_bytes()).chars() {
                push_cp(out, c as u32);
            }
        }
        Value::Array(a) => {
            for x in a.values() {
                append_to_unicode(ctx, o, x, out)?;
            }
        }
        _ => obj_error(ctx, Some(o), U_ILLEGAL_ARGUMENT_ERROR, "toUCallback() specified illegal type for substitution character")?,
    }
    Ok(())
}

/// `php_converter_append_fromUnicode_target`.
fn append_from_unicode(ctx: &mut Ctx, o: &Object, v: &Value, out: &mut Vec<u8>) -> Result<(), Unwind> {
    match &*v.deref() {
        Value::Null => {}
        Value::Int(b) => out.push(*b as u8),
        Value::Str(s) => out.extend_from_slice(s.as_bytes()),
        Value::Array(a) => {
            for x in a.values() {
                append_from_unicode(ctx, o, x, out)?;
            }
        }
        _ => obj_error(ctx, Some(o), U_ILLEGAL_ARGUMENT_ERROR, "fromUCallback() specified illegal type for substitution character")?,
    }
    Ok(())
}

/// `php_converter_do_convert`: `Ok(None)` after the error is recorded.
fn do_convert(ctx: &mut Ctx, o: Option<&Object>, src: &Cnv, dest: &Cnv, input: &[u8]) -> Result<Option<Vec<u8>>, Unwind> {
    let reset = |ctx: &mut Ctx, method: &[u8]| -> Result<(), Unwind> {
        if let Some(o) = o {
            if user_handler(ctx, o).is_some() {
                let args = if method == b"toUCallback" {
                    vec![Value::Int(REASON_RESET), Value::string(b""), Value::string(b""), Value::Ref(PhpRef::new(Value::Int(0)))]
                } else {
                    vec![Value::Int(REASON_RESET), Value::Array(Array::new()), Value::Int(0), Value::Ref(PhpRef::new(Value::Int(0)))]
                };
                ctx.call_method(o, method, &args)?;
            }
        }
        Ok(())
    };
    // ICU's preflight pass, then the real one: callbacks run twice.
    let mut units = Vec::new();
    for _ in 0..2 {
        reset(ctx, b"toUCallback")?;
        let r = match o {
            Some(obj) => match user_handler(ctx, obj) {
                Some(mut h) => {
                    let mut hh: Option<&mut Handler<'_>> = Some(&mut h);
                    to_unicode(src, input, &mut hh)?
                }
                None => to_unicode(src, input, &mut None)?,
            },
            None => to_unicode(src, input, &mut None)?,
        };
        match r {
            Ok(u) => units = u,
            Err(code) => {
                obj_error(ctx, o, code, &failure(code))?;
                return Ok(None);
            }
        }
    }
    let mut bytes = Vec::new();
    for _ in 0..2 {
        reset(ctx, b"fromUCallback")?;
        let r = match o {
            Some(obj) => match user_handler(ctx, obj) {
                Some(mut h) => {
                    let mut hh: Option<&mut Handler<'_>> = Some(&mut h);
                    from_unicode(dest, &units, &mut hh)?
                }
                None => from_unicode(dest, &units, &mut None)?,
            },
            None => from_unicode(dest, &units, &mut None)?,
        };
        match r {
            Ok(b) => bytes = b,
            Err(code) => {
                obj_error(ctx, o, code, &failure(code))?;
                return Ok(None);
            }
        }
    }
    Ok(Some(bytes))
}

fn convert(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let input = str_arg(args, 0);
    let reverse = args.get(1).is_some_and(Value::to_bool);
    let (src, dest) = with(o, |s| {
        s.err.reset();
        (s.src.clone(), s.dest.clone())
    });
    let (Some(src), Some(dest)) = (src, dest) else {
        obj_error(ctx, Some(o), state::U_INVALID_STATE_ERROR, "Internal converters not initialized")?;
        return Ok(Value::Bool(false));
    };
    let (a, b) = if reverse { (dest, src) } else { (src, dest) };
    Ok(match do_convert(ctx, Some(o), &a, &b, &input)? {
        Some(v) => Value::string(&v),
        None => Value::Bool(false),
    })
}

fn transcode(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let input = str_arg(args, 0);
    let dest_name = crate::text_arg(args, 1);
    let src_name = crate::text_arg(args, 2);
    let Some(mut src) = open(ctx, None, &src_name, false)? else {
        return Ok(Value::Bool(false));
    };
    let Some(mut dest) = open(ctx, None, &dest_name, false)? else {
        return Ok(Value::Bool(false));
    };
    let mut error = 0;
    if let Some(Value::Array(opts)) = crate::opt_arg(args, 3).map(|v| v.deref().into_owned()) {
        let get = |k: &str| match opts.get(&rphp_value::ArrayKey::Str(k.as_bytes().into())).map(|v| v.deref().into_owned()) {
            Some(Value::Str(s)) => Some(s.as_bytes().to_vec()),
            _ => None,
        };
        if let Some(s) = get("from_subst") {
            error = set_subst(&mut src, &s[..s.len() & 0x7F]);
        }
        if error == 0 {
            if let Some(s) = get("to_subst") {
                error = set_subst(&mut dest, &s[..s.len() & 0x7F]);
            }
        }
    }
    if error != 0 {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, error, &failure(error))?;
        return Ok(Value::Bool(false));
    }
    Ok(match do_convert(ctx, None, &src, &dest, &input)? {
        Some(v) => Value::string(&v),
        None => Value::Bool(false),
    })
}

/// `ucnv_setSubstChars`: the length must be one the converter takes.
fn set_subst(c: &mut Cnv, s: &[u8]) -> i64 {
    if !c.conv().subst_lens.contains(&s.len()) {
        return U_ILLEGAL_ARGUMENT_ERROR;
    }
    c.subst = s.to_vec();
    0
}

fn set_subst_chars(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let chars = str_arg(args, 0);
    let (e1, e2) = with(o, |s| {
        s.err.reset();
        let e1 = s.src.as_mut().map_or(state::U_INVALID_STATE_ERROR, |c| set_subst(c, &chars));
        let e2 = s.dest.as_mut().map_or(state::U_INVALID_STATE_ERROR, |c| set_subst(c, &chars));
        (e1, e2)
    });
    for e in [e1, e2] {
        if e != 0 {
            obj_error(ctx, Some(o), e, &failure(e))?;
        }
    }
    Ok(Value::Bool(e1 == 0 && e2 == 0))
}

fn get_subst_chars(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with(o, |s| {
        s.err.reset();
        s.src.as_ref().map_or(Value::Null, |c| Value::string(&c.subst))
    }))
}

fn set_encoding(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value], dest: bool) -> NativeResult {
    let o = this(o)?;
    let name = crate::text_arg(args, 0);
    with(o, |s| s.err.reset());
    match open(ctx, Some(o), &name, false)? {
        Some(c) => {
            with(o, |s| if dest { s.dest = Some(c) } else { s.src = Some(c) });
            Ok(Value::Bool(true))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn set_source_encoding(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    set_encoding(ctx, o, args, false)
}

fn set_destination_encoding(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    set_encoding(ctx, o, args, true)
}

fn get_encoding(o: Option<&Object>, dest: bool, f: impl Fn(&Cnv) -> Value) -> NativeResult {
    let o = this(o)?;
    Ok(with(o, |s| {
        s.err.reset();
        let c = if dest { &s.dest } else { &s.src };
        c.as_ref().map_or(Value::Null, f)
    }))
}

fn get_source_encoding(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    get_encoding(o, false, |c| Value::string(c.conv().name.as_bytes()))
}

fn get_destination_encoding(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    get_encoding(o, true, |c| Value::string(c.conv().name.as_bytes()))
}

fn get_source_type(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    get_encoding(o, false, |c| Value::Int(c.conv().ty))
}

fn get_destination_type(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    get_encoding(o, true, |c| Value::Int(c.conv().ty))
}

fn get_error_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with(o, |s| s.err.code)))
}

fn get_error_message(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::string(with(o, |s| s.err.message()).as_bytes()))
}

/// `php_converter_default_callback`: the source converter's substitution
/// bytes for a real fault, `$error` cleared.
fn default_callback(o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let reason = args.first().map_or(0, Value::to_int);
    if !(0..=2).contains(&reason) {
        return Ok(Value::Null);
    }
    let subst = with(o, |s| s.src.as_ref().map(|c| c.subst.clone()));
    let (chars, code) = match subst {
        Some(s) => (s, 0),
        None => (vec![0x1A], state::U_INVALID_STATE_ERROR),
    };
    if let Some(slot) = args.get_mut(3) {
        Value::assign(slot, Value::Int(code));
    }
    Ok(Value::string(&chars))
}

fn to_u_callback(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    default_callback(o, args)
}

fn from_u_callback(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    default_callback(o, args)
}

fn reason_text(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let name = match args.first().map_or(-1, Value::to_int) {
        0 => "REASON_UNASSIGNED",
        1 => "REASON_ILLEGAL",
        2 => "REASON_IRREGULAR",
        3 => "REASON_RESET",
        4 => "REASON_CLOSE",
        5 => "REASON_CLONE",
        _ => {
            let who = ctx.active_function_name();
            return Err(Unwind::value_error(format!("{who}(): Argument #1 ($reason) must be a UConverter::REASON_* constant")));
        }
    };
    Ok(Value::string(name.as_bytes()))
}

fn get_available(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let mut a = Array::new();
    for c in data::CONVERTERS {
        a.push(Value::string(c.name.as_bytes()));
    }
    Ok(Value::Array(a))
}

fn get_aliases(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let name = crate::text_arg(args, 0);
    let mut a = Array::new();
    // A converter's own name first (`UTF16_PlatformEndian` is one), then
    // the aliases.
    let own = data::CONVERTERS.iter().position(|c| strip(c.name) == strip(&name));
    if let Some((idx, _)) = own.map(|i| (i, false)).or_else(|| lookup(&name)) {
        for alias in data::CONVERTERS[idx].aliases {
            a.push(Value::string(alias.as_bytes()));
        }
    }
    Ok(Value::Array(a))
}

fn get_standards(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let mut a = Array::new();
    for s in data::STANDARDS {
        a.push(Value::string(s.as_bytes()));
    }
    Ok(Value::Array(a))
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("convert", convert),
    ("fromUCallback", from_u_callback),
    ("getAliases", get_aliases),
    ("getAvailable", get_available),
    ("getDestinationEncoding", get_destination_encoding),
    ("getDestinationType", get_destination_type),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
    ("getSourceEncoding", get_source_encoding),
    ("getSourceType", get_source_type),
    ("getStandards", get_standards),
    ("getSubstChars", get_subst_chars),
    ("reasonText", reason_text),
    ("setDestinationEncoding", set_destination_encoding),
    ("setSourceEncoding", set_source_encoding),
    ("setSubstChars", set_subst_chars),
    ("toUCallback", to_u_callback),
    ("transcode", transcode),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::UCONVERTER, METHODS, |b| b.payload_clone(payload_clone));
}
