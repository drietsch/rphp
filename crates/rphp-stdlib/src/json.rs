//! `json` extension — a hand-rolled `json_encode` / `json_decode` (no serde, no
//! external crates). Output is byte-exact against stock PHP 8.5 with
//! `serialize_precision=-1` (the default): scalars, the shortest-round-trip
//! float form, slash- and unicode-escaping, and `JSON_PRETTY_PRINT`.
//!
//! Decoding is RFC-strict (rejects leading zeros, trailing garbage, raw control
//! bytes, lone surrogates, …) and returns `null` on any parse error, as PHP
//! does. Because the engine has no object value type yet, a JSON **object**
//! decodes to a string-keyed PHP **array** regardless of the `$assoc` argument
//! (PHP's default would return a `stdClass`); see the crate notes.
use rphp_value::{array_key, Array, ArrayKey, Str, Value};

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

/// `json_encode` flag bits (a subset of PHP's `JSON_*` constants). Passed as a
/// plain integer; only the three the encoder honours are named here.
const JSON_UNESCAPED_SLASHES: i64 = 64;
const JSON_PRETTY_PRINT: i64 = 128;
const JSON_UNESCAPED_UNICODE: i64 = 256;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("json_encode", 1, Some(2), json_encode),
    nf!("json_decode", 1, Some(4), json_decode),
];

// ---- json_encode ------------------------------------------------------------

/// PHP `json_encode($value, $flags = 0)`. Returns the JSON string, or `false`
/// when a value cannot be encoded (a non-finite float, or a byte string that is
/// not valid UTF-8) — matching PHP, which fails the whole call in that case.
pub(crate) fn json_encode(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let flags = args.get(1).map_or(0, Value::to_int);
    let mut out = Vec::new();
    match encode(&mut out, &args[0], flags, 0) {
        Ok(()) => Ok(Value::Str(Str::from_vec(out))),
        Err(()) => Ok(Value::Bool(false)),
    }
}

/// Serialize one value at nesting `depth` (used for pretty-print indentation).
/// `Err(())` aborts the whole encode (PHP returns `false`).
fn encode(out: &mut Vec<u8>, v: &Value, flags: i64, depth: usize) -> Result<(), ()> {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Float(f) => {
            // PHP cannot represent NaN/Inf in JSON and fails the call.
            if !f.is_finite() {
                return Err(());
            }
            out.extend_from_slice(format_double(*f).as_bytes());
        }
        Value::Str(s) => encode_string(out, s.as_bytes(), flags)?,
        Value::Array(a) => encode_array(out, a, flags, depth)?,
        // An object with no public properties encodes as `{}` (a closure here).
        Value::Closure(_) => out.extend_from_slice(b"{}"),
    }
    Ok(())
}

/// A PHP array is a JSON **array** iff it is a list — integer keys `0..n-1` in
/// order; otherwise it is a JSON **object** keyed by each key's string form.
fn is_list(a: &Array) -> bool {
    for (i, (k, _)) in a.iter().enumerate() {
        match k {
            ArrayKey::Int(n) if *n == i as i64 => {}
            _ => return false,
        }
    }
    true
}

fn encode_array(out: &mut Vec<u8>, a: &Array, flags: i64, depth: usize) -> Result<(), ()> {
    let pretty = flags & JSON_PRETTY_PRINT != 0;
    let list = is_list(a);
    let (open, close) = if list { (b'[', b']') } else { (b'{', b'}') };
    out.push(open);
    // Empty container stays compact even under JSON_PRETTY_PRINT: "[]" / "{}".
    if a.is_empty() {
        out.push(close);
        return Ok(());
    }
    for (i, (k, v)) in a.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        if pretty {
            out.push(b'\n');
            indent(out, (depth + 1) * 4);
        }
        if !list {
            // Object keys are JSON strings (int keys use their decimal form).
            match k {
                ArrayKey::Int(n) => encode_string(out, n.to_string().as_bytes(), flags)?,
                ArrayKey::Str(b) => encode_string(out, b, flags)?,
            }
            out.push(b':');
            if pretty {
                out.push(b' ');
            }
        }
        encode(out, v, flags, depth + 1)?;
    }
    if pretty {
        out.push(b'\n');
        indent(out, depth * 4);
    }
    out.push(close);
    Ok(())
}

fn indent(out: &mut Vec<u8>, spaces: usize) {
    out.resize(out.len() + spaces, b' ');
}

/// Emit a JSON string literal. Assumes the bytes are UTF-8 (PHP's contract);
/// invalid UTF-8 aborts the encode with `Err(())` exactly as PHP does. Forward
/// slashes are escaped by default (PHP's quirk); non-ASCII is `\uXXXX`-escaped
/// (surrogate pair above U+FFFF) unless `JSON_UNESCAPED_UNICODE` is set.
fn encode_string(out: &mut Vec<u8>, bytes: &[u8], flags: i64) -> Result<(), ()> {
    let unescaped_slashes = flags & JSON_UNESCAPED_SLASHES != 0;
    let unescaped_unicode = flags & JSON_UNESCAPED_UNICODE != 0;
    let s = std::str::from_utf8(bytes).map_err(|_| ())?;
    out.push(b'"');
    for ch in s.chars() {
        let cp = ch as u32;
        match cp {
            0x22 => out.extend_from_slice(b"\\\""),
            0x5c => out.extend_from_slice(b"\\\\"),
            0x2f => {
                if unescaped_slashes {
                    out.push(b'/');
                } else {
                    out.extend_from_slice(b"\\/");
                }
            }
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0a => out.extend_from_slice(b"\\n"),
            0x0d => out.extend_from_slice(b"\\r"),
            0x09 => out.extend_from_slice(b"\\t"),
            _ if cp < 0x20 => push_u_escape(out, cp),
            _ if cp < 0x80 => out.push(cp as u8),
            _ if unescaped_unicode => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ if cp <= 0xFFFF => push_u_escape(out, cp),
            _ => {
                // Encode as a UTF-16 surrogate pair.
                let v = cp - 0x10000;
                push_u_escape(out, 0xD800 + (v >> 10));
                push_u_escape(out, 0xDC00 + (v & 0x3FF));
            }
        }
    }
    out.push(b'"');
    Ok(())
}

/// Append a `\uXXXX` escape with lowercase hex (PHP's casing).
fn push_u_escape(out: &mut Vec<u8>, cp: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.extend_from_slice(b"\\u");
    out.push(HEX[((cp >> 12) & 0xF) as usize]);
    out.push(HEX[((cp >> 8) & 0xF) as usize]);
    out.push(HEX[((cp >> 4) & 0xF) as usize]);
    out.push(HEX[(cp & 0xF) as usize]);
}

/// Shortest-round-trip float text matching PHP's `json_encode` under
/// `serialize_precision=-1`, i.e. a re-implementation of php-src's `php_gcvt`
/// with `ndigit = 17`. An integer-valued float prints without a decimal point
/// (`1.0` → `"1"`); `JSON_PRESERVE_ZERO_FRACTION` (which would append `.0`) is
/// not modelled. The caller has already rejected non-finite values.
fn format_double(f: f64) -> String {
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".to_string() } else { "0".to_string() };
    }
    let neg = f < 0.0;
    // Rust's `{:e}` (no precision) yields the shortest round-tripping mantissa
    // and a bare exponent — the same digit string `zend_dtoa(mode 0)` produces.
    let sci = format!("{:e}", f.abs());
    let (mant, exp_str) = sci.split_once('e').expect("LowerExp always has 'e'");
    let exp: i32 = exp_str.parse().expect("valid exponent");
    let digits: Vec<u8> = mant.bytes().filter(|&c| c != b'.').collect();
    // dtoa's `decpt`: the decimal point sits this many digits from the start.
    let decpt = exp + 1;
    let len = digits.len() as i32;

    let mut out = String::new();
    if neg {
        out.push('-');
    }
    const NDIGIT: i32 = 17;
    if !(-3..=NDIGIT).contains(&decpt) {
        // Exponential: "d.ddde±N".
        let mut e = decpt - 1;
        let esign = e < 0;
        if esign {
            e = -e;
        }
        out.push(digits[0] as char);
        out.push('.');
        if len == 1 {
            out.push('0');
        } else {
            for &b in &digits[1..] {
                out.push(b as char);
            }
        }
        out.push('e');
        out.push(if esign { '-' } else { '+' });
        out.push_str(&e.to_string());
    } else if decpt < 0 {
        // Pure fraction: "0.00…digits".
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        for &b in &digits {
            out.push(b as char);
        }
    } else {
        // Standard: integer part, padded with zeros past the digit string, then
        // any fractional remainder.
        let dp = decpt as usize;
        let mut src = 0usize;
        for _ in 0..dp {
            if src < digits.len() {
                out.push(digits[src] as char);
                src += 1;
            } else {
                out.push('0');
            }
        }
        if src < digits.len() {
            if src == 0 {
                out.push('0');
            }
            out.push('.');
            for &b in &digits[src..] {
                out.push(b as char);
            }
        }
    }
    out
}

// ---- json_decode ------------------------------------------------------------

/// PHP `json_decode($json, $associative = false, $depth = 512, $flags = 0)`.
/// Returns the decoded value, or `null` on any syntax error. A JSON object
/// always decodes to a string-keyed array (the engine has no object type yet),
/// so `$associative` and `$flags` are ignored. `$depth` must be positive.
pub(crate) fn json_decode(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let bytes = args[0].to_php_bytes();
    let depth = args.get(2).map_or(512, Value::to_int);
    if depth <= 0 {
        return Err(NativeError::new(
            "json_decode(): Argument #3 ($depth) must be greater than 0",
        ));
    }
    let mut p = Parser { b: &bytes, pos: 0, max_depth: depth };
    p.skip_ws();
    let val = match p.parse_value(1) {
        Some(v) => v,
        None => return Ok(Value::Null),
    };
    p.skip_ws();
    // Trailing non-whitespace after the value is an error.
    if p.pos != p.b.len() {
        return Ok(Value::Null);
    }
    Ok(val)
}

/// A cursor over the JSON byte slice. `max_depth` mirrors PHP's nesting limit:
/// entering a container costs one level and must not exceed it.
struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    max_depth: i64,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while let Some(&c) = self.b.get(self.pos) {
            // JSON insignificant whitespace.
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Parse one value. `depth` is this value's nesting level (the document is
    /// level 1); a container checks that its contents would not exceed
    /// `max_depth`.
    fn parse_value(&mut self, depth: i64) -> Option<Value> {
        self.skip_ws();
        match *self.b.get(self.pos)? {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth),
            b'"' => self.parse_string().map(|s| Value::Str(Str::from_vec(s))),
            b't' => self.parse_lit(b"true", Value::Bool(true)),
            b'f' => self.parse_lit(b"false", Value::Bool(false)),
            b'n' => self.parse_lit(b"null", Value::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => None,
        }
    }

    fn parse_lit(&mut self, word: &[u8], v: Value) -> Option<Value> {
        if self.b[self.pos..].starts_with(word) {
            self.pos += word.len();
            Some(v)
        } else {
            None
        }
    }

    fn parse_array(&mut self, depth: i64) -> Option<Value> {
        if depth + 1 > self.max_depth {
            return None;
        }
        self.pos += 1; // consume '['
        let mut arr = Array::new();
        self.skip_ws();
        if self.b.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Some(Value::Array(arr));
        }
        loop {
            let v = self.parse_value(depth + 1)?;
            arr.push(v);
            self.skip_ws();
            match self.b.get(self.pos) {
                Some(&b',') => self.pos += 1,
                Some(&b']') => {
                    self.pos += 1;
                    return Some(Value::Array(arr));
                }
                _ => return None,
            }
        }
    }

    fn parse_object(&mut self, depth: i64) -> Option<Value> {
        if depth + 1 > self.max_depth {
            return None;
        }
        self.pos += 1; // consume '{'
        let mut arr = Array::new();
        self.skip_ws();
        if self.b.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Some(Value::Array(arr));
        }
        loop {
            self.skip_ws();
            if self.b.get(self.pos) != Some(&b'"') {
                return None;
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.b.get(self.pos) != Some(&b':') {
                return None;
            }
            self.pos += 1;
            let v = self.parse_value(depth + 1)?;
            // Key coercion matches PHP array semantics (int-like strings become
            // int keys); a later duplicate key overwrites the earlier value.
            if let Some(k) = array_key(&Value::Str(Str::from_vec(key))) {
                arr.set(k, v);
            }
            self.skip_ws();
            match self.b.get(self.pos) {
                Some(&b',') => self.pos += 1,
                Some(&b'}') => {
                    self.pos += 1;
                    return Some(Value::Array(arr));
                }
                _ => return None,
            }
        }
    }

    /// Parse a `"`-delimited string into its decoded bytes. The opening quote is
    /// at `self.pos`. Rejects raw control bytes and bad escapes (returns `None`).
    fn parse_string(&mut self) -> Option<Vec<u8>> {
        self.pos += 1; // consume opening '"'
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.pos)?;
            self.pos += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let e = *self.b.get(self.pos)?;
                    self.pos += 1;
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
                            let cp = self.parse_hex4()?;
                            let scalar = if (0xD800..=0xDBFF).contains(&cp) {
                                // High surrogate: must be followed by `\uDC00..`.
                                if self.b.get(self.pos) != Some(&b'\\')
                                    || self.b.get(self.pos + 1) != Some(&b'u')
                                {
                                    return None;
                                }
                                self.pos += 2;
                                let lo = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..=0xDFFF).contains(&cp) {
                                return None; // lone low surrogate
                            } else {
                                cp
                            };
                            push_utf8(&mut out, scalar);
                        }
                        _ => return None,
                    }
                }
                // Unescaped control bytes are illegal in a JSON string.
                _ if c < 0x20 => return None,
                // Any other byte (incl. UTF-8 continuation bytes) passes through.
                _ => out.push(c),
            }
        }
    }

    /// Read exactly four hex digits as a code unit.
    fn parse_hex4(&mut self) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..4 {
            let d = *self.b.get(self.pos)?;
            let nibble = match d {
                b'0'..=b'9' => (d - b'0') as u32,
                b'a'..=b'f' => (d - b'a' + 10) as u32,
                b'A'..=b'F' => (d - b'A' + 10) as u32,
                _ => return None,
            };
            v = (v << 4) | nibble;
            self.pos += 1;
        }
        Some(v)
    }

    /// Parse a JSON number (RFC grammar — no leading zeros, no leading `+`,
    /// digits required around `.` and after `e`). Integral and i64-representable
    /// → `Int`, otherwise `Float`.
    fn parse_number(&mut self) -> Option<Value> {
        let start = self.pos;
        let n = self.b.len();
        let mut i = self.pos;
        let mut is_float = false;
        if self.b[i] == b'-' {
            i += 1;
        }
        // Integer part: a lone "0", or [1-9][0-9]*.
        match self.b.get(i) {
            Some(b'0') => i += 1,
            Some(d) if d.is_ascii_digit() => {
                while i < n && self.b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => return None,
        }
        // Fraction.
        if i < n && self.b[i] == b'.' {
            is_float = true;
            i += 1;
            if i >= n || !self.b[i].is_ascii_digit() {
                return None;
            }
            while i < n && self.b[i].is_ascii_digit() {
                i += 1;
            }
        }
        // Exponent.
        if i < n && (self.b[i] == b'e' || self.b[i] == b'E') {
            is_float = true;
            i += 1;
            if i < n && (self.b[i] == b'+' || self.b[i] == b'-') {
                i += 1;
            }
            if i >= n || !self.b[i].is_ascii_digit() {
                return None;
            }
            while i < n && self.b[i].is_ascii_digit() {
                i += 1;
            }
        }
        // The numeric run is pure ASCII, so this never fails.
        let s = std::str::from_utf8(&self.b[start..i]).ok()?;
        self.pos = i;
        if is_float {
            s.parse::<f64>().ok().map(Value::Float)
        } else {
            // An integer literal that overflows i64 promotes to float, as PHP.
            match s.parse::<i64>() {
                Ok(v) => Some(Value::Int(v)),
                Err(_) => s.parse::<f64>().ok().map(Value::Float),
            }
        }
    }
}

/// Append a Unicode scalar value to `out` as UTF-8. `cp` is never a surrogate
/// (the caller resolves pairs first), so the conversion always succeeds.
fn push_utf8(out: &mut Vec<u8>, cp: u32) {
    if let Some(ch) = char::from_u32(cp) {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}
