//! Compiler contract (roadmap F3/F6): the compiler never panics on a parsed
//! program, and everything it refuses is a *known* diagnostic — the not-yet-
//! lowered `RPHP_E0300` or one of the semantic codes it owns.
//!
//! Inputs: every `examples/tier-a/**/*.php` snippet (which must also compile
//! cleanly — they run byte-exact against php) and every `.php` source under
//! `crates/rphp-parser/tests/` except the negative corpus (files the front end
//! rejects are the parser's business and are skipped here).

use std::path::{Path, PathBuf};

use rphp_compiler::{compile, CompileOptions};
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_span::FileId;

/// The codes the compiler may report on a syntactically valid program: its
/// own, and the resolution/validation codes of the `rphp-hir` pass it runs
/// first (php's compile-time fatals).
const KNOWN_CODES: &[&str] = &[
    "RPHP_E0011", // superset rejected with php's text (HIR validation)
    "RPHP_E0025", // strict_types placement (HIR validation)
    "RPHP_E0102", // redeclared function
    "RPHP_E0104", // `[]` read
    "RPHP_E0106", // redeclared class
    "RPHP_E0107", // undefined class
    "RPHP_E0108", // non-constant property default
    "RPHP_E0110", // invalid scope
    "RPHP_E0111", // invalid break/continue
    "RPHP_E0112", // undefined / duplicate label
    "RPHP_E0113", // goto into loop
    "RPHP_E0114", // invalid write target
    "RPHP_E0200", // import conflict (HIR)
    "RPHP_E0201", // reserved class name (HIR)
    "RPHP_E0202", // undefined / duplicate label (HIR)
    "RPHP_E0203", // invalid jump (HIR)
    "RPHP_E0204", // self/static without class scope (HIR)
    "RPHP_E0205", // parent without parent (HIR)
    "RPHP_E0206", // mixed namespace declaration forms (HIR)
    "RPHP_E0300", // not lowered yet
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// Every `.php` file under `dir`, recursively, skipping `skip_dirs` by name.
fn php_files(dir: &Path, skip_dirs: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !skip_dirs.contains(&name) {
                php_files(&path, skip_dirs, out);
            }
        } else if path.extension().is_some_and(|e| e == "php") {
            out.push(path);
        }
    }
}

/// One error the compiler reported: its code and message.
type Reported = (&'static str, String);

/// Parse and compile one file; `Ok(None)` when the front end rejects it,
/// `Ok(Some(errors))` with the compiler's errors otherwise.
fn compile_file(path: &Path) -> Result<Option<Vec<Reported>>, String> {
    let src = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    compile_source(path, &src)
}

/// [`compile_file`] over in-memory source (a `.phpt` `--FILE--` section).
fn compile_source(path: &Path, src: &[u8]) -> Result<Option<Vec<Reported>>, String> {
    let mut interner = Interner::new();
    let opts = ParseOptions {
        file: FileId(0),
        path: Some(path),
        short_open_tag: true,
    };
    let parsed = parse_v2(src, opts, &mut interner);
    if parsed.diagnostics.iter().any(|d| d.is_error()) {
        return Ok(None);
    }
    let line_of = |off: u32| 1 + src[..off as usize].iter().filter(|&&b| b == b'\n').count() as u32;
    let opts = CompileOptions {
        line_of: Some(&line_of),
        file: Some(path.to_path_buf()),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile(parsed.program, &mut interner, &opts)
    }))
    .map_err(|_| format!("{}: the compiler panicked", path.display()))?;
    Ok(Some(match result {
        Ok(_) => Vec::new(),
        Err(diags) => diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| (d.code, d.message.clone()))
            .collect(),
    }))
}

#[test]
fn tier_a_snippets_compile_cleanly() {
    let mut files = Vec::new();
    php_files(&repo_root().join("examples/tier-a"), &[], &mut files);
    assert!(!files.is_empty(), "no tier-a snippets found");
    let mut failures = Vec::new();
    for path in &files {
        match compile_file(path) {
            Ok(Some(errors)) if errors.is_empty() => {}
            Ok(Some(errors)) => failures.push(format!("{}: {errors:?}", path.display())),
            Ok(None) => failures.push(format!("{}: does not parse", path.display())),
            Err(e) => failures.push(e),
        }
    }
    assert!(
        failures.is_empty(),
        "tier-a snippets must compile:\n{}",
        failures.join("\n")
    );
}

#[test]
fn parser_corpus_never_panics_and_only_reports_known_codes() {
    let mut files = Vec::new();
    php_files(&repo_root().join("examples/tier-a"), &[], &mut files);
    php_files(
        &repo_root().join("crates/rphp-parser/tests"),
        &["negative", "invalid", "reject"],
        &mut files,
    );
    let mut failures = Vec::new();
    let (mut compiled, mut refused, mut skipped) = (0usize, 0usize, 0usize);
    for path in &files {
        match compile_file(path) {
            Ok(Some(errors)) if errors.is_empty() => compiled += 1,
            Ok(Some(errors)) => {
                refused += 1;
                for (code, _) in errors {
                    if !KNOWN_CODES.contains(&code) {
                        failures.push(format!("{}: unknown code {code}", path.display()));
                    }
                }
            }
            Ok(None) => skipped += 1,
            Err(e) => failures.push(e),
        }
    }
    println!(
        "contract: {compiled} compiled, {refused} refused with known codes, {skipped} not parsed"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The `--FILE--` section of a `.phpt` test, if it has exactly one.
fn phpt_file_section(text: &[u8]) -> Option<&[u8]> {
    let start = find(text, b"--FILE--\n")? + "--FILE--\n".len();
    let rest = &text[start..];
    let mut end = rest.len();
    let mut off = 0;
    while off < rest.len() {
        let line_end = find(&rest[off..], b"\n").map_or(rest.len(), |i| off + i);
        let line = &rest[off..line_end];
        if line.len() > 4
            && line.starts_with(b"--")
            && line.ends_with(b"--")
            && line[2..line.len() - 2]
                .iter()
                .all(|b| b.is_ascii_uppercase() || *b == b'_')
        {
            end = off;
            break;
        }
        off = line_end + 1;
    }
    Some(&rest[..end])
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Opt-in stress corpus: the `--FILE--` sections of php-src's `Zend/tests`
/// and `tests/lang` (`cargo xtask fetch-php-src`, `$PHP_SRC_DIR` or
/// `vendor-php-src/`). Skipped when the checkout is absent. The compiler must
/// never panic and must only report known codes; how many files it refuses
/// is informational (the engine waves lower them one by one).
#[test]
fn php_src_sweep_never_panics() {
    // php-src has deliberately deep nesting tests (`bug64660.phpt`: hundreds
    // of `[`); like the xtask corpus job, run on a thread with a generous
    // (virtually reserved) stack instead of the 2 MiB test-thread default.
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(php_src_sweep)
        .expect("spawn sweep thread")
        .join()
        .expect("sweep thread panicked");
}

fn php_src_sweep() {
    let root = std::env::var_os("PHP_SRC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("vendor-php-src"));
    if !root.join("Zend/tests").is_dir() {
        eprintln!(
            "skipping php-src sweep: {} has no Zend/tests",
            root.display()
        );
        return;
    }
    let mut files = Vec::new();
    for dir in ["Zend/tests", "tests/lang"] {
        phpt_files(&root.join(dir), &mut files);
    }
    let mut failures = Vec::new();
    let (mut compiled, mut refused, mut skipped) = (0usize, 0usize, 0usize);
    // How many files each not-lowered construct blocks (first E0300 per file).
    let mut blockers: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for path in &files {
        let Ok(text) = std::fs::read(path) else {
            continue;
        };
        let Some(src) = phpt_file_section(&text) else {
            skipped += 1;
            continue;
        };
        if std::env::var_os("RPHP_CONTRACT_TRACE").is_some() {
            eprintln!("sweep: {}", path.display());
        }
        match compile_source(path, src) {
            Ok(Some(errors)) if errors.is_empty() => compiled += 1,
            Ok(Some(errors)) => {
                refused += 1;
                if let Some((_, msg)) = errors.iter().find(|(code, _)| *code == "RPHP_E0300") {
                    let what = msg
                        .trim_start_matches("unsupported construct: ")
                        .trim_end_matches(" (not lowered yet)")
                        .to_string();
                    *blockers.entry(what).or_default() += 1;
                }
                for (code, _) in errors {
                    if !KNOWN_CODES.contains(&code) {
                        failures.push(format!("{}: unknown code {code}", path.display()));
                    }
                }
            }
            Ok(None) => skipped += 1,
            Err(e) => failures.push(e),
        }
    }
    println!(
        "php-src sweep: {} files, {compiled} compiled, {refused} refused with known codes, {skipped} skipped (no/unparsed --FILE--)",
        files.len()
    );
    let mut top: Vec<(String, usize)> = blockers.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (what, n) in top.iter().take(25) {
        println!("  {n:5}  {what}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn phpt_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            phpt_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "phpt") {
            out.push(path);
        }
    }
}
