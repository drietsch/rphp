//! The fuzzy differential oracle (ADR-008, `specs/base/10-testing.md` §20.2).
//!
//! A snippet is run as a real process under stock `php` (pinned ini, scrubbed
//! environment — see [`oracle`]) and under `rphp`; the two [`RunResult`]s are
//! compared under a fixed policy ([`compare`]):
//!
//! * **stdout** byte-exact, or against an `EXPECTF` template when a
//!   `<snippet>.expectf` sidecar exists;
//! * **stderr** after stripping php's `PHP `-prefixed log duplicates and after
//!   normalization;
//! * **exit code** exact, always.
//!
//! Output that legitimately depends on the environment is handled by the
//! divergence allowlist ([`allowlist`], `examples/tier-a/divergences.toml`):
//! each entry names a snippet (or glob), one of the **closed set** of
//! [`Category`]s, a reason, and optionally its own normalizer. Matching
//! entries run their normalizer over *both* sides, so the rest of the output
//! is still compared. Anything outside the allowlist that differs is a bug in
//! rphp.
//!
//! Sidecars next to a snippet `<dir>/<topic>.php`:
//! * `<dir>/<topic>.expectf` — template both stdouts must match;
//! * `<dir>/<topic>.exit` — the exit code the snippet is expected to end with
//!   (default 0), used by php-independent smoke tests.

pub mod allowlist;
pub mod compare;
pub mod normalize;
pub mod oracle;

pub use allowlist::{glob_match, relative_name, Allowlist, AllowlistError, Entry, Rule};
pub use compare::{
    compare, escape_bytes, expectf_matches, expectf_regex, run_rphp_isolated, run_snippet,
    run_snippet_full, strip_log_duplicates, unified_diff, Channel, Policy, SnippetRun, Verdict,
};
pub use normalize::{Category, NormalizeContext};
pub use oracle::{
    find_on_path, find_php, find_rphp, php_args, php_command, rphp_command, run_command, run_php,
    run_rphp, scrub_env, RunResult, DEFAULT_TIMEOUT, KEPT_ENV, PHP_INI,
};

use std::path::{Path, PathBuf};

/// Every `*.php` file under `root` (recursively), sorted by path.
pub fn collect_snippets(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("php"))
        .collect();
    out.sort();
    out
}

/// The sidecar path for `snippet` with the given extension
/// (`a/b.php` + `"exit"` → `a/b.exit`).
pub fn sidecar(snippet: &Path, ext: &str) -> PathBuf {
    snippet.with_extension(ext)
}

/// The exit code a snippet is expected to produce on its own: the content of
/// its `.exit` sidecar, or 0 when there is none.
pub fn expected_exit(snippet: &Path) -> std::io::Result<i32> {
    match std::fs::read_to_string(sidecar(snippet, "exit")) {
        Ok(s) => s.trim().parse::<i32>().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: not an exit code: {e}", sidecar(snippet, "exit").display()),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}
