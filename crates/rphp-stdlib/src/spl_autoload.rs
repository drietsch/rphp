//! The autoloader stack (php-src `ext/spl/php_spl.c`), plan E7.
//!
//! The engine owns the stack and the lookup path (`rphp-runtime`'s
//! `autoload.rs`); this module is the php-facing surface. Registering the
//! same callable twice is a no-op, which is what lets a package's bootstrap
//! run more than once safely — Composer's generated `autoload_real.php`
//! depends on it.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("spl_autoload_register", 0, Some(3), spl_autoload_register),
    nf!("spl_autoload_unregister", 1, Some(1), spl_autoload_unregister),
    nf!("spl_autoload_functions", 0, Some(0), spl_autoload_functions),
    nf!("spl_autoload_call", 1, Some(1), spl_autoload_call),
    nf!("class_alias", 2, Some(3), class_alias),
    nf!("spl_autoload", 1, Some(2), spl_autoload),
    nf!("spl_autoload_extensions", 0, Some(1), spl_autoload_extensions),
    nf!("spl_classes", 0, Some(0), spl_classes),
];

/// The extensions `spl_autoload()` tries by default.
const DEFAULT_EXTENSIONS: &str = ".inc,.php";

thread_local! {
    /// What `spl_autoload_extensions()` last set (`None`: the default).
    static EXTENSIONS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Forget `spl_autoload_extensions()`'s setting at the end of a request.
pub(crate) fn request_shutdown() {
    EXTENSIONS.with(|e| *e.borrow_mut() = None);
}

/// `spl_autoload_register(?callable $callback = null, bool $throw = true, bool $prepend = false): bool`
fn spl_autoload_register(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if args.get(1).is_some_and(|v| !v.to_bool()) {
        ctx.notice(
            "spl_autoload_register(): Argument #2 ($do_throw) has been ignored, spl_autoload_register() will always throw",
        )?;
    }
    let mut callback = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    // No callback registers php's default loader, `spl_autoload`.
    if matches!(callback, Value::Null) {
        callback = Value::string(b"spl_autoload");
    }
    if !ctx.is_callable(&callback) {
        return Err(Unwind::type_error(
            "spl_autoload_register(): Argument #1 ($callback) must be a valid callback",
        ));
    }
    let prepend = args.get(2).is_some_and(Value::to_bool);
    if !ctx.autoloader_registered(&callback) {
        ctx.autoloader_add(callback, prepend);
    }
    Ok(Value::Bool(true))
}

/// `spl_autoload_unregister(callable $callback): bool`
fn spl_autoload_unregister(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(ctx.autoloader_remove(&args[0])))
}

/// `spl_autoload_functions(): array` — the stack in call order.
fn spl_autoload_functions(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for f in ctx.autoloaders() {
        out.push(f);
    }
    Ok(Value::Array(out))
}

/// `spl_autoload_call(string $class): void` — run the stack for `$class`,
/// whether or not it is already declared.
fn spl_autoload_call(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    ctx.autoload_call(&name)?;
    Ok(Value::Null)
}

/// `class_alias(string $class, string $alias, bool $autoload = true): bool`
fn class_alias(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let original = args[0].to_php_bytes();
    let alias = args[1].to_php_bytes();
    let autoload = args.get(2).is_none_or(Value::to_bool);
    let id = if autoload {
        ctx.lookup_class(&original)?
    } else {
        ctx.class_by_name(&original)
    };
    let Some(id) = id else {
        return Err(Unwind::error(format!(
            "Class \"{}\" not found",
            String::from_utf8_lossy(&original)
        )));
    };
    if ctx.class_by_name(&alias).is_some() {
        ctx.warn(&format!(
            "Cannot declare class {}, because the name is already in use",
            String::from_utf8_lossy(&alias)
        ))?;
        return Ok(Value::Bool(false));
    }
    ctx.alias_class(&alias, id);
    Ok(Value::Bool(true))
}

/// `spl_autoload(string $class, ?string $file_extensions = null): void` —
/// php's default loader: the lowercased class name (namespace separators as
/// directory separators) plus each extension in turn, looked up on the
/// include path and included once; it stops at the first file that
/// declared the class.
fn spl_autoload(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let class = args[0].to_php_bytes();
    let exts = match args.get(1).map(|v| v.deref().into_owned()) {
        Some(Value::Null) | None => current_extensions(),
        Some(v) => String::from_utf8_lossy(&v.to_php_bytes()).into_owned(),
    };
    // php refuses a name that could not be a class (`../x`, a NUL byte)
    // before it touches the filesystem.
    let valid = !class.is_empty()
        && class.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'\\' || b >= 0x80);
    if !valid {
        return Ok(Value::Null);
    }
    let base: Vec<u8> = class
        .iter()
        .map(|&b| if b == b'\\' { b'/' } else { b.to_ascii_lowercase() })
        .collect();
    for ext in exts.split(',') {
        let mut file = base.clone();
        file.extend_from_slice(ext.as_bytes());
        let Some(path) = ctx.resolve_include_path(&file) else {
            continue;
        };
        if ctx.include_once_from_native(&path)? && ctx.class_by_name(&class).is_some() {
            break;
        }
    }
    Ok(Value::Null)
}

fn current_extensions() -> String {
    EXTENSIONS.with(|e| e.borrow().clone()).unwrap_or_else(|| DEFAULT_EXTENSIONS.to_string())
}

/// `spl_autoload_extensions(?string $file_extensions = null): string`
fn spl_autoload_extensions(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if let Some(v) = args.first().map(|v| v.deref().into_owned()) {
        if !matches!(v, Value::Null) {
            let s = String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
            EXTENSIONS.with(|e| *e.borrow_mut() = Some(s));
        }
    }
    Ok(Value::string(current_extensions().as_bytes()))
}

/// The classes and interfaces ext/spl declares, in php's order.
const SPL_CLASSES: &[&str] = &[
    "AppendIterator", "ArrayIterator", "ArrayObject", "BadFunctionCallException",
    "BadMethodCallException", "CachingIterator", "CallbackFilterIterator", "DirectoryIterator",
    "DomainException", "EmptyIterator", "FilesystemIterator", "FilterIterator", "GlobIterator",
    "InfiniteIterator", "InvalidArgumentException", "IteratorIterator", "LengthException",
    "LimitIterator", "LogicException", "MultipleIterator", "NoRewindIterator", "OuterIterator",
    "OutOfBoundsException", "OutOfRangeException", "OverflowException", "ParentIterator",
    "RangeException", "RecursiveArrayIterator", "RecursiveCachingIterator",
    "RecursiveCallbackFilterIterator", "RecursiveDirectoryIterator", "RecursiveFilterIterator",
    "RecursiveIterator", "RecursiveIteratorIterator", "RecursiveRegexIterator",
    "RecursiveTreeIterator", "RegexIterator", "RuntimeException", "SeekableIterator",
    "SplDoublyLinkedList", "SplFileInfo", "SplFileObject", "SplFixedArray", "SplHeap",
    "SplMinHeap", "SplMaxHeap", "SplObjectStorage", "SplObserver", "SplPriorityQueue",
    "SplQueue", "SplStack", "SplSubject", "SplTempFileObject", "UnderflowException",
    "UnexpectedValueException",
];

/// `spl_classes(): array` — name => name.
fn spl_classes(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for name in SPL_CLASSES {
        out.set(rphp_value::ArrayKey::str(name.as_bytes()), Value::string(name.as_bytes()));
    }
    Ok(Value::Array(out))
}
