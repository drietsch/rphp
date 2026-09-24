//! The `filter` extension (php-src `ext/filter`): `filter_var` and friends.
//!
//! php's model is one function with a *filter id* and an options argument that
//! is either a bare flags int or an array `['flags' => …, 'options' => […]]`.
//! A failed validation yields `false`, or the `default` option when one is
//! given, or `null` under `FILTER_NULL_ON_FAILURE` — that three-way outcome is
//! the part callers actually depend on (Symfony's `ParameterBag::getInt`
//! leans on it), so it is modelled exactly.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("filter_var", 1, Some(3), filter_var),
    nf!("filter_var_array", 1, Some(3), filter_var_array),
    nf!("filter_has_var", 2, Some(2), filter_has_var),
    nf!("filter_input", 2, Some(4), filter_input),
    nf!("filter_input_array", 1, Some(3), filter_input_array),
    nf!("filter_list", 0, Some(0), filter_list),
    nf!("filter_id", 1, Some(1), filter_id),
];

// php's filter ids, from `php -r 'echo FILTER_VALIDATE_INT;'`.
const VALIDATE_INT: i64 = 257;
const VALIDATE_BOOL: i64 = 258;
const VALIDATE_FLOAT: i64 = 259;
const VALIDATE_REGEXP: i64 = 272;
const VALIDATE_DOMAIN: i64 = 277;
const VALIDATE_URL: i64 = 273;
const VALIDATE_EMAIL: i64 = 274;
const VALIDATE_IP: i64 = 275;
const VALIDATE_MAC: i64 = 276;
const DEFAULT: i64 = 516;
const UNSAFE_RAW: i64 = 516;
const SANITIZE_STRING: i64 = 513;
const SANITIZE_ENCODED: i64 = 514;
const SANITIZE_SPECIAL_CHARS: i64 = 515;
const SANITIZE_EMAIL: i64 = 517;
const SANITIZE_URL: i64 = 518;
const SANITIZE_NUMBER_INT: i64 = 519;
const SANITIZE_NUMBER_FLOAT: i64 = 520;
const SANITIZE_ADD_SLASHES: i64 = 523;
const SANITIZE_FULL_SPECIAL_CHARS: i64 = 522;

const CALLBACK: i64 = 1024;
const NULL_ON_FAILURE: i64 = 134_217_728;
const REQUIRE_SCALAR: i64 = 33_554_432;
const REQUIRE_ARRAY: i64 = 16_777_216;
const FORCE_ARRAY: i64 = 67_108_864;
const FLAG_ALLOW_OCTAL: i64 = 1;
const FLAG_ALLOW_HEX: i64 = 2;
const FLAG_ALLOW_FRACTION: i64 = 4096;
const FLAG_ALLOW_THOUSAND: i64 = 8192;
const FLAG_IPV4: i64 = 1_048_576;
const FLAG_IPV6: i64 = 2_097_152;

/// `(name, id)` for `filter_list()` / `filter_id()`.
const NAMES: &[(&str, i64)] = &[
    ("int", VALIDATE_INT),
    ("boolean", VALIDATE_BOOL),
    ("float", VALIDATE_FLOAT),
    ("validate_regexp", VALIDATE_REGEXP),
    ("validate_domain", VALIDATE_DOMAIN),
    ("validate_url", VALIDATE_URL),
    ("validate_email", VALIDATE_EMAIL),
    ("validate_ip", VALIDATE_IP),
    ("validate_mac", VALIDATE_MAC),
    ("string", SANITIZE_STRING),
    ("encoded", SANITIZE_ENCODED),
    ("special_chars", SANITIZE_SPECIAL_CHARS),
    ("full_special_chars", SANITIZE_FULL_SPECIAL_CHARS),
    ("unsafe_raw", UNSAFE_RAW),
    ("email", SANITIZE_EMAIL),
    ("url", SANITIZE_URL),
    ("number_int", SANITIZE_NUMBER_INT),
    ("number_float", SANITIZE_NUMBER_FLOAT),
    ("add_slashes", SANITIZE_ADD_SLASHES),
];

/// The `$options` argument, split into php's three parts.
struct Opts {
    flags: i64,
    default: Option<Value>,
    min: Option<i64>,
    max: Option<i64>,
    regexp: Option<Vec<u8>>,
}

impl Opts {
    /// `$options` is either a bare flags int or
    /// `['flags' => int, 'options' => ['min_range' => …, 'default' => …]]`.
    fn parse(v: Option<&Value>) -> Opts {
        let mut o = Opts {
            flags: 0,
            default: None,
            min: None,
            max: None,
            regexp: None,
        };
        let Some(v) = v else { return o };
        match &*v.deref() {
            Value::Array(a) => {
                if let Some(f) = a.get_deref(&ArrayKey::str(b"flags")) {
                    o.flags = f.to_int();
                }
                if let Some(Value::Array(inner)) =
                    a.get_deref(&ArrayKey::str(b"options")).map(|v| v.deref().into_owned())
                {
                    o.default = inner.get_deref(&ArrayKey::str(b"default"));
                    o.min = inner.get_deref(&ArrayKey::str(b"min_range")).map(|v| v.to_int());
                    o.max = inner.get_deref(&ArrayKey::str(b"max_range")).map(|v| v.to_int());
                    o.regexp = inner
                        .get_deref(&ArrayKey::str(b"regexp"))
                        .map(|v| v.to_php_bytes().to_vec());
                }
            }
            other => o.flags = other.to_int(),
        }
        o
    }

    /// What php returns when a validation fails: the `default` option, else
    /// `null` under `FILTER_NULL_ON_FAILURE`, else `false`.
    fn failure(&self) -> Value {
        if let Some(d) = &self.default {
            return d.clone();
        }
        if self.flags & NULL_ON_FAILURE != 0 {
            Value::Null
        } else {
            Value::Bool(false)
        }
    }
}

/// `filter_var(mixed $value, int $filter = FILTER_DEFAULT, array|int $options = 0): mixed`
fn filter_var(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let id = args.get(1).map_or(DEFAULT, Value::to_int);
    let opts = Opts::parse(args.get(2));
    let value = args[0].deref().into_owned();
    // `FILTER_CALLBACK`: the `options` callable maps the value (or every
    // leaf of an array).
    if id == CALLBACK {
        let callback = args
            .get(2)
            .and_then(|v| match &*v.deref() {
                Value::Array(a) => a.get_deref(&ArrayKey::str(b"options")),
                _ => None,
            })
            .unwrap_or(Value::Null);
        if !ctx.is_callable(&callback) {
            return Err(Unwind::type_error("filter_var(): Option must be a valid callback"));
        }
        return apply_callback(ctx, &value, &callback);
    }
    // The array flags: `REQUIRE_ARRAY` filters every leaf of an array (a
    // scalar fails), `FORCE_ARRAY` wraps a scalar first, and an array
    // without either fails (`REQUIRE_SCALAR` is the default).
    let is_array = matches!(value, Value::Array(_));
    if opts.flags & (REQUIRE_ARRAY | FORCE_ARRAY) != 0 {
        if !is_array && opts.flags & FORCE_ARRAY != 0 {
            let mut a = Array::new();
            a.push(apply(&value, id, &opts));
            return Ok(Value::Array(a));
        }
        if !is_array {
            return Ok(opts.failure());
        }
        return Ok(apply_deep(&value, id, &opts));
    }
    if is_array && opts.flags & REQUIRE_SCALAR == 0 {
        return Ok(opts.failure());
    }
    if is_array {
        return Ok(opts.failure());
    }
    Ok(apply(&value, id, &opts))
}

/// A filter over every leaf of a (nested) array.
fn apply_deep(value: &Value, id: i64, o: &Opts) -> Value {
    match value {
        Value::Array(a) => {
            let mut out = Array::new();
            for (k, v) in a.iter() {
                out.set(k.clone(), apply_deep(&v.deref().into_owned(), id, o));
            }
            Value::Array(out)
        }
        leaf => apply(leaf, id, o),
    }
}

/// `FILTER_CALLBACK` over a value or every leaf of an array; the value
/// reaches the callback as a string, as php hands it over.
fn apply_callback(ctx: &mut Ctx, value: &Value, callback: &Value) -> NativeResult {
    match value {
        Value::Array(a) => {
            let mut out = Array::new();
            for (k, v) in a.iter() {
                out.set(k.clone(), apply_callback(ctx, &v.deref().into_owned(), callback)?);
            }
            Ok(Value::Array(out))
        }
        leaf => ctx.call_value(callback, &[Value::string(&leaf.to_php_bytes())]),
    }
}

/// Run one filter over one value.
fn apply(value: &Value, id: i64, o: &Opts) -> Value {
    let raw = value.to_php_bytes().to_vec();
    let s = String::from_utf8_lossy(&raw).into_owned();
    let t = s.trim();
    match id {
        VALIDATE_INT => {
            // php accepts optional surrounding space and, under the flags,
            // hex/octal spellings; a fractional part is never an int.
            let parsed = if o.flags & FLAG_ALLOW_HEX != 0 && (t.starts_with("0x") || t.starts_with("0X")) {
                i64::from_str_radix(&t[2..], 16).ok()
            } else if o.flags & FLAG_ALLOW_OCTAL != 0 && t.starts_with('0') && t.len() > 1 {
                i64::from_str_radix(&t[1..], 8).ok()
            } else {
                t.parse::<i64>().ok()
            };
            match parsed {
                Some(n) if o.min.is_none_or(|m| n >= m) && o.max.is_none_or(|m| n <= m) => {
                    Value::Int(n)
                }
                _ => o.failure(),
            }
        }
        VALIDATE_FLOAT => {
            let cleaned = if o.flags & FLAG_ALLOW_THOUSAND != 0 {
                t.replace(',', "")
            } else {
                t.to_string()
            };
            match cleaned.parse::<f64>() {
                Ok(f) if f.is_finite() => Value::Float(f),
                _ => o.failure(),
            }
        }
        VALIDATE_BOOL => match t.to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Value::Bool(true),
            "0" | "false" | "off" | "no" | "" => {
                // php's `false` set returns false even under NULL_ON_FAILURE;
                // only an *unrecognised* string is a failure.
                Value::Bool(false)
            }
            _ => {
                if o.flags & NULL_ON_FAILURE != 0 || o.default.is_some() {
                    o.failure()
                } else {
                    Value::Bool(false)
                }
            }
        },
        VALIDATE_EMAIL => {
            if is_email(t) {
                Value::Str(Str::from_vec(t.as_bytes().to_vec()))
            } else {
                o.failure()
            }
        }
        VALIDATE_URL => {
            if is_url(t) {
                Value::Str(Str::from_vec(t.as_bytes().to_vec()))
            } else {
                o.failure()
            }
        }
        VALIDATE_IP => {
            let v4 = t.parse::<std::net::Ipv4Addr>().is_ok();
            let v6 = t.parse::<std::net::Ipv6Addr>().is_ok();
            let want4 = o.flags & FLAG_IPV4 != 0;
            let want6 = o.flags & FLAG_IPV6 != 0;
            let ok = match (want4, want6) {
                (true, false) => v4,
                (false, true) => v6,
                _ => v4 || v6,
            };
            if ok {
                Value::Str(Str::from_vec(t.as_bytes().to_vec()))
            } else {
                o.failure()
            }
        }
        VALIDATE_MAC => {
            let parts: Vec<&str> = t.split([':', '-']).collect();
            let ok = parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
            if ok {
                Value::Str(Str::from_vec(t.as_bytes().to_vec()))
            } else {
                o.failure()
            }
        }
        VALIDATE_DOMAIN => {
            let ok = !t.is_empty()
                && t.len() <= 253
                && t.split('.').all(|l| {
                    !l.is_empty()
                        && l.len() <= 63
                        && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                });
            if ok {
                Value::Str(Str::from_vec(t.as_bytes().to_vec()))
            } else {
                o.failure()
            }
        }
        SANITIZE_NUMBER_INT => Value::Str(Str::from_vec(
            raw.iter()
                .copied()
                .filter(|b| b.is_ascii_digit() || *b == b'+' || *b == b'-')
                .collect(),
        )),
        SANITIZE_NUMBER_FLOAT => Value::Str(Str::from_vec(
            raw.iter()
                .copied()
                .filter(|b| {
                    b.is_ascii_digit()
                        || *b == b'+'
                        || *b == b'-'
                        || (o.flags & FLAG_ALLOW_FRACTION != 0 && *b == b'.')
                        || (o.flags & FLAG_ALLOW_THOUSAND != 0 && *b == b',')
                })
                .collect(),
        )),
        SANITIZE_EMAIL => Value::Str(Str::from_vec(
            raw.iter()
                .copied()
                .filter(|b| {
                    b.is_ascii_alphanumeric()
                        || b"!#$%&'*+-=?^_`{|}~@.[]".contains(b)
                })
                .collect(),
        )),
        SANITIZE_URL => Value::Str(Str::from_vec(
            raw.iter().copied().filter(|b| b.is_ascii_graphic()).collect(),
        )),
        SANITIZE_ADD_SLASHES => {
            let mut out = Vec::with_capacity(raw.len());
            for b in raw {
                if matches!(b, b'\'' | b'"' | b'\\' | 0) {
                    out.push(b'\\');
                }
                out.push(b);
            }
            Value::Str(Str::from_vec(out))
        }
        // `FILTER_DEFAULT` / `FILTER_UNSAFE_RAW` pass the value through as a
        // string, which is also the honest fallback for a filter id this
        // build does not implement.
        _ => Value::Str(Str::from_vec(raw)),
    }
}

/// php's email grammar, reduced to what `FILTER_VALIDATE_EMAIL` accepts in
/// practice: a non-empty local part, one `@`, and a dotted domain.
fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.is_empty() {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    let local_ok = local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~.".contains(c));
    let domain_ok = domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    local_ok && domain_ok
}

/// `FILTER_VALIDATE_URL`: a scheme, `://`, and a host.
fn is_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    if scheme.is_empty()
        || !scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains(' ')
}

/// `filter_var_array(array $array, array|int $options = FILTER_DEFAULT, bool $add_empty = true): array|false|null`
fn filter_var_array(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Array(input) = &*args[0].deref() else {
        return Ok(Value::Bool(false));
    };
    let input = input.clone();
    let add_empty = args.get(2).is_none_or(Value::to_bool);
    let mut out = Array::new();
    match args.get(1).map(|v| v.deref().into_owned()) {
        // A per-key specification.
        Some(Value::Array(spec)) => {
            for (k, s) in spec.iter() {
                let key = k.clone();
                match input.get_deref(&key) {
                    Some(v) => {
                        let spec_v = s.deref().into_owned();
                        let (id, o) = match &spec_v {
                            Value::Array(inner) => (
                                inner
                                    .get_deref(&ArrayKey::str(b"filter"))
                                    .map_or(DEFAULT, |f| f.to_int()),
                                Opts::parse(Some(&spec_v)),
                            ),
                            other => (other.to_int(), Opts::parse(None)),
                        };
                        out.set(key, apply(&v, id, &o));
                    }
                    None if add_empty => out.set(key, Value::Null),
                    None => {}
                }
            }
        }
        // One filter for every entry.
        other => {
            let id = other.as_ref().map_or(DEFAULT, Value::to_int);
            let o = Opts::parse(None);
            for (k, v) in input.iter() {
                out.set(k.clone(), apply(&v.deref(), id, &o));
            }
        }
    }
    Ok(Value::Array(out))
}

/// The array an `INPUT_*` constant names, as the SAPI registered it
/// (`None`: never initialized), or php's `ValueError`.
fn input_array(ctx: &Ctx, func: &str, v: &Value) -> Result<Option<Array>, Unwind> {
    let inputs = &ctx.filter_inputs;
    Ok(match v.deref().to_int() {
        0 => inputs.post.clone(),
        1 => inputs.get.clone(),
        2 => inputs.cookie.clone(),
        4 => inputs.env.clone(),
        5 => inputs.server.clone(),
        _ => {
            return Err(Unwind::value_error(format!(
                "{func}(): Argument #1 ($type) must be an INPUT_* constant"
            )))
        }
    })
}

/// `filter_has_var(int $input_type, string $var_name): bool` — whether the
/// request's original input carried the name (the superglobals may have
/// been changed since; php does not look at them).
fn filter_has_var(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let input = input_array(ctx, "filter_has_var", &args[0]).map_err(|_| {
        Unwind::value_error("filter_has_var(): Argument #1 ($input_type) must be an INPUT_* constant")
    })?;
    let name = args[1].to_php_bytes();
    Ok(Value::Bool(input.is_some_and(|a| {
        rphp_value::array_key(&Value::string(&name)).is_some_and(|k| a.get(&k).is_some())
    })))
}

/// `filter_input(int $type, string $var_name, int $filter = FILTER_DEFAULT, array|int $options = 0): mixed`
/// — `filter_var()` over one variable of the request's original input. A
/// missing one is `null`, or `false` under `FILTER_NULL_ON_FAILURE` (the
/// flag inverts both answers), or the `default` option.
fn filter_input(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let input = input_array(ctx, "filter_input", &args[0])?;
    let name = args[1].to_php_bytes();
    let found = input.and_then(|a| {
        rphp_value::array_key(&Value::string(&name)).and_then(|k| a.get_deref(&k))
    });
    let Some(value) = found else {
        let o = Opts::parse(args.get(3));
        if let Some(d) = o.default {
            return Ok(d);
        }
        return Ok(if o.flags & NULL_ON_FAILURE != 0 { Value::Bool(false) } else { Value::Null });
    };
    let mut rest: Vec<Value> = vec![value];
    rest.extend(args.iter().skip(2).cloned());
    filter_var(ctx, &mut rest)
}

/// `filter_input_array(int $type, array|int $options = FILTER_DEFAULT, bool $add_empty = true): array|false|null`
/// — `filter_var_array()` over the request's original input; `null` when
/// that input was never initialized.
fn filter_input_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(input) = input_array(ctx, "filter_input_array", &args[0])? else {
        return Ok(Value::Null);
    };
    let mut rest: Vec<Value> = vec![Value::Array(input)];
    rest.extend(args.iter().skip(1).cloned());
    filter_var_array(ctx, &mut rest)
}

/// `filter_list(): array`
fn filter_list(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    for (n, _) in NAMES {
        a.push(Value::string(n.as_bytes()));
    }
    Ok(Value::Array(a))
}

/// `filter_id(string $name): int|false`
fn filter_id(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let want = args[0].to_php_bytes().to_vec();
    Ok(match NAMES.iter().find(|(n, _)| n.as_bytes() == &want[..]) {
        Some((_, id)) => Value::Int(*id),
        None => Value::Bool(false),
    })
}

/// php's `FILTER_*` constants.
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("FILTER_VALIDATE_INT", VALIDATE_INT),
        ("FILTER_VALIDATE_BOOLEAN", VALIDATE_BOOL),
        ("FILTER_VALIDATE_BOOL", VALIDATE_BOOL),
        ("FILTER_VALIDATE_FLOAT", VALIDATE_FLOAT),
        ("FILTER_VALIDATE_REGEXP", VALIDATE_REGEXP),
        ("FILTER_VALIDATE_DOMAIN", VALIDATE_DOMAIN),
        ("FILTER_VALIDATE_URL", VALIDATE_URL),
        ("FILTER_VALIDATE_EMAIL", VALIDATE_EMAIL),
        ("FILTER_VALIDATE_IP", VALIDATE_IP),
        ("FILTER_VALIDATE_MAC", VALIDATE_MAC),
        ("FILTER_DEFAULT", DEFAULT),
        ("FILTER_UNSAFE_RAW", UNSAFE_RAW),
        ("FILTER_SANITIZE_ENCODED", SANITIZE_ENCODED),
        ("FILTER_SANITIZE_SPECIAL_CHARS", SANITIZE_SPECIAL_CHARS),
        ("FILTER_SANITIZE_FULL_SPECIAL_CHARS", SANITIZE_FULL_SPECIAL_CHARS),
        ("FILTER_SANITIZE_EMAIL", SANITIZE_EMAIL),
        ("FILTER_SANITIZE_URL", SANITIZE_URL),
        ("FILTER_SANITIZE_NUMBER_INT", SANITIZE_NUMBER_INT),
        ("FILTER_SANITIZE_NUMBER_FLOAT", SANITIZE_NUMBER_FLOAT),
        ("FILTER_SANITIZE_ADD_SLASHES", SANITIZE_ADD_SLASHES),
        ("FILTER_CALLBACK", CALLBACK),
        ("FILTER_THROW_ON_FAILURE", 268_435_456),
        ("FILTER_FLAG_EMAIL_UNICODE", 1_048_576),
        ("FILTER_FLAG_NONE", 0),
        ("FILTER_NULL_ON_FAILURE", NULL_ON_FAILURE),
        ("FILTER_FLAG_ALLOW_OCTAL", FLAG_ALLOW_OCTAL),
        ("FILTER_FLAG_ALLOW_HEX", FLAG_ALLOW_HEX),
        ("FILTER_FLAG_ALLOW_FRACTION", FLAG_ALLOW_FRACTION),
        ("FILTER_FLAG_ALLOW_THOUSAND", FLAG_ALLOW_THOUSAND),
        ("FILTER_FLAG_IPV4", FLAG_IPV4),
        ("FILTER_FLAG_IPV6", FLAG_IPV6),
        ("FILTER_REQUIRE_SCALAR", 33_554_432),
        ("FILTER_REQUIRE_ARRAY", 16_777_216),
        ("FILTER_FORCE_ARRAY", 67_108_864),
        ("FILTER_FLAG_STRIP_LOW", 4),
        ("FILTER_FLAG_STRIP_HIGH", 8),
        ("FILTER_FLAG_STRIP_BACKTICK", 512),
        ("FILTER_FLAG_ENCODE_LOW", 16),
        ("FILTER_FLAG_ENCODE_HIGH", 32),
        ("FILTER_FLAG_ENCODE_AMP", 64),
        ("FILTER_FLAG_NO_ENCODE_QUOTES", 128),
        ("FILTER_FLAG_EMPTY_STRING_NULL", 256),
        ("FILTER_FLAG_ALLOW_SCIENTIFIC", 16_384),
        ("FILTER_FLAG_PATH_REQUIRED", 262_144),
        ("FILTER_FLAG_QUERY_REQUIRED", 524_288),
        ("FILTER_FLAG_NO_PRIV_RANGE", 8_388_608),
        ("FILTER_FLAG_NO_RES_RANGE", 4_194_304),
        ("FILTER_FLAG_GLOBAL_RANGE", 536_870_912),
        ("FILTER_FLAG_HOSTNAME", 1_048_576),
        ("INPUT_GET", 1),
        ("INPUT_POST", 0),
        ("INPUT_COOKIE", 2),
        ("INPUT_SERVER", 5),
        ("INPUT_ENV", 4),
    ] {
        r.constant(name, Value::Int(v));
    }
    for name in ["FILTER_SANITIZE_STRING", "FILTER_SANITIZE_STRIPPED"] {
        r.deprecated_constant(
            name,
            Value::Int(SANITIZE_STRING),
            " since 8.1, use htmlspecialchars() instead",
        );
    }
}
