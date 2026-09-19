//! The four directory iterators (php-src `ext/spl/spl_directory.c`):
//! `DirectoryIterator`, `FilesystemIterator`, `RecursiveDirectoryIterator`
//! and `GlobIterator`.
//!
//! # `current()` returns `$this`
//!
//! `DirectoryIterator::current()` hands back the iterator itself, so every
//! element of a `foreach` is the *same* object with a moved cursor:
//!
//! ```text
//! foreach (new DirectoryIterator('/tmp') as $e) { … }   // $e === the iterator
//! ```
//!
//! That is why `$e->getFilename()` works and why collecting the elements
//! into an array gives one repeated handle. `FilesystemIterator` changes it
//! with its `CURRENT_AS_*` flags — a fresh `SplFileInfo` by default, the
//! pathname string, or still `$this`.
//!
//! # Two cursors, not one
//!
//! php keeps a `DIR*` **and** an integer `index`. The index is what
//! `DirectoryIterator::key()` answers and what `seek()` counts against, and
//! `next()` bumps it exactly once however many `.`/`..` entries `SKIP_DOTS`
//! then swallows — and it keeps climbing after the directory is exhausted.
//! [`Dir::pos`](super::common::Dir::pos) is the real cursor,
//! [`Dir::index`](super::common::Dir::index) is php's counter.
//!
//! The listing is snapshotted at construction instead of held as an open
//! handle (the engine has no directory resource), and `rewind()` re-reads
//! the directory, which is what `rewinddir(3)` gives php.
//!
//! # `GlobIterator`'s path moves
//!
//! For a glob the "directory" is whatever directory the *current* match
//! lives in: php asks the glob stream for it on every access, so
//! `getPath()` answers `/tmp` in the middle of a walk and `''` once the
//! iterator is exhausted — and the key of an exhausted `GlobIterator` is
//! `''` where a `FilesystemIterator`'s is `/tmp/`. Both fall out of php's
//! "empty path means the entry stands alone" concatenation rule.
//!
//! `GlobIterator` is also **uncloneable**, as php marks it; the other three
//! clone their cursor.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Object, Str, Value};

use super::common::{
    as_path, basename, clone_payload, debug_array, dir_open_path, entry_name, extension, file_name,
    int_arg, is_dot, list_dir, lstat, stat, str_arg, sync, this, with_fs, Dir, Kind,
};
use super::fileinfo::{check_entry, make_info};
use crate::filestat::io_text;

// ---- class constants -------------------------------------------------------

/// `FilesystemIterator::CURRENT_MODE_MASK`
const CURRENT_MODE_MASK: i64 = 0x000F0;
/// `FilesystemIterator::CURRENT_AS_PATHNAME`
const CURRENT_AS_PATHNAME: i64 = 0x00020;
/// `FilesystemIterator::CURRENT_AS_FILEINFO`
const CURRENT_AS_FILEINFO: i64 = 0x00000;
/// `FilesystemIterator::CURRENT_AS_SELF`
const CURRENT_AS_SELF: i64 = 0x00010;
/// `FilesystemIterator::KEY_MODE_MASK`
const KEY_MODE_MASK: i64 = 0x00F00;
/// `FilesystemIterator::KEY_AS_PATHNAME`
const KEY_AS_PATHNAME: i64 = 0x00000;
/// `FilesystemIterator::KEY_AS_FILENAME`. `NEW_CURRENT_AND_KEY` is the same
/// bit: php defines it as `CURRENT_AS_FILEINFO | KEY_AS_FILENAME`, and
/// `CURRENT_AS_FILEINFO` is zero.
const KEY_AS_FILENAME: i64 = 0x00100;
/// `FilesystemIterator::FOLLOW_SYMLINKS`
const FOLLOW_SYMLINKS: i64 = 0x04000;
/// `FilesystemIterator::OTHER_MODE_MASK`
const OTHER_MODE_MASK: i64 = 0x07000;
/// `FilesystemIterator::SKIP_DOTS`
const SKIP_DOTS: i64 = 0x01000;
/// `FilesystemIterator::UNIX_PATHS`
const UNIX_PATHS: i64 = 0x02000;

/// The bits `getFlags()` reports and `setFlags()` may change. Everything
/// outside it is dropped, which is why `setFlags(1)` answers `0`.
const FLAGS_MASK: i64 = CURRENT_MODE_MASK | KEY_MODE_MASK | OTHER_MODE_MASK;

// ---- opening ---------------------------------------------------------------

/// Read a directory into php's entry order, `.` and `..` first, or raise
/// php's `UnexpectedValueException` with the docref-shaped text the SPL
/// error handler produces: `Class::__construct(/x): Failed to open
/// directory: No such file or directory`.
fn read_entries(ctx: &mut Ctx, who: &str, raw: &[u8]) -> Result<Vec<Vec<u8>>, Unwind> {
    let path = as_path(ctx, raw);
    list_dir(&path).map_err(|e| {
        Unwind::exception(
            "UnexpectedValueException",
            format!(
                "{who}({}): Failed to open directory: {}",
                String::from_utf8_lossy(raw),
                io_text(&e)
            ),
        )
    })
}

/// The matches of a glob pattern, in `glob()`'s order — which is
/// `glob(3)`'s, sorted, and exactly what the `glob://` stream wrapper feeds
/// php's `GlobIterator`.
fn read_matches(ctx: &mut Ctx, pattern: &[u8]) -> Result<Vec<Vec<u8>>, Unwind> {
    let id = ctx.native_by_name(b"glob").expect("glob is registered");
    let mut argv = [Value::Str(Str::from_vec(pattern.to_vec())), Value::Int(0)];
    Ok(match ctx.call_native(id, &mut argv)? {
        Value::Array(a) => a.values().map(|v| v.deref().to_php_bytes()).collect(),
        // `glob()` answers `false` only for a flag the platform lacks, which
        // this call never passes; an unmatched pattern is an empty list.
        _ => Vec::new(),
    })
}

/// Move the cursor onto the first entry it may stop at, honouring
/// `SKIP_DOTS`. php does this once in the constructor and again in every
/// `rewind()`.
fn settle(d: &mut Dir) {
    if d.flags & SKIP_DOTS != 0 {
        while d.pos < d.entries.len() && is_dot(&d.name_at(d.pos)) {
            d.pos += 1;
        }
    }
}

/// php's `ValueError` for a constructor handed an empty path.
fn not_empty(who: &str, param: &str, raw: &[u8]) -> Result<(), Unwind> {
    if raw.is_empty() {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 (${param}) must not be empty"
        )));
    }
    Ok(())
}

/// The shared constructor body: open, seed the state, position the cursor.
/// `glob` carries the `"glob://…"` spelling php dumps.
fn open_dir(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    raw: &[u8],
    flags: i64,
    glob: Option<Vec<u8>>,
) -> NativeResult {
    let (entries, path) = match &glob {
        // The glob wrapper strips the scheme and globs the rest; the path
        // of a glob iterator is read live off the current match, so there
        // is no fixed `_path` to store.
        Some(_) => (read_matches(ctx, raw)?, Vec::new()),
        None => (read_entries(ctx, who, raw)?, dir_open_path(raw)),
    };
    let st = with_fs(o, |s| {
        s.kind = Kind::Dir;
        s.file_name = None;
        s.path = path;
        let mut d = Dir {
            entries,
            pos: 0,
            index: 0,
            flags,
            sub_path: Vec::new(),
            glob,
            source: raw.to_vec(),
        };
        settle(&mut d);
        s.dir = Some(d);
        s.clone()
    });
    sync(o, &st);
    Ok(Value::Null)
}

/// The flag word as stored (unmasked; `getFlags()` masks on the way out).
fn flags_of(o: &Object) -> i64 {
    with_fs(o, |s| s.dir.as_ref().map_or(0, |d| d.flags))
}

// ---- DirectoryIterator -----------------------------------------------------

/// `DirectoryIterator::__construct(string $directory)` — no flags at all,
/// so `.` and `..` are always part of the walk.
fn di_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "DirectoryIterator::__construct";
    let raw = str_arg(ctx, who, 1, "directory", &args[0])?;
    not_empty(who, "directory", &raw)?;
    open_dir(ctx, o, who, &raw, 0, None)
}

/// `rewind(): void` — php's `rewinddir(3)`, which re-reads the directory,
/// so an entry created since the constructor ran shows up.
fn di_rewind(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let (source, is_glob) = with_fs(o, |s| {
        s.dir.as_ref().map_or((Vec::new(), false), |d| {
            (d.source.clone(), d.glob.is_some())
        })
    });
    // A glob stream rewinds by resetting its index; only a real directory
    // is read again. A directory that has since gone away keeps the old
    // listing, which is what an already-open `DIR*` would give php.
    let fresh = if is_glob {
        None
    } else {
        list_dir(&as_path(ctx, &source)).ok()
    };
    let st = with_fs(o, |s| {
        if let Some(d) = &mut s.dir {
            if let Some(names) = fresh {
                d.entries = names;
            }
            d.pos = 0;
            d.index = 0;
            settle(d);
        }
        s.clone()
    });
    sync(o, &st);
    Ok(Value::Null)
}

/// `valid(): bool` — php's "the entry name is not empty".
fn di_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(with_fs(o, |s| {
        s.dir.as_ref().is_some_and(|d| d.pos < d.entries.len())
    })))
}

/// `key(): int` — php's `index`, which counts `next()` calls and keeps
/// climbing past the end of the directory.
fn di_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_fs(o, |s| {
        s.dir.as_ref().map_or(0, |d| d.index)
    })))
}

/// `current(): DirectoryIterator` — the iterator itself, even when it is
/// no longer valid.
fn di_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Object(this(o)?.clone()))
}

/// `next(): void` — one step of the cursor and exactly one step of php's
/// index, however many dots `SKIP_DOTS` then skips over.
fn di_next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = with_fs(o, |s| {
        if let Some(d) = &mut s.dir {
            d.index += 1;
            if d.pos < d.entries.len() {
                d.pos += 1;
            }
            settle(d);
        }
        s.clone()
    });
    sync(o, &st);
    Ok(Value::Null)
}

/// `seek(int $offset): void` — php drives `rewind()`, `valid()` and
/// `next()` **through the object**, so a subclass's overrides run. A
/// negative offset is a no-op (the loop never starts) and running off the
/// end is `OutOfBoundsException: Seek position 100 is out of range`.
fn di_seek(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let target = int_arg(ctx, "DirectoryIterator::seek", 1, "offset", &args[0])?;
    let index = |o: &Object| with_fs(o, |s| s.dir.as_ref().map_or(0, |d| d.index));
    if index(o) > target {
        ctx.call_method(o, b"rewind", &[])?;
    }
    while index(o) < target {
        if !ctx.call_method(o, b"valid", &[])?.to_bool() {
            return Err(Unwind::exception(
                "OutOfBoundsException",
                format!("Seek position {target} is out of range"),
            ));
        }
        ctx.call_method(o, b"next", &[])?;
    }
    Ok(Value::Null)
}

/// `isDot(): bool`
fn di_is_dot(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(with_fs(o, |s| is_dot(&entry_name(s)))))
}

/// `getFilename(): string` — the bare entry name, which is why a
/// `DirectoryIterator` overrides `SplFileInfo`'s version at all.
fn di_get_filename(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| entry_name(s)))))
}

/// `getExtension(): string`
fn di_get_extension(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| {
        extension(&entry_name(s))
    }))))
}

/// `getBasename(string $suffix = ''): string`
fn di_get_basename(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let suffix = match args.first() {
        Some(v) => str_arg(ctx, "DirectoryIterator::getBasename", 1, "suffix", v)?,
        None => Vec::new(),
    };
    let name = with_fs(o, |s| entry_name(s));
    Ok(Value::Str(Str::from_vec(basename(&name, &suffix))))
}

/// `__toString(): string` — the entry name, **not** the pathname
/// `SplFileInfo::__toString()` gives.
fn di_to_string(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| entry_name(s)))))
}

/// `__debugInfo(): array`
fn dir_debug_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Array(with_fs(o, |s| debug_array(s))))
}

// ---- FilesystemIterator ----------------------------------------------------

/// `FilesystemIterator::__construct(string $directory, int $flags = FilesystemIterator::SKIP_DOTS)`
fn fi_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "FilesystemIterator::__construct";
    let raw = str_arg(ctx, who, 1, "directory", &args[0])?;
    let flags = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "flags", v)?,
        None => SKIP_DOTS,
    };
    not_empty(who, "directory", &raw)?;
    open_dir(ctx, o, who, &raw, flags, None)
}

/// `key(): string` — the pathname, or the bare entry name under
/// `KEY_AS_FILENAME`.
fn fi_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| {
        let flags = s.dir.as_ref().map_or(0, |d| d.flags);
        if flags & KEY_AS_FILENAME != 0 {
            entry_name(s)
        } else {
            // php builds the name unconditionally here, so an exhausted
            // iterator keys on `<dir>/` rather than on nothing.
            file_name(s).unwrap_or_default()
        }
    }))))
}

/// `current(): string|SplFileInfo|FilesystemIterator` — php tests the
/// masked mode word against `CURRENT_AS_PATHNAME` then
/// `CURRENT_AS_FILEINFO`, and falls back to `$this`, so any other value in
/// those four bits also means "self".
fn fi_current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = with_fs(o, |s| s.clone());
    let mode = st.dir.as_ref().map_or(0, |d| d.flags) & CURRENT_MODE_MASK;
    if mode == CURRENT_AS_PATHNAME {
        return Ok(Value::Str(Str::from_vec(
            file_name(&st).unwrap_or_default(),
        )));
    }
    if mode == CURRENT_AS_FILEINFO {
        // php builds the `SplFileInfo` through `create_type`, which refuses
        // an exhausted iterator with `RuntimeException: Could not open file`.
        check_entry(&st)?;
        let name = file_name(&st).unwrap_or_default();
        return make_info(ctx, &st, None, &name);
    }
    Ok(Value::Object(o.clone()))
}

/// `getFlags(): int`
fn fi_get_flags(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(flags_of(o) & FLAGS_MASK))
}

/// `setFlags(int $flags): void` — only the three mode masks move; bits
/// outside them are dropped rather than kept.
fn fi_set_flags(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let new = int_arg(ctx, "FilesystemIterator::setFlags", 1, "flags", &args[0])?;
    with_fs(o, |s| {
        if let Some(d) = &mut s.dir {
            d.flags = (d.flags & !FLAGS_MASK) | (new & FLAGS_MASK);
        }
    });
    Ok(Value::Null)
}

// ---- RecursiveDirectoryIterator --------------------------------------------

/// `RecursiveDirectoryIterator::__construct(string $directory, int $flags = 0)`
/// — note the default: unlike `FilesystemIterator` it does **not** skip
/// dots, so a plain `foreach` over one visits `.` and `..`.
fn rdi_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "RecursiveDirectoryIterator::__construct";
    let raw = str_arg(ctx, who, 1, "directory", &args[0])?;
    let flags = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "flags", v)?,
        None => 0,
    };
    not_empty(who, "directory", &raw)?;
    open_dir(ctx, o, who, &raw, flags, None)
}

/// `hasChildren(bool $allowLinks = false): bool` — `.` and `..` never
/// have children, and a symlink to a directory only does when the caller
/// asks or `FOLLOW_SYMLINKS` is set.
fn rdi_has_children(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let allow_links = args.first().is_some_and(|v| v.deref().to_bool());
    let st = with_fs(o, |s| s.clone());
    let name = entry_name(&st);
    if name.is_empty() || is_dot(&name) {
        return Ok(Value::Bool(false));
    }
    let full = file_name(&st).unwrap_or_default();
    let follow = st.dir.as_ref().map_or(0, |d| d.flags) & FOLLOW_SYMLINKS != 0;
    if !allow_links && !follow && lstat(ctx, &full).is_some_and(|m| m.file_type().is_symlink()) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(stat(ctx, &full).is_some_and(|m| m.is_dir())))
}

/// `getChildren(): RecursiveDirectoryIterator` — `new static($pathname,
/// $flags)` with the sub-path extended, so a subclass keeps its own class
/// all the way down and `getSubPathname()` stays relative to the root of
/// the walk. Descending into something that is not a directory surfaces as
/// the constructor's `UnexpectedValueException`, which is what php does.
fn rdi_get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = with_fs(o, |s| s.clone());
    let name = file_name(&st).unwrap_or_default();
    let flags = st.dir.as_ref().map_or(0, |d| d.flags);
    let child = ctx.new_object(o.class_id())?;
    ctx.call_method(
        &child,
        b"__construct",
        &[Value::Str(Str::from_vec(name)), Value::Int(flags)],
    )?;
    let entry = entry_name(&st);
    let parent_sub = st.dir.as_ref().map_or(Vec::new(), |d| d.sub_path.clone());
    let sub = if parent_sub.is_empty() {
        entry
    } else {
        let mut s = parent_sub;
        s.push(b'/');
        s.extend_from_slice(&entry);
        s
    };
    let cst = with_fs(&child, |s| {
        if let Some(d) = &mut s.dir {
            d.sub_path = sub;
        }
        s.info_class = st.info_class.clone();
        s.file_class = st.file_class.clone();
        s.clone()
    });
    sync(&child, &cst);
    Ok(Value::Object(child))
}

/// `getSubPath(): string` — the directories between the root of the walk
/// and this iterator, empty at the root.
fn rdi_get_sub_path(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| {
        s.dir.as_ref().map_or(Vec::new(), |d| d.sub_path.clone())
    }))))
}

/// `getSubPathname(): string` — the sub-path plus the entry. An exhausted
/// child answers `sub/`, because php concatenates the empty entry name
/// unconditionally.
fn rdi_get_sub_pathname(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Str(Str::from_vec(with_fs(o, |s| {
        let sub = s.dir.as_ref().map_or(Vec::new(), |d| d.sub_path.clone());
        let entry = entry_name(s);
        if sub.is_empty() {
            entry
        } else {
            let mut out = sub;
            out.push(b'/');
            out.extend_from_slice(&entry);
            out
        }
    }))))
}

// ---- GlobIterator ----------------------------------------------------------

/// Whether the pattern already names the `glob://` wrapper, which php
/// checks case-insensitively before prefixing.
fn has_glob_scheme(raw: &[u8]) -> bool {
    raw.len() >= 7 && raw[..7].eq_ignore_ascii_case(b"glob://")
}

/// `GlobIterator::__construct(string $pattern, int $flags = 0)` — php
/// opens the pattern through the `glob://` stream wrapper, so the name it
/// remembers (and dumps as `DirectoryIterator::$glob`) carries the scheme
/// while the glob itself runs on the rest.
fn glob_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let who = "GlobIterator::__construct";
    let raw = str_arg(ctx, who, 1, "pattern", &args[0])?;
    let flags = match args.get(1) {
        Some(v) => int_arg(ctx, who, 2, "flags", v)?,
        None => 0,
    };
    not_empty(who, "pattern", &raw)?;
    let (pattern, shown) = if has_glob_scheme(&raw) {
        (raw[7..].to_vec(), raw.clone())
    } else {
        let mut shown = b"glob://".to_vec();
        shown.extend_from_slice(&raw);
        (raw.clone(), shown)
    };
    open_dir(ctx, o, who, &pattern, flags, Some(shown))
}

/// `count(): int` — how many paths the pattern matched.
fn glob_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_fs(o, |s| {
        s.dir.as_ref().map_or(0, |d| d.entries.len() as i64)
    })))
}

/// `__clone()` — php marks `GlobIterator` uncloneable, so `clone $g` is
/// `Error: Trying to clone an uncloneable object of class GlobIterator`.
/// There is no "no-clone" class flag here, so the refusal is a `__clone`
/// body; the engine copies the slots first and then throws, which is the
/// same observable.
pub(super) fn no_clone(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let class = ctx.class_name_of(o);
    Err(Unwind::error(format!(
        "Trying to clone an uncloneable object of class {class}"
    )))
}

// ---- registration ----------------------------------------------------------

/// Register the four iterators. `glob` and `subPathName` are real private
/// slots because php's debug view shows them; both start out
/// [`Value::Uninit`] so an unconstructed iterator dumps the single
/// `pathName` member php dumps.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("DirectoryIterator")
        .extends("SplFileInfo")
        .implements(&["SeekableIterator"])
        .prop("glob", Visibility::Private, Value::Uninit)
        .payload_clone(clone_payload)
        .method("__construct", nm!(1, Some(1), di_construct))
        .method("getFilename", nm!(0, Some(0), di_get_filename))
        .method("getExtension", nm!(0, Some(0), di_get_extension))
        .method("getBasename", nm!(0, Some(1), di_get_basename))
        .method("isDot", nm!(0, Some(0), di_is_dot))
        .method("rewind", nm!(0, Some(0), di_rewind))
        .method("valid", nm!(0, Some(0), di_valid))
        .method("key", nm!(0, Some(0), di_key))
        .method("current", nm!(0, Some(0), di_current))
        .method("next", nm!(0, Some(0), di_next))
        .method("seek", nm!(1, Some(1), di_seek))
        .method("__toString", nm!(0, Some(0), di_to_string))
        .method("__debugInfo", nm!(0, Some(0), dir_debug_info))
        .finish();

    r.class("FilesystemIterator")
        .extends("DirectoryIterator")
        .class_const("CURRENT_MODE_MASK", Value::Int(CURRENT_MODE_MASK))
        .class_const("CURRENT_AS_PATHNAME", Value::Int(CURRENT_AS_PATHNAME))
        .class_const("CURRENT_AS_FILEINFO", Value::Int(CURRENT_AS_FILEINFO))
        .class_const("CURRENT_AS_SELF", Value::Int(CURRENT_AS_SELF))
        .class_const("KEY_MODE_MASK", Value::Int(KEY_MODE_MASK))
        .class_const("KEY_AS_PATHNAME", Value::Int(KEY_AS_PATHNAME))
        .class_const("FOLLOW_SYMLINKS", Value::Int(FOLLOW_SYMLINKS))
        .class_const("KEY_AS_FILENAME", Value::Int(KEY_AS_FILENAME))
        // php's `NEW_CURRENT_AND_KEY` is `CURRENT_AS_FILEINFO |
        // KEY_AS_FILENAME`, and `CURRENT_AS_FILEINFO` is zero, so the two
        // constants share a value.
        .class_const("NEW_CURRENT_AND_KEY", Value::Int(KEY_AS_FILENAME))
        .class_const("OTHER_MODE_MASK", Value::Int(OTHER_MODE_MASK))
        .class_const("SKIP_DOTS", Value::Int(SKIP_DOTS))
        .class_const("UNIX_PATHS", Value::Int(UNIX_PATHS))
        .method("__construct", nm!(1, Some(2), fi_construct))
        .method("rewind", nm!(0, Some(0), di_rewind))
        .method("key", nm!(0, Some(0), fi_key))
        .method("current", nm!(0, Some(0), fi_current))
        .method("getFlags", nm!(0, Some(0), fi_get_flags))
        .method("setFlags", nm!(1, Some(1), fi_set_flags))
        .finish();

    r.class("RecursiveDirectoryIterator")
        .extends("FilesystemIterator")
        .implements(&["RecursiveIterator"])
        .prop("subPathName", Visibility::Private, Value::Uninit)
        .method("__construct", nm!(1, Some(2), rdi_construct))
        .method("hasChildren", nm!(0, Some(1), rdi_has_children))
        .method("getChildren", nm!(0, Some(0), rdi_get_children))
        .method("getSubPath", nm!(0, Some(0), rdi_get_sub_path))
        .method("getSubPathname", nm!(0, Some(0), rdi_get_sub_pathname))
        .finish();

    r.class("GlobIterator")
        .extends("FilesystemIterator")
        .implements(&["Countable"])
        .method("__construct", nm!(1, Some(2), glob_construct))
        .method("count", nm!(0, Some(0), glob_count))
        .method("__clone", nm!(0, Some(0), no_clone))
        .finish();
}
