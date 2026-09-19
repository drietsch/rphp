//! SPL's filesystem classes (php-src `ext/spl/spl_directory.c`): the
//! `SplFileInfo` value object, the four directory iterators built on it, and
//! `SplFileObject`/`SplTempFileObject`.
//!
//! # One object, three shapes
//!
//! php gives all seven classes the same C struct, `spl_filesystem_object`,
//! and switches on a `type` field — `SPL_FS_INFO`, `SPL_FS_DIR`,
//! `SPL_FS_FILE`. That is why `DirectoryIterator` *is* an `SplFileInfo` that
//! walks, and why `getPathname()` means "the stored name" for one shape and
//! "the directory plus the entry under the cursor" for another. This module
//! keeps the same single state, [`common::Fs`], in the instance's native
//! [`Payload`](rphp_value::Payload), and every class registers
//! [`ClassBuilder::payload_clone`](rphp_runtime::ClassBuilder::payload_clone)
//! so `clone $it` copies the cursor.
//!
//! Two fields carry the whole path model:
//!
//! * `file_name` — the name the constructor was handed, with **trailing
//!   slashes stripped** (all but the first character): `new
//!   SplFileInfo('/a/b//')` stores `/a/b`. It is `None` before the
//!   constructor has run, which is what makes `getFilename()` throw `Error:
//!   Object not initialized` on a `newInstanceWithoutConstructor` object
//!   while `getPathname()` quietly answers `''`.
//! * `path` — php's `_path`, the directory part. It is *not* `dirname()`:
//!   php scans back to the last slash and then drops that slash **only when
//!   more than one character is left**, so `/tmp` has the path `''`, not
//!   `/`, and `getPathInfo()` (which really is `dirname()`) answers `/`
//!   where `getPath()` answers `''`.
//!
//! # What is a real property
//!
//! php synthesizes the dump through `get_debug_info`, and the members it
//! shows are:
//!
//! ```text
//! object(SplFileObject)#1 (5) {
//!   ["pathName":"SplFileInfo":private]=>   string(12) "/tmp/f.txt"
//!   ["fileName":"SplFileInfo":private]=>   string(5) "f.txt"
//!   ["openMode":"SplFileObject":private]=> string(1) "r"
//!   ["delimiter":"SplFileObject":private]=> string(1) ","
//!   ["enclosure":"SplFileObject":private]=> string(1) "\""
//! }
//! ```
//!
//! so those are declared as real private slots here and refreshed
//! ([`common::sync`]) after every state change, which makes `var_dump` and
//! `print_r` byte-exact. A slot php leaves out — `fileName` on an
//! uninitialized or exhausted iterator — is stored as
//! [`Value::Uninit`](rphp_value::Value), which the formatters skip and
//! `prop_count` does not count, so even the member count matches.
//!
//! # Known divergences (ADR-004)
//!
//! * php's debug view of *every* directory iterator carries
//!   `["subPathName":"RecursiveDirectoryIterator":private]`, including on a
//!   plain `DirectoryIterator`, a `FilesystemIterator` and a `GlobIterator`
//!   — a property whose declaring class is one those classes do not extend,
//!   which [`ClassBuilder`](rphp_runtime::ClassBuilder) cannot express. The
//!   slot is declared on `RecursiveDirectoryIterator`, where it is exact;
//!   the other three dump one member short. `__debugInfo()` returns php's
//!   full array on all of them for when the formatters learn to consult it.
//! * `(array) $info` yields the mangled private keys here and `[]` in php,
//!   the same gap `spl_containers2.rs` records: php answers
//!   `get_properties_for(ARRAY_CAST)` with nothing and only fills the debug
//!   view.
//! * `serialize(new SplFileInfo(…))` is refused by php with `Exception:
//!   Serialization of 'SplFileInfo' is not allowed`; the engine has no
//!   "internal class without a serializer" rule yet, so it serializes the
//!   two private slots instead.
//! * `SplFileInfo::getCTime()` reports `st_ctime`, php's inode-change time,
//!   which is what php's own `SplFileInfo` and `filectime()` both answer.
//!   `filestat.rs`'s `filectime()` currently reports the *birth* time on
//!   macOS, so the two disagree inside rphp where they agree in php.
//! * `getRealPath()` on a directory iterator that has run off the end
//!   answers `false` here. php answers whatever its `file_name` cache last
//!   held — the directory, if a `key()` happened to build one, and `false`
//!   otherwise — because that cache is never cleared. The model keeps no
//!   such cache, so the answer is the consistent one.
//! * A directory listing is snapshotted at construction rather than held as
//!   an open `DIR*` (the engine has no directory-handle resource);
//!   `rewind()` re-reads it, which is what `rewinddir(3)` gives php. An
//!   entry created *between* a construction and the first walk is therefore
//!   visible in php only after a `rewind()`, and here too.
//! * `SplFileInfo::_bad_state_ex()` is `final public` in php. `nm!` cannot
//!   set `final`, so Reflection reports it as non-final; the deprecation
//!   and the `Error` are php's.
//! * `SplFileObject::ftruncate()` is **not registered**: it has to shorten
//!   the `Stream`'s buffer, and no native `file.rs` exposes can — the same
//!   blocker `file2.rs` records for the procedural `ftruncate()`. The other
//!   `fileobject.rs` gaps (the overridden-`getCurrentLine` path, `$context`,
//!   the multi-line CSV record, the notice name on an unusable handle for
//!   the calls that are forwarded whole) are listed in that file's header.
//!
//! Everything else is checked against stock php 8.5 by
//! `examples/tier-a/spl/fileinfo.php` and `examples/tier-a/spl/directory.php`.

mod common;
mod fileinfo;
mod fileobject;
mod iterators;

use rphp_runtime::{NativeFn, Registry};

/// Functions this module provides: none. `ext/spl`'s filesystem half is
/// classes only — the procedural `glob()`, `scandir()` and the handle
/// functions live in `file2.rs`, `dir.rs` and `file.rs`.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides. Every constant here is a *class*
/// constant (`FilesystemIterator::SKIP_DOTS`, `SplFileObject::READ_CSV`),
/// declared with its class, so there is nothing global to add.
pub(crate) fn register_constants(_r: &mut Registry) {}

/// Register the filesystem family, parents before children:
/// `SplFileInfo`, then `DirectoryIterator` → `FilesystemIterator` →
/// `RecursiveDirectoryIterator` / `GlobIterator`, then `SplFileObject` →
/// `SplTempFileObject`.
///
/// `RecursiveIterator`, `SeekableIterator` and `Countable` are registered by
/// `spl_interfaces`/`spl_containers` before this runs and are only
/// referenced, never re-declared.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.interp().class_by_name(b"SplFileInfo").is_some() {
        return;
    }
    fileinfo::register_classes(r);
    iterators::register_classes(r);
    fileobject::register_classes(r);
}
