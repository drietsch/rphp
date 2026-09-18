//! The `filestat` half of ext/standard: existence and metadata predicates
//! over the real filesystem.
//!
//! php keeps a **per-request stat cache** so repeated `file_exists`/`filesize`
//! on the same path do not re-`stat`; `clearstatcache()` drops it. The cache
//! is modelled here because code that writes a file and immediately re-stats
//! it depends on the invalidation — every function in this module that
//! *changes* a path clears its entry.
//!
//! A failing predicate returns `false` and, for the ones php warns about,
//! emits through the ordinary warning channel so `@` suppresses it.

use std::fs;
use std::path::{Path, PathBuf};

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};
use rphp_value::Value;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("file_exists", 1, Some(1), file_exists),
    nf!("is_file", 1, Some(1), is_file),
    nf!("is_dir", 1, Some(1), is_dir),
    nf!("is_link", 1, Some(1), is_link),
    nf!("is_readable", 1, Some(1), is_readable),
    nf!("is_writable", 1, Some(1), is_writable),
    nf!("is_writeable", 1, Some(1), is_writable),
    nf!("is_executable", 1, Some(1), is_executable),
    nf!("filesize", 1, Some(1), filesize),
    nf!("filemtime", 1, Some(1), filemtime),
    nf!("fileatime", 1, Some(1), fileatime),
    nf!("filectime", 1, Some(1), filectime),
    nf!("filetype", 1, Some(1), filetype),
    nf!("fileperms", 1, Some(1), fileperms),
    nf!("realpath", 1, Some(1), realpath),
    nf!("touch", 1, Some(3), touch),
    nf!("clearstatcache", 0, Some(2), clearstatcache),
];

/// Look `path` up through php's per-request stat cache
/// (`ExtState::stat_cache`).
fn stat(ctx: &mut Ctx, path: &Path) -> Option<fs::Metadata> {
    if let Some(hit) = ctx.ext.stat_cache.get(path) {
        return hit.clone();
    }
    let md = fs::metadata(path).ok();
    ctx.ext.stat_cache.insert(path.to_path_buf(), md.clone());
    md
}

/// Drop `path` from the stat cache (after anything that changes it).
pub(crate) fn invalidate(ctx: &mut Ctx, path: &Path) {
    ctx.ext.stat_cache.remove(path);
}

/// The argument as a path, resolved against the interpreter's cwd so a
/// relative path means the same thing it does to php.
pub(crate) fn arg_path(ctx: &Ctx, v: &Value) -> PathBuf {
    let bytes = v.to_php_bytes();
    let s = String::from_utf8_lossy(&bytes).into_owned();
    let p = PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        ctx.cwd.join(p)
    }
}

fn file_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(stat(ctx, &p).is_some()))
}

fn is_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(stat(ctx, &p).is_some_and(|m| m.is_file())))
}

fn is_dir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(stat(ctx, &p).is_some_and(|m| m.is_dir())))
}

/// `is_link` needs the *unfollowed* stat, so it bypasses the cache.
fn is_link(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(
        fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()),
    ))
}

fn is_readable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    // Readability is "can I open it", which is the honest test on every
    // platform; the permission bits alone would lie under ACLs.
    Ok(Value::Bool(match stat(ctx, &p) {
        Some(m) if m.is_dir() => fs::read_dir(&p).is_ok(),
        Some(_) => fs::File::open(&p).is_ok(),
        None => false,
    }))
}

fn is_writable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(match stat(ctx, &p) {
        Some(m) => !m.permissions().readonly(),
        None => false,
    }))
}

fn is_executable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(match stat(ctx, &p) {
        Some(m) => mode_of(&m) & 0o111 != 0,
        None => false,
    }))
}

/// The unix mode bits of a metadata, 0 where the platform has none.
fn mode_of(m: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        0
    }
}

/// The shared shape of the metadata accessors: `false` **and a warning** when
/// the path does not exist, which is what php does for all of them.
fn stat_field(
    ctx: &mut Ctx,
    args: &[Value],
    func: &str,
    f: impl Fn(&fs::Metadata) -> Value,
) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match stat(ctx, &p) {
        Some(m) => Ok(f(&m)),
        None => {
            let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "{func}(): stat failed for {shown}"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

fn filesize(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stat_field(ctx, args, "filesize", |m| Value::Int(m.len() as i64))
}

/// Seconds since the epoch from a filesystem timestamp.
fn secs(t: std::io::Result<std::time::SystemTime>) -> Value {
    match t
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        Some(d) => Value::Int(d.as_secs() as i64),
        None => Value::Bool(false),
    }
}

fn filemtime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stat_field(ctx, args, "filemtime", |m| secs(m.modified()))
}

fn fileatime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stat_field(ctx, args, "fileatime", |m| secs(m.accessed()))
}

fn filectime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stat_field(ctx, args, "filectime", |m| secs(m.created()))
}

fn fileperms(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stat_field(ctx, args, "fileperms", |m| Value::Int(i64::from(mode_of(m))))
}

fn filetype(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    stat_field(ctx, args, "filetype", |m| {
        let t = m.file_type();
        let s: &[u8] = if t.is_dir() {
            b"dir"
        } else if t.is_symlink() {
            b"link"
        } else {
            b"file"
        };
        Value::string(s)
    })
}

/// `realpath(string $path): string|false` — canonical path, `false` when it
/// does not exist.
fn realpath(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(match fs::canonicalize(&p) {
        Ok(c) => Value::string(c.to_string_lossy().as_bytes()),
        Err(_) => Value::Bool(false),
    })
}

/// `touch(string $filename, ?int $mtime = null, ?int $atime = null): bool` —
/// creates the file when it does not exist.
fn touch(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    let ok = if p.exists() {
        true
    } else {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .is_ok()
    };
    invalidate(ctx, &p);
    if !ok {
        let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
        ctx.warn(&format!("touch(): Unable to create file {shown}"))?;
    }
    Ok(Value::Bool(ok))
}

/// `clearstatcache(bool $clear_realpath_cache = false, string $filename = ""): void`
fn clearstatcache(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match args.get(1) {
        Some(v) if !v.to_php_bytes().is_empty() => {
            let p = arg_path(ctx, v);
            invalidate(ctx, &p);
        }
        _ => ctx.ext.stat_cache.clear(),
    }
    Ok(Value::Null)
}

/// This module declares no constants yet.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}
