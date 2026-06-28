//! Output / debugging builtins: `var_dump`, `print_r`.
//!
//! These write to the run's stdout buffer ([`Ctx::out`]) rather than returning a
//! string (except `print_r($v, true)`, which returns it). The formatting is
//! byte-exact against stock PHP for the value shapes the M-stdlib slice supports
//! (scalars + arrays). Floats use a shortest-round-trip form (PHP's
//! `serialize_precision=-1`); the scientific-notation threshold is a documented
//! divergence axis until the dedicated serializer lands.
use rphp_value::{ArrayKey, Str, Value};

use crate::{nf, Ctx, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("var_dump", 1, None, var_dump),
    nf!("print_r", 1, Some(2), print_r),
];

/// PHP `var_dump(...$values)`: dump each argument's type and value. Returns null.
pub(crate) fn var_dump(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    for v in args {
        dump(ctx.out(), v, 0);
    }
    Ok(Value::Null)
}

/// PHP `print_r($value, $return = false)`: human-readable form. With `$return`
/// truthy, returns the string; otherwise writes it to stdout and returns `true`.
pub(crate) fn print_r(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let return_mode = args.get(1).is_some_and(Value::to_bool);
    let mut buf = Vec::new();
    print_r_buf(&mut buf, &args[0], 0);
    if return_mode {
        Ok(Value::Str(Str::from_vec(buf)))
    } else {
        ctx.out().extend_from_slice(&buf);
        Ok(Value::Bool(true))
    }
}

// ---- var_dump ---------------------------------------------------------------

fn indent(out: &mut Vec<u8>, spaces: usize) {
    out.resize(out.len() + spaces, b' ');
}

fn dump_float(out: &mut Vec<u8>, f: f64) {
    let s = if f.is_nan() {
        "NAN".to_string()
    } else if f.is_infinite() {
        if f < 0.0 { "-INF" } else { "INF" }.to_string()
    } else {
        // Shortest round-trip, matching serialize_precision=-1 for the common
        // (non-scientific) magnitudes.
        format!("{f}")
    };
    out.extend_from_slice(s.as_bytes());
}

/// Emit the `var_dump` representation of `v`. `pad` is the indentation (in
/// spaces) of the *enclosing* array; the caller has already written `pad` spaces
/// before the value when this is an array element.
fn dump(out: &mut Vec<u8>, v: &Value, pad: usize) {
    match v {
        Value::Null => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => {
            out.extend_from_slice(if *b { b"bool(true)\n" } else { b"bool(false)\n" });
        }
        Value::Int(i) => {
            out.extend_from_slice(format!("int({i})\n").as_bytes());
        }
        Value::Float(f) => {
            out.extend_from_slice(b"float(");
            dump_float(out, *f);
            out.extend_from_slice(b")\n");
        }
        Value::Str(s) => {
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, val) in a.iter() {
                indent(out, pad + 2);
                dump_key(out, k);
                indent(out, pad + 2);
                dump(out, val, pad + 2);
            }
            indent(out, pad);
            out.extend_from_slice(b"}\n");
        }
        // A closure is an object; PHP prints `object(Closure)#N (0) {}` with a
        // live object id we don't model, so this is a documented divergence.
        Value::Closure(_) => out.extend_from_slice(b"object(Closure) {\n}\n"),
    }
}

fn dump_key(out: &mut Vec<u8>, k: &ArrayKey) {
    match k {
        ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
        ArrayKey::Str(b) => {
            out.extend_from_slice(b"[\"");
            out.extend_from_slice(b);
            out.extend_from_slice(b"\"]=>\n");
        }
    }
}

// ---- print_r ----------------------------------------------------------------

/// Emit the `print_r` representation of `v`. `pad` is the indentation applied to
/// the `(` / `)` lines of an array; scalars print their plain string cast.
fn print_r_buf(out: &mut Vec<u8>, v: &Value, pad: usize) {
    match v {
        Value::Array(a) => {
            out.extend_from_slice(b"Array\n");
            indent(out, pad);
            out.extend_from_slice(b"(\n");
            for (k, val) in a.iter() {
                indent(out, pad + 4);
                out.push(b'[');
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(b) => out.extend_from_slice(b),
                }
                out.extend_from_slice(b"] => ");
                print_r_buf(out, val, pad + 8);
                out.push(b'\n');
            }
            indent(out, pad);
            out.extend_from_slice(b")\n");
        }
        // Scalars use the same string conversion as `echo`.
        _ => v.append_php_bytes(out),
    }
}
