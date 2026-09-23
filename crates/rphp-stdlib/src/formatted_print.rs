//! The `printf` family (php-src `ext/standard/formatted_print.c`): `sprintf`,
//! `printf`, `vsprintf`, `vprintf`. One byte-oriented formatter behind all
//! four, modelled on `php_formatted_print`: positional `%n$`, the `-`/`+`/
//! `0`/` `/`'x` flags, `*` width and precision (PHP 8), and the conversions
//! `b c d e E f F g G h H o s u x X` (`%i` is not one of php's). Missing arguments are collected over the
//! whole format and reported once (`ArgumentCountError`), format faults are
//! `ValueError`s with php's texts. `fprintf`/`vfprintf` wait on streams.
use rphp_value::{Str, Value};

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("sprintf", 1, None, sprintf),
    nf!("printf", 1, None, printf),
    nf!("vsprintf", 2, Some(2), vsprintf),
    nf!("vprintf", 2, Some(2), vprintf),
    nf!("fprintf", 2, None, fprintf),
    nf!("vfprintf", 3, Some(3), vfprintf),
];

/// php's `INT_MAX`: the bound on argument numbers, widths and precisions.
const INT_MAX: i64 = 2_147_483_647;
/// The largest precision a float conversion accepts before the notice.
const MAX_FLOAT_PRECISION: usize = 53;

/// How the "arguments required" error is phrased: `sprintf`/`printf` count the
/// format as an argument; the `v*` variants describe the values array.
#[derive(Clone, Copy)]
enum ArgStyle {
    /// `N arguments are required, M given` with `extra` leading parameters.
    Positional { extra: usize },
    /// `The arguments array must contain N items, M given`.
    Array,
}

/// Lay out `%e`/`%E`: PHP uses a signed exponent with no leading zeros and a
/// minimum of one digit (e.g. `1.234568e+4`), unlike C's two-digit exponent.
fn format_exp(value: f64, prec: usize, upper: bool) -> Vec<u8> {
    let s = format!("{:.*e}", prec, value);
    let (mant, exp) = s.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let echar = if upper { b'E' } else { b'e' };
    let mut out = mant.as_bytes().to_vec();
    out.push(echar);
    out.push(if e < 0 { b'-' } else { b'+' });
    out.extend_from_slice(e.unsigned_abs().to_string().as_bytes());
    out
}

/// PHP's `%g`/`%G` (modeled on php_gcvt + zend_dtoa mode 2): render with at most
/// `ndigit` significant digits, switching to exponential form when the decimal
/// exponent is `< -4` or `>= ndigit`. Trailing zeros are dropped — except that an
/// exact half-way value rounded down to `ndigit` digits keeps them (so `%.4g` of
/// 71905 is `"7.190e+4"`), matching zend_dtoa. Exponential mantissas force a `.0`.
pub(crate) fn php_gcvt(value: f64, ndigit: usize, upper: bool) -> Vec<u8> {
    if value == 0.0 {
        return vec![b'0'];
    }
    // Shortest round-tripping significant digits. An exact tie at `ndigit` shows
    // up here as exactly `ndigit + 1` digits ending in '5'.
    let short = format!("{:e}", value);
    let (sm, _) = short.split_once('e').expect("LowerExp always has 'e'");
    let mut sd: Vec<u8> = sm.bytes().filter(|&b| b != b'.').collect();
    while sd.len() > 1 && *sd.last().unwrap() == b'0' {
        sd.pop();
    }
    let is_tie = sd.len() == ndigit + 1 && sd[ndigit] == b'5';
    // Round to exactly `ndigit` significant digits.
    let sci = format!("{:.*e}", ndigit.saturating_sub(1), value);
    let (mant, exp) = sci.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let mut digits: Vec<u8> = mant.bytes().filter(|&b| b != b'.').collect();
    let decpt = e + 1;
    let nd = ndigit as i32;
    let e_style = decpt > nd || decpt < -3;
    // Keep the rounding-induced trailing zeros only for an exact tie shown in
    // exponential form (never for a power-of-ten carry, whose tail is all zeros).
    let keep = is_tie && e_style && digits[1..].iter().any(|&b| b != b'0');
    if !keep {
        while digits.len() > 1 && *digits.last().unwrap() == b'0' {
            digits.pop();
        }
    }
    gcvt_layout(&digits, decpt, e_style, upper)
}

/// `%g` with precision -1: the shortest round-tripping digits laid out with
/// php's 17-digit threshold for exponential form.
fn php_gcvt_shortest(value: f64, upper: bool) -> Vec<u8> {
    if value == 0.0 {
        return vec![b'0'];
    }
    let short = format!("{:e}", value);
    let (mant, exp) = short.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let digits: Vec<u8> = mant.bytes().filter(|&b| b != b'.').collect();
    let decpt = e + 1;
    let e_style = !(-3..=17).contains(&decpt);
    gcvt_layout(&digits, decpt, e_style, upper)
}

/// Lay out significant `digits` with the decimal point after `decpt` of them:
/// `d.ddde±X` when `e_style`, otherwise plain fixed notation.
fn gcvt_layout(digits: &[u8], decpt: i32, e_style: bool, upper: bool) -> Vec<u8> {
    let exp_char = if upper { b'E' } else { b'e' };
    let mut out = Vec::new();
    if e_style {
        let mut d = decpt - 1;
        let sign = if d < 0 {
            d = -d;
            b'-'
        } else {
            b'+'
        };
        out.push(digits[0]);
        out.push(b'.');
        if digits.len() == 1 {
            out.push(b'0');
        } else {
            out.extend_from_slice(&digits[1..]);
        }
        out.push(exp_char);
        out.push(sign);
        out.extend_from_slice(d.to_string().as_bytes());
    } else if decpt < 0 {
        // "0.00ddd": -decpt leading fractional zeros.
        out.push(b'0');
        out.push(b'.');
        out.resize(out.len() + (-decpt) as usize, b'0');
        out.extend_from_slice(digits);
    } else {
        let dp = decpt as usize;
        let mut idx = 0;
        for _ in 0..dp {
            out.push(*digits.get(idx).unwrap_or(&b'0'));
            if idx < digits.len() {
                idx += 1;
            }
        }
        if idx < digits.len() {
            if idx == 0 {
                out.push(b'0');
            }
            out.push(b'.');
            out.extend_from_slice(&digits[idx..]);
        }
    }
    out
}

/// Parse a run of decimal digits at `*i`, returning `None` (php's `-1`) when
/// the value exceeds `INT_MAX`.
fn get_number(format: &[u8], i: &mut usize) -> Option<i64> {
    let mut n: i64 = 0;
    while *i < format.len() && format[*i].is_ascii_digit() {
        n = n.saturating_mul(10).saturating_add((format[*i] - b'0') as i64);
        *i += 1;
    }
    (n <= INT_MAX).then_some(n)
}

/// `php_sprintf_appendstring`: pad `body` (already carrying its sign, if any,
/// in `prefix`) to `width`. Right alignment with `'0'` padding puts the sign
/// before the zeros; every other case pads outside the whole thing.
fn append_padded(
    out: &mut Vec<u8>,
    prefix: &[u8],
    body: &[u8],
    width: usize,
    pad: u8,
    left: bool,
) {
    let content = prefix.len() + body.len();
    if content >= width {
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
        return;
    }
    let padlen = width - content;
    if left {
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(pad, padlen));
    } else if pad == b'0' {
        out.extend_from_slice(prefix);
        out.extend(std::iter::repeat_n(b'0', padlen));
        out.extend_from_slice(body);
    } else {
        out.extend(std::iter::repeat_n(pad, padlen));
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
    }
}

/// The shared engine behind the family. Operates entirely on bytes so binary
/// strings round-trip; `args` are the values after the format.
fn do_sprintf(
    ctx: &mut Ctx,
    func: &str,
    format: &[u8],
    args: &[Value],
    style: ArgStyle,
) -> Result<Vec<u8>, Unwind> {
    let mut out = Vec::new();
    let n = format.len();
    let mut i = 0;
    // Sequential argument cursor (0-based) and the highest missing index seen.
    let mut currarg = 0usize;
    let mut max_missing: Option<usize> = None;
    let missing = |idx: usize, max_missing: &mut Option<usize>| {
        *max_missing = Some(max_missing.map_or(idx, |m| m.max(idx)));
    };
    while i < n {
        if format[i] != b'%' {
            out.push(format[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i < n && format[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }
        // Optional positional "N$".
        let mut argnum: Option<usize> = None;
        {
            let mut j = i;
            while j < n && format[j].is_ascii_digit() {
                j += 1;
            }
            if j > i && j < n && format[j] == b'$' {
                let mut k = i;
                let num = get_number(format, &mut k);
                match num {
                    Some(v) if v > 0 => argnum = Some((v - 1) as usize),
                    _ => {
                        return Err(Unwind::value_error(format!(
                            "Argument number specifier must be greater than zero and less than {INT_MAX}"
                        )))
                    }
                }
                i = j + 1;
            }
        }
        // Flags.
        let mut left = false;
        let mut plus = false;
        let mut pad = b' ';
        loop {
            match format.get(i) {
                Some(b' ') | Some(b'0') => pad = format[i],
                Some(b'-') => left = true,
                Some(b'+') => plus = true,
                Some(b'\'') => {
                    if i + 1 < n {
                        i += 1;
                        pad = format[i];
                    } else {
                        return Err(Unwind::value_error("Missing padding character"));
                    }
                }
                _ => break,
            }
            i += 1;
        }
        // Resolve an argument index for a `*` width/precision: `*N$` or the
        // next sequential one.
        let star_arg = |i: &mut usize, currarg: &mut usize| -> Result<usize, Unwind> {
            let mut j = *i;
            while j < n && format[j].is_ascii_digit() {
                j += 1;
            }
            if j > *i && j < n && format[j] == b'$' {
                let mut k = *i;
                let num = get_number(format, &mut k);
                *i = j + 1;
                match num {
                    Some(v) if v > 0 => Ok((v - 1) as usize),
                    _ => Err(Unwind::value_error(format!(
                        "Argument number specifier must be greater than zero and less than {INT_MAX}"
                    ))),
                }
            } else {
                let a = *currarg;
                *currarg += 1;
                Ok(a)
            }
        };
        // Width.
        let mut width = 0usize;
        if i < n && format[i] == b'*' {
            i += 1;
            let a = star_arg(&mut i, &mut currarg)?;
            match args.get(a) {
                None => missing(a, &mut max_missing),
                Some(Value::Int(w)) => {
                    if *w < 0 || *w > INT_MAX {
                        return Err(Unwind::value_error(format!(
                            "Width must be between 0 and {INT_MAX}"
                        )));
                    }
                    width = *w as usize;
                }
                Some(_) => return Err(Unwind::value_error("Width must be an integer")),
            }
        } else if i < n && format[i].is_ascii_digit() {
            match get_number(format, &mut i) {
                Some(w) => width = w as usize,
                None => {
                    return Err(Unwind::value_error(format!(
                        "Width must be between 0 and {INT_MAX}"
                    )))
                }
            }
        }
        // Precision (`shortest` is the `*` value -1, valid only for %g-family).
        let mut precision: Option<usize> = None;
        let mut shortest = false;
        if i < n && format[i] == b'.' {
            i += 1;
            if i < n && format[i] == b'*' {
                i += 1;
                let a = star_arg(&mut i, &mut currarg)?;
                match args.get(a) {
                    None => missing(a, &mut max_missing),
                    Some(Value::Int(p)) => {
                        if *p < -1 || *p > INT_MAX {
                            return Err(Unwind::value_error(format!(
                                "Precision must be between -1 and {INT_MAX}"
                            )));
                        }
                        if *p < 0 {
                            shortest = true;
                        } else {
                            precision = Some(*p as usize);
                        }
                    }
                    Some(_) => {
                        return Err(Unwind::value_error("Precision must be an integer"))
                    }
                }
            } else if i < n && format[i].is_ascii_digit() {
                match get_number(format, &mut i) {
                    Some(p) => precision = Some(p as usize),
                    None => {
                        return Err(Unwind::value_error(format!(
                            "Precision must be between 0 and {INT_MAX}"
                        )))
                    }
                }
            } else {
                precision = Some(0);
            }
        }
        // One `l` length modifier is accepted and ignored.
        if format.get(i) == Some(&b'l') {
            i += 1;
        }
        let Some(&conv) = format.get(i) else {
            return Err(Unwind::value_error("Missing format specifier at end of string"));
        };
        i += 1;
        // Resolve the argument for this conversion (php takes the sequential
        // slot before it looks at the conversion, so `% %` skips an argument).
        let idx = match argnum {
            Some(a) => a,
            None => {
                let a = currarg;
                currarg += 1;
                a
            }
        };
        // A `%` conversion (after flags/width) is a literal percent sign.
        if conv == b'%' {
            out.push(b'%');
            continue;
        }
        let arg = match args.get(idx) {
            Some(a) => a.deref().into_owned(),
            None => {
                // php records the shortage and skips the conversion (its
                // specifier is not even validated); the format keeps being
                // scanned so a later fault can still win.
                missing(idx, &mut max_missing);
                continue;
            }
        };

        let signed = |neg: bool| -> Vec<u8> {
            if neg {
                vec![b'-']
            } else if plus {
                vec![b'+']
            } else {
                Vec::new()
            }
        };

        // A float conversion's precision: default 6, capped at 53 with a notice.
        let float_prec = |ctx: &mut Ctx, default: usize| -> Result<usize, Unwind> {
            match precision {
                None => Ok(default),
                Some(p) if p > MAX_FLOAT_PRECISION => {
                    ctx.notice(&format!(
                        "{func}(): Requested precision of {p} digits was truncated to PHP maximum of {MAX_FLOAT_PRECISION} digits"
                    ))?;
                    Ok(MAX_FLOAT_PRECISION)
                }
                Some(p) => Ok(p),
            }
        };

        match conv {
            b's' => {
                // `%s` of an object is its `__toString()`, as `(string)` is.
                let mut b = match &*arg.deref() {
                    Value::Object(_) => ctx.to_string(&arg)?.as_bytes().to_vec(),
                    _ => arg.to_php_bytes(),
                };
                if let Some(p) = precision {
                    if b.len() > p {
                        b.truncate(p);
                    }
                }
                append_padded(&mut out, &[], &b, width, pad, left);
            }
            b'd' => {
                let v = arg.to_int();
                let mag = (v as i128).unsigned_abs();
                // Integers are never right-padded with zeros.
                let p = if left && pad == b'0' { b' ' } else { pad };
                append_padded(&mut out, &signed(v < 0), mag.to_string().as_bytes(), width, p, left);
            }
            b'u' => {
                let p = if left && pad == b'0' { b' ' } else { pad };
                append_padded(&mut out, &[], (arg.to_int() as u64).to_string().as_bytes(), width, p, left);
            }
            b'x' | b'X' | b'o' | b'b' => {
                // php passes a zero maximum width to its string appender when
                // a precision was given, so the digits vanish and only the
                // padding remains (`%04.4o` is "0000").
                let digits = if precision.is_some() {
                    String::new()
                } else {
                    let v = arg.to_int() as u64;
                    match conv {
                        b'x' => format!("{v:x}"),
                        b'X' => format!("{v:X}"),
                        b'o' => format!("{v:o}"),
                        _ => format!("{v:b}"),
                    }
                };
                append_padded(&mut out, &[], digits.as_bytes(), width, pad, left);
            }
            // `%c` ignores width and padding entirely.
            b'c' => out.push((arg.to_int() & 0xff) as u8),
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'h' | b'H' => {
                if shortest && !matches!(conv, b'g' | b'G' | b'h' | b'H') {
                    return Err(Unwind::value_error(
                        "Precision -1 is only supported for %g, %G, %h and %H",
                    ));
                }
                let f = arg.to_float();
                // NaN/Inf are never padded and never get a `+`.
                if f.is_nan() {
                    out.extend_from_slice(b"NaN");
                    continue;
                }
                if f.is_infinite() {
                    if f < 0.0 {
                        out.push(b'-');
                    }
                    out.extend_from_slice(b"INF");
                    continue;
                }
                // The sign follows `f < 0` (so -0.0 prints unsigned, while a
                // negative value rounding to zero keeps its `-`).
                let neg = f < 0.0;
                let body = match conv {
                    b'f' | b'F' => {
                        let prec = float_prec(ctx, 6)?;
                        format!("{:.*}", prec, f.abs()).into_bytes()
                    }
                    b'e' | b'E' => {
                        let prec = float_prec(ctx, 6)?;
                        format_exp(f.abs(), prec, conv == b'E')
                    }
                    _ if shortest => php_gcvt_shortest(f.abs(), conv == b'G' || conv == b'H'),
                    _ => {
                        let nd = float_prec(ctx, 6)?.max(1);
                        php_gcvt(f.abs(), nd, conv == b'G' || conv == b'H')
                    }
                };
                append_padded(&mut out, &signed(neg), &body, width, pad, left);
            }
            other => {
                return Err(Unwind::value_error(format!(
                    "Unknown format specifier \"{}\"",
                    other as char
                )));
            }
        }
        if shortest && !matches!(conv, b'g' | b'G' | b'h' | b'H') {
            return Err(Unwind::value_error(
                "Precision -1 is only supported for %g, %G, %h and %H",
            ));
        }
    }
    if let Some(m) = max_missing {
        return Err(match style {
            ArgStyle::Positional { extra } => Unwind::argument_count_error(format!(
                "{} arguments are required, {} given",
                m + extra + 1,
                args.len() + extra
            )),
            ArgStyle::Array => Unwind::value_error(format!(
                "The arguments array must contain {} items, {} given",
                m + 1,
                args.len()
            )),
        });
    }
    Ok(out)
}

/// The values of the `$values` array argument of `vsprintf`/`vprintf`.
fn values_array(func: &str, v: &Value) -> Result<Vec<Value>, Unwind> {
    match &*v.deref() {
        Value::Array(a) => Ok(a.iter().map(|(_, v)| v.deref().into_owned()).collect()),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #2 ($values) must be of type array, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// `sprintf(string $format, mixed ...$values): string`.
pub(crate) fn sprintf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let format = args[0].to_php_bytes();
    let rendered = do_sprintf(ctx, "sprintf", &format, &args[1..], ArgStyle::Positional { extra: 1 })?;
    Ok(Value::Str(Str::from_vec(rendered)))
}

/// `printf(string $format, mixed ...$values): int` — writes the rendered
/// string and returns its byte length.
pub(crate) fn printf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let format = args[0].to_php_bytes();
    let rendered = do_sprintf(ctx, "printf", &format, &args[1..], ArgStyle::Positional { extra: 1 })?;
    let len = rendered.len() as i64;
    ctx.out().extend_from_slice(&rendered);
    Ok(Value::Int(len))
}

/// `vsprintf(string $format, array $values): string`.
pub(crate) fn vsprintf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let format = args[0].to_php_bytes();
    let vals = values_array("vsprintf", &args[1])?;
    let rendered = do_sprintf(ctx, "vsprintf", &format, &vals, ArgStyle::Array)?;
    Ok(Value::Str(Str::from_vec(rendered)))
}

/// `vprintf(string $format, array $values): int`.
pub(crate) fn vprintf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let format = args[0].to_php_bytes();
    let vals = values_array("vprintf", &args[1])?;
    let rendered = do_sprintf(ctx, "vprintf", &format, &vals, ArgStyle::Array)?;
    let len = rendered.len() as i64;
    ctx.out().extend_from_slice(&rendered);
    Ok(Value::Int(len))
}

/// The stream argument of `fprintf()`/`vfprintf()`, or php's TypeError.
fn stream_arg(func: &str, v: &Value) -> Result<Value, Unwind> {
    match &*v.deref() {
        Value::Resource(_) => Ok(v.clone()),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// Write what `fprintf()`/`vfprintf()` rendered through `fwrite()` — so a
/// stream's filters see it — and answer the rendered length, which php
/// returns whatever the write did.
fn write_rendered(ctx: &mut Ctx, stream: Value, rendered: Vec<u8>) -> NativeResult {
    let len = rendered.len() as i64;
    ctx.call_function(b"fwrite", &[stream, Value::Str(Str::from_vec(rendered))])?;
    Ok(Value::Int(len))
}

/// `fprintf(resource $stream, string $format, mixed ...$values): int`.
pub(crate) fn fprintf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = stream_arg("fprintf", &args[0])?;
    let format = args[1].to_php_bytes();
    let rendered = do_sprintf(ctx, "fprintf", &format, &args[2..], ArgStyle::Positional { extra: 2 })?;
    write_rendered(ctx, stream, rendered)
}

/// `vfprintf(resource $stream, string $format, array $values): int`.
pub(crate) fn vfprintf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = stream_arg("vfprintf", &args[0])?;
    let format = args[1].to_php_bytes();
    let vals = match &*args[2].deref() {
        Value::Array(a) => a.iter().map(|(_, v)| v.deref().into_owned()).collect::<Vec<_>>(),
        other => {
            return Err(Unwind::type_error(format!(
                "vfprintf(): Argument #3 ($values) must be of type array, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    };
    let rendered = do_sprintf(ctx, "vfprintf", &format, &vals, ArgStyle::Array)?;
    write_rendered(ctx, stream, rendered)
}

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14/3.14159 here are sprintf inputs, not pi
mod tests {
    use crate::tests::{call_err, call_named};
    use rphp_runtime::ErrorKind;
    use rphp_value::Value;

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    fn fmt(format: &str, args: &[Value]) -> String {
        let mut all = vec![s(format)];
        all.extend_from_slice(args);
        call_named(b"sprintf", &all).to_php_string()
    }

    #[test]
    fn padding_rules_per_conversion() {
        assert_eq!(fmt("%-05d|", &[Value::Int(-42)]), "-42  |");
        assert_eq!(fmt("%-05s|", &[s("ab")]), "ab000|");
        assert_eq!(fmt("%-08.2f|", &[Value::Float(-3.14)]), "-3.14000|");
        assert_eq!(fmt("%05d", &[Value::Int(-5)]), "-0005");
        assert_eq!(fmt("%'#5d", &[Value::Int(-5)]), "###-5");
        assert_eq!(fmt("%5c|", &[Value::Int(65)]), "A|");
        assert_eq!(fmt("%+d|%+d", &[Value::Int(5), Value::Int(-5)]), "+5|-5");
    }

    #[test]
    fn star_width_and_precision() {
        assert_eq!(
            fmt("%*d|%-*d|%.*f", &[Value::Int(5), Value::Int(42), Value::Int(4), Value::Int(7), Value::Int(2), Value::Float(3.14159)]),
            "   42|7   |3.14"
        );
        assert_eq!(fmt("%2$*1$d|", &[Value::Int(4), Value::Int(1)]), "   1|");
    }

    #[test]
    fn missing_arguments_are_counted_over_the_whole_format() {
        let e = call_err(b"sprintf", &[s("%s %s %s")]);
        assert_eq!(e.kind(), Some(ErrorKind::ArgumentCountError));
        assert_eq!(e.message(), Some("4 arguments are required, 1 given"));
        // A missing argument skips its specifier unvalidated; with the
        // arguments present the bad specifier is a ValueError.
        let e = call_err(b"sprintf", &[s("%s %v")]);
        assert_eq!(e.message(), Some("3 arguments are required, 1 given"));
        let e = call_err(b"sprintf", &[s("%s %v"), s("a"), s("b")]);
        assert_eq!(e.message(), Some("Unknown format specifier \"v\""));
        let e = call_err(b"vsprintf", &[s("%d %d"), crate::tests::arr(&[Value::Int(1)])]);
        assert_eq!(e.kind(), Some(ErrorKind::ValueError));
        assert_eq!(e.message(), Some("The arguments array must contain 2 items, 1 given"));
        let e = call_err(b"sprintf", &[s("%0$s"), s("a")]);
        assert_eq!(
            e.message(),
            Some("Argument number specifier must be greater than zero and less than 2147483647")
        );
        assert_eq!(call_err(b"sprintf", &[s("abc%"), s("a")]).message(), Some("Missing format specifier at end of string"));
        assert_eq!(call_err(b"sprintf", &[s("%'"), s("a")]).message(), Some("Missing padding character"));
    }

    #[test]
    fn floats_and_specials() {
        assert_eq!(fmt("%.0f|%.0f|%.2f", &[Value::Float(2.5), Value::Float(3.5), Value::Float(1.005)]), "2|4|1.00");
        assert_eq!(fmt("%e", &[Value::Float(0.0)]), "0.000000e+0");
        assert_eq!(fmt("%.4g|%g|%G", &[Value::Float(71905.0), Value::Float(1e-5), Value::Float(1e-10)]), "7.190e+4|1.0e-5|1.0E-10");
        assert_eq!(fmt("%h %H", &[Value::Float(1234.5678), Value::Float(0.00001234)]), "1234.57 1.234E-5");
        assert_eq!(fmt("%5.1f|%f|%+f", &[Value::Float(f64::INFINITY), Value::Float(f64::NAN), Value::Float(f64::NEG_INFINITY)]), "INF|NaN|-INF");
        assert_eq!(fmt("%.1f|%d", &[Value::Float(-0.04), Value::Float(-0.0)]), "-0.0|0");
    }
}
