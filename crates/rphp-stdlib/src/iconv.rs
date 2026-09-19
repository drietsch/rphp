//! php-src `ext/iconv` over the pure-Rust [`crate::libmbfl`] (plan S8,
//! ADR-032): the conversion function, the four string functions that count
//! characters instead of bytes, the three MIME header functions and the
//! encoding settings.
//!
//! **Which iconv this reports itself as.** php names the C library it was
//! built against in `ICONV_IMPL`/`ICONV_VERSION`, and the answer decides
//! observable behaviour — above all what `//TRANSLIT` produces, which no
//! two implementations agree on. This module reproduces **GNU libiconv
//! 1.11**, because that is the implementation whose transliteration answers
//! were measured into [`translit`], and it says so in the two constants.
//!
//! **The two suffixes.** A target charset may carry `//TRANSLIT`,
//! `//IGNORE` or both, in either order and in any case:
//!
//! * `//TRANSLIT` replaces a character the target cannot hold with the
//!   table's approximation — `U+00C6` becomes `AE`, `U+2014` becomes `-` —
//!   recursively, since a replacement may itself need replacing. A
//!   character with no entry is still an error, which is why
//!   `iconv('UTF-8', 'ASCII//TRANSLIT', '☃')` is `false` in php too.
//! * `//IGNORE` drops what the target cannot hold, and drops an illegal
//!   *input* sequence as well. It does not rescue a string whose last
//!   character is truncated: php reports that one whatever the suffixes
//!   say, because the input ran out before the character did.
//!
//! Both faults are an `E_NOTICE` and `false`, never an exception.
//!
//! **MIME.** `iconv_mime_encode()` folds by *input* bytes rather than
//! output characters and never splits a character across two encoded
//! words; `iconv_mime_decode()` holds back the whitespace that follows a
//! word and emits it only when ordinary text follows, which is what makes
//! two adjacent words one run of text. Both rules are php's, and
//! `examples/tier-a/iconv/basics.php` pins them.
//!
//! **Known divergences (ADR-004).**
//!
//! * An **empty** charset name means "the locale's charset" to libiconv.
//!   The engine has no locale, so an empty name is UTF-8 here, which is
//!   what a UTF-8 locale gives php.
//! * The string functions convert through UCS-4LE in php, and its name is
//!   in their error message (`conversion from "BOGUS" to "UCS-4LE"`); here
//!   the decoder answers code points directly and the message is
//!   reproduced verbatim.
//! * `iconv_set_encoding()` writes the `iconv.*` ini entries, as php does,
//!   and they are registered with php's empty defaults — but php also
//!   consults the *locale* when they are empty, where this falls back to
//!   `default_charset`.
//! * The three code points `U+02BB`, `U+2018` and `U+201A` transliterate
//!   differently per target converter in libiconv 1.11, which a
//!   per-code-point table cannot express; [`translit`] stores the ASCII
//!   answer (`'`), so an ISO-8859-1/2/3 or CP850 target gets `'` where php
//!   gives `` ` ``.

mod translit;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use crate::libmbfl::{self as mbfl, ConvertBuf, Encoding, ErrorMode, BAD_INPUT};

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("iconv", 3, Some(3), iconv),
    nf!("iconv_strlen", 1, Some(2), iconv_strlen),
    nf!("iconv_substr", 2, Some(4), iconv_substr),
    nf!("iconv_strpos", 2, Some(4), iconv_strpos),
    nf!("iconv_strrpos", 2, Some(3), iconv_strrpos),
    nf!("iconv_mime_encode", 2, Some(3), iconv_mime_encode),
    nf!("iconv_mime_decode", 1, Some(3), iconv_mime_decode),
    nf!(
        "iconv_mime_decode_headers",
        1,
        Some(3),
        iconv_mime_decode_headers
    ),
    nf!("iconv_set_encoding", 2, Some(2), iconv_set_encoding),
    nf!("iconv_get_encoding", 0, Some(1), iconv_get_encoding),
    nf!("ob_iconv_handler", 2, Some(2), ob_iconv_handler),
];

/// `ICONV_MIME_DECODE_STRICT`.
const MIME_DECODE_STRICT: i64 = 1;
/// `ICONV_MIME_DECODE_CONTINUE_ON_ERROR`.
const MIME_DECODE_CONTINUE_ON_ERROR: i64 = 2;

/// Constants this module provides, and the three ini entries
/// `iconv_set_encoding()` writes.
pub(crate) fn register_constants(r: &mut Registry) {
    r.constant("ICONV_IMPL", Value::string(b"libiconv"));
    r.constant("ICONV_VERSION", Value::string(b"1.11"));
    r.constant("ICONV_MIME_DECODE_STRICT", Value::Int(MIME_DECODE_STRICT));
    r.constant(
        "ICONV_MIME_DECODE_CONTINUE_ON_ERROR",
        Value::Int(MIME_DECODE_CONTINUE_ON_ERROR),
    );
    let ini = &mut r.interp().ini;
    for name in [
        "iconv.input_encoding",
        "iconv.output_encoding",
        "iconv.internal_encoding",
    ] {
        ini.register(name, "");
    }
}

// ---- charsets ----------------------------------------------------------------------

/// A charset name as iconv reads it: an encoding plus the two suffixes.
struct Spec {
    enc: &'static Encoding,
    translit: bool,
    ignore: bool,
}

/// Split `//TRANSLIT` / `//IGNORE` off a charset name and resolve the rest.
/// Both suffixes may appear, in either order, in any case; an empty name is
/// the locale's charset, which is UTF-8 here (see the module header).
fn parse_spec(name: &[u8]) -> Option<Spec> {
    let mut base = name.to_vec();
    let (mut translit, mut ignore) = (false, false);
    loop {
        let lower = base.to_ascii_lowercase();
        if lower.ends_with(b"//translit") {
            translit = true;
            base.truncate(base.len() - 10);
            continue;
        }
        if lower.ends_with(b"//ignore") {
            ignore = true;
            base.truncate(base.len() - 8);
            continue;
        }
        break;
    }
    if base.is_empty() {
        base = b"UTF-8".to_vec();
    }
    mbfl::name2encoding(&base).map(|enc| Spec {
        enc,
        translit,
        ignore,
    })
}

/// php's two conversion faults, one `E_NOTICE` apiece.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    Illegal,
    Incomplete,
}

impl Fault {
    fn message(self) -> &'static str {
        match self {
            Fault::Illegal => "Detected an illegal character in input string",
            Fault::Incomplete => "Detected an incomplete multibyte character in input string",
        }
    }
}

/// Whether the input's **last** character ran out of bytes rather than
/// being illegal. The walk is over the encoding's `mblen_table`, so it is
/// exact for every encoding that has one and conservative (never
/// "incomplete") for the rest.
fn incomplete_tail(enc: &Encoding, input: &[u8]) -> bool {
    let Some(tbl) = enc.mblen_table else {
        return false;
    };
    let mut i = 0;
    while i < input.len() {
        let n = tbl[input[i] as usize] as usize;
        if n == 0 {
            return false;
        }
        if i + n > input.len() {
            return true;
        }
        i += n;
    }
    false
}

/// Decode `input`, reporting php's fault for a sequence that is illegal or
/// truncated. `ignore` drops an illegal sequence, which is what `//IGNORE`
/// on the *target* does to the source as well; a truncated tail is a fault
/// either way.
fn to_wchars(enc: &Encoding, input: &[u8], ignore: bool) -> Result<Vec<u32>, Fault> {
    let mut w = enc.decode(input);
    if !w.contains(&BAD_INPUT) {
        return Ok(w);
    }
    if w.last() == Some(&BAD_INPUT)
        && w.iter().filter(|c| **c == BAD_INPUT).count() == 1
        && incomplete_tail(enc, input)
    {
        return Err(Fault::Incomplete);
    }
    if !ignore {
        return Err(Fault::Illegal);
    }
    w.retain(|c| *c != BAD_INPUT);
    Ok(w)
}

/// Whether the encoding can represent `cp` at all.
fn encodable(enc: &Encoding, cp: u32) -> bool {
    let mut buf = ConvertBuf::new(b'?' as u32, ErrorMode::None);
    enc.encode(&[cp], &mut buf, true);
    buf.errors == 0
}

/// Append `cp`'s transliteration, recursively: `U+00BC` is `1/4` and
/// `U+2033` is `''`, whose own characters must be representable in turn.
/// Nothing is appended when the chain does not terminate in the target.
fn push_translit(enc: &Encoding, cp: u32, out: &mut Vec<u32>, depth: u32) -> bool {
    if depth > 4 {
        return false;
    }
    let Some(rep) = translit::lookup(cp) else {
        return false;
    };
    let mark = out.len();
    for &r in rep {
        if encodable(enc, r) {
            out.push(r);
            continue;
        }
        if push_translit(enc, r, out, depth + 1) {
            continue;
        }
        out.truncate(mark);
        return false;
    }
    true
}

/// Encode code points for the target, applying its two suffixes. The whole
/// string is encoded in one pass at the end so a stateful target
/// (ISO-2022-JP) sees its escape sequences once.
fn from_wchars(spec: &Spec, w: &[u32]) -> Result<Vec<u8>, Fault> {
    let mut cps: Vec<u32> = Vec::with_capacity(w.len());
    for &cp in w {
        if encodable(spec.enc, cp) {
            cps.push(cp);
            continue;
        }
        if spec.translit && push_translit(spec.enc, cp, &mut cps, 0) {
            continue;
        }
        if spec.ignore {
            continue;
        }
        return Err(Fault::Illegal);
    }
    let mut buf = ConvertBuf::new(b'?' as u32, ErrorMode::None);
    spec.enc.encode(&cps, &mut buf, true);
    Ok(buf.out)
}

/// php's `Wrong encoding` warning, which names the pair it could not open.
fn wrong_encoding(ctx: &mut Ctx, who: &str, from: &[u8], to: &[u8]) -> Result<(), Unwind> {
    ctx.warn(&format!(
        "{who}(): Wrong encoding, conversion from \"{}\" to \"{}\" is not allowed",
        String::from_utf8_lossy(from),
        String::from_utf8_lossy(to)
    ))
}

/// The whole conversion, with php's answer for each way it can fail.
fn convert_or_warn(
    ctx: &mut Ctx,
    who: &str,
    from_name: &[u8],
    to_name: &[u8],
    s: &[u8],
) -> NativeResult {
    let (Some(from), Some(to)) = (parse_spec(from_name), parse_spec(to_name)) else {
        wrong_encoding(ctx, who, from_name, to_name)?;
        return Ok(Value::Bool(false));
    };
    match to_wchars(from.enc, s, to.ignore).and_then(|w| from_wchars(&to, &w)) {
        Ok(bytes) => Ok(Value::string(&bytes)),
        Err(f) => {
            ctx.notice(&format!("{who}(): {}", f.message()))?;
            Ok(Value::Bool(false))
        }
    }
}

// ---- arguments ---------------------------------------------------------------------

/// A `string` parameter.
fn str_arg(v: &Value, func: &str, n: usize, name: &str) -> Result<Vec<u8>, Unwind> {
    match v.deref().as_ref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => {
            Err(Unwind::type_error(format!(
                "{func}(): Argument #{n} (${name}) must be of type string, {} given",
                v.type_name()
            )))
        }
        _ => Ok(v.to_php_bytes()),
    }
}

/// A `?string` parameter: absent or `null` is `None`.
fn opt_str_arg(args: &[Value], i: usize, func: &str, name: &str) -> Result<Option<Vec<u8>>, Unwind> {
    match args.get(i) {
        None => Ok(None),
        Some(v) => match &*v.deref() {
            Value::Null => Ok(None),
            _ => str_arg(v, func, i + 1, name).map(Some),
        },
    }
}

/// An `int` parameter, absent or `null` being `None`.
fn opt_int_arg(args: &[Value], i: usize) -> Option<i64> {
    match args.get(i) {
        None => None,
        Some(v) => match &*v.deref() {
            Value::Null => None,
            other => Some(other.to_int()),
        },
    }
}

/// The charset the string functions and the MIME functions default to:
/// `iconv.internal_encoding`, then `default_charset`, then UTF-8.
fn internal_name(ctx: &Ctx) -> Vec<u8> {
    for key in ["iconv.internal_encoding", "default_charset"] {
        let v = ctx.ini_get(key).unwrap_or("");
        if !v.is_empty() {
            return v.as_bytes().to_vec();
        }
    }
    b"UTF-8".to_vec()
}

/// The code points of `s`, or `None` after reporting php's answer for a
/// charset it cannot open or a string it cannot read. The charset in the
/// error message is the one php names: the string functions convert *to*
/// UCS-4LE, so that is the other half of the pair.
fn wchars_or_warn(
    ctx: &mut Ctx,
    who: &str,
    s: &[u8],
    name: &[u8],
) -> Result<Option<Vec<u32>>, Unwind> {
    let Some(spec) = parse_spec(name) else {
        wrong_encoding(ctx, who, name, b"UCS-4LE")?;
        return Ok(None);
    };
    match to_wchars(spec.enc, s, spec.ignore) {
        Ok(w) => Ok(Some(w)),
        Err(f) => {
            ctx.notice(&format!("{who}(): {}", f.message()))?;
            Ok(None)
        }
    }
}

/// Encode code points back into the charset the caller named, which the
/// string functions already opened, so it cannot fail here.
fn encode_back(name: &[u8], w: &[u32]) -> Vec<u8> {
    let Some(spec) = parse_spec(name) else {
        return Vec::new();
    };
    from_wchars(&spec, w).unwrap_or_default()
}

// ---- the conversion function -------------------------------------------------------

/// `iconv(string $from_encoding, string $to_encoding, string $string): string|false`
fn iconv(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from = str_arg(&args[0], "iconv", 1, "from_encoding")?;
    let to = str_arg(&args[1], "iconv", 2, "to_encoding")?;
    let s = str_arg(&args[2], "iconv", 3, "string")?;
    convert_or_warn(ctx, "iconv", &from, &to, &s)
}

// ---- the string functions ----------------------------------------------------------

/// `iconv_strlen(string $string, ?string $encoding = null): int|false`
fn iconv_strlen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "iconv_strlen", 1, "string")?;
    let name = match opt_str_arg(args, 1, "iconv_strlen", "encoding")? {
        Some(n) => n,
        None => internal_name(ctx),
    };
    Ok(match wchars_or_warn(ctx, "iconv_strlen", &s, &name)? {
        Some(w) => Value::Int(w.len() as i64),
        None => Value::Bool(false),
    })
}

/// php's window for `$offset`/`$length` over `len` characters, with
/// `substr()`'s rules: a negative offset counts from the end, a negative
/// length stops that many characters before it.
fn window(len: usize, offset: i64, length: Option<i64>) -> (usize, usize) {
    let len = len as i64;
    let start = if offset < 0 {
        (len + offset).max(0)
    } else {
        offset.min(len)
    };
    let end = match length {
        None => len,
        Some(n) if n < 0 => (len + n).max(start),
        Some(n) => (start + n).min(len),
    };
    (start as usize, end.max(start) as usize)
}

/// `iconv_substr(string $string, int $offset, ?int $length = null, ?string $encoding = null): string|false`
fn iconv_substr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "iconv_substr", 1, "string")?;
    let offset = args[1].deref().to_int();
    let length = opt_int_arg(args, 2);
    let name = match opt_str_arg(args, 3, "iconv_substr", "encoding")? {
        Some(n) => n,
        None => internal_name(ctx),
    };
    let Some(w) = wchars_or_warn(ctx, "iconv_substr", &s, &name)? else {
        return Ok(Value::Bool(false));
    };
    let (from, to) = window(w.len(), offset, length);
    Ok(Value::string(&encode_back(&name, &w[from..to])))
}

/// The first index at which `needle` occurs in `hay`, both as code points.
fn find(hay: &[u32], needle: &[u32], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// `iconv_strpos(string $haystack, string $needle, int $offset = 0, ?string $encoding = null): int|false`
fn iconv_strpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_strpos";
    let hay = str_arg(&args[0], who, 1, "haystack")?;
    let needle = str_arg(&args[1], who, 2, "needle")?;
    let offset = opt_int_arg(args, 2).unwrap_or(0);
    let name = match opt_str_arg(args, 3, who, "encoding")? {
        Some(n) => n,
        None => internal_name(ctx),
    };
    let (Some(hay), Some(needle)) = (
        wchars_or_warn(ctx, who, &hay, &name)?,
        wchars_or_warn(ctx, who, &needle, &name)?,
    ) else {
        return Ok(Value::Bool(false));
    };
    let len = hay.len() as i64;
    let start = if offset < 0 { len + offset } else { offset };
    if start < 0 || start > len {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        )));
    }
    Ok(match find(&hay, &needle, start as usize) {
        Some(i) => Value::Int(i as i64),
        None => Value::Bool(false),
    })
}

/// `iconv_strrpos(string $haystack, string $needle, ?string $encoding = null): int|false`
fn iconv_strrpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_strrpos";
    let hay = str_arg(&args[0], who, 1, "haystack")?;
    let needle = str_arg(&args[1], who, 2, "needle")?;
    let name = match opt_str_arg(args, 2, who, "encoding")? {
        Some(n) => n,
        None => internal_name(ctx),
    };
    let (Some(hay), Some(needle)) = (
        wchars_or_warn(ctx, who, &hay, &name)?,
        wchars_or_warn(ctx, who, &needle, &name)?,
    ) else {
        return Ok(Value::Bool(false));
    };
    if needle.is_empty() || needle.len() > hay.len() {
        return Ok(Value::Bool(false));
    }
    let last = (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| hay[i..i + needle.len()] == *needle);
    Ok(match last {
        Some(i) => Value::Int(i as i64),
        None => Value::Bool(false),
    })
}

// ---- MIME headers ------------------------------------------------------------------

/// The `=?charset?B?…?=` overhead around a chunk.
fn word_overhead(charset: &[u8]) -> usize {
    // "=?" + charset + "?B?" + "?="
    2 + charset.len() + 3 + 2
}

/// Base64 of `bytes`, RFC 2045's alphabet with padding.
fn base64(bytes: &[u8]) -> Vec<u8> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(A[(n >> 18) as usize & 63]);
        out.push(A[(n >> 12) as usize & 63]);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if c.len() > 2 { A[n as usize & 63] } else { b'=' });
    }
    out
}

/// Decode base64, ignoring every byte outside the alphabet — which is what
/// makes `=?UTF-8?B?!!!?=` an empty string rather than an error.
fn base64_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Whether php's `Q` scheme writes the byte as itself. Everything outside
/// printable ASCII is escaped, and so are `=`, `?` and `_` — php escapes
/// the space as `=20` rather than `_`.
fn q_literal(b: u8) -> bool {
    (0x21..=0x7E).contains(&b) && b != b'=' && b != b'?' && b != b'_'
}

/// The `Q` encoding of one byte.
fn q_encode(b: u8, out: &mut Vec<u8>) {
    if q_literal(b) {
        out.push(b);
        return;
    }
    out.push(b'=');
    out.push(b"0123456789ABCDEF"[(b >> 4) as usize]);
    out.push(b"0123456789ABCDEF"[(b & 0xF) as usize]);
}

/// `iconv_mime_encode(string $field_name, string $field_value, array $options = []): string|false`
///
/// php folds by *input* bytes, not by output characters: the budget for a
/// `B` chunk is the base64 groups that fit in what is left of the line,
/// less one group it keeps in hand, and a `Q` chunk fills the line exactly.
/// A character is never split across two encoded words.
fn iconv_mime_encode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_mime_encode";
    let field = str_arg(&args[0], who, 1, "field_name")?;
    let value = str_arg(&args[1], who, 2, "field_value")?;
    let opts = match args.get(2).map(|v| v.deref()) {
        Some(v) => match &*v {
            Value::Array(a) => Some(a.clone()),
            Value::Null => None,
            _ => {
                return Err(Unwind::type_error(format!(
                    "{who}(): Argument #3 ($options) must be of type array, {} given",
                    args[2].type_name()
                )))
            }
        },
        None => None,
    };
    let opt = |key: &[u8]| -> Option<Vec<u8>> {
        opts.as_ref()
            .and_then(|a| a.get_deref(&ArrayKey::str(key)))
            .map(|v| v.to_php_bytes())
    };
    let base64_scheme = !opt(b"scheme").is_some_and(|s| matches!(s.first(), Some(b'Q' | b'q')));
    let in_name = opt(b"input-charset").unwrap_or_else(|| internal_name(ctx));
    let out_name = opt(b"output-charset").unwrap_or_else(|| internal_name(ctx));
    let max_line = opt(b"line-length")
        .map(|v| String::from_utf8_lossy(&v).parse::<i64>().unwrap_or(76))
        .unwrap_or(76)
        .max(1) as usize;
    let brk = opt(b"line-break-chars").unwrap_or_else(|| b"\r\n".to_vec());

    let (Some(from), Some(to)) = (parse_spec(&in_name), parse_spec(&out_name)) else {
        wrong_encoding(ctx, who, &in_name, &out_name)?;
        return Ok(Value::Bool(false));
    };
    let w = match to_wchars(from.enc, &value, to.ignore) {
        Ok(w) => w,
        Err(f) => {
            ctx.notice(&format!("{who}(): {}", f.message()))?;
            return Ok(Value::Bool(false));
        }
    };
    // Every character on its own, so a chunk can end only on a boundary.
    let mut chars: Vec<Vec<u8>> = Vec::with_capacity(w.len());
    for &cp in &w {
        match from_wchars(&to, &[cp]) {
            Ok(bytes) => chars.push(bytes),
            Err(f) => {
                ctx.notice(&format!("{who}(): {}", f.message()))?;
                return Ok(Value::Bool(false));
            }
        }
    }

    let charset = to.enc.name.as_bytes();
    let ovh = word_overhead(charset);
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&field);
    out.extend_from_slice(b": ");
    let mut prefix = field.len() + 2;
    let mut i = 0;
    loop {
        let room = max_line.saturating_sub(prefix + ovh);
        let mut chunk: Vec<u8> = Vec::new();
        let mut encoded = 0usize;
        while i < chars.len() {
            let c = &chars[i];
            if base64_scheme {
                // The budget is whole base64 groups, less the one php keeps
                // in hand; below one character nothing would ever fit, so
                // the character goes out alone and the line runs long.
                let budget = (room / 4 * 3).saturating_sub(4);
                if chunk.len() + c.len() > budget.max(c.len()) {
                    break;
                }
            } else {
                let mut q = Vec::new();
                for &b in c {
                    q_encode(b, &mut q);
                }
                if encoded + q.len() > room.max(q.len()) {
                    break;
                }
                encoded += q.len();
            }
            chunk.extend_from_slice(c);
            i += 1;
        }
        if chunk.is_empty() && i < chars.len() {
            // A line length too small for even one character: take it
            // anyway rather than loop for ever.
            chunk.extend_from_slice(&chars[i]);
            i += 1;
        }
        out.extend_from_slice(b"=?");
        out.extend_from_slice(charset);
        out.extend_from_slice(if base64_scheme { b"?B?" } else { b"?Q?" });
        if base64_scheme {
            out.extend_from_slice(&base64(&chunk));
        } else {
            let mut q = Vec::new();
            for &b in &chunk {
                q_encode(b, &mut q);
            }
            out.extend_from_slice(&q);
        }
        out.extend_from_slice(b"?=");
        if i >= chars.len() {
            break;
        }
        out.extend_from_slice(&brk);
        out.push(b' ');
        prefix = 1;
    }
    Ok(Value::string(&out))
}

/// One `=?charset?scheme?text?=` word, already decoded into bytes.
struct Word {
    charset: Vec<u8>,
    text: Vec<u8>,
}

/// Read an encoded word at `s[at..]`, answering it and the index just past
/// it. `None` means the text is not a well-formed encoded word.
fn read_word(s: &[u8], at: usize) -> Option<(Word, usize)> {
    let rest = s.get(at..)?;
    let rest = rest.strip_prefix(b"=?")?;
    let mut parts = rest.splitn(3, |&b| b == b'?');
    let charset = parts.next()?.to_vec();
    let scheme = parts.next()?;
    let tail = parts.next()?;
    let end = tail.windows(2).position(|w| w == b"?=")?;
    let body = &tail[..end];
    // The charset is whatever stands there, spaces and all: php hands it to
    // the converter and reports `conversion from "???"` when it does not
    // open, rather than calling the word malformed.
    let text = match scheme {
        [b'B' | b'b'] => base64_decode(body),
        [b'Q' | b'q'] => {
            let mut out = Vec::with_capacity(body.len());
            let mut i = 0;
            while i < body.len() {
                match body[i] {
                    b'_' => out.push(b' '),
                    b'=' if i + 2 < body.len() => {
                        let hex = std::str::from_utf8(&body[i + 1..i + 3]).ok()?;
                        out.push(u8::from_str_radix(hex, 16).ok()?);
                        i += 2;
                    }
                    b => out.push(b),
                }
                i += 1;
            }
            out
        }
        _ => return None,
    };
    // `at` + "=?" + charset + "?" + scheme + "?" + body + "?="
    let used = 2 + charset.len() + 1 + scheme.len() + 1 + end + 2;
    Some((Word { charset, text }, at + used))
}

/// php's `Malformed string` warning.
fn malformed(ctx: &mut Ctx, who: &str) -> Result<(), Unwind> {
    ctx.warn(&format!("{who}(): Malformed string"))
}

/// The length of a folding line break at `at` — a line break whose next
/// line starts with a space or a tab — including the whitespace run that
/// follows it.
fn fold_at(s: &[u8], at: usize) -> Option<usize> {
    let brk = if s[at..].starts_with(b"\r\n") {
        2
    } else if s[at] == b'\n' {
        1
    } else {
        return None;
    };
    if !matches!(s.get(at + brk), Some(b' ' | b'\t')) {
        return None;
    }
    let mut n = brk;
    while matches!(s.get(at + n), Some(b' ' | b'\t')) {
        n += 1;
    }
    Some(n)
}

/// Decode one header value.
///
/// Encoded words become text in `target`; everything else is copied
/// through. What decides the spacing is php's rule for the **linear
/// whitespace after a word**: it is held back, and it reaches the output
/// only when ordinary text follows. Another encoded word, the end of the
/// value or a fold all discard it — which is why `"=?…?= =?…?="` is two
/// characters with nothing between them, `"=?…?=  junk"` keeps both spaces,
/// and a value ending in whitespace loses it. A fold *not* after a word is
/// one space, which is how a header split over two lines reads.
fn decode_value(
    ctx: &mut Ctx,
    who: &str,
    s: &[u8],
    target: &[u8],
    mode: i64,
) -> Result<Option<Vec<u8>>, Unwind> {
    let Some(spec) = parse_spec(target) else {
        wrong_encoding(ctx, who, target, b"UTF-8")?;
        return Ok(None);
    };
    let strict = mode & MIME_DECODE_STRICT != 0;
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut ws: Vec<u8> = Vec::new();
    let mut after_word = false;
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = fold_at(s, i) {
            ws = if after_word { Vec::new() } else { vec![b' '] };
            i += n;
            continue;
        }
        if s[i].is_ascii_whitespace() {
            ws.push(s[i]);
            i += 1;
            continue;
        }
        // RFC 2047 lets an encoded word touch only whitespace or the ends
        // of the text, and `ICONV_MIME_DECODE_STRICT` holds php to it: one
        // with anything else beside it is not a word, and its text goes
        // through as it stands.
        let boundary = !strict || i == 0 || s[i - 1].is_ascii_whitespace();
        if boundary && s[i] == b'=' && s.get(i + 1) == Some(&b'?') {
            match read_word(s, i) {
                Some((w, next)) if !strict || s.get(next).is_none_or(u8::is_ascii_whitespace) => {
                    if !after_word {
                        out.extend_from_slice(&ws);
                    }
                    ws.clear();
                    let Some(from) = parse_spec(&w.charset) else {
                        if mode & MIME_DECODE_CONTINUE_ON_ERROR == 0 {
                            wrong_encoding(ctx, who, b"???", target)?;
                            return Ok(None);
                        }
                        out.extend_from_slice(&s[i..next]);
                        i = next;
                        after_word = true;
                        continue;
                    };
                    match to_wchars(from.enc, &w.text, spec.ignore)
                        .and_then(|cps| from_wchars(&spec, &cps))
                    {
                        Ok(bytes) => out.extend_from_slice(&bytes),
                        Err(f) => {
                            if mode & MIME_DECODE_CONTINUE_ON_ERROR == 0 {
                                ctx.notice(&format!("{who}(): {}", f.message()))?;
                                return Ok(None);
                            }
                        }
                    }
                    i = next;
                    after_word = true;
                    continue;
                }
                // Strict mode meeting a well-formed word with text beside
                // it: not a word after all, so its text goes through. A
                // word that does not parse is malformed in either mode.
                Some(_) => {}
                None if mode & MIME_DECODE_CONTINUE_ON_ERROR == 0 => {
                    malformed(ctx, who)?;
                    return Ok(None);
                }
                None => {}
            }
        }
        out.extend_from_slice(&ws);
        ws.clear();
        out.push(s[i]);
        after_word = false;
        i += 1;
    }
    Ok(Some(out))
}

/// Split a header block into logical lines: a line break that is not a
/// fold ends one, and the fold stays in the value for [`decode_value`].
fn logical_lines(s: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = fold_at(s, i) {
            i += n;
            continue;
        }
        let brk = if s[i..].starts_with(b"\r\n") {
            2
        } else if s[i] == b'\n' {
            1
        } else {
            0
        };
        if brk > 0 {
            out.push(&s[start..i]);
            i += brk;
            start = i;
            continue;
        }
        i += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `iconv_mime_decode(string $string, int $mode = 0, ?string $encoding = null): string|false`
fn iconv_mime_decode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_mime_decode";
    let s = str_arg(&args[0], who, 1, "string")?;
    let mode = opt_int_arg(args, 1).unwrap_or(0);
    let target = match opt_str_arg(args, 2, who, "encoding")? {
        Some(n) => n,
        None => internal_name(ctx),
    };
    Ok(match decode_value(ctx, who, &s, &target, mode)? {
        Some(v) => Value::string(&v),
        None => Value::Bool(false),
    })
}

/// `iconv_mime_decode_headers(string $headers, int $mode = 0, ?string $encoding = null): array|false`
///
/// A name that repeats collects its values into an array, which is how php
/// answers `Received:` — and a line that is not `Name: value` is skipped
/// rather than reported.
fn iconv_mime_decode_headers(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_mime_decode_headers";
    let s = str_arg(&args[0], who, 1, "headers")?;
    let mode = opt_int_arg(args, 1).unwrap_or(0);
    let target = match opt_str_arg(args, 2, who, "encoding")? {
        Some(n) => n,
        None => internal_name(ctx),
    };
    let mut out = Array::new();
    for line in logical_lines(&s) {
        if line.is_empty() {
            break;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let name = &line[..colon];
        if name.is_empty() || name.contains(&b' ') {
            continue;
        }
        let value = line[colon + 1..]
            .strip_prefix(b" ")
            .unwrap_or(&line[colon + 1..]);
        let Some(decoded) = decode_value(ctx, who, value, &target, mode)? else {
            return Ok(Value::Bool(false));
        };
        let key = ArrayKey::str(name);
        match out.get_deref(&key) {
            None => out.set(key, Value::string(&decoded)),
            Some(Value::Array(mut a)) => {
                a.push(Value::string(&decoded));
                out.set(key, Value::Array(a));
            }
            Some(first) => {
                let mut a = Array::new();
                a.push(first);
                a.push(Value::string(&decoded));
                out.set(key, Value::Array(a));
            }
        }
    }
    Ok(Value::Array(out))
}

// ---- the settings ------------------------------------------------------------------

/// The ini entry a `$type` names, or `None` when it names nothing.
fn type_ini(kind: &[u8]) -> Option<&'static str> {
    match kind {
        b"input_encoding" => Some("iconv.input_encoding"),
        b"output_encoding" => Some("iconv.output_encoding"),
        b"internal_encoding" => Some("iconv.internal_encoding"),
        _ => None,
    }
}

/// `iconv_set_encoding(string $type, string $encoding): bool` — php
/// deprecates all three ini entries and validates the charset not at all.
fn iconv_set_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_set_encoding";
    let kind = str_arg(&args[0], who, 1, "type")?;
    let enc = str_arg(&args[1], who, 2, "encoding")?;
    let Some(key) = type_ini(&kind) else {
        return Ok(Value::Bool(false));
    };
    ctx.deprecated(&format!("{who}(): Use of {key} is deprecated"))?;
    ctx.ini_set(key, &String::from_utf8_lossy(&enc));
    Ok(Value::Bool(true))
}

/// The value `iconv_get_encoding()` reports for one entry: what was set,
/// or — and this is where the two defaults part company — php's *engine*
/// charset. An empty `iconv.input_encoding` does not inherit
/// `iconv.internal_encoding`; only the string functions do that, which is
/// why setting the internal one leaves the other two reading `UTF-8`.
fn setting(ctx: &Ctx, key: &str) -> Vec<u8> {
    let v = ctx.ini_get(key).unwrap_or("");
    if !v.is_empty() {
        return v.as_bytes().to_vec();
    }
    let charset = ctx.ini_get("default_charset").unwrap_or("");
    if charset.is_empty() {
        b"UTF-8".to_vec()
    } else {
        charset.as_bytes().to_vec()
    }
}

/// `iconv_get_encoding(string $type = "all"): array|string|false`
fn iconv_get_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "iconv_get_encoding";
    let kind = match opt_str_arg(args, 0, who, "type")? {
        Some(k) => k,
        None => b"all".to_vec(),
    };
    if kind.eq_ignore_ascii_case(b"all") {
        let mut a = Array::new();
        for (name, key) in [
            ("input_encoding", "iconv.input_encoding"),
            ("output_encoding", "iconv.output_encoding"),
            ("internal_encoding", "iconv.internal_encoding"),
        ] {
            a.set(
                ArrayKey::str(name.as_bytes()),
                Value::string(&setting(ctx, key)),
            );
        }
        return Ok(Value::Array(a));
    }
    Ok(match type_ini(&kind) {
        Some(key) => Value::string(&setting(ctx, key)),
        None => Value::Bool(false),
    })
}

/// `ob_iconv_handler(string $contents, int $status): string` — the output
/// handler that converts the buffer from the internal to the output
/// encoding. With both unset they are the same charset and the buffer goes
/// out untouched, which is php's behaviour too.
fn ob_iconv_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let who = "ob_iconv_handler";
    let s = str_arg(&args[0], who, 1, "contents")?;
    let _status = opt_int_arg(args, 1).unwrap_or(0);
    let from = setting(ctx, "iconv.internal_encoding");
    let to = setting(ctx, "iconv.output_encoding");
    if from.eq_ignore_ascii_case(&to) {
        return Ok(Value::string(&s));
    }
    convert_or_warn(ctx, who, &from, &to, &s)
}
