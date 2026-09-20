//! `json` extension — a hand-rolled `json_encode` / `json_decode` /
//! `json_validate` (no serde), a transcription of php-src's
//! `ext/json/json_encoder.c` and the re2c/bison scanner-parser pair
//! (`json_scanner.re`, `json_parser.y`). Output is byte-exact against stock
//! PHP 8.5: every `JSON_*` flag, `serialize_precision` float layout, pretty
//! printing, the depth accounting (checked *after* a container's contents on
//! encode, on entering a container on decode), recursion detection,
//! `JsonSerializable`, php's error codes and texts behind
//! `json_last_error()` / `json_last_error_msg()`, and `JsonException` under
//! `JSON_THROW_ON_ERROR`.
//!
//! Decoding follows the scanner's exact classification: a raw control byte
//! is `JSON_ERROR_CTRL_CHAR` (including a NUL before the end and an
//! unterminated string), invalid UTF-8 is `JSON_ERROR_UTF8` (per byte under
//! `JSON_INVALID_UTF8_IGNORE`/`_SUBSTITUTE`), a lone or mismatched surrogate
//! escape is `JSON_ERROR_UTF16`, and anything else is `JSON_ERROR_SYNTAX`.
//! JSON objects decode to `stdClass` (dynamic properties, php's
//! `\0`-prefixed name check) unless `$associative` / `JSON_OBJECT_AS_ARRAY`
//! ask for arrays.
use rphp_value::{array_key, Array, ArrayKey, Object, PhpRef, Str, Value, Vis};

use rphp_runtime::{nf, nm, Ctx, NativeFn, NativeResult, Registry, Unwind};

use crate::output::{php_gcvt, serialize_precision};

// ---- flags & error codes ------------------------------------------------------

const JSON_HEX_TAG: i64 = 1;
const JSON_HEX_AMP: i64 = 2;
const JSON_HEX_APOS: i64 = 4;
const JSON_HEX_QUOT: i64 = 8;
const JSON_FORCE_OBJECT: i64 = 16;
const JSON_NUMERIC_CHECK: i64 = 32;
const JSON_UNESCAPED_SLASHES: i64 = 64;
const JSON_PRETTY_PRINT: i64 = 128;
const JSON_UNESCAPED_UNICODE: i64 = 256;
const JSON_PARTIAL_OUTPUT_ON_ERROR: i64 = 512;
const JSON_PRESERVE_ZERO_FRACTION: i64 = 1024;
const JSON_UNESCAPED_LINE_TERMINATORS: i64 = 2048;
const JSON_OBJECT_AS_ARRAY: i64 = 1;
const JSON_BIGINT_AS_STRING: i64 = 2;
const JSON_INVALID_UTF8_IGNORE: i64 = 1_048_576;
const JSON_INVALID_UTF8_SUBSTITUTE: i64 = 2_097_152;
const JSON_THROW_ON_ERROR: i64 = 4_194_304;

const JSON_ERROR_NONE: i64 = 0;
const JSON_ERROR_DEPTH: i64 = 1;
const JSON_ERROR_STATE_MISMATCH: i64 = 2;
const JSON_ERROR_CTRL_CHAR: i64 = 3;
const JSON_ERROR_SYNTAX: i64 = 4;
const JSON_ERROR_UTF8: i64 = 5;
const JSON_ERROR_RECURSION: i64 = 6;
const JSON_ERROR_INF_OR_NAN: i64 = 7;
const JSON_ERROR_UNSUPPORTED_TYPE: i64 = 8;
const JSON_ERROR_INVALID_PROPERTY_NAME: i64 = 9;
const JSON_ERROR_UTF16: i64 = 10;
const JSON_ERROR_NON_BACKED_ENUM: i64 = 11;

/// `PHP_JSON_PARSER_DEFAULT_DEPTH`.
const DEFAULT_DEPTH: i64 = 512;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("json_encode", 1, Some(3), json_encode),
    nf!("json_decode", 1, Some(4), json_decode),
    nf!("json_validate", 1, Some(3), json_validate),
    nf!("json_last_error", 0, Some(0), json_last_error),
    nf!("json_last_error_msg", 0, Some(0), json_last_error_msg),
];

/// The `JSON_*` constants and the `JsonSerializable` interface (see
/// `lib.rs`; `JsonException` is declared with the core exception tree).
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("JSON_HEX_TAG", JSON_HEX_TAG),
        ("JSON_HEX_AMP", JSON_HEX_AMP),
        ("JSON_HEX_APOS", JSON_HEX_APOS),
        ("JSON_HEX_QUOT", JSON_HEX_QUOT),
        ("JSON_FORCE_OBJECT", JSON_FORCE_OBJECT),
        ("JSON_NUMERIC_CHECK", JSON_NUMERIC_CHECK),
        ("JSON_UNESCAPED_SLASHES", JSON_UNESCAPED_SLASHES),
        ("JSON_PRETTY_PRINT", JSON_PRETTY_PRINT),
        ("JSON_UNESCAPED_UNICODE", JSON_UNESCAPED_UNICODE),
        ("JSON_PARTIAL_OUTPUT_ON_ERROR", JSON_PARTIAL_OUTPUT_ON_ERROR),
        ("JSON_PRESERVE_ZERO_FRACTION", JSON_PRESERVE_ZERO_FRACTION),
        ("JSON_UNESCAPED_LINE_TERMINATORS", JSON_UNESCAPED_LINE_TERMINATORS),
        ("JSON_OBJECT_AS_ARRAY", JSON_OBJECT_AS_ARRAY),
        ("JSON_BIGINT_AS_STRING", JSON_BIGINT_AS_STRING),
        ("JSON_INVALID_UTF8_IGNORE", JSON_INVALID_UTF8_IGNORE),
        ("JSON_INVALID_UTF8_SUBSTITUTE", JSON_INVALID_UTF8_SUBSTITUTE),
        ("JSON_THROW_ON_ERROR", JSON_THROW_ON_ERROR),
        ("JSON_ERROR_NONE", JSON_ERROR_NONE),
        ("JSON_ERROR_DEPTH", JSON_ERROR_DEPTH),
        ("JSON_ERROR_STATE_MISMATCH", JSON_ERROR_STATE_MISMATCH),
        ("JSON_ERROR_CTRL_CHAR", JSON_ERROR_CTRL_CHAR),
        ("JSON_ERROR_SYNTAX", JSON_ERROR_SYNTAX),
        ("JSON_ERROR_UTF8", JSON_ERROR_UTF8),
        ("JSON_ERROR_RECURSION", JSON_ERROR_RECURSION),
        ("JSON_ERROR_INF_OR_NAN", JSON_ERROR_INF_OR_NAN),
        ("JSON_ERROR_UNSUPPORTED_TYPE", JSON_ERROR_UNSUPPORTED_TYPE),
        ("JSON_ERROR_INVALID_PROPERTY_NAME", JSON_ERROR_INVALID_PROPERTY_NAME),
        ("JSON_ERROR_UTF16", JSON_ERROR_UTF16),
        ("JSON_ERROR_NON_BACKED_ENUM", JSON_ERROR_NON_BACKED_ENUM),
    ] {
        r.constant(name, Value::Int(v));
    }
    if r.interp().class_by_name(b"JsonSerializable").is_none() {
        r.interface("JsonSerializable")
            .abstract_method("jsonSerialize", nm!(0, Some(0), json_serialize_abstract))
            .finish();
    }
}

/// The placeholder body of the abstract `JsonSerializable::jsonSerialize()`.
fn json_serialize_abstract(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot call abstract method JsonSerializable::jsonSerialize()"))
}

/// php's `php_json_get_error_msg`.
fn error_text(code: i64) -> &'static str {
    match code {
        JSON_ERROR_NONE => "No error",
        JSON_ERROR_DEPTH => "Maximum stack depth exceeded",
        JSON_ERROR_STATE_MISMATCH => "State mismatch (invalid or malformed JSON)",
        JSON_ERROR_CTRL_CHAR => "Control character error, possibly incorrectly encoded",
        JSON_ERROR_SYNTAX => "Syntax error",
        JSON_ERROR_UTF8 => "Malformed UTF-8 characters, possibly incorrectly encoded",
        JSON_ERROR_RECURSION => "Recursion detected",
        JSON_ERROR_INF_OR_NAN => "Inf and NaN cannot be JSON encoded",
        JSON_ERROR_UNSUPPORTED_TYPE => "Type is not supported",
        JSON_ERROR_INVALID_PROPERTY_NAME => "The decoded property name is invalid",
        JSON_ERROR_UTF16 => "Single unpaired UTF-16 surrogate in unicode escape",
        JSON_ERROR_NON_BACKED_ENUM => "Non-backed enums have no default serialization",
        _ => "Unknown error",
    }
}

/// Record `code` as the request's last json error.
fn set_error(ctx: &mut Ctx, code: i64) {
    ctx.ext.json_last_error = code;
    ctx.ext.json_last_error_msg = error_text(code).to_string();
}

/// `throw new JsonException(msg, code)`.
fn json_exception(ctx: &mut Ctx, code: i64) -> Unwind {
    let msg = error_text(code);
    match ctx.class_by_name(b"JsonException") {
        Some(cid) => Unwind::Throw(ctx.create_throwable(cid, msg, code, None, None)),
        None => Unwind::exception("JsonException", msg),
    }
}

/// `json_last_error()`.
pub(crate) fn json_last_error(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(ctx.ext.json_last_error))
}

/// `json_last_error_msg()`.
pub(crate) fn json_last_error_msg(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(error_text(ctx.ext.json_last_error).as_bytes()))
}

/// zpp `int` for `$depth` / `$flags`.
fn int_arg(ctx: &mut Ctx, func: &str, n: usize, name: &str, v: &Value) -> Result<i64, Unwind> {
    match v {
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{func}(): Passing null to parameter #{n} (${name}) of type int is deprecated"
            ))?;
            Ok(0)
        }
        Value::Array(_) | Value::Object(_) | Value::Closure(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type int, {} given",
            rphp_runtime::value_name(v)
        ))),
        Value::Str(_) if !v.is_numeric() => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type int, string given"
        ))),
        other => Ok(other.to_int()),
    }
}

/// zpp `string` for `$json`.
fn str_arg(ctx: &mut Ctx, func: &str, n: usize, name: &str, v: &Value) -> Result<Vec<u8>, Unwind> {
    match v {
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{func}(): Passing null to parameter #{n} (${name}) of type string is deprecated"
            ))?;
            Ok(Vec::new())
        }
        Value::Array(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type string, array given"
        ))),
        Value::Object(_) | Value::Closure(_) => match ctx.to_string(v) {
            Ok(s) => Ok(s.as_bytes().to_vec()),
            Err(_) => Err(Unwind::type_error(format!(
                "{func}(): Argument #{n} (${name}) must be of type string, {} given",
                rphp_runtime::value_name(v)
            ))),
        },
        other => Ok(other.to_php_bytes()),
    }
}

// ---- json_encode ------------------------------------------------------------

/// PHP `json_encode($value, $flags = 0, $depth = 512)`: the JSON text, or
/// `false` when a value cannot be encoded (unless
/// `JSON_PARTIAL_OUTPUT_ON_ERROR` keeps the partial text, or
/// `JSON_THROW_ON_ERROR` throws `JsonException`).
pub(crate) fn json_encode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let flags = match args.get(1) {
        Some(v) => int_arg(ctx, "json_encode", 2, "flags", v)?,
        None => 0,
    };
    let depth = match args.get(2) {
        Some(v) => int_arg(ctx, "json_encode", 3, "depth", v)?,
        None => DEFAULT_DEPTH,
    };
    let mut enc = Encoder {
        flags,
        depth: 0,
        max_depth: depth,
        error: JSON_ERROR_NONE,
        precision: serialize_precision(ctx),
        seen_objects: Vec::new(),
        seen_refs: Vec::new(),
        out: Vec::new(),
    };
    let value = args[0].clone();
    let _ = enc.encode(ctx, &value)?;
    let partial = flags & JSON_PARTIAL_OUTPUT_ON_ERROR != 0;
    if flags & JSON_THROW_ON_ERROR == 0 || partial {
        set_error(ctx, enc.error);
        if enc.error != JSON_ERROR_NONE && !partial {
            return Ok(Value::Bool(false));
        }
    } else if enc.error != JSON_ERROR_NONE {
        return Err(json_exception(ctx, enc.error));
    }
    Ok(Value::Str(Str::from_vec(enc.out)))
}

/// php's `php_json_encoder` plus the output buffer and the recursion guards.
struct Encoder {
    flags: i64,
    depth: i64,
    max_depth: i64,
    error: i64,
    precision: i64,
    /// Objects on the encode path (ids), for `JSON_ERROR_RECURSION`.
    seen_objects: Vec<u32>,
    /// Reference cells on the encode path (`$a[] = &$a`).
    seen_refs: Vec<PhpRef>,
    out: Vec<u8>,
}

/// Whether the encoding of a value succeeded (`SUCCESS`) or failed with
/// `enc.error` set (`FAILURE`); a `FAILURE` aborts the enclosing container
/// unless `JSON_PARTIAL_OUTPUT_ON_ERROR` is set.
type EncResult = Result<bool, Unwind>;

impl Encoder {
    fn flag(&self, f: i64) -> bool {
        self.flags & f != 0
    }

    fn pretty(&self) -> bool {
        self.flag(JSON_PRETTY_PRINT)
    }

    fn pretty_char(&mut self, c: u8) {
        if self.pretty() {
            self.out.push(c);
        }
    }

    fn pretty_indent(&mut self) {
        if self.pretty() {
            for _ in 0..self.depth {
                self.out.extend_from_slice(b"    ");
            }
        }
    }

    /// `php_json_encode_zval`.
    fn encode(&mut self, ctx: &mut Ctx, v: &Value) -> EncResult {
        match v {
            Value::Null | Value::Uninit => self.out.extend_from_slice(b"null"),
            Value::Bool(true) => self.out.extend_from_slice(b"true"),
            Value::Bool(false) => self.out.extend_from_slice(b"false"),
            Value::Int(i) => self.out.extend_from_slice(i.to_string().as_bytes()),
            Value::Float(f) => {
                if f.is_finite() {
                    self.encode_double(*f);
                } else {
                    self.error = JSON_ERROR_INF_OR_NAN;
                    self.out.push(b'0');
                }
            }
            Value::Str(s) => return Ok(self.escape_string(s.as_bytes(), self.flags, false)),
            Value::Object(o) => {
                // A lazy object initializes first; a proxy encodes its real
                // instance.
                let o = &ctx.lazy_resolve(&o.clone())?;
                if let Some(iface) = ctx.class_by_name(b"JsonSerializable") {
                    if ctx.object_instanceof(o, iface) {
                        return self.encode_serializable(ctx, o);
                    }
                }
                return self.encode_object(ctx, o);
            }
            Value::Array(a) => return self.encode_array(ctx, a),
            // A closure is an object without public properties.
            Value::Closure(_) => self.out.extend_from_slice(b"{}"),
            Value::Ref(r) => {
                if self.seen_refs.iter().any(|s| s.ptr_eq(r)) {
                    self.error = JSON_ERROR_RECURSION;
                    self.out.extend_from_slice(b"null");
                    return Ok(false);
                }
                self.seen_refs.push(r.clone());
                let inner = r.get();
                let ok = self.encode(ctx, &inner)?;
                self.seen_refs.pop();
                return Ok(ok);
            }
            Value::Resource(_) => {
                self.error = JSON_ERROR_UNSUPPORTED_TYPE;
                if self.flag(JSON_PARTIAL_OUTPUT_ON_ERROR) {
                    self.out.extend_from_slice(b"null");
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `php_json_encode_double`: `serialize_precision` layout with `e`, plus
    /// `.0` under `JSON_PRESERVE_ZERO_FRACTION` when no `.` is present.
    fn encode_double(&mut self, f: f64) {
        let mut num = php_gcvt(f, self.precision, b'e');
        if self.flag(JSON_PRESERVE_ZERO_FRACTION) && !num.contains('.') {
            num.push_str(".0");
        }
        self.out.extend_from_slice(num.as_bytes());
    }

    /// The depth check php runs after a container's contents: exceeding
    /// `$depth` is an error, fatal to the container unless partial output.
    fn check_depth(&mut self) -> bool {
        if self.depth > self.max_depth {
            self.error = JSON_ERROR_DEPTH;
            if !self.flag(JSON_PARTIAL_OUTPUT_ON_ERROR) {
                return false;
            }
        }
        true
    }

    /// `php_json_encode_array` for a PHP array: a JSON array when it is a
    /// list (and `JSON_FORCE_OBJECT` is off), else an object keyed by the
    /// keys' string forms.
    fn encode_array(&mut self, ctx: &mut Ctx, a: &Array) -> EncResult {
        let as_list = !self.flag(JSON_FORCE_OBJECT) && a.is_list();
        self.out.push(if as_list { b'[' } else { b'{' });
        self.depth += 1;
        let mut need_comma = false;
        let partial = self.flag(JSON_PARTIAL_OUTPUT_ON_ERROR);
        for (k, v) in a.iter() {
            if need_comma {
                self.out.push(b',');
            } else {
                need_comma = true;
            }
            self.pretty_char(b'\n');
            self.pretty_indent();
            if !as_list {
                match k {
                    ArrayKey::Str(s) => {
                        if !self.escape_string(s, self.flags & !JSON_NUMERIC_CHECK, true) && !partial {
                            return Ok(false);
                        }
                    }
                    ArrayKey::Int(i) => {
                        self.out.push(b'"');
                        self.out.extend_from_slice(i.to_string().as_bytes());
                        self.out.push(b'"');
                    }
                }
                self.out.push(b':');
                self.pretty_char(b' ');
            }
            if !self.encode(ctx, v)? && !partial {
                return Ok(false);
            }
        }
        if !self.check_depth() {
            return Ok(false);
        }
        self.depth -= 1;
        if need_comma {
            self.pretty_char(b'\n');
            self.pretty_indent();
        }
        self.out.push(if as_list { b']' } else { b'}' });
        Ok(true)
    }

    /// `php_json_encode_array` for an object: its public, initialized
    /// properties in declaration order then dynamic ones, with recursion
    /// detection on the object itself.
    fn encode_object(&mut self, ctx: &mut Ctx, o: &Object) -> EncResult {
        if self.seen_objects.contains(&o.id()) {
            self.error = JSON_ERROR_RECURSION;
            self.out.extend_from_slice(b"null");
            return Ok(false);
        }
        self.seen_objects.push(o.id());
        let props: Vec<(Vec<u8>, Value)> = o.with_data(|d| {
            d.props_in_order()
                .filter(|p| p.vis == Vis::Public && !p.value.is_uninit())
                .map(|p| (p.name.to_vec(), p.value.clone()))
                .collect()
        });
        self.out.push(b'{');
        self.depth += 1;
        let mut need_comma = false;
        let partial = self.flag(JSON_PARTIAL_OUTPUT_ON_ERROR);
        let mut ok = true;
        for (name, v) in &props {
            if need_comma {
                self.out.push(b',');
            } else {
                need_comma = true;
            }
            self.pretty_char(b'\n');
            self.pretty_indent();
            if !self.escape_string(name, self.flags & !JSON_NUMERIC_CHECK, true) && !partial {
                ok = false;
                break;
            }
            self.out.push(b':');
            self.pretty_char(b' ');
            if !self.encode(ctx, v)? && !partial {
                ok = false;
                break;
            }
        }
        self.seen_objects.pop();
        if !ok {
            return Ok(false);
        }
        if !self.check_depth() {
            return Ok(false);
        }
        self.depth -= 1;
        if need_comma {
            self.pretty_char(b'\n');
            self.pretty_indent();
        }
        self.out.push(b'}');
        Ok(true)
    }

    /// `php_json_encode_serializable_object`: call `jsonSerialize()` and
    /// encode its result (`return $this` encodes the object's properties).
    fn encode_serializable(&mut self, ctx: &mut Ctx, o: &Object) -> EncResult {
        if self.seen_objects.contains(&o.id()) {
            self.error = JSON_ERROR_RECURSION;
            if self.flag(JSON_PARTIAL_OUTPUT_ON_ERROR) {
                self.out.extend_from_slice(b"null");
            }
            return Ok(false);
        }
        self.seen_objects.push(o.id());
        let r = ctx.call_method(o, b"jsonSerialize", &[]);
        let r = match r {
            Ok(v) => v,
            Err(u) => {
                self.seen_objects.pop();
                return Err(u);
            }
        };
        
        match &r {
            Value::Object(ret) if ret.ptr_eq(o) => {
                self.seen_objects.pop();
                self.encode_object(ctx, o)
            }
            _ => {
                let res = self.encode(ctx, &r);
                self.seen_objects.pop();
                res
            }
        }
    }

    /// `php_json_escape_string`: `true` on success; on invalid UTF-8 (without
    /// the ignore/substitute flags) the partial literal is dropped, the error
    /// recorded and, under `JSON_PARTIAL_OUTPUT_ON_ERROR`, `null` (or `""`
    /// for a key) written instead.
    fn escape_string(&mut self, s: &[u8], flags: i64, is_key: bool) -> bool {
        if s.is_empty() {
            self.out.extend_from_slice(b"\"\"");
            return true;
        }
        if flags & JSON_NUMERIC_CHECK != 0 {
            // `is_numeric_string` (whole string, php 8 surrounding whitespace).
            let sv = Value::Str(Str::new(s));
            if sv.is_numeric() {
                match sv.to_number() {
                    Value::Int(i) => {
                        self.out.extend_from_slice(i.to_string().as_bytes());
                        return true;
                    }
                    Value::Float(f) if f.is_finite() => {
                        self.encode_double(f);
                        return true;
                    }
                    _ => {}
                }
            }
        }
        let checkpoint = self.out.len();
        self.out.push(b'"');
        let mut i = 0;
        while i < s.len() {
            let b = s[i];
            if b >= 0x80 {
                // `php_next_utf8_char`: a malformed sequence is charged as
                // one unit of php's length (one substitution per unit).
                let start = i;
                match crate::html::next_utf8_char(s, &mut i) {
                    Some(cp) => {
                        if flags & JSON_UNESCAPED_UNICODE != 0
                            && (flags & JSON_UNESCAPED_LINE_TERMINATORS != 0 || !(0x2028..=0x2029).contains(&cp))
                        {
                            self.out.extend_from_slice(&s[start..i]);
                        } else if cp >= 0x10000 {
                            let v = cp - 0x10000;
                            push_u_escape(&mut self.out, 0xD800 | (v >> 10));
                            push_u_escape(&mut self.out, 0xDC00 | (v & 0x3FF));
                        } else {
                            push_u_escape(&mut self.out, cp);
                        }
                    }
                    None => {
                        if flags & JSON_INVALID_UTF8_IGNORE != 0 {
                            // dropped
                        } else if flags & JSON_INVALID_UTF8_SUBSTITUTE != 0 {
                            if flags & JSON_UNESCAPED_UNICODE != 0 {
                                self.out.extend_from_slice("\u{fffd}".as_bytes());
                            } else {
                                self.out.extend_from_slice(b"\\ufffd");
                            }
                        } else {
                            self.out.truncate(checkpoint);
                            self.error = JSON_ERROR_UTF8;
                            if flags & JSON_PARTIAL_OUTPUT_ON_ERROR != 0 {
                                // A value becomes `null`, a key the empty name.
                                self.out.extend_from_slice(if is_key { b"\"\"" } else { b"null" });
                            }
                            return false;
                        }
                    }
                }
                continue;
            }
            match b {
                b'"' => self.out.extend_from_slice(if flags & JSON_HEX_QUOT != 0 { b"\\u0022" } else { b"\\\"" }),
                b'\\' => self.out.extend_from_slice(b"\\\\"),
                b'/' => {
                    if flags & JSON_UNESCAPED_SLASHES != 0 {
                        self.out.push(b'/');
                    } else {
                        self.out.extend_from_slice(b"\\/");
                    }
                }
                0x08 => self.out.extend_from_slice(b"\\b"),
                0x0c => self.out.extend_from_slice(b"\\f"),
                b'\n' => self.out.extend_from_slice(b"\\n"),
                b'\r' => self.out.extend_from_slice(b"\\r"),
                b'\t' => self.out.extend_from_slice(b"\\t"),
                b'<' => self.out.extend_from_slice(if flags & JSON_HEX_TAG != 0 { b"\\u003C" } else { b"<" }),
                b'>' => self.out.extend_from_slice(if flags & JSON_HEX_TAG != 0 { b"\\u003E" } else { b">" }),
                b'&' => self.out.extend_from_slice(if flags & JSON_HEX_AMP != 0 { b"\\u0026" } else { b"&" }),
                b'\'' => self.out.extend_from_slice(if flags & JSON_HEX_APOS != 0 { b"\\u0027" } else { b"'" }),
                c if c < 0x20 => push_u_escape(&mut self.out, u32::from(c)),
                c => self.out.push(c),
            }
            i += 1;
        }
        self.out.push(b'"');
        true
    }
}

/// Append a `\uXXXX` escape with lowercase hex (php's casing).
fn push_u_escape(out: &mut Vec<u8>, cp: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.extend_from_slice(b"\\u");
    out.push(HEX[((cp >> 12) & 0xF) as usize]);
    out.push(HEX[((cp >> 8) & 0xF) as usize]);
    out.push(HEX[((cp >> 4) & 0xF) as usize]);
    out.push(HEX[(cp & 0xF) as usize]);
}

/// Decode one well-formed UTF-8 sequence starting at `i` (php's
/// `php_next_utf8_char` / the scanner's `UTF8` classes: no overlongs, no
/// surrogates, at most U+10FFFF): `(code point, byte length)`.
fn utf8_char(s: &[u8], i: usize) -> Option<(u32, usize)> {
    let b0 = *s.get(i)?;
    let cont = |k: usize| s.get(i + k).copied().filter(|c| c & 0xC0 == 0x80).map(|c| u32::from(c & 0x3F));
    match b0 {
        0x00..=0x7F => Some((u32::from(b0), 1)),
        0xC2..=0xDF => Some(((u32::from(b0 & 0x1F) << 6) | cont(1)?, 2)),
        0xE0..=0xEF => {
            let c1 = cont(1)?;
            let c2 = cont(2)?;
            let cp = (u32::from(b0 & 0x0F) << 12) | (c1 << 6) | c2;
            if cp < 0x800 || (0xD800..=0xDFFF).contains(&cp) {
                return None;
            }
            Some((cp, 3))
        }
        0xF0..=0xF4 => {
            let c1 = cont(1)?;
            let c2 = cont(2)?;
            let c3 = cont(3)?;
            let cp = (u32::from(b0 & 0x07) << 18) | (c1 << 12) | (c2 << 6) | c3;
            if !(0x10000..=0x10FFFF).contains(&cp) {
                return None;
            }
            Some((cp, 4))
        }
        _ => None,
    }
}

// ---- json_decode ------------------------------------------------------------

/// PHP `json_decode($json, $associative = null, $depth = 512, $flags = 0)`.
pub(crate) fn json_decode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let json = str_arg(ctx, "json_decode", 1, "json", &args[0])?;
    let assoc = match args.get(1) {
        None | Some(Value::Null) => None,
        Some(Value::Array(_) | Value::Object(_) | Value::Closure(_)) => {
            return Err(Unwind::type_error(format!(
                "json_decode(): Argument #2 ($associative) must be of type ?bool, {} given",
                rphp_runtime::value_name(&args[1])
            )))
        }
        Some(v) => Some(v.to_bool()),
    };
    let depth = match args.get(2) {
        Some(v) => int_arg(ctx, "json_decode", 3, "depth", v)?,
        None => DEFAULT_DEPTH,
    };
    let mut flags = match args.get(3) {
        Some(v) => int_arg(ctx, "json_decode", 4, "flags", v)?,
        None => 0,
    };
    let throw = flags & JSON_THROW_ON_ERROR != 0;
    if !throw {
        set_error(ctx, JSON_ERROR_NONE);
    }
    if json.is_empty() {
        if throw {
            return Err(json_exception(ctx, JSON_ERROR_SYNTAX));
        }
        set_error(ctx, JSON_ERROR_SYNTAX);
        return Ok(Value::Null);
    }
    if depth <= 0 {
        return Err(Unwind::value_error("json_decode(): Argument #3 ($depth) must be greater than 0"));
    }
    if depth > i64::from(i32::MAX) {
        return Err(Unwind::value_error(format!(
            "json_decode(): Argument #3 ($depth) must be less than {}",
            i32::MAX
        )));
    }
    // For BC the bool $associative overrides the JSON_OBJECT_AS_ARRAY bit.
    match assoc {
        Some(true) => flags |= JSON_OBJECT_AS_ARRAY,
        Some(false) => flags &= !JSON_OBJECT_AS_ARRAY,
        None => {}
    }
    let mut p = Parser { b: &json, pos: 0, max_depth: depth, flags, validate_only: false, stdclass: ctx.well_known.stdclass };
    match p.parse_document(ctx) {
        Ok(v) => Ok(v),
        Err(code) => {
            if throw {
                return Err(json_exception(ctx, code));
            }
            set_error(ctx, code);
            Ok(Value::Null)
        }
    }
}

/// PHP 8.3 `json_validate($json, $depth = 512, $flags = 0)`: whether the
/// text parses, recording the error code without building a value.
pub(crate) fn json_validate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let json = str_arg(ctx, "json_validate", 1, "json", &args[0])?;
    let depth = match args.get(1) {
        Some(v) => int_arg(ctx, "json_validate", 2, "depth", v)?,
        None => DEFAULT_DEPTH,
    };
    let flags = match args.get(2) {
        Some(v) => int_arg(ctx, "json_validate", 3, "flags", v)?,
        None => 0,
    };
    set_error(ctx, JSON_ERROR_NONE);
    if json.is_empty() {
        set_error(ctx, JSON_ERROR_SYNTAX);
        return Ok(Value::Bool(false));
    }
    if depth <= 0 {
        return Err(Unwind::value_error("json_validate(): Argument #2 ($depth) must be greater than 0"));
    }
    if depth > i64::from(i32::MAX) {
        return Err(Unwind::value_error(format!(
            "json_validate(): Argument #2 ($depth) must be less than {}",
            i32::MAX
        )));
    }
    if flags != 0 && flags != JSON_INVALID_UTF8_IGNORE {
        return Err(Unwind::value_error(
            "json_validate(): Argument #3 ($flags) must be a valid flag (allowed flags: JSON_INVALID_UTF8_IGNORE)",
        ));
    }
    let mut p = Parser { b: &json, pos: 0, max_depth: depth, flags, validate_only: true, stdclass: None };
    match p.parse_document(ctx) {
        Ok(_) => Ok(Value::Bool(true)),
        Err(code) => {
            set_error(ctx, code);
            Ok(Value::Bool(false))
        }
    }
}

/// A cursor over the JSON bytes (php's scanner + parser in one).
struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    max_depth: i64,
    flags: i64,
    /// `json_validate`: classify without building values.
    validate_only: bool,
    /// The `stdClass` id for object results (`None` decodes to arrays).
    stdclass: Option<u32>,
}

/// A parse failure: the `JSON_ERROR_*` code.
type ParseResult<T> = Result<T, i64>;

impl Parser<'_> {
    fn objects_as_arrays(&self) -> bool {
        self.flags & JSON_OBJECT_AS_ARRAY != 0 || self.stdclass.is_none()
    }

    /// `ws` in the scanner: space, tab, LF, CR.
    fn skip_ws(&mut self) {
        while let Some(&c) = self.b.get(self.pos) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// The whole document: one value, then only whitespace to the end.
    fn parse_document(&mut self, ctx: &mut Ctx) -> ParseResult<Value> {
        let v = self.parse_value(ctx, 1)?;
        self.skip_ws();
        if self.pos != self.b.len() {
            return Err(self.stray_error());
        }
        Ok(v)
    }

    /// The scanner's classification of an unexpected byte in value
    /// position: NUL / control → `CTRL_CHAR`, invalid UTF-8 → `UTF8`, any
    /// other token → `SYNTAX`.
    fn stray_error(&self) -> i64 {
        match self.b.get(self.pos) {
            None => JSON_ERROR_SYNTAX,
            Some(&c) if c < 0x20 => JSON_ERROR_CTRL_CHAR,
            Some(&c) if c >= 0x80 && utf8_char(self.b, self.pos).is_none() => JSON_ERROR_UTF8,
            Some(_) => JSON_ERROR_SYNTAX,
        }
    }

    /// One value at nesting `depth` (the document is 1).
    fn parse_value(&mut self, ctx: &mut Ctx, depth: i64) -> ParseResult<Value> {
        self.skip_ws();
        match self.b.get(self.pos) {
            None => Err(JSON_ERROR_SYNTAX),
            Some(b'{') => self.parse_object(ctx, depth),
            Some(b'[') => self.parse_array(ctx, depth),
            Some(b'"') => self.parse_string().map(|s| Value::Str(Str::from_vec(s))),
            Some(b't') => self.parse_lit(b"true", Value::Bool(true)),
            Some(b'f') => self.parse_lit(b"false", Value::Bool(false)),
            Some(b'n') => self.parse_lit(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(self.stray_error()),
        }
    }

    fn parse_lit(&mut self, word: &[u8], v: Value) -> ParseResult<Value> {
        if self.b[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(JSON_ERROR_SYNTAX)
        }
    }

    /// After a value inside a container: the separator or the closer.
    fn expect_sep(&mut self, close: u8) -> ParseResult<bool> {
        self.skip_ws();
        match self.b.get(self.pos) {
            Some(&b',') => {
                self.pos += 1;
                Ok(true)
            }
            Some(&c) if c == close => {
                self.pos += 1;
                Ok(false)
            }
            _ => Err(self.stray_error()),
        }
    }

    fn parse_array(&mut self, ctx: &mut Ctx, depth: i64) -> ParseResult<Value> {
        if depth >= self.max_depth {
            return Err(JSON_ERROR_DEPTH);
        }
        self.pos += 1;
        let mut arr = Array::new();
        self.skip_ws();
        if self.b.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(Value::Array(arr));
        }
        loop {
            let v = self.parse_value(ctx, depth + 1)?;
            if !self.validate_only {
                arr.push(v);
            }
            if !self.expect_sep(b']')? {
                return Ok(Value::Array(arr));
            }
        }
    }

    /// An object. As in php's parser the `stdClass` instance is created when
    /// the first member's value has been parsed (or at `}` for `{}`), so
    /// object handles are numbered in php's order.
    fn parse_object(&mut self, ctx: &mut Ctx, depth: i64) -> ParseResult<Value> {
        if depth >= self.max_depth {
            return Err(JSON_ERROR_DEPTH);
        }
        self.pos += 1;
        let want_object = !self.objects_as_arrays() && !self.validate_only;
        let mut arr = Array::new();
        let mut obj: Option<Object> = None;
        self.skip_ws();
        if self.b.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(self.finish_object(ctx, arr, obj, want_object));
        }
        loop {
            self.skip_ws();
            if self.b.get(self.pos) != Some(&b'"') {
                return Err(self.stray_error());
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.b.get(self.pos) != Some(&b':') {
                return Err(self.stray_error());
            }
            self.pos += 1;
            let v = self.parse_value(ctx, depth + 1)?;
            if want_object {
                let o = match &obj {
                    Some(o) => o,
                    None => obj.insert(ctx.instantiate(self.stdclass.expect("stdClass registered"))),
                };
                // php: a property name starting with NUL cannot exist.
                if key.first() == Some(&0) {
                    return Err(JSON_ERROR_INVALID_PROPERTY_NAME);
                }
                o.dyn_set(&key, v);
            } else if !self.validate_only {
                if let Some(k) = array_key(&Value::Str(Str::from_vec(key))) {
                    arr.set(k, v);
                }
            }
            if !self.expect_sep(b'}')? {
                return Ok(self.finish_object(ctx, arr, obj, want_object));
            }
        }
    }

    fn finish_object(&self, ctx: &mut Ctx, arr: Array, obj: Option<Object>, want_object: bool) -> Value {
        match obj {
            Some(o) => Value::Object(o),
            None if want_object => Value::Object(ctx.instantiate(self.stdclass.expect("stdClass registered"))),
            None => Value::Array(arr),
        }
    }

    /// A `"`-delimited string (opening quote at `pos`) decoded to bytes.
    fn parse_string(&mut self) -> ParseResult<Vec<u8>> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            // The end of input inside a string is the scanner's NUL → CTRL_CHAR.
            let c = *self.b.get(self.pos).ok_or(JSON_ERROR_CTRL_CHAR)?;
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    let e = *self.b.get(self.pos + 1).ok_or(JSON_ERROR_SYNTAX)?;
                    self.pos += 2;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let cp = self.parse_hex4().ok_or(JSON_ERROR_SYNTAX)?;
                            let scalar = if (0xD800..=0xDBFF).contains(&cp) {
                                // A high surrogate must be followed by `\uDC00..\uDFFF`.
                                if self.b.get(self.pos) != Some(&b'\\') || self.b.get(self.pos + 1) != Some(&b'u') {
                                    return Err(JSON_ERROR_UTF16);
                                }
                                let save = self.pos;
                                self.pos += 2;
                                let lo = match self.parse_hex4() {
                                    Some(lo) if (0xDC00..=0xDFFF).contains(&lo) => lo,
                                    _ => {
                                        self.pos = save;
                                        return Err(JSON_ERROR_UTF16);
                                    }
                                };
                                0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..=0xDFFF).contains(&cp) {
                                return Err(JSON_ERROR_UTF16);
                            } else {
                                cp
                            };
                            if let Some(ch) = char::from_u32(scalar) {
                                let mut buf = [0u8; 4];
                                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                        }
                        _ => return Err(JSON_ERROR_SYNTAX),
                    }
                }
                _ if c < 0x20 => return Err(JSON_ERROR_CTRL_CHAR),
                _ if c < 0x80 => {
                    out.push(c);
                    self.pos += 1;
                }
                _ => match utf8_char(self.b, self.pos) {
                    Some((_, len)) => {
                        out.extend_from_slice(&self.b[self.pos..self.pos + len]);
                        self.pos += len;
                    }
                    None => {
                        if self.flags & JSON_INVALID_UTF8_IGNORE != 0 {
                            // dropped, one byte at a time
                        } else if self.flags & JSON_INVALID_UTF8_SUBSTITUTE != 0 {
                            out.extend_from_slice("\u{fffd}".as_bytes());
                        } else {
                            return Err(JSON_ERROR_UTF8);
                        }
                        self.pos += 1;
                    }
                },
            }
        }
    }

    /// Exactly four hex digits.
    fn parse_hex4(&mut self) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..4 {
            let d = *self.b.get(self.pos)?;
            let nibble = match d {
                b'0'..=b'9' => u32::from(d - b'0'),
                b'a'..=b'f' => u32::from(d - b'a' + 10),
                b'A'..=b'F' => u32::from(d - b'A' + 10),
                _ => return None,
            };
            v = (v << 4) | nibble;
            self.pos += 1;
        }
        Some(v)
    }

    /// A number per the scanner's grammar (`-? (0 | [1-9][0-9]*) frac? exp?`).
    /// An integer of 19+ digits beyond `i64` becomes a float, or a string
    /// under `JSON_BIGINT_AS_STRING`.
    fn parse_number(&mut self) -> ParseResult<Value> {
        let start = self.pos;
        let n = self.b.len();
        let mut i = self.pos;
        let mut is_float = false;
        let negative = self.b[i] == b'-';
        if negative {
            i += 1;
        }
        match self.b.get(i) {
            Some(b'0') => i += 1,
            Some(d) if d.is_ascii_digit() => {
                while i < n && self.b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => return Err(JSON_ERROR_SYNTAX),
        }
        let int_end = i;
        if i < n && self.b[i] == b'.' {
            if !self.b.get(i + 1).is_some_and(u8::is_ascii_digit) {
                self.pos = i + 1;
                return Err(self.stray_error());
            }
            is_float = true;
            i += 1;
            while i < n && self.b[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i < n && (self.b[i] == b'e' || self.b[i] == b'E') {
            let mut j = i + 1;
            if j < n && (self.b[j] == b'+' || self.b[j] == b'-') {
                j += 1;
            }
            if !self.b.get(j).is_some_and(u8::is_ascii_digit) {
                self.pos = i;
                return Err(JSON_ERROR_SYNTAX);
            }
            is_float = true;
            i = j;
            while i < n && self.b[i].is_ascii_digit() {
                i += 1;
            }
        }
        let text = std::str::from_utf8(&self.b[start..i]).map_err(|_| JSON_ERROR_SYNTAX)?;
        self.pos = i;
        if is_float {
            return Ok(Value::Float(text.parse::<f64>().unwrap_or(0.0)));
        }
        // php's bigint rule: 19 digits compare against LONG_MIN's magnitude.
        let digits = int_end - start - usize::from(negative);
        const MAX_LEN: usize = 19;
        const LONG_MIN_DIGITS: &[u8] = b"9223372036854775808";
        let bigint = if digits > MAX_LEN {
            true
        } else if digits == MAX_LEN {
            let d = &self.b[start + usize::from(negative)..int_end];
            !(d < LONG_MIN_DIGITS || (d == LONG_MIN_DIGITS && negative))
        } else {
            false
        };
        if !bigint {
            Ok(Value::Int(text.parse::<i64>().unwrap_or(0)))
        } else if self.flags & JSON_BIGINT_AS_STRING != 0 {
            Ok(Value::string(text.as_bytes()))
        } else {
            Ok(Value::Float(text.parse::<f64>().unwrap_or(0.0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_classifier_matches_the_scanner() {
        assert_eq!(utf8_char("é".as_bytes(), 0), Some((0xE9, 2)));
        assert_eq!(utf8_char("😀".as_bytes(), 0), Some((0x1F600, 4)));
        assert_eq!(utf8_char(b"\xC0\x80", 0), None);
        assert_eq!(utf8_char(b"\xED\xA0\x80", 0), None);
        assert_eq!(utf8_char(b"\xF4\x90\x80\x80", 0), None);
        assert_eq!(utf8_char(b"\xE2\x82", 0), None);
    }

    #[test]
    fn u_escapes_are_lowercase() {
        let mut out = Vec::new();
        push_u_escape(&mut out, 0x1F);
        assert_eq!(out, b"\\u001f");
    }
}
