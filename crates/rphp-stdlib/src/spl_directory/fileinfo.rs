//! `SplFileInfo` (php-src `ext/spl/spl_directory.c`): a path plus the
//! syscalls that ask the filesystem about it.
//!
//! **The two failure modes are not uniform, and php means it.** A predicate
//! (`isFile`, `isDir`, `isLink`, `isReadable`, `isWritable`, `isExecutable`)
//! answers `false` for a path that is not there. A *metadata* accessor
//! (`getPerms`, `getInode`, `getSize`, `getOwner`, `getGroup`, `getATime`,
//! `getMTime`, `getCTime`) throws `RuntimeException:
//! SplFileInfo::getSize(): stat failed for /x` — and names `SplFileInfo`
//! even when the receiver is a subclass, because php builds that text from
//! a literal. `getType()` throws too, but with php's own capitalisation,
//! `Lstat failed for /x`, because it is the one accessor that does not
//! follow a symlink. `getLinkTarget()` throws `Unable to read link /x,
//! error: Invalid argument` — the C `strerror` text — and `getRealPath()`
//! quietly answers `false`.
//!
//! **Before the constructor** (`newInstanceWithoutConstructor`) php has no
//! `file_name` at all: `getPathname()`, `getPath()` and `__toString()`
//! answer `''`, `getRealPath()` answers `false`, and everything that needs
//! the name throws `Error: Object not initialized`.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Object, Str, Value};

use super::common::{
    as_path, basename, class_arg, clone_payload, debug_array, entry_name, extension, file_name,
    file_name_part, lstat, object_path, pathname, required_class_arg, set_file_name, special_type,
    stat, stat_fields, str_arg, sync, this, with_fs, Fs, Kind,
};
use crate::filestat::io_text;

/// The pathname, or php's `Error` for an object whose constructor never
/// ran. A directory iterator always has one (it is built from the cursor),
/// which is why only the `SplFileInfo`/`SplFileObject` shapes can fail.
pub(super) fn need_name(st: &Fs) -> Result<Vec<u8>, Unwind> {
    file_name(st).ok_or_else(|| Unwind::error("Object not initialized"))
}

/// php's text for a metadata accessor on a path it cannot stat. The class
/// is the literal `SplFileInfo`, not the receiver's class.
fn stat_failed(method: &str, name: &[u8]) -> Unwind {
    Unwind::exception(
        "RuntimeException",
        format!(
            "SplFileInfo::{method}(): stat failed for {}",
            String::from_utf8_lossy(name)
        ),
    )
}

/// The shared shape of the eight metadata accessors.
fn stat_field(
    ctx: &mut Ctx,
    o: Option<&Object>,
    method: &str,
    f: impl Fn(&std::fs::Metadata) -> Value,
) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    match stat(ctx, &name) {
        Some(m) => Ok(f(&m)),
        None => Err(stat_failed(method, &name)),
    }
}

// ---- construction ----------------------------------------------------------

/// `__construct(string $filename)`
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = str_arg(ctx, "SplFileInfo::__construct", 1, "filename", &args[0])?;
    let st = with_fs(o, |s| {
        s.kind = Kind::Info;
        set_file_name(s, &name);
        s.clone()
    });
    sync(o, &st);
    Ok(Value::Null)
}

// ---- the path accessors ----------------------------------------------------

/// `getPath(): string` — php's `_path`, which is *not* `dirname()`: `/tmp`
/// has the path `''`.
fn get_path(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| object_path(s)))))
}

/// `getFilename(): string`
fn get_filename(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = with_fs(o, |s| s.clone());
    need_name(&st)?;
    Ok(Value::Str(Str::from_vec(
        file_name_part(&st).unwrap_or_default(),
    )))
}

/// `getExtension(): string`
fn get_extension(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    Ok(Value::Str(Str::from_vec(extension(&name))))
}

/// `getBasename(string $suffix = ''): string`
fn get_basename(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    let suffix = match args.first() {
        Some(v) => str_arg(ctx, "SplFileInfo::getBasename", 1, "suffix", v)?,
        None => Vec::new(),
    };
    Ok(Value::Str(Str::from_vec(basename(&name, &suffix))))
}

/// `getPathname(): string` — the empty string, not an error, before the
/// constructor has run.
fn get_pathname(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(
        with_fs(o, |s| pathname(s)).unwrap_or_default(),
    )))
}

/// `__toString(): string` — the pathname. `DirectoryIterator` overrides
/// this with the bare entry name.
fn to_string(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    get_pathname(ctx, o, args)
}

// ---- the metadata accessors ------------------------------------------------

/// `getPerms(): int|false` — the whole mode word, file-type bits included,
/// so a regular file reports `0100644`.
fn get_perms(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getPerms", |m| {
        Value::Int(i64::from(stat_fields(m).mode))
    })
}

/// `getInode(): int|false`
fn get_inode(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getInode", |m| Value::Int(stat_fields(m).ino))
}

/// `getSize(): int|false`
fn get_size(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getSize", |m| Value::Int(m.len() as i64))
}

/// `getOwner(): int|false`
fn get_owner(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getOwner", |m| Value::Int(stat_fields(m).uid))
}

/// `getGroup(): int|false`
fn get_group(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getGroup", |m| Value::Int(stat_fields(m).gid))
}

/// `getATime(): int|false`
fn get_atime(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getATime", |m| Value::Int(stat_fields(m).atime))
}

/// `getMTime(): int|false`
fn get_mtime(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getMTime", |m| Value::Int(stat_fields(m).mtime))
}

/// `getCTime(): int|false` — `st_ctime`, the inode-change time. Not the
/// birth time, which is what macOS's `st_birthtime` would give.
fn get_ctime(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    stat_field(ctx, o, "getCTime", |m| Value::Int(stat_fields(m).ctime))
}

/// `getType(): string|false` — the one accessor that does **not** follow a
/// symlink, and the one whose failure text php capitalises differently.
fn get_type(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    let Some(m) = lstat(ctx, &name) else {
        return Err(Unwind::exception(
            "RuntimeException",
            format!(
                "SplFileInfo::getType(): Lstat failed for {}",
                String::from_utf8_lossy(&name)
            ),
        ));
    };
    let t = m.file_type();
    let s: &[u8] = if t.is_symlink() {
        &b"link"[..]
    } else if t.is_dir() {
        &b"dir"[..]
    } else if t.is_file() {
        &b"file"[..]
    } else {
        special_type(&t).unwrap_or(&b"unknown"[..])
    };
    Ok(Value::string(s))
}

// ---- the predicates --------------------------------------------------------

/// `isWritable(): bool` — the same test `filestat.rs`'s `is_writable()`
/// makes, so the two agree inside rphp as they do in php.
fn is_writable(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    Ok(Value::Bool(
        stat(ctx, &name).is_some_and(|m| !m.permissions().readonly()),
    ))
}

/// `isReadable(): bool`
fn is_readable(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    let path = as_path(ctx, &name);
    Ok(Value::Bool(match stat(ctx, &name) {
        Some(m) if m.is_dir() => std::fs::read_dir(&path).is_ok(),
        Some(_) => std::fs::File::open(&path).is_ok(),
        None => false,
    }))
}

/// `isExecutable(): bool`
fn is_executable(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    Ok(Value::Bool(
        stat(ctx, &name).is_some_and(|m| stat_fields(&m).mode & 0o111 != 0),
    ))
}

/// `isFile(): bool` — follows a symlink, so a link to a file is a file.
fn is_file(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    Ok(Value::Bool(stat(ctx, &name).is_some_and(|m| m.is_file())))
}

/// `isDir(): bool`
fn is_dir(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    Ok(Value::Bool(stat(ctx, &name).is_some_and(|m| m.is_dir())))
}

/// `isLink(): bool`
fn is_link(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    Ok(Value::Bool(
        lstat(ctx, &name).is_some_and(|m| m.file_type().is_symlink()),
    ))
}

/// `getLinkTarget(): string|false` — the raw target, not a resolved path,
/// so a dangling link still answers.
fn get_link_target(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = with_fs(o, |s| need_name(s))?;
    match std::fs::read_link(as_path(ctx, &name)) {
        Ok(t) => Ok(Value::string(t.to_string_lossy().as_bytes())),
        Err(e) => Err(Unwind::exception(
            "RuntimeException",
            format!(
                "Unable to read link {}, error: {}",
                String::from_utf8_lossy(&name),
                io_text(&e)
            ),
        )),
    }
}

/// `getRealPath(): string|false` — `false`, never an exception, for a path
/// that does not resolve.
fn get_real_path(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Some(name) = with_fs(o, |s| pathname(s)) else {
        return Ok(Value::Bool(false));
    };
    Ok(match std::fs::canonicalize(as_path(ctx, &name)) {
        Ok(c) => Value::string(c.to_string_lossy().as_bytes()),
        Err(_) => Value::Bool(false),
    })
}

// ---- spawning other objects ------------------------------------------------

/// php's `Could not open file`, which `spl_filesystem_object_create_type`
/// raises when a directory iterator has run off the end and is asked to
/// build an `SplFileInfo` out of its (now empty) entry.
pub(super) fn check_entry(st: &Fs) -> Result<(), Unwind> {
    if st.kind == Kind::Dir && entry_name(st).is_empty() {
        return Err(Unwind::exception("RuntimeException", "Could not open file"));
    }
    Ok(())
}

/// Build an `SplFileInfo` (or a subclass) over `name`, carrying the
/// spawning object's `setInfoClass`/`setFileClass` choices with it, as php
/// does. A class whose `__construct` is php's own is filled in directly;
/// only a subclass that *declares* a constructor gets it called — which is
/// exactly php's `ce->constructor->common.scope != spl_ce_SplFileInfo`
/// test, and the reason a failure inside `openFile()` is reported under
/// `SplFileInfo::openFile` rather than under the child's constructor.
pub(super) fn make_info(
    ctx: &mut Ctx,
    src: &Fs,
    class: Option<Vec<u8>>,
    name: &[u8],
) -> NativeResult {
    let class = class
        .or_else(|| src.info_class.clone())
        .unwrap_or_else(|| b"SplFileInfo".to_vec());
    let cid = ctx.lookup_class(&class)?.ok_or_else(|| {
        Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(&class)
        ))
    })?;
    let obj = ctx.new_object(cid)?;
    let base = ctx.class_by_name(b"SplFileInfo").expect("registered");
    let own_ctor = ctx
        .resolve_method(cid, b"__construct")
        .is_some_and(|m| m.decl != base);
    if own_ctor {
        ctx.call_method(&obj, b"__construct", &[Value::string(name)])?;
    } else {
        let st = with_fs(&obj, |s| {
            s.kind = Kind::Info;
            set_file_name(s, name);
            s.info_class = src.info_class.clone();
            s.file_class = src.file_class.clone();
            s.clone()
        });
        sync(&obj, &st);
    }
    Ok(Value::Object(obj))
}

/// `getFileInfo(?string $class = null): SplFileInfo`
fn get_file_info(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = with_fs(o, |s| s.clone());
    let base = st
        .info_class
        .clone()
        .unwrap_or_else(|| b"SplFileInfo".to_vec());
    let class = class_arg(ctx, "SplFileInfo::getFileInfo", &base, args.first())?;
    check_entry(&st)?;
    let name = need_name(&st)?;
    make_info(ctx, &st, class, &name)
}

/// `getPathInfo(?string $class = null): ?SplFileInfo` — `dirname()` of the
/// pathname, which is *not* `getPath()`: `new SplFileInfo('a')` has the
/// path `''` and the path info `.`. `null` when there is no pathname at
/// all.
fn get_path_info(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = with_fs(o, |s| s.clone());
    let base = st
        .info_class
        .clone()
        .unwrap_or_else(|| b"SplFileInfo".to_vec());
    let class = class_arg(ctx, "SplFileInfo::getPathInfo", &base, args.first())?;
    let Some(name) = pathname(&st) else {
        return Ok(Value::Null);
    };
    if name.is_empty() {
        return Ok(Value::Null);
    }
    let id = ctx
        .native_by_name(b"dirname")
        .expect("dirname is registered");
    let mut argv = [Value::string(&name)];
    let parent = ctx.call_native(id, &mut argv)?.to_php_bytes();
    make_info(ctx, &st, class, &parent)
}

/// `openFile(string $mode = 'r', bool $useIncludePath = false,
/// ?resource $context = null): SplFileObject`
fn open_file(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let mode = match args.first() {
        Some(v) => str_arg(ctx, "SplFileInfo::openFile", 1, "mode", v)?,
        None => b"r".to_vec(),
    };
    let use_include_path = args.get(1).is_some_and(|v| v.deref().to_bool());
    let st = with_fs(o, |s| s.clone());
    check_entry(&st)?;
    let name = need_name(&st)?;
    super::fileobject::spawn(
        ctx,
        &st,
        "SplFileInfo::openFile",
        &name,
        &mode,
        use_include_path,
    )
}

/// `setFileClass(string $class = SplFileObject::class): void`
fn set_file_class(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let class = match args.first() {
        Some(v) => required_class_arg(ctx, "SplFileInfo::setFileClass", "SplFileObject", v)?,
        None => b"SplFileObject".to_vec(),
    };
    with_fs(o, |s| s.file_class = Some(class));
    Ok(Value::Null)
}

/// `setInfoClass(string $class = SplFileInfo::class): void`
fn set_info_class(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let class = match args.first() {
        Some(v) => required_class_arg(ctx, "SplFileInfo::setInfoClass", "SplFileInfo", v)?,
        None => b"SplFileInfo".to_vec(),
    };
    with_fs(o, |s| s.info_class = Some(class));
    Ok(Value::Null)
}

/// `_bad_state_ex(): void` — php's internal "you forgot
/// `parent::__construct()`" helper, still public and still deprecated.
///
/// php marks the method `ZEND_ACC_DEPRECATED`, so the engine raises the
/// notice before the body runs; there is no per-method deprecation flag
/// here, so the body raises it itself, which lands in the same place.
fn bad_state_ex(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    ctx.deprecated("Method SplFileInfo::_bad_state_ex() is deprecated since 8.2")?;
    Err(Unwind::error(
        "The parent constructor was not called: the object is in an invalid state",
    ))
}

/// `__debugInfo(): array` — php's synthesized dump, mangled keys and all.
fn debug_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(with_fs(o, |s| debug_array(s))))
}

// ---- registration ----------------------------------------------------------

/// Register `SplFileInfo`. `pathName` and `fileName` are real private slots
/// because php's debug view shows them; `fileName` starts out
/// [`Value::Uninit`] so an unconstructed object dumps the single member php
/// dumps.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("SplFileInfo")
        .implements(&["Stringable"])
        .prop("pathName", Visibility::Private, Value::string(b""))
        .prop("fileName", Visibility::Private, Value::Uninit)
        .payload_clone(clone_payload)
        .method("__construct", nm!(1, Some(1), construct))
        .method("getPath", nm!(0, Some(0), get_path))
        .method("getFilename", nm!(0, Some(0), get_filename))
        .method("getExtension", nm!(0, Some(0), get_extension))
        .method("getBasename", nm!(0, Some(1), get_basename))
        .method("getPathname", nm!(0, Some(0), get_pathname))
        .method("getPerms", nm!(0, Some(0), get_perms))
        .method("getInode", nm!(0, Some(0), get_inode))
        .method("getSize", nm!(0, Some(0), get_size))
        .method("getOwner", nm!(0, Some(0), get_owner))
        .method("getGroup", nm!(0, Some(0), get_group))
        .method("getATime", nm!(0, Some(0), get_atime))
        .method("getMTime", nm!(0, Some(0), get_mtime))
        .method("getCTime", nm!(0, Some(0), get_ctime))
        .method("getType", nm!(0, Some(0), get_type))
        .method("isWritable", nm!(0, Some(0), is_writable))
        .method("isReadable", nm!(0, Some(0), is_readable))
        .method("isExecutable", nm!(0, Some(0), is_executable))
        .method("isFile", nm!(0, Some(0), is_file))
        .method("isDir", nm!(0, Some(0), is_dir))
        .method("isLink", nm!(0, Some(0), is_link))
        .method("getLinkTarget", nm!(0, Some(0), get_link_target))
        .method("getRealPath", nm!(0, Some(0), get_real_path))
        .method("getFileInfo", nm!(0, Some(1), get_file_info))
        .method("getPathInfo", nm!(0, Some(1), get_path_info))
        .method("openFile", nm!(0, Some(3), open_file))
        .method("setFileClass", nm!(0, Some(1), set_file_class))
        .method("setInfoClass", nm!(0, Some(1), set_info_class))
        .method("_bad_state_ex", nm!(0, Some(0), bad_state_ex))
        .method("__toString", nm!(0, Some(0), to_string))
        .method("__debugInfo", nm!(0, Some(0), debug_info))
        .finish();
}
