//! The filesystem tail of ext/standard: the parts of `file.c` and
//! `filestat.c` that need a syscall the whole-file functions never make —
//! temporary files, `glob(3)`, hard and symbolic links, the raw `stat`
//! buffer, ownership, filesystem free space, and the handle operations that
//! act on an open stream rather than on its bytes.
//!
//! ## Reaching a stream from here
//!
//! `file.rs` owns `Stream` — a byte buffer, a cursor, and php's end-of-file
//! flag, which is set by a read that came back short and cleared by a seek.
//! Its fields and its `with_stream` helper are private to that module, so the
//! handle functions here reach a stream the only other way the engine offers:
//! by calling the registered natives (`fgets`, `fwrite`, `fseek`,
//! `stream_get_meta_data`, …) back through the interpreter. That is the very
//! code a PHP script would run, so the model is respected rather than
//! re-implemented — but a diagnostic raised *inside* one of those calls would
//! name the inner function, so every function that can fail on the handle
//! first re-derives readability and writability from the mode string the
//! handle reports, exactly as `fopen` derives them, and raises php's notice
//! under its own name. Nothing here delegates to a call that can then fail.
//!
//! ## Known divergences
//!
//! * `ftruncate()` and `tmpfile()` are absent: both need to shorten or own a
//!   `Stream`'s buffer, which no registered native exposes.
//! * `fgetcsv()`'s `$length` is ignored, because `file.rs`'s `fgets()`
//!   ignores its own.
//! * `fstat()` on a `php://` handle reports php's synthetic memory-stream
//!   buffer; on `php://stdout`/`stderr` php reports the real stat of the
//!   process's descriptor, which this model has no descriptor for.
//! * `chown()`/`chgrp()` resolve a *name* through `/etc/passwd` and
//!   `/etc/group`; php uses `getpwnam(3)`/`getgrnam(3)`, which on macOS also
//!   asks Open Directory, so a name that lives only there is reported as
//!   `Unable to find uid for …`. Numeric ids are unaffected.
//! * `flock()` locks a second descriptor on the handle's path rather than the
//!   handle's own, because the model keeps no descriptor. Two handles on one
//!   file still conflict, which is what `flock(2)` does in php; what differs
//!   is that `fclose()` does not drop the lock — it lives until `LOCK_UN` or
//!   the end of the process.
//!
//! Platform: the `GLOB_*` values, `blksize`/`blocks` in a `stat` array and
//! `flock`'s semantics are all what php answers on **this macOS build** and
//! are read from the platform rather than hard-coded, except `GLOB_ONLYDIR`,
//! which php defines itself (`1 << 30`) wherever `glob(3)` lacks it — macOS
//! included — and emulates with a `stat` over the results.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

use crate::filestat::{arg_path, clear_stat_cache, errno_text, io_text};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("tempnam", 2, Some(2), tempnam),
    nf!("glob", 1, Some(2), glob),
    nf!("link", 2, Some(2), link),
    nf!("symlink", 2, Some(2), symlink),
    nf!("readlink", 1, Some(1), readlink),
    nf!("linkinfo", 1, Some(1), linkinfo),
    nf!("stat", 1, Some(1), stat),
    nf!("lstat", 1, Some(1), lstat),
    nf!("fstat", 1, Some(1), fstat),
    nf!("chown", 2, Some(2), chown),
    nf!("chgrp", 2, Some(2), chgrp),
    nf!("fileowner", 1, Some(1), fileowner),
    nf!("filegroup", 1, Some(1), filegroup),
    nf!("disk_free_space", 1, Some(1), disk_free_space),
    nf!("diskfreespace", 1, Some(1), diskfreespace),
    nf!("disk_total_space", 1, Some(1), disk_total_space),
    // `$would_block` is the only by-reference parameter in this module.
    nf_ref!("flock", 2, Some(3), 0b100, flock),
    nf!("fpassthru", 1, Some(1), fpassthru),
    nf!("ftruncate", 2, Some(2), ftruncate),
    nf!("tmpfile", 0, Some(0), tmpfile),
    nf!("fsync", 1, Some(1), fsync),
    nf!("fdatasync", 1, Some(1), fdatasync),
    // `fscanf($stream, $format, &...$vars)`: every position from #2 on is
    // written back, as `sscanf` does.
    nf_ref!("fscanf", 2, None, 0xFFFF_FFFC, fscanf),
    nf!("fgetcsv", 1, Some(5), fgetcsv),
    nf!("fputcsv", 2, Some(6), fputcsv),
];

/// php's `GLOB_*`. `GLOB_ONLYDIR` is php's own bit, not the platform's; the
/// rest are BSD `glob(3)`'s values, which is what php exposes on macOS.
const GLOB_ERR: i64 = 4;
const GLOB_MARK: i64 = 8;
const GLOB_NOCHECK: i64 = 16;
const GLOB_NOSORT: i64 = 32;
const GLOB_BRACE: i64 = 128;
const GLOB_NOESCAPE: i64 = 4096;
const GLOB_ONLYDIR: i64 = 1 << 30;
const GLOB_AVAILABLE_FLAGS: i64 =
    GLOB_ERR | GLOB_MARK | GLOB_NOCHECK | GLOB_NOSORT | GLOB_BRACE | GLOB_NOESCAPE | GLOB_ONLYDIR;

/// php's `LOCK_NB`. `LOCK_SH`/`LOCK_EX`/`LOCK_UN` are declared in `file.rs`.
const LOCK_NB: i64 = 4;

/// php's `GLOB_*` and `LOCK_NB`.
pub(crate) fn register_constants(r: &mut rphp_runtime::Registry) {
    for (name, v) in [
        ("GLOB_ERR", GLOB_ERR),
        ("GLOB_MARK", GLOB_MARK),
        ("GLOB_NOCHECK", GLOB_NOCHECK),
        ("GLOB_NOSORT", GLOB_NOSORT),
        ("GLOB_BRACE", GLOB_BRACE),
        ("GLOB_NOESCAPE", GLOB_NOESCAPE),
        ("GLOB_ONLYDIR", GLOB_ONLYDIR),
        ("GLOB_AVAILABLE_FLAGS", GLOB_AVAILABLE_FLAGS),
        ("LOCK_NB", LOCK_NB),
    ] {
        r.constant(name, Value::Int(v));
    }
}

/// The argument as php prints it in a diagnostic: the string the caller
/// passed, never the path it resolved to.
fn shown(v: &Value) -> String {
    String::from_utf8_lossy(&v.to_php_bytes()).into_owned()
}

/// Look `path` up through php's one-entry stat cache (`filestat.rs`).
fn cached_stat(ctx: &mut Ctx, path: &Path) -> Option<fs::Metadata> {
    crate::filestat::cached_stat(ctx, path)
}

// ---- stat ------------------------------------------------------------------

/// The named half of php's stat array, in php's order. The numeric half is
/// the same thirteen values under keys `0`–`12`, which is why the array has
/// 26 entries and not 13.
const STAT_KEYS: [&str; 13] = [
    "dev", "ino", "mode", "nlink", "uid", "gid", "rdev", "size", "atime", "mtime", "ctime",
    "blksize", "blocks",
];

/// php's 26-entry stat array: the thirteen values first under their numeric
/// keys, then again under their names.
pub(crate) fn stat_array(vals: &[i64; 13]) -> Array {
    let mut a = Array::new();
    for v in vals {
        a.push(Value::Int(*v));
    }
    for (i, key) in STAT_KEYS.iter().enumerate() {
        a.set(ArrayKey::str(key.as_bytes()), Value::Int(vals[i]));
    }
    a
}

/// `struct stat` in php's order. `blksize` and `blocks` are the platform's:
/// on this macOS build an APFS file reports a 4096-byte block size and a
/// directory reports zero blocks.
#[cfg(unix)]
fn stat_values(md: &fs::Metadata) -> [i64; 13] {
    use std::os::unix::fs::MetadataExt;
    [
        md.dev() as i64,
        md.ino() as i64,
        i64::from(md.mode()),
        md.nlink() as i64,
        i64::from(md.uid()),
        i64::from(md.gid()),
        md.rdev() as i64,
        md.size() as i64,
        md.atime(),
        md.mtime(),
        md.ctime(),
        md.blksize() as i64,
        md.blocks() as i64,
    ]
}

/// Where there is no `struct stat`, only the size is knowable; rphp has no
/// Windows target yet, so this only keeps the code compiling.
#[cfg(not(unix))]
fn stat_values(md: &fs::Metadata) -> [i64; 13] {
    [0, 0, 0, 1, 0, 0, 0, md.len() as i64, 0, 0, 0, 0, 0]
}

/// `stat(string $filename): array|false`
fn stat(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "stat", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    match cached_stat(ctx, &p) {
        Some(md) => Ok(Value::Array(stat_array(&stat_values(&md)))),
        None => {
            ctx.warn(&format!("stat(): stat failed for {}", shown(&args[0])))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `lstat(string $filename): array|false` — the *unfollowed* stat, so it
/// bypasses the cache the way `is_link()` does; php capitalises this one
/// message and no other.
fn lstat(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "lstat", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    match fs::symlink_metadata(&p) {
        Ok(md) => Ok(Value::Array(stat_array(&stat_values(&md)))),
        Err(_) => {
            ctx.warn(&format!("lstat(): Lstat failed for {}", shown(&args[0])))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `fstat(resource $stream): array` — the stat of the file behind the handle.
/// A `php://` handle has no file, and php answers a synthetic buffer whose
/// `dev` is 12 (`/dev/null`, chosen by php so nothing can collide with it),
/// `rdev`/`blksize`/`blocks` are `-1` and whose mode is `0100666`, or
/// `0100444` when the handle cannot be written.
fn fstat(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A user wrapper's stream answers for itself.
    if let Some(v) = crate::stream_wrappers::handle_op(ctx, "fstat", args)? {
        return Ok(v);
    }
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, "fstat")?;
    if meta.plainfile {
        if let Ok(md) = fs::metadata(Path::new(&meta.uri)) {
            return Ok(Value::Array(stat_array(&stat_values(&md))));
        }
    }
    let size = buffer_len(ctx, &stream, meta.eof)?;
    let mode = if meta.writable() { 0o100_666 } else { 0o100_444 };
    Ok(Value::Array(stat_array(&[
        12, 0, mode, 1, 0, 0, -1, size, 0, 0, 0, -1, -1,
    ])))
}

/// The length of a handle's buffer, read the only way the public stream API
/// allows: seek to the end, ask, seek back. A seek clears php's end-of-file
/// flag, so a handle that had it set is left at the end with a short read
/// that sets it again — the only state the round trip can disturb.
fn buffer_len(ctx: &mut Ctx, stream: &Value, eof: bool) -> Result<i64, Unwind> {
    let pos = ctx.call_function(b"ftell", &[stream.clone()])?.to_int();
    ctx.call_function(b"fseek", &[stream.clone(), Value::Int(0), Value::Int(2)])?;
    let end = ctx.call_function(b"ftell", &[stream.clone()])?.to_int();
    ctx.call_function(b"fseek", &[stream.clone(), Value::Int(pos), Value::Int(0)])?;
    if eof && pos == end {
        ctx.call_function(b"fread", &[stream.clone(), Value::Int(1)])?;
    }
    Ok(end)
}

/// `fileowner(string $filename): int|false`
fn fileowner(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "fileowner", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "fileowner", 4)
}

/// `filegroup(string $filename): int|false`
fn filegroup(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "filegroup", args)? {
        return Ok(v);
    }
    stat_field(ctx, args, "filegroup", 5)
}

/// One field of the stat buffer, with the warning php raises for every
/// metadata accessor when the path is not there.
fn stat_field(ctx: &mut Ctx, args: &[Value], func: &str, field: usize) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match cached_stat(ctx, &p) {
        Some(md) => Ok(Value::Int(stat_values(&md)[field])),
        None => {
            ctx.warn(&format!("{func}(): stat failed for {}", shown(&args[0])))?;
            Ok(Value::Bool(false))
        }
    }
}

// ---- ownership -------------------------------------------------------------

/// `chown(string $filename, string|int $user): bool`
fn chown(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "chown", args)? {
        return Ok(v);
    }
    set_owner(ctx, args, true)
}

/// `chgrp(string $filename, string|int $group): bool`
fn chgrp(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "chgrp", args)? {
        return Ok(v);
    }
    set_owner(ctx, args, false)
}

/// The shared body. php decides between an id and a name on the *type* of the
/// argument, not on its contents: `chown($f, "0")` is a lookup of the user
/// named `0` and fails, while `chown($f, 0)` is uid 0.
fn set_owner(ctx: &mut Ctx, args: &mut [Value], user: bool) -> NativeResult {
    let func = if user { "chown" } else { "chgrp" };
    let by_name = matches!(&*args[1].deref(), Value::Str(_));
    let id = if by_name {
        let name = args[1].to_php_bytes();
        match lookup_id(&name, user) {
            Some(id) => id,
            None => {
                let what = if user { "uid" } else { "gid" };
                let asked = String::from_utf8_lossy(&name).into_owned();
                ctx.warn(&format!("{func}(): Unable to find {what} for {asked}"))?;
                return Ok(Value::Bool(false));
            }
        }
    } else {
        args[1].to_int() as u32
    };
    let path = arg_path(ctx, &args[0]);
    let r = apply_owner(&path, user, id);
    clear_stat_cache(ctx);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            ctx.warn(&format!("{func}(): {}", io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// A user or group name in `/etc/passwd` / `/etc/group` (see the module
/// header for what this misses next to `getpwnam(3)`). Both files put the id
/// in the third colon-separated field.
pub(crate) fn lookup_id(name: &[u8], user: bool) -> Option<u32> {
    let file = if user { "/etc/passwd" } else { "/etc/group" };
    let data = fs::read(file).ok()?;
    for line in data.split(|&b| b == b'\n') {
        let mut fields = line.split(|&b| b == b':');
        if fields.next() != Some(name) {
            continue;
        }
        if let Some(id) = fields.nth(1) {
            return std::str::from_utf8(id).ok()?.trim().parse().ok();
        }
    }
    None
}

#[cfg(unix)]
fn apply_owner(path: &Path, user: bool, id: u32) -> std::io::Result<()> {
    // php's `chown` follows a symlink; `lchown` is the separate function.
    if user {
        std::os::unix::fs::chown(path, Some(id), None)
    } else {
        std::os::unix::fs::chown(path, None, Some(id))
    }
}

#[cfg(not(unix))]
fn apply_owner(_path: &Path, _user: bool, _id: u32) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

// ---- free space ------------------------------------------------------------

/// `disk_free_space(string $directory): float|false`
fn disk_free_space(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    disk_space(ctx, args, "disk_free_space", false)
}

/// `diskfreespace(string $directory): float|false` — the alias, which names
/// *itself* in its warning.
fn diskfreespace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    disk_space(ctx, args, "diskfreespace", false)
}

/// `disk_total_space(string $directory): float|false`
fn disk_total_space(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    disk_space(ctx, args, "disk_total_space", true)
}

/// php multiplies the fragment size by the block count — `f_bavail` for free
/// space, because that is what an unprivileged process may actually use, and
/// `f_blocks` for the total. The result is a float even when it is integral.
fn disk_space(ctx: &mut Ctx, args: &[Value], func: &str, total: bool) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match rustix::fs::statvfs(&p) {
        Ok(v) => {
            let unit = if v.f_frsize != 0 { v.f_frsize } else { v.f_bsize };
            let blocks = if total { v.f_blocks } else { v.f_bavail };
            Ok(Value::Float(unit as f64 * blocks as f64))
        }
        Err(e) => {
            ctx.warn(&format!("{func}(): {}", errno_text(e.raw_os_error())))?;
            Ok(Value::Bool(false))
        }
    }
}

// ---- temporary files -------------------------------------------------------

/// `tempnam(string $directory, string $prefix): string|false` — an empty file
/// nobody else holds, readable and writable only by its owner.
///
/// php runs the prefix through `basename` and cuts it to 63 bytes, so a
/// prefix carrying a path cannot place the file somewhere else, and it
/// resolves the directory, which is why the answer on macOS comes back under
/// `/private/var` rather than `/var`.
fn tempnam(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let asked = args[0].to_php_bytes();
    let mut pfx = basename_bytes(&args[1].to_php_bytes());
    pfx.truncate(63);
    let pfx = String::from_utf8_lossy(&pfx).into_owned();

    let mut dir = if asked.is_empty() {
        None
    } else {
        fs::canonicalize(arg_path(ctx, &args[0]))
            .ok()
            .filter(|d| d.is_dir())
    };
    // An unusable directory is not an error: php says so and falls back. An
    // *empty* directory argument was never a request, so it says nothing.
    if dir.is_none() && !asked.is_empty() {
        ctx.notice("tempnam(): file created in the system's temporary directory")?;
    }
    if dir.is_none() {
        dir = system_temp_dir(ctx)?;
    }
    let Some(dir) = dir else {
        return Ok(Value::Bool(false));
    };
    for _ in 0..16 {
        let path = dir.join(format!("{pfx}{}", random_suffix()));
        if create_private(&path).is_ok() {
            clear_stat_cache(ctx);
            return Ok(Value::string(path.to_string_lossy().as_bytes()));
        }
    }
    Ok(Value::Bool(false))
}

/// php's `php_basename` as far as `tempnam` needs it.
fn basename_bytes(p: &[u8]) -> Vec<u8> {
    let trimmed = match p.iter().rposition(|&c| c != b'/') {
        Some(i) => &p[..=i],
        None => p,
    };
    match trimmed.iter().rposition(|&c| c == b'/') {
        Some(i) => trimmed[i + 1..].to_vec(),
        None => trimmed.to_vec(),
    }
}

/// The resolved system temporary directory, or `None` when there is none to
/// fall back to.
fn system_temp_dir(ctx: &mut Ctx) -> Result<Option<PathBuf>, Unwind> {
    let mut no_args: [Value; 0] = [];
    let v = crate::info::sys_get_temp_dir(ctx, &mut no_args)?;
    let p = PathBuf::from(String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    Ok(fs::canonicalize(&p).ok().filter(|d| d.is_dir()))
}

/// The 19 characters of `[0-9A-Za-z]` php appends to a temporary name.
fn random_suffix() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut buf = [0u8; 19];
    let read = fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf));
    if read.is_err() {
        // Only reached where there is no kernel entropy device; `O_EXCL`
        // still makes the name unique, this only makes it unpredictable.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (t >> (i % 8 * 8)) as u8 ^ i as u8;
        }
    }
    buf.iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

/// Create the file only if the name is still free, then take away every
/// permission but the owner's — php's temporary files are `0600`.
fn create_private(path: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    set_private(path)
}

#[cfg(unix)]
fn set_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

// ---- links -----------------------------------------------------------------

/// `link(string $target, string $link): bool` — a hard link, so both names
/// resolve against the interpreter's cwd.
fn link(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let target = arg_path(ctx, &args[0]);
    let path = arg_path(ctx, &args[1]);
    let r = fs::hard_link(&target, &path);
    clear_stat_cache(ctx);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            ctx.warn(&format!("link(): {}", io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `symlink(string $target, string $link): bool`
///
/// The target is stored byte for byte, never resolved: a relative target is
/// read by the kernel against the link's own directory, so resolving it here
/// against the cwd would silently change what the link points at.
fn symlink(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let target = PathBuf::from(String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned());
    let path = arg_path(ctx, &args[1]);
    let r = make_symlink(&target, &path);
    clear_stat_cache(ctx);
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            ctx.warn(&format!("symlink(): {}", io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, path: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, _path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// `readlink(string $path): string|false` — what the link stores, which is
/// `EINVAL` (php: `Invalid argument`) for anything that is not a link.
fn readlink(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match fs::read_link(&p) {
        Ok(t) => Ok(Value::string(t.to_string_lossy().as_bytes())),
        Err(e) => {
            ctx.warn(&format!("readlink(): {}", io_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `linkinfo(string $path): int|false` — the `st_dev` of the *unfollowed*
/// stat, which php uses only as "does this path exist at all". The failure
/// answer is `-1`, not `false`, which is php's C return leaking through.
fn linkinfo(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let p = arg_path(ctx, &args[0]);
    match fs::symlink_metadata(&p) {
        Ok(md) => Ok(Value::Int(stat_values(&md)[0])),
        Err(e) => {
            ctx.warn(&format!("linkinfo(): {}", io_text(&e)))?;
            Ok(Value::Int(-1))
        }
    }
}

// ---- glob ------------------------------------------------------------------

/// `glob(string $pattern, int $flags = 0): array|false`
///
/// A pattern that matches nothing is an empty array, never `false`: php
/// swallows `GLOB_NOMATCH` so `foreach (glob(…) as …)` needs no check. Only
/// an unsupported flag produces `false`.
fn glob(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let pattern = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let flags = args.get(1).map_or(0, Value::to_int);
    if flags & !GLOB_AVAILABLE_FLAGS != 0 {
        ctx.warn(
            "glob(): At least one of the passed flags is invalid or not supported on this platform",
        )?;
        return Ok(Value::Bool(false));
    }
    let noescape = flags & GLOB_NOESCAPE != 0;
    let cwd = ctx.cwd.clone();
    let patterns = if flags & GLOB_BRACE != 0 {
        brace_expand(&pattern, noescape)
    } else {
        vec![pattern]
    };
    let mut out = Array::new();
    // Each brace alternative is globbed on its own and sorted on its own, so
    // `{b,a}` answers in brace order — BSD `glob(3)`, which php inherits.
    for p in patterns {
        let mut hits = Vec::new();
        glob_one(&cwd, &p, noescape, &mut hits);
        if hits.is_empty() {
            if flags & GLOB_NOCHECK == 0 {
                continue;
            }
            hits.push(unescape(&p, noescape));
        } else {
            if flags & GLOB_MARK != 0 {
                for h in &mut hits {
                    if !h.ends_with('/') && cwd.join(&*h).is_dir() {
                        h.push('/');
                    }
                }
            }
            if flags & GLOB_NOSORT == 0 {
                hits.sort();
            }
        }
        for h in hits {
            if flags & GLOB_ONLYDIR != 0 && !cwd.join(&h).is_dir() {
                continue;
            }
            out.push(Value::string(h.as_bytes()));
        }
    }
    Ok(Value::Array(out))
}

/// Match one brace-free pattern against the filesystem.
fn glob_one(cwd: &Path, pattern: &str, noescape: bool, out: &mut Vec<String>) {
    if pattern.is_empty() {
        return;
    }
    let (prefix, rest) = match pattern.strip_prefix('/') {
        Some(rest) => ("/", rest),
        None => ("", pattern),
    };
    let parts: Vec<&str> = rest.split('/').collect();
    glob_walk(cwd, prefix, &parts, noescape, out);
}

/// Walk the pattern one path component at a time. A component with no
/// wildcard is appended without reading its directory, exactly as `glob(3)`
/// does — which is why `glob("nodir/*")` is simply empty and not an error.
fn glob_walk(cwd: &Path, prefix: &str, parts: &[&str], noescape: bool, out: &mut Vec<String>) {
    if parts.is_empty() {
        if prefix.is_empty() {
            return;
        }
        let p = cwd.join(prefix);
        // A trailing `/` in the pattern demands a directory; otherwise the
        // path only has to exist *as a name*, so a dangling symlink matches.
        let ok = if prefix.ends_with('/') {
            fs::metadata(&p).is_ok_and(|m| m.is_dir())
        } else {
            fs::symlink_metadata(&p).is_ok()
        };
        if ok {
            out.push(prefix.to_string());
        }
        return;
    }
    let part = parts[0];
    let rest = &parts[1..];
    if !has_magic(part, noescape) {
        glob_walk(cwd, &join_glob(prefix, &unescape(part, noescape)), rest, noescape, out);
        return;
    }
    let dir = if prefix.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(prefix)
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    // `readdir(3)` hands `glob(3)` `.` and `..` as well, and the pattern can
    // match them — which is why `glob(".*")` lists both.
    let mut names: Vec<String> = vec![".".to_string(), "..".to_string()];
    names.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
    for name in names {
        if fnmatch(part.as_bytes(), name.as_bytes(), noescape) {
            glob_walk(cwd, &join_glob(prefix, &name), rest, noescape, out);
        }
    }
}

/// Append one component to a partial match, keeping the separator the pattern
/// had (so a relative pattern answers relative paths).
fn join_glob(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if prefix.ends_with('/') {
        format!("{prefix}{name}")
    } else {
        format!("{prefix}/{name}")
    }
}

/// Whether a component has to be matched against a directory listing.
fn has_magic(pat: &str, noescape: bool) -> bool {
    let b = pat.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'*' | b'?' | b'[' => return true,
            b'\\' if !noescape => i += 2,
            _ => i += 1,
        }
    }
    false
}

/// Drop the backslashes that were only there to quote a wildcard.
fn unescape(pat: &str, noescape: bool) -> String {
    if noescape || !pat.contains('\\') {
        return pat.to_string();
    }
    let mut out = String::with_capacity(pat.len());
    let mut chars = pat.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// `glob(3)`'s matcher for one path component.
fn fnmatch(pat: &[u8], name: &[u8], noescape: bool) -> bool {
    // A leading dot is only matched by a literal dot, so `*` never picks up a
    // hidden entry while `.*` picks up `.` and `..` with it.
    if name.first() == Some(&b'.') {
        let literal_dot = match pat.first() {
            Some(b'.') => true,
            Some(b'\\') if !noescape => pat.get(1) == Some(&b'.'),
            _ => false,
        };
        if !literal_dot {
            return false;
        }
    }
    fnmatch_at(pat, name, noescape)
}

fn fnmatch_at(pat: &[u8], name: &[u8], noescape: bool) -> bool {
    let mut p = 0;
    let mut n = 0;
    while p < pat.len() {
        match pat[p] {
            b'*' => {
                while p < pat.len() && pat[p] == b'*' {
                    p += 1;
                }
                if p == pat.len() {
                    return true;
                }
                return (n..=name.len()).any(|k| fnmatch_at(&pat[p..], &name[k..], noescape));
            }
            b'?' => {
                if n >= name.len() {
                    return false;
                }
                p += 1;
                n += 1;
            }
            b'[' => {
                if n >= name.len() {
                    return false;
                }
                match class_match(&pat[p..], name[n], noescape) {
                    Some((consumed, hit)) => {
                        if !hit {
                            return false;
                        }
                        p += consumed;
                        n += 1;
                    }
                    // A `[` that is never closed is an ordinary character.
                    None => {
                        if name[n] != b'[' {
                            return false;
                        }
                        p += 1;
                        n += 1;
                    }
                }
            }
            b'\\' if !noescape && p + 1 < pat.len() => {
                if n >= name.len() || name[n] != pat[p + 1] {
                    return false;
                }
                p += 2;
                n += 1;
            }
            c => {
                if n >= name.len() || name[n] != c {
                    return false;
                }
                p += 1;
                n += 1;
            }
        }
    }
    n == name.len()
}

/// A `[…]` class at the start of `pat` against one byte: how many bytes the
/// class took and whether it matched, or `None` when the bracket never
/// closes. Only `!` negates — BSD's matcher does not know `^`, so
/// `glob("[^a].txt")` really does match a file called `a.txt`.
fn class_match(pat: &[u8], c: u8, noescape: bool) -> Option<(usize, bool)> {
    let mut i = 1;
    let negate = pat.get(i) == Some(&b'!');
    if negate {
        i += 1;
    }
    let mut hit = false;
    let mut first = true;
    loop {
        let &b = pat.get(i)?;
        // A `]` straight after the bracket (or after the `!`) is literal.
        if b == b']' && !first {
            i += 1;
            break;
        }
        first = false;
        let lo = if b == b'\\' && !noescape {
            i += 1;
            *pat.get(i)?
        } else {
            b
        };
        i += 1;
        if pat.get(i) == Some(&b'-') && pat.get(i + 1).is_some_and(|&x| x != b']') {
            i += 1;
            let hi = if pat[i] == b'\\' && !noescape {
                i += 1;
                *pat.get(i)?
            } else {
                pat[i]
            };
            i += 1;
            if (lo..=hi).contains(&c) {
                hit = true;
            }
        } else if lo == c {
            hit = true;
        }
    }
    Some((i, hit != negate))
}

/// `GLOB_BRACE`: expand the left-most brace, then each result again. A brace
/// that is never closed is left alone and matched literally.
fn brace_expand(pattern: &str, noescape: bool) -> Vec<String> {
    let b = pattern.as_bytes();
    let mut i = 0;
    let open = loop {
        if i >= b.len() {
            return vec![pattern.to_string()];
        }
        match b[i] {
            b'\\' if !noescape => i += 2,
            b'{' => break i,
            _ => i += 1,
        }
    };
    let mut depth = 0usize;
    let mut alts: Vec<&str> = Vec::new();
    let mut start = open + 1;
    let mut j = open;
    let close = loop {
        if j >= b.len() {
            return vec![pattern.to_string()];
        }
        match b[j] {
            b'\\' if !noescape => j += 2,
            b'{' => {
                depth += 1;
                j += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    break j;
                }
                j += 1;
            }
            b',' if depth == 1 => {
                alts.push(&pattern[start..j]);
                start = j + 1;
                j += 1;
            }
            _ => j += 1,
        }
    };
    alts.push(&pattern[start..close]);
    let head = &pattern[..open];
    let tail = &pattern[close + 1..];
    let mut out = Vec::new();
    for a in alts {
        out.extend(brace_expand(&format!("{head}{a}{tail}"), noescape));
    }
    out
}

// ---- open handles ----------------------------------------------------------

/// What `stream_get_meta_data()` tells this module about a handle. Reading it
/// back through the public native is how the module sees a `Stream` at all.
struct Meta {
    uri: String,
    mode: String,
    /// A real file (`plainfile`), as against a `php://` handle.
    plainfile: bool,
    /// A `php://memory` / `php://temp` buffer, as against `php://stdout` and
    /// friends, which have no buffer to read back.
    buffered: bool,
    eof: bool,
}

impl Meta {
    /// Whether a read can reach the handle, derived from the mode exactly as
    /// `fopen` derives it — a `php://` buffer ignores the mode for reading.
    fn readable(&self) -> bool {
        if self.plainfile {
            self.mode.contains('r') || self.mode.contains('+')
        } else {
            self.buffered
        }
    }

    /// Whether a write can reach the handle. The output handles always take
    /// one; everything else goes by the mode.
    fn writable(&self) -> bool {
        if self.plainfile || self.buffered {
            let m = &self.mode;
            m.contains('w') || m.contains('a') || m.contains('x') || m.contains('c') || m.contains('+')
        } else {
            true
        }
    }
}

/// One string out of a meta-data array.
fn meta_str(a: &Array, key: &str) -> String {
    a.get(&ArrayKey::str(key.as_bytes()))
        .map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned())
        .unwrap_or_default()
}

/// Describe the handle in `v`, raising php's `TypeError` under `func`'s own
/// name when it is not a stream at all.
fn stream_meta(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Meta, Unwind> {
    let wrong = match &*v.deref() {
        Value::Resource(r) if r.kind() == "stream" => None,
        other => Some(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(&other)
        ))),
    };
    if let Some(e) = wrong {
        return Err(e);
    }
    // A user wrapper's stream is described without asking it (php's
    // functions that read through one never look at its metadata).
    if crate::stream_wrappers::is_user_stream(v) {
        return Ok(Meta { uri: String::new(), mode: "r+".into(), plainfile: false, buffered: true, eof: false });
    }
    let meta = ctx.call_function(b"stream_get_meta_data", &[v.clone()])?;
    let Value::Array(a) = meta else {
        return Err(Unwind::error(format!(
            "{func}(): supplied resource is not a valid stream"
        )));
    };
    let kind = meta_str(&a, "stream_type");
    Ok(Meta {
        uri: meta_str(&a, "uri"),
        mode: meta_str(&a, "mode"),
        plainfile: meta_str(&a, "wrapper_type") == "plainfile",
        buffered: kind == "MEMORY" || kind == "TEMP",
        eof: a
            .get(&ArrayKey::str(b"eof"))
            .is_some_and(Value::to_bool),
    })
}

/// The resource id behind a stream value, which is what a held lock is keyed
/// on.
fn resource_id(v: &Value) -> Option<u32> {
    match &*v.deref() {
        Value::Resource(r) => Some(r.id()),
        _ => None,
    }
}

thread_local! {
    /// The descriptors holding an advisory lock, by stream resource id. The
    /// model keeps no descriptor of its own, so `flock` opens one and holds
    /// it here until `LOCK_UN` (see the module header).
    static LOCKS: RefCell<HashMap<u32, fs::File>> = RefCell::new(HashMap::new());
}

/// How one locking attempt ended.
enum Lock {
    Taken,
    WouldBlock,
    Failed,
}

/// `flock(resource $stream, int $operation, int &$would_block = null): bool`
///
/// php reads the *operation* out of the low two bits, so `flock($f, 99)` is a
/// perfectly good `LOCK_UN` (99 & 3 == 3) while `flock($f, 0)` throws.
fn flock(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, "flock")?;
    let operation = args[1].to_int();
    let act = operation & 3;
    if !(1..=3).contains(&act) {
        return Err(Unwind::value_error(
            "flock(): Argument #2 ($operation) must be one of LOCK_SH, LOCK_EX, or LOCK_UN",
        ));
    }
    // A user wrapper's stream answers for itself.
    if let Some(v) = crate::stream_wrappers::handle_op(ctx, "flock", args)? {
        return Ok(v);
    }
    // php clears `$would_block` before it tries, so a success leaves 0 behind
    // even when the variable held something else.
    if args.len() > 2 {
        args[2] = Value::Int(0);
    }
    let Some(id) = resource_id(&stream).filter(|_| meta.plainfile) else {
        // A memory stream cannot be locked, and php says so only by
        // answering `false`.
        return Ok(Value::Bool(false));
    };
    if act == 3 {
        LOCKS.with(|m| {
            if let Some(f) = m.borrow_mut().remove(&id) {
                let _ = f.unlock();
            }
        });
        return Ok(Value::Bool(true));
    }
    let shared = act == 1;
    let nonblocking = operation & LOCK_NB != 0;
    let outcome = LOCKS.with(|m| {
        let mut held = m.borrow_mut();
        if !held.contains_key(&id) {
            match fs::OpenOptions::new().read(true).open(Path::new(&meta.uri)) {
                Ok(f) => {
                    held.insert(id, f);
                }
                Err(_) => return Lock::Failed,
            }
        }
        let f = &held[&id];
        if nonblocking {
            let r = if shared { f.try_lock_shared() } else { f.try_lock() };
            match r {
                Ok(()) => Lock::Taken,
                Err(fs::TryLockError::WouldBlock) => Lock::WouldBlock,
                Err(fs::TryLockError::Error(_)) => Lock::Failed,
            }
        } else {
            let r = if shared { f.lock_shared() } else { f.lock() };
            match r {
                Ok(()) => Lock::Taken,
                Err(_) => Lock::Failed,
            }
        }
    });
    match outcome {
        Lock::Taken => Ok(Value::Bool(true)),
        Lock::WouldBlock => {
            if args.len() > 2 {
                args[2] = Value::Int(1);
            }
            Ok(Value::Bool(false))
        }
        Lock::Failed => Ok(Value::Bool(false)),
    }
}

/// `fpassthru(resource $stream): int` — write what is left of the handle to
/// output. The failure answer is `-1`, php's C return, not `false`.
fn fpassthru(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A user wrapper's stream answers for itself.
    if let Some(v) = crate::stream_wrappers::handle_op(ctx, "fpassthru", args)? {
        return Ok(v);
    }
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, "fpassthru")?;
    if !meta.readable() {
        ctx.notice("fpassthru(): Read of 8192 bytes failed with errno=9 Bad file descriptor")?;
        return Ok(Value::Int(-1));
    }
    let data = ctx
        .call_function(b"stream_get_contents", &[stream])?
        .to_php_bytes();
    let n = data.len() as i64;
    ctx.echo(&data);
    Ok(Value::Int(n))
}

/// `fsync(resource $stream): bool`
fn fsync(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sync_handle(ctx, args, "fsync", true)
}

/// `fdatasync(resource $stream): bool` — the contents, without waiting for
/// the metadata.
fn fdatasync(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    sync_handle(ctx, args, "fdatasync", false)
}

/// Both sync functions: flush the handle's buffer to its file, then ask the
/// kernel to put it on the disk. A handle with no file cannot be synced and
/// php says so.
fn sync_handle(ctx: &mut Ctx, args: &mut [Value], func: &str, metadata: bool) -> NativeResult {
    // A user wrapper's stream answers for itself.
    if let Some(v) = crate::stream_wrappers::handle_op(ctx, func, args)? {
        return Ok(v);
    }
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, func)?;
    if !meta.plainfile {
        ctx.warn(&format!("{func}(): Can't fsync this stream!"))?;
        return Ok(Value::Bool(false));
    }
    ctx.call_function(b"fflush", &[stream])?;
    let synced = fs::OpenOptions::new()
        .read(true)
        .open(Path::new(&meta.uri))
        .and_then(|f| if metadata { f.sync_all() } else { f.sync_data() });
    Ok(Value::Bool(synced.is_ok()))
}

/// `fscanf(resource $stream, string $format, mixed &...$vars): array|int|false`
///
/// One line, then `sscanf` over it — so the cursor ends past the newline and
/// the end-of-file flag is whatever reading that line left behind.
fn fscanf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, "fscanf")?;
    if !meta.readable() {
        ctx.notice("fscanf(): Read of 8192 bytes failed with errno=9 Bad file descriptor")?;
        return Ok(Value::Bool(false));
    }
    let line = ctx.call_function(b"fgets", &[stream])?;
    if matches!(line, Value::Bool(false)) {
        return Ok(Value::Bool(false));
    }
    let mut scan: Vec<Value> = Vec::with_capacity(args.len());
    scan.push(line);
    scan.extend(args[1..].iter().cloned());
    let result = crate::string2::sscanf(ctx, &mut scan)?;
    // `sscanf` writes its captures into the slots it was handed; positions
    // line up because the subject replaced the stream in slot 0.
    for (i, v) in scan.into_iter().enumerate().skip(2) {
        args[i] = v;
    }
    Ok(result)
}

// ---- CSV -------------------------------------------------------------------

/// The `$separator` / `$enclosure` / `$escape` trio both CSV functions take
/// from position `base` on. Leaving `$escape` out is deprecated in 8.4+,
/// because its default is going away.
fn csv_args(
    ctx: &mut Ctx,
    args: &[Value],
    func: &str,
    base: usize,
) -> Result<(u8, u8, Option<u8>), Unwind> {
    let delim = args.get(base).map_or_else(|| b",".to_vec(), Value::to_php_bytes);
    if delim.len() != 1 {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #{} ($separator) must be a single character",
            base + 1
        )));
    }
    let enc = args
        .get(base + 1)
        .map_or_else(|| b"\"".to_vec(), Value::to_php_bytes);
    if enc.len() != 1 {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #{} ($enclosure) must be a single character",
            base + 2
        )));
    }
    let escape = match args.get(base + 2) {
        Some(e) => {
            let e = e.to_php_bytes();
            if e.len() > 1 {
                return Err(Unwind::value_error(format!(
                    "{func}(): Argument #{} ($escape) must be empty or a single character",
                    base + 3
                )));
            }
            e.first().copied()
        }
        None => {
            ctx.deprecated(&format!(
                "{func}(): the $escape parameter must be provided as its default value will change"
            ))?;
            Some(b'\\')
        }
    };
    Ok((delim[0], enc[0], escape))
}

/// `fgetcsv(resource $stream, ?int $length = null, string $separator = ",", string $enclosure = "\"", string $escape = "\\"): array|false`
///
/// A record is not a line: a newline inside an enclosure belongs to the
/// field, so lines are read until the enclosures balance or the stream runs
/// out. An empty line is one `null` field, and only a read that found nothing
/// at all is `false`.
fn fgetcsv(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, "fgetcsv")?;
    let (delim, enc, escape) = csv_args(ctx, args, "fgetcsv", 2)?;
    if !meta.readable() {
        ctx.notice("fgetcsv(): Read of 8192 bytes failed with errno=9 Bad file descriptor")?;
        return Ok(Value::Bool(false));
    }
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let line = ctx.call_function(b"fgets", &[stream.clone()])?;
        if matches!(line, Value::Bool(false)) {
            break;
        }
        buf.extend_from_slice(&line.to_php_bytes());
        if !csv_enclosure_open(&buf, delim, enc, escape) {
            break;
        }
    }
    if buf.is_empty() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Array(crate::string2::parse_csv_line(
        &buf, delim, enc, escape,
    )))
}

/// Whether `buf` ends inside an enclosure, i.e. whether the record needs
/// another line. The field walk mirrors `parse_csv_line`, down to skipping
/// leading whitespace only in front of an enclosure.
fn csv_enclosure_open(buf: &[u8], delim: u8, enc: u8, escape: Option<u8>) -> bool {
    let mut i = 0usize;
    loop {
        let mut skip = i;
        while skip < buf.len() && buf[skip] != delim && is_csv_space(buf[skip]) {
            skip += 1;
        }
        if skip < buf.len() && buf[skip] == enc {
            i = skip;
        }
        if i >= buf.len() {
            return false;
        }
        if buf[i] == enc {
            i += 1;
            loop {
                if i >= buf.len() {
                    return true;
                }
                if Some(buf[i]) == escape {
                    i += 2;
                } else if buf[i] == enc {
                    // A doubled enclosure is one literal enclosure and the
                    // field goes on.
                    if buf.get(i + 1) == Some(&enc) {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        }
        while i < buf.len() && buf[i] != delim {
            i += 1;
        }
        if i >= buf.len() {
            return false;
        }
        i += 1;
    }
}

/// The whitespace C's `isspace` reports, which is what php's CSV reader skips.
fn is_csv_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `fputcsv(resource $stream, array $fields, string $separator = ",", string $enclosure = "\"", string $escape = "\\", string $eol = "\n"): int|false`
fn fputcsv(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let meta = stream_meta(ctx, &stream, "fputcsv")?;
    // php checks the argument *types* before it looks at the separator, so a
    // non-array `$fields` throws before the `$escape` deprecation is raised.
    let fields = args[1].deref().into_owned();
    let Value::Array(fields) = fields else {
        return Err(Unwind::type_error(format!(
            "fputcsv(): Argument #2 ($fields) must be of type array, {} given",
            rphp_runtime::value_name(&args[1].deref())
        )));
    };
    let (delim, enc, escape) = csv_args(ctx, args, "fputcsv", 2)?;
    let eol = args.get(5).map_or_else(|| b"\n".to_vec(), Value::to_php_bytes);
    let line = build_csv(&fields, delim, enc, escape, &eol);
    if !meta.writable() {
        ctx.notice(&format!(
            "fputcsv(): Write of {} bytes failed with errno=9 Bad file descriptor",
            line.len()
        ))?;
        return Ok(Value::Bool(false));
    }
    ctx.call_function(b"fwrite", &[stream, Value::Str(Str::from_vec(line))])
}

/// php's `php_fputcsv`: a field is enclosed when it holds the separator, the
/// enclosure, the escape, a newline, a tab or a space — and inside an
/// enclosed field the enclosure is doubled *unless* the escape character came
/// first, which is why `a\"b` comes back out as `"a\"b"`.
fn build_csv(fields: &Array, delim: u8, enc: u8, escape: Option<u8>, eol: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let last = fields.len().saturating_sub(1);
    for (i, (_, v)) in fields.iter().enumerate() {
        let f = v.deref().to_php_bytes();
        let enclose = !f.is_empty()
            && f.iter().any(|&c| {
                c == delim || c == enc || Some(c) == escape || matches!(c, b'\n' | b'\r' | b'\t' | b' ')
            });
        if enclose {
            out.push(enc);
            let mut escaped = false;
            for &c in &f {
                if Some(c) == escape {
                    escaped = true;
                } else if !escaped && c == enc {
                    out.push(enc);
                } else {
                    escaped = false;
                }
                out.push(c);
            }
            out.push(enc);
        } else {
            out.extend_from_slice(&f);
        }
        if i != last {
            out.push(delim);
        }
    }
    out.extend_from_slice(eol);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matcher_follows_bsd_rules() {
        assert!(fnmatch(b"*.txt", b"a.txt", false));
        assert!(!fnmatch(b"*", b".hidden", false));
        assert!(fnmatch(b".*", b"..", false));
        assert!(fnmatch(b"?.txt", b"a.txt", false));
        assert!(!fnmatch(b"?.txt", b"ab.txt", false));
        assert!(fnmatch(b"[a-c].dat", b"c.dat", false));
        assert!(fnmatch(b"[!a].txt", b"b.txt", false));
        assert!(!fnmatch(b"[!a].txt", b"a.txt", false));
        // `^` is not a negation in BSD's matcher, only a member of the class.
        assert!(fnmatch(b"[^a].txt", b"a.txt", false));
        assert!(fnmatch(b"a\\*b", b"a*b", false));
        assert!(!fnmatch(b"a\\*b", b"axb", false));
        // An unclosed bracket is an ordinary character.
        assert!(fnmatch(b"[abc", b"[abc", false));
    }

    #[test]
    fn braces_expand_left_to_right() {
        assert_eq!(brace_expand("*.{txt,dat}", false), ["*.txt", "*.dat"]);
        assert_eq!(brace_expand("{a,{b,c}}.*", false), ["a.*", "b.*", "c.*"]);
        assert_eq!(brace_expand("a{,.txt}", false), ["a", "a.txt"]);
        // An unclosed brace is matched literally.
        assert_eq!(brace_expand("{a.txt", false), ["{a.txt"]);
    }

    #[test]
    fn csv_records_span_lines_until_the_enclosures_balance() {
        assert!(csv_enclosure_open(b"\"x,1\",\"y\n", b',', b'"', Some(b'\\')));
        assert!(!csv_enclosure_open(b"\"x,1\",\"y\ny2\",z\n", b',', b'"', Some(b'\\')));
        assert!(!csv_enclosure_open(b"a,b\n", b',', b'"', Some(b'\\')));
        assert!(!csv_enclosure_open(b"\"a\"\"b\"\n", b',', b'"', Some(b'\\')));
        assert!(csv_enclosure_open(b"\"abc\n", b',', b'"', Some(b'\\')));
    }

    #[test]
    fn csv_output_quotes_what_php_quotes() {
        let row = |items: &[&str]| {
            let mut a = Array::new();
            for i in items {
                a.push(Value::string(i.as_bytes()));
            }
            a
        };
        let out = |items: &[&str]| build_csv(&row(items), b',', b'"', Some(b'\\'), b"\n");
        assert_eq!(out(&["a", "b", "c"]), b"a,b,c\n".to_vec());
        assert_eq!(out(&["a b"]), b"\"a b\"\n".to_vec());
        assert_eq!(out(&["plain"]), b"plain\n".to_vec());
        assert_eq!(out(&[""]), b"\n".to_vec());
        assert_eq!(out(&["a\"b"]), b"\"a\"\"b\"\n".to_vec());
        // The escape suppresses the doubling of the enclosure after it.
        assert_eq!(out(&["a\\\"b"]), b"\"a\\\"b\"\n".to_vec());
        assert_eq!(
            build_csv(&Array::new(), b',', b'"', Some(b'\\'), b"\n"),
            b"\n".to_vec()
        );
    }

    #[test]
    fn a_stat_array_has_phps_twenty_six_entries() {
        let a = stat_array(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(a.len(), 26);
        assert_eq!(a.get(&ArrayKey::Int(0)).map(Value::to_int), Some(1));
        assert_eq!(a.get(&ArrayKey::Int(12)).map(Value::to_int), Some(13));
        assert_eq!(
            a.get(&ArrayKey::str(b"blocks"))
                .map(Value::to_int),
            Some(13)
        );
        assert_eq!(
            a.get(&ArrayKey::str(b"dev")).map(Value::to_int),
            Some(1)
        );
    }

    #[test]
    fn a_temporary_name_is_a_prefix_and_nineteen_characters() {
        assert_eq!(random_suffix().len(), 19);
        assert!(random_suffix().bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_eq!(basename_bytes(b"a/b"), b"b".to_vec());
        assert_eq!(basename_bytes(b"pre"), b"pre".to_vec());
        assert_eq!(basename_bytes(b"a/b/"), b"b".to_vec());
    }
}

/// `ftruncate(resource $stream, int $size): bool` — cut the file to `size`,
/// or extend it with NUL bytes. The cursor does not move, which is why php
/// leaves `ftell()` where it was even when the file is now shorter.
fn ftruncate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let size = args[1].to_int();
    if size < 0 {
        return Err(Unwind::value_error(
            "ftruncate(): Argument #2 ($size) must be greater than or equal to 0",
        ));
    }
    // A user wrapper's stream answers for itself.
    if let Some(v) = crate::stream_wrappers::handle_op(ctx, "ftruncate", args)? {
        return Ok(v);
    }
    let stream = args[0].clone();
    crate::file::with_stream_mut(ctx, &stream, "ftruncate", |s| {
        s.truncate_to(size as usize);
        Value::Bool(true)
    })
}

/// `tmpfile(): resource|false` — a file in the temporary directory, opened
/// read/write and removed when it is closed. php reports it as an ordinary
/// `plainfile`/`STDIO` stream, not a `php://` one.
fn tmpfile(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let dir = std::env::temp_dir();
    for _ in 0..64 {
        let name = format!("php{:x}", fastrand_u32());
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        if fs::write(&path, b"").is_err() {
            break;
        }
        let res = crate::file::open_resource(ctx, &path, "r+b");
        // php keeps the name while the handle is open (its `uri` can be
        // read by path) and removes the file when the handle closes.
        crate::file::mark_temporary(ctx, &res);
        return Ok(res);
    }
    Ok(Value::Bool(false))
}

/// A non-cryptographic name source for the temporary file, seeded from the
/// clock and the process (php uses `mkstemp`, which rphp cannot reach
/// without an `unsafe` call).
fn fastrand_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    nanos ^ std::process::id().rotate_left(13)
}
