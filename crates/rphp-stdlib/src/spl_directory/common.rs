//! The state every class in `spl_directory.rs` shares — php's one
//! `spl_filesystem_object` — plus the path arithmetic, the stat helpers and
//! the argument coercions the seven classes reach for.
//!
//! Nothing here re-enters the VM while the payload is borrowed: a method
//! reads what it needs out of [`with_fs`], calls, then writes back. That is
//! the same rule `spl_decorators.rs` states, and it matters more here
//! because `getChildren()` and `openFile()` construct user subclasses.

use std::fs;
use std::path::PathBuf;

use rphp_runtime::{Ctx, Unwind};
use rphp_value::{Object, Payload, Str, Value};

use crate::filestat::arg_path;

/// Which of php's three shapes an instance is in (`intern->type`).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `SPL_FS_INFO` — a bare `SplFileInfo`.
    #[default]
    Info,
    /// `SPL_FS_DIR` — a `DirectoryIterator` and everything below it.
    Dir,
    /// `SPL_FS_FILE` — an `SplFileObject`.
    File,
}

/// The directory half of the state: the entries the iterator walks, the
/// cursor, the flag word and the recursion bookkeeping.
///
/// php holds an open `DIR*` and reads one entry at a time; the listing is
/// snapshotted here instead, because the engine has no directory-handle
/// resource and because `clone` has to reproduce a cursor position anyway
/// (php's clone re-opens the directory and re-reads up to the old index).
/// `rewind()` re-reads the directory, which is what `rewinddir(3)` gives
/// php.
#[derive(Clone, Default)]
pub(crate) struct Dir {
    /// For a directory: the entry names, `.` and `..` first. For a
    /// `GlobIterator`: the whole match paths, which carry their own
    /// directory part (php reads that back out of the glob stream, so
    /// `getPath()` follows the cursor).
    ///
    /// Shared: the iterator's every method starts from a copy of the
    /// state, and a directory listing copied per step made a walk
    /// quadratic.
    pub entries: std::rc::Rc<Vec<Vec<u8>>>,
    /// The cursor into [`Dir::entries`]; `entries.len()` means exhausted,
    /// which is php's empty `d_name`.
    pub pos: usize,
    /// php's `u.dir.index`, the number `DirectoryIterator::key()` answers.
    /// It counts `next()` calls, so with `SKIP_DOTS` it is *not*
    /// [`Dir::pos`], and it keeps climbing past the end.
    pub index: i64,
    /// The `FilesystemIterator::*` flag word, as given (php masks on read).
    pub flags: i64,
    /// php's `u.dir.sub_path`: the part of the path below the root of a
    /// `RecursiveDirectoryIterator` walk. Empty at the root, which is php's
    /// `NULL`.
    pub sub_path: Vec<u8>,
    /// `"glob://<pattern>"` for a `GlobIterator`, `None` otherwise — the
    /// value php's debug view shows under `DirectoryIterator::$glob`.
    pub glob: Option<Vec<u8>>,
    /// The directory the iterator was opened on, so `rewind()` can re-read
    /// it, or the glob pattern.
    pub source: Vec<u8>,
}

/// The file half of the state: the open handle and php's one-line cache.
#[derive(Clone)]
pub(crate) struct File {
    /// The stream resource, opened through `file.rs`'s `fopen`.
    pub stream: Value,
    /// The mode string, which php dumps as `SplFileObject::$openMode`.
    pub open_mode: Vec<u8>,
    /// `DROP_NEW_LINE | READ_AHEAD | SKIP_EMPTY | READ_CSV` (php masks on
    /// read, and the directory flags share the same word in php's struct).
    pub flags: i64,
    /// `setMaxLineLen()`; `0` means "a whole line".
    pub max_line_len: i64,
    /// `getCsvControl()`.
    pub delimiter: u8,
    pub enclosure: u8,
    /// Empty when the caller asked for no escaping at all, which php
    /// allows and reports back as `""`.
    pub escape: Vec<u8>,
    /// Whether an escape character was ever named explicitly. php turns
    /// the `fputcsv()`/`fgetcsv()` deprecation off once `setCsvControl()`
    /// has supplied one.
    pub escape_given: bool,
    /// php's `u.file.current_line`: the line under the cursor, or `None`
    /// when the cache is empty (`spl_filesystem_file_free_line`).
    pub current_line: Option<Vec<u8>>,
    /// php's `u.file.current_zval`: the parsed row under `READ_CSV`.
    pub current_zval: Option<Value>,
    /// php's `u.file.current_line_num`, which `key()` answers.
    pub line_num: i64,
}

impl Dir {
    /// The entry name at a position: a directory entry as `readdir(3)`
    /// gave it, or the basename of a glob match.
    pub(crate) fn name_at(&self, i: usize) -> Vec<u8> {
        match self.entries.get(i) {
            Some(e) if self.glob.is_some() => split_match(e).1.to_vec(),
            Some(e) => e.clone(),
            None => Vec::new(),
        }
    }
}

/// php's `spl_filesystem_object`: one payload for all seven classes.
#[derive(Clone, Default)]
pub(crate) struct Fs {
    pub kind: Kind,
    /// php's `intern->file_name`. `None` until a constructor runs.
    pub file_name: Option<Vec<u8>>,
    /// php's `intern->_path` (see the module header: not `dirname()`).
    pub path: Vec<u8>,
    /// `setInfoClass()`; `None` means `SplFileInfo`.
    pub info_class: Option<Vec<u8>>,
    /// `setFileClass()`; `None` means `SplFileObject`.
    pub file_class: Option<Vec<u8>>,
    pub dir: Option<Dir>,
    pub file: Option<File>,
}

// ---- the payload -----------------------------------------------------------

/// The receiver of an instance method.
pub(crate) fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// Run `f` on the instance's hidden state, installing an empty one first
/// when there is none (an instance built by
/// [`Interp::instantiate`](rphp_runtime::Interp::instantiate) — a cast, an
/// `unserialize` — has no payload).
///
/// The payload is borrowed for the whole call, so `f` must never re-enter
/// the VM.
pub(crate) fn with_fs<R>(o: &Object, f: impl FnOnce(&mut Fs) -> R) -> R {
    let present = o.with_payload::<Fs, _>(|_| ()).is_some();
    if !present {
        o.set_payload(Payload::Native(Box::new(Fs::default())));
    }
    o.with_payload::<Fs, _>(f)
        .expect("payload installed just above")
}

/// `clone` copies the whole state, cursor included — php re-opens the
/// directory and reads forward to the old index, which comes to the same
/// thing.
pub(crate) fn clone_payload(
    _: &mut rphp_runtime::Interp,
    src: &Object,
    dst: &Object,
) -> Result<(), Unwind> {
    let copy = with_fs(src, |s| s.clone());
    dst.set_payload(Payload::Native(Box::new(copy)));
    Ok(())
}

// ---- path arithmetic -------------------------------------------------------

/// php's `spl_filesystem_info_set_filename`: trailing slashes go, but never
/// the first character, so `'/'` and `'//'` both keep one slash and `''`
/// stays empty.
pub(crate) fn strip_trailing_slashes(s: &[u8]) -> &[u8] {
    let mut n = s.len();
    while n > 1 && s[n - 1] == b'/' {
        n -= 1;
    }
    &s[..n]
}

/// The length of php's `_path` for a file name: scan back to the last
/// slash, then drop that slash **only if more than one character is left**.
/// That `else length = 0` in php-src is why `/tmp` has the path `''`.
pub(crate) fn path_prefix_len(s: &[u8]) -> usize {
    let mut n = s.len();
    while n > 1 && s[n - 1] != b'/' {
        n -= 1;
    }
    if n > 1 {
        n - 1
    } else {
        0
    }
}

/// Store a name the way `SplFileInfo::__construct` does and derive `_path`.
pub(crate) fn set_file_name(st: &mut Fs, raw: &[u8]) {
    let name = strip_trailing_slashes(raw).to_vec();
    st.path = name[..path_prefix_len(&name)].to_vec();
    st.file_name = Some(name);
}

/// php's `spl_filesystem_dir_open` path: **one** trailing slash comes off a
/// name longer than a single character, so `'/a/b/'` opens with the path
/// `/a/b` while `'/a/b//'` keeps `/a/b/` — and then the entry is appended
/// with another slash, giving `/a/b//x`.
pub(crate) fn dir_open_path(raw: &[u8]) -> Vec<u8> {
    if raw.len() > 1 && raw[raw.len() - 1] == b'/' {
        raw[..raw.len() - 1].to_vec()
    } else {
        raw.to_vec()
    }
}

/// Split a glob match into the directory part php reports as the path and
/// the entry name it reports as the filename.
fn split_match(m: &[u8]) -> (&[u8], &[u8]) {
    match m.iter().rposition(|&b| b == b'/') {
        Some(i) => (&m[..i], &m[i + 1..]),
        None => (&m[..0], m),
    }
}

/// The entry name under the cursor (php's `u.dir.entry.d_name`), empty past
/// the end.
pub(crate) fn entry_name(st: &Fs) -> Vec<u8> {
    match &st.dir {
        Some(d) => d.name_at(d.pos),
        None => Vec::new(),
    }
}

/// A directory listing in php's shape: `readdir(3)` order with `.` and
/// `..` in front, which is what php's `DirectoryIterator` walks and what
/// `scandir()` in `dir.rs` starts from.
pub(crate) fn list_dir(path: &std::path::Path) -> std::io::Result<Vec<Vec<u8>>> {
    let rd = fs::read_dir(path)?;
    let mut names: Vec<Vec<u8>> = vec![b".".to_vec(), b"..".to_vec()];
    for e in rd.flatten() {
        names.push(e.file_name().to_string_lossy().into_owned().into_bytes());
    }
    Ok(names)
}

/// php's `spl_filesystem_object_get_path`: `_path` for a directory, but the
/// **live** directory part of the current match for a `GlobIterator`, which
/// the glob stream keeps updating as entries are read. That is why
/// `getPath()` on an exhausted `GlobIterator` answers `''`.
pub(crate) fn object_path(st: &Fs) -> Vec<u8> {
    if let Some(d) = &st.dir {
        if d.glob.is_some() {
            return match d.entries.get(d.pos) {
                Some(e) => split_match(e).0.to_vec(),
                None => Vec::new(),
            };
        }
    }
    st.path.clone()
}

/// php's `spl_filesystem_object_get_file_name`: for a directory the path
/// and the entry joined by a slash — *unless* the path is empty, in which
/// case the entry stands alone. For the other two shapes it is the stored
/// name.
///
/// This join is **unconditional**, so an exhausted `FilesystemIterator`
/// names `<dir>/` — which is exactly what its `key()` answers.
pub(crate) fn file_name(st: &Fs) -> Option<Vec<u8>> {
    if st.kind != Kind::Dir {
        return st.file_name.clone();
    }
    let path = object_path(st);
    let name = entry_name(st);
    Some(if path.is_empty() {
        name
    } else {
        let mut out = path;
        out.push(b'/');
        out.extend_from_slice(&name);
        out
    })
}

/// php's `spl_filesystem_object_get_pathname`, which is *not* the same
/// thing: for a directory iterator it answers nothing at all once the
/// cursor has run off the end. That is why an exhausted
/// `FilesystemIterator` has the key `<dir>/` and the pathname `''`, and
/// why its dump shows `pathName` as the empty string.
pub(crate) fn pathname(st: &Fs) -> Option<Vec<u8>> {
    if st.kind == Kind::Dir && entry_name(st).is_empty() {
        return None;
    }
    file_name(st)
}

/// `SplFileInfo::getFilename()`: the tail of the name after `_path`.
pub(crate) fn file_name_part(st: &Fs) -> Option<Vec<u8>> {
    let full = file_name(st)?;
    let path_len = object_path(st).len();
    Some(if path_len > 0 && path_len < full.len() {
        full[path_len + 1..].to_vec()
    } else {
        full
    })
}

/// `.` or `..`, the two entries `SKIP_DOTS` drops and `hasChildren()`
/// refuses to descend into.
pub(crate) fn is_dot(name: &[u8]) -> bool {
    name == b"." || name == b".."
}

/// php's `php_basename` with no suffix, which `getBasename()` and
/// `getExtension()` both start from.
pub(crate) fn basename(s: &[u8], suffix: &[u8]) -> Vec<u8> {
    // Trailing slashes are not part of the name.
    let mut end = s.len();
    while end > 0 && s[end - 1] == b'/' {
        end -= 1;
    }
    let start = s[..end]
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(0, |i| i + 1);
    let base = &s[start..end];
    if !suffix.is_empty() && base.len() > suffix.len() && base.ends_with(suffix) {
        base[..base.len() - suffix.len()].to_vec()
    } else {
        base.to_vec()
    }
}

/// `getExtension()`: everything after the last dot of the basename, and the
/// empty string when there is none. `.hidden` therefore has the extension
/// `hidden` and `.` has none, both of which php agrees with.
pub(crate) fn extension(s: &[u8]) -> Vec<u8> {
    let base = basename(s, b"");
    match base.iter().rposition(|&b| b == b'.') {
        Some(i) => base[i + 1..].to_vec(),
        None => Vec::new(),
    }
}

// ---- the php-visible slots -------------------------------------------------

/// Write a declared slot, and *only* a declared slot: an undeclared name
/// would land in the dynamic-property table and show up in every dump.
/// `subPathName` exists on `RecursiveDirectoryIterator` alone, so a plain
/// `DirectoryIterator` must silently skip it.
fn set_declared(o: &Object, name: &[u8], v: Value) {
    if let Some(i) = o.layout().slot_of(name) {
        o.set_slot(i, v);
    }
}

/// Refresh the private slots php's `get_debug_info` synthesizes, so
/// `var_dump`/`print_r` show what php shows. A slot php leaves out becomes
/// [`Value::Uninit`], which the formatters skip and `prop_count` ignores.
///
/// Called after every state change: a constructor, a `rewind`, a `next`, a
/// `setCsvControl`.
pub(crate) fn sync(o: &Object, st: &Fs) {
    set_declared(
        o,
        b"pathName",
        match pathname(st) {
            Some(p) => Value::Str(Str::from_vec(p)),
            None => Value::string(b""),
        },
    );
    // php omits `fileName` entirely while `intern->file_name` is NULL —
    // before the constructor, and on a directory iterator that has run off
    // the end.
    let shown = if st.kind == Kind::Dir {
        let name = entry_name(st);
        if name.is_empty() {
            None
        } else {
            file_name_part(st)
        }
    } else {
        st.file_name.as_ref().and(file_name_part(st))
    };
    set_declared(
        o,
        b"fileName",
        match shown {
            Some(f) => Value::Str(Str::from_vec(f)),
            None => Value::Uninit,
        },
    );
    if let Some(d) = &st.dir {
        set_declared(
            o,
            b"glob",
            match &d.glob {
                Some(g) => Value::Str(Str::from_vec(g.clone())),
                None => Value::Bool(false),
            },
        );
        set_declared(
            o,
            b"subPathName",
            Value::Str(Str::from_vec(d.sub_path.clone())),
        );
    }
    if let Some(f) = &st.file {
        set_declared(
            o,
            b"openMode",
            Value::Str(Str::from_vec(f.open_mode.clone())),
        );
        set_declared(
            o,
            b"delimiter",
            Value::Str(Str::from_vec(vec![f.delimiter])),
        );
        set_declared(
            o,
            b"enclosure",
            Value::Str(Str::from_vec(vec![f.enclosure])),
        );
    }
}

/// php's mangled private-property key, `"\0Class\0prop"`, for the
/// `__debugInfo()` arrays.
pub(crate) fn mangled(class: &[u8], prop: &[u8]) -> rphp_value::ArrayKey {
    let mut name = vec![0u8];
    name.extend_from_slice(class);
    name.push(0);
    name.extend_from_slice(prop);
    rphp_value::ArrayKey::Str(name.into())
}

/// The array php's `get_debug_info` builds, which `__debugInfo()` returns
/// verbatim. It carries `subPathName` for *every* directory iterator, which
/// the declared slots cannot (see the module header).
pub(crate) fn debug_array(st: &Fs) -> rphp_value::Array {
    let mut out = rphp_value::Array::new();
    out.set(
        mangled(b"SplFileInfo", b"pathName"),
        match pathname(st) {
            Some(p) => Value::Str(Str::from_vec(p)),
            None => Value::string(b""),
        },
    );
    let has_name = if st.kind == Kind::Dir {
        !entry_name(st).is_empty()
    } else {
        st.file_name.is_some()
    };
    if has_name {
        if let Some(f) = file_name_part(st) {
            out.set(
                mangled(b"SplFileInfo", b"fileName"),
                Value::Str(Str::from_vec(f)),
            );
        }
    }
    if let Some(d) = &st.dir {
        out.set(
            mangled(b"DirectoryIterator", b"glob"),
            match &d.glob {
                Some(g) => Value::Str(Str::from_vec(g.clone())),
                None => Value::Bool(false),
            },
        );
        out.set(
            mangled(b"RecursiveDirectoryIterator", b"subPathName"),
            Value::Str(Str::from_vec(d.sub_path.clone())),
        );
    }
    if let Some(f) = &st.file {
        out.set(
            mangled(b"SplFileObject", b"openMode"),
            Value::Str(Str::from_vec(f.open_mode.clone())),
        );
        out.set(
            mangled(b"SplFileObject", b"delimiter"),
            Value::Str(Str::from_vec(vec![f.delimiter])),
        );
        out.set(
            mangled(b"SplFileObject", b"enclosure"),
            Value::Str(Str::from_vec(vec![f.enclosure])),
        );
    }
    out
}

// ---- the filesystem --------------------------------------------------------

/// The stored name as a path the engine can hand a syscall: relative names
/// resolve against the interpreter's cwd, exactly as `filestat.rs` does it.
pub(crate) fn as_path(ctx: &Ctx, name: &[u8]) -> PathBuf {
    arg_path(ctx, &Value::Str(Str::from_vec(name.to_vec())))
}

/// A following `stat(2)` through php's per-request stat cache — the same
/// map `filestat.rs` fills, so `clearstatcache()` and a write through
/// `file_put_contents()` are visible here too.
pub(crate) fn stat(ctx: &mut Ctx, name: &[u8]) -> Option<fs::Metadata> {
    let p = as_path(ctx, name);
    crate::filestat::cached_stat(ctx, &p)
}

/// An `lstat(2)`, which does *not* go through the cache: the cache holds
/// the followed stat, and `getType()`/`isLink()` need the link itself.
pub(crate) fn lstat(ctx: &Ctx, name: &[u8]) -> Option<fs::Metadata> {
    fs::symlink_metadata(as_path(ctx, name)).ok()
}

/// The `struct stat` fields `SplFileInfo` publishes that
/// `std::fs::Metadata` only offers through the unix extension trait.
pub(crate) struct StatFields {
    pub mode: u32,
    pub ino: i64,
    pub uid: i64,
    pub gid: i64,
    pub atime: i64,
    pub mtime: i64,
    /// `st_ctime`, the inode-change time — not the birth time.
    pub ctime: i64,
}

#[cfg(unix)]
pub(crate) fn stat_fields(m: &fs::Metadata) -> StatFields {
    use std::os::unix::fs::MetadataExt;
    StatFields {
        mode: m.mode(),
        ino: m.ino() as i64,
        uid: i64::from(m.uid()),
        gid: i64::from(m.gid()),
        atime: m.atime(),
        mtime: m.mtime(),
        ctime: m.ctime(),
    }
}

/// Where there is no `struct stat`, php's Windows build synthesizes these;
/// rphp has no Windows target yet, so this only keeps the code compiling.
#[cfg(not(unix))]
pub(crate) fn stat_fields(_m: &fs::Metadata) -> StatFields {
    StatFields {
        mode: 0,
        ino: 0,
        uid: 0,
        gid: 0,
        atime: 0,
        mtime: 0,
        ctime: 0,
    }
}

/// The `getType()` name of a file that is neither a directory, a regular
/// file nor a symlink.
#[cfg(unix)]
pub(crate) fn special_type(t: &fs::FileType) -> Option<&'static [u8]> {
    use std::os::unix::fs::FileTypeExt;
    if t.is_fifo() {
        Some(&b"fifo"[..])
    } else if t.is_char_device() {
        Some(&b"char"[..])
    } else if t.is_block_device() {
        Some(&b"block"[..])
    } else if t.is_socket() {
        Some(&b"socket"[..])
    } else {
        None
    }
}

#[cfg(not(unix))]
pub(crate) fn special_type(_t: &fs::FileType) -> Option<&'static [u8]> {
    None
}

// ---- argument coercion -----------------------------------------------------

/// php's weak `string` parameter coercion with php's diagnostics: a scalar
/// converts, `null` is the 8.1 null-to-non-nullable deprecation, an object
/// goes through `__toString`, and an array is the `TypeError`
/// `zend_parse_parameters` raises.
pub(crate) fn str_arg(
    ctx: &mut Ctx,
    who: &str,
    pos: u32,
    name: &str,
    v: &Value,
) -> Result<Vec<u8>, Unwind> {
    match &*v.deref() {
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{who}(): Passing null to parameter #{pos} (${name}) of type string is deprecated"
            ))?;
            Ok(Vec::new())
        }
        Value::Array(_) => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type string, array given"
        ))),
        v @ (Value::Object(_) | Value::Closure(_)) => {
            let v = v.clone();
            match ctx.to_string(&v) {
                Ok(s) => Ok(s.as_bytes().to_vec()),
                Err(_) => Err(Unwind::type_error(format!(
                    "{who}(): Argument #{pos} (${name}) must be of type string, {} given",
                    rphp_runtime::value_name(&v)
                ))),
            }
        }
        other => Ok(other.to_php_bytes()),
    }
}

/// php's weak `int` parameter coercion with php's diagnostics (the helper
/// `spl_containers2.rs` and `spl_decorators.rs` also carry).
pub(crate) fn int_arg(
    ctx: &mut Ctx,
    who: &str,
    pos: u32,
    name: &str,
    v: &Value,
) -> Result<i64, Unwind> {
    match &*v.deref() {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() {
                ctx.deprecated(&format!(
                    "Implicit conversion from float {} to int loses precision",
                    Value::Float(*f).to_php_string()
                ))?;
            }
            Ok(*f as i64)
        }
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{who}(): Passing null to parameter #{pos} (${name}) of type int is deprecated"
            ))?;
            Ok(0)
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(s.to_int()),
        other => Err(Unwind::type_error(format!(
            "{who}(): Argument #{pos} (${name}) must be of type int, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// An optional `?string $class` naming a class that must derive from
/// `base`, with php's `must be a class name derived from X or null` text.
///
/// `base` is **the object's current `info_class`**, not `SplFileInfo`:
/// `zend_parse_arg_class` checks against whatever the preset value was, so
/// after `setInfoClass(MyInfo::class)` php reports `derived from MyInfo`. A
/// name the autoloader cannot produce is reported the same way php reports
/// it — as this `TypeError`, not as `Class not found`.
pub(crate) fn class_arg(
    ctx: &mut Ctx,
    who: &str,
    base: &[u8],
    v: Option<&Value>,
) -> Result<Option<Vec<u8>>, Unwind> {
    let Some(v) = v else { return Ok(None) };
    let given = v.deref().into_owned();
    if matches!(given, Value::Null | Value::Uninit) {
        return Ok(None);
    }
    let name = str_arg(ctx, who, 1, "class", &given)?;
    let base_id = ctx.lookup_class(base)?;
    if let (Some(base_id), Ok(Some(id))) = (base_id, ctx.lookup_class(&name)) {
        if ctx.is_subclass_or_eq(id, base_id) {
            return Ok(Some(name));
        }
    }
    Err(Unwind::type_error(format!(
        "{who}(): Argument #1 ($class) must be a class name derived from {} or null, {} given",
        String::from_utf8_lossy(base),
        String::from_utf8_lossy(&name)
    )))
}

/// The same check without the `or null` wording, for
/// `setInfoClass`/`setFileClass`, whose parameter is not nullable.
pub(crate) fn required_class_arg(
    ctx: &mut Ctx,
    who: &str,
    base: &str,
    v: &Value,
) -> Result<Vec<u8>, Unwind> {
    let name = str_arg(ctx, who, 1, "class", v)?;
    let base_id = ctx
        .class_by_name(base.as_bytes())
        .expect("registered above");
    if let Ok(Some(id)) = ctx.lookup_class(&name) {
        if ctx.is_subclass_or_eq(id, base_id) {
            return Ok(name);
        }
    }
    Err(Unwind::type_error(format!(
        "{who}(): Argument #1 ($class) must be a class name derived from {base}, {} given",
        String::from_utf8_lossy(&name)
    )))
}
