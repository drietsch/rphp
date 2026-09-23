//! The embedding API every SAPI sits on (spec 09, ADR-014/033): wires the
//! front end (`rphp-parser`'s mago adapter / `rphp-compiler`), the engine
//! (`rphp-runtime`) and the extension bundle (`rphp-stdlib`) into an
//! [`Engine`] (per process: configuration, ini defaults) that produces
//! [`Interp`]s (per request/script) and runs them with php's request
//! lifecycle: `{main}` → shutdown functions → output flush, rendering an
//! uncaught fault like php-cli and returning the exit code.
//!
//! `rphp-sapi-cli` (and later SAPIs) depend on this crate only.
#![forbid(unsafe_code)]

mod constants;
mod sink;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rphp_bytecode::Module;
use rphp_compiler::{compile, CompileOptions};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_runtime::CompileFailure;
use rphp_source::SourceMap;
use rphp_value::{Array, ArrayKey, PhpRef, Value};

pub mod cgi;

pub use constants::{php_os, php_os_family};
pub use rphp_runtime::{Interp, OutputSink, Registry, SapiKind, Unwind};
/// A host's PDO drivers: register one per DSN scheme (`pdo::register_host_driver`).
pub mod pdo {
    pub use rphp_ext_pdo::driver::{Column, DbError, Driver, ParamKey, ResultSet, SqlValue};
    pub use rphp_ext_pdo::{register_host_driver, DriverFactory, HostDsn};
}
pub use sink::{BufferSink, StdoutSink};
/// A host's say over what scripts may spawn (`proc_open`, `exec`, …): a
/// [`spawn::SpawnPolicy`] installed with [`spawn::set_policy`].
pub mod spawn {
    pub use rphp_stdlib::{SpawnPolicy, SpawnRequest};

    /// Install `policy` on an interpreter; every spawn asks it first. With
    /// none installed, nothing is refused.
    pub fn set_policy(it: &mut rphp_runtime::Interp, policy: SpawnPolicy) {
        it.ext.slots.insert(rphp_stdlib::SPAWN_POLICY_SLOT, Box::new(policy));
    }
}

/// Why [`Engine::compile`] failed.
#[derive(Clone, Debug)]
pub enum CompileError {
    /// The front end rejected the source. php reports the first error as a
    /// fatal `Parse error` (exit 255) through the error display channel;
    /// `rendered` holds every diagnostic in the tool form.
    Parse {
        /// The first error's message (`syntax error, unexpected token ";"`).
        message: String,
        /// The 1-based line of the first error.
        line: u32,
        /// Every diagnostic, rendered one per element.
        rendered: Vec<String>,
    },
    /// The program parsed but resolution/validation (`rphp-hir`) rejected
    /// it with one of php's compile-time fatals (`Cannot use X as Y because
    /// the name is already in use`, `Cannot mix bracketed namespace
    /// declarations …`, an invalid `break` level, …): php reports the first
    /// one as `Fatal error: <message> in <file> on line <N>` (exit 255).
    Fatal {
        /// The first error's message, php's text.
        message: String,
        /// The 1-based line of the first error.
        line: u32,
        /// Every diagnostic, rendered one per element.
        rendered: Vec<String>,
    },
    /// The program parsed but the compiler rejected it (unsupported
    /// construct, undefined function, wrong arity, ...): the rendered
    /// diagnostics, one per element.
    Compile(Vec<String>),
}

impl CompileError {
    /// The rendered diagnostics, whichever stage failed.
    pub fn rendered(&self) -> &[String] {
        match self {
            CompileError::Parse { rendered, .. }
            | CompileError::Fatal { rendered, .. }
            | CompileError::Compile(rendered) => rendered,
        }
    }

    /// The rendered diagnostics, whichever stage failed.
    pub fn into_rendered(self) -> Vec<String> {
        match self {
            CompileError::Parse { rendered, .. }
            | CompileError::Fatal { rendered, .. }
            | CompileError::Compile(rendered) => rendered,
        }
    }
}

/// How an [`Engine`] is set up.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// The SAPI creating the engine (`PHP_SAPI`).
    pub sapi: SapiKind,
    /// The script's `$argv` (`argv[0]` = the script path as given, or
    /// `Standard input code` for `-r`).
    pub argv: Vec<String>,
    /// `-d key=value` overrides, applied after the defaults.
    pub ini: Vec<(String, String)>,
    /// The working directory (`None` = the process's).
    pub cwd: Option<PathBuf>,
    /// The script file, when running a file.
    pub script_path: Option<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig::embed()
    }
}

impl EngineConfig {
    /// A host-program configuration: no argv, no script.
    pub fn embed() -> EngineConfig {
        EngineConfig {
            sapi: SapiKind::Embed,
            argv: Vec::new(),
            ini: Vec::new(),
            cwd: None,
            script_path: None,
        }
    }

    /// A command-line configuration.
    pub fn cli() -> EngineConfig {
        EngineConfig {
            sapi: SapiKind::Cli,
            ..EngineConfig::embed()
        }
    }

    /// The built-in web server's configuration (`php -S`).
    pub fn server() -> EngineConfig {
        EngineConfig {
            sapi: SapiKind::Server,
            ..EngineConfig::embed()
        }
    }

    /// The FastCGI SAPI's configuration (php-fpm's).
    pub fn fcgi() -> EngineConfig {
        EngineConfig {
            sapi: SapiKind::Fcgi,
            ..EngineConfig::embed()
        }
    }
}

/// Standard-library declarations written in php and compiled into every
/// interpreter: `PropertyHookType` (php 8.4), the enum
/// `ReflectionProperty::hasHook()`/`getHook()` take.
const PRELUDE: &str = r#"<?php
namespace Dom {
    enum AdjacentPosition: string {
        case BeforeBegin = 'beforebegin';
        case AfterBegin = 'afterbegin';
        case BeforeEnd = 'beforeend';
        case AfterEnd = 'afterend';
    }
}
namespace {
enum PropertyHookType: string { case Get = 'get'; case Set = 'set'; }
enum RoundingMode {
    case HalfAwayFromZero;
    case HalfTowardsZero;
    case HalfEven;
    case HalfOdd;
    case TowardsZero;
    case AwayFromZero;
    case NegativeInfinity;
    case PositiveInfinity;
}
#[Attribute(Attribute::TARGET_CLASS)]
final class Attribute {
    const TARGET_CLASS = 1;
    const TARGET_FUNCTION = 2;
    const TARGET_METHOD = 4;
    const TARGET_PROPERTY = 8;
    const TARGET_CLASS_CONSTANT = 16;
    const TARGET_PARAMETER = 32;
    const TARGET_CONSTANT = 64;
    const TARGET_ALL = 127;
    const IS_REPEATABLE = 128;
    public function __construct(public int $flags = Attribute::TARGET_ALL) {}
}
#[Attribute(Attribute::TARGET_METHOD)]
final class ReturnTypeWillChange { public function __construct() {} }
#[Attribute(Attribute::TARGET_CLASS)]
final class AllowDynamicProperties { public function __construct() {} }
#[Attribute(Attribute::TARGET_PARAMETER)]
final class SensitiveParameter { public function __construct() {} }
#[Attribute(Attribute::TARGET_METHOD | Attribute::TARGET_PROPERTY)]
final class Override { public function __construct() {} }
#[Attribute(Attribute::TARGET_METHOD | Attribute::TARGET_FUNCTION | Attribute::TARGET_CLASS_CONSTANT | Attribute::TARGET_CLASS | Attribute::TARGET_CONSTANT)]
final class Deprecated {
    public function __construct(public readonly ?string $message = null, public readonly ?string $since = null) {}
}
}
"#;

/// The per-process engine: creates interpreters, compiles and runs code.
pub struct Engine {
    config: EngineConfig,
}

thread_local! {
    /// Compiled units by file — opcache's role. Per thread, because a
    /// compiled module holds `Rc`-counted string constants: the built-in
    /// server keeps one long-lived thread per worker and serves its
    /// requests on it, so the cache lives across requests there; a CLI
    /// run compiles each file once anyway.
    static UNITS: RefCell<HashMap<String, CachedUnit>> = RefCell::new(HashMap::new());
}

/// A compiled file and what it was compiled under: the file's size and
/// mtime (a change recompiles, as opcache's `validate_timestamps` does)
/// and the two ini settings that shape compilation.
struct CachedUnit {
    size: u64,
    mtime: Option<std::time::SystemTime>,
    short_open_tag: bool,
    assertions: i8,
    module: Module,
    /// When the file was last found unchanged: within
    /// [`REVALIDATE_EVERY`] of it the include path trusts the entry
    /// without a `stat`, as opcache's `revalidate_freq` does.
    validated: std::time::Instant,
}

/// How long a cached unit is trusted without a fresh `stat` of its file
/// (opcache's `revalidate_freq`, 2 seconds).
const REVALIDATE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

impl Engine {
    /// An engine with `config`.
    pub fn new(config: EngineConfig) -> Engine {
        Engine { config }
    }

    /// Compile a file's source through the thread's unit cache: a hit
    /// (same size, mtime and compile-time ini) hands out a copy of the
    /// compiled module without parsing; a miss compiles and remembers the
    /// result. Only real files are cached (`-r` code and `eval` have no
    /// path).
    fn compile_cached(interp: &Interp, src: &[u8], name: &str) -> Result<Module, CompileError> {
        if !Path::new(name).is_absolute() {
            return compile_unit(interp, src, name);
        }
        let (short_open_tag, assertions) = Engine::compile_ini(interp);
        let mtime = std::fs::metadata(name).ok().and_then(|m| m.modified().ok());
        let size = src.len() as u64;
        if let Some(module) = Engine::cached_unit(name, size, mtime, short_open_tag, assertions) {
            return Ok(module);
        }
        let module = compile_unit(interp, src, name)?;
        UNITS.with(|units| {
            units.borrow_mut().insert(
                name.to_string(),
                CachedUnit { size, mtime, short_open_tag, assertions, module: module.clone(), validated: std::time::Instant::now() },
            );
        });
        Ok(module)
    }

    /// The two ini settings a compiled unit depends on.
    fn compile_ini(interp: &Interp) -> (bool, i8) {
        let short_open_tag = interp.ini_get("short_open_tag").is_some_and(rphp_runtime::parse_bool);
        let assertions = interp.ini_get("zend.assertions").and_then(|v| v.parse::<i8>().ok()).unwrap_or(1);
        (short_open_tag, assertions)
    }

    /// The cached module for `name` if it was compiled for a file of this
    /// size and mtime under these ini settings.
    fn cached_unit(name: &str, size: u64, mtime: Option<std::time::SystemTime>, short_open_tag: bool, assertions: i8) -> Option<Module> {
        UNITS.with(|units| {
            let mut units = units.borrow_mut();
            let u = units.get_mut(name)?;
            if u.size == size && u.mtime == mtime && u.short_open_tag == short_open_tag && u.assertions == assertions {
                u.validated = std::time::Instant::now();
                Some(u.module.clone())
            } else {
                None
            }
        })
    }

    /// The include path's cache probe: an entry validated within the last
    /// two seconds is trusted as it is; otherwise one `stat` decides
    /// whether the file needs reading at all.
    fn cached_unit_for_file(interp: &Interp, name: &str) -> Option<Module> {
        let (short_open_tag, assertions) = Engine::compile_ini(interp);
        let fresh = UNITS.with(|units| {
            let units = units.borrow();
            units
                .get(name)
                .filter(|u| {
                    u.short_open_tag == short_open_tag && u.assertions == assertions && u.validated.elapsed() < REVALIDATE_EVERY
                })
                .map(|u| u.module.clone())
        });
        if fresh.is_some() {
            return fresh;
        }
        let meta = std::fs::metadata(name).ok()?;
        Engine::cached_unit(name, meta.len(), meta.modified().ok(), short_open_tag, assertions)
    }

    /// The configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// A fresh interpreter writing to `sink`: the stdlib and the engine
    /// constants registered, the ini defaults with the configured overrides
    /// applied, `$_SERVER`/`$argv`/`$argc` seeded into the globals table,
    /// and the compile hook `include`/`require` use installed.
    pub fn new_interp(&self, sink: Box<dyn OutputSink>) -> Interp {
        let mut it = self.new_interp_unseeded(sink);
        self.seed_globals(&mut it);
        it
    }

    /// [`Engine::new_interp`] without the CLI's `$_SERVER`/`$argv` seeding:
    /// a web SAPI binds its own request afterwards.
    pub fn new_interp_unseeded(&self, sink: Box<dyn OutputSink>) -> Interp {
        let cfg = &self.config;
        let mut it = Interp::new(sink);
        it.sapi = cfg.sapi;
        if let Some(cwd) = &cfg.cwd {
            it.cwd = cwd.clone();
        }
        it.argv = cfg.argv.iter().map(|s| s.as_bytes().to_vec()).collect();
        it.script_path = cfg.script_path.clone();
        if cfg.sapi.is_web() {
            for (k, v) in rphp_runtime::SERVER_DEFAULTS {
                it.ini_set(k, v);
            }
        }
        for (k, v) in &cfg.ini {
            // `-d` accepts unknown directives too (php stores them as-is).
            if it.ini_set(k, v).is_none() {
                it.ini.register(k, v);
            }
        }
        rphp_stdlib::register(&mut Registry(&mut it));
        rphp_ext_pdo::register(&mut Registry(&mut it));
        rphp_ext_dom::register(&mut Registry(&mut it));
        rphp_ext_intl::register(&mut Registry(&mut it));
        rphp_ext_bcmath::register(&mut Registry(&mut it));
        rphp_ext_xml::register(&mut Registry(&mut it));
        rphp_ext_posix::register(&mut Registry(&mut it));
        // `Generator` is the engine's own class but implements the stdlib's
        // `Iterator`, so it is registered after the extensions (E8).
        rphp_runtime::register_generator_class(&mut Registry(&mut it));
        rphp_runtime::register_fiber_classes(&mut Registry(&mut it));
        constants::register(&mut Registry(&mut it), cfg.sapi);
        // The standard library's php-written part: what the native registry
        // cannot declare (an enum; the engine's attribute classes, which
        // carry their own `#[Attribute(...)]` with constant arguments).
        match compile_unit(&it, PRELUDE.as_bytes(), "prelude") {
            Ok(module) => {
                let _ = it.run_prelude(module);
            }
            Err(e) => debug_assert!(false, "prelude does not compile: {e:?}"),
        }
        // php starts its execution timer at request startup: `max_execution_time`
        // (30 under a web SAPI, 0 on the command line).
        let limit = it.ini.int("max_execution_time");
        if limit > 0 {
            it.set_time_limit(limit as u64);
        }
        it.cached_unit_hook = Some(Box::new(Engine::cached_unit_for_file));
        it.compile_hook = Some(Box::new(|interp: &Interp, src: &[u8], name: &str| {
            Engine::compile_cached(interp, src, name).map_err(|e| match e {
                CompileError::Parse { message, line, .. } => {
                    CompileFailure::Parse { message, line }
                }
                // php renders a compile-time fatal of an included file as a
                // `Fatal error:`; the runtime's hook contract has no such
                // variant yet, so it goes through the rejection channel.
                CompileError::Fatal { rendered, .. } | CompileError::Compile(rendered) => {
                    CompileFailure::Rejected(rendered)
                }
            })
        }));
        it
    }

    /// `$_SERVER`, `$argv`, `$argc` as the CLI SAPI seeds them.
    fn seed_globals(&self, it: &mut Interp) {
        let cfg = &self.config;
        let mut argv = Array::new();
        for a in &cfg.argv {
            argv.push(Value::string(a.as_bytes()));
        }
        let argc = Value::Int(cfg.argv.len() as i64);
        let mut server = Array::new();
        for (k, v) in std::env::vars_os() {
            server.set(
                ArrayKey::str(k.to_string_lossy().as_bytes()),
                Value::string(v.to_string_lossy().as_bytes()),
            );
        }
        let script = cfg.argv.first().cloned().unwrap_or_default();
        for key in [
            "PHP_SELF",
            "SCRIPT_NAME",
            "SCRIPT_FILENAME",
            "PATH_TRANSLATED",
        ] {
            server.set(
                ArrayKey::str(key.as_bytes()),
                Value::string(script.as_bytes()),
            );
        }
        server.set(ArrayKey::str(b"DOCUMENT_ROOT"), Value::string(b""));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        server.set(ArrayKey::str(b"REQUEST_TIME_FLOAT"), Value::Float(now));
        server.set(ArrayKey::str(b"REQUEST_TIME"), Value::Int(now as i64));
        server.set(ArrayKey::str(b"argv"), Value::Array(argv.clone()));
        server.set(ArrayKey::str(b"argc"), argc.clone());
        // php's CLI creates these seven up front, in this order — which is
        // the order `array_keys($GLOBALS)` shows. `$_ENV` and `$_REQUEST` are
        // *not* here: php fills them the first time a script touches one
        // (`Interp::auto_global_cell`).
        it.globals.insert(b"argv", PhpRef::new(Value::Array(argv)));
        it.globals.insert(b"argc", PhpRef::new(argc));
        for name in [&b"_GET"[..], b"_POST", b"_COOKIE", b"_FILES"] {
            it.globals
                .insert(name, PhpRef::new(Value::Array(Array::new())));
        }
        it.globals
            .insert(b"_SERVER", PhpRef::new(Value::Array(server)));
    }

    /// Parse and compile `src` (named `name` in diagnostics) against
    /// `interp`'s registry, with line tables. The front end honours the
    /// interpreter's `short_open_tag`; a parse error aborts before
    /// compilation ([`CompileError::Parse`]).
    pub fn compile(&self, interp: &Interp, src: &[u8], name: &str) -> Result<Module, CompileError> {
        compile_unit(interp, src, name)
    }

    /// Compile `src` and install it as `interp`'s program (its hoisted
    /// functions and classes are declared; a redeclaration is a fatal error
    /// reported through the diagnostics channel).
    pub fn load(&self, interp: &mut Interp, src: &[u8], name: &str) -> Result<(), CompileError> {
        let module = self.compile(interp, src, name)?;
        interp.script_name = name.to_string();
        if let Err(u) = interp.load_module(module) {
            interp.handle_top_level_unwind(u);
            interp.finish_output();
            return Err(CompileError::Compile(Vec::new()));
        }
        Ok(())
    }

    /// Report a failed [`Engine::load`] the way php-cli does and return the
    /// exit code (255): a parse error is displayed as
    /// `Parse error: <message> in <file> on line <N>` through the error
    /// display channel (`display_errors`, `log_errors`), like a fatal error
    /// but without a backtrace; a resolution/validation error is php's
    /// compile-time fatal, rendered through the same channel as a runtime
    /// fatal (`Fatal error: <message> in <file> on line <N>` plus the
    /// `Stack trace:` block php ≥ 8.5 prints); a compile rejection prints
    /// its rendered diagnostics on stderr (php has no equivalent: these are
    /// constructs the engine does not lower yet).
    pub fn report_load_error(&self, interp: &mut Interp, name: &str, err: CompileError) -> i32 {
        match err {
            CompileError::Parse { message, line, .. } => {
                // The standard display path: `log_errors`, `html_errors`,
                // `error_get_last()` all apply to a parse error of the
                // main script as to any other.
                let _ = interp.emit_error_full(
                    rphp_runtime::ErrLevel::Parse,
                    &message,
                    name,
                    line,
                    false,
                );
                interp.finish_output();
            }
            CompileError::Fatal { message, line, .. } => {
                let _ = interp.fatal_at(&message, name, line);
                interp.finish_output();
            }
            CompileError::Compile(lines) => {
                for line in lines {
                    eprintln!("{line}");
                }
            }
        }
        255
    }

    /// Run the loaded program with php's request lifecycle: `{main}`, the
    /// shutdown functions (in order; a fault or `exit()` in one ends the
    /// sequence), the output flush. An uncaught fault is rendered like
    /// php-cli. Returns the exit code (0, 255 on a fatal error, or the
    /// `exit()` code).
    pub fn execute(&self, interp: &mut Interp) -> i32 {
        let code = match interp.run_main() {
            Ok(_) => 0,
            Err(u) => interp.handle_top_level_unwind(u),
        };
        let code = interp.run_shutdown_functions(code);
        // php's `zend_call_destructors`: after the shutdown functions and
        // before the output buffers are flushed (ADR-017).
        let code = match interp.shutdown_destructors() {
            Ok(()) => code,
            Err(u) => interp.handle_top_level_unwind(u),
        };
        interp.finish_output();
        request_shutdown(interp);
        code
    }

    /// Compile and run `src` as the unit named `name` (`Command line code`
    /// for `-r`), writing to `sink`. A parse error is displayed like php's
    /// and a compile rejection goes to stderr; both exit 255 (php's code for
    /// a fatal compile error).
    pub fn run_code(&self, src: &[u8], name: &str, sink: Box<dyn OutputSink>) -> i32 {
        on_request_stack(|| {
            let mut interp = self.new_interp(sink);
            if let Err(err) = self.load(&mut interp, src, name) {
                return self.report_load_error(&mut interp, name, err);
            }
            self.execute(&mut interp)
        })
    }

    /// Run the script at `path` (diagnostics name it by its canonical
    /// absolute path, as php does). An unreadable file prints php's
    /// `Could not open input file: <path>` and returns 1.
    pub fn run_file(&self, path: &Path, sink: Box<dyn OutputSink>) -> i32 {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => {
                println!("Could not open input file: {}", path.display());
                return 1;
            }
        };
        let name = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let name = name.to_string_lossy().into_owned();
        on_request_stack(|| {
            let mut interp = self.new_interp(sink);
            interp.script_path = Some(PathBuf::from(&name));
            if let Err(err) = self.load(&mut interp, &bytes, &name) {
                return self.report_load_error(&mut interp, &name, err);
            }
            self.execute(&mut interp)
        })
    }
}

/// The stack size of the thread a request runs on. PHP→PHP calls never
/// recurse on the Rust stack (ADR-018), but every native→PHP re-entry
/// (`array_map` callbacks, `usort`, output handlers, default-value thunks…)
/// nests one dispatch loop on it; the engine caps that nesting at
/// [`rphp_runtime::MAX_REENTRY_DEPTH`], and this reservation gives every
/// level room (debug builds spend ~50 KiB per level). Virtual only: pages
/// are committed as they are touched.
pub const REQUEST_STACK_SIZE: usize = 512 << 20;

/// Run `f` on a thread with [`REQUEST_STACK_SIZE`] of stack, blocking until
/// it finishes. Falls back to the current thread if no thread can be
/// spawned.
/// A long-lived thread with the engine's stack reservation (the built-in
/// server's workers): `f` runs on it and the handle joins it.
pub fn request_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new()
        .name("rphp-request".into())
        .stack_size(REQUEST_STACK_SIZE)
        .spawn(f)
}

pub fn on_request_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        match std::thread::Builder::new()
            .name("rphp-request".into())
            .stack_size(REQUEST_STACK_SIZE)
            .spawn_scoped(scope, f)
        {
            Ok(handle) => handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(_) => unreachable!("spawn failure is handled by the caller"),
        }
    })
}

/// Parse and compile `src` (named `name` in diagnostics; the file path for
/// scripts and included files, `Command line code` for `-r`) against
/// `interp`'s ini, with line tables. The front end honours `short_open_tag`;
/// a parse error aborts before compilation ([`CompileError::Parse`]).
fn compile_unit(interp: &Interp, src: &[u8], name: &str) -> Result<Module, CompileError> {
    let mut sources = SourceMap::new();
    let id = sources.add(name.to_string(), src.to_vec());
    let mut interner = Interner::new();
    let opts = ParseOptions {
        file: id,
        path: Some(Path::new(name)),
        short_open_tag: interp
            .ini_get("short_open_tag")
            .is_some_and(rphp_runtime::parse_bool),
    };
    let parsed = parse_v2(src, opts, &mut interner);
    let render =
        |diags: &[Diagnostic]| diags.iter().map(|d| d.render(&sources)).collect::<Vec<_>>();
    let file = sources.get(id);
    if let Some(first) = parsed.diagnostics.iter().find(|d| d.is_error()) {
        let line = first
            .primary
            .as_ref()
            .map_or(1, |l| file.line_col(l.span.lo).0);
        // The adapter's conformance pass reports php's *compile-time*
        // rejections too (`Cannot re-assign $this`, `Cannot mix bracketed
        // namespace declarations …`): those are fatals in php.
        let rendered = render(&parsed.diagnostics);
        let message = first.message.clone();
        return Err(if is_php_compile_fatal(first.code, &message) {
            CompileError::Fatal {
                message,
                line,
                rendered,
            }
        } else {
            CompileError::Parse {
                message,
                line,
                rendered,
            }
        });
    }
    let line_of = |offset: u32| file.line_col(offset).0;
    // A real file path gives `__FILE__`/`__DIR__`; the `-r` unit name does not.
    let path = Path::new(name);
    let opts = CompileOptions {
        line_of: Some(&line_of),
        file: path.is_absolute().then(|| path.to_path_buf()),
        // php reads `zend.assertions` when it *compiles* a unit, which is why
        // changing it at run time does nothing.
        assertions: interp
            .ini_get("zend.assertions")
            .and_then(|v| v.parse::<i8>().ok())
            .unwrap_or(1),
        source: Some(src),
    };
    // `compile` runs the HIR pass (resolution, validation, desugaring) and
    // then lowers; a resolution/validation error is php's compile-time
    // fatal, an `RPHP_E0300`-style rejection is the engine's own.
    compile(parsed.program, &mut interner, &opts).map_err(|diags| {
        let first = diags.iter().find(|d| d.is_error());
        let line = |d: &Diagnostic| d.primary.as_ref().map_or(1, |l| file.line_col(l.span.lo).0);
        match first {
            // php's own compile-time errors carry php's text: the HIR's
            // resolution/validation codes are fatals, except the rejections
            // php's grammar itself makes (`syntax error, unexpected token
            // "const"`), which are parse errors.
            Some(d) if is_php_compile_fatal(d.code, &d.message) => CompileError::Fatal {
                message: d.message.clone(),
                line: line(d),
                rendered: render(&diags),
            },
            Some(d) if is_php_parse_error(d.code, &d.message) => CompileError::Parse {
                message: d.message.clone(),
                line: line(d),
                rendered: render(&diags),
            },
            _ => CompileError::Compile(render(&diags)),
        }
    })
}

/// Whether a front-end diagnostic is one php's grammar reports (`Parse
/// error:`): the lexer/parser codes, and the superset/validation
/// rejections whose text is php's parser's.
fn is_php_parse_error(code: &str, message: &str) -> bool {
    matches!(
        code,
        codes::UNEXPECTED_CHAR
            | codes::UNEXPECTED_TOKEN
            | codes::UNTERMINATED
            | codes::UNEXPECTED_EOF
            | codes::RECURSION_LIMIT
            | codes::INVALID_LITERAL
            | codes::UNSUPPORTED_NODE
    ) || message.starts_with("syntax error")
        || message.starts_with("The (real) cast")
}

/// Whether a front-end diagnostic is one of php's compile-time fatals
/// (`Fatal error:`): the adapter's conformance/lvalue rejections and the
/// HIR's resolution/validation codes — everything php's compiler (not its
/// grammar) refuses, with php's own text. Engine rejections (`RPHP_E0300`
/// and the compiler's own codes) are neither.
fn is_php_compile_fatal(code: &str, message: &str) -> bool {
    if is_php_parse_error(code, message) {
        return false;
    }
    matches!(
        code,
        codes::SUPERSET_REJECTED
            | codes::LVALUE_NOT_WRITABLE
            | codes::LVALUE_NULLSAFE
            | codes::LVALUE_THIS
            | codes::LVALUE_GLOBALS
            | codes::LVALUE_APPEND
            | codes::LVALUE_DESTRUCTURING
            | codes::IMPORT_CONFLICT
            | codes::RESERVED_CLASS_NAME
            | codes::UNDEFINED_LABEL
            | codes::INVALID_JUMP
            | codes::NO_CLASS_SCOPE
            | codes::NO_PARENT_SCOPE
            | codes::NAMESPACE_MIX
            | codes::REDECLARED_FUNCTION
            | codes::REDECLARED_CLASS
    )
}

/// php's per-request module shutdown, after the output is flushed: the
/// extensions forget what they kept for this request (an active session is
/// written and closed first). A server's worker thread runs many requests,
/// so every one ends here.
pub fn request_shutdown(interp: &mut Interp) {
    rphp_stdlib::request_shutdown(interp);
    interp.dump_profile();
}

/// Evaluate PHP source through the full parse → compile → run pipeline on an
/// embed-SAPI engine and return the captured output bytes. Compile
/// diagnostics and an uncaught fault are returned as the `Err` string
/// (`Uncaught <Class>: <message>`); `exit()` is not an error. Warnings and
/// notices are **not** displayed (`display_errors=0`): this helper captures
/// the script's own output; php-cli's diagnostics display is the
/// [`Engine`]'s job.
pub fn eval_to_bytes(src: &[u8]) -> Result<Vec<u8>, String> {
    let mut config = EngineConfig::embed();
    config
        .ini
        .push(("display_errors".to_string(), "0".to_string()));
    let engine = Engine::new(config);
    let buffer = BufferSink::new();
    let sink = buffer.clone();
    on_request_stack(move || {
        let mut interp = engine.new_interp(Box::new(sink));
        engine
            .load(&mut interp, src, "Command line code")
            .map_err(|e| e.into_rendered().join("\n"))?;
        match interp.run_main() {
            Ok(_) | Err(Unwind::Exit(_)) => {}
            Err(u) => return Err(interp.describe_unwind(&u)),
        }
        // Objects still alive at the end are destructed as php does at
        // request shutdown (their output belongs to the script's).
        if let Err(u) = interp.shutdown_destructors() {
            if !matches!(u, Unwind::Exit(_)) {
                return Err(interp.describe_unwind(&u));
            }
        }
        interp.finish_output();
        Ok(())
    })?;
    Ok(buffer.take())
}

/// As [`eval_to_bytes`], decoded lossily as UTF-8.
pub fn eval_to_string(src: &[u8]) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&eval_to_bytes(src)?).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &[u8]) -> (i32, String) {
        let engine = Engine::new(EngineConfig::cli());
        let buffer = BufferSink::new();
        let code = engine.run_code(src, "Command line code", Box::new(buffer.clone()));
        (code, String::from_utf8_lossy(&buffer.take()).into_owned())
    }

    #[test]
    fn spawn_policy_refuses_with_a_warning_and_false() {
        let engine = Engine::new(EngineConfig::cli());
        let buffer = BufferSink::new();
        let sink = buffer.clone();
        let src = b"<?php $r = proc_open(['/bin/echo', 'hi'], [1 => ['pipe', 'w']], $p); var_dump($r); var_dump(shell_exec('echo ok'));";
        on_request_stack(move || {
            let mut interp = engine.new_interp(Box::new(sink));
            spawn::set_policy(
                &mut interp,
                Box::new(|req: &spawn::SpawnRequest<'_>| {
                    // The shell is allowed, a direct program is not.
                    if req.argv[0] == b"/bin/sh" {
                        Ok(())
                    } else {
                        Err(format!("{} is not on the allowlist", String::from_utf8_lossy(&req.argv[0])))
                    }
                }),
            );
            engine.load(&mut interp, src, "Command line code").map_err(|e| e.into_rendered().join("\n"))?;
            let _ = interp.run_main();
            interp.finish_output();
            Ok::<(), String>(())
        })
        .unwrap();
        let out = String::from_utf8_lossy(&buffer.take()).into_owned();
        assert!(out.contains("Warning: proc_open(): Unable to fork [/bin/echo hi]: /bin/echo is not on the allowlist"), "{out}");
        assert!(out.contains("bool(false)\nstring(3) \"ok\n\""), "{out}");
    }

    #[test]
    fn eval_helpers_capture_output_and_faults() {
        assert_eq!(eval_to_string(b"<?php echo 1 + 2;").unwrap(), "3");
        let err = eval_to_string(b"<?php echo 1 / 0;").unwrap_err();
        assert_eq!(err, "Uncaught DivisionByZeroError: Division by zero");
        let err = eval_to_string(b"<?php $x = ;").unwrap_err();
        assert!(err.contains("RPHP_E"), "{err}");
        let err = eval_to_string(b"<?php nope();").unwrap_err();
        assert!(err.contains("undefined function"), "{err}");
    }

    #[test]
    fn parse_error_renders_like_php_cli() {
        // php: nothing runs, `Parse error: ... in <file> on line N` on stdout
        // (display_errors=1), exit 255.
        let (code, out) = run(b"<?php\necho \"a\";\necho 1 +;\n");
        assert_eq!(code, 255);
        assert!(
            out.starts_with("\nParse error: ")
                && out.ends_with(" in Command line code on line 3\n"),
            "{out:?}"
        );
        assert!(!out.contains("Stack trace"), "{out:?}");
        // display_errors=0 hides it; the exit code stays.
        let mut cfg = EngineConfig::cli();
        cfg.ini.push(("display_errors".into(), "0".into()));
        let engine = Engine::new(cfg);
        let buffer = BufferSink::new();
        let code = engine.run_code(
            b"<?php echo 1 +;",
            "Command line code",
            Box::new(buffer.clone()),
        );
        assert_eq!(code, 255);
        assert_eq!(buffer.take(), b"");
        // The structured error carries the line and the rendered diagnostics.
        let engine = Engine::new(EngineConfig::cli());
        let interp = engine.new_interp(Box::new(BufferSink::new()));
        match engine.compile(&interp, b"<?php\n\n$x = ;", "t.php") {
            Err(CompileError::Parse { line, rendered, .. }) => {
                assert_eq!(line, 3);
                assert!(rendered[0].contains("RPHP_E"), "{rendered:?}");
            }
            other => panic!("expected a parse error, got {other:?}"),
        }
        // An unsupported construct is a compile rejection, not a parse error.
        // (`yield` used to sit here; it lowers as of E8, and an enum case
        // initializer as of the P4 tail, so this uses one that is still
        // unlowered.)
        match engine.compile(&interp, b"<?php $x = `ls`;", "t.php") {
            Err(CompileError::Compile(lines)) => {
                assert!(lines[0].contains("RPHP_E0300"), "{lines:?}")
            }
            other => panic!("expected a compile error, got {other:?}"),
        }
    }

    #[test]
    fn short_open_tag_follows_ini() {
        assert_eq!(eval_to_string(b"<? echo 1;").unwrap(), "1");
        let mut cfg = EngineConfig::cli();
        cfg.ini.push(("short_open_tag".into(), "0".into()));
        let engine = Engine::new(cfg);
        let buffer = BufferSink::new();
        let code = engine.run_code(b"<? echo 1;", "Command line code", Box::new(buffer.clone()));
        assert_eq!(code, 0);
        assert_eq!(buffer.take(), b"<? echo 1;");
    }

    #[test]
    fn fatal_after_output_renders_like_php_cli() {
        let (code, out) = run(b"<?php\necho \"before\\n\";\n$f = 'nope';\n$f();\n");
        assert_eq!(code, 255);
        assert_eq!(
            out,
            "before\n\nFatal error: Uncaught Error: Call to undefined function nope() in Command line code:4\n\
             Stack trace:\n#0 {main}\n  thrown in Command line code on line 4\n"
        );
    }

    #[test]
    fn native_fault_shows_the_native_frame() {
        let (code, out) = run(b"<?php\n\necho intdiv(1, 0);\n");
        assert_eq!(code, 255);
        assert_eq!(
            out,
            "\nFatal error: Uncaught DivisionByZeroError: Division by zero in Command line code:3\n\
             Stack trace:\n#0 Command line code(3): intdiv(1, 0)\n#1 {main}\n  thrown in Command line code on line 3\n"
        );
    }

    #[test]
    fn nested_user_frames_render_call_sites() {
        // (A direct `nope()` is a compile-time diagnostic; a callable string
        // is the runtime fault.)
        let (code, out) = run(b"<?php\nfunction f($a) { $g = 'nope'; $g(); }\nf(1);\n");
        assert_eq!(code, 255);
        assert_eq!(
            out,
            "\nFatal error: Uncaught Error: Call to undefined function nope() in Command line code:2\n\
             Stack trace:\n#0 Command line code(3): f(1)\n#1 {main}\n  thrown in Command line code on line 2\n"
        );
    }

    #[test]
    fn warnings_constants_and_ob_through_the_pipeline() {
        let (code, out) = run(
            b"<?php\n$a = [];\necho $a[\"k\"];\necho constant('PHP_EOL');\nob_start();\necho \"x\";\n$s = ob_get_clean();\necho \"[\" . $s . \"]\";\nvar_dump(defined('E_ALL'), constant('E_ALL'), ini_get('display_errors'));\n",
        );
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "\nWarning: Undefined array key \"k\" in Command line code on line 3\n\n[x]bool(true)\nint(30719)\nstring(1) \"1\"\n"
        );
    }

    #[test]
    fn error_handler_receives_vm_warnings_and_shutdown_runs() {
        let (code, out) = run(
            b"<?php\nset_error_handler(function ($no, $str, $file, $line) { echo \"H[$no] $str @$line\\n\"; return true; });\nregister_shutdown_function(function () { echo \"shut\\n\"; });\n$a = [];\necho $a[5];\necho \"end\\n\";\n",
        );
        assert_eq!(code, 0);
        assert_eq!(out, "H[2] Undefined array key 5 @5\nend\nshut\n");
    }

    #[test]
    fn ini_overrides_and_argv_are_applied() {
        let mut cfg = EngineConfig::cli();
        cfg.ini.push(("display_errors".into(), "0".into()));
        cfg.ini.push(("custom.flag".into(), "yes".into()));
        cfg.argv = vec!["s.php".into(), "a".into()];
        let engine = Engine::new(cfg);
        let buffer = BufferSink::new();
        let mut it = engine.new_interp(Box::new(buffer.clone()));
        assert_eq!(it.ini_get("display_errors"), Some("0"));
        assert_eq!(it.ini_get("custom.flag"), Some("yes"));
        assert_eq!(it.argv(), &[b"s.php".to_vec(), b"a".to_vec()]);
        assert_eq!(
            it.globals.get(b"argc").map(|c| c.get()),
            Some(Value::Int(2))
        );
        assert_eq!(it.constant(b"PHP_SAPI"), Some(Value::string(b"cli")));
        it.warn("hidden").unwrap();
        assert_eq!(buffer.take(), b"");
    }
}
