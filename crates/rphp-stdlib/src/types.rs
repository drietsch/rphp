//! Type-inspection and scalar-cast builtins (php-src `ext/standard/type.c`).
use rphp_value::{Str, Value};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Unwind};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("gettype", 1, Some(1), gettype),
    nf!("is_int", 1, Some(1), is_int),
    nf!("is_integer", 1, Some(1), is_int),
    nf!("is_long", 1, Some(1), is_int),
    nf!("is_string", 1, Some(1), is_string),
    nf!("is_bool", 1, Some(1), is_bool),
    nf!("is_float", 1, Some(1), is_float),
    nf!("is_double", 1, Some(1), is_float),
    nf!("is_array", 1, Some(1), is_array),
    nf!("is_null", 1, Some(1), is_null),
    nf!("is_numeric", 1, Some(1), is_numeric),
    nf!("is_scalar", 1, Some(1), is_scalar),
    nf!("is_object", 1, Some(1), is_object),
    nf!("is_resource", 1, Some(1), is_resource),
    nf!("intval", 1, Some(2), intval),
    nf!("floatval", 1, Some(1), floatval),
    nf!("doubleval", 1, Some(1), floatval),
    nf!("strval", 1, Some(1), strval),
    nf!("boolval", 1, Some(1), boolval),
    nf!("get_debug_type", 1, Some(1), get_debug_type),
    nf_ref!("settype", 2, Some(2), 0b1, settype),
    nf!("is_iterable", 1, Some(1), is_iterable),
    nf!("is_countable", 1, Some(1), is_countable),
    nf_ref!("is_callable", 1, Some(3), 0b100, is_callable),
];

pub(crate) fn gettype(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name: &[u8] = match &*args[0].deref() {
        // An uninitialized typed property never reaches a call (the runtime
        // errors first); as a value it is null.
        Value::Null | Value::Uninit => b"NULL",
        Value::Bool(_) => b"boolean",
        Value::Int(_) => b"integer",
        Value::Float(_) => b"double",
        Value::Str(_) => b"string",
        Value::Array(_) => b"array",
        Value::Closure(_) | Value::Object(_) => b"object",
        // "resource" or "resource (closed)".
        Value::Resource(r) => r.type_name().as_bytes(),
        // Dereferenced above; PHP's own fallback spelling.
        Value::Ref(_) => b"unknown type",
    };
    Ok(Value::string(name))
}

pub(crate) fn is_int(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Int(_))))
}

pub(crate) fn is_string(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Str(_))))
}

pub(crate) fn is_bool(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Bool(_))))
}

pub(crate) fn is_float(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Float(_))))
}

pub(crate) fn is_array(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Array(_))))
}

pub(crate) fn is_null(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Null | Value::Uninit)))
}

pub(crate) fn is_numeric(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(args[0].is_numeric()))
}

pub(crate) fn is_scalar(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(
        *args[0].deref(),
        Value::Int(_) | Value::Float(_) | Value::Str(_) | Value::Bool(_)
    )))
}

pub(crate) fn is_object(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(*args[0].deref(), Value::Object(_) | Value::Closure(_))))
}

/// PHP `is_resource`: an **open** resource (a closed one is not a resource
/// any more, though `gettype` still says `resource (closed)`).
pub(crate) fn is_resource(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(matches!(&*args[0].deref(), Value::Resource(r) if !r.is_closed())))
}

pub(crate) fn intval(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // `intval($s, $base)` parses a *string* in the given base (base 0 auto-detects
    // from a `0x`/`0b`/`0` prefix); for non-strings, or base 10, it is the plain
    // integer cast.
    if let (Value::Str(s), Some(base)) = (&args[0], args.get(1)) {
        let base = base.to_int();
        if base != 10 {
            // `strtol` rejects a base outside 2..=36 (other than 0) with 0.
            if base != 0 && !(2..=36).contains(&base) {
                return Ok(Value::Int(0));
            }
            return Ok(Value::Int(parse_in_base(s.as_bytes(), base)));
        }
    }
    Ok(Value::Int(args[0].to_int()))
}

/// `strtoll`-style lenient base parse used by `intval($s, $base)`: skip leading
/// whitespace and an optional sign, strip the base prefix (`0x`/`0b`/`0o`, or
/// auto-detected when `base == 0`), then consume valid digits and stop at the
/// first one out of range. Overflow saturates, as PHP's does.
fn parse_in_base(s: &[u8], mut base: i64) -> i64 {
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let neg = i < s.len() && {
        let sign = s[i] == b'-';
        if s[i] == b'+' || s[i] == b'-' {
            i += 1;
        }
        sign
    };
    let has_prefix = |i: usize, c: u8| i + 1 < s.len() && s[i] == b'0' && s[i + 1] | 0x20 == c;
    if base == 0 {
        // Auto-detect the base from a conventional prefix.
        if has_prefix(i, b'x') {
            base = 16;
            i += 2;
        } else if has_prefix(i, b'b') {
            base = 2;
            i += 2;
        } else if i < s.len() && s[i] == b'0' {
            base = 8;
            i += 1;
        } else {
            base = 10;
        }
    } else {
        // An explicit base 16 / 2 may still carry its matching prefix; strip
        // it (`strtol` accepts `0x`, php's `intval` adds `0b`; `0o` is not
        // recognised here, unlike by `octdec`).
        let prefix = match base {
            16 => Some(b'x'),
            2 => Some(b'b'),
            _ => None,
        };
        if prefix.is_some_and(|c| has_prefix(i, c)) {
            i += 2;
        }
    }
    let mut acc: i64 = 0;
    while i < s.len() {
        let digit = match s[i] {
            c @ b'0'..=b'9' => (c - b'0') as i64,
            c @ b'a'..=b'z' => (c - b'a' + 10) as i64,
            c @ b'A'..=b'Z' => (c - b'A' + 10) as i64,
            _ => break,
        };
        if digit >= base {
            break;
        }
        acc = acc.saturating_mul(base).saturating_add(digit);
        i += 1;
    }
    if neg {
        -acc
    } else {
        acc
    }
}

pub(crate) fn floatval(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Float(args[0].to_float()))
}

pub(crate) fn strval(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Str(rphp_value::Str::from_vec(args[0].to_php_bytes())))
}

pub(crate) fn boolval(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(args[0].to_bool()))
}

/// `get_debug_type(mixed $value): string` — the php 8 canonical type names:
/// `int`, `float`, `string`, `bool`, `null`, `array`, the class name of an
/// object (`Closure` for closures), `resource (kind)` / `resource (closed)`.
pub(crate) fn get_debug_type(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v = args[0].deref();
    Ok(Value::Str(Str::from_vec(match &*v {
        Value::Null | Value::Uninit => b"null".to_vec(),
        Value::Bool(_) => b"bool".to_vec(),
        Value::Int(_) => b"int".to_vec(),
        Value::Float(_) => b"float".to_vec(),
        Value::Str(_) => b"string".to_vec(),
        Value::Array(_) => b"array".to_vec(),
        Value::Closure(_) => b"Closure".to_vec(),
        // `get_debug_type` is a message-shaped answer: php's `%s` stops at
        // the NUL of an anonymous class's name.
        Value::Object(o) => rphp_value::display_class_name(o.layout().class_name()).to_vec(),
        Value::Resource(r) => {
            if r.is_closed() {
                b"resource (closed)".to_vec()
            } else {
                format!("resource ({})", r.kind()).into_bytes()
            }
        }
        Value::Ref(_) => b"unknown type".to_vec(),
    })))
}

/// `settype(mixed &$var, string $type): bool` — the cast named by `$type`
/// (`int`/`integer`, `float`/`double`, `string`, `bool`/`boolean`, `array`,
/// `null`, all case-insensitive) written back through the reference.
/// `object` needs `stdClass` (plan E6) and `resource` is not convertible:
/// both raise as php does for the latter.
pub(crate) fn settype(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ty = args[1].to_php_bytes().to_ascii_lowercase();
    let cur = args[0].deref().into_owned();
    // The casts themselves (`(int)`, `(object)`, …) with their notices.
    let kind = match &ty[..] {
        b"int" | b"integer" => rphp_runtime::CastKind::Int,
        b"float" | b"double" => rphp_runtime::CastKind::Float,
        b"string" => rphp_runtime::CastKind::String,
        b"bool" | b"boolean" => rphp_runtime::CastKind::Bool,
        b"array" => rphp_runtime::CastKind::Array,
        b"object" => rphp_runtime::CastKind::Object,
        b"null" => {
            Value::assign(&mut args[0], Value::Null);
            return Ok(Value::Bool(true));
        }
        b"resource" => return Err(Unwind::value_error("Cannot convert to resource type")),
        _ => {
            return Err(Unwind::value_error(
                "settype(): Argument #2 ($type) must be a valid type",
            ))
        }
    };
    let new = ctx.cast(&cur, kind)?;
    Value::assign(&mut args[0], new);
    Ok(Value::Bool(true))
}

/// `is_iterable(mixed $value): bool` — arrays (and `Traversable` objects,
/// once interfaces exist).
pub(crate) fn is_iterable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(match &*args[0].deref() {
        Value::Array(_) => true,
        Value::Object(o) => ctx
            .class_by_name(b"Traversable")
            .is_some_and(|t| ctx.object_instanceof(o, t)),
        _ => false,
    }))
}

/// `is_countable(mixed $value): bool` — arrays and `Countable` objects.
pub(crate) fn is_countable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(match &*args[0].deref() {
        Value::Array(_) => true,
        Value::Object(o) => ctx
            .class_by_name(b"Countable")
            .is_some_and(|t| ctx.object_instanceof(o, t)),
        _ => false,
    }))
}

/// `is_callable(mixed $value, bool $syntax_only = false, string &$callable_name = null): bool`
/// — a closure, the name of a declared user function or native, or a
/// `[$object, 'method']` pair resolving through the class table. The
/// `'Class::method'` / `['Class', 'method']` forms need a *static* method,
/// which the class model does not declare yet (E6), so they are only
/// callable with `$syntax_only`. `$callable_name` receives php's rendering
/// (`strlen`, `Foo::bar`, `Closure::__invoke`, or the string form of a
/// non-callable value).
pub(crate) fn is_callable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v = args[0].deref().into_owned();
    let syntax_only = args.get(1).is_some_and(Value::to_bool);
    // php loads the class a `['C', 'm']` / `'C::m'` names before deciding.
    if !syntax_only {
        ctx.autoload_callable(&v)?;
    }
    let (callable, name): (bool, Vec<u8>) = match &v {
        Value::Closure(_) => (true, b"Closure::__invoke".to_vec()),
        Value::Str(s) => {
            let name = s.as_bytes();
            let bare = name.strip_prefix(b"\\").unwrap_or(name);
            // `'A::m'` resolves through the engine's shared callable path
            // (visibility and static-context rules included).
            let ok = if bare.windows(2).any(|w| w == b"::") {
                syntax_only || ctx.is_callable(&v)
            } else {
                syntax_only || ctx.function_exists(bare)
            };
            (ok, name.to_vec())
        }
        Value::Array(a) => {
            let target = a.get_deref(&rphp_value::ArrayKey::Int(0));
            let method = a.get_deref(&rphp_value::ArrayKey::Int(1));
            match (target, method) {
                (Some(t), Some(Value::Str(m))) if a.len() == 2 => {
                    let (class_name, ok) = match &t {
                        Value::Object(o) => {
                            let class = o.layout().class_name().to_vec();
                            // The engine's shared path, so `__call` and the
                            // visibility rules count (`methods.rs`).
                            let ok = syntax_only || ctx.is_callable(&v);
                            (class, ok)
                        }
                        Value::Str(c) => (c.as_bytes().to_vec(), syntax_only || ctx.is_callable(&v)),
                        _ => (Vec::new(), false),
                    };
                    let mut name = class_name;
                    name.extend_from_slice(b"::");
                    name.extend_from_slice(m.as_bytes());
                    (ok, name)
                }
                _ => (false, b"Array".to_vec()),
            }
        }
        // An object is callable when its class defines `__invoke`; php names
        // it `C::__invoke`.
        Value::Object(o) => {
            let mut name = o.layout().class_name().to_vec();
            name.extend_from_slice(b"::__invoke");
            (syntax_only || ctx.is_callable(&v), name)
        }
        other => (false, other.to_php_bytes()),
    };
    if args.len() > 2 {
        Value::assign(&mut args[2], Value::Str(Str::from_vec(name)));
    }
    Ok(Value::Bool(callable))
}

/// C `isspace` in the C locale: space, `\t`, `\n`, `\v`, `\f`, `\r`
/// (Rust's `is_ascii_whitespace` leaves out the vertical tab).
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[cfg(test)]
mod tests {
    use rphp_runtime::ErrorKind;
    use rphp_value::Value;

    use crate::tests::{arr, call_err, call_named, interp};

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    #[test]
    fn get_debug_type_uses_php8_names() {
        assert_eq!(call_named(b"get_debug_type", &[Value::Int(1)]), s("int"));
        assert_eq!(call_named(b"get_debug_type", &[Value::Float(1.0)]), s("float"));
        assert_eq!(call_named(b"get_debug_type", &[Value::Null]), s("null"));
        assert_eq!(call_named(b"get_debug_type", &[Value::Bool(true)]), s("bool"));
        assert_eq!(call_named(b"get_debug_type", &[arr(&[])]), s("array"));
    }

    #[test]
    fn settype_casts_in_place() {
        let mut it = interp();
        let mut args = [s("12abc"), s("integer")];
        it.call_native(it.native_by_name(b"settype").unwrap(), &mut args).unwrap();
        assert_eq!(args[0], Value::Int(12));
        let mut args = [Value::Int(0), s("BOOL")];
        it.call_native(it.native_by_name(b"settype").unwrap(), &mut args).unwrap();
        assert_eq!(args[0], Value::Bool(false));
        let mut args = [s("x"), s("array")];
        it.call_native(it.native_by_name(b"settype").unwrap(), &mut args).unwrap();
        assert_eq!(args[0], arr(&[s("x")]));
        assert_eq!(call_err(b"settype", &[Value::Int(1), s("nope")]).kind(), Some(ErrorKind::ValueError));
        assert_eq!(call_err(b"settype", &[Value::Int(1), s("resource")]).message(), Some("Cannot convert to resource type"));
    }

    #[test]
    fn intval_base_and_predicates() {
        assert_eq!(call_named(b"intval", &[s("12"), Value::Int(37)]), Value::Int(0));
        assert_eq!(call_named(b"intval", &[s("0x1A"), Value::Int(16)]), Value::Int(26));
        assert_eq!(call_named(b"intval", &[s("0b101"), Value::Int(0)]), Value::Int(5));
        assert_eq!(call_named(b"is_iterable", &[arr(&[])]), Value::Bool(true));
        assert_eq!(call_named(b"is_countable", &[s("x")]), Value::Bool(false));
        assert_eq!(call_named(b"is_callable", &[s("strlen")]), Value::Bool(true));
        assert_eq!(call_named(b"is_callable", &[s("nope")]), Value::Bool(false));
        assert_eq!(call_named(b"is_callable", &[s("nope"), Value::Bool(true)]), Value::Bool(true));
    }
}
