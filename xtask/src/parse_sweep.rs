//! `cargo xtask parse-sweep` — parse-only sweep over php-src's `.phpt` tests.
//!
//! Walks `vendor-php-src/Zend/tests` and `vendor-php-src/tests/lang` (the
//! sparse checkout produced by `tools/php-src/checkout.sh`), extracts each
//! test's `--FILE--` section and classifies the expected outcome from the
//! `--EXPECT*--` section: *parse error* when it begins with `Parse error` or
//! mentions `syntax error` in its first two lines (unless the error is raised
//! from `eval()'d code`, in which case the file itself parses), else *parses*.
//! Our front end's verdict is compared against that classification; the report
//! has the same shape as `cargo xtask corpus` (false rejects grouped by first
//! diagnostic, false accepts, panics, timing) and a JSON copy is written to
//! `target/parse-sweep-report.json`.
//!
//! The report machinery is shared with `corpus.rs`; `our_verdict` is
//! deliberately duplicated so F2 repoints each job independently.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::corpus::{
    discover, finish, guarded, install_quiet_panic_hook, parse_common, repo_root, select,
    summarize, target_dir, thread_pool, Outcome, Timing, Verdict,
};
use crate::XtaskResult;

const HELP: &str = "\
cargo xtask parse-sweep — parse-only sweep over php-src Zend/tests + tests/lang

USAGE:
    cargo xtask parse-sweep [options]

OPTIONS:
    --php-src <root>     php-src checkout (default: $RPHP_PHP_SRC_DIR or vendor-php-src)
    --dir <path>         sweep this directory of .phpt files instead (repeatable)
    --jobs <N>           worker threads (default: all cores)
    --limit <N>          only the first N tests (after sorting/filtering)
    --filter <substr>    only paths containing <substr>
    --report <text|json> report format on stdout (default: text)
    --top <N>            false-reject groups to print (default: 20)
    --stack-mb <N>       worker thread stack in MiB (default: 1024; deep nesting)
    --allow-failures     exit 0 even when verdicts disagree
    -h, --help           this help

The JSON report is always written to target/parse-sweep-report.json.
Fetch the corpus with tools/php-src/checkout.sh (or `cargo xtask fetch-php-src`).
";

const DEFAULT_SUBDIRS: [&str; 2] = ["Zend/tests", "tests/lang"];

/// Parse `src` with the current front end and reduce the result to a verdict.
///
/// Duplicated from `corpus.rs` on purpose (see the module docs); F2 repoints
/// the body at the mago adapter.
fn our_verdict(src: &[u8]) -> Result<(), Vec<String>> {
    let mut sources = rphp_source::SourceMap::new();
    let id = sources.add("<phpt>", src.to_vec());
    let mut interner = rphp_intern::Interner::new();
    let (_program, diags) = rphp_parser::parse(src, id, &mut interner);
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

// ---------------------------------------------------------------------------
// .phpt section extraction
// ---------------------------------------------------------------------------

/// One `--NAME--` section: its name and raw body (lines up to the next header).
struct Section<'a> {
    name: &'a str,
    body: &'a [u8],
}

/// `--NAME--` header lines, as run-tests.php matches them (`^--([_A-Z]+)--`).
fn header_name(line: &[u8]) -> Option<&str> {
    let rest = line.strip_prefix(b"--")?;
    let end = rest
        .iter()
        .position(|b| !(b.is_ascii_uppercase() || *b == b'_'))?;
    if end == 0 || !rest[end..].starts_with(b"--") {
        return None;
    }
    std::str::from_utf8(&rest[..end]).ok()
}

/// Split a `.phpt` into sections, byte-exact. Lines keep their terminators.
fn sections(bytes: &[u8]) -> Vec<Section<'_>> {
    let mut out: Vec<Section<'_>> = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let end = bytes[pos..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| pos + i + 1)
            .unwrap_or(bytes.len());
        let line = &bytes[pos..end];
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
        if let Some(name) = header_name(trimmed) {
            out.push(Section {
                name,
                body: &bytes[end..end],
            });
        } else if let Some(last) = out.last_mut() {
            let start = last.body.as_ptr() as usize - bytes.as_ptr() as usize;
            last.body = &bytes[start..end];
        }
        pos = end;
    }
    out
}

/// Why a test was skipped by the sweep (not a parity failure).
#[derive(Debug, PartialEq, Eq)]
enum Skip {
    NoFileSection,
    ExternalFileMissing,
}

/// The extracted test: source to parse and the expectation derived from the
/// `--EXPECT*--` section.
struct Case {
    source: Vec<u8>,
    expected: Verdict,
}

/// First two non-blank lines of the expectation, joined.
fn expect_head(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Classify the expectation head: `Verdict::Err` for a file-level parse error.
fn classify(head: &str) -> Verdict {
    let first = head.lines().next().unwrap_or("");
    let first = first.strip_prefix("PHP ").unwrap_or(first);
    let is_parse = first.starts_with("Parse error") || head.contains("syntax error");
    if !is_parse {
        return Verdict::Ok;
    }
    // A parse error inside eval()'d code means the file itself parsed.
    let error_line = head
        .lines()
        .find(|l| l.contains("Parse error") || l.contains("syntax error"))
        .unwrap_or(first);
    if error_line.contains("eval()'d code") {
        return Verdict::Ok;
    }
    Verdict::Err(error_line.to_string())
}

fn extract(bytes: &[u8], path: &Path) -> Result<Case, Skip> {
    let secs = sections(bytes);
    let mut source: Option<Vec<u8>> = None;
    for s in &secs {
        match s.name {
            "FILE" => source = Some(s.body.to_vec()),
            "FILEEOF" => {
                let body = s.body.strip_suffix(b"\n").unwrap_or(s.body);
                let body = body.strip_suffix(b"\r").unwrap_or(body);
                source = Some(body.to_vec());
            }
            "FILE_EXTERNAL" => {
                let rel = String::from_utf8_lossy(s.body).trim().to_string();
                let dir = path.parent().unwrap_or(Path::new("."));
                source = Some(std::fs::read(dir.join(rel)).map_err(|_| Skip::ExternalFileMissing)?);
            }
            _ => {}
        }
        if source.is_some() {
            break;
        }
    }
    let source = source.ok_or(Skip::NoFileSection)?;
    let expected = secs
        .iter()
        .find(|s| matches!(s.name, "EXPECT" | "EXPECTF" | "EXPECTREGEX"))
        .map(|s| classify(&expect_head(s.body)))
        .unwrap_or(Verdict::Ok);
    Ok(Case { source, expected })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the parse sweep.
pub fn run(args: &[String]) -> XtaskResult {
    let mut a = pico_args::Arguments::from_vec(args.iter().map(OsString::from).collect());
    if a.contains(["-h", "--help"]) {
        print!("{HELP}");
        return Ok(());
    }
    let php_src: PathBuf = a
        .opt_value_from_str("--php-src")?
        .or_else(|| std::env::var_os("RPHP_PHP_SRC_DIR").map(PathBuf::from))
        .unwrap_or_else(|| repo_root().join("vendor-php-src"));
    let mut dirs: Vec<PathBuf> = a.values_from_str("--dir")?;
    let common = parse_common(&mut a)?;
    let rest = a.finish();
    if !rest.is_empty() {
        eprint!("{HELP}");
        return Err(format!("unexpected arguments: {rest:?}").into());
    }
    if dirs.is_empty() {
        dirs = DEFAULT_SUBDIRS.iter().map(|d| php_src.join(d)).collect();
    }
    for d in &dirs {
        if !d.is_dir() {
            return Err(format!(
                "missing php-src test directory {}\n\
                 fetch the pinned corpus with `tools/php-src/checkout.sh` \
                 (or `cargo xtask fetch-php-src`), or point --php-src / \
                 RPHP_PHP_SRC_DIR at an existing checkout",
                d.display()
            )
            .into());
        }
    }

    let started = Instant::now();
    let (files, _) = discover(&dirs, "phpt", true);
    let files = select(files, &common);
    let discover_ms = started.elapsed().as_millis();
    if files.is_empty() {
        return Err("no .phpt files found".into());
    }

    let pool = thread_pool(&common)?;
    let threads = pool.current_num_threads();
    install_quiet_panic_hook();

    struct Row {
        outcome: Option<Outcome>,
        skipped: Option<Skip>,
        ours: Duration,
    }

    let rows: Vec<Result<Row, String>> = pool.install(|| {
        files
            .par_iter()
            .map(|path| {
                let display = path.to_string_lossy().into_owned();
                let bytes = std::fs::read(path).map_err(|e| format!("{display}: {e}"))?;
                let case = match extract(&bytes, path) {
                    Ok(c) => c,
                    Err(skip) => {
                        return Ok(Row {
                            outcome: None,
                            skipped: Some(skip),
                            ours: Duration::ZERO,
                        })
                    }
                };
                let t = Instant::now();
                let (ours, panicked) = guarded(|| our_verdict(&case.source));
                Ok(Row {
                    outcome: Some(Outcome {
                        path: display,
                        expected: case.expected,
                        ours,
                        panicked,
                        note: None,
                    }),
                    skipped: None,
                    ours: t.elapsed(),
                })
            })
            .collect()
    });
    let _ = std::panic::take_hook();

    let mut timing = Timing {
        discover_ms,
        threads,
        ..Timing::default()
    };
    let mut outcomes = Vec::with_capacity(rows.len());
    let mut skipped = 0usize;
    let mut ours_total = Duration::ZERO;
    for row in rows {
        let row = row?;
        ours_total += row.ours;
        match row.outcome {
            Some(o) => outcomes.push(o),
            None => {
                let _ = row.skipped;
                skipped += 1;
            }
        }
    }
    timing.ours_ms = ours_total.as_millis();
    timing.total_ms = started.elapsed().as_millis();

    let roots = dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    let report = summarize("parse-sweep", roots, skipped, &outcomes, common.top, timing);
    finish(
        &report,
        "expect",
        common.json,
        &target_dir().join("parse-sweep-report.json"),
        common.allow_failures,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_names() {
        assert_eq!(header_name(b"--FILE--"), Some("FILE"));
        assert_eq!(header_name(b"--EXPECTF--"), Some("EXPECTF"));
        assert_eq!(header_name(b"--FILE_EXTERNAL--"), Some("FILE_EXTERNAL"));
        assert_eq!(header_name(b"--file--"), None);
        assert_eq!(header_name(b"-- not --"), None);
        assert_eq!(header_name(b"----"), None);
        assert_eq!(header_name(b"--TEST-- trailing junk"), Some("TEST"));
    }

    #[test]
    fn sections_are_byte_exact() {
        let src = b"--TEST--\nname\n--FILE--\n<?php\necho 1;\n--EXPECT--\n1\n";
        let s = sections(src);
        let names: Vec<&str> = s.iter().map(|x| x.name).collect();
        assert_eq!(names, ["TEST", "FILE", "EXPECT"]);
        assert_eq!(s[1].body, b"<?php\necho 1;\n");
        assert_eq!(s[2].body, b"1\n");
    }

    #[test]
    fn classify_expectations() {
        assert_eq!(classify(""), Verdict::Ok);
        assert_eq!(classify("int(1)\nint(2)"), Verdict::Ok);
        assert!(matches!(
            classify("Parse error: syntax error, unexpected token \"}\" in %s on line 3"),
            Verdict::Err(_)
        ));
        assert!(matches!(
            classify("PHP Parse error: syntax error"),
            Verdict::Err(_)
        ));
        assert!(matches!(
            classify("Fatal error: syntax error, unexpected X"),
            Verdict::Err(_)
        ));
        assert_eq!(classify("Fatal error: Cannot redeclare foo()"), Verdict::Ok);
        assert_eq!(
            classify(
                "Parse error: syntax error, unexpected end of file in %s : eval()'d code on line 1"
            ),
            Verdict::Ok
        );
        // Only the first two lines count.
        assert_eq!(
            expect_head(b"\nint(1)\nint(2)\nParse error: nope\n"),
            "int(1)\nint(2)"
        );
    }

    #[test]
    fn extract_file_and_fileeof() {
        let t = b"--TEST--\nx\n--FILE--\n<?php\necho 1;\n--EXPECT--\n1\n";
        let c = extract(t, Path::new("t.phpt")).unwrap();
        assert_eq!(c.source, b"<?php\necho 1;\n");
        assert_eq!(c.expected, Verdict::Ok);

        let t = b"--TEST--\nx\n--EXPECT--\nParse error: syntax error, unexpected end of file\n--FILEEOF--\n<?php\nfunction f( {\n";
        let c = extract(t, Path::new("t.phpt")).unwrap();
        assert_eq!(c.source, b"<?php\nfunction f( {");
        assert!(matches!(c.expected, Verdict::Err(_)));

        let t = b"--TEST--\nx\n--EXPECT--\n1\n";
        match extract(t, Path::new("t.phpt")) {
            Err(skip) => assert_eq!(skip, Skip::NoFileSection),
            Ok(_) => panic!("a test without --FILE-- must be skipped"),
        }
    }

    #[test]
    fn our_verdict_agrees_on_trivial_cases() {
        assert!(our_verdict(b"<?php echo 1;").is_ok());
        assert!(our_verdict(b"<?php echo 1 +;").is_err());
    }
}
