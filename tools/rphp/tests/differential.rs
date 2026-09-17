//! Tier-A differential harness (plan T2, ADR-008): every
//! `examples/tier-a/<ext>/<topic>.php` snippet is run as a process under the
//! freshly built `rphp` and under stock `php` with the pinned oracle ini, and
//! compared under the policy in `rphp_test::differential` — stdout byte-exact
//! (or `.expectf`), stderr normalized, exit codes exact, divergences only
//! where `examples/tier-a/divergences.toml` documents them.
//!
//! * [`tier_a_snippets_run_under_rphp`] needs no php: each snippet must run
//!   and exit with the code its `.exit` sidecar declares (default 0).
//! * [`tier_a_snippets_match_stock_php`] is **skipped** (not failed) when no
//!   php is found (`PHP_BIN` or `php` on `PATH`).
//!
//! Run with `cargo test -p rphp --test differential -- --nocapture` to see the
//! per-snippet status lines.

use std::path::{Path, PathBuf};

use rphp_test::differential::{
    collect_snippets, expected_exit, find_php, run_rphp_isolated, run_snippet_full, Allowlist,
    Category, SnippetRun, Verdict, DEFAULT_TIMEOUT,
};

/// The binary under test, built by cargo for this package.
const RPHP: &str = env!("CARGO_BIN_EXE_rphp");

fn tier_a_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/tier-a");
    dir.canonicalize().unwrap_or_else(|e| panic!("{} should exist: {e}", dir.display()))
}

fn snippets(root: &Path) -> Vec<PathBuf> {
    let out = collect_snippets(root);
    assert!(!out.is_empty(), "no tier-a snippets found under {}", root.display());
    out
}

fn name_of(root: &Path, path: &Path) -> String {
    rphp_test::differential::relative_name(Some(root), path)
}

fn tags(run: &SnippetRun) -> String {
    let mut parts: Vec<String> = run.categories.iter().map(Category::to_string).collect();
    if run.expectf {
        parts.push("expectf".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  [{}]", parts.join(", "))
    }
}

/// php-independent: every snippet runs under rphp and exits with the code its
/// `.exit` sidecar declares (0 when there is none). Catches pipeline and
/// registry regressions even where no php is installed.
#[test]
fn tier_a_snippets_run_under_rphp() {
    let root = tier_a_dir();
    let mut failures: Vec<String> = Vec::new();
    let paths = snippets(&root);
    for path in &paths {
        let name = name_of(&root, path);
        let want = match expected_exit(path) {
            Ok(code) => code,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        match run_rphp_isolated(Path::new(RPHP), path, DEFAULT_TIMEOUT) {
            Ok(res) if res.timed_out => {
                println!("FAIL {name}: timed out");
                failures.push(format!("{name}: rphp timed out after {DEFAULT_TIMEOUT:?}"));
            }
            Ok(res) if res.status != want => {
                println!("FAIL {name}: exit {} (expected {want})", res.status);
                failures.push(format!(
                    "{name}: rphp exited {} but {} was expected\n--- stderr ---\n{}",
                    res.status,
                    want,
                    res.stderr_lossy()
                ));
            }
            Ok(res) => println!("ok   {name} (exit {})", res.status),
            Err(e) => {
                println!("FAIL {name}: {e}");
                failures.push(format!("{name}: cannot run rphp: {e}"));
            }
        }
    }
    println!("tier-a: {}/{} snippets run under rphp", paths.len() - failures.len(), paths.len());
    assert!(failures.is_empty(), "snippet(s) failed under rphp:\n\n{}", failures.join("\n\n"));
}

/// The differential oracle: each snippet under php and rphp, compared under
/// the allowlist. Skipped (not failed) when no php is available. A
/// `divergences.toml` that fails to load, or an entry that matches no snippet,
/// is a failure — the allowlist must stay honest.
#[test]
fn tier_a_snippets_match_stock_php() {
    let Some(php) = find_php() else {
        eprintln!("skipping differential test: no `php` on PATH (set PHP_BIN to point at one)");
        return;
    };
    let root = tier_a_dir();
    let allowlist_path = root.join("divergences.toml");
    let allowlist = Allowlist::load(&allowlist_path)
        .unwrap_or_else(|e| panic!("{} is invalid: {e}", allowlist_path.display()));
    let paths = snippets(&root);
    let names: Vec<String> = paths.iter().map(|p| allowlist.relative_name(p)).collect();

    let mut failures: Vec<String> = Vec::new();
    for stale in allowlist.unmatched(&names) {
        failures.push(format!(
            "divergences.toml: entry `{}` ({}) matches no snippet — delete it",
            stale.snippet, stale.category
        ));
    }

    println!("oracle: {}", php.display());
    let mut matched = 0usize;
    let mut allowlisted = 0usize;
    for (path, name) in paths.iter().zip(&names) {
        match run_snippet_full(&php, Path::new(RPHP), path, &allowlist, DEFAULT_TIMEOUT) {
            Ok(run) => {
                let tags = tags(&run);
                match &run.verdict {
                    Verdict::Match => {
                        matched += 1;
                        if !run.categories.is_empty() {
                            allowlisted += 1;
                        }
                        println!("ok   {name} (exit {}){tags}", run.php.status);
                    }
                    Verdict::Mismatch { channel, first_diff } => {
                        println!("FAIL {name} [{channel}]{tags}");
                        failures.push(format!(
                            "{name}: {channel} differs from stock php (php exit {}, rphp exit {}){tags}\n{first_diff}",
                            run.php.status, run.rphp.status
                        ));
                    }
                }
            }
            Err(e) => {
                println!("FAIL {name}: {e}");
                failures.push(format!("{name}: harness error: {e}"));
            }
        }
    }
    println!(
        "tier-a: {matched}/{} snippets match stock php ({allowlisted} under allowlist entries)",
        paths.len()
    );
    assert!(failures.is_empty(), "snippet(s) diverged from stock php:\n\n{}", failures.join("\n\n"));
}
