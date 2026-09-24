//! The `filestat` half of ext/standard: existence and metadata predicates
//! over the real filesystem.
//!
//! php keeps a **per-request stat cache** so repeated `file_exists`/`filesize`
//! on the same path do not re-`stat`; `clearstatcache()` drops it. It holds
//! exactly one path — the last one looked at — and this models that, because
//! a cache that held more would be stale where php is not (an entry whose
//! directory has since been moved away). Every function here that *changes*
//! a path empties the slot, whichever path it changed, as php's
//! `php_clear_stat_cache()` does.
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
    nf!("fileinode", 1, Some(1), fileinode),
    nf!("realpath", 1, Some(1), realpath),
    nf!("touch", 1, Some(3), touch),
    nf!("clearstatcache", 0, Some(2), clearstatcache),
    nf!("umask", 0, Some(1), umask),
    nf!("chmod", 2, Some(2), chmod),
];

/// Look `path` up through php's per-request stat cache
/// (`ExtState::stat_cache`).
fn stat(ctx: &mut Ctx, path: &Path) -> Option<fs::Metadata> {
    cached_stat(ctx, path)
}

/// php's one-entry stat cache: the last path stat'ed, and what it said.
/// Looking at a different path replaces it, exactly as php's does.
pub(crate) fn cached_stat(ctx: &mut Ctx, path: &Path) -> Option<fs::Metadata> {
    if let Some((cached, md)) = &ctx.ext.stat_cache {
        if cached == path {
            return md.clone();
        }
    }
    let md = fs::metadata(path).ok();
    // php's slot is one for every wrapper: a plain path displaces a url.
    crate::stream_wrappers::clear_stat_cache(ctx);
    ctx.ext.stat_cache = Some((path.to_path_buf(), md.clone()));
    md
}

/// Empty the stat cache, as php's `php_clear_stat_cache()` does, after
/// anything changed a path.
///
/// It takes no path because the slot holds one: a change to *any* path is a
/// change to what the cache knows, which is what makes `rename("a", "a2")`
/// answer `is_dir("a/b")` afresh — the shape Symfony's cache-directory
/// dance depends on.
pub(crate) fn clear_stat_cache(ctx: &mut Ctx) {
    ctx.ext.stat_cache = None;
    crate::stream_wrappers::clear_stat_cache(ctx);
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
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "file_exists", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(stat(ctx, &p).is_some()))
}

fn is_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "is_file", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(stat(ctx, &p).is_some_and(|m| m.is_file())))
}

fn is_dir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "is_dir", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(stat(ctx, &p).is_some_and(|m| m.is_dir())))
}

/// `is_link` needs the *unfollowed* stat, so it bypasses the cache.
fn is_link(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "is_link", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(
        fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()),
    ))
}

fn is_readable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "is_readable", args)? {
        return Ok(v);
    }
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
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "is_writable", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    Ok(Value::Bool(match stat(ctx, &p) {
        Some(m) => !m.permissions().readonly(),
        None => false,
    }))
}

fn is_executable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "is_executable", args)? {
        return Ok(v);
    }
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
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "filesize", args)? {
        return Ok(v);
    }
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
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "filemtime", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "filemtime", |m| secs(m.modified()))
}

fn fileatime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "fileatime", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "fileatime", |m| secs(m.accessed()))
}

fn filectime(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "filectime", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "filectime", |m| secs(m.created()))
}

fn fileperms(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "fileperms", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "fileperms", |m| Value::Int(i64::from(mode_of(m))))
}

/// `fileinode(string $filename): int|false`
#[cfg(unix)]
fn fileinode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "fileinode", args)? {
        return Ok(v);
    }
    use std::os::unix::fs::MetadataExt;
    stat_field(ctx, args, "fileinode", |m| Value::Int(m.ino() as i64))
}

/// `fileinode()` where there are no inodes: php answers `0`, which is what
/// its `stat` fills the field with.
#[cfg(not(unix))]
fn fileinode(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "fileinode", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "fileinode", |_| Value::Int(0))
}

fn filetype(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "filetype", args)? {
        return Ok(v);
    }
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
/// `realpath()` through php's realpath cache (shared with the include
/// path; `clearstatcache(true)` drops it).
fn realpath(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    Ok(match rphp_runtime::realpath_cached(&p) {
        Some(c) => Value::string(c.to_string_lossy().as_bytes()),
        None => Value::Bool(false),
    })
}

/// `touch(string $filename, ?int $mtime = null, ?int $atime = null): bool` —
/// creates the file when it does not exist.
fn touch(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "touch", args)? {
        return Ok(v);
    }
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
    clear_stat_cache(ctx);
    if !ok {
        let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
        ctx.warn(&format!("touch(): Unable to create file {shown}"))?;
    }
    Ok(Value::Bool(ok))
}

/// `clearstatcache(bool $clear_realpath_cache = false, string $filename = ""): void`
/// `clearstatcache(bool $clear_realpath_cache = false, string $filename = "")`.
fn clearstatcache(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // `$filename` narrows php's multi-entry cache to one path; ours *is* one
    // path, so either way the slot goes.
    clear_stat_cache(ctx);
    if args.first().is_some_and(|v| v.to_bool()) {
        rphp_runtime::clear_realpath_cache();
    }
    Ok(Value::Null)
}

/// This module declares no constants yet.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}

/// `umask(?int $mask = null): int` — set the process's file-creation mask and
/// answer the previous one; with no mask (or `null`), just answer the current
/// one, which is read the only way the C API allows: set it to `0`, then put
/// it back. Symfony's `GenericRuntime` calls `umask(0o000)` in debug mode.
fn umask(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mask = args
        .first()
        .map(|v| v.deref().into_owned())
        .filter(|v| !matches!(v, Value::Null | Value::Uninit))
        .map(|v| v.to_int());
    Ok(Value::Int(set_umask(mask)))
}

#[cfg(unix)]
fn set_umask(mask: Option<i64>) -> i64 {
    use rustix::fs::Mode;
    use rustix::process::umask;
    match mask {
        Some(m) => umask(Mode::from_bits_retain(m as u16)).bits().into(),
        // The C API has no "read" call: setting it to `0` answers the old
        // mask, which is then put back — what php's own `umask()` does.
        None => {
            let old = umask(Mode::empty());
            umask(old);
            old.bits().into()
        }
    }
}

/// Where there is no `umask(2)`, php's Windows build keeps the mask itself;
/// rphp has no Windows target yet, so this only keeps the code compiling.
#[cfg(not(unix))]
fn set_umask(_mask: Option<i64>) -> i64 {
    0
}

/// `chmod(string $filename, int $permissions): bool` — php reports the
/// failure reason through the warning channel and answers `false`.
fn chmod(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "chmod", args)? {
        return Ok(v);
    }
    let path = arg_path(ctx, &args[0]);
    let mode = args[1].to_int();
    match rustix::fs::chmod(&path, rustix::fs::Mode::from_bits_retain(mode as u16)) {
        Ok(()) => {
            clear_stat_cache(ctx);
            Ok(Value::Bool(true))
        }
        Err(e) => {
            ctx.warn(&format!("chmod(): {}", errno_text(e.raw_os_error())))?;
            Ok(Value::Bool(false))
        }
    }
}

/// php prints the C `strerror` text for a failed syscall, where Rust's
/// `io::Error` renders `No such file or directory (os error 2)` and `rustix`
/// hands back a bare errno. Every filesystem function that reports a failure
/// goes through here so the message is php's.
pub(crate) fn errno_text(code: i32) -> &'static str {
    match code {
        1 => "Operation not permitted",
        2 => "No such file or directory",
        13 => "Permission denied",
        17 => "File exists",
        20 => "Not a directory",
        21 => "Is a directory",
        22 => "Invalid argument",
        28 => "No space left on device",
        30 => "Read-only file system",
        62 => "Too many levels of symbolic links",
        63 => "File name too long",
        66 => "Directory not empty",
        _ => "Operation failed",
    }
}

/// The text php shows for a failed `std::fs` call.
pub(crate) fn io_text(e: &std::io::Error) -> &'static str {
    match e.raw_os_error() {
        Some(code) => errno_text(code),
        None => "Operation failed",
    }
}
