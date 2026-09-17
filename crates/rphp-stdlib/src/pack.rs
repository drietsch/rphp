//! `pack()` / `unpack()` — a transcription of php-src's `ext/standard/pack.c`:
//! every format code (`a A Z h H c C s S n v i I l L N V q Q J P f g G d e E
//! x X @`) with repeat counts and `*`, php's two-pass `pack` (argument
//! accounting with its `ValueError`s, then the output-size pass with `X`/`@`
//! repositioning), `unpack`'s named keys (`Nlen/a*data`, the 1-based suffix
//! for repeated codes, `zend_symtable` key normalisation) and every warning
//! text of PHP 8.5 (`Type C: not enough input values, need 1 values but only
//! 0 were provided`, `Type H: illegal hex digit z`, `Type X: outside of
//! string`, `Type h: integer overflow`, …).
//!
//! Machine-endian codes (`s S i I l L q Q f d`) are little-endian: every
//! target rphp builds for is, as is the oracle.
use rphp_value::{array_key, Array, Str, Value};

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("pack", 1, None, pack),
    nf!("unpack", 2, Some(3), unpack),
];

/// `pack`/`unpack` define no constants.
pub(crate) fn register_constants(_r: &mut Registry) {}

/// `zval_get_string` for a `mixed` pack argument.
fn arg_bytes(ctx: &mut Ctx, v: &Value) -> Result<Vec<u8>, Unwind> {
    Ok(ctx.to_string(v)?.as_bytes().to_vec())
}

/// `zval_get_long` for a `mixed` pack argument (never warns: `(int)` cast
/// semantics — non-numeric strings are 0, arrays 0/1).
fn arg_long(v: &Value) -> i64 {
    v.to_int()
}

/// `zval_get_double`.
fn arg_double(v: &Value) -> f64 {
    v.to_float()
}

/// php's `atoi` over the digits at `format[i..]`, advancing `i`; the value is
/// clamped into `int`.
fn read_count(format: &[u8], i: &mut usize) -> i32 {
    let mut n: i64 = 0;
    while *i < format.len() && format[*i].is_ascii_digit() {
        n = n.saturating_mul(10).saturating_add(i64::from(format[*i] - b'0'));
        *i += 1;
    }
    n.min(i64::from(i32::MAX)) as i32
}

/// One parsed `pack` directive.
struct PackCode {
    code: u8,
    arg: i32,
}

/// PHP `pack(string $format, mixed ...$values): string`.
pub(crate) fn pack(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let format = match &args[0] {
        Value::Array(_) => {
            return Err(Unwind::type_error("pack(): Argument #1 ($format) must be of type string, array given"))
        }
        v => arg_bytes(ctx, v)?,
    };
    let values = &args[1..];
    let num_args = values.len();
    let mut codes: Vec<PackCode> = Vec::with_capacity(format.len());
    let mut currentarg = 0usize;

    // Pass 1: parse the codes and account for their arguments.
    let mut i = 0;
    while i < format.len() {
        let code = format[i];
        i += 1;
        let mut arg: i32 = 1;
        if i < format.len() {
            let c = format[i];
            if c == b'*' {
                arg = -1;
                i += 1;
            } else if c.is_ascii_digit() {
                arg = read_count(&format, &mut i);
            }
        }
        match code {
            b'x' | b'X' | b'@' => {
                if arg < 0 {
                    ctx.warn(&format!("pack(): Type {}: '*' ignored", code as char))?;
                    arg = 1;
                }
            }
            b'a' | b'A' | b'Z' | b'h' | b'H' => {
                if currentarg >= num_args {
                    return Err(Unwind::value_error(format!("Type {}: not enough arguments", code as char)));
                }
                if arg < 0 {
                    let s = arg_bytes(ctx, &values[currentarg])?;
                    // `Z*` is always NUL-terminated: one byte more.
                    arg = (s.len() + usize::from(code == b'Z')).min(i32::MAX as usize) as i32;
                }
                currentarg += 1;
            }
            b'q' | b'Q' | b'J' | b'P' | b'c' | b'C' | b's' | b'S' | b'i' | b'I' | b'l' | b'L' | b'n' | b'N'
            | b'v' | b'V' | b'f' | b'g' | b'G' | b'd' | b'e' | b'E' => {
                if arg < 0 {
                    arg = (num_args - currentarg).min(i32::MAX as usize) as i32;
                }
                currentarg += arg as usize;
                if currentarg > num_args {
                    return Err(Unwind::value_error(format!("Type {}: too few arguments", code as char)));
                }
            }
            _ => {
                return Err(Unwind::value_error(format!(
                    "Type {}: unknown format code",
                    String::from_utf8_lossy(&[code])
                )))
            }
        }
        codes.push(PackCode { code, arg });
    }
    if currentarg < num_args {
        ctx.warn(&format!("pack(): {} arguments unused", num_args - currentarg))?;
    }

    // Pass 2: the output size (with `X` / `@` repositioning).
    let mut outputpos: i64 = 0;
    let mut outputsize: i64 = 0;
    for pc in &codes {
        let arg = i64::from(pc.arg);
        let unit: i64 = match pc.code {
            b'h' | b'H' => 0, // handled below
            b'a' | b'A' | b'Z' | b'c' | b'C' | b'x' => 1,
            b's' | b'S' | b'n' | b'v' => 2,
            b'i' | b'I' | b'l' | b'L' | b'N' | b'V' | b'f' | b'g' | b'G' => 4,
            b'q' | b'Q' | b'J' | b'P' | b'd' | b'e' | b'E' => 8,
            _ => 0,
        };
        match pc.code {
            b'h' | b'H' => outputpos += (arg + arg % 2) / 2,
            b'X' => {
                outputpos -= arg;
                if outputpos < 0 {
                    ctx.warn("pack(): Type X: outside of string")?;
                    outputpos = 0;
                }
            }
            b'@' => outputpos = arg,
            _ => {
                if arg < 0 || (i64::from(i32::MAX) - outputpos) / unit < arg {
                    return Err(Unwind::value_error(format!(
                        "Type {}: integer overflow in format string",
                        pc.code as char
                    )));
                }
                outputpos += arg * unit;
            }
        }
        if outputsize < outputpos {
            outputsize = outputpos;
        }
    }

    // Pass 3: the packing itself.
    let mut out = vec![0u8; outputsize as usize];
    let mut pos = 0usize;
    let mut currentarg = 0usize;
    for pc in &codes {
        let code = pc.code;
        let arg = pc.arg.max(0) as usize;
        match code {
            b'a' | b'A' | b'Z' => {
                let s = arg_bytes(ctx, &values[currentarg])?;
                currentarg += 1;
                let copy = if code == b'Z' { arg.saturating_sub(1) } else { arg };
                let fill = if code == b'A' { b' ' } else { 0 };
                for b in &mut out[pos..pos + arg] {
                    *b = fill;
                }
                let n = s.len().min(copy);
                out[pos..pos + n].copy_from_slice(&s[..n]);
                pos += arg;
            }
            b'h' | b'H' => {
                let s = arg_bytes(ctx, &values[currentarg])?;
                currentarg += 1;
                let mut count = arg;
                if count > s.len() {
                    ctx.warn(&format!("pack(): Type {}: not enough characters in string", code as char))?;
                    count = s.len();
                }
                let mut nibbleshift = if code == b'h' { 0 } else { 4 };
                let mut first = true;
                // php walks `outputpos - 1` and pre-increments on each new byte.
                let mut cur = pos.wrapping_sub(1);
                for &c in &s[..count] {
                    let n = match c {
                        b'0'..=b'9' => c - b'0',
                        b'A'..=b'F' => c - b'A' + 10,
                        b'a'..=b'f' => c - b'a' + 10,
                        _ => {
                            ctx.warn(&format!(
                                "pack(): Type {}: illegal hex digit {}",
                                code as char,
                                String::from_utf8_lossy(&[c])
                            ))?;
                            0
                        }
                    };
                    if first {
                        cur = cur.wrapping_add(1);
                        out[cur] = 0;
                        first = false;
                    } else {
                        first = true;
                    }
                    out[cur] |= n << nibbleshift;
                    nibbleshift = (nibbleshift + 4) & 7;
                }
                pos = cur.wrapping_add(1);
            }
            b'c' | b'C' => {
                for _ in 0..arg {
                    out[pos] = arg_long(&values[currentarg]).to_le_bytes()[0];
                    currentarg += 1;
                    pos += 1;
                }
            }
            b's' | b'S' | b'n' | b'v' => {
                for _ in 0..arg {
                    let v = arg_long(&values[currentarg]) as u16;
                    currentarg += 1;
                    out[pos..pos + 2].copy_from_slice(&if code == b'n' { v.to_be_bytes() } else { v.to_le_bytes() });
                    pos += 2;
                }
            }
            b'i' | b'I' | b'l' | b'L' | b'N' | b'V' => {
                for _ in 0..arg {
                    let v = arg_long(&values[currentarg]) as u32;
                    currentarg += 1;
                    out[pos..pos + 4].copy_from_slice(&if code == b'N' { v.to_be_bytes() } else { v.to_le_bytes() });
                    pos += 4;
                }
            }
            b'q' | b'Q' | b'J' | b'P' => {
                for _ in 0..arg {
                    let v = arg_long(&values[currentarg]) as u64;
                    currentarg += 1;
                    out[pos..pos + 8].copy_from_slice(&if code == b'J' { v.to_be_bytes() } else { v.to_le_bytes() });
                    pos += 8;
                }
            }
            b'f' | b'g' | b'G' => {
                for _ in 0..arg {
                    let v = arg_double(&values[currentarg]) as f32;
                    currentarg += 1;
                    out[pos..pos + 4].copy_from_slice(&if code == b'G' { v.to_be_bytes() } else { v.to_le_bytes() });
                    pos += 4;
                }
            }
            b'd' | b'e' | b'E' => {
                for _ in 0..arg {
                    let v = arg_double(&values[currentarg]);
                    currentarg += 1;
                    out[pos..pos + 8].copy_from_slice(&if code == b'E' { v.to_be_bytes() } else { v.to_le_bytes() });
                    pos += 8;
                }
            }
            b'x' => {
                for b in &mut out[pos..pos + arg] {
                    *b = 0;
                }
                pos += arg;
            }
            b'X' => pos = pos.saturating_sub(arg),
            b'@' => {
                if arg > pos {
                    for b in &mut out[pos..arg] {
                        *b = 0;
                    }
                }
                pos = arg;
            }
            _ => {}
        }
    }
    out.truncate(pos);
    Ok(Value::Str(Str::from_vec(out)))
}

/// PHP `unpack(string $format, string $string, int $offset = 0): array|false`.
pub(crate) fn unpack(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let format = match &args[0] {
        Value::Array(_) => {
            return Err(Unwind::type_error("unpack(): Argument #1 ($format) must be of type string, array given"))
        }
        v => arg_bytes(ctx, v)?,
    };
    let data = match &args[1] {
        Value::Array(_) => {
            return Err(Unwind::type_error("unpack(): Argument #2 ($string) must be of type string, array given"))
        }
        v => arg_bytes(ctx, v)?,
    };
    let offset = match args.get(2) {
        Some(Value::Array(_) | Value::Object(_)) => {
            return Err(Unwind::type_error(format!(
                "unpack(): Argument #3 ($offset) must be of type int, {} given",
                rphp_runtime::value_name(&args[2])
            )))
        }
        Some(v) => v.to_int(),
        None => 0,
    };
    if offset < 0 || offset > data.len() as i64 {
        return Err(Unwind::value_error(
            "unpack(): Argument #3 ($offset) must be contained in argument #2 ($data)",
        ));
    }
    let input = &data[offset as usize..];
    let inputlen = input.len() as i64;
    let mut inputpos: i64 = 0;
    let mut out = Array::new();

    let overflow = |ctx: &mut Ctx, ty: u8| -> NativeResult {
        ctx.warn(&format!("unpack(): Type {}: integer overflow", ty as char))?;
        Ok(Value::Bool(false))
    };

    let mut f = 0usize;
    while f < format.len() {
        let ty = format[f];
        f += 1;
        let mut repetitions: i64 = 1;
        if f < format.len() {
            let c = format[f];
            if c.is_ascii_digit() {
                let mut n: i64 = 0;
                while f < format.len() && format[f].is_ascii_digit() {
                    n = n.saturating_mul(10).saturating_add(i64::from(format[f] - b'0'));
                    f += 1;
                }
                if n > i64::from(i32::MAX) {
                    return overflow(ctx, ty);
                }
                repetitions = n;
            } else if c == b'*' {
                repetitions = -1;
                f += 1;
            }
        }
        // The key name runs to the next `/`.
        let name_start = f;
        while f < format.len() && format[f] != b'/' {
            f += 1;
        }
        let name = &format[name_start..f.min(name_start + 200)];
        if f < format.len() {
            f += 1; // the '/'
        }
        let argb = repetitions;

        let mut size: i64 = match ty {
            b'X' => {
                if repetitions < 0 {
                    ctx.warn("unpack(): Type X: '*' ignored")?;
                    repetitions = 1;
                }
                -1
            }
            b'@' => 0,
            b'a' | b'A' | b'Z' => {
                let s = repetitions;
                repetitions = 1;
                s
            }
            b'h' | b'H' => {
                let s = if repetitions > 0 { (repetitions + repetitions % 2) / 2 } else { repetitions };
                repetitions = 1;
                if s > i64::from(i32::MAX) {
                    return overflow(ctx, ty);
                }
                s
            }
            b'c' | b'C' | b'x' => 1,
            b's' | b'S' | b'n' | b'v' => 2,
            b'i' | b'I' | b'l' | b'L' | b'N' | b'V' | b'f' | b'g' | b'G' => 4,
            b'q' | b'Q' | b'J' | b'P' | b'd' | b'e' | b'E' => 8,
            _ => {
                return Err(Unwind::value_error(format!(
                    "Invalid format type {}",
                    String::from_utf8_lossy(&[ty])
                )))
            }
        };
        if size != 0 && size != -1 && size < 0 {
            return overflow(ctx, ty);
        }

        let mut i: i64 = 0;
        while i != repetitions {
            if inputpos + size <= inputlen {
                let key: Vec<u8> = if repetitions == 1 && !name.is_empty() {
                    name.to_vec()
                } else {
                    let mut k = name.to_vec();
                    k.extend_from_slice((i + 1).to_string().as_bytes());
                    k
                };
                let p = inputpos as usize;
                let val: Option<Value> = match ty {
                    b'a' => {
                        let mut len = inputlen - inputpos;
                        if size >= 0 && len > size {
                            len = size;
                        }
                        size = len;
                        Some(Value::string(&input[p..p + len as usize]))
                    }
                    b'A' => {
                        let mut len = inputlen - inputpos;
                        if size >= 0 && len > size {
                            len = size;
                        }
                        size = len;
                        let mut end = len as usize;
                        while end > 0 && matches!(input[p + end - 1], 0 | b' ' | b'\t' | b'\r' | b'\n') {
                            end -= 1;
                        }
                        Some(Value::string(&input[p..p + end]))
                    }
                    b'Z' => {
                        let mut len = inputlen - inputpos;
                        if size >= 0 && len > size {
                            len = size;
                        }
                        size = len;
                        let slice = &input[p..p + len as usize];
                        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
                        Some(Value::string(&slice[..end]))
                    }
                    b'h' | b'H' => {
                        let mut len = (inputlen - inputpos) * 2;
                        if size >= 0 && len > size * 2 {
                            len = size * 2;
                        }
                        if len > 0 && argb > 0 {
                            len -= argb % 2;
                        }
                        let mut nibbleshift = if ty == b'h' { 0 } else { 4 };
                        let mut buf = Vec::with_capacity(len as usize);
                        let mut ipos = 0usize;
                        let mut first = true;
                        for _ in 0..len {
                            let cc = (input[p + ipos] >> nibbleshift) & 0xf;
                            buf.push(if cc < 10 { b'0' + cc } else { b'a' + cc - 10 });
                            nibbleshift = (nibbleshift + 4) & 7;
                            if first {
                                first = false;
                            } else {
                                ipos += 1;
                                first = true;
                            }
                        }
                        Some(Value::Str(Str::from_vec(buf)))
                    }
                    b'c' => Some(Value::Int(i64::from(input[p] as i8))),
                    b'C' => Some(Value::Int(i64::from(input[p]))),
                    b's' | b'S' | b'n' | b'v' => {
                        let b = [input[p], input[p + 1]];
                        let v = match ty {
                            b's' => i64::from(i16::from_le_bytes(b)),
                            b'n' => i64::from(u16::from_be_bytes(b)),
                            _ => i64::from(u16::from_le_bytes(b)),
                        };
                        Some(Value::Int(v))
                    }
                    b'i' | b'I' | b'l' | b'L' | b'N' | b'V' => {
                        let b = [input[p], input[p + 1], input[p + 2], input[p + 3]];
                        let v = match ty {
                            b'i' | b'l' => i64::from(i32::from_le_bytes(b)),
                            b'N' => i64::from(u32::from_be_bytes(b)),
                            _ => i64::from(u32::from_le_bytes(b)),
                        };
                        Some(Value::Int(v))
                    }
                    b'q' | b'Q' | b'J' | b'P' => {
                        let mut b = [0u8; 8];
                        b.copy_from_slice(&input[p..p + 8]);
                        let v = match ty {
                            b'J' => u64::from_be_bytes(b) as i64,
                            _ => u64::from_le_bytes(b) as i64,
                        };
                        Some(Value::Int(v))
                    }
                    b'f' | b'g' | b'G' => {
                        let b = [input[p], input[p + 1], input[p + 2], input[p + 3]];
                        let v = if ty == b'G' { f32::from_be_bytes(b) } else { f32::from_le_bytes(b) };
                        Some(Value::Float(f64::from(v)))
                    }
                    b'd' | b'e' | b'E' => {
                        let mut b = [0u8; 8];
                        b.copy_from_slice(&input[p..p + 8]);
                        let v = if ty == b'E' { f64::from_be_bytes(b) } else { f64::from_le_bytes(b) };
                        Some(Value::Float(v))
                    }
                    b'x' => None,
                    b'X' => None,
                    b'@' => {
                        if repetitions <= inputlen {
                            inputpos = repetitions;
                        } else {
                            ctx.warn("unpack(): Type @: outside of string")?;
                        }
                        i = repetitions - 1;
                        None
                    }
                    _ => None,
                };
                if let Some(v) = val {
                    if let Some(k) = array_key(&Value::Str(Str::from_vec(key))) {
                        out.set(k, v);
                    }
                }
                inputpos += size;
                if inputpos < 0 {
                    if size != -1 {
                        ctx.warn(&format!("unpack(): Type {}: outside of string", ty as char))?;
                    }
                    inputpos = 0;
                }
            } else if repetitions < 0 {
                // The `*` repeater stops at the end of the input.
                break;
            } else {
                let have = inputlen - inputpos;
                ctx.warn(&format!(
                    "unpack(): Type {}: not enough input values, need {} values but only {} {} provided",
                    ty as char,
                    size,
                    have,
                    if have == 1 { "was" } else { "were" }
                ))?;
                return Ok(Value::Bool(false));
            }
            i += 1;
        }
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use rphp_value::Value;

    use crate::tests::call_named;

    fn hex(v: &Value) -> String {
        v.to_php_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn pack_fixed_and_variable_codes() {
        let r = call_named(b"pack", &[Value::string(b"a5A5Z5"), Value::string(b"ab"), Value::string(b"cd"), Value::string(b"ef")]);
        assert_eq!(hex(&r), "616200000063642020206566000000");
        let r = call_named(b"pack", &[Value::string(b"h*H*"), Value::string(b"abc"), Value::string(b"ABC")]);
        assert_eq!(hex(&r), "ba0cabc0");
        let r = call_named(b"pack", &[Value::string(b"nvNVJP"), Value::Int(258), Value::Int(258), Value::Int(258), Value::Int(258), Value::Int(258), Value::Int(258)]);
        assert_eq!(hex(&r), "01020201000001020201000000000000000001020201000000000000");
        let r = call_named(b"pack", &[Value::string(b"a5@2a1"), Value::string(b"abcde"), Value::string(b"z")]);
        assert_eq!(hex(&r), "61627a");
    }

    #[test]
    fn unpack_names_and_repeaters() {
        let r = call_named(b"unpack", &[Value::string(b"Nlen/a*data"), Value::string(b"\0\0\0\x05hello")]);
        let Value::Array(a) = r else { panic!() };
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"len")), Some(&Value::Int(5)));
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"data")), Some(&Value::string(b"hello")));
        let r = call_named(b"unpack", &[Value::string(b"C2x/Cy"), Value::string(b"abc")]);
        let Value::Array(a) = r else { panic!() };
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"x2")), Some(&Value::Int(98)));
        assert_eq!(a.get(&rphp_value::ArrayKey::str(b"y")), Some(&Value::Int(99)));
        let r = call_named(b"unpack", &[Value::string(b"H3"), Value::string(b"\xba\x0c")]);
        let Value::Array(a) = r else { panic!() };
        assert_eq!(a.get(&rphp_value::ArrayKey::Int(1)), Some(&Value::string(b"ba0")));
    }
}
