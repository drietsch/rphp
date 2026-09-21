//! php-src `ext/mbstring/mbstring.c` (minus `mb_ereg*`, which needs
//! Oniguruma and is cataloged) over the pure-Rust [`crate::libmbfl`]
//! (plan S8, ADR-032).
//!
//! Every function decodes its input to wchars with the selected encoding,
//! works on code points, and re-encodes with the request's substitute
//! character — the shape of php 8.3+'s "fast" conversion API, whose
//! observable rules (illegal sequences become `?`, byte-exact fast paths
//! for fixed-width encodings and `mb_strcut`) are kept.
//!
//! **State.** php keeps the current internal encoding, HTTP output
//! encoding, substitute character and detect order in module globals that
//! `mb_*` setters change without touching the ini table. Until `ExtState`
//! grows an mbstring slot they live in a thread-local ([`State`]); a value
//! never set through an `mb_*` function is derived from the ini directives
//! (`mbstring.internal_encoding` → `default_charset`, …) on every read, so
//! `ini_set()` keeps working for those. `mb_language()` alters the
//! `mbstring.language` ini entry exactly as php does.

use std::cell::RefCell;

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use crate::libmbfl::case::{convert_case, convert_case_wchars, CaseMode};
use crate::libmbfl::tables::eaw::{EAW, FIRST_DOUBLEWIDTH};
use crate::libmbfl::{self as mbfl, detect, ConvertBuf, Encoding, ErrorMode, Id, Language, BAD_INPUT};
use crate::strings::str_value;

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("mb_language", 0, Some(1), mb_language),
    nf!("mb_internal_encoding", 0, Some(1), mb_internal_encoding),
    nf!("mb_http_input", 0, Some(1), mb_http_input),
    nf!("mb_http_output", 0, Some(1), mb_http_output),
    nf!("mb_detect_order", 0, Some(1), mb_detect_order),
    nf!("mb_substitute_character", 0, Some(1), mb_substitute_character),
    nf!("mb_preferred_mime_name", 1, Some(1), mb_preferred_mime_name),
    nf_ref!("mb_parse_str", 2, Some(2), 0b10, mb_parse_str),
    nf!("mb_output_handler", 2, Some(2), mb_output_handler),
    nf!("mb_str_split", 1, Some(3), mb_str_split),
    nf!("mb_strlen", 1, Some(2), mb_strlen),
    nf!("mb_strpos", 2, Some(4), mb_strpos),
    nf!("mb_strrpos", 2, Some(4), mb_strrpos),
    nf!("mb_stripos", 2, Some(4), mb_stripos),
    nf!("mb_strripos", 2, Some(4), mb_strripos),
    nf!("mb_strstr", 2, Some(4), mb_strstr),
    nf!("mb_strrchr", 2, Some(4), mb_strrchr),
    nf!("mb_stristr", 2, Some(4), mb_stristr),
    nf!("mb_strrichr", 2, Some(4), mb_strrichr),
    nf!("mb_substr_count", 2, Some(3), mb_substr_count),
    nf!("mb_substr", 2, Some(4), mb_substr),
    nf!("mb_strcut", 2, Some(4), mb_strcut),
    nf!("mb_strwidth", 1, Some(2), mb_strwidth),
    nf!("mb_strimwidth", 3, Some(5), mb_strimwidth),
    nf!("mb_convert_encoding", 2, Some(3), mb_convert_encoding),
    nf!("mb_convert_case", 2, Some(3), mb_convert_case),
    nf!("mb_strtoupper", 1, Some(2), mb_strtoupper),
    nf!("mb_strtolower", 1, Some(2), mb_strtolower),
    nf!("mb_ucfirst", 1, Some(2), mb_ucfirst),
    nf!("mb_lcfirst", 1, Some(2), mb_lcfirst),
    nf!("mb_trim", 1, Some(3), mb_trim),
    nf!("mb_ltrim", 1, Some(3), mb_ltrim),
    nf!("mb_rtrim", 1, Some(3), mb_rtrim),
    nf!("mb_detect_encoding", 1, Some(3), mb_detect_encoding),
    nf!("mb_list_encodings", 0, Some(0), mb_list_encodings),
    nf!("mb_encoding_aliases", 1, Some(1), mb_encoding_aliases),
    nf_ref!("mb_convert_variables", 3, None, 0xFFFF_FFFC, mb_convert_variables),
    nf!("mb_encode_numericentity", 2, Some(4), mb_encode_numericentity),
    nf!("mb_decode_numericentity", 2, Some(3), mb_decode_numericentity),
    nf!("mb_get_info", 0, Some(1), mb_get_info),
    nf!("mb_check_encoding", 0, Some(2), mb_check_encoding),
    nf!("mb_ord", 1, Some(2), mb_ord),
    nf!("mb_chr", 1, Some(2), mb_chr),
    nf!("mb_str_pad", 2, Some(5), mb_str_pad),
    nf!("mb_scrub", 1, Some(2), mb_scrub),
    nf!("mb_encode_mimeheader", 1, Some(5), mb_encode_mimeheader),
    nf!("mb_decode_mimeheader", 1, Some(1), mb_decode_mimeheader),
];

/// The `MB_CASE_*` constants, `MB_ONIGURUMA_VERSION`, and the ini
/// directives (`manifest/php-8.5.0/ini.json`; `internal_encoding` /
/// `input_encoding` / `output_encoding` are the core directives the
/// defaults fall back to).
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("MB_CASE_UPPER", 0),
        ("MB_CASE_LOWER", 1),
        ("MB_CASE_TITLE", 2),
        ("MB_CASE_FOLD", 3),
        ("MB_CASE_UPPER_SIMPLE", 4),
        ("MB_CASE_LOWER_SIMPLE", 5),
        ("MB_CASE_TITLE_SIMPLE", 6),
        ("MB_CASE_FOLD_SIMPLE", 7),
    ] {
        r.constant(name, Value::Int(v));
    }
    r.constant("MB_ONIGURUMA_VERSION", Value::string(b"6.9.10"));
    let ini = &mut r.interp().ini;
    for (name, default) in [
        ("mbstring.language", "neutral"),
        ("mbstring.detect_order", ""),
        ("mbstring.http_input", ""),
        ("mbstring.http_output", ""),
        ("mbstring.internal_encoding", ""),
        ("mbstring.substitute_character", ""),
        ("mbstring.encoding_translation", "0"),
        ("mbstring.http_output_conv_mimetypes", "^(text/|application/xhtml\\+xml)"),
        ("mbstring.strict_detection", "0"),
        ("mbstring.regex_retry_limit", "1000000"),
        ("mbstring.regex_stack_limit", "100000"),
        ("internal_encoding", ""),
        ("input_encoding", ""),
        ("output_encoding", ""),
    ] {
        ini.register(name, default);
    }
}

// ---- request state -------------------------------------------------------------------

/// The mbstring module globals `mb_*` setters change (see the module docs).
#[derive(Default)]
struct State {
    /// `mb_internal_encoding($x)`.
    internal: Option<Id>,
    /// `mb_http_output($x)` (`Id::Pass` for `pass`).
    http_output: Option<Id>,
    /// `mb_substitute_character($x)`.
    subst: Option<(ErrorMode, u32)>,
    /// `mb_detect_order($x)`, or the snapshot php takes at request start.
    detect_order: Option<Vec<Id>>,
    /// `MBSTRG(illegalchars)`.
    illegal_chars: i64,
    /// `MBSTRG(http_input_identify)` (set by `mb_parse_str`).
    http_input_identify: Option<Id>,
    /// `mb_output_handler` has started converting.
    outconv_enabled: bool,
    /// `MBSTRG(last_used_encoding_name)`: the name php resolved last; a
    /// repeat of it skips the byte-codec deprecation notice.
    last_used: Option<Vec<u8>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// php's `RSHUTDOWN`: the runtime settings (`mb_internal_encoding()` and
/// friends) fall back to the ini defaults for the next request.
pub(crate) fn request_shutdown() {
    with_state(|s| *s = State::default());
}

/// Forget every runtime setting (tests).
#[cfg(test)]
pub(crate) fn reset_state() {
    with_state(|s| *s = State::default());
}

fn ini_str(ctx: &Ctx, name: &str) -> String {
    ctx.ini_get(name).unwrap_or("").to_string()
}

/// `php_get_internal_encoding()` → `mbfl_name2encoding`, falling back to
/// UTF-8 for an empty or unknown name.
fn internal_encoding(ctx: &Ctx) -> &'static Encoding {
    if let Some(id) = with_state(|s| s.internal) {
        return mbfl::by_id(id);
    }
    let mut name = ini_str(ctx, "mbstring.internal_encoding");
    if name.is_empty() {
        name = ini_str(ctx, "internal_encoding");
    }
    if name.is_empty() {
        name = ini_str(ctx, "default_charset");
    }
    mbfl::name2encoding(name.as_bytes()).unwrap_or(&mbfl::UTF8)
}

/// The current HTTP output encoding (`mbstring.http_output` →
/// `output_encoding` → `default_charset`).
fn http_output_encoding(ctx: &Ctx) -> &'static Encoding {
    if let Some(id) = with_state(|s| s.http_output) {
        return mbfl::by_id(id);
    }
    let mut name = ini_str(ctx, "mbstring.http_output");
    if name.is_empty() {
        name = ini_str(ctx, "output_encoding");
    }
    if name.is_empty() {
        name = ini_str(ctx, "default_charset");
    }
    if name == "pass" {
        return &mbfl::PASS;
    }
    mbfl::name2encoding(name.as_bytes()).unwrap_or(&mbfl::UTF8)
}

/// The HTTP input encoding list (`mbstring.http_input` → `input_encoding`
/// → `default_charset`).
fn http_input_list(ctx: &Ctx) -> Vec<&'static Encoding> {
    let name = ini_str(ctx, "mbstring.http_input");
    if name == "pass" {
        return vec![&mbfl::PASS];
    }
    if !name.is_empty() {
        if let Ok(list) = parse_encoding_list(ctx, name.as_bytes()) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    let mut name = ini_str(ctx, "input_encoding");
    if name.is_empty() {
        name = ini_str(ctx, "default_charset");
    }
    match mbfl::name2encoding(name.as_bytes()) {
        Some(e) => vec![e],
        None => vec![&mbfl::UTF8],
    }
}

/// `OnUpdate_mbstring_substitute_character`: the mode and character from
/// the ini value, unless `mb_substitute_character()` overrode them.
fn substitute(ctx: &Ctx) -> (ErrorMode, u32) {
    if let Some(s) = with_state(|s| s.subst) {
        return s;
    }
    let v = ini_str(ctx, "mbstring.substitute_character");
    if v.eq_ignore_ascii_case("none") {
        (ErrorMode::None, b'?' as u32)
    } else if v.eq_ignore_ascii_case("long") {
        (ErrorMode::Long, b'?' as u32)
    } else if v.eq_ignore_ascii_case("entity") {
        (ErrorMode::Entity, b'?' as u32)
    } else {
        (ErrorMode::Char, strtol0(&v).unwrap_or(b'?' as i64) as u32)
    }
}

/// C `strtol(s, &end, 0)` requiring the whole string to be consumed.
fn strtol0(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if t.len() > 1 && t.starts_with('0') {
        i64::from_str_radix(&t[1..], 8).ok()?
    } else if t.is_empty() {
        return None;
    } else {
        t.parse::<i64>().ok()?
    };
    Some(if neg { -v } else { v })
}

fn language(ctx: &Ctx) -> &'static Language {
    let name = ini_str(ctx, "mbstring.language");
    mbfl::name2language(name.as_bytes()).unwrap_or_else(|| mbfl::name2language(b"neutral").unwrap())
}

/// `MBSTRG(default_detect_order_list)`: the language's default list.
fn default_detect_order(ctx: &Ctx) -> Vec<&'static Encoding> {
    language(ctx).detect_order.iter().map(|&id| mbfl::by_id(id)).collect()
}

/// `MBSTRG(current_detect_order_list)`: `mb_detect_order()`'s list, else
/// the snapshot of `mbstring.detect_order` / the language default taken
/// the first time it is needed (php populates it at request start).
fn current_detect_order(ctx: &Ctx) -> Vec<&'static Encoding> {
    if let Some(ids) = with_state(|s| s.detect_order.clone()) {
        return ids.into_iter().map(mbfl::by_id).collect();
    }
    let ini = ini_str(ctx, "mbstring.detect_order");
    let list = match parse_encoding_list(ctx, ini.as_bytes()) {
        Ok(l) if !l.is_empty() => l,
        _ => default_detect_order(ctx),
    };
    let ids: Vec<Id> = list.iter().map(|e| e.id).collect();
    with_state(|s| s.detect_order = Some(ids));
    list
}

fn add_illegal(n: u32) {
    with_state(|s| s.illegal_chars += n as i64);
}

// ---- argument helpers -------------------------------------------------------------------

fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

/// A `string` parameter: arrays and objects are a `TypeError`.
fn str_arg(v: &Value, func: &str, n: usize, name: &str) -> Result<Vec<u8>, Unwind> {
    match v.deref().as_ref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type string, {} given",
            rphp_runtime::value_name(&v)
        ))),
        _ => Ok(bytes(v)),
    }
}

/// A `?string` parameter: `None` for absent / `null`.
fn opt_str_arg(args: &[Value], i: usize, func: &str, name: &str) -> Result<Option<Vec<u8>>, Unwind> {
    match args.get(i) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => str_arg(v, func, i + 1, name).map(Some),
    }
}

fn opt_int_arg(args: &[Value], i: usize) -> Option<i64> {
    match args.get(i) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_int()),
    }
}

fn invalid_encoding(func: &str, n: usize, param: &str, name: &[u8]) -> Unwind {
    Unwind::value_error(format!(
        "{func}(): Argument #{n} (${param}) must be a valid encoding, \"{}\" given",
        String::from_utf8_lossy(name)
    ))
}

/// `php_mb_get_encoding`: the named encoding (with php's deprecation for
/// the byte codecs), or the internal encoding for `null`.
fn get_encoding(ctx: &mut Ctx, name: Option<&[u8]>, func: &str, n: usize, param: &str) -> Result<&'static Encoding, Unwind> {
    let Some(name) = name else {
        return Ok(internal_encoding(ctx));
    };
    if with_state(|s| s.last_used.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(name))) {
        return Ok(mbfl::name2encoding(name).unwrap_or(&mbfl::UTF8));
    }
    let Some(enc) = mbfl::name2encoding(name) else {
        return Err(invalid_encoding(func, n, param, name));
    };
    with_state(|s| s.last_used = Some(name.to_vec()));
    if enc.id <= Id::QPrint {
        let msg = match enc.id {
            Id::Base64 => "Handling Base64 via mbstring is deprecated; use base64_encode/base64_decode instead",
            Id::QPrint => "Handling QPrint via mbstring is deprecated; use quoted_printable_encode/quoted_printable_decode instead",
            Id::HtmlEnt => "Handling HTML entities via mbstring is deprecated; use htmlspecialchars, htmlentities, or mb_encode_numericentity/mb_decode_numericentity instead",
            Id::Uuencode => "Handling Uuencode via mbstring is deprecated; use convert_uuencode/convert_uudecode instead",
            _ => "",
        };
        if !msg.is_empty() {
            ctx.deprecated(&format!("{func}(): {msg}"))?;
        }
    }
    Ok(enc)
}

/// The `?string $encoding` argument at `i` (parameter name `encoding`).
fn enc_arg(ctx: &mut Ctx, args: &[Value], i: usize, func: &str) -> Result<&'static Encoding, Unwind> {
    let name = opt_str_arg(args, i, func, "encoding")?;
    get_encoding(ctx, name.as_deref(), func, i + 1, "encoding")
}

/// `php_mb_parse_encoding_list`: a comma-separated list (`auto` expands to
/// the language default). `Err` carries the offending name.
fn parse_encoding_list(ctx: &Ctx, value: &[u8]) -> Result<Vec<&'static Encoding>, Vec<u8>> {
    let mut out = Vec::new();
    if value.is_empty() {
        return Ok(out);
    }
    let value = if value.len() > 2 && value[0] == b'"' && value[value.len() - 1] == b'"' { &value[1..value.len() - 1] } else { value };
    let mut included_auto = false;
    for item in value.split(|&b| b == b',') {
        let item = trim_sp(item);
        if item.eq_ignore_ascii_case(b"auto") {
            if !included_auto {
                included_auto = true;
                out.extend(default_detect_order(ctx));
            }
        } else if let Some(e) = mbfl::name2encoding(item) {
            out.push(e);
        } else {
            return Err(item.to_vec());
        }
    }
    Ok(out)
}

fn trim_sp(s: &[u8]) -> &[u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && (s[a] == b' ' || s[a] == b'\t') {
        a += 1;
    }
    while b > a && (s[b - 1] == b' ' || s[b - 1] == b'\t') {
        b -= 1;
    }
    &s[a..b]
}

/// `php_mb_parse_encoding_array`.
fn parse_encoding_array(ctx: &Ctx, arr: &Array) -> Result<Vec<&'static Encoding>, Vec<u8>> {
    let mut out = Vec::new();
    let mut included_auto = false;
    for (_, v) in arr.iter() {
        let name = bytes(v);
        if name.eq_ignore_ascii_case(b"auto") {
            if !included_auto {
                included_auto = true;
                out.extend(default_detect_order(ctx));
            }
        } else if let Some(e) = mbfl::name2encoding(&name) {
            out.push(e);
        } else {
            return Err(name);
        }
    }
    Ok(out)
}

/// An `array|string|null` encoding-list argument; `None` for absent/null.
fn encoding_list_arg(ctx: &Ctx, args: &[Value], i: usize, func: &str, param: &str) -> Result<Option<Vec<&'static Encoding>>, Unwind> {
    let r = match args.get(i) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => parse_encoding_array(ctx, a),
        Some(v) => parse_encoding_list(ctx, &bytes(v)),
    };
    match r {
        Ok(l) => Ok(Some(l)),
        Err(bad) => Err(Unwind::value_error(format!(
            "{func}(): Argument #{} (${param}) contains invalid encoding \"{}\"",
            i + 1,
            String::from_utf8_lossy(&bad)
        ))),
    }
}

/// Whether an argument array is (a copy of) `mb_list_encodings()`: then the
/// list order is not significant for detection.
fn is_full_list(v: &Value) -> bool {
    match v {
        Value::Array(a) => {
            a.len() == mbfl::ENCODINGS.len() && a.values().zip(mbfl::ENCODINGS).all(|(v, e)| bytes(v) == e.name.as_bytes())
        }
        _ => false,
    }
}

/// `remove_non_encodings_from_elist`.
fn remove_non_encodings(list: &mut Vec<&'static Encoding>) {
    list.retain(|e| !e.is_byte_encoding());
}

/// `php_mb_convert_encoding_ex`: convert with the request's substitute
/// settings, counting illegal characters.
fn convert_ex(ctx: &Ctx, input: &[u8], to: &Encoding, from: &Encoding) -> Vec<u8> {
    let (mode, ch) = substitute(ctx);
    let (out, errs) = mbfl::convert(input, from, to, ch, mode);
    add_illegal(errs);
    out
}

/// `php_mb_convert_encoding`: one source encoding, or detection among
/// several (`None` + warning when nothing matches).
fn convert_detect(ctx: &mut Ctx, input: &[u8], to: &Encoding, from: &[&'static Encoding]) -> Result<Option<Vec<u8>>, Unwind> {
    let from_enc = if from.len() == 1 {
        from[0]
    } else {
        let strict = ctx.ini.bool("mbstring.strict_detection");
        match detect::guess(&[input], from, strict, true) {
            Some(e) => e,
            None => {
                ctx.warn("mb_convert_encoding(): Unable to detect character encoding")?;
                return Ok(None);
            }
        }
    };
    Ok(Some(convert_ex(ctx, input, to, from_enc)))
}

// ---- settings functions ---------------------------------------------------------------

/// `mb_language(?string $language = null): string|bool`
fn mb_language(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match opt_str_arg(args, 0, "mb_language", "language")? {
        None => Ok(str_value(language(ctx).name.as_bytes().to_vec())),
        Some(name) => match mbfl::name2language(&name) {
            Some(_) => {
                ctx.ini_set("mbstring.language", &String::from_utf8_lossy(&name));
                Ok(Value::Bool(true))
            }
            None => Err(Unwind::value_error(format!(
                "mb_language(): Argument #1 ($language) must be a valid language, \"{}\" given",
                String::from_utf8_lossy(&name)
            ))),
        },
    }
}

/// `mb_internal_encoding(?string $encoding = null): string|bool`
fn mb_internal_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match opt_str_arg(args, 0, "mb_internal_encoding", "encoding")? {
        None => Ok(str_value(internal_encoding(ctx).name.as_bytes().to_vec())),
        Some(name) => match mbfl::name2encoding(&name) {
            Some(e) => {
                with_state(|s| s.internal = Some(e.id));
                Ok(Value::Bool(true))
            }
            None => Err(invalid_encoding("mb_internal_encoding", 1, "encoding", &name)),
        },
    }
}

/// `mb_http_input(?string $type = null): array|string|false`
fn mb_http_input(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let err = || Unwind::value_error("mb_http_input(): Argument #1 ($type) must be one of \"G\", \"P\", \"C\", \"S\", \"I\", or \"L\"");
    let ty = opt_str_arg(args, 0, "mb_http_input", "type")?;
    let identified = with_state(|s| s.http_input_identify);
    let result = match ty.as_deref() {
        None => identified,
        Some([c]) => match c.to_ascii_uppercase() {
            b'G' | b'P' | b'C' | b'S' => None,
            b'I' => {
                let mut arr = Array::new();
                for e in http_input_list(ctx) {
                    arr.push(str_value(e.name.as_bytes().to_vec()));
                }
                return Ok(Value::Array(arr));
            }
            b'L' => {
                let names: Vec<&str> = http_input_list(ctx).iter().map(|e| e.name).collect();
                if names.is_empty() {
                    return Ok(Value::Bool(false));
                }
                return Ok(str_value(names.join(",").into_bytes()));
            }
            _ => return Err(err()),
        },
        Some(_) => return Err(err()),
    };
    Ok(match result {
        Some(id) => str_value(mbfl::by_id(id).name.as_bytes().to_vec()),
        None => Value::Bool(false),
    })
}

/// `mb_http_output(?string $encoding = null): string|bool`
fn mb_http_output(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match opt_str_arg(args, 0, "mb_http_output", "encoding")? {
        None => Ok(str_value(http_output_encoding(ctx).name.as_bytes().to_vec())),
        Some(name) => {
            let enc = if name == b"pass" { Some(&mbfl::PASS) } else { mbfl::name2encoding(&name) };
            match enc {
                Some(e) => {
                    with_state(|s| s.http_output = Some(e.id));
                    Ok(Value::Bool(true))
                }
                None => Err(invalid_encoding("mb_http_output", 1, "encoding", &name)),
            }
        }
    }
}

fn names_array(list: &[&Encoding]) -> Value {
    let mut arr = Array::new();
    for e in list {
        arr.push(str_value(e.name.as_bytes().to_vec()));
    }
    Value::Array(arr)
}

/// `mb_detect_order(array|string|null $encoding = null): array|bool`
fn mb_detect_order(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match encoding_list_arg(ctx, args, 0, "mb_detect_order", "encoding")? {
        None => Ok(names_array(&current_detect_order(ctx))),
        Some(list) => {
            if list.is_empty() {
                return Err(Unwind::value_error("mb_detect_order(): Argument #1 ($encoding) must specify at least one encoding"));
            }
            let ids: Vec<Id> = list.iter().map(|e| e.id).collect();
            with_state(|s| s.detect_order = Some(ids));
            Ok(Value::Bool(true))
        }
    }
}

/// `php_mb_check_code_point`.
fn valid_code_point(cp: i64) -> bool {
    (0..0x110000).contains(&cp) && !(0xD800..=0xDFFF).contains(&cp)
}

fn subst_value(mode: ErrorMode, ch: u32) -> Value {
    match mode {
        ErrorMode::None => Value::string(b"none"),
        ErrorMode::Long => Value::string(b"long"),
        ErrorMode::Entity => Value::string(b"entity"),
        _ => Value::Int(ch as i64),
    }
}

/// `mb_substitute_character(string|int|null $substitute_character = null): string|int|bool`
fn mb_substitute_character(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => {
            let (mode, ch) = substitute(ctx);
            Ok(subst_value(mode, ch))
        }
        Some(Value::Str(s)) => {
            let mode = if s.as_bytes().eq_ignore_ascii_case(b"none") {
                ErrorMode::None
            } else if s.as_bytes().eq_ignore_ascii_case(b"long") {
                ErrorMode::Long
            } else if s.as_bytes().eq_ignore_ascii_case(b"entity") {
                ErrorMode::Entity
            } else {
                return Err(Unwind::value_error(
                    "mb_substitute_character(): Argument #1 ($substitute_character) must be \"none\", \"long\", \"entity\" or a valid codepoint",
                ));
            };
            let ch = substitute(ctx).1;
            with_state(|s| s.subst = Some((mode, ch)));
            Ok(Value::Bool(true))
        }
        Some(v) => {
            let cp = v.to_int();
            if !valid_code_point(cp) {
                return Err(Unwind::value_error("mb_substitute_character(): Argument #1 ($substitute_character) is not a valid codepoint"));
            }
            with_state(|s| s.subst = Some((ErrorMode::Char, cp as u32)));
            Ok(Value::Bool(true))
        }
    }
}

/// `mb_preferred_mime_name(string $encoding): string|false`
fn mb_preferred_mime_name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = str_arg(&args[0], "mb_preferred_mime_name", 1, "encoding")?;
    let Some(enc) = mbfl::name2encoding(&name) else {
        return Err(invalid_encoding("mb_preferred_mime_name", 1, "encoding", &name));
    };
    match mbfl::preferred_mime_name(enc) {
        Some(m) => Ok(str_value(m.as_bytes().to_vec())),
        None => {
            ctx.warn(&format!("mb_preferred_mime_name(): No MIME preferred name corresponding to \"{}\"", String::from_utf8_lossy(&name)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `mb_parse_str(string $string, array &$result): bool` — `parse_str` plus
/// conversion of every key and value from the detected HTTP input encoding
/// to the internal encoding.
fn mb_parse_str(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let input = str_arg(&args[0], "mb_parse_str", 1, "string")?;
    let mut tmp = [args[0].clone(), Value::Null];
    crate::url::parse_str(ctx, &mut tmp)?;
    let Value::Array(parsed) = tmp[1].clone().unref() else {
        Value::assign(&mut args[1], Value::empty_array());
        return Ok(Value::Bool(false));
    };
    let list = http_input_list(ctx);
    let to = internal_encoding(ctx);
    let strict = ctx.ini.bool("mbstring.strict_detection");
    let from = if list.len() == 1 { Some(list[0]) } else { detect::guess(&[&input], &list, strict, true) };
    let Some(from) = from else {
        Value::assign(&mut args[1], Value::Array(parsed));
        return Ok(Value::Bool(false));
    };
    let converted = if from.id == Id::Pass { Value::Array(parsed) } else { convert_value_recursive(ctx, &Value::Array(parsed), to, from) };
    with_state(|s| s.http_input_identify = Some(from.id));
    Value::assign(&mut args[1], converted);
    Ok(Value::Bool(true))
}

/// Convert every string (and string key) inside `v` from `from` to `to`.
fn convert_value_recursive(ctx: &Ctx, v: &Value, to: &Encoding, from: &Encoding) -> Value {
    match v.deref().as_ref() {
        Value::Str(s) => str_value(convert_ex(ctx, s.as_bytes(), to, from)),
        Value::Array(a) => {
            let mut out = Array::new();
            for (k, item) in a.iter() {
                let key = match k {
                    ArrayKey::Str(s) => ArrayKey::str(&convert_ex(ctx, s, to, from)),
                    other => other.clone(),
                };
                out.set(key, convert_value_recursive(ctx, item, to, from));
            }
            Value::Array(out)
        }
        other => other.clone(),
    }
}

/// `mb_output_handler(string $string, int $status): string` — converts
/// from the internal encoding to the HTTP output encoding (the
/// `Content-Type` header php would add is a SAPI concern; the CLI has no
/// headers).
fn mb_output_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_output_handler", 1, "string")?;
    let status = args[1].to_int();
    let enc = http_output_encoding(ctx);
    if enc.id == Id::Pass {
        return Ok(str_value(s));
    }
    if status & 1 != 0 {
        with_state(|st| st.outconv_enabled = true);
    }
    if !with_state(|st| st.outconv_enabled) {
        return Ok(str_value(s));
    }
    let from = internal_encoding(ctx);
    let out = convert_ex(ctx, &s, enc, from);
    if status & 8 != 0 {
        with_state(|st| st.outconv_enabled = false);
    }
    Ok(str_value(out))
}

// ---- length / search / substring ------------------------------------------------------

/// `mb_get_strlen`.
fn strlen_of(s: &[u8], enc: &Encoding) -> usize {
    if let Some(w) = enc.fixed_width() {
        return s.len() / w;
    }
    if enc.is_utf8() && mbfl::unicode::check_utf8(s) {
        return utf8_len(s);
    }
    enc.decode(s).len()
}

/// `mb_strlen(string $string, ?string $encoding = null): int`
fn mb_strlen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_strlen", 1, "string")?;
    let enc = enc_arg(ctx, args, 1, "mb_strlen")?;
    Ok(Value::Int(strlen_of(&s, enc) as i64))
}

/// `mb_fast_strlen_utf8`: non-continuation bytes.
fn utf8_len(s: &[u8]) -> usize {
    s.iter().filter(|&&c| c < 0x80 || c & 0xC0 != 0x80).count()
}

/// `offset_to_pointer_utf8`: the byte index `offset` characters into `s`
/// (from the end for negative offsets), `None` when out of range.
fn utf8_offset(s: &[u8], offset: i64) -> Option<usize> {
    if offset < 0 {
        let mut pos = s.len();
        let mut offset = offset;
        while offset < 0 {
            if pos == 0 {
                return None;
            }
            pos -= 1;
            let c = s[pos];
            if c < 0x80 || c & 0xC0 != 0x80 {
                offset += 1;
            }
        }
        Some(pos)
    } else {
        let mut pos = 0usize;
        let mut offset = offset;
        while offset > 0 {
            if pos >= s.len() {
                return None;
            }
            pos += mbfl::unicode::MBLEN_UTF8[s[pos] as usize] as usize;
            offset -= 1;
        }
        Some(pos.min(s.len()))
    }
}

fn to_utf8_for_search(s: &[u8], enc: &Encoding) -> Vec<u8> {
    if enc.is_utf8() {
        s.to_vec()
    } else {
        mbfl::convert(s, enc, &mbfl::UTF8, 0, ErrorMode::BadUtf8).0
    }
}

fn find_bytes(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() {
        return Some(0);
    }
    if n.len() > h.len() {
        return None;
    }
    h.windows(n.len()).position(|w| w == n)
}

fn rfind_bytes(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() {
        return Some(h.len());
    }
    if n.len() > h.len() {
        return None;
    }
    h.windows(n.len()).rposition(|w| w == n)
}

/// `mb_find_strpos` result.
enum Found {
    At(usize),
    NotFound,
    BadOffset,
}

/// `mb_find_strpos`: search in UTF-8 space, returning a character index.
fn find_strpos(haystack: &[u8], needle: &[u8], enc: &Encoding, offset: i64, reverse: bool) -> Found {
    let h = to_utf8_for_search(haystack, enc);
    let n = to_utf8_for_search(needle, enc);
    let Some(off) = utf8_offset(&h, offset) else {
        return Found::BadOffset;
    };
    if h.len() < n.len() {
        return Found::NotFound;
    }
    let found = if !reverse {
        find_bytes(&h[off..], &n).map(|p| p + off)
    } else if offset >= 0 {
        rfind_bytes(&h[off..], &n).map(|p| p + off)
    } else {
        let needle_len = utf8_len(needle);
        let end = utf8_offset(&h[off..], needle_len as i64).map(|p| p + off).unwrap_or(h.len());
        rfind_bytes(&h[..end], &n)
    };
    match found {
        Some(p) => Found::At(utf8_len(&h[..p])),
        None => Found::NotFound,
    }
}

/// `php_mb_stripos`: simple case-fold both sides to UTF-8, then search.
fn find_stripos(haystack: &[u8], needle: &[u8], enc: &Encoding, offset: i64, reverse: bool) -> Found {
    let fold = |s: &[u8]| {
        let mut buf = ConvertBuf::new(0, ErrorMode::BadUtf8);
        convert_case(CaseMode::FoldSimple, s, enc, &mbfl::UTF8, &mut buf);
        buf.out
    };
    find_strpos(&fold(haystack), &fold(needle), &mbfl::UTF8, offset, reverse)
}

fn strpos_common(ctx: &mut Ctx, args: &mut [Value], func: &str, reverse: bool, ci: bool) -> NativeResult {
    let h = str_arg(&args[0], func, 1, "haystack")?;
    let n = str_arg(&args[1], func, 2, "needle")?;
    let offset = opt_int_arg(args, 2).unwrap_or(0);
    let enc = enc_arg(ctx, args, 3, func)?;
    let r = if ci { find_stripos(&h, &n, enc, offset, reverse) } else { find_strpos(&h, &n, enc, offset, reverse) };
    match r {
        Found::At(p) => Ok(Value::Int(p as i64)),
        Found::NotFound => Ok(Value::Bool(false)),
        Found::BadOffset => Err(Unwind::value_error(format!("{func}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"))),
    }
}

/// `mb_strpos(string $haystack, string $needle, int $offset = 0, ?string $encoding = null): int|false`
fn mb_strpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_common(ctx, args, "mb_strpos", false, false)
}

/// `mb_strrpos(...)`
fn mb_strrpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_common(ctx, args, "mb_strrpos", true, false)
}

/// `mb_stripos(...)`
fn mb_stripos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_common(ctx, args, "mb_stripos", false, true)
}

/// `mb_strripos(...)`
fn mb_strripos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_common(ctx, args, "mb_strripos", true, true)
}

const UNTIL_END: usize = usize::MAX;

/// `mb_get_substr_slow`: decode, slice, re-encode with substitution.
fn substr_slow(ctx: &Ctx, s: &[u8], from: usize, len: usize, enc: &Encoding) -> Vec<u8> {
    let w = enc.decode(s);
    if from >= w.len() {
        return Vec::new();
    }
    let end = if len == UNTIL_END { w.len() } else { from.saturating_add(len).min(w.len()) };
    let (mode, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, mode);
    enc.encode(&w[from..end], &mut buf, true);
    buf.out
}

/// `mb_get_substr`.
fn get_substr(ctx: &Ctx, s: &[u8], from: usize, len: usize, enc: &Encoding) -> Vec<u8> {
    if len == 0 || (from >= s.len() && enc.id != Id::SjisMac) {
        return Vec::new();
    }
    if let Some(w) = enc.fixed_width() {
        let from = from.saturating_mul(w);
        if from >= s.len() {
            return Vec::new();
        }
        let len = if len == UNTIL_END { s.len() - from } else { len.saturating_mul(w).min(s.len() - from) };
        return s[from..from + len].to_vec();
    }
    substr_slow(ctx, s, from, len, enc)
}

fn strstr_common(ctx: &mut Ctx, args: &mut [Value], func: &str, reverse: bool, ci: bool) -> NativeResult {
    let h = str_arg(&args[0], func, 1, "haystack")?;
    let n = str_arg(&args[1], func, 2, "needle")?;
    let part = args.get(2).is_some_and(Value::to_bool);
    let enc = enc_arg(ctx, args, 3, func)?;
    let r = if ci { find_stripos(&h, &n, enc, 0, reverse) } else { find_strpos(&h, &n, enc, 0, reverse) };
    match r {
        Found::At(p) => Ok(str_value(if part { get_substr(ctx, &h, 0, p, enc) } else { get_substr(ctx, &h, p, UNTIL_END, enc) })),
        _ => Ok(Value::Bool(false)),
    }
}

/// `mb_strstr(string $haystack, string $needle, bool $before_needle = false, ?string $encoding = null): string|false`
fn mb_strstr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strstr_common(ctx, args, "mb_strstr", false, false)
}

/// `mb_strrchr(...)`
fn mb_strrchr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strstr_common(ctx, args, "mb_strrchr", true, false)
}

/// `mb_stristr(...)`
fn mb_stristr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strstr_common(ctx, args, "mb_stristr", false, true)
}

/// `mb_strrichr(...)`
fn mb_strrichr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strstr_common(ctx, args, "mb_strrichr", true, true)
}

/// `mb_substr_count(string $haystack, string $needle, ?string $encoding = null): int`
fn mb_substr_count(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let h = str_arg(&args[0], "mb_substr_count", 1, "haystack")?;
    let n = str_arg(&args[1], "mb_substr_count", 2, "needle")?;
    if n.is_empty() {
        return Err(Unwind::value_error("mb_substr_count(): Argument #2 ($needle) must not be empty"));
    }
    let enc = enc_arg(ctx, args, 2, "mb_substr_count")?;
    let (h8, n8) = if enc.is_utf8() {
        let conv = |s: &[u8]| if mbfl::unicode::check_utf8(s) { s.to_vec() } else { to_utf8_for_search(s, enc) };
        (conv(&h), conv(&n))
    } else {
        let h8 = to_utf8_for_search(&h, enc);
        let n8 = to_utf8_for_search(&n, enc);
        if n8.is_empty() {
            return Err(Unwind::value_error("mb_substr_count(): Argument #2 ($needle) must not be empty"));
        }
        (h8, n8)
    };
    let mut count = 0i64;
    if h8.len() >= n8.len() {
        let mut p = 0;
        while let Some(i) = find_bytes(&h8[p..], &n8) {
            p += i + n8.len();
            count += 1;
        }
    }
    Ok(Value::Int(count))
}

/// `mb_substr(string $string, int $start, ?int $length = null, ?string $encoding = null): string`
fn mb_substr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_substr", 1, "string")?;
    let from = args[1].to_int();
    let len = opt_int_arg(args, 2);
    if from == i64::MIN {
        return Err(Unwind::value_error(format!("mb_substr(): Argument #2 ($start) must be between {} and {}", i64::MIN + 1, i64::MAX)));
    }
    if len == Some(i64::MIN) {
        return Err(Unwind::value_error(format!("mb_substr(): Argument #3 ($length) must be between {} and {}", i64::MIN + 1, i64::MAX)));
    }
    let enc = enc_arg(ctx, args, 3, "mb_substr")?;
    let mblen = if from < 0 || len.is_some_and(|l| l < 0) { strlen_of(&s, enc) as i64 } else { 0 };
    let real_from = if from >= 0 {
        from as usize
    } else if -from < mblen {
        (mblen + from) as usize
    } else {
        0
    };
    let real_len = match len {
        None => UNTIL_END,
        Some(l) if l >= 0 => l as usize,
        Some(l) if (real_from as i64) < mblen && -l < mblen - real_from as i64 => (mblen - real_from as i64 + l) as usize,
        Some(_) => 0,
    };
    Ok(str_value(get_substr(ctx, &s, real_from, real_len, enc)))
}

/// `mb_strcut(string $string, int $start, ?int $length = null, ?string $encoding = null): string`
fn mb_strcut(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_strcut", 1, "string")?;
    let mut from = args[1].to_int();
    let mut len = opt_int_arg(args, 2).unwrap_or(s.len() as i64);
    let enc = enc_arg(ctx, args, 3, "mb_strcut")?;
    let n = s.len() as i64;
    if from < 0 {
        from = (n + from).max(0);
    }
    if len < 0 {
        len = (n - from + len).max(0);
    }
    if from > n || len == 0 {
        return Ok(Value::string(b""));
    }
    let (from, len) = (from as usize, len as usize);
    if let Some(cut) = enc.cut(&s, from, len) {
        return Ok(str_value(cut));
    }
    if let Some(w) = enc.fixed_width() {
        let from = from & !(w - 1);
        let len = len.min(s.len() - from) & !(w - 1);
        return Ok(str_value(s[from..from + len].to_vec()));
    }
    if let Some(tab) = enc.mblen_table {
        let mut p = 0usize;
        let mut m = 0usize;
        while p < from {
            m = tab[s[p] as usize] as usize;
            p += m;
        }
        if p > from {
            p -= m;
        }
        let start = p;
        let end = if len >= s.len() - start {
            s.len()
        } else {
            let q = p + len;
            while p < q {
                m = tab[s[p] as usize] as usize;
                p += m;
            }
            if p > q {
                p -= m;
            }
            p
        };
        return Ok(str_value(s[start..end.min(s.len())].to_vec()));
    }
    // Stateful encodings (`mbfl_strcut`): keep the characters whose bytes
    // lie inside [from, from + len) and re-encode them.
    let w = enc.decode(&s[..]);
    let _ = w;
    let (mode, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, mode);
    let wchars = enc.decode(&s[from.min(s.len())..(from + len).min(s.len())]);
    enc.encode(&wchars, &mut buf, true);
    Ok(str_value(buf.out))
}

/// `character_width`: 2 for East Asian wide/fullwidth, else 1.
fn char_width(c: u32) -> usize {
    if c < FIRST_DOUBLEWIDTH {
        return 1;
    }
    let i = EAW.partition_point(|&(_, hi)| hi < c);
    if EAW.get(i).is_some_and(|&(lo, hi)| lo <= c && c <= hi) {
        2
    } else {
        1
    }
}

fn strwidth_of(s: &[u8], enc: &Encoding) -> usize {
    enc.decode(s).iter().map(|&w| char_width(w)).sum()
}

/// `mb_strwidth(string $string, ?string $encoding = null): int`
fn mb_strwidth(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_strwidth", 1, "string")?;
    let enc = enc_arg(ctx, args, 1, "mb_strwidth")?;
    Ok(Value::Int(strwidth_of(&s, enc) as i64))
}

/// `mb_trim_string` (the `mb_strimwidth` core).
fn trim_width(ctx: &Ctx, input: &[u8], marker: &[u8], enc: &Encoding, from: usize, width: usize) -> Vec<u8> {
    let w = enc.decode(input);
    let mut remaining = width;
    let mut input_err = false;
    let mut cut_at: Option<usize> = None;
    for (i, &c) in w.iter().enumerate().skip(from) {
        let cw = char_width(c);
        input_err |= c == BAD_INPUT;
        if remaining < cw {
            cut_at = Some(i);
            break;
        }
        remaining -= cw;
    }
    let Some(_) = cut_at else {
        // Fits: unchanged (from 0), or the tail; erroneous bytes are
        // converted to error markers.
        if !input_err {
            if from == 0 {
                return input.to_vec();
            }
            return get_substr(ctx, input, from, UNTIL_END, enc);
        }
        return substr_slow(ctx, input, from, UNTIL_END, enc);
    };
    let marker_width = strwidth_of(marker, enc);
    if width <= marker_width {
        return marker.to_vec();
    }
    let mut width = width - marker_width;
    let (mode, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, mode);
    let mut end = from;
    for (i, &c) in w.iter().enumerate().skip(from) {
        let cw = char_width(c);
        if width < cw {
            break;
        }
        width -= cw;
        end = i + 1;
    }
    enc.encode(&w[from..end], &mut buf, true);
    buf.out.extend_from_slice(marker);
    buf.out
}

/// `mb_strimwidth(string $string, int $start, int $width, string $trim_marker = "", ?string $encoding = null): string`
fn mb_strimwidth(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_strimwidth", 1, "string")?;
    let mut from = args[1].to_int();
    let mut width = args[2].to_int();
    let marker = match args.get(3) {
        Some(v) => str_arg(v, "mb_strimwidth", 4, "trim_marker")?,
        None => Vec::new(),
    };
    let enc = enc_arg(ctx, args, 4, "mb_strimwidth")?;
    if from != 0 {
        let len = strlen_of(&s, enc) as i64;
        if from < 0 {
            from += len;
        }
        if from < 0 || from > len {
            return Err(Unwind::value_error("mb_strimwidth(): Argument #2 ($start) is out of range"));
        }
    }
    if width < 0 {
        ctx.deprecated("mb_strimwidth(): passing a negative integer to argument #3 ($width) is deprecated")?;
        width += strwidth_of(&s, enc) as i64;
        if from > 0 {
            let trimmed = get_substr(ctx, &s, 0, from as usize, enc);
            width -= strwidth_of(&trimmed, enc) as i64;
        }
        if width < 0 {
            return Err(Unwind::value_error("mb_strimwidth(): Argument #3 ($width) is out of range"));
        }
    }
    Ok(str_value(trim_width(ctx, &s, &marker, enc, from as usize, width as usize)))
}

// ---- conversion ------------------------------------------------------------------------------

/// `mb_convert_encoding(array|string $string, string $to_encoding, array|string|null $from_encoding = null): array|string|false`
fn mb_convert_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let to_name = str_arg(&args[1], "mb_convert_encoding", 2, "to_encoding")?;
    let to = get_encoding(ctx, Some(&to_name), "mb_convert_encoding", 2, "to_encoding")?;
    let mut from = match encoding_list_arg(ctx, args, 2, "mb_convert_encoding", "from_encoding")? {
        Some(l) => l,
        None => vec![internal_encoding(ctx)],
    };
    if from.len() > 1 {
        remove_non_encodings(&mut from);
    }
    if from.is_empty() {
        return Err(Unwind::value_error("mb_convert_encoding(): Argument #3 ($from_encoding) must specify at least one encoding"));
    }
    match args[0].deref().as_ref() {
        Value::Array(a) => {
            let a = a.clone();
            Ok(convert_array_recursive(ctx, &a, to, &from)?.map_or(Value::empty_array(), Value::Array))
        }
        Value::Object(_) | Value::Closure(_) => Err(Unwind::type_error(format!(
            "mb_convert_encoding(): Argument #1 ($string) must be of type array|string, {} given",
            rphp_runtime::value_name(&args[0])
        ))),
        v => match convert_detect(ctx, &bytes(v), to, &from)? {
            Some(out) => Ok(str_value(out)),
            None => Ok(Value::Bool(false)),
        },
    }
}

/// `php_mb_convert_encoding_recursive`.
fn convert_array_recursive(ctx: &mut Ctx, input: &Array, to: &Encoding, from: &[&'static Encoding]) -> Result<Option<Array>, Unwind> {
    let mut out = Array::new();
    for (k, v) in input.iter() {
        let key = match k {
            ArrayKey::Str(s) => match convert_detect(ctx, s, to, from)? {
                Some(c) => ArrayKey::str(&c),
                None => continue,
            },
            other => other.clone(),
        };
        let item = match v.deref().as_ref() {
            Value::Str(s) => match convert_detect(ctx, s.as_bytes(), to, from)? {
                Some(c) => str_value(c),
                None => continue,
            },
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => v.deref().into_owned(),
            Value::Array(a) => match convert_array_recursive(ctx, a, to, from)? {
                Some(c) => Value::Array(c),
                None => Value::empty_array(),
            },
            _ => {
                ctx.warn("mb_convert_encoding(): Object is not supported")?;
                continue;
            }
        };
        out.set(key, item);
    }
    Ok(Some(out))
}

fn case_common(ctx: &mut Ctx, args: &mut [Value], func: &str, mode: CaseMode, enc_idx: usize) -> NativeResult {
    let s = str_arg(&args[0], func, 1, "string")?;
    let enc = enc_arg(ctx, args, enc_idx, func)?;
    let (m, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, m);
    convert_case(mode, &s, enc, enc, &mut buf);
    add_illegal(buf.errors);
    Ok(str_value(buf.out))
}

/// `mb_convert_case(string $string, int $mode, ?string $encoding = null): string`
fn mb_convert_case(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mode = args[1].to_int();
    let Some(m) = CaseMode::from_i64(mode) else {
        return Err(Unwind::value_error("mb_convert_case(): Argument #2 ($mode) must be one of the MB_CASE_* constants"));
    };
    case_common(ctx, args, "mb_convert_case", m, 2)
}

/// `mb_strtoupper(string $string, ?string $encoding = null): string`
fn mb_strtoupper(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    case_common(ctx, args, "mb_strtoupper", CaseMode::Upper, 1)
}

/// `mb_strtolower(string $string, ?string $encoding = null): string`
fn mb_strtolower(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    case_common(ctx, args, "mb_strtolower", CaseMode::Lower, 1)
}

/// `php_mb_ulcfirst`.
fn ulcfirst(ctx: &mut Ctx, args: &mut [Value], func: &str, mode: CaseMode) -> NativeResult {
    let s = str_arg(&args[0], func, 1, "string")?;
    let enc = enc_arg(ctx, args, 1, func)?;
    let first = get_substr(ctx, &s, 0, 1, enc);
    let (m, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, m);
    convert_case(mode, &first, enc, enc, &mut buf);
    if buf.out == first {
        return Ok(str_value(s));
    }
    let mut out = buf.out;
    out.extend(get_substr(ctx, &s, 1, UNTIL_END, enc));
    Ok(str_value(out))
}

/// `mb_ucfirst(string $string, ?string $encoding = null): string`
fn mb_ucfirst(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ulcfirst(ctx, args, "mb_ucfirst", CaseMode::Title)
}

/// `mb_lcfirst(string $string, ?string $encoding = null): string`
fn mb_lcfirst(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ulcfirst(ctx, args, "mb_lcfirst", CaseMode::Lower)
}

const TRIM_DEFAULT: &[u32] = &[
    0x20, 0x0C, 0x0A, 0x0D, 0x09, 0x0B, 0x00, 0xA0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200A,
    0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0x85, 0x180E,
];

fn trim_common(ctx: &mut Ctx, args: &mut [Value], func: &str, left: bool, right: bool) -> NativeResult {
    let s = str_arg(&args[0], func, 1, "string")?;
    let what = opt_str_arg(args, 1, func, "characters")?;
    let enc = enc_arg(ctx, args, 2, func)?;
    let set: Vec<u32> = match &what {
        Some(w) => {
            let set = enc.decode(w);
            if set.is_empty() {
                return Ok(str_value(s));
            }
            set
        }
        None => TRIM_DEFAULT.to_vec(),
    };
    let w = enc.decode(&s);
    let total = w.len();
    let mut l = 0;
    if left {
        while l < total && set.contains(&w[l]) {
            l += 1;
        }
    }
    let mut r = 0;
    if right {
        while r < total - l && set.contains(&w[total - 1 - r]) {
            r += 1;
        }
    }
    if l == 0 && r == 0 {
        return Ok(str_value(s));
    }
    Ok(str_value(get_substr(ctx, &s, l, total - (r + l), enc)))
}

/// `mb_trim(string $string, ?string $characters = null, ?string $encoding = null): string`
fn mb_trim(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    trim_common(ctx, args, "mb_trim", true, true)
}

/// `mb_ltrim(...)`
fn mb_ltrim(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    trim_common(ctx, args, "mb_ltrim", true, false)
}

/// `mb_rtrim(...)`
fn mb_rtrim(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    trim_common(ctx, args, "mb_rtrim", false, true)
}

/// `mb_detect_encoding(string $string, array|string|null $encodings = null, bool $strict = false): string|false`
fn mb_detect_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_detect_encoding", 1, "string")?;
    let order_significant = !args.get(1).is_some_and(is_full_list);
    let mut list = match encoding_list_arg(ctx, args, 1, "mb_detect_encoding", "encodings")? {
        Some(l) => l,
        None => current_detect_order(ctx),
    };
    if list.is_empty() {
        return Err(Unwind::value_error("mb_detect_encoding(): Argument #2 ($encodings) must specify at least one encoding"));
    }
    remove_non_encodings(&mut list);
    if list.is_empty() {
        return Ok(Value::Bool(false));
    }
    let strict = match args.get(2) {
        Some(v) => v.to_bool(),
        None => ctx.ini.bool("mbstring.strict_detection"),
    };
    match detect::guess(&[&s], &list, strict, order_significant) {
        Some(e) => Ok(str_value(e.name.as_bytes().to_vec())),
        None => Ok(Value::Bool(false)),
    }
}

/// `mb_list_encodings(): array`
fn mb_list_encodings(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let list: Vec<&Encoding> = mbfl::ENCODINGS.iter().collect();
    Ok(names_array(&list))
}

/// `mb_encoding_aliases(string $encoding): array`
fn mb_encoding_aliases(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = str_arg(&args[0], "mb_encoding_aliases", 1, "encoding")?;
    let enc = get_encoding(ctx, Some(&name), "mb_encoding_aliases", 1, "encoding")?;
    let mut arr = Array::new();
    for a in enc.aliases {
        arr.push(str_value(a.as_bytes().to_vec()));
    }
    Ok(Value::Array(arr))
}

/// Collect every string inside `v` (`mb_recursive_find_strings`).
fn collect_strings(v: &Value, out: &mut Vec<Vec<u8>>) {
    match v.deref().as_ref() {
        Value::Str(s) => out.push(s.as_bytes().to_vec()),
        Value::Array(a) => {
            for (_, item) in a.iter() {
                collect_strings(item, out);
            }
        }
        _ => {}
    }
}

/// `mb_convert_variables(string $to_encoding, array|string $from_encoding, mixed &$var, mixed &...$vars): string|false`
fn mb_convert_variables(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let to_name = str_arg(&args[0], "mb_convert_variables", 1, "to_encoding")?;
    let to = get_encoding(ctx, Some(&to_name), "mb_convert_variables", 1, "to_encoding")?;
    let order_significant = !is_full_list(&args[1]);
    let list = match encoding_list_arg(ctx, args, 1, "mb_convert_variables", "from_encoding")? {
        Some(l) => l,
        None => Vec::new(),
    };
    if list.is_empty() {
        return Err(Unwind::value_error("mb_convert_variables(): Argument #2 ($from_encoding) must specify at least one encoding"));
    }
    let from = if list.len() == 1 {
        list[0]
    } else {
        let mut strings = Vec::new();
        for v in &args[2..] {
            collect_strings(v, &mut strings);
        }
        let refs: Vec<&[u8]> = strings.iter().map(|s| s.as_slice()).collect();
        let strict = ctx.ini.bool("mbstring.strict_detection");
        match detect::guess(&refs, &list, strict, order_significant) {
            Some(e) => e,
            None => {
                ctx.warn("mb_convert_variables(): Unable to detect encoding")?;
                return Ok(Value::Bool(false));
            }
        }
    };
    for v in args[2..].iter_mut() {
        let converted = convert_value_recursive(ctx, v, to, from);
        Value::assign(v, converted);
    }
    Ok(str_value(from.name.as_bytes().to_vec()))
}

/// `make_conversion_map`.
fn conversion_map(func: &str, v: &Value) -> Result<Vec<u32>, Unwind> {
    let v = v.deref();
    let Value::Array(a) = v.as_ref() else {
        return Err(Unwind::type_error(format!("{func}(): Argument #2 ($map) must be of type array, {} given", rphp_runtime::value_name(&v))));
    };
    if a.len() % 4 != 0 {
        return Err(Unwind::value_error(format!("{func}(): Argument #2 ($map) must have a multiple of 4 elements")));
    }
    let mut map = Vec::with_capacity(a.len());
    for (_, item) in a.iter() {
        match item.deref().as_ref() {
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Null => map.push(item.to_int() as u32),
            Value::Str(_) if item.is_numeric() => map.push(item.to_int() as u32),
            _ => return Err(Unwind::value_error(format!("{func}(): Argument #2 ($map) must only be composed of values of type int"))),
        }
    }
    Ok(map)
}

/// `mb_encode_numericentity(string $string, array $map, ?string $encoding = null, bool $hex = false): string`
fn mb_encode_numericentity(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_encode_numericentity", 1, "string")?;
    let enc = enc_arg(ctx, args, 2, "mb_encode_numericentity")?;
    let map = conversion_map("mb_encode_numericentity", &args[1])?;
    let hex = args.get(3).is_some_and(Value::to_bool);
    let w = enc.decode(&s);
    let mut converted = Vec::with_capacity(w.len());
    for &c in &w {
        let mut hit = None;
        for m in map.chunks(4) {
            if c >= m[0] && c <= m[1] {
                hit = Some(c.wrapping_add(m[2]) & m[3]);
                break;
            }
        }
        match hit {
            Some(v) => {
                converted.push(b'&' as u32);
                converted.push(b'#' as u32);
                if hex {
                    converted.push(b'x' as u32);
                }
                let digits = if hex { format!("{v:X}") } else { v.to_string() };
                converted.extend(digits.bytes().map(|b| b as u32));
                converted.push(b';' as u32);
            }
            None => converted.push(c),
        }
    }
    let (mode, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, mode);
    enc.encode(&converted, &mut buf, true);
    Ok(str_value(buf.out))
}

/// `html_numeric_entity_deconvert`.
fn deconvert(number: u32, map: &[u32]) -> Option<u32> {
    for m in map.chunks(4) {
        let cp = number.wrapping_sub(m[2]);
        if cp >= m[0] && cp <= m[1] {
            return Some(cp);
        }
    }
    None
}

/// `html_numeric_entity_decode` on a whole wchar array.
fn decode_entities(w: &[u32], map: &[u32]) -> Vec<u32> {
    const DEC_MIN: usize = 3;
    const HEX_MIN: usize = 4;
    const DEC_MAX: usize = 12;
    const HEX_MAX: usize = 11;
    let n = w.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    let is_hex = |c: u32| (b'0' as u32..=b'9' as u32).contains(&c) || (b'A' as u32..=b'F' as u32).contains(&c) || (b'a' as u32..=b'f' as u32).contains(&c);
    let is_dec = |c: u32| (b'0' as u32..=b'9' as u32).contains(&c);
    while i < n {
        if w[i] != b'&' as u32 {
            out.push(w[i]);
            i += 1;
            continue;
        }
        let mut p2 = i + 1;
        if p2 < n && w[p2] == b'#' as u32 {
            p2 += 1;
            if p2 < n && w[p2] == b'x' as u32 {
                p2 += 1;
                while p2 < n && is_hex(w[p2]) {
                    p2 += 1;
                }
                let len = p2 - i;
                if !(HEX_MIN..=HEX_MAX).contains(&len) {
                    out.extend_from_slice(&w[i..p2]);
                } else {
                    let mut value: u32 = 0;
                    for &c in &w[i + 3..p2] {
                        let d = if c <= b'9' as u32 {
                            c - b'0' as u32
                        } else if c >= b'a' as u32 {
                            10 + c - b'a' as u32
                        } else {
                            10 + c - b'A' as u32
                        };
                        value = value.wrapping_mul(16).wrapping_add(d);
                    }
                    match deconvert(value, map) {
                        Some(cp) => {
                            out.push(cp);
                            if p2 < n && w[p2] == b';' as u32 {
                                p2 += 1;
                            }
                        }
                        None => out.extend_from_slice(&w[i..p2]),
                    }
                }
            } else {
                while p2 < n && is_dec(w[p2]) {
                    p2 += 1;
                }
                let len = p2 - i;
                if !(DEC_MIN..=DEC_MAX).contains(&len) {
                    out.extend_from_slice(&w[i..p2]);
                } else {
                    let mut value: u32 = 0;
                    let mut too_big = false;
                    for &c in &w[i + 2..p2] {
                        if value > 0x1999_9999 {
                            too_big = true;
                            break;
                        }
                        value = value * 10 + (c - b'0' as u32);
                    }
                    if too_big {
                        out.extend_from_slice(&w[i..p2]);
                    } else {
                        match deconvert(value, map) {
                            Some(cp) => {
                                out.push(cp);
                                if p2 < n && w[p2] == b';' as u32 {
                                    p2 += 1;
                                }
                            }
                            None => out.extend_from_slice(&w[i..p2]),
                        }
                    }
                }
            }
        } else {
            out.push(b'&' as u32);
        }
        i = p2;
    }
    out
}

/// `mb_decode_numericentity(string $string, array $map, ?string $encoding = null): string`
fn mb_decode_numericentity(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_decode_numericentity", 1, "string")?;
    let enc = enc_arg(ctx, args, 2, "mb_decode_numericentity")?;
    let map = conversion_map("mb_decode_numericentity", &args[1])?;
    let w = enc.decode(&s);
    let converted = decode_entities(&w, &map);
    let (mode, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, mode);
    enc.encode(&converted, &mut buf, true);
    Ok(str_value(buf.out))
}

/// `mb_get_info(string $type = "all"): array|string|int|false|null`
fn mb_get_info(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ty = match args.first() {
        Some(v) => str_arg(v, "mb_get_info", 1, "type")?,
        None => b"all".to_vec(),
    };
    let lang = language(ctx);
    let (mode, ch) = substitute(ctx);
    let on_off = |b: bool| Value::string(if b { b"On" } else { b"Off" });
    let name = |id: Id| str_value(mbfl::by_id(id).name.as_bytes().to_vec());
    let t = ty.to_ascii_lowercase();
    let v = match t.as_slice() {
        b"all" => {
            let mut arr = Array::new();
            let mut set = |k: &str, v: Value| arr.set(ArrayKey::str(k.as_bytes()), v);
            set("internal_encoding", str_value(internal_encoding(ctx).name.as_bytes().to_vec()));
            if let Some(id) = with_state(|s| s.http_input_identify) {
                set("http_input", name(id));
            }
            set("http_output", str_value(http_output_encoding(ctx).name.as_bytes().to_vec()));
            set("http_output_conv_mimetypes", str_value(ini_str(ctx, "mbstring.http_output_conv_mimetypes").into_bytes()));
            set("mail_charset", name(lang.mail_charset));
            set("mail_header_encoding", name(lang.mail_header));
            set("mail_body_encoding", name(lang.mail_body));
            set("illegal_chars", Value::Int(with_state(|s| s.illegal_chars)));
            set("encoding_translation", on_off(ctx.ini.bool("mbstring.encoding_translation")));
            set("language", str_value(lang.name.as_bytes().to_vec()));
            let order = current_detect_order(ctx);
            if !order.is_empty() {
                set("detect_order", names_array(&order));
            }
            set("substitute_character", subst_value(mode, ch));
            set("strict_detection", on_off(ctx.ini.bool("mbstring.strict_detection")));
            Value::Array(arr)
        }
        b"internal_encoding" => str_value(internal_encoding(ctx).name.as_bytes().to_vec()),
        b"http_input" => match with_state(|s| s.http_input_identify) {
            Some(id) => name(id),
            None => Value::Null,
        },
        b"http_output" => str_value(http_output_encoding(ctx).name.as_bytes().to_vec()),
        b"http_output_conv_mimetypes" => str_value(ini_str(ctx, "mbstring.http_output_conv_mimetypes").into_bytes()),
        b"mail_charset" => name(lang.mail_charset),
        b"mail_header_encoding" => name(lang.mail_header),
        b"mail_body_encoding" => name(lang.mail_body),
        b"illegal_chars" => Value::Int(with_state(|s| s.illegal_chars)),
        b"encoding_translation" => on_off(ctx.ini.bool("mbstring.encoding_translation")),
        b"language" => str_value(lang.name.as_bytes().to_vec()),
        b"detect_order" => names_array(&current_detect_order(ctx)),
        b"substitute_character" => subst_value(mode, ch),
        b"strict_detection" => on_off(ctx.ini.bool("mbstring.strict_detection")),
        _ => {
            ctx.warn("mb_get_info(): argument #1 ($type) must be a valid type")?;
            Value::Bool(false)
        }
    };
    Ok(v)
}

/// `php_mb_check_encoding_recursive`.
fn check_encoding_recursive(ctx: &mut Ctx, a: &Array, enc: &Encoding) -> Result<bool, Unwind> {
    for (k, v) in a.iter() {
        if let ArrayKey::Str(s) = k {
            if !enc.check(s) {
                return Ok(false);
            }
        }
        let ok = match v.deref().as_ref() {
            Value::Str(s) => enc.check(s.as_bytes()),
            Value::Array(inner) => check_encoding_recursive(ctx, inner, enc)?,
            Value::Int(_) | Value::Float(_) | Value::Null | Value::Bool(_) => true,
            _ => false,
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `mb_check_encoding(array|string|null $value = null, ?string $encoding = null): bool`
fn mb_check_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let enc = enc_arg(ctx, args, 1, "mb_check_encoding")?;
    match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => Ok(Value::Bool(check_encoding_recursive(ctx, &a, enc)?)),
        Some(Value::Null) | None => {
            ctx.deprecated("mb_check_encoding(): Calling mb_check_encoding() without argument is deprecated")?;
            Ok(Value::Bool(with_state(|s| s.illegal_chars) == 0))
        }
        Some(Value::Object(_)) | Some(Value::Closure(_)) => Err(Unwind::type_error(format!(
            "mb_check_encoding(): Argument #1 ($value) must be of type array|string|null, {} given",
            rphp_runtime::value_name(&args[0])
        ))),
        Some(v) => Ok(Value::Bool(enc.check(&bytes(&v)))),
    }
}

/// `mb_ord(string $string, ?string $encoding = null): int|false`
fn mb_ord(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_ord", 1, "string")?;
    if s.is_empty() {
        return Err(Unwind::value_error("mb_ord(): Argument #1 ($string) must not be empty"));
    }
    let enc = enc_arg(ctx, args, 1, "mb_ord")?;
    if enc.is_unsupported_for_ord_chr() {
        return Err(Unwind::value_error(format!("mb_ord() does not support the \"{}\" encoding", enc.name)));
    }
    let w = enc.decode(&s);
    match w.first() {
        Some(&c) if c != BAD_INPUT => Ok(Value::Int(c as i64)),
        _ => Ok(Value::Bool(false)),
    }
}

/// `mb_chr(int $codepoint, ?string $encoding = null): string|false`
fn mb_chr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let cp = args[0].to_int();
    let enc = enc_arg(ctx, args, 1, "mb_chr")?;
    if enc.is_unsupported_for_ord_chr() {
        return Err(Unwind::value_error(format!("mb_chr() does not support the \"{}\" encoding", enc.name)));
    }
    if !(0..=0x10FFFF).contains(&cp) {
        return Ok(Value::Bool(false));
    }
    if enc.is_utf8() {
        if (0xD800..=0xDFFF).contains(&cp) {
            return Ok(Value::Bool(false));
        }
        let mut out = Vec::with_capacity(4);
        mbfl::unicode::push_utf8(cp as u32, &mut out);
        return Ok(str_value(out));
    }
    let (mode, ch) = substitute(ctx);
    let mut buf = ConvertBuf::new(ch, mode);
    enc.encode(&[cp as u32], &mut buf, true);
    if buf.errors != 0 {
        return Ok(Value::Bool(false));
    }
    Ok(str_value(buf.out))
}

/// `mb_str_split(string $string, int $length = 1, ?string $encoding = null): array`
fn mb_str_split(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_str_split", 1, "string")?;
    let split_len = opt_int_arg(args, 1).unwrap_or(1);
    if split_len <= 0 {
        return Err(Unwind::value_error("mb_str_split(): Argument #2 ($length) must be greater than 0"));
    }
    if split_len > (u32::MAX / 4) as i64 {
        return Err(Unwind::value_error("mb_str_split(): Argument #2 ($length) is too large"));
    }
    let enc = enc_arg(ctx, args, 2, "mb_str_split")?;
    let split_len = split_len as usize;
    let mut arr = Array::new();
    if s.is_empty() {
        return Ok(Value::Array(arr));
    }
    if let Some(w) = enc.fixed_width() {
        let chunk = w * split_len;
        for c in s.chunks(chunk) {
            arr.push(str_value(c.to_vec()));
        }
    } else if let Some(tab) = enc.mblen_table {
        let mut p = 0;
        while p < s.len() {
            let start = p;
            let mut count = 0;
            while count < split_len && p < s.len() {
                p += tab[s[p] as usize] as usize;
                count += 1;
            }
            let p2 = p.min(s.len());
            arr.push(str_value(s[start..p2].to_vec()));
            p = p2;
        }
    } else {
        let w = enc.decode(&s);
        let (mode, ch) = substitute(ctx);
        for chunk in w.chunks(split_len) {
            let mut buf = ConvertBuf::new(ch, mode);
            enc.encode(chunk, &mut buf, true);
            arr.push(str_value(buf.out));
        }
    }
    Ok(Value::Array(arr))
}

/// `mb_str_pad(string $string, int $length, string $pad_string = " ", int $pad_type = STR_PAD_RIGHT, ?string $encoding = null): string`
fn mb_str_pad(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_str_pad", 1, "string")?;
    let pad_to = args[1].to_int();
    let pad = match args.get(2) {
        Some(v) => str_arg(v, "mb_str_pad", 3, "pad_string")?,
        None => b" ".to_vec(),
    };
    let pad_type = opt_int_arg(args, 3).unwrap_or(1);
    let enc = enc_arg(ctx, args, 4, "mb_str_pad")?;
    let input_len = strlen_of(&s, enc);
    if pad_to < 0 || pad_to as usize <= input_len {
        return Ok(str_value(s));
    }
    if pad.is_empty() {
        return Err(Unwind::value_error("mb_str_pad(): Argument #3 ($pad_string) must not be empty"));
    }
    if !(0..=2).contains(&pad_type) {
        return Err(Unwind::value_error("mb_str_pad(): Argument #4 ($pad_type) must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH"));
    }
    let pad_len = strlen_of(&pad, enc);
    let num = pad_to as usize - input_len;
    let (left, right) = match pad_type {
        0 => (num, 0),
        1 => (0, num),
        _ => (num / 2, num - num / 2),
    };
    let mut out = Vec::new();
    for _ in 0..left / pad_len {
        out.extend_from_slice(&pad);
    }
    out.extend(get_substr(ctx, &pad, 0, left % pad_len, enc));
    out.extend_from_slice(&s);
    for _ in 0..right / pad_len {
        out.extend_from_slice(&pad);
    }
    out.extend(get_substr(ctx, &pad, 0, right % pad_len, enc));
    Ok(str_value(out))
}

/// `mb_scrub(string $string, ?string $encoding = null): string`
fn mb_scrub(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_scrub", 1, "string")?;
    let enc = enc_arg(ctx, args, 1, "mb_scrub")?;
    if enc.id == Id::Utf8 && mbfl::unicode::check_utf8(&s) {
        return Ok(str_value(s));
    }
    Ok(str_value(convert_ex(ctx, &s, enc, enc)))
}

// ---- MIME headers -------------------------------------------------------------------------

/// `mime_char_needs_qencode` (`unicode_prop.h`): bytes that Q-encoding must
/// escape besides non-ASCII and `=`.
fn needs_qencode(c: u8) -> bool {
    !matches!(c, b'!' | b'*' | b'+' | b'-' | b'/' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z')
}

fn transfer_encoded_size(bytes: &[u8], base64: bool) -> usize {
    if base64 {
        bytes.len().div_ceil(3) * 4
    } else {
        bytes.iter().map(|&c| if c > 0x7F || c == b'=' || needs_qencode(c) { 3 } else { 1 }).sum()
    }
}

fn transfer_encode(bytes: &[u8], base64: bool, out: &mut Vec<u8>) {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if base64 {
        for chunk in bytes.chunks(3) {
            let bits = chunk.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
            out.push(B64[((bits >> 18) & 0x3F) as usize]);
            out.push(B64[((bits >> 12) & 0x3F) as usize]);
            out.push(if chunk.len() > 1 { B64[((bits >> 6) & 0x3F) as usize] } else { b'=' });
            out.push(if chunk.len() > 2 { B64[(bits & 0x3F) as usize] } else { b'=' });
        }
    } else {
        for &c in bytes {
            if c > 0x7F || c == b'=' || needs_qencode(c) {
                out.push(b'=');
                out.push(b"0123456789ABCDEF"[(c >> 4) as usize]);
                out.push(b"0123456789ABCDEF"[(c & 0xF) as usize]);
            } else {
                out.push(c);
            }
        }
    }
}

/// `mb_mime_header_encode`: RFC 2047 encoded words, folded at 76 columns.
fn mime_header_encode(input: &[u8], incode: &Encoding, outcode: &Encoding, base64: bool, linefeed: &[u8], mut indent: i64) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    if !(0..74).contains(&indent) {
        indent = 0;
    }
    let mut linefeed = &linefeed[..linefeed.len().min(8)];
    if let Some(nul) = linefeed.iter().position(|&b| b == 0) {
        linefeed = &linefeed[..nul];
    }
    let mime_name = outcode.mime_name.unwrap_or("");
    let w = incode.decode(input);

    // Pass-through: ASCII without spaces (except leading ones) or `=?_`.
    let mut leading = true;
    let mut passthrough = true;
    for &c in &w {
        if leading && c == b' ' as u32 {
            continue;
        }
        leading = false;
        if !(0x21..=0x7E).contains(&c) || c == b'=' as u32 || c == b'?' as u32 || c == b'_' as u32 {
            passthrough = false;
            break;
        }
    }
    if passthrough {
        return input.to_vec();
    }

    let mut buf: Vec<u8> = Vec::with_capacity(input.len() * 2);
    let mut line_start = 0usize;
    let e = w.len();
    let mut word_start = 0usize;
    let mut p = 0usize;
    // ASCII mode: emit space-delimited words as plain text until a
    // character that needs encoding (or an over-long word) is found.
    let mut mime_from: Option<usize> = None;
    while p < e && w[p] == b' ' as u32 && p - word_start <= 74 {
        p += 1;
    }
    while p < e {
        let c = w[p];
        p += 1;
        if !(0x20..=0x7E).contains(&c) || c == b'?' as u32 || c == b'=' as u32 || c == b'_' as u32 || (c == b' ' as u32 && p - word_start > 74) {
            if buf.len() - line_start + indent as usize + mime_name.len() > 55 {
                buf.extend_from_slice(linefeed);
                buf.push(b' ');
                indent = 0;
                line_start = buf.len();
            } else if !buf.is_empty() {
                buf.push(b' ');
            }
            mime_from = Some(word_start);
            break;
        } else if c == b' ' as u32 {
            if buf.len() - line_start + (p - word_start) + indent as usize > 75 {
                buf.extend_from_slice(linefeed);
                buf.push(b' ');
                indent = 0;
                line_start = buf.len();
            } else if !buf.is_empty() {
                buf.push(b' ');
            }
            while word_start < p - 1 {
                buf.push((w[word_start] & 0xFF) as u8);
                word_start += 1;
            }
            word_start += 1;
            while p < e && w[p] == b' ' as u32 {
                p += 1;
            }
        }
    }
    let Some(mut p) = mime_from else {
        // Reached the end in ASCII mode: trailing word without a space.
        if word_start < e && !buf.is_empty() {
            if buf.len() - line_start + (e - word_start) + indent as usize > 74 {
                buf.extend_from_slice(linefeed);
                buf.push(b' ');
            } else {
                buf.push(b' ');
            }
        }
        while word_start < e {
            buf.push((w[word_start] & 0xFF) as u8);
            word_start += 1;
        }
        return buf;
    };

    // MIME mode: encoded words, each sized by binary search to fit the line.
    let mut tmp = ConvertBuf::new(b'?' as u32, ErrorMode::Char);
    loop {
        // start_new_line
        buf.extend_from_slice(b"=?");
        buf.extend_from_slice(mime_name.as_bytes());
        buf.extend_from_slice(if base64 { b"?B?" } else { b"?Q?" });
        let mut n = 12usize;
        let space_available = 73usize.saturating_sub(indent as usize).saturating_sub(buf.len() - line_start);
        loop {
            let tmppos = tmp.len();
            let tmpstate = tmp.state;
            n = n.min(e - p);
            outcode.encode(&w[p..p + n], &mut tmp, false);
            let tmppos2 = tmp.len();
            let tmpstate2 = tmp.state;
            outcode.encode(&[], &mut tmp, true);
            if transfer_encoded_size(&tmp.out, base64) <= space_available || (n == 1 && tmppos == 0) {
                p += n;
                if p == e {
                    let bytes = std::mem::take(&mut tmp.out);
                    transfer_encode(&bytes, base64, &mut buf);
                    buf.extend_from_slice(b"?=");
                    return buf;
                }
                tmp.reset(tmppos2);
                tmp.state = tmpstate2;
            } else {
                tmp.reset(tmppos);
                tmp.state = tmpstate;
                if n == 1 {
                    outcode.encode(&[], &mut tmp, true);
                    let bytes = std::mem::take(&mut tmp.out);
                    transfer_encode(&bytes, base64, &mut buf);
                    tmp.state = 0;
                    buf.extend_from_slice(b"?=");
                    indent = 0;
                    if p < e {
                        buf.extend_from_slice(linefeed);
                        buf.push(b' ');
                        line_start = buf.len();
                        break;
                    }
                    return buf;
                }
                n = (n >> 1).max(1);
            }
        }
    }
}

/// `mb_encode_mimeheader(string $string, ?string $charset = null, ?string $transfer_encoding = null, string $newline = "\r\n", int $indent = 0): string`
fn mb_encode_mimeheader(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_encode_mimeheader", 1, "string")?;
    let mut base64 = true;
    let charset = match opt_str_arg(args, 1, "mb_encode_mimeheader", "charset")? {
        Some(name) => {
            let enc = get_encoding(ctx, Some(&name), "mb_encode_mimeheader", 2, "charset")?;
            if mbfl::preferred_mime_name(enc).is_none() || enc.id == Id::QPrint {
                return Err(Unwind::value_error(format!(
                    "mb_encode_mimeheader(): Argument #2 ($charset) \"{}\" cannot be used for MIME header encoding",
                    String::from_utf8_lossy(&name)
                )));
            }
            enc
        }
        None => {
            let lang = language(ctx);
            let transenc = mbfl::by_id(lang.mail_header);
            if transenc.name.starts_with(['Q', 'q']) {
                base64 = false;
            }
            mbfl::by_id(lang.mail_charset)
        }
    };
    if let Some(t) = opt_str_arg(args, 2, "mb_encode_mimeheader", "transfer_encoding")? {
        if t.first().is_some_and(|c| *c == b'Q' || *c == b'q') {
            base64 = false;
        }
    }
    let linefeed = match args.get(3) {
        Some(v) => str_arg(v, "mb_encode_mimeheader", 4, "newline")?,
        None => b"\r\n".to_vec(),
    };
    let indent = opt_int_arg(args, 4).unwrap_or(0);
    let incode = internal_encoding(ctx);
    Ok(str_value(mime_header_encode(&s, incode, charset, base64, &linefeed, indent)))
}

fn decode_b64_char(c: u8) -> i8 {
    match c {
        b'A'..=b'Z' => (c - b'A') as i8,
        b'a'..=b'z' => (c - b'a' + 26) as i8,
        b'0'..=b'9' => (c - b'0' + 52) as i8,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    }
}

fn qp_val(c: u8) -> i8 {
    match c {
        b'0'..=b'9' => (c - b'0') as i8,
        b'A'..=b'F' => (c - b'A' + 10) as i8,
        b'a'..=b'f' => (c - b'a' + 10) as i8,
        _ => -1,
    }
}

/// `mime_header_decode_encoded_word`: decode one `=?charset?X?text?=` at
/// `p`, appending to `buf`; returns the index after it, or `None` when it
/// is not a well-formed encoded word.
fn decode_encoded_word(s: &[u8], p: usize, outcode: &Encoding, buf: &mut ConvertBuf) -> Option<usize> {
    let mut e = s.len();
    if e - p < 6 {
        return None;
    }
    let charset = p + 2;
    let charset_end = charset + s[charset..].iter().position(|&b| b == b'?')?;
    let encoding = charset_end + 1;
    let mut p = encoding + 1;
    if p >= e || s[p] != b'?' {
        return None;
    }
    p += 1;
    let incode = mbfl::name2encoding(&s[charset..charset_end])?;
    if let Some(i) = find_bytes(&s[p..], b"?=") {
        e = p + i;
    } else if p < e && s[e - 1] == b'?' {
        e -= 1;
    }
    let mut decoded = Vec::with_capacity(e.saturating_sub(p));
    match s[encoding] {
        b'Q' | b'q' => {
            while p < e {
                let c = s[p];
                p += 1;
                if c == b'_' {
                    decoded.push(b' ');
                    continue;
                } else if c == b'=' && e - p >= 2 {
                    let c2 = s[p];
                    let c3 = s[p + 1];
                    p += 2;
                    if qp_val(c2) >= 0 && qp_val(c3) >= 0 {
                        decoded.push(((qp_val(c2) as u8) << 4) | (qp_val(c3) as u8 & 0xF));
                        continue;
                    } else if c2 == b'\r' {
                        if c3 != b'\n' {
                            p -= 1;
                        }
                        continue;
                    } else if c2 == b'\n' {
                        p -= 1;
                        continue;
                    }
                }
                decoded.push(c);
            }
        }
        b'B' | b'b' => {
            let mut bits = 0u32;
            let mut cache = 0u32;
            while p < e {
                let c = s[p];
                p += 1;
                if matches!(c, b'\r' | b'\n' | b' ' | b'\t' | b'=') {
                    continue;
                }
                let v = decode_b64_char(c);
                if v < 0 {
                    decoded.push(b'?');
                    continue;
                }
                bits += 6;
                cache = (cache << 6) | (v as u32 & 0x3F);
                if bits == 24 {
                    decoded.push((cache >> 16) as u8);
                    decoded.push((cache >> 8) as u8);
                    decoded.push(cache as u8);
                    bits = 0;
                    cache = 0;
                }
            }
            if bits == 18 {
                decoded.push((cache >> 10) as u8);
                decoded.push((cache >> 2) as u8);
            } else if bits == 12 {
                decoded.push((cache >> 4) as u8);
            }
        }
        _ => return None,
    }
    let w = incode.decode(&decoded);
    outcode.encode(&w, buf, false);
    Some(e + 2)
}

/// `mb_mime_header_decode`.
fn mime_header_decode(s: &[u8], outcode: &Encoding) -> Vec<u8> {
    let e = s.len();
    let mut p = 0;
    let mut space_pending = false;
    let mut buf = ConvertBuf::new(b'?' as u32, ErrorMode::Char);
    while p < e {
        let c = s[p];
        if c == b'=' && p + 1 < e && s[p + 1] == b'?' && e - p >= 6 {
            if let Some(i) = s[p + 2..].iter().position(|&b| b == b'?') {
                let incode_end = p + 2 + i;
                if e - incode_end >= 3 {
                    if let Some(next) = decode_encoded_word(s, p, outcode, &mut buf) {
                        p = next;
                        if p < e && (s[p] == b'\n' || s[p] == b'\r') {
                            p += 1;
                            while p < e && matches!(s[p], b'\n' | b'\r' | b'\t' | b' ') {
                                p += 1;
                            }
                            space_pending = true;
                        }
                        continue;
                    }
                }
            }
        }
        if space_pending {
            outcode.encode(&[b' ' as u32], &mut buf, false);
            space_pending = false;
        }
        if c != b'\n' && c != b'\r' {
            let mut end = p + 1;
            while end < e && s[end] != b'=' && s[end] != b'\n' && s[end] != b'\r' {
                end += 1;
            }
            let w: Vec<u32> = s[p..end].iter().map(|&b| if b < 0x80 { b as u32 } else { BAD_INPUT }).collect();
            outcode.encode(&w, &mut buf, false);
            p = end;
        }
        if p < e && (s[p] == b'\n' || s[p] == b'\r') {
            p += 1;
            while p < e && matches!(s[p], b'\n' | b'\r' | b'\t' | b' ') {
                p += 1;
            }
            if p < e {
                outcode.encode(&[b' ' as u32], &mut buf, false);
            }
        }
    }
    outcode.encode(&[], &mut buf, true);
    buf.out
}

/// `mb_decode_mimeheader(string $string): string`
fn mb_decode_mimeheader(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(&args[0], "mb_decode_mimeheader", 1, "string")?;
    let outcode = internal_encoding(ctx);
    Ok(str_value(mime_header_decode(&s, outcode)))
}

/// Case-map wchars with the internal encoding's rules (used by tests and
/// by `iconv`'s case-insensitive helpers).
#[allow(dead_code)]
pub(crate) fn fold_wchars(w: &[u32]) -> Vec<u32> {
    convert_case_wchars(CaseMode::FoldSimple, w, &mbfl::UTF8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{arr, call_err, call_named};

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    #[test]
    fn strlen_substr_strpos() {
        reset_state();
        assert_eq!(call_named(b"mb_strlen", &[s("日本語")]), Value::Int(3));
        assert_eq!(call_named(b"mb_strlen", &[s("日本語"), s("8bit")]), Value::Int(9));
        assert_eq!(call_named(b"mb_substr", &[s("日本語テキスト"), Value::Int(1), Value::Int(2)]), s("本語"));
        assert_eq!(call_named(b"mb_substr", &[s("日本語"), Value::Int(-2)]), s("本語"));
        assert_eq!(call_named(b"mb_strpos", &[s("日本語"), s("語")]), Value::Int(2));
        assert_eq!(call_named(b"mb_strrpos", &[s("日本日"), s("日")]), Value::Int(2));
        assert_eq!(call_named(b"mb_stripos", &[s("ÄBC"), s("äb")]), Value::Int(0));
        assert_eq!(call_named(b"mb_strpos", &[s("abc"), s("d")]), Value::Bool(false));
        let err = call_err(b"mb_strpos", &[s("abc"), s("a"), Value::Int(5)]);
        assert_eq!(err.message(), Some("mb_strpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"));
        let err = call_err(b"mb_strlen", &[s("x"), s("nope")]);
        assert_eq!(err.message(), Some("mb_strlen(): Argument #2 ($encoding) must be a valid encoding, \"nope\" given"));
    }

    #[test]
    fn conversion_and_detection() {
        reset_state();
        assert_eq!(call_named(b"mb_convert_encoding", &[s("é"), s("ISO-8859-1"), s("UTF-8")]), Value::string(b"\xe9"));
        assert_eq!(call_named(b"mb_convert_encoding", &[Value::string(b"caf\xe9"), s("UTF-8"), s("UTF-8, ISO-8859-1")]), s("café"));
        assert_eq!(call_named(b"mb_detect_encoding", &[Value::string(b"caf\xe9"), s("UTF-8, ISO-8859-1")]), s("ISO-8859-1"));
        assert_eq!(call_named(b"mb_detect_encoding", &[s("日本語"), arr(&[s("ASCII"), s("UTF-8")]), Value::Bool(true)]), s("UTF-8"));
        assert_eq!(call_named(b"mb_check_encoding", &[Value::string(b"\xff"), s("UTF-8")]), Value::Bool(false));
        assert_eq!(call_named(b"mb_scrub", &[Value::string(b"a\xffb")]), s("a?b"));
        assert_eq!(call_named(b"mb_ord", &[s("日")]), Value::Int(0x65E5));
        assert_eq!(call_named(b"mb_chr", &[Value::Int(0x1F600)]), s("😀"));
    }

    #[test]
    fn case_width_and_entities() {
        reset_state();
        assert_eq!(call_named(b"mb_strtoupper", &[s("straße")]), s("STRASSE"));
        assert_eq!(call_named(b"mb_convert_case", &[s("hello wORLD"), Value::Int(2)]), s("Hello World"));
        assert_eq!(call_named(b"mb_strwidth", &[s("日本a")]), Value::Int(5));
        assert_eq!(call_named(b"mb_strimwidth", &[s("Hello World"), Value::Int(0), Value::Int(8), s("...")]), s("Hello..."));
        let map = arr(&[Value::Int(0x80), Value::Int(0x10FFFF), Value::Int(0), Value::Int(0x1FFFFF)]);
        assert_eq!(call_named(b"mb_encode_numericentity", &[s("aé"), map.clone()]), s("a&#233;"));
        assert_eq!(call_named(b"mb_decode_numericentity", &[s("a&#233;&#xE9;"), map]), s("aéé"));
        assert_eq!(call_named(b"mb_str_pad", &[s("日"), Value::Int(3), s("本")]), s("日本本"));
        assert_eq!(call_named(b"mb_trim", &[s("\u{3000}日 ")]), s("日"));
    }

    #[test]
    fn mime_headers() {
        reset_state();
        assert_eq!(call_named(b"mb_encode_mimeheader", &[s("Subject: 日本語")]), s("Subject: =?UTF-8?B?5pel5pys6Kqe?="));
        assert_eq!(call_named(b"mb_decode_mimeheader", &[s("=?UTF-8?B?5pel5pys6Kqe?= x =?ISO-8859-1?Q?caf=E9?=")]), s("日本語 x café"));
        assert_eq!(call_named(b"mb_encode_mimeheader", &[s("plain")]), s("plain"));
    }
}
