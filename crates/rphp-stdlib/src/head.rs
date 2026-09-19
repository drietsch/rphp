//! The response-header surface (php-src `ext/standard/head.c`).
//!
//! The CLI SAPI has nowhere to send a header, and php reflects that in two
//! ways rather than one: the *list* is still kept, so `headers_list()`
//! answers it, and the moment any output reaches the SAPI the headers count
//! as sent — after that every call here is php's
//! `Cannot modify header information - headers already sent by (output
//! started at FILE:LINE)` and does nothing. Output held in an `ob_*` level
//! has not been sent, so the same program with `ob_start()` can still set
//! headers, exactly as under a web SAPI.

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("header", 1, Some(3), header),
    nf!("header_remove", 0, Some(1), header_remove),
    nf!("headers_list", 0, Some(0), headers_list),
    nf_ref!("headers_sent", 0, Some(2), 0b11, headers_sent),
    nf!("http_response_code", 0, Some(1), http_response_code),
];

/// php's complaint, with the place the output started.
fn already_sent(ctx: &Ctx) -> String {
    match &ctx.output_started {
        Some((file, line)) => format!("(output started at {file}:{line})"),
        None => "(output started at unknown:0)".to_string(),
    }
}

/// Whether the headers are gone, warning the way `header()` does.
fn refuse(ctx: &mut Ctx) -> Result<bool, Unwind> {
    ctx.note_output();
    if !ctx.out.sent() {
        return Ok(false);
    }
    let msg = format!(
        "Cannot modify header information - headers already sent by {}",
        already_sent(ctx)
    );
    ctx.warn(&msg)?;
    Ok(true)
}

/// `header(string $header, bool $replace = true, int $response_code = 0): void`
fn header(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if refuse(ctx)? {
        return Ok(Value::Null);
    }
    let line = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let replace = args.get(1).is_none_or(|v| v.deref().to_bool());
    let code = args.get(2).map_or(0, |v| v.deref().to_int());
    if code > 0 {
        ctx.response_code = code;
    }
    // `HTTP/1.1 404 Not Found` is a status line, not a field: php reads the
    // code out of it and keeps the line.
    let name = match line.split_once(':') {
        Some((n, _)) => n.trim().to_string(),
        None => {
            if let Some(rest) = line.strip_prefix("HTTP/") {
                if let Some(code) = rest.split_whitespace().nth(1).and_then(|c| c.parse().ok()) {
                    ctx.response_code = code;
                }
            }
            line.clone()
        }
    };
    if replace {
        let lname = name.to_ascii_lowercase();
        ctx.headers
            .retain(|(n, _)| !n.eq_ignore_ascii_case(&lname));
    }
    ctx.headers.push((name, line));
    Ok(Value::Null)
}

/// `header_remove(?string $name = null): void` — one field, or all of them.
fn header_remove(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if refuse(ctx)? {
        return Ok(Value::Null);
    }
    match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Str(s)) => {
            let name = String::from_utf8_lossy(s.as_bytes()).into_owned();
            ctx.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        }
        _ => ctx.headers.clear(),
    }
    Ok(Value::Null)
}

/// `headers_list(): array` — the field lines, in the order they were set.
fn headers_list(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (_, line) in &ctx.headers {
        out.push(Value::string(line.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `headers_sent(&$filename = null, &$line = null): bool` — and where the
/// output that sent them started.
fn headers_sent(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.note_output();
    let sent = ctx.out.sent();
    let (file, line) = ctx
        .output_started
        .clone()
        .unwrap_or_else(|| (String::new(), 0));
    if let Some(slot) = args.first_mut() {
        Value::assign(slot, Value::string(file.as_bytes()));
    }
    if let Some(slot) = args.get_mut(1) {
        Value::assign(slot, Value::Int(i64::from(line)));
    }
    Ok(Value::Bool(sent))
}

/// `http_response_code(int $response_code = 0): int|bool` — the previous code,
/// or `false` when nothing has set one (which is the CLI's normal state).
fn http_response_code(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.note_output();
    let previous = ctx.response_code;
    let Some(code) = args.first().map(|v| v.deref().to_int()) else {
        return Ok(if previous == 0 {
            Value::Bool(false)
        } else {
            Value::Int(previous)
        });
    };
    if ctx.out.sent() {
        let msg = format!(
            "http_response_code(): Cannot set response code - headers already sent {}",
            already_sent(ctx)
        );
        ctx.warn(&msg)?;
        return Ok(Value::Bool(false));
    }
    ctx.response_code = code;
    Ok(if previous == 0 {
        Value::Bool(true)
    } else {
        Value::Int(previous)
    })
}

/// No constants: the `HTTP_*` names belong to other extensions.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}
