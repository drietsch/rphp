//! [`Interp`]: all per-request engine state (plan E1, ADR-014). One `Interp`
//! runs one script; the SAPI (through `rphp-embed`) creates it, registers the
//! extension bundle and engine constants, loads the compiled module, runs
//! `{main}`, the shutdown functions and the output flush, and reads the exit
//! code back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use rphp_bytecode::{ClassId, Module};
use rphp_value::{Layout, ObjectIdAllocator, PropMeta, Value};

use crate::errors::E_ALL;
use crate::frames::FrameInfo;
use crate::ini::IniTable;
use crate::output::{OutputStack, SharedBuffer};
use crate::registry::{NativeFn, NativeId, Unwind};
use crate::resources::ResourceTable;
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
}

impl SapiKind {
    /// The `PHP_SAPI` string.
    pub fn name(self) -> &'static str {
        match self {
            SapiKind::Cli => "cli",
            SapiKind::Embed => "embed",
            SapiKind::Server => "cli-server",
        }
    }
}

/// Per-extension state slots the stdlib keeps between calls.
#[derive(Clone, Debug, Default)]
pub struct ExtState {
    /// `preg_last_error()`.
    pub preg_last_error: i64,
    /// `preg_last_error_msg()`.
    pub preg_last_error_msg: String,
    /// `json_last_error()`.
    pub json_last_error: i64,
    /// `json_last_error_msg()`.
    pub json_last_error_msg: String,
}

/// The interpreter: registry, tables, output, diagnostics, and the
/// per-request bookkeeping the engine needs. Fields the stdlib reads and
/// writes directly are public; the registry and frame stack go through the
/// methods in `api.rs`.
pub struct Interp {
    pub(crate) natives: Vec<NativeFn>,
    /// Lowercased name → id.
    pub(crate) native_index: HashMap<Box<[u8]>, NativeId>,
    pub(crate) constants: HashMap<Box<[u8]>, Value>,
    pub(crate) module: Option<Rc<Module>>,
    /// The output stack (`echo`, `ob_*`) over the SAPI's sink.
    pub out: OutputStack,
    /// The ini table.
    pub ini: IniTable,
    /// `error_reporting()`.
    pub error_reporting: i64,
    /// The `set_error_handler` stack: `(callback, level mask)`; a `Null`
    /// callback entry means "no handler" (`set_error_handler(null)`).
    pub error_handler: Vec<(Value, i64)>,
    /// The `set_exception_handler` stack (`Null` = none).
    pub exception_handler: Vec<Value>,
    /// `register_shutdown_function` callbacks with their bound arguments.
    pub shutdown: Vec<(Value, Vec<Value>)>,
    /// `error_get_last()`.
    pub last_error: Option<LastError>,
    /// `@` nesting depth.
    pub silence: u32,
    /// Per-extension state slots.
    pub ext: ExtState,
    /// The resource table.
    pub resources: ResourceTable,
    /// Object handle allocator.
    pub object_ids: ObjectIdAllocator,
    /// One instance layout (and default slots) per class, built lazily.
    pub(crate) layouts: Vec<Option<(Rc<Layout>, Vec<Value>)>>,
    /// Superglobals / globals seeded by the SAPI (`_SERVER`, `argv`, `argc`).
    /// Not visible to PHP code until the compiler lowers global fetches.
    pub globals: HashMap<Box<[u8]>, Value>,
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
    pub(crate) frames: Vec<FrameInfo>,
    pub(crate) in_error_handler: bool,
    test_buf: Option<SharedBuffer>,
}

impl Interp {
    /// A bare interpreter writing to `sink`: no natives, no constants, the
    /// core ini defaults, `error_reporting = E_ALL`. The SAPI (rphp-embed)
    /// populates it through [`crate::Registry`].
    pub fn new(sink: Box<dyn crate::OutputSink>) -> Interp {
        Interp {
            natives: Vec::new(),
            native_index: HashMap::new(),
            constants: HashMap::new(),
            module: None,
            out: OutputStack::new(sink),
            ini: IniTable::with_core_defaults(),
            error_reporting: E_ALL,
            error_handler: Vec::new(),
            exception_handler: Vec::new(),
            shutdown: Vec::new(),
            last_error: None,
            silence: 0,
            ext: ExtState::default(),
            resources: ResourceTable::new(),
            object_ids: ObjectIdAllocator::new(),
            layouts: Vec::new(),
            globals: HashMap::new(),
            argv: Vec::new(),
            script_path: None,
            script_name: String::from("Command line code"),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            sapi: SapiKind::Embed,
            strict_default: false,
            frames: Vec::new(),
            in_error_handler: false,
            test_buf: None,
        }
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

    /// Install the compiled program. Replaces any earlier module (and the
    /// layout cache built for it).
    pub fn load_module(&mut self, module: Module) {
        self.layouts = vec![None; module.classes.len()];
        self.module = Some(Rc::new(module));
    }

    /// The loaded module, if any.
    pub fn module(&self) -> Option<&Rc<Module>> {
        self.module.as_ref()
    }

    /// Run the loaded module's `{main}` to completion.
    pub fn run_main(&mut self) -> Result<Value, Unwind> {
        let Some(module) = self.module.clone() else {
            return Err(Unwind::error("No script loaded"));
        };
        crate::exec::exec_function(self, &module, module.main, &[], None)
    }

    /// Register a native; a re-registered name keeps its id.
    pub fn register_native(&mut self, f: NativeFn) -> NativeId {
        let key: Box<[u8]> = f.name.as_bytes().to_ascii_lowercase().into_boxed_slice();
        if let Some(&id) = self.native_index.get(&key) {
            self.natives[id.0 as usize] = f;
            return id;
        }
        let id = NativeId(self.natives.len() as u32);
        self.natives.push(f);
        self.native_index.insert(key, id);
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

    /// The instance layout and default slot values of `class`, built on
    /// first use from the module's parent-first property set.
    pub(crate) fn instance_layout(
        &mut self,
        module: &Module,
        class: ClassId,
    ) -> (Rc<Layout>, Vec<Value>) {
        let idx = class as usize;
        if idx >= self.layouts.len() {
            self.layouts.resize(idx + 1, None);
        }
        if let Some(cached) = &self.layouts[idx] {
            return cached.clone();
        }
        let mut metas = Vec::new();
        let mut defaults = Vec::new();
        for (name, default, vis) in module.instance_props(class) {
            let decl_class = module.resolve_prop(class, &name).map_or(class, |(_, d)| d);
            metas.push(PropMeta {
                name,
                vis,
                decl_class,
                decl_class_name: Rc::from(&module.class(decl_class).name_bytes[..]),
            });
            defaults.push(default);
        }
        let layout = Rc::new(Layout::new(
            Rc::from(&module.class(class).name_bytes[..]),
            metas,
        ));
        self.layouts[idx] = Some((layout.clone(), defaults.clone()));
        (layout, defaults)
    }

    // ---- frame-info stack ------------------------------------------------

    pub(crate) fn push_frame(&mut self, f: FrameInfo) {
        self.frames.push(f);
    }

    pub(crate) fn pop_frame(&mut self) {
        self.frames.pop();
    }

    pub(crate) fn set_pc(&mut self, pc: usize) {
        if let Some(f) = self.frames.last_mut() {
            f.pc = pc;
        }
    }

    /// The frame-info stack (bottom first).
    pub fn frames(&self) -> &[FrameInfo] {
        &self.frames
    }

    /// Fill in the fault site of a pending unwind that has none yet — called
    /// at every frame boundary while the faulting frame is still on top.
    pub(crate) fn locate_fault<T>(&self, r: Result<T, Unwind>) -> Result<T, Unwind> {
        match r {
            Err(Unwind::Pending(mut p)) if p.site.is_none() => {
                p.site = Some(Box::new(crate::FaultSite {
                    file: self.current_file().to_string(),
                    line: self.current_line(),
                    trace: self.render_trace(),
                }));
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
            Unwind::Pending(p) => {
                self.render_uncaught(&p);
                255
            }
            Unwind::Throw(o) => {
                let p = crate::PendingThrow {
                    kind: crate::ErrorKind::Exception("Exception"),
                    message: format!(
                        "Uncaught exception of class {}",
                        String::from_utf8_lossy(o.layout().class_name())
                    ),
                    site: None,
                };
                self.render_uncaught(&p);
                255
            }
        }
    }

    /// Run the registered shutdown functions in order (php: a fault or
    /// `exit()` inside one ends the sequence — the remaining ones do not
    /// run — and the exit code becomes 255 / the exit's code).
    pub fn run_shutdown_functions(&mut self, code: i32) -> i32 {
        let mut code = code;
        let main = self.module.as_ref().map(|m| m.main);
        let base = self.frames.len();
        if self.frames.is_empty() {
            if let Some(main) = main {
                self.frames.push(FrameInfo::user(main, Vec::new()));
            }
        }
        self.frames.push(FrameInfo::internal());
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
