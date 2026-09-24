//! Directory functions (ext/standard `dir.c`): listing, creating and removing
//! directories, and the process working directory.

use std::fs;

use rphp_runtime::{nf, nm, ClassFlags, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, Str, Value};

use crate::filestat::{arg_path, clear_stat_cache};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("scandir", 1, Some(3), scandir),
    nf!("mkdir", 1, Some(4), mkdir),
    nf!("rmdir", 1, Some(2), rmdir),
    nf!("getcwd", 0, Some(0), getcwd),
    nf!("chdir", 1, Some(1), chdir),
    nf!("opendir", 1, Some(2), opendir),
    nf!("readdir", 0, Some(1), readdir),
    nf!("rewinddir", 0, Some(1), rewinddir),
    nf!("closedir", 0, Some(1), closedir),
    nf!("dir", 1, Some(2), dir),
];

// ---- directory streams -----------------------------------------------------

/// An open directory: its entries as the OS lists them — `.` and `..`
/// first, the rest in `readdir(3)` order, which is the order `read_dir`
/// walks — and the cursor. php reports it as a `stream` resource.
struct DirStream {
    entries: Vec<Vec<u8>>,
    pos: usize,
}

/// The resource type php gives a directory handle.
const STREAM: &str = "stream";

/// The last directory `opendir()` opened, which the handle-less calls use.
const LAST_DIR_SLOT: &str = "dir.last-opened";

#[derive(Default)]
struct LastDir(Option<Value>);

/// Open `path` as a directory stream, or php's warning under `func`.
fn open_dir(ctx: &mut Ctx, func: &str, raw: &Value) -> Result<Option<Value>, Unwind> {
    let p = arg_path(ctx, raw);
    let rd = match fs::read_dir(&p) {
        Ok(rd) => rd,
        Err(e) => {
            let shown = String::from_utf8_lossy(&raw.to_php_bytes()).into_owned();
            let why = match e.kind() {
                std::io::ErrorKind::PermissionDenied => "Permission denied",
                _ if p.is_file() => "Not a directory",
                _ => "No such file or directory",
            };
            ctx.warn(&format!("{func}({shown}): Failed to open directory: {why}"))?;
            return Ok(None);
        }
    };
    let mut entries: Vec<Vec<u8>> = vec![b".".to_vec(), b"..".to_vec()];
    for e in rd.flatten() {
        use std::os::unix::ffi::OsStrExt;
        entries.push(e.file_name().as_bytes().to_vec());
    }
    let h = ctx.resources.add(STREAM, Box::new(DirStream { entries, pos: 0 }));
    ctx.ext.slot::<LastDir>(LAST_DIR_SLOT).0 = Some(h.clone());
    Ok(Some(h))
}

/// `opendir(string $directory, ?resource $context = null): resource|false`
fn opendir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(h) = crate::stream_wrappers::open_dir(ctx, "opendir", &args[0], args.get(1))? {
        if let Some(h) = &h {
            ctx.ext.slot::<LastDir>(LAST_DIR_SLOT).0 = Some(h.clone());
        }
        return Ok(h.unwrap_or(Value::Bool(false)));
    }
    Ok(open_dir(ctx, "opendir", &args[0])?.unwrap_or(Value::Bool(false)))
}

/// The handle a `readdir`/`rewinddir`/`closedir` call is about: the
/// argument, or — deprecated since 8.5 — the last opened one.
fn dir_handle(ctx: &mut Ctx, func: &str, arg: Option<&Value>) -> Result<rphp_value::Resource, Unwind> {
    let v = match arg.map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => {
            let Some(last) = ctx.ext.slot::<LastDir>(LAST_DIR_SLOT).0.clone() else {
                return Err(Unwind::type_error("No resource supplied"));
            };
            ctx.deprecated(&format!(
                "{func}(): Passing null is deprecated, instead the last opened directory stream should be provided"
            ))?;
            match &last {
                Value::Resource(r) if r.kind() == STREAM => {}
                _ => return Err(Unwind::type_error("No resource supplied")),
            }
            last
        }
        Some(v) => v,
    };
    match &v {
        Value::Resource(r) if r.kind() != STREAM => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($dir_handle) must be an open stream resource"
        ))),
        Value::Resource(r) => {
            let is_dir = r.payload_mut().as_mut().is_some_and(|p| p.downcast_mut::<DirStream>().is_some())
                || crate::stream_wrappers::is_user_dir(r);
            if !is_dir {
                return Err(Unwind::type_error(format!(
                    "{func}(): Argument #1 ($dir_handle) must be a valid Directory resource"
                )));
            }
            Ok(r.clone())
        }
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($dir_handle) must be of type resource or null, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// Run `f` over the directory stream behind a checked handle.
fn with_dir<R>(r: &rphp_value::Resource, f: impl FnOnce(&mut DirStream) -> R) -> Option<R> {
    let mut payload = r.payload_mut();
    payload.as_mut().and_then(|p| p.downcast_mut::<DirStream>()).map(f)
}

fn read_entry(r: &rphp_value::Resource) -> Value {
    with_dir(r, |d| {
        let e = d.entries.get(d.pos).cloned();
        if e.is_some() {
            d.pos += 1;
        }
        e
    })
    .flatten()
    .map_or(Value::Bool(false), |e| Value::Str(Str::from_vec(e)))
}

/// `readdir(?resource $dir_handle = null): string|false`
fn readdir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = dir_handle(ctx, "readdir", args.first())?;
    // A user wrapper's directory answers for itself.
    if let Some(v) = crate::stream_wrappers::dir_op(ctx, "readdir", &r)? {
        return Ok(v);
    }
    Ok(read_entry(&r))
}

/// `rewinddir(?resource $dir_handle = null): void`
fn rewinddir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = dir_handle(ctx, "rewinddir", args.first())?;
    // A user wrapper's directory answers for itself.
    if let Some(v) = crate::stream_wrappers::dir_op(ctx, "rewinddir", &r)? {
        return Ok(v);
    }
    with_dir(&r, |d| d.pos = 0);
    Ok(Value::Null)
}

/// `closedir(?resource $dir_handle = null): void`
fn closedir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = dir_handle(ctx, "closedir", args.first())?;
    // A user wrapper's directory answers for itself.
    if let Some(v) = crate::stream_wrappers::dir_op(ctx, "closedir", &r)? {
        return Ok(v);
    }
    ctx.resources.close(r.id());
    Ok(Value::Null)
}

// ---- the Directory class ---------------------------------------------------

/// `dir(string $directory, ?resource $context = null): Directory|false`
fn dir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    let user = crate::stream_wrappers::open_dir(ctx, "dir", &args[0], args.get(1))?;
    let opened = match user {
        Some(h) => h,
        None => open_dir(ctx, "dir", &args[0])?,
    };
    let Some(h) = opened else {
        return Ok(Value::Bool(false));
    };
    let cid = ctx.class_by_name(b"Directory").ok_or_else(|| Unwind::error("Class \"Directory\" not found"))?;
    let o = ctx.instantiate(cid);
    o.set(b"path", Value::Str(Str::from_vec(args[0].to_php_bytes().to_vec())));
    o.set(b"handle", h);
    Ok(Value::Object(o))
}

pub(crate) fn register_classes(r: &mut Registry) {
    r.class("Directory")
        .flags(ClassFlags::FINAL)
        .readonly_prop("path", "string")
        .readonly_prop("handle", "mixed")
        .method("__construct", nm!(0, Some(0), directory_construct))
        .method("close", nm!(0, Some(0), directory_close))
        .method("rewind", nm!(0, Some(0), directory_rewind))
        .method("read", nm!(0, Some(0), directory_read))
        .finish();
}

fn directory_construct(_ctx: &mut Ctx, _this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot directly construct Directory, use dir() instead"))
}

/// A `Directory` method's handle, open, or php's TypeError.
fn directory_handle(this: Option<&Object>, method: &str) -> Result<rphp_value::Resource, Unwind> {
    let h = this.and_then(|o| o.get(b"handle")).map(|v| v.deref().into_owned());
    match h {
        Some(Value::Resource(r)) if r.kind() == STREAM => Ok(r),
        Some(Value::Resource(_)) => Err(Unwind::type_error(format!(
            "Directory::{method}(): cannot use Directory resource after it has been closed"
        ))),
        _ => Err(Unwind::error(format!("Unable to find my handle property"))),
    }
}

fn directory_read(ctx: &mut Ctx, this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    let r = directory_handle(this, "read")?;
    if let Some(v) = crate::stream_wrappers::dir_op(ctx, "readdir", &r)? {
        return Ok(v);
    }
    Ok(read_entry(&r))
}

fn directory_rewind(ctx: &mut Ctx, this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    let r = directory_handle(this, "rewind")?;
    if let Some(v) = crate::stream_wrappers::dir_op(ctx, "rewinddir", &r)? {
        return Ok(v);
    }
    with_dir(&r, |d| d.pos = 0);
    Ok(Value::Null)
}

fn directory_close(ctx: &mut Ctx, this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    let r = directory_handle(this, "close")?;
    if let Some(v) = crate::stream_wrappers::dir_op(ctx, "closedir", &r)? {
        return Ok(v);
    }
    ctx.resources.close(r.id());
    Ok(Value::Null)
}

/// php's `SCANDIR_SORT_ASCENDING`, the default.
const SORT_ASCENDING: i64 = 0;
/// php's `SCANDIR_SORT_DESCENDING`.
const SORT_DESCENDING: i64 = 1;
/// php's `SCANDIR_SORT_NONE`.
const SORT_NONE: i64 = 2;

/// `scandir(string $directory, int $sorting_order = SCANDIR_SORT_ASCENDING, ...): array|false`
///
/// Includes `.` and `..`, as php does.
fn scandir(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::scandir(ctx, args)? {
        return Ok(v);
    }
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
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "mkdir", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    let recursive = args.get(2).is_some_and(Value::to_bool);
    let r = if recursive {
        fs::create_dir_all(&p)
    } else {
        fs::create_dir(&p)
    };
    clear_stat_cache(ctx);
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
    // A registered user wrapper takes the url.
    if let Some(v) = crate::stream_wrappers::path_op(ctx, "rmdir", args)? {
        return Ok(v);
    }
    let p = arg_path(ctx, &args[0]);
    let r = fs::remove_dir(&p);
    clear_stat_cache(ctx);
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
pub(crate) fn register_constants(r: &mut rphp_runtime::Registry) {
    r.constant("SCANDIR_SORT_ASCENDING", Value::Int(SORT_ASCENDING));
    r.constant("SCANDIR_SORT_DESCENDING", Value::Int(SORT_DESCENDING));
    r.constant("SCANDIR_SORT_NONE", Value::Int(SORT_NONE));
}
