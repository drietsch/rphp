//! [`Interp`]: all per-request engine state (plan E1/E3, ADR-014/016/018/
//! 019/020). One `Interp` runs one script; the SAPI (through `rphp-embed`)
//! creates it, registers the extension bundle and engine constants, loads
//! the compiled unit, runs `{main}`, the shutdown functions and the output
//! flush, and reads the exit code back.

use std::collections::HashSet;

use hashbrown::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use rphp_bytecode::Module;
use rphp_value::{ObjectIdAllocator, Value};

use crate::errors::E_ALL;
use crate::frame::{Frame, FrameKind, RetTarget};
use crate::ini::IniTable;
use crate::output::{OutputStack, SharedBuffer};
use crate::registry::{FnFlags, NativeFn, NativeId, Unwind};
use crate::resources::ResourceTable;
use crate::symtab::Symtab;
use crate::class::{ClassDef, WellKnown};
use crate::unit::{FuncRt, UnitRt};
use crate::LastError;

/// Which SAPI created this interpreter (`PHP_SAPI`, `php_sapi_name()`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SapiKind {
    /// The command line (`cli`).
    Cli,
    /// A host program embedding the engine (`embed`).
    Embed,
    /// An HTTP server SAPI (`cli-server`).
    Server,
    /// The FastCGI SAPI (`fpm-fcgi`, as php-fpm names itself).
    Fcgi,
}

impl SapiKind {
    /// The `PHP_SAPI` string.
    pub fn name(self) -> &'static str {
        match self {
            SapiKind::Cli => "cli",
            SapiKind::Embed => "embed",
            SapiKind::Server => "cli-server",
            SapiKind::Fcgi => "fpm-fcgi",
        }
    }

    /// Whether the SAPI serves HTTP requests (headers, `getallheaders()`).
    pub fn is_web(self) -> bool {
        matches!(self, SapiKind::Server | SapiKind::Fcgi)
    }
}

/// Per-extension state slots the stdlib keeps between calls.
#[derive(Default)]
pub struct ExtState {
    /// `preg_last_error()`.
    pub preg_last_error: i64,
    /// `preg_last_error_msg()`.
    pub preg_last_error_msg: String,
    /// `json_last_error()`.
    pub json_last_error: i64,
    /// `json_last_error_msg()`.
    pub json_last_error_msg: String,
    /// php's per-request stat cache (`filestat.rs`): path → metadata, with
    /// `None` meaning "checked, does not exist". `clearstatcache()` empties
    /// it and anything that changes a path drops its entry.
    pub stat_cache: std::collections::HashMap<std::path::PathBuf, Option<std::fs::Metadata>>,
    /// What `openlog()` set (`syslog.rs`): `(ident, flags, facility)`.
    pub syslog: (Option<Vec<u8>>, i64, i64),
    /// Per-extension state an extension crate keeps under its own key
    /// (`libxml`'s error list, …), typed by the crate that owns it.
    pub slots: std::collections::HashMap<&'static str, Box<dyn std::any::Any>>,
}

impl ExtState {
    /// The extension state under `key`, created with `Default` on first use.
    pub fn slot<T: Default + 'static>(&mut self, key: &'static str) -> &mut T {
        let entry = self
            .slots
            .entry(key)
            .or_insert_with(|| Box::new(T::default()));
        if !entry.is::<T>() {
            *entry = Box::new(T::default());
        }
        entry.downcast_mut::<T>().expect("slot type checked")
    }
}

/// Why the compile hook could not produce a unit for `include`/`eval`.
#[derive(Clone, Debug)]
pub enum CompileFailure {
    /// A parse error: php reports it as a fatal `Parse error` (or throws
    /// `ParseError` inside `eval`).
    Parse { message: String, line: u32 },
    /// The compiler rejected the program (constructs not lowered yet); the
    /// rendered diagnostics.
    Rejected(Vec<String>),
}

/// The callback `include`/`require` use to compile a file on demand
/// (installed by `rphp-embed`): source bytes and the file name as php
/// reports it → a module.
pub type CompileHook = Box<dyn Fn(&Interp, &[u8], &str) -> Result<Module, CompileFailure>>;

/// The embedder's compiled-unit cache, asked by name (the canonical path)
/// before an included file is read: `Some` is the module compiled for the
/// file as it is on disk now.
pub type CachedUnitHook = Box<dyn Fn(&Interp, &str) -> Option<Module>>;

/// The maximum nesting of native→PHP re-entries (`run_until` on the Rust
/// stack) before the engine gives up, so the host stack cannot overflow.
pub const MAX_REENTRY_DEPTH: usize = 512;
/// The maximum number of frames before the engine gives up (php has no
/// such limit — it runs out of memory instead; cataloged divergence).
pub const MAX_FRAMES: usize = 1_000_000;

/// The interpreter: registry, tables, output, diagnostics, the frame and
/// register stacks, and the per-request bookkeeping the engine needs. Fields
/// the stdlib reads and writes directly are public; the registry, the
/// tables and the frame stack go through methods.
pub struct Interp {
    pub(crate) natives: Vec<NativeFn>,
    /// Argument vectors a native call takes and hands back (the drained
    /// window and the frame's copy for traces), so a call allocates nothing.
    pub(crate) vec_pool: Vec<Vec<Value>>,
    /// Lowercased name → id.
    pub(crate) native_index: HashMap<Box<[u8]>, NativeId>,
    pub(crate) constants: HashMap<Box<[u8]>, Value>,
    /// Constants php deprecates: the text after `Constant X is deprecated`.
    pub(crate) deprecated_constants: HashMap<Box<[u8]>, &'static str>,
    /// The names `define()` added, so `get_defined_constants(true)` can put
    /// them under php's `user` category (the engine's own are `Core`).
    pub(crate) user_constants: Vec<Box<[u8]>>,
    /// Every loaded unit.
    pub(crate) units: Vec<Rc<UnitRt>>,
    /// Every function of every loaded unit, by process-wide id.
    pub(crate) funcs: Vec<Rc<FuncRt>>,
    /// Lowercased name → declared user function.
    pub(crate) func_index: HashMap<Box<[u8]>, u32>,
    /// Bumped by every function declaration (inline-cache stamp).
    pub(crate) func_gen: u32,
    /// Every class of every loaded unit (plus engine classes), by id.
    pub(crate) classes: Vec<Rc<ClassDef>>,
    /// Lowercased name → declared class.
    pub(crate) class_index: HashMap<Box<[u8]>, u32>,
    /// Declared classes in declaration order (`get_declared_classes`).
    pub(crate) class_order: Vec<u32>,
    /// Bumped by every class declaration.
    pub(crate) class_gen: u32,
    /// Ids of the classes the engine needs to find by role.
    pub well_known: WellKnown,
    /// Live objects whose class has `__destruct`, in creation order, for
    /// the end-of-request destructor pass (ADR-017).
    pub(crate) destructibles: Vec<rphp_value::WeakObject>,
    /// `true` while the end-of-request destructor pass runs.
    pub(crate) in_shutdown: bool,
    /// The entry script's `{main}` (set by [`Interp::load_module`]).
    pub(crate) main_func: Option<u32>,
    /// The frame stack.
    pub(crate) frames: Vec<Frame>,
    /// The contiguous register stack (every user frame's window).
    pub(crate) stack: Vec<Value>,
    /// Native→PHP re-entries in progress.
    pub(crate) reentry_depth: usize,
    /// Canonical paths of files included with `_once`.
    pub(crate) included: HashSet<PathBuf>,
    /// Parked generator bodies, indexed by the id in a `Generator` object's
    /// payload (E8).
    pub(crate) generators: Vec<crate::generator::GeneratorState>,
    /// Every fiber built this request (`fiber.rs`).
    pub(crate) fibers: Vec<crate::fiber::FiberState>,
    /// The running fibers, innermost last.
    pub(crate) fiber_stack: Vec<u32>,
    /// Extensions registered by crates outside the stdlib (`pdo`, `dom`,
    /// …), for `extension_loaded()` / `get_loaded_extensions()`.
    pub extensions: Vec<&'static str>,
    /// Set by `Fiber::suspend()` for the dispatch loop, which parks the
    /// fiber once the call op that reached it has returned.
    pub(crate) fiber_suspending: Option<u32>,
    /// The value a re-entry boundary must hand back when its frame was
    /// removed without returning through `do_return` — which is what calling
    /// a generator function from native code does (E8).
    pub(crate) boundary_value: Option<rphp_value::Value>,
    /// The `spl_autoload_register` stack, in call order (E7).
    pub(crate) autoloaders: Vec<rphp_value::Value>,
    /// Class names an autoloader is running for right now, so a loader that
    /// touches its own class cannot recurse forever.
    pub(crate) autoloading: Vec<Box<[u8]>>,
    /// The compile hook for `include`/`require`.
    pub compile_hook: Option<CompileHook>,
    /// See [`CachedUnitHook`].
    pub cached_unit_hook: Option<CachedUnitHook>,
    /// The output stack (`echo`, `ob_*`) over the SAPI's sink.
    pub out: OutputStack,
    /// The ini table.
    pub ini: IniTable,
    /// `error_reporting()`.
    pub error_reporting: i64,
    /// The `set_error_handler` stack: `(callback, level mask)`; a `Null`
    /// callback entry means "no handler" (`set_error_handler(null)`).
    pub error_handler: Vec<(Value, i64)>,
    /// Where the first output that reached the SAPI came from, which php
    /// names in `Cannot modify header information - headers already sent by
    /// (output started at FILE:LINE)`.
    pub output_started: Option<(String, u32)>,
    /// Where the output *currently being produced* comes from, kept only
    /// until the first bytes reach the SAPI (after that php never asks
    /// again).
    pub(crate) pending_site: Option<(String, u32)>,
    /// The response head (`header()`, `http_response_code()`), shared with
    /// the SAPI's sink, which sends it ahead of the first output byte.
    pub head: crate::output::SharedHead,
    /// The request's header fields as received, in order and with their
    /// original spelling (`getallheaders()`); empty outside a web SAPI.
    pub request_headers: Vec<(String, String)>,
    /// A CGI SAPI's environment for the request (php-fpm's: `USER`, `HOME`,
    /// the FastCGI parameters, `FCGI_ROLE`), which `getenv()`, `putenv()`
    /// and `$_ENV` see in place of the process's; `None` = the process's.
    pub request_env: Option<Vec<(String, String)>>,
    /// The raw request body (`php://input`); `None` outside a web SAPI.
    pub request_body: Option<std::sync::Arc<[u8]>>,
    /// Where `log_errors` entries go (`PHP Warning:  …`): stderr when
    /// `None`, else the SAPI's log (`php -S` stamps each line).
    pub error_log: Option<Box<dyn FnMut(&str)>>,
    /// The request's start time (`$_SERVER['REQUEST_TIME_FLOAT']`), which
    /// `setcookie()`'s `Max-Age` counts from; `None` = the clock.
    pub request_time: Option<f64>,
    /// The temporary files this request's uploads landed in (`$_FILES`):
    /// what `is_uploaded_file()` checks, removed at request end unless
    /// `move_uploaded_file()` took one.
    pub uploaded_files: Vec<PathBuf>,
    /// The `set_exception_handler` stack (`Null` = none).
    pub exception_handler: Vec<Value>,
    /// `register_shutdown_function` callbacks with their bound arguments.
    pub shutdown: Vec<(Value, Vec<Value>)>,
    /// `error_get_last()`.
    pub last_error: Option<LastError>,
    /// `@` nesting depth.
    pub silence: u32,
    /// A `FnFlags::LIGHT` native is running without a frame: a
    /// diagnostic it emits cannot be attributed, so `emit_error` aborts
    /// the call with `Unwind::Retry` and the engine re-runs it on the
    /// full path.
    pub light_native: bool,
    /// Per-extension state slots.
    pub ext: ExtState,
    /// The resource table.
    pub resources: ResourceTable,
    /// Object handle allocator.
    pub object_ids: ObjectIdAllocator,
    /// The global symbol table: superglobals seeded by the SAPI (`_SERVER`,
    /// `argv`, `argc`) and every variable of the entry script's `{main}`.
    pub globals: Symtab,
    /// The script's argument vector (`$argv`), byte strings.
    pub argv: Vec<Vec<u8>>,
    /// The script file, when running a file.
    pub script_path: Option<PathBuf>,
    /// The name diagnostics print for the running unit (`/abs/file.php`,
    /// `Command line code`).
    pub script_name: String,
    /// The working directory the request started in.
    pub cwd: PathBuf,
    /// The creating SAPI.
    pub sapi: SapiKind,
    /// `declare(strict_types=1)` default for units that do not declare it.
    pub strict_default: bool,
    pub(crate) in_error_handler: bool,
    test_buf: Option<SharedBuffer>,
}

/// The builtins that take the frameless call path (`FnFlags::LIGHT`):
/// functions of their arguments alone, the ones that dominate call counts
/// in real code. A name here must not reach for the calling frame, call
/// back, write output or modify its arguments; anything it emits or throws
/// re-runs the call on the full path.
const LIGHT_NATIVES: &[&str] = &[
    "strlen", "count", "sizeof", "abs", "max", "min", "intdiv", "floor", "ceil", "round",
    "is_int", "is_integer", "is_long", "is_float", "is_double", "is_string", "is_bool",
    "is_array", "is_object", "is_null", "is_numeric", "is_scalar", "is_iterable",
    "is_countable", "gettype", "get_debug_type", "intval", "floatval", "boolval",
    "strval", "strtolower", "strtoupper", "ucfirst", "lcfirst", "ucwords", "trim",
    "ltrim", "rtrim", "str_repeat", "str_replace", "str_ireplace", "substr", "strpos",
    "stripos", "strrpos", "strripos", "str_contains", "str_starts_with", "str_ends_with",
    "strstr", "stristr", "strrchr", "substr_count", "str_pad", "strrev", "strcmp",
    "strcasecmp", "strncmp", "strncasecmp", "implode", "join", "explode", "sprintf",
    "nl2br", "htmlspecialchars", "htmlentities", "addslashes", "stripslashes",
    "chr", "ord", "dechex", "hexdec", "decbin", "bindec", "decoct", "octdec",
    "array_key_exists", "key_exists", "in_array", "array_search", "array_keys",
    "array_values", "array_merge", "array_slice", "array_reverse", "array_flip",
    "array_key_first", "array_key_last", "array_is_list", "array_sum", "array_product",
    "array_unique", "array_fill", "array_fill_keys", "array_combine", "array_pad",
    "array_column", "array_chunk", "range", "md5", "sha1", "crc32", "hash",
    "base64_encode", "base64_decode", "bin2hex", "hex2bin", "urlencode", "urldecode",
    "rawurlencode", "rawurldecode", "json_encode", "ctype_digit", "ctype_alpha",
    "ctype_alnum", "ctype_space", "ctype_upper", "ctype_lower", "ctype_xdigit",
    "mb_strlen", "mb_substr", "mb_strtolower", "mb_strtoupper", "mb_strpos",
    "str_split", "wordwrap", "number_format", "fmod", "sqrt", "pow", "log", "exp",
    "sin", "cos", "tan", "pi", "is_nan", "is_finite", "is_infinite", "spl_object_id",
    "spl_object_hash", "array_map_keys_placeholder",
];

impl Interp {
    /// A bare interpreter writing to `sink`: no natives, no constants, the
    /// core ini defaults, `error_reporting = E_ALL`, and the engine class
    /// `stdClass`. The SAPI (rphp-embed) populates it through
    /// [`crate::Registry`].
    pub fn new(sink: Box<dyn crate::OutputSink>) -> Interp {
        let mut it = Interp {
            natives: Vec::new(),
            native_index: HashMap::new(),
            constants: HashMap::new(),
            deprecated_constants: HashMap::new(),
            user_constants: Vec::new(),
            units: Vec::new(),
            funcs: Vec::new(),
            func_index: HashMap::new(),
            func_gen: 0,
            classes: Vec::new(),
            class_index: HashMap::new(),
            class_order: Vec::new(),
            class_gen: 0,
            well_known: WellKnown::default(),
            destructibles: Vec::new(),
            in_shutdown: false,
            main_func: None,
            frames: Vec::new(),
            vec_pool: Vec::new(),
            stack: Vec::new(),
            light_native: false,
            reentry_depth: 0,
            included: HashSet::new(),
            generators: Vec::new(),
            fibers: Vec::new(),
            fiber_stack: Vec::new(),
            fiber_suspending: None,
            extensions: Vec::new(),
            boundary_value: None,
            autoloaders: Vec::new(),
            autoloading: Vec::new(),
            compile_hook: None,
            cached_unit_hook: None,
            out: OutputStack::new(sink),
            ini: IniTable::with_core_defaults(),
            error_reporting: E_ALL,
            error_handler: Vec::new(),
            output_started: None,
            pending_site: None,
            head: crate::output::SharedHead::default(),
            request_headers: Vec::new(),
            request_env: None,
            request_body: None,
            error_log: None,
            request_time: None,
            uploaded_files: Vec::new(),
            exception_handler: Vec::new(),
            shutdown: Vec::new(),
            last_error: None,
            silence: 0,
            ext: ExtState::default(),
            resources: ResourceTable::new(),
            object_ids: ObjectIdAllocator::new(),
            globals: Symtab::new(),
            argv: Vec::new(),
            script_path: None,
            script_name: String::from("Command line code"),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            sapi: SapiKind::Embed,
            strict_default: false,
            in_error_handler: false,
            test_buf: None,
        };
        crate::Registry(&mut it)
            .class("stdClass")
            .flags(rphp_bytecode::ClassFlags::ALLOW_DYNAMIC)
            .finish();
        // What `unserialize()` builds for a class the program does not
        // declare: the original name in a property, then whatever the data
        // held. php refuses to touch it afterwards.
        crate::Registry(&mut it)
            .class("__PHP_Incomplete_Class")
            .flags(rphp_bytecode::ClassFlags::ALLOW_DYNAMIC)
            .finish();
        it
    }

    /// An interpreter for unit tests: output goes to an in-memory buffer
    /// readable through [`Interp::test_output`].
    pub fn new_for_tests() -> Interp {
        let buf = SharedBuffer::new();
        let mut it = Interp::new(Box::new(buf.clone()));
        it.test_buf = Some(buf);
        it
    }

    /// Everything written to the test sink so far (staging flushed first).
    /// Empty for an interpreter not created by [`Interp::new_for_tests`].
    pub fn test_output(&mut self) -> Vec<u8> {
        self.out.flush_pending();
        self.test_buf
            .as_ref()
            .map(SharedBuffer::contents)
            .unwrap_or_default()
    }

    /// Everything written to the test sink so far, clearing it.
    pub fn take_test_output(&mut self) -> Vec<u8> {
        self.out.flush_pending();
        self.test_buf
            .as_ref()
            .map(SharedBuffer::take)
            .unwrap_or_default()
    }

    /// Install the entry program: load the unit (declaring its hoisted
    /// functions and classes) and remember its `{main}` for
    /// [`Interp::run_main`]. A redeclaration fault is returned.
    pub fn load_module(&mut self, module: Module) -> Result<(), Unwind> {
        let main = self.load_unit(module)?;
        self.main_func = Some(main);
        Ok(())
    }

    /// Run a php-source prelude: a unit the SAPI declares before the script
    /// (pieces of the standard library written in php). Its classes count
    /// as internal afterwards, and no `{main}` is left loaded.
    pub fn run_prelude(&mut self, module: Module) -> Result<(), Unwind> {
        let before = self.classes.len();
        self.load_module(module)?;
        self.run_main()?;
        self.main_func = None;
        for i in before..self.classes.len() {
            if let Some(c) = Rc::get_mut(&mut self.classes[i]) {
                c.internal = true;
            }
        }
        Ok(())
    }

    /// The entry program's `{main}`, if loaded.
    pub fn main_func(&self) -> Option<&Rc<FuncRt>> {
        self.main_func.map(|id| &self.funcs[id as usize])
    }

    /// Run the loaded program's `{main}` to completion: its frame binds to
    /// the globals table.
    pub fn run_main(&mut self) -> Result<Value, Unwind> {
        let Some(main) = self.main_func else {
            return Err(Unwind::error("No script loaded"));
        };
        let func = self.funcs[main as usize].clone();
        let depth = self.frames.len();
        self.push_user_frame(
            func,
            FrameKind::ReentryBoundary,
            &[],
            None,
            None,
            None,
            RetTarget::Discard,
            Some(self.globals.clone()),
        )?;
        self.run_until(depth)
    }

    /// Register a native; a re-registered name keeps its id.
    pub fn register_native(&mut self, mut f: NativeFn) -> NativeId {
        let key: Box<[u8]> = f.name.as_bytes().to_ascii_lowercase().into_boxed_slice();
        if f.by_ref == 0 && LIGHT_NATIVES.iter().any(|n| n.as_bytes() == &*key) {
            f.flags |= FnFlags::LIGHT;
        }
        if let Some(&id) = self.native_index.get(&key) {
            self.natives[id.0 as usize] = f;
            return id;
        }
        let id = NativeId(self.natives.len() as u32);
        self.natives.push(f);
        self.native_index.insert(key, id);
        self.func_gen += 1;
        id
    }

    /// Resolve a (case-insensitive) function name to its native id.
    pub fn native_by_name(&self, name: &[u8]) -> Option<NativeId> {
        let key = name.to_ascii_lowercase();
        self.native_index.get(key.as_slice()).copied()
    }

    /// The descriptor of a registered native.
    pub fn native(&self, id: NativeId) -> &NativeFn {
        &self.natives[id.0 as usize]
    }

    /// Every registered native, by id.
    pub fn natives(&self) -> &[NativeFn] {
        &self.natives
    }

    /// Whether a function of this name exists (user or native).
    pub fn function_exists(&self, name: &[u8]) -> bool {
        let name = name.strip_prefix(b"\\").unwrap_or(name);
        self.user_function(name).is_some() || self.native_by_name(name).is_some()
    }

    /// Whether a class of this name is declared.
    pub fn class_exists(&self, name: &[u8]) -> bool {
        self.class_by_name(name).is_some()
    }

    /// Fill in the fault site of a pending unwind that has none yet — called
    /// while the faulting frame is still on top.
    pub(crate) fn locate_fault<T>(&self, r: Result<T, Unwind>) -> Result<T, Unwind> {
        match r {
            Err(Unwind::Pending(mut p)) if p.site.is_none() => {
                p.site = Some(Box::new(self.capture_site()));
                Err(Unwind::Pending(p))
            }
            other => other,
        }
    }

    // ---- request end -----------------------------------------------------

    /// Turn the unwind that ended `{main}` into an exit code, rendering an
    /// uncaught fault like php-cli (255).
    pub fn handle_top_level_unwind(&mut self, u: Unwind) -> i32 {
        match u {
            Unwind::Exit(code) => code,
            Unwind::Pending(p) => match self.materialize(p) {
                Ok(o) => self.uncaught_object(o),
                Err(p) => {
                    self.render_uncaught(&p);
                    255
                }
            },
            Unwind::Throw(o) => self.uncaught_object(o),
            Unwind::Retry => unreachable!("a light native's retry never leaves DoCall"),
        }
    }

    /// An uncaught throwable: the `set_exception_handler` callback runs
    /// with it (a throw inside the handler is itself uncaught, rendered
    /// without a handler; a handler that returns yields exit code 0), else
    /// php's uncaught rendering with exit code 255.
    fn uncaught_object(&mut self, o: rphp_value::Object) -> i32 {
        let handler = self
            .exception_handler
            .last()
            .cloned()
            .filter(|h| !matches!(h, Value::Null));
        if let Some(h) = handler {
            let base = self.frames.len();
            let silence = self.silence;
            self.frames.push(Frame::internal(silence));
            // php unsets the handler while it runs.
            self.exception_handler.push(Value::Null);
            let r = self.call_value(&h, &[Value::Object(o)]);
            self.exception_handler.pop();
            self.frames.truncate(base);
            // php: a handler that returns leaves the exit status alone (0).
            return match r {
                Ok(_) => 0,
                Err(Unwind::Exit(code)) => code,
                Err(u) => {
                    let u = self.materialize_unwind(u);
                    match u {
                        Unwind::Throw(e) => self.render_uncaught_object(&e),
                        Unwind::Pending(p) => self.render_uncaught(&p),
                        Unwind::Exit(_) | Unwind::Retry => unreachable!("handled above"),
                    }
                    255
                }
            };
        }
        self.render_uncaught_object(&o);
        255
    }

    /// Run the registered shutdown functions in order (php: a fault or
    /// `exit()` inside one ends the sequence — the remaining ones do not
    /// run — and the exit code becomes 255 / the exit's code).
    pub fn run_shutdown_functions(&mut self, code: i32) -> i32 {
        let mut code = code;
        let base = self.frames.len();
        let silence = self.silence;
        self.frames.push(Frame::internal(silence));
        while !self.shutdown.is_empty() {
            let (cb, args) = self.shutdown.remove(0);
            match self.call_value(&cb, &args) {
                Ok(_) => {}
                Err(u) => {
                    code = self.handle_top_level_unwind(u);
                    break;
                }
            }
        }
        self.frames.truncate(base);
        code
    }

    /// End-of-request output handling: pop and flush every `ob_*` level
    /// (running their handlers with `PHP_OUTPUT_HANDLER_FINAL`), then flush
    /// the sink.
    pub fn finish_output(&mut self) {
        while self.out.level() > 0 {
            let _ = self.ob_flush_top(true);
        }
        self.out.flush_sink();
    }
}
