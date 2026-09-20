//! php-src `ext/standard/info.c`, `pageinfo.c`, the environment / ini /
//! process half of `basic_functions.c`, and the `gc_*` / extension queries
//! of `Zend/zend_builtin_functions.c`: `php_uname`, `phpinfo`,
//! `php_ini_loaded_file`, `php_ini_scanned_files`, `get_cfg_var`,
//! `ini_get_all`, `get_include_path`, `set_include_path`, `getenv`,
//! `putenv`, `sys_get_temp_dir`, `getmypid`, `getmyuid`, `getmygid`,
//! `getmyinode`, `getlastmod`, `get_current_user`, `gc_*`, `zend_version`,
//! `extension_loaded`, `get_loaded_extensions`, `get_extension_funcs`,
//! `connection_status`, `connection_aborted`, `ignore_user_abort`.
//! (`phpversion`, `php_sapi_name`, `ini_restore`, `set_time_limit` are in
//! `basic_functions.rs`.)
//!
//! Platform facts come from `std` only: `php_uname` reads `uname` through a
//! child process once (falling back to `std::env::consts`), the hostname
//! from `HOSTNAME` / `/proc/sys/kernel/hostname` / `hostname`, the script
//! owner from the script file's metadata and `/etc/passwd`. Values that
//! depend on the machine are allowlisted as `platform-value` / `pid` in the
//! differential suite.

use std::sync::OnceLock;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("php_uname", 0, Some(1), php_uname),
    nf!("phpinfo", 0, Some(1), phpinfo),
    nf!("php_ini_loaded_file", 0, Some(0), php_ini_loaded_file),
    nf!("php_ini_scanned_files", 0, Some(0), php_ini_scanned_files),
    nf!("get_cfg_var", 1, Some(1), get_cfg_var),
    nf!("ini_get_all", 0, Some(2), ini_get_all),
    nf!("get_include_path", 0, Some(0), get_include_path),
    nf!("set_include_path", 1, Some(1), set_include_path),
    nf!("getenv", 0, Some(2), getenv),
    nf!("putenv", 1, Some(1), putenv),
    nf!("sys_get_temp_dir", 0, Some(0), sys_get_temp_dir),
    nf!("getmypid", 0, Some(0), getmypid),
    nf!("getmyuid", 0, Some(0), getmyuid),
    nf!("getmygid", 0, Some(0), getmygid),
    nf!("getmyinode", 0, Some(0), getmyinode),
    nf!("getlastmod", 0, Some(0), getlastmod),
    nf!("get_current_user", 0, Some(0), get_current_user),
    nf!("gc_enable", 0, Some(0), gc_enable),
    nf!("gc_disable", 0, Some(0), gc_disable),
    nf!("gc_enabled", 0, Some(0), gc_enabled),
    nf!("gc_collect_cycles", 0, Some(0), gc_collect_cycles),
    nf!("gc_mem_caches", 0, Some(0), gc_mem_caches),
    nf!("gc_status", 0, Some(0), gc_status),
    nf!("zend_version", 0, Some(0), zend_version),
    nf!("extension_loaded", 1, Some(1), extension_loaded),
    nf!("get_loaded_extensions", 0, Some(1), get_loaded_extensions),
    nf!("get_extension_funcs", 1, Some(1), get_extension_funcs),
    nf!("connection_status", 0, Some(0), connection_status),
    nf!("connection_aborted", 0, Some(0), connection_status),
    nf!("ignore_user_abort", 0, Some(1), ignore_user_abort),
];

/// `INI_*`, `CONNECTION_*`, `INFO_*`, `CREDITS_*`.
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("INI_USER", 1),
        ("INI_PERDIR", 2),
        ("INI_SYSTEM", 4),
        ("INI_ALL", 7),
        ("INI_SCANNER_NORMAL", 0),
        ("INI_SCANNER_RAW", 1),
        ("INI_SCANNER_TYPED", 2),
        ("CONNECTION_NORMAL", 0),
        ("CONNECTION_ABORTED", 1),
        ("CONNECTION_TIMEOUT", 2),
        ("INFO_GENERAL", 1),
        ("INFO_CREDITS", 2),
        ("INFO_CONFIGURATION", 4),
        ("INFO_MODULES", 8),
        ("INFO_ENVIRONMENT", 16),
        ("INFO_VARIABLES", 32),
        ("INFO_LICENSE", 64),
        ("INFO_ALL", 4294967295),
        ("CREDITS_GROUP", 1),
        ("CREDITS_GENERAL", 2),
        ("CREDITS_SAPI", 4),
        ("CREDITS_MODULES", 8),
        ("CREDITS_DOCS", 16),
        ("CREDITS_FULLPAGE", 32),
        ("CREDITS_QA", 64),
        ("CREDITS_ALL", 4294967295),
    ] {
        r.constant(name, Value::Int(v));
    }
    r.interp().ini.register("ignore_user_abort", "0");
}

/// The version string every bundled extension reports (`phpversion($ext)`).
pub(crate) const PHP_VERSION: &str = "8.5.0";
/// `zend_version()` of the engine we track.
const ZEND_VERSION: &str = "4.5.0";

// ---- uname ------------------------------------------------------------------

/// `uname -s`, `-n`, `-r`, `-v`, `-m`, resolved once.
struct Uname {
    sysname: String,
    nodename: String,
    release: String,
    version: String,
    machine: String,
}

fn uname_field(flag: &str) -> Option<String> {
    let out = std::process::Command::new("uname").arg(flag).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            return h;
        }
    }
    uname_field("-n").unwrap_or_else(|| "localhost".to_string())
}

fn uname() -> &'static Uname {
    static UNAME: OnceLock<Uname> = OnceLock::new();
    UNAME.get_or_init(|| {
        let sysname = uname_field("-s").unwrap_or_else(|| {
            match std::env::consts::OS {
                "macos" => "Darwin",
                "linux" => "Linux",
                "windows" => "Windows NT",
                "freebsd" => "FreeBSD",
                other => other,
            }
            .to_string()
        });
        let machine = uname_field("-m").unwrap_or_else(|| std::env::consts::ARCH.to_string());
        Uname {
            sysname,
            nodename: hostname(),
            release: uname_field("-r")
                .or_else(|| std::fs::read_to_string("/proc/sys/kernel/osrelease").ok().map(|s| s.trim().to_string()))
                .unwrap_or_default(),
            version: uname_field("-v")
                .or_else(|| std::fs::read_to_string("/proc/sys/kernel/version").ok().map(|s| s.trim().to_string()))
                .unwrap_or_default(),
            machine,
        }
    })
}

/// `php_uname(string $mode = "a"): string`
pub(crate) fn php_uname(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mode = args.first().map(Value::to_php_bytes).unwrap_or_else(|| b"a".to_vec());
    let u = uname();
    let s = match mode.first() {
        Some(b's') => u.sysname.clone(),
        Some(b'n') => u.nodename.clone(),
        Some(b'r') => u.release.clone(),
        Some(b'v') => u.version.clone(),
        Some(b'm') => u.machine.clone(),
        Some(b'a') => format!("{} {} {} {} {}", u.sysname, u.nodename, u.release, u.version, u.machine),
        _ => {
            return Err(Unwind::value_error(
                "php_uname(): Argument #1 ($mode) must be one of \"a\", \"m\", \"n\", \"r\", \"s\", or \"v\"",
            ))
        }
    };
    Ok(Value::string(s.as_bytes()))
}

// ---- phpinfo / ini ----------------------------------------------------------

/// `phpinfo(int $flags = INFO_ALL): true` — the CLI text form, reduced to
/// the general block and the ini table (allowlisted `platform-value`).
pub(crate) fn phpinfo(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let flags = args.first().map_or(-1, Value::to_int);
    let mut out = Vec::new();
    if flags & 1 != 0 {
        let u = uname();
        out.extend_from_slice(b"phpinfo()\n");
        out.extend_from_slice(format!("PHP Version => {PHP_VERSION}\n\n").as_bytes());
        out.extend_from_slice(
            format!(
                "System => {} {} {} {} {}\nServer API => {}\nZend Engine => {}\nEngine => rphp\n\n",
                u.sysname,
                u.nodename,
                u.release,
                u.version,
                u.machine,
                ctx.sapi.name(),
                ZEND_VERSION
            )
            .as_bytes(),
        );
    }
    if flags & 4 != 0 {
        out.extend_from_slice(b"Configuration\n\n");
        let mut rows: Vec<(String, String, String)> = ctx
            .ini
            .iter()
            .map(|(k, e)| (k.to_string(), e.value.clone(), e.default.clone()))
            .collect();
        rows.sort();
        for (k, local, global) in rows {
            let show = |v: &str| if v.is_empty() { "no value".to_string() } else { v.to_string() };
            out.extend_from_slice(format!("{k} => {} => {}\n", show(&local), show(&global)).as_bytes());
        }
        out.push(b'\n');
    }
    ctx.out().extend_from_slice(&out);
    Ok(Value::Bool(true))
}

/// `php_ini_loaded_file(): string|false` — no php.ini is ever loaded.
pub(crate) fn php_ini_loaded_file(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

/// `php_ini_scanned_files(): string|false`
pub(crate) fn php_ini_scanned_files(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

/// `get_cfg_var(string $option): string|array|false` — the value from the
/// configuration file; there is none, so always `false` (as `php -n`).
pub(crate) fn get_cfg_var(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

/// The extension and access mask of the registered directives that are not
/// `Core` / `INI_ALL` (the manifest's `ini.json`).
const INI_META: &[(&str, &str, i64)] = &[
    ("allow_url_fopen", "Core", 4),
    ("allow_url_include", "Core", 4),
    ("arg_separator.input", "Core", 6),
    ("assert.active", "standard", 7),
    ("assert.exception", "standard", 7),
    ("auto_append_file", "Core", 6),
    ("auto_detect_line_endings", "standard", 7),
    ("auto_prepend_file", "Core", 6),
    ("date.timezone", "date", 7),
    ("default_socket_timeout", "standard", 7),
    ("disable_functions", "Core", 4),
    ("expose_php", "Core", 4),
    ("file_uploads", "Core", 4),
    ("max_file_uploads", "Core", 6),
    ("max_input_time", "Core", 6),
    ("max_input_vars", "Core", 6),
    ("output_buffering", "Core", 6),
    ("output_handler", "Core", 6),
    ("pcre.backtrack_limit", "pcre", 7),
    ("pcre.jit", "pcre", 7),
    ("pcre.recursion_limit", "pcre", 7),
    ("post_max_size", "Core", 6),
    ("register_argc_argv", "Core", 6),
    ("request_order", "Core", 6),
    ("short_open_tag", "Core", 6),
    ("sys_temp_dir", "Core", 4),
    ("unserialize_max_depth", "standard", 7),
    ("upload_max_filesize", "Core", 6),
    ("user_agent", "standard", 7),
    ("variables_order", "Core", 6),
    ("xmlrpc_errors", "Core", 4),
];

fn ini_meta(name: &str) -> (&'static str, i64) {
    INI_META
        .iter()
        .find(|(n, _, _)| *n == name)
        .map_or(("Core", 7), |(_, ext, access)| (ext, *access))
}

/// `ini_get_all(?string $extension = null, bool $details = true): array|false`
pub(crate) fn ini_get_all(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ext = match args.first() {
        Some(v) if !matches!(*v.deref(), Value::Null) => Some(v.to_php_string()),
        _ => None,
    };
    let details = args.get(1).is_none_or(Value::to_bool);
    if let Some(e) = &ext {
        if !EXTENSIONS.iter().any(|(n, _)| n.eq_ignore_ascii_case(e)) {
            ctx.warn(&format!("ini_get_all(): Extension \"{e}\" cannot be found"))?;
            return Ok(Value::Bool(false));
        }
    }
    let mut rows: Vec<(String, String, String)> = ctx
        .ini
        .iter()
        .filter(|(k, _)| ext.as_ref().is_none_or(|e| ini_meta(k).0.eq_ignore_ascii_case(e)))
        .map(|(k, e)| (k.to_string(), e.default.clone(), e.value.clone()))
        .collect();
    rows.sort();
    let mut out = Array::new();
    for (name, global, local) in rows {
        let v = if details {
            let mut d = Array::new();
            d.set(ArrayKey::str(b"global_value"), Value::string(global.as_bytes()));
            d.set(ArrayKey::str(b"local_value"), Value::string(local.as_bytes()));
            d.set(ArrayKey::str(b"access"), Value::Int(ini_meta(&name).1));
            Value::Array(d)
        } else {
            Value::string(local.as_bytes())
        };
        out.set(ArrayKey::str(name.as_bytes()), v);
    }
    Ok(Value::Array(out))
}

/// `get_include_path(): string|false`
pub(crate) fn get_include_path(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(ctx.ini_get("include_path").map_or(Value::Bool(false), |v| Value::string(v.as_bytes())))
}

/// `set_include_path(string $include_path): string|false` — the previous
/// value.
pub(crate) fn set_include_path(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let new = args[0].to_php_string();
    Ok(ctx.ini_set("include_path", &new).map_or(Value::Bool(false), |old| Value::string(old.as_bytes())))
}

// ---- environment ------------------------------------------------------------

/// `getenv(?string $name = null, bool $local_only = false): string|array|false`
pub(crate) fn getenv(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match args.first() {
        Some(v) if !matches!(*v.deref(), Value::Null) => {
            let name = v.to_php_bytes();
            let name = String::from_utf8_lossy(&name).into_owned();
            if name.is_empty() || name.contains(['=', '\0']) {
                return Ok(Value::Bool(false));
            }
            Ok(match std::env::var_os(&name) {
                Some(val) => Value::string(val.to_string_lossy().as_bytes()),
                None => Value::Bool(false),
            })
        }
        _ => {
            let mut a = Array::new();
            for (k, v) in std::env::vars_os() {
                a.set(
                    ArrayKey::str(k.to_string_lossy().as_bytes()),
                    Value::string(v.to_string_lossy().as_bytes()),
                );
            }
            Ok(Value::Array(a))
        }
    }
}

/// `putenv(string $assignment): bool` — `NAME=value` sets, a bare `NAME`
/// unsets; the change is visible to `getenv` and to child processes.
pub(crate) fn putenv(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let assignment = args[0].to_php_bytes();
    let text = String::from_utf8_lossy(&assignment).into_owned();
    let (name, value) = match text.split_once('=') {
        Some((n, v)) => (n.to_string(), Some(v.to_string())),
        None => (text.clone(), None),
    };
    if name.is_empty() || name.contains('\0') || value.as_ref().is_some_and(|v| v.contains('\0')) {
        return Err(Unwind::value_error("putenv(): Argument #1 ($assignment) must have a valid syntax"));
    }
    match value {
        Some(v) => std::env::set_var(&name, v),
        None => std::env::remove_var(&name),
    }
    Ok(Value::Bool(true))
}

/// `sys_get_temp_dir(): string` — the `sys_temp_dir` ini, else `TMPDIR`,
/// else `/tmp`; without a trailing slash.
pub(crate) fn sys_get_temp_dir(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut dir = match ctx.ini_get("sys_temp_dir") {
        Some(d) if !d.is_empty() => d.to_string(),
        _ => std::env::var("TMPDIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".to_string()),
    };
    while dir.len() > 1 && dir.ends_with('/') {
        dir.pop();
    }
    Ok(Value::string(dir.as_bytes()))
}

// ---- process / script --------------------------------------------------------

/// `getmypid(): int|false`
pub(crate) fn getmypid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(std::process::id() as i64))
}

/// The main script's metadata (`stat` of `$_SERVER['SCRIPT_FILENAME']`).
fn script_meta(ctx: &Ctx) -> Option<std::fs::Metadata> {
    std::fs::metadata(ctx.script_path.as_ref()?).ok()
}

/// `getmyuid(): int|false` — the script owner's uid.
pub(crate) fn getmyuid(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    use std::os::unix::fs::MetadataExt;
    Ok(script_meta(ctx).map_or(Value::Bool(false), |m| Value::Int(m.uid() as i64)))
}

/// `getmygid(): int|false` — the script owner's gid.
pub(crate) fn getmygid(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    use std::os::unix::fs::MetadataExt;
    Ok(script_meta(ctx).map_or(Value::Bool(false), |m| Value::Int(m.gid() as i64)))
}

/// `getmyinode(): int|false`
pub(crate) fn getmyinode(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    use std::os::unix::fs::MetadataExt;
    Ok(script_meta(ctx).map_or(Value::Bool(false), |m| Value::Int(m.ino() as i64)))
}

/// `getlastmod(): int|false` — the script's mtime as a Unix timestamp.
pub(crate) fn getlastmod(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    use std::os::unix::fs::MetadataExt;
    Ok(script_meta(ctx).map_or(Value::Bool(false), |m| Value::Int(m.mtime())))
}

/// `get_current_user(): string` — the script owner's login name
/// (`getpwuid`): looked up in `/etc/passwd`, then through `id -un <uid>`
/// (directory services on macOS), else the `USER` / `LOGNAME` environment.
pub(crate) fn get_current_user(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    use std::os::unix::fs::MetadataExt;
    if let Some(uid) = script_meta(ctx).map(|m| m.uid()) {
        if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
            for line in passwd.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() > 2 && fields[2].parse::<u32>().ok() == Some(uid) {
                    return Ok(Value::string(fields[0].as_bytes()));
                }
            }
        }
        if let Ok(out) = std::process::Command::new("id").args(["-un", &uid.to_string()]).output() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !name.is_empty() {
                return Ok(Value::string(name.as_bytes()));
            }
        }
    }
    let name = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default();
    Ok(Value::string(name.as_bytes()))
}

// ---- gc ---------------------------------------------------------------------

/// `gc_enable(): void` — the safe-`Rc` heap has no cycle collector; the
/// flag is kept in `zend.enable_gc` so `gc_enabled()` reflects it.
pub(crate) fn gc_enable(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.ini_set("zend.enable_gc", "1");
    Ok(Value::Null)
}

/// `gc_disable(): void`
pub(crate) fn gc_disable(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.ini_set("zend.enable_gc", "0");
    Ok(Value::Null)
}

/// `gc_enabled(): bool`
pub(crate) fn gc_enabled(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(ctx.ini.bool("zend.enable_gc")))
}

thread_local! {
    /// Explicit `gc_collect_cycles()` calls, reported as `gc_status()["runs"]`.
    static GC_RUNS: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

/// `gc_collect_cycles(): int` — nothing to collect: 0 (the call still
/// counts as a run).
pub(crate) fn gc_collect_cycles(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    GC_RUNS.with(|r| r.set(r.get() + 1));
    Ok(Value::Int(0))
}

/// `gc_mem_caches(): int` — nothing to release: 0.
pub(crate) fn gc_mem_caches(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(0))
}

/// `gc_status(): array` — php's shape with an idle collector's figures.
pub(crate) fn gc_status(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    for (k, v) in [
        ("running", Value::Bool(false)),
        ("protected", Value::Bool(false)),
        ("full", Value::Bool(false)),
        ("runs", Value::Int(GC_RUNS.with(std::cell::Cell::get))),
        ("collected", Value::Int(0)),
        ("threshold", Value::Int(10001)),
        ("buffer_size", Value::Int(16384)),
        ("roots", Value::Int(0)),
        ("application_time", Value::Float(0.0)),
        ("collector_time", Value::Float(0.0)),
        ("destructor_time", Value::Float(0.0)),
        ("free_time", Value::Float(0.0)),
    ] {
        a.set(ArrayKey::str(k.as_bytes()), v);
    }
    Ok(Value::Array(a))
}

/// `zend_version(): string`
pub(crate) fn zend_version(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(ZEND_VERSION.as_bytes()))
}

/// Which extension declares an internal class, as php's own
/// `ReflectionClass::getExtensionName()` answers it. The rows were measured
/// against php 8.5 for every class this engine declares; a class that is
/// not listed belongs to `Core`, which is where php puts everything the
/// engine itself declares.
pub(crate) fn extension_of_class(name: &[u8]) -> &'static str {
    const BY_EXTENSION: &[(&str, &[&str])] = &[
        (
            "standard",
            &["__PHP_Incomplete_Class", "AssertionError"],
        ),
        ("json", &["JsonException", "JsonSerializable"]),
        ("hash", &["HashContext"]),
        (
            "session",
            &[
                "SessionHandler",
                "SessionHandlerInterface",
                "SessionIdInterface",
                "SessionUpdateTimestampHandlerInterface",
            ],
        ),
        (
            "SPL",
            &[
                "LogicException", "BadFunctionCallException", "BadMethodCallException",
                "DomainException", "InvalidArgumentException", "LengthException",
                "OutOfRangeException", "RuntimeException", "OutOfBoundsException",
                "OverflowException", "RangeException", "UnderflowException",
                "UnexpectedValueException", "ArrayIterator", "ArrayObject",
                "SplObjectStorage", "SplDoublyLinkedList", "SplStack", "SplQueue",
                "SplFixedArray", "SplHeap", "SplMinHeap", "SplMaxHeap",
                "SplPriorityQueue", "SplSubject", "SplObserver", "IteratorIterator",
                "FilterIterator", "CallbackFilterIterator", "RecursiveFilterIterator",
                "RecursiveCallbackFilterIterator", "ParentIterator", "LimitIterator",
                "CachingIterator", "RecursiveCachingIterator", "NoRewindIterator",
                "InfiniteIterator", "EmptyIterator", "AppendIterator", "RegexIterator",
                "RecursiveRegexIterator", "RecursiveIteratorIterator",
                "RecursiveArrayIterator", "MultipleIterator", "SplFileInfo",
                "DirectoryIterator", "FilesystemIterator", "RecursiveDirectoryIterator",
                "GlobIterator", "SplFileObject", "SplTempFileObject", "SeekableIterator",
                "OuterIterator", "RecursiveIterator",
            ],
        ),
    ];
    for (ext, names) in BY_EXTENSION {
        if names.iter().any(|n| n.as_bytes().eq_ignore_ascii_case(name)) {
            return ext;
        }
    }
    // The three families php names after their extension, whole.
    for (prefix, ext) in [
        (&b"Reflect"[..], "Reflection"),
        (b"Date", "date"),
        (b"Random\\", "random"),
    ] {
        if name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return ext;
        }
    }
    "Core"
}

// ---- extensions -------------------------------------------------------------

/// The bundled extensions in php's module order, with the registry slices
/// each one contributes. `Core` and `standard` share `basic_functions.rs` /
/// `errorfunc.rs` / `info.rs`; [`CORE_FUNCTIONS`] tells them apart.
const EXTENSIONS: &[(&str, &[&[NativeFn]])] = &[
    ("Core", &[crate::basic_functions::FUNCTIONS, crate::errorfunc::FUNCTIONS, FUNCTIONS]),
    ("date", &[crate::date::FUNCTIONS, crate::date::CLASS_FUNCTIONS]),
    ("pcre", &[crate::pcre::FUNCTIONS]),
    ("ctype", &[crate::ctype::FUNCTIONS]),
    ("json", &[crate::json::FUNCTIONS]),
    ("mbstring", &[crate::mbstring::FUNCTIONS]),
    (
        "SPL",
        &[
            crate::spl_autoload::FUNCTIONS,
            crate::spl_containers2::FUNCTIONS,
            crate::spl_decorators::FUNCTIONS,
            crate::spl_directory::FUNCTIONS,
            crate::spl_fixedarray::FUNCTIONS,
            crate::spl_heaps::FUNCTIONS,
            crate::spl_iterators::FUNCTIONS,
        ],
    ),
    ("filter", &[crate::filter::FUNCTIONS]),
    ("hash", &[crate::hash::FUNCTIONS]),
    ("iconv", &[crate::iconv::FUNCTIONS]),
    ("session", &[crate::session::FUNCTIONS]),
    (
        "standard",
        &[
            crate::output::FUNCTIONS,
            crate::types::FUNCTIONS,
            crate::strings::FUNCTIONS,
            crate::arrays::FUNCTIONS,
            crate::math::FUNCTIONS,
            crate::funcs::FUNCTIONS,
            crate::output_buffering::FUNCTIONS,
            crate::basic_functions::FUNCTIONS,
            crate::errorfunc::FUNCTIONS,
            crate::string2::FUNCTIONS,
            crate::array2::FUNCTIONS,
            crate::var::FUNCTIONS,
            crate::url::FUNCTIONS,
            FUNCTIONS,
            crate::versioning::FUNCTIONS,
            crate::formatted_print::FUNCTIONS,
            crate::html::FUNCTIONS,
            crate::base64::FUNCTIONS,
            crate::uniqid::FUNCTIONS,
            crate::random::FUNCTIONS,
            crate::net::FUNCTIONS,
            crate::file::FUNCTIONS,
            crate::file2::FUNCTIONS,
            crate::filestat::FUNCTIONS,
            crate::dir::FUNCTIONS,
            crate::exec::FUNCTIONS,
            crate::head::FUNCTIONS,
            crate::password::FUNCTIONS,
        ],
    ),
    ("random", &[crate::random::FUNCTIONS]),
    ("Reflection", &[crate::reflection::FUNCTIONS]),
    ("tokenizer", &[crate::tokenizer::FUNCTIONS]),
];

/// Functions php attributes to `Core` (`Zend/zend_builtin_functions.c`,
/// the manifest's `extension == "Core"`), whichever module implements them.
const CORE_FUNCTIONS: &[&str] = &[
    "class_alias", "class_exists", "clone", "debug_backtrace", "debug_print_backtrace", "define",
    "defined", "die", "enum_exists", "error_reporting", "exit", "extension_loaded", "func_get_arg",
    "func_get_args", "func_num_args", "function_exists", "gc_collect_cycles", "gc_disable",
    "gc_enable", "gc_enabled", "gc_mem_caches", "gc_status", "get_called_class", "get_class",
    "get_class_methods", "get_class_vars", "get_declared_classes", "get_declared_interfaces",
    "get_declared_traits", "get_defined_constants", "get_defined_functions", "get_defined_vars",
    "get_error_handler", "get_exception_handler", "get_extension_funcs", "get_included_files",
    "get_loaded_extensions", "get_mangled_object_vars", "get_object_vars", "get_parent_class",
    "get_required_files", "get_resource_id", "get_resource_type", "get_resources",
    "interface_exists", "is_a", "is_subclass_of", "method_exists", "property_exists",
    "restore_error_handler", "restore_exception_handler", "set_error_handler",
    "set_exception_handler", "strcasecmp", "strcmp", "strlen", "strncasecmp", "strncmp",
    "trait_exists", "trigger_error", "user_error", "zend_version",
];

/// Functions of `ext/random` that live in `random.rs` beside the three
/// `standard` ones (`shuffle`, `str_shuffle`, `array_rand`).
const RANDOM_FUNCTIONS: &[&str] = &[
    "mt_srand",
    "srand",
    "mt_rand",
    "rand",
    "mt_getrandmax",
    "getrandmax",
    "random_int",
    "random_bytes",
    "lcg_value",
];

/// Which of the bundled extensions `name` belongs to.
pub(crate) fn extension_of(name: &str) -> &'static str {
    if CORE_FUNCTIONS.iter().any(|f| f.eq_ignore_ascii_case(name)) {
        return "Core";
    }
    if RANDOM_FUNCTIONS.iter().any(|f| f.eq_ignore_ascii_case(name)) {
        return "random";
    }
    for (ext, slices) in EXTENSIONS {
        // Core / standard / random share modules; the name lists above
        // settle those.
        if *ext == "Core" || *ext == "standard" || *ext == "random" {
            continue;
        }
        if slices.iter().any(|s| s.iter().any(|f| f.name.eq_ignore_ascii_case(name))) {
            return ext;
        }
    }
    "standard"
}

/// `extension_loaded(string $extension): bool`
pub(crate) fn extension_loaded(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_string();
    Ok(Value::Bool(
        EXTENSIONS.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name))
            || ctx.extensions.iter().any(|n| n.eq_ignore_ascii_case(&name)),
    ))
}

/// `get_loaded_extensions(bool $zend_extensions = false): array`
pub(crate) fn get_loaded_extensions(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    if !args.first().is_some_and(Value::to_bool) {
        for (n, _) in EXTENSIONS {
            a.push(Value::string(n.as_bytes()));
        }
        for n in &ctx.extensions {
            a.push(Value::string(n.as_bytes()));
        }
    }
    Ok(Value::Array(a))
}

/// `get_extension_funcs(string $extension): array|false`
pub(crate) fn get_extension_funcs(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_string();
    let Some((ext, _)) = EXTENSIONS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) else {
        return Ok(Value::Bool(false));
    };
    let mut a = Array::new();
    for f in ctx.natives() {
        if extension_of(f.name) == *ext {
            a.push(Value::string(f.name.to_ascii_lowercase().as_bytes()));
        }
    }
    Ok(Value::Array(a))
}

/// `phpversion($extension)` support: the version of a bundled extension.
pub(crate) fn extension_version(name: &str) -> Option<&'static str> {
    EXTENSIONS.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)).then_some(PHP_VERSION)
}

// ---- connection -------------------------------------------------------------

/// `connection_status(): int` / `connection_aborted(): int` — the CLI never
/// loses its client: `CONNECTION_NORMAL` / 0.
pub(crate) fn connection_status(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(0))
}

/// `ignore_user_abort(?bool $enable = null): int` — the previous setting.
pub(crate) fn ignore_user_abort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let old = ctx.ini.int("ignore_user_abort");
    if let Some(v) = args.first() {
        if !matches!(*v.deref(), Value::Null) {
            let new = if v.to_bool() { "1" } else { "0" };
            ctx.ini_set("ignore_user_abort", new);
        }
    }
    Ok(Value::Int(old))
}

#[cfg(test)]
mod tests {
    use rphp_runtime::ErrorKind;
    use rphp_value::{ArrayKey, Value};

    use crate::tests::{call_err, call_named, interp};

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    #[test]
    fn uname_modes_and_versions() {
        assert!(!call_named(b"php_uname", &[s("s")]).to_php_bytes().is_empty());
        assert_eq!(call_err(b"php_uname", &[s("x")]).kind(), Some(ErrorKind::ValueError));
        assert_eq!(call_named(b"zend_version", &[]), s("4.5.0"));
        assert_eq!(call_named(b"extension_loaded", &[s("STANDARD")]), Value::Bool(true));
        assert_eq!(call_named(b"extension_loaded", &[s("nope")]), Value::Bool(false));
        assert_eq!(call_named(b"get_extension_funcs", &[s("nope")]), Value::Bool(false));
        let Value::Array(funcs) = call_named(b"get_extension_funcs", &[s("ctype")]) else { panic!() };
        assert!(funcs.values().any(|v| v.to_php_bytes() == b"ctype_alpha"));
        let Value::Array(core) = call_named(b"get_extension_funcs", &[s("Core")]) else { panic!() };
        assert!(core.values().any(|v| v.to_php_bytes() == b"zend_version"));
        // `strlen` is Core in php (Zend/zend_builtin_functions.c), `strrev` is standard.
        assert!(core.values().any(|v| v.to_php_bytes() == b"strlen"));
        assert!(!core.values().any(|v| v.to_php_bytes() == b"strrev"));
    }

    #[test]
    fn env_and_ini_queries() {
        let mut it = interp();
        assert_eq!(it.call_function(b"putenv", &[s("RPHP_INFO_TEST=bar")]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_function(b"getenv", &[s("RPHP_INFO_TEST")]).unwrap(), s("bar"));
        assert_eq!(it.call_function(b"putenv", &[s("RPHP_INFO_TEST")]).unwrap(), Value::Bool(true));
        assert_eq!(it.call_function(b"getenv", &[s("RPHP_INFO_TEST")]).unwrap(), Value::Bool(false));
        assert_eq!(it.call_function(b"putenv", &[s("=x")]).unwrap_err().kind(), Some(ErrorKind::ValueError));
        assert!(matches!(it.call_function(b"getenv", &[]).unwrap(), Value::Array(_)));
        let Value::Array(all) = it.call_function(b"ini_get_all", &[s("pcre")]).unwrap() else { panic!() };
        let keys: Vec<Vec<u8>> = all.keys().map(|k| k.to_value().to_php_bytes()).collect();
        assert_eq!(keys, [&b"pcre.backtrack_limit"[..], b"pcre.jit", b"pcre.recursion_limit"]);
        let Value::Array(jit) = all.get_deref(&ArrayKey::str(b"pcre.jit")).unwrap() else { panic!() };
        assert_eq!(jit.get_deref(&ArrayKey::str(b"access")).unwrap(), Value::Int(7));
        assert_eq!(it.call_function(b"ini_get_all", &[s("nope")]).unwrap(), Value::Bool(false));
        assert_eq!(it.call_function(b"set_include_path", &[s("/x")]).unwrap(), s(".:/usr/share/php"));
        assert_eq!(it.call_function(b"get_include_path", &[]).unwrap(), s("/x"));
        assert_eq!(it.call_function(b"ignore_user_abort", &[Value::Bool(true)]).unwrap(), Value::Int(0));
        assert_eq!(it.call_function(b"ignore_user_abort", &[]).unwrap(), Value::Int(1));
        it.call_function(b"gc_disable", &[]).unwrap();
        assert_eq!(it.call_function(b"gc_enabled", &[]).unwrap(), Value::Bool(false));
        assert!(!it.call_function(b"sys_get_temp_dir", &[]).unwrap().to_php_bytes().ends_with(b"/"));
    }
}
