//! The `file` half of ext/standard: path arithmetic and whole-file I/O.
//!
//! The path functions (`dirname`, `basename`, `pathinfo`) live in
//! `string2.rs`, where php also groups them — they are pure string
//! arithmetic and never touch the filesystem.
//!
//! Reads and writes here go through the stat cache in `filestat.rs`, which
//! they invalidate whenever they change a path.

use std::fs;
use std::io::Write;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};
use rphp_value::{Array, Str, Value};

use crate::filestat::{arg_path, invalidate};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("file_get_contents", 1, Some(5), file_get_contents),
    nf!("file_put_contents", 2, Some(4), file_put_contents),
    nf!("file", 1, Some(3), file),
    nf!("readfile", 1, Some(3), readfile),
    nf!("unlink", 1, Some(2), unlink),
    nf!("copy", 2, Some(3), copy),
    nf!("rename", 2, Some(3), rename),
    nf!("stream_resolve_include_path", 1, Some(1), stream_resolve_include_path),
];

/// php's `FILE_*` flags that these functions honour.
const FILE_IGNORE_NEW_LINES: i64 = 2;
const FILE_SKIP_EMPTY_LINES: i64 = 4;
const FILE_APPEND: i64 = 8;

/// `file_get_contents(string $filename, ...): string|false`
fn file_get_contents(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match fs::read(&p) {
        Ok(b) => Ok(Value::Str(Str::from_vec(b))),
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "file_get_contents({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `file_put_contents(string $filename, mixed $data, int $flags = 0, ...): int|false`
///
/// Returns the **byte count**, not a bool.
fn file_put_contents(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    // An array argument is concatenated, as php does for `file()` output.
    let data: Vec<u8> = match &*args[1].deref() {
        Value::Array(a) => {
            let mut out = Vec::new();
            for (_, v) in a.iter() {
                out.extend_from_slice(&v.deref().to_php_bytes());
            }
            out
        }
        other => other.to_php_bytes().to_vec(),
    };
    let flags = args.get(2).map_or(0, Value::to_int);
    let r = if flags & FILE_APPEND != 0 {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .and_then(|mut f| f.write_all(&data))
    } else {
        fs::write(&p, &data)
    };
    invalidate(ctx, &p);
    match r {
        Ok(()) => Ok(Value::Int(data.len() as i64)),
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "file_put_contents({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// Split file contents into php's `file()` lines: the newline stays on the
/// line unless `FILE_IGNORE_NEW_LINES`.
fn split_lines(data: &[u8], flags: i64) -> Vec<Vec<u8>> {
    let keep_nl = flags & FILE_IGNORE_NEW_LINES == 0;
    let skip_empty = flags & FILE_SKIP_EMPTY_LINES != 0;
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            let end = if keep_nl { i + 1 } else { i };
            let mut line = data[start..end].to_vec();
            if !keep_nl && line.last() == Some(&b'\r') {
                line.pop();
            }
            if !(skip_empty && line.is_empty()) {
                out.push(line);
            }
            start = i + 1;
        }
    }
    if start < data.len() {
        let line = data[start..].to_vec();
        if !(skip_empty && line.is_empty()) {
            out.push(line);
        }
    }
    out
}

/// `file(string $filename, int $flags = 0, ...): array|false`
fn file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let flags = args.get(1).map_or(0, Value::to_int);
    match fs::read(&p) {
        Ok(b) => {
            let mut a = Array::new();
            for line in split_lines(&b, flags) {
                a.push(Value::Str(Str::from_vec(line)));
            }
            Ok(Value::Array(a))
        }
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "file({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `readfile(string $filename, ...): int|false` — write the file to output.
fn readfile(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match fs::read(&p) {
        Ok(b) => {
            let n = b.len() as i64;
            ctx.echo(&b);
            Ok(Value::Int(n))
        }
        Err(_) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "readfile({shown}): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `unlink(string $filename, ...): bool`
fn unlink(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let r = fs::remove_file(&p);
    invalidate(ctx, &p);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!("unlink({shown}): {e}"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `copy(string $from, string $to, ...): bool`
fn copy(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from = arg_path(ctx, &args[0]);
    let to = arg_path(ctx, &args[1]);
    let r = fs::copy(&from, &to);
    invalidate(ctx, &to);
    match r {
        Ok(_) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!("copy({shown}): {e}"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `rename(string $from, string $to, ...): bool`
fn rename(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let from = arg_path(ctx, &args[0]);
    let to = arg_path(ctx, &args[1]);
    let r = fs::rename(&from, &to);
    invalidate(ctx, &from);
    invalidate(ctx, &to);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!("rename({shown}): {e}"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `stream_resolve_include_path(string $filename): string|false` — the
/// canonical path when the file is reachable, `false` otherwise.
fn stream_resolve_include_path(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(match fs::canonicalize(&p) {
        Ok(c) => Value::string(c.to_string_lossy().as_bytes()),
        Err(_) => Value::Bool(false),
    })
}

/// php's `FILE_*` and `PATHINFO_*` constants.
pub(crate) fn register_constants(r: &mut rphp_runtime::Registry) {
    for (name, v) in [
        ("FILE_USE_INCLUDE_PATH", 1i64),
        ("FILE_IGNORE_NEW_LINES", FILE_IGNORE_NEW_LINES),
        ("FILE_SKIP_EMPTY_LINES", FILE_SKIP_EMPTY_LINES),
        ("FILE_APPEND", FILE_APPEND),
        ("FILE_NO_DEFAULT_CONTEXT", 16),
        ("LOCK_SH", 1),
        ("LOCK_EX", 2),
        ("LOCK_UN", 3),
        ("PATHINFO_DIRNAME", 1),
        ("PATHINFO_BASENAME", 2),
        ("PATHINFO_EXTENSION", 4),
        ("PATHINFO_FILENAME", 8),
    ] {
        r.constant(name, Value::Int(v));
    }
}
