//! Directory functions (ext/standard `dir.c`): listing, creating and removing
//! directories, and the process working directory.

use std::fs;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};
use rphp_value::{Array, Value};

use crate::filestat::{arg_path, invalidate};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("scandir", 1, Some(3), scandir),
    nf!("mkdir", 1, Some(4), mkdir),
    nf!("rmdir", 1, Some(2), rmdir),
    nf!("getcwd", 0, Some(0), getcwd),
    nf!("chdir", 1, Some(1), chdir),
];

/// php's `SCANDIR_SORT_DESCENDING`.
const SORT_DESCENDING: i64 = 1;
/// php's `SCANDIR_SORT_NONE`.
const SORT_NONE: i64 = 2;

/// `scandir(string $directory, int $sorting_order = SCANDIR_SORT_ASCENDING, ...): array|false`
///
/// Includes `.` and `..`, as php does.
fn scandir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let order = args.get(1).map_or(0, Value::to_int);
    let Ok(rd) = fs::read_dir(&p) else {
        let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
        ctx.warn(&format!(
            "scandir({shown}): Failed to open directory: No such file or directory"
        ))?;
        return Ok(Value::Bool(false));
    };
    let mut names: Vec<Vec<u8>> = vec![b".".to_vec(), b"..".to_vec()];
    for e in rd.flatten() {
        names.push(e.file_name().to_string_lossy().into_owned().into_bytes());
    }
    if order != SORT_NONE {
        names.sort();
        if order == SORT_DESCENDING {
            names.reverse();
        }
    }
    let mut a = Array::new();
    for n in names {
        a.push(Value::Str(rphp_value::Str::from_vec(n)));
    }
    Ok(Value::Array(a))
}

/// `mkdir(string $directory, int $permissions = 0777, bool $recursive = false, ...): bool`
fn mkdir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let recursive = args.get(2).is_some_and(Value::to_bool);
    let r = if recursive {
        fs::create_dir_all(&p)
    } else {
        fs::create_dir(&p)
    };
    invalidate(ctx, &p);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            // php names no path in `mkdir()`'s message.
            let _ = shown;
            ctx.warn(&format!("mkdir(): {}", crate::filestat::io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `rmdir(string $directory, ...): bool` — only removes an *empty* directory.
fn rmdir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let r = fs::remove_dir(&p);
    invalidate(ctx, &p);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!("rmdir({shown}): {}", crate::filestat::io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `getcwd(): string|false`
fn getcwd(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::string(ctx.cwd.to_string_lossy().as_bytes()))
}

/// `chdir(string $directory): bool` — moves the *interpreter's* cwd, which is
/// what relative paths resolve against.
fn chdir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    if !p.is_dir() {
        let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
        ctx.warn(&format!("chdir(): No such file or directory (errno 2), {shown}"))?;
        return Ok(Value::Bool(false));
    }
    ctx.cwd = fs::canonicalize(&p).unwrap_or(p);
    Ok(Value::Bool(true))
}

/// This module declares no constants yet.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}
