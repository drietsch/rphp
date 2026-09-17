//! Engine constants (`Zend/zend_constants.c`, `main/main.c`): the `PHP_*`,
//! `E_*`, `DIRECTORY_SEPARATOR` … set, with the values `manifest/php-8.5.0/
//! constants.json` records for stock PHP 8.5.0 (platform values from the
//! build target). Not reachable as bare names from PHP code until the
//! compiler lowers constant fetches (F3/E3); `constant()`/`defined()` see them.

use rphp_runtime::{Registry, SapiKind};
use rphp_value::Value;

/// `PHP_OS` for the build target (php's `uname -s` at build time).
pub fn php_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "WINNT"
    } else if cfg!(target_os = "freebsd") {
        "FreeBSD"
    } else if cfg!(target_os = "openbsd") {
        "OpenBSD"
    } else if cfg!(target_os = "netbsd") {
        "NetBSD"
    } else if cfg!(target_os = "solaris") || cfg!(target_os = "illumos") {
        "SunOS"
    } else {
        "Unknown"
    }
}

/// `PHP_OS_FAMILY`.
pub fn php_os_family() -> &'static str {
    if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )) {
        "BSD"
    } else if cfg!(target_os = "solaris") || cfg!(target_os = "illumos") {
        "Solaris"
    } else {
        "Unknown"
    }
}

/// Register the engine constants into `r`.
pub fn register(r: &mut Registry, sapi: SapiKind) {
    let s = |v: &str| Value::string(v.as_bytes());
    let windows = cfg!(target_os = "windows");

    // Version.
    r.constant("PHP_VERSION", s("8.5.0"));
    r.constant("PHP_MAJOR_VERSION", Value::Int(8));
    r.constant("PHP_MINOR_VERSION", Value::Int(5));
    r.constant("PHP_RELEASE_VERSION", Value::Int(0));
    r.constant("PHP_EXTRA_VERSION", s(""));
    r.constant("PHP_VERSION_ID", Value::Int(80500));
    r.constant("PHP_ZTS", Value::Bool(false));
    r.constant("PHP_DEBUG", Value::Bool(false));
    r.constant("ZEND_THREAD_SAFE", Value::Bool(false));
    r.constant("ZEND_DEBUG_BUILD", Value::Bool(false));

    // Platform.
    r.constant("PHP_OS", s(php_os()));
    r.constant("PHP_OS_FAMILY", s(php_os_family()));
    r.constant("PHP_EOL", s("\n"));
    r.constant("DIRECTORY_SEPARATOR", s(if windows { "\\" } else { "/" }));
    r.constant("PATH_SEPARATOR", s(if windows { ";" } else { ":" }));
    r.constant("PHP_SHLIB_SUFFIX", s(if windows { "dll" } else { "so" }));
    r.constant(
        "PHP_MAXPATHLEN",
        Value::Int(if cfg!(target_os = "linux") {
            4096
        } else {
            1024
        }),
    );
    r.constant("PHP_FD_SETSIZE", Value::Int(1024));
    r.constant("PHP_SAPI", s(sapi.name()));
    let binary = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    r.constant("PHP_BINARY", s(&binary));

    // Integer / float limits.
    r.constant("PHP_INT_MAX", Value::Int(i64::MAX));
    r.constant("PHP_INT_MIN", Value::Int(i64::MIN));
    r.constant("PHP_INT_SIZE", Value::Int(8));
    r.constant("PHP_FLOAT_DIG", Value::Int(15));
    r.constant("PHP_FLOAT_EPSILON", Value::Float(f64::EPSILON));
    r.constant("PHP_FLOAT_MIN", Value::Float(f64::MIN_POSITIVE));
    r.constant("PHP_FLOAT_MAX", Value::Float(f64::MAX));

    // Error levels (E_STRICT is still defined in 8.5, deprecated).
    for (name, v) in [
        ("E_ERROR", rphp_runtime::E_ERROR),
        ("E_WARNING", rphp_runtime::E_WARNING),
        ("E_PARSE", rphp_runtime::E_PARSE),
        ("E_NOTICE", rphp_runtime::E_NOTICE),
        ("E_CORE_ERROR", rphp_runtime::E_CORE_ERROR),
        ("E_CORE_WARNING", rphp_runtime::E_CORE_WARNING),
        ("E_COMPILE_ERROR", rphp_runtime::E_COMPILE_ERROR),
        ("E_COMPILE_WARNING", rphp_runtime::E_COMPILE_WARNING),
        ("E_USER_ERROR", rphp_runtime::E_USER_ERROR),
        ("E_USER_WARNING", rphp_runtime::E_USER_WARNING),
        ("E_USER_NOTICE", rphp_runtime::E_USER_NOTICE),
        ("E_STRICT", rphp_runtime::E_STRICT),
        ("E_RECOVERABLE_ERROR", rphp_runtime::E_RECOVERABLE_ERROR),
        ("E_DEPRECATED", rphp_runtime::E_DEPRECATED),
        ("E_USER_DEPRECATED", rphp_runtime::E_USER_DEPRECATED),
        ("E_ALL", rphp_runtime::E_ALL),
        ("DEBUG_BACKTRACE_PROVIDE_OBJECT", 1),
        ("DEBUG_BACKTRACE_IGNORE_ARGS", 2),
        ("UPLOAD_ERR_OK", 0),
        ("UPLOAD_ERR_INI_SIZE", 1),
        ("UPLOAD_ERR_FORM_SIZE", 2),
        ("UPLOAD_ERR_PARTIAL", 3),
        ("UPLOAD_ERR_NO_FILE", 4),
        ("UPLOAD_ERR_NO_TMP_DIR", 6),
        ("UPLOAD_ERR_CANT_WRITE", 7),
        ("UPLOAD_ERR_EXTENSION", 8),
    ] {
        r.constant(name, Value::Int(v));
    }

    // The language literals are constants too (`constant('TRUE')`).
    r.constant("TRUE", Value::Bool(true));
    r.constant("FALSE", Value::Bool(false));
    r.constant("NULL", Value::Null);
    r.constant("PHP_CLI_PROCESS_TITLE", Value::Bool(sapi == SapiKind::Cli));
}
