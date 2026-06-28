//! Differential oracle (`specs/base/10-testing.md`): every `examples/tier-a/*.php`
//! snippet is run through both the rphp pipeline and stock `php`, and their
//! stdout must match byte-for-byte. This is the must-not-regress gate for the
//! stdlib burn-down — each extension's snippet exercises all of its functions.
//!
//! The test is **skipped** (not failed) when no `php` is on `PATH`, so CI without
//! a PHP install stays green; locally (PHP 8.5) it is a real comparison.

use std::path::PathBuf;
use std::process::Command;

use rphp_sapi_cli::eval_to_bytes;

fn tier_a_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/tier-a")
}

fn php_available() -> bool {
    Command::new("php")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn snippets() -> Vec<PathBuf> {
    let dir = tier_a_dir();
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("examples/tier-a should exist")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("php"))
        .collect();
    out.sort();
    assert!(!out.is_empty(), "no tier-a snippets found in {}", dir.display());
    out
}

/// php-independent: every snippet must run through the rphp pipeline without
/// error. Catches pipeline/registry regressions even where `php` is unavailable.
#[test]
fn tier_a_snippets_evaluate() {
    let mut failures = Vec::new();
    for path in snippets() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read(&path).expect("read snippet");
        if let Err(e) = eval_to_bytes(&src) {
            failures.push(format!("{name}: rphp failed to evaluate:\n{e}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The differential oracle: rphp stdout must equal stock `php` stdout, byte for
/// byte. Skipped (not failed) when no `php` is on PATH.
#[test]
fn tier_a_snippets_match_stock_php() {
    if !php_available() {
        eprintln!("skipping differential test: no `php` on PATH");
        return;
    }
    let mut failures: Vec<String> = Vec::new();
    for path in snippets() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read(&path).expect("read snippet");
        let ours = match eval_to_bytes(&src) {
            Ok(bytes) => bytes,
            Err(e) => {
                failures.push(format!("{name}: rphp failed to evaluate:\n{e}"));
                continue;
            }
        };
        // Compare stdout only (warnings/notices go to stderr).
        let php = Command::new("php").arg(&path).output().expect("run php");
        if ours != php.stdout {
            failures.push(format!(
                "{name}: output differs from stock php\n--- rphp ---\n{}\n--- php ---\n{}",
                String::from_utf8_lossy(&ours),
                String::from_utf8_lossy(&php.stdout),
            ));
        }
    }
    assert!(failures.is_empty(), "snippet(s) diverged from stock php:\n\n{}", failures.join("\n\n"));
}
