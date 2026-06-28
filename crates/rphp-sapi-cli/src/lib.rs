//! The CLI SAPI: wires lexer -> parser -> compiler -> runtime, plus `--emit`
//! pipeline dumps.
//!
//! [`run`] is the entry point `tools/rphp` calls; keep its signature stable.
//! Usage:
//!   rphp <file.php>                  run a script
//!   rphp run <file.php>              run a script
//!   rphp --emit=tokens <file.php>    dump the token stream
//!   rphp --emit=ast <file.php>       dump the AST
//!   rphp --emit=bytecode <file.php>  dump the bytecode module
#![forbid(unsafe_code)]

use rphp_bytecode::Module;
use rphp_compiler::compile;
use rphp_diagnostics::Diagnostic;
use rphp_intern::Interner;
use rphp_lexer::lex;
use rphp_parser::parse;
use rphp_source::SourceMap;

const USAGE: &str = "\
rphp — a clean-room PHP 8.5 engine (M0)

USAGE:
    rphp <file.php>                  run a PHP script
    rphp run <file.php>              run a PHP script
    rphp --emit=tokens <file.php>    dump the token stream
    rphp --emit=ast <file.php>       dump the parsed AST
    rphp --emit=bytecode <file.php>  dump the compiled bytecode module
    rphp --help | -h                 show this help
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EmitKind {
    Tokens,
    Ast,
    Bytecode,
}

/// Run the CLI with the given args (excluding argv[0]); returns a process exit
/// code. The binary calls `std::process::exit` with the returned value, so this
/// function never exits the process itself.
pub fn run(args: Vec<String>) -> i32 {
    // No arguments at all: friendly help on stdout.
    if args.is_empty() {
        print!("{USAGE}");
        return 0;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return 0;
    }

    let mut emit: Option<EmitKind> = None;
    let mut file: Option<String> = None;

    for a in &args {
        // A leading `run` sub-command is accepted and ignored.
        if a == "run" && file.is_none() {
            continue;
        }
        if let Some(rest) = a.strip_prefix("--emit=") {
            match rest {
                "tokens" => emit = Some(EmitKind::Tokens),
                "ast" => emit = Some(EmitKind::Ast),
                "bytecode" => emit = Some(EmitKind::Bytecode),
                other => {
                    eprintln!("rphp: unknown emit kind `{other}`\n");
                    eprint!("{USAGE}");
                    return 2;
                }
            }
        } else if a.starts_with('-') && a != "-" {
            eprintln!("rphp: unknown flag `{a}`\n");
            eprint!("{USAGE}");
            return 2;
        } else if file.is_none() {
            file = Some(a.clone());
        } else {
            eprintln!("rphp: unexpected extra argument `{a}`\n");
            eprint!("{USAGE}");
            return 2;
        }
    }

    let Some(file) = file else {
        eprintln!("rphp: no input file given\n");
        eprint!("{USAGE}");
        return 1;
    };

    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("rphp: cannot read `{file}`: {e}");
            return 1;
        }
    };

    match emit {
        Some(EmitKind::Tokens) => emit_tokens(&file, &bytes),
        Some(EmitKind::Ast) => emit_ast(&file, &bytes),
        Some(EmitKind::Bytecode) => emit_bytecode(&file, &bytes),
        None => run_file(&file, &bytes),
    }
}

/// Parse + compile `bytes` into a [`Module`]. On failure, returns the rendered
/// diagnostic lines (already mapped to source positions). Parse errors abort
/// before compilation, matching PHP's "syntax error => fatal" behavior.
fn compile_to_module(name: &str, bytes: &[u8]) -> Result<(SourceMap, Module), Vec<String>> {
    let mut sources = SourceMap::new();
    let id = sources.add(name.to_string(), bytes.to_vec());
    let mut interner = Interner::new();

    let (program, diags) = parse(bytes, id, &mut interner);
    if diags.iter().any(Diagnostic::is_error) {
        return Err(render_all(&diags, &sources));
    }

    match compile(&program, &interner) {
        Ok(module) => Ok((sources, module)),
        Err(diags) => Err(render_all(&diags, &sources)),
    }
}

/// Render every diagnostic against the source map, one entry per element.
fn render_all(diags: &[Diagnostic], sources: &SourceMap) -> Vec<String> {
    diags.iter().map(|d| d.render(sources)).collect()
}

/// Run a script end-to-end, printing its output to real stdout.
fn run_file(name: &str, bytes: &[u8]) -> i32 {
    let (_sources, module) = match compile_to_module(name, bytes) {
        Ok(pair) => pair,
        Err(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
            return 1;
        }
    };

    match rphp_runtime::run(&module) {
        Ok(out) => {
            // `echo` output is raw bytes (PHP strings are byte strings), so write
            // it through directly rather than via a UTF-8 `print!`.
            use std::io::Write;
            let _ = std::io::stdout().write_all(&out.stdout);
            0
        }
        Err(err) => {
            // PHP surfaces uncaught runtime faults as a fatal error and exits 255.
            eprintln!("PHP Fatal error:  Uncaught Error: {}", err.message);
            255
        }
    }
}

/// `--emit=tokens`: one `TokenKind` (debug form) per line on stdout.
fn emit_tokens(name: &str, bytes: &[u8]) -> i32 {
    let mut sources = SourceMap::new();
    let id = sources.add(name.to_string(), bytes.to_vec());
    let mut interner = Interner::new();

    let result = lex(bytes, id, &mut interner);
    for tok in &result.tokens {
        println!("{:?}", tok.kind);
    }
    for d in &result.diagnostics {
        eprintln!("{}", d.render(&sources));
    }
    0
}

/// `--emit=ast`: pretty-debug dump of the parsed `Program` on stdout.
fn emit_ast(name: &str, bytes: &[u8]) -> i32 {
    let mut sources = SourceMap::new();
    let id = sources.add(name.to_string(), bytes.to_vec());
    let mut interner = Interner::new();

    let (program, diags) = parse(bytes, id, &mut interner);
    println!("{program:#?}");
    for d in &diags {
        eprintln!("{}", d.render(&sources));
    }
    0
}

/// `--emit=bytecode`: pretty-debug dump of the compiled `Module` on stdout.
fn emit_bytecode(name: &str, bytes: &[u8]) -> i32 {
    match compile_to_module(name, bytes) {
        Ok((_sources, module)) => {
            println!("{module:#?}");
            0
        }
        Err(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
            1
        }
    }
}

/// Evaluate PHP source through the full parse -> compile -> run pipeline and
/// return the captured `echo` output. Diagnostics and runtime faults are
/// returned as the `Err` string. Intended for tests and embedders that want the
/// output as a value rather than printed to stdout.
pub fn eval_to_string(src: &[u8]) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&eval_to_bytes(src)?).into_owned())
}

/// As [`eval_to_string`], but returns the raw (binary-safe) `echo` bytes.
pub fn eval_to_bytes(src: &[u8]) -> Result<Vec<u8>, String> {
    let (_sources, module) =
        compile_to_module("<eval>", src).map_err(|lines| lines.join("\n"))?;
    let out = rphp_runtime::run(&module).map_err(|e| e.message)?;
    Ok(out.stdout)
}
