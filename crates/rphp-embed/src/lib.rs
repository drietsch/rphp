//! The embedding API every SAPI sits on (spec 09, ADR-014/033): wires the
//! front end (`rphp-lexer`/`rphp-parser`/`rphp-compiler`), the engine
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

use std::path::{Path, PathBuf};

use rphp_bytecode::Module;
use rphp_compiler::{compile, CompileOptions, KnownFunctions, NativeSig};
use rphp_diagnostics::Diagnostic;
use rphp_intern::Interner;
use rphp_parser::parse;
use rphp_source::SourceMap;
use rphp_value::{Array, ArrayKey, Value};

pub use constants::{php_os, php_os_family};
pub use rphp_runtime::{Interp, OutputSink, Registry, SapiKind, Unwind};
pub use sink::{BufferSink, StdoutSink};

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
}

/// The per-process engine: creates interpreters, compiles and runs code.
pub struct Engine {
    config: EngineConfig,
}

/// The compiler's view of an interpreter's registry.
struct InterpNatives<'a>(&'a Interp);

impl KnownFunctions for InterpNatives<'_> {
    fn native(&self, name: &[u8]) -> Option<NativeSig> {
        let id = self.0.native_by_name(name)?;
        let f = self.0.native(id);
        Some(NativeSig {
            id: id.0,
            min_args: f.min_args as usize,
            max_args: f.max_args.map(usize::from),
            by_ref: f.by_ref,
        })
    }
}

impl Engine {
    /// An engine with `config`.
    pub fn new(config: EngineConfig) -> Engine {
        Engine { config }
    }

    /// The configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// A fresh interpreter writing to `sink`: the stdlib and the engine
    /// constants registered, the ini defaults with the configured overrides
    /// applied, `$_SERVER`/`$argv`/`$argc` seeded (not yet visible to PHP
    /// code — the compiler does not lower global fetches).
    pub fn new_interp(&self, sink: Box<dyn OutputSink>) -> Interp {
        let cfg = &self.config;
        let mut it = Interp::new(sink);
        it.sapi = cfg.sapi;
        if let Some(cwd) = &cfg.cwd {
            it.cwd = cwd.clone();
        }
        it.argv = cfg.argv.iter().map(|s| s.as_bytes().to_vec()).collect();
        it.script_path = cfg.script_path.clone();
        for (k, v) in &cfg.ini {
            // `-d` accepts unknown directives too (php stores them as-is).
            if it.ini_set(k, v).is_none() {
                it.ini.register(k, v);
            }
        }
        rphp_stdlib::register(&mut Registry(&mut it));
        constants::register(&mut Registry(&mut it), cfg.sapi);
        self.seed_globals(&mut it);
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
        it.globals
            .insert(Box::from(&b"_SERVER"[..]), Value::Array(server));
        it.globals
            .insert(Box::from(&b"argv"[..]), Value::Array(argv));
        it.globals.insert(Box::from(&b"argc"[..]), argc);
    }

    /// Parse and compile `src` (named `name` in diagnostics) against
    /// `interp`'s registry, with line tables. On failure the rendered
    /// diagnostics, one per element. A parse error aborts before compilation.
    pub fn compile(&self, interp: &Interp, src: &[u8], name: &str) -> Result<Module, Vec<String>> {
        let mut sources = SourceMap::new();
        let id = sources.add(name.to_string(), src.to_vec());
        let mut interner = Interner::new();
        let (program, diags) = parse(src, id, &mut interner);
        let render =
            |diags: &[Diagnostic]| diags.iter().map(|d| d.render(&sources)).collect::<Vec<_>>();
        if diags.iter().any(Diagnostic::is_error) {
            return Err(render(&diags));
        }
        let file = sources.get(id);
        let line_of = |offset: u32| file.line_col(offset).0;
        let natives = InterpNatives(interp);
        let opts = CompileOptions {
            natives: &natives,
            line_of: Some(&line_of),
        };
        compile(&program, &interner, &opts).map_err(|d| render(&d))
    }

    /// Compile `src` and install it as `interp`'s program.
    pub fn load(&self, interp: &mut Interp, src: &[u8], name: &str) -> Result<(), Vec<String>> {
        let module = self.compile(interp, src, name)?;
        interp.script_name = name.to_string();
        interp.load_module(module);
        Ok(())
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
        interp.finish_output();
        code
    }

    /// Compile and run `src` as the unit named `name` (`Command line code`
    /// for `-r`), writing to `sink`. Compile errors go to stderr (exit 255,
    /// php's code for a fatal compile error).
    pub fn run_code(&self, src: &[u8], name: &str, sink: Box<dyn OutputSink>) -> i32 {
        let mut interp = self.new_interp(sink);
        if let Err(lines) = self.load(&mut interp, src, name) {
            for line in lines {
                eprintln!("{line}");
            }
            return 255;
        }
        self.execute(&mut interp)
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
        let mut interp = self.new_interp(sink);
        interp.script_path = Some(name.clone());
        if let Err(lines) = self.load(&mut interp, &bytes, &name.to_string_lossy()) {
            for line in lines {
                eprintln!("{line}");
            }
            return 255;
        }
        self.execute(&mut interp)
    }
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
    let mut interp = engine.new_interp(Box::new(buffer.clone()));
    engine
        .load(&mut interp, src, "Command line code")
        .map_err(|lines| lines.join("\n"))?;
    match interp.run_main() {
        Ok(_) | Err(Unwind::Exit(_)) => {}
        Err(u) => return Err(u.describe()),
    }
    interp.finish_output();
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
        assert_eq!(it.globals.get(&b"argc"[..]), Some(&Value::Int(2)));
        assert_eq!(it.constant(b"PHP_SAPI"), Some(Value::string(b"cli")));
        it.warn("hidden").unwrap();
        assert_eq!(buffer.take(), b"");
    }
}
