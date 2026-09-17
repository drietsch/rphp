//! The CLI SAPI over `rphp-embed`: php's argument grammar (the subset below),
//! streaming output straight to stdout, php-cli's error display and exit
//! codes, plus the `--emit` pipeline dumps and `-l`.
//!
//! [`run`] is the entry point `tools/rphp` calls; keep its signature stable.
//! Usage:
//!   rphp [options] [-f] <file.php> [--] [args...]   run a script
//!   rphp [options] -r <code> [--] [args...]         run code (`Command line code`)
//!   rphp run <file.php>                             run a script
//!   rphp -d key=value                               set an ini directive (repeatable)
//!   rphp -l <file.php>                              syntax check only (php's `-l`)
//!   rphp --emit=tokens|ast|bytecode <file.php>      dump a pipeline stage
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use rphp_embed::{Engine, EngineConfig, StdoutSink};
use rphp_intern::Interner;
use rphp_lexer::lex;
use rphp_parser::parse;
use rphp_source::SourceMap;

const USAGE: &str = "\
rphp — a clean-room PHP 8.5 engine

USAGE:
    rphp [options] [-f] <file.php> [--] [args...]   run a PHP script
    rphp [options] -r <code> [--] [args...]         run PHP code (`Command line code`)
    rphp run <file.php>                             run a PHP script

OPTIONS:
    -d key[=value]                   set an ini directive (repeatable; `-d display_errors=0`)
    -f <file>                        the script to run
    -r <code>                        run <code> without the opening `<?php` tag
    -n                               accepted for php compatibility (no php.ini is read anyway)
    -l | --lint <file.php>           syntax check only; exit 0 or 255 like `php -l`
    --emit=tokens <file.php>         dump the token stream
    --emit=ast <file.php>            dump the parsed AST
    --emit=bytecode <file.php>       dump the compiled bytecode module
    --                               end of options; the rest is the script's $argv
    --help | -h                      show this help

Everything after the script name is passed to the script as arguments.
Exit codes: 0 success, 255 fatal error / parse error (or the script's exit()),
1 unreadable file, 2 usage error.
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EmitKind {
    Tokens,
    Ast,
    Bytecode,
}

/// The parsed command line.
#[derive(Default, Debug)]
struct Cli {
    emit: Option<EmitKind>,
    lint_only: bool,
    file: Option<String>,
    code: Option<String>,
    ini: Vec<(String, String)>,
    script_args: Vec<String>,
}

/// Parse `args` (excluding argv[0]); `Err(code)` when help was printed (0)
/// or the arguments are invalid (2).
fn parse_args(args: &[String]) -> Result<Cli, i32> {
    let mut cli = Cli::default();
    let mut i = 0;
    let mut after_dd = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        // php: everything after the script name (or after `--`) is the script's.
        if after_dd || cli.file.is_some() {
            cli.script_args.push(a.clone());
            continue;
        }
        match a.as_str() {
            "--" => after_dd = true,
            "--help" | "-h" => {
                print!("{USAGE}");
                return Err(0);
            }
            "run" if cli.code.is_none() => {}
            "-l" | "--lint" => cli.lint_only = true,
            "-n" => {}
            "-d" | "-r" | "-f" => {
                let Some(v) = args.get(i) else {
                    eprintln!("rphp: `{a}` needs an argument\n");
                    eprint!("{USAGE}");
                    return Err(2);
                };
                i += 1;
                apply_option(&mut cli, a, v);
            }
            _ if a.starts_with("--emit=") => {
                cli.emit = Some(match &a["--emit=".len()..] {
                    "tokens" => EmitKind::Tokens,
                    "ast" => EmitKind::Ast,
                    "bytecode" => EmitKind::Bytecode,
                    other => {
                        eprintln!("rphp: unknown emit kind `{other}`\n");
                        eprint!("{USAGE}");
                        return Err(2);
                    }
                });
            }
            _ if a.starts_with("-d") || a.starts_with("-r") || a.starts_with("-f") => {
                let (flag, v) = a.split_at(2);
                apply_option(&mut cli, flag, v);
            }
            _ if a.starts_with('-') && a != "-" => {
                eprintln!("rphp: unknown flag `{a}`\n");
                eprint!("{USAGE}");
                return Err(2);
            }
            _ => {
                if cli.code.is_some() {
                    cli.script_args.push(a.clone());
                } else {
                    cli.file = Some(a.clone());
                }
            }
        }
    }
    Ok(cli)
}

fn apply_option(cli: &mut Cli, flag: &str, value: &str) {
    match flag {
        "-d" => {
            let (k, v) = match value.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                // php: `-d name` alone sets the directive to "1".
                None => (value.to_string(), "1".to_string()),
            };
            cli.ini.push((k, v));
        }
        "-r" => cli.code = Some(value.to_string()),
        _ => cli.file = Some(value.to_string()),
    }
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
    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };

    if let Some(code) = &cli.code {
        if cli.lint_only || cli.emit.is_some() {
            eprintln!("rphp: `-r` cannot be combined with `-l` or `--emit`\n");
            eprint!("{USAGE}");
            return 2;
        }
        let mut argv = vec!["Standard input code".to_string()];
        argv.extend(cli.script_args.iter().cloned());
        let engine = Engine::new(EngineConfig {
            argv,
            ini: cli.ini.clone(),
            ..EngineConfig::cli()
        });
        let src = format!("<?php {code}");
        return engine.run_code(
            src.as_bytes(),
            "Command line code",
            Box::new(StdoutSink::new()),
        );
    }

    let Some(file) = cli.file.clone() else {
        eprintln!("rphp: no input file given\n");
        eprint!("{USAGE}");
        return 1;
    };
    if cli.lint_only && cli.emit.is_some() {
        eprintln!("rphp: `-l` cannot be combined with `--emit`\n");
        eprint!("{USAGE}");
        return 2;
    }

    if cli.lint_only || cli.emit.is_some() {
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(_) => {
                println!("Could not open input file: {file}");
                return 1;
            }
        };
        if cli.lint_only {
            return lint_file(&file, &bytes);
        }
        return match cli.emit {
            Some(EmitKind::Tokens) => emit_tokens(&file, &bytes),
            Some(EmitKind::Ast) => emit_ast(&file, &bytes),
            Some(EmitKind::Bytecode) => emit_bytecode(&file, &bytes),
            None => unreachable!(),
        };
    }

    let mut argv = vec![file.clone()];
    argv.extend(cli.script_args.iter().cloned());
    let engine = Engine::new(EngineConfig {
        argv,
        ini: cli.ini.clone(),
        script_path: Some(PathBuf::from(&file)),
        ..EngineConfig::cli()
    });
    engine.run_file(Path::new(&file), Box::new(StdoutSink::new()))
}

/// Parse `bytes` (named `name` in diagnostics) without compiling or running.
///
/// `Ok(())` when the front end reports no error-severity diagnostic, else the
/// rendered diagnostics in order. This is what `rphp -l` prints; embedders and
/// tests can call it directly.
pub fn lint(name: &str, bytes: &[u8]) -> Result<(), Vec<String>> {
    let mut sources = SourceMap::new();
    let id = sources.add(name.to_string(), bytes.to_vec());
    let mut interner = Interner::new();

    let (_program, diags) = parse(bytes, id, &mut interner);
    let errors: Vec<String> = diags
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.render(&sources))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// `rphp -l <file>`: syntax check only, mirroring `php -l`. Prints
/// `No syntax errors detected in <file>` and returns 0, or the diagnostics on
/// stderr followed by `Errors parsing <file>` on stdout and returns 255 (php's
/// exit code for a parse failure).
fn lint_file(name: &str, bytes: &[u8]) -> i32 {
    match lint(name, bytes) {
        Ok(()) => {
            println!("No syntax errors detected in {name}");
            0
        }
        Err(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
            println!("Errors parsing {name}");
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

/// `--emit=bytecode`: pretty-debug dump of the compiled `Module` on stdout,
/// compiled against the engine's registry (so `CallNative` ids are the ones
/// a run would use).
fn emit_bytecode(name: &str, bytes: &[u8]) -> i32 {
    let engine = Engine::new(EngineConfig::cli());
    let interp = engine.new_interp(Box::new(StdoutSink::new()));
    match engine.compile(&interp, bytes, name) {
        Ok(module) => {
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
    rphp_embed::eval_to_string(src)
}

/// As [`eval_to_string`], but returns the raw (binary-safe) `echo` bytes.
pub fn eval_to_bytes(src: &[u8]) -> Result<Vec<u8>, String> {
    rphp_embed::eval_to_bytes(src)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn php_argument_grammar() {
        let cli = parse_args(&args(&[
            "-d",
            "display_errors=0",
            "-dprecision=10",
            "-n",
            "s.php",
            "-x",
            "--",
            "y",
        ]))
        .unwrap();
        assert_eq!(cli.file.as_deref(), Some("s.php"));
        assert_eq!(
            cli.ini,
            vec![
                ("display_errors".into(), "0".into()),
                ("precision".into(), "10".into())
            ]
        );
        assert_eq!(cli.script_args, args(&["-x", "--", "y"]));
        let cli = parse_args(&args(&["-r", "echo 1;", "--", "a", "b"])).unwrap();
        assert_eq!(cli.code.as_deref(), Some("echo 1;"));
        assert_eq!(cli.script_args, args(&["a", "b"]));
        let cli = parse_args(&args(&["-f", "s.php", "q"])).unwrap();
        assert_eq!(cli.file.as_deref(), Some("s.php"));
        assert_eq!(cli.script_args, args(&["q"]));
        let cli = parse_args(&args(&["run", "s.php"])).unwrap();
        assert_eq!(cli.file.as_deref(), Some("s.php"));
        assert_eq!(parse_args(&args(&["--nope"])).unwrap_err(), 2);
        assert_eq!(parse_args(&args(&["-d"])).unwrap_err(), 2);
        let cli = parse_args(&args(&["-d", "flag"])).unwrap();
        assert_eq!(cli.ini, vec![("flag".into(), "1".into())]);
    }
}
