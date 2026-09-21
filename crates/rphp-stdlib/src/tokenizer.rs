//! php-src `ext/tokenizer`: `token_get_all()`, `token_name()`, the `T_*`
//! constants and the `PhpToken` class, over the workspace's own
//! byte-exact scanner ([`rphp_tokenizer`], ADR-007).
//!
//! The scanner already reproduces `token_get_all()` of php 8.5 token for
//! token — ids, extents, line numbers and every quirk of the Zend scanner —
//! and this module is only the php-visible shape of its output: a
//! single-character token is the character itself, anything else is
//! `[id, text, line]`, and `PhpToken` adds the byte offset.
//!
//! **Known divergences (ADR-004).**
//!
//! * `TOKEN_PARSE` is accepted and ignored: php then runs the *parser* over
//!   the code, reclassifying a keyword the grammar accepted as an identifier
//!   (`$o->list`, `Foo::class`) to `T_STRING` and raising a `ParseError` for
//!   code that does not parse. Neither happens here — the token stream is
//!   the plain scan.
//! * `MyToken::tokenize()` on a subclass answers `PhpToken` instances, not
//!   `MyToken` ones: a native static method is not told the class it was
//!   called on.

use rphp_runtime::{nf, nm, Ctx, NativeFn, NativeMethod, NativeResult, Registry, Unwind};
use rphp_tokenizer::{ids, Options, RawToken};
use rphp_value::{Array, Object, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("token_get_all", 1, Some(2), token_get_all),
    nf!("token_name", 1, Some(1), token_name),
];

/// `TOKEN_PARSE`.
const TOKEN_PARSE: i64 = 1;

/// Every `T_*` constant, and `TOKEN_PARSE`.
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, id) in ids::ALL {
        r.constant(name, Value::Int(i64::from(*id)));
    }
    r.constant("TOKEN_PARSE", Value::Int(TOKEN_PARSE));
}

/// Scan `code` with the request's `short_open_tag`.
fn scan(ctx: &Ctx, code: &[u8]) -> Vec<RawToken> {
    rphp_tokenizer::tokenize(
        code,
        Options {
            short_open_tag: ctx.ini.bool("short_open_tag"),
        },
    )
}

/// A `string` parameter.
fn str_arg(v: &Value, func: &str, n: usize, name: &str) -> Result<Vec<u8>, Unwind> {
    match v.deref().as_ref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => {
            Err(Unwind::type_error(format!(
                "{func}(): Argument #{n} (${name}) must be of type string, {} given",
                rphp_runtime::value_name(&v)
            )))
        }
        _ => Ok(v.to_php_bytes()),
    }
}

/// `token_get_all(string $code, int $flags = 0): array`
fn token_get_all(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let code = str_arg(&args[0], "token_get_all", 1, "code")?;
    let mut out = Array::new();
    for t in scan(ctx, &code) {
        let text = &code[t.lo as usize..t.hi as usize];
        if t.id < 256 {
            out.push(Value::string(text));
            continue;
        }
        let mut row = Array::new();
        row.push(Value::Int(i64::from(t.id)));
        row.push(Value::string(text));
        row.push(Value::Int(i64::from(t.line)));
        out.push(Value::Array(row));
    }
    Ok(Value::Array(out))
}

/// `token_name(int $id): string` — `UNKNOWN` for a single-character token
/// and for an id php has not assigned.
fn token_name(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let id = args[0].deref().to_int();
    let name = u16::try_from(id)
        .ok()
        .and_then(rphp_tokenizer::token_name)
        .unwrap_or("UNKNOWN");
    Ok(Value::string(name.as_bytes()))
}

// ---- PhpToken ------------------------------------------------------------------

/// Register `PhpToken`.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.interp().class_by_name(b"PhpToken").is_some() {
        return;
    }
    r.class("PhpToken")
        .implements(&["Stringable"])
        .prop("id", rphp_runtime::Visibility::Public, Value::Int(0))
        .prop("text", rphp_runtime::Visibility::Public, Value::string(b""))
        .prop("line", rphp_runtime::Visibility::Public, Value::Int(-1))
        .prop("pos", rphp_runtime::Visibility::Public, Value::Int(-1))
        .method("__construct", nm!(2, Some(4), token_construct))
        .method("tokenize", static_method(1, Some(2), token_tokenize))
        .method("getTokenName", nm!(0, Some(0), token_get_name))
        .method("is", nm!(1, Some(1), token_is))
        .method("isIgnorable", nm!(0, Some(0), token_is_ignorable))
        .method("__toString", nm!(0, Some(0), token_to_string))
        .finish();
}

/// A static native method (`nm!` declares instance ones).
fn static_method(min: u8, max: Option<u8>, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

/// The receiver of an instance method.
fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// `PhpToken::__construct(int $id, string $text, int $line = -1, int $pos = -1)`
fn token_construct(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    o.set(b"id", Value::Int(args[0].deref().to_int()));
    o.set(b"text", Value::string(&args[1].to_php_bytes()));
    o.set(
        b"line",
        Value::Int(args.get(2).map_or(-1, |v| v.deref().to_int())),
    );
    o.set(
        b"pos",
        Value::Int(args.get(3).map_or(-1, |v| v.deref().to_int())),
    );
    Ok(Value::Null)
}

/// `PhpToken::tokenize(string $code, int $flags = 0): static[]`
fn token_tokenize(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let code = str_arg(&args[0], "PhpToken::tokenize", 1, "code")?;
    let cid = ctx
        .class_by_name(b"PhpToken")
        .ok_or_else(|| Unwind::error("Class \"PhpToken\" not found"))?;
    let mut out = Array::new();
    for t in scan(ctx, &code) {
        let text = &code[t.lo as usize..t.hi as usize];
        let o = ctx.instantiate(cid);
        o.set(b"id", Value::Int(i64::from(t.id)));
        o.set(b"text", Value::string(text));
        o.set(b"line", Value::Int(i64::from(t.line)));
        o.set(b"pos", Value::Int(i64::from(t.lo)));
        out.push(Value::Object(o));
    }
    Ok(Value::Array(out))
}

fn prop_int(o: &Object, name: &[u8]) -> i64 {
    o.get_deref(name).map_or(0, |v| v.to_int())
}

/// `PhpToken::getTokenName(): ?string` — the `T_*` name, the character
/// itself for a single-character token, `null` for an unassigned id.
fn token_get_name(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let id = prop_int(o, b"id");
    if (0..256).contains(&id) {
        return Ok(Value::string(&[id as u8]));
    }
    Ok(match u16::try_from(id).ok().and_then(rphp_tokenizer::token_name) {
        Some(n) => Value::string(n.as_bytes()),
        None => Value::Null,
    })
}

/// `PhpToken::is(int|string|array $kind): bool` — an id, a text, or a list
/// of either.
fn token_is(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let id = prop_int(o, b"id");
    let text = o.get_deref(b"text").map(|v| v.to_php_bytes()).unwrap_or_default();
    let matches = |k: &Value| -> Result<bool, Unwind> {
        Ok(match &*k.deref() {
            Value::Int(n) => *n == id,
            Value::Str(s) => s.as_bytes() == text.as_slice(),
            other => {
                return Err(Unwind::type_error(format!(
                    "PhpToken::is(): Argument #1 ($kind) must only have elements of type string|int, {} given",
                    rphp_runtime::value_name(&other)
                )))
            }
        })
    };
    let kind = args[0].deref().into_owned();
    Ok(Value::Bool(match &kind {
        Value::Array(list) => {
            let mut hit = false;
            for (_, v) in list.iter() {
                if matches(v)? {
                    hit = true;
                    break;
                }
            }
            hit
        }
        Value::Int(_) | Value::Str(_) => matches(&kind)?,
        other => {
            return Err(Unwind::type_error(format!(
                "PhpToken::is(): Argument #1 ($kind) must be of type string|int|array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    }))
}

/// `PhpToken::isIgnorable(): bool`
fn token_is_ignorable(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let id = prop_int(this(o)?, b"id");
    Ok(Value::Bool(
        u16::try_from(id).is_ok_and(rphp_tokenizer::is_ignorable),
    ))
}

/// `PhpToken::__toString(): string`
fn token_to_string(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(o.get_deref(b"text").unwrap_or_else(|| Value::string(b"")))
}
