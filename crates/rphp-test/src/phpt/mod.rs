//! php-src `.phpt` runner.
//!
//! A faithful port of the parts of php-src's `run-tests.php` (8.5.0) that
//! decide whether a test passes: section parsing ([`parse`]), section
//! semantics ([`sections`]), EXPECT/EXPECTF/EXPECTREGEX matching and diffs
//! ([`expectf`]), running one test as a real process ([`run`]), reports and
//! ratcheting baselines ([`report`]) and a content-addressed result cache
//! ([`cache`]). [`Runner`] ties them together on a rayon pool; `cargo xtask
//! phpt` is the command-line front end.
//!
//! The runner is engine-agnostic: `--engine php` runs the corpus under stock
//! php (which is how the runner itself is validated — php-src's own tests
//! must pass there), `--engine rphp` scores rphp.

pub mod cache;
pub mod expectf;
pub mod parse;
pub mod report;
pub mod run;
pub mod sections;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use sha2::{Digest, Sha256};

pub use cache::Cache;
pub use expectf::Diff;
pub use parse::{ExpectKind, ParseError, TestFile};
pub use report::{Baseline, Counts, Failure, Ratchet, SliceReport};
pub use run::{Engine, Outcome, RunOptions, TestResult};
pub use sections::{CaptureStdio, EnvMap, IniSettings};

/// The repository root (two levels above this crate's manifest).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Every `*.phpt` under `dir`, sorted.
pub fn discover(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("phpt"))
        .map(|e| e.into_path())
        .collect();
    out.sort();
    out
}

/// Does `rel` (a `/`-separated path) match `filter`? A filter containing
/// `*`, `?` or `[` is a glob matched against the whole path (`*` also
/// crosses `/`), anything else is a substring test.
pub fn filter_matches(rel: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if !filter.contains(['*', '?', '[']) {
        return rel.contains(filter);
    }
    let mut re = String::from("^");
    let mut chars = filter.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '[' => {
                re.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    re.push('^');
                }
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    if c == '\\' {
                        re.push('\\');
                    }
                    re.push(c);
                }
                re.push(']');
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    match regex::Regex::new(&re) {
        Ok(re) => re.is_match(rel),
        Err(_) => rel.contains(filter),
    }
}

/// Keep the tests whose path relative to `root` matches `filter`.
pub fn filter_tests(tests: Vec<PathBuf>, root: &Path, filter: &str) -> Vec<PathBuf> {
    tests
        .into_iter()
        .filter(|p| {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(p)
                .display()
                .to_string()
                .replace('\\', "/");
            filter_matches(&rel, filter)
                || filter_matches(
                    p.file_name()
                        .map(|f| f.to_string_lossy())
                        .as_deref()
                        .unwrap_or(""),
                    filter,
                )
        })
        .collect()
}

/// Runs many tests in parallel with optional caching.
pub struct Runner<'e> {
    /// The engine under test.
    pub engine: &'e Engine,
    /// Per-test options.
    pub options: RunOptions,
    /// Result cache; `None` disables caching.
    pub cache: Option<Cache>,
}

/// Does the test carry a `--CONFLICTS--` section or sit in a directory with a
/// `CONFLICTS` file? Those tests run one at a time after the parallel batch.
fn has_conflicts(path: &Path, dir_flags: &Mutex<HashMap<PathBuf, bool>>) -> bool {
    let dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let dir_has = {
        let mut m = dir_flags.lock().unwrap_or_else(|e| e.into_inner());
        *m.entry(dir.clone())
            .or_insert_with(|| dir.join("CONFLICTS").is_file())
    };
    if dir_has {
        return true;
    }
    match std::fs::read(path) {
        Ok(bytes) => bytes
            .windows(b"\n--CONFLICTS--".len())
            .any(|w| w == b"\n--CONFLICTS--"),
        Err(_) => false,
    }
}

/// Digest of the support files in a test directory (everything that is not a
/// `.phpt` or a generated artefact), so `.inc` edits invalidate the cache.
fn dir_digest(dir: &Path, memo: &Mutex<HashMap<PathBuf, String>>) -> String {
    if let Some(d) = memo
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(dir)
        .cloned()
    {
        return d;
    }
    let mut names: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                names.push(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    names.sort();
    let has_phpt = |n: &str| names.binary_search(&n.to_string()).is_ok();
    let mut h = Sha256::new();
    for n in &names {
        if n.ends_with(".phpt") || run::GeneratedFiles::is_generated_name(n, has_phpt) {
            continue;
        }
        h.update(n.as_bytes());
        h.update(b"\0");
        if let Ok(bytes) = std::fs::read(dir.join(n)) {
            h.update(&bytes);
        }
        h.update(b"\0");
    }
    let digest = run::hex(&h.finalize());
    memo.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(dir.to_path_buf(), digest.clone());
    digest
}

impl<'e> Runner<'e> {
    /// Run every test, in the order given, calling `on_result` as each one
    /// finishes (from worker threads). Tests with CONFLICTS run sequentially
    /// after the parallel batch.
    pub fn run_all<F>(&self, tests: &[PathBuf], on_result: F) -> Vec<TestResult>
    where
        F: Fn(&TestResult) + Sync,
    {
        let fingerprint = self.engine.fingerprint().to_string();
        let dir_memo: Mutex<HashMap<PathBuf, String>> = Mutex::new(HashMap::new());
        let conflict_memo: Mutex<HashMap<PathBuf, bool>> = Mutex::new(HashMap::new());

        let run_one = |path: &PathBuf| -> TestResult {
            let key = self.cache.as_ref().map(|_| {
                let bytes = std::fs::read(path).unwrap_or_default();
                let dir = path.parent().unwrap_or_else(|| Path::new("."));
                let digest = dir_digest(dir, &dir_memo);
                let cwd = self
                    .options
                    .cwd
                    .as_ref()
                    .map(|c| c.display().to_string())
                    .unwrap_or_default();
                Cache::key(&[
                    fingerprint.as_bytes(),
                    path.display().to_string().as_bytes(),
                    &bytes,
                    digest.as_bytes(),
                    self.options.timeout.as_secs().to_string().as_bytes(),
                    cwd.as_bytes(),
                ])
            });
            if let (Some(cache), Some(key)) = (&self.cache, &key) {
                if let Some(hit) = cache.get(key) {
                    on_result(&hit);
                    return hit;
                }
            }
            let result = run::run_test(self.engine, &self.options, path);
            if let (Some(cache), Some(key)) = (&self.cache, &key) {
                if !matches!(result.outcome, Outcome::Timeout) {
                    let _ = cache.put(key, &result);
                }
            }
            on_result(&result);
            result
        };

        let (parallel, sequential): (Vec<usize>, Vec<usize>) =
            (0..tests.len()).partition(|&i| !has_conflicts(&tests[i], &conflict_memo));

        let mut slots: Vec<Option<TestResult>> = (0..tests.len()).map(|_| None).collect();
        let done: Vec<(usize, TestResult)> = parallel
            .par_iter()
            .map(|&i| (i, run_one(&tests[i])))
            .collect();
        for (i, r) in done {
            slots[i] = Some(r);
        }
        for i in sequential {
            slots[i] = Some(run_one(&tests[i]));
        }
        slots.into_iter().flatten().collect()
    }
}
