//! `cargo xtask corpus` — two-way parse-parity job over a real-world PHP corpus.
//!
//! For every `*.php` file under the given directories the job records two
//! verdicts and compares them:
//!
//! * the **php verdict** from `php -n -l <file>` (stock PHP is the oracle),
//!   cached under `target/corpus-cache/<sha256 of contents>` as `ok` or
//!   `err:<message>` so reruns cost nothing;
//! * the **rphp verdict** from our front end ([`our_verdict`]; one small
//!   function so F2 can repoint it at the mago adapter).
//!
//! The report lists *false rejects* (php ok, ours error) grouped by the first
//! diagnostic's code and message prefix, *false accepts* (php error, ours ok),
//! panics, and timing. A negative corpus (`--negative <dir>`) holds files that
//! must be rejected. The exit status is non-zero on any disagreement unless
//! `--allow-failures`; a machine-readable copy always lands in
//! `target/corpus-report.json`.
//!
//! This job is the F2 gate (roadmap Track F): with the hand-written M0 parser
//! the numbers are expected to be terrible.
//!
//! The report machinery (`Verdict`, `Outcome`, `summarize`, `render_text`, the
//! panic guard) is `pub(crate)` so `parse-sweep` produces the same shape.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::XtaskResult;

const HELP: &str = "\
cargo xtask corpus — two-way parse parity vs `php -l` over a PHP corpus

USAGE:
    cargo xtask corpus --dir <path> [--dir <path>...] [options]

OPTIONS:
    --dir <path>         corpus root (repeatable; default: $RPHP_CORPUS_DIR)
    --negative <dir>     files that must be rejected (repeatable)
    --jobs <N>           worker threads (default: all cores)
    --limit <N>          only the first N files (after sorting/filtering)
    --filter <substr>    only paths containing <substr>
    --include-tests      do not skip `/Tests/` and `/tests/` directories
    --report <text|json> report format on stdout (default: text)
    --top <N>            false-reject groups to print (default: 20)
    --stack-mb <N>       worker thread stack in MiB (default: 1024; deep nesting)
    --no-php-cache       ignore and rewrite target/corpus-cache
    --short-open-tag     lint with short_open_tag=1 (default: 0)
    --php <bin>          php binary (default: $RPHP_PHP or `php`)
    --allow-failures     exit 0 even when verdicts disagree
    -h, --help           this help

The JSON report is always written to target/corpus-report.json.
";

// ---------------------------------------------------------------------------
// Our front end
// ---------------------------------------------------------------------------

/// Parse `src` with the current front end and reduce the result to a verdict.
///
/// `Ok(())` when no error-severity diagnostic was produced; otherwise the
/// rendered diagnostics in order (the first one drives grouping). This is the
/// only place the corpus job touches the parser: F2 swaps the body for the
/// mago adapter without changing anything else in this file.
fn our_verdict(src: &[u8]) -> Result<(), Vec<String>> {
    let mut sources = rphp_source::SourceMap::new();
    let id = sources.add("<corpus>", src.to_vec());
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

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Replace the default panic hook with one that records the message and
/// location in a thread-local instead of printing. Call once before a sweep;
/// [`guarded`] reads the record back.
pub(crate) fn install_quiet_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        LAST_PANIC.with(|p| *p.borrow_mut() = Some(format!("{msg} (at {loc})")));
    }));
}

/// Run a verdict function under `catch_unwind`; a panic becomes an `Err` with
/// a single `error[PANIC]: ...` line and `panicked = true`.
pub(crate) fn guarded(
    f: impl FnOnce() -> Result<(), Vec<String>>,
) -> (Result<(), Vec<String>>, bool) {
    LAST_PANIC.with(|p| *p.borrow_mut() = None);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => (r, false),
        Err(_) => {
            let msg = LAST_PANIC
                .with(|p| p.borrow_mut().take())
                .unwrap_or_else(|| "unknown panic".to_string());
            (Err(vec![format!("error[PANIC]: {msg}")]), true)
        }
    }
}

// ---------------------------------------------------------------------------
// Verdicts, outcomes, report
// ---------------------------------------------------------------------------

/// What the oracle (php, or a `.phpt` expectation) says about a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub(crate) enum Verdict {
    /// The file parses.
    Ok,
    /// The file must be rejected; the payload is the oracle's message.
    Err(String),
}

/// One file's pair of verdicts.
#[derive(Clone, Debug)]
pub(crate) struct Outcome {
    /// Path as discovered (relative if the root was given relative).
    pub path: String,
    /// The oracle's verdict.
    pub expected: Verdict,
    /// Our front end's verdict (rendered diagnostics on error).
    pub ours: Result<(), Vec<String>>,
    /// Our front end panicked (already folded into `ours` as `error[PANIC]`).
    pub panicked: bool,
    /// Extra note for the report (e.g. negative-corpus file php accepted).
    pub note: Option<String>,
}

/// A cluster of false rejects sharing the first diagnostic's code + prefix.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Group {
    pub key: String,
    pub count: usize,
    /// Up to three `file:line` examples.
    pub examples: Vec<String>,
}

/// One false accept (the oracle rejects, we accept).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct FalseAccept {
    pub file: String,
    pub expected: String,
}

/// Wall/CPU timing of a sweep.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Timing {
    pub discover_ms: u128,
    /// Cumulative oracle time across worker threads.
    pub oracle_ms: u128,
    /// Cumulative front-end time across worker threads.
    pub ours_ms: u128,
    pub total_ms: u128,
    pub threads: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
}

/// The full report; serialized verbatim to JSON.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Report {
    pub job: &'static str,
    pub roots: Vec<String>,
    pub files: usize,
    pub skipped: usize,
    pub expected_ok: usize,
    pub expected_err: usize,
    pub ours_ok: usize,
    pub ours_err: usize,
    pub panics: usize,
    pub agree: usize,
    pub false_rejects: usize,
    pub false_accepts: usize,
    pub false_reject_groups: Vec<Group>,
    pub false_reject_group_count: usize,
    pub false_accept_examples: Vec<FalseAccept>,
    pub false_accept_count: usize,
    pub notes: Vec<String>,
    pub timing: Timing,
}

/// Parsed pieces of one rendered diagnostic (`error[CODE]: msg\n  --> f:l:c`).
struct DiagParts {
    code: String,
    message: String,
    line: Option<u32>,
}

fn split_rendered(rendered: &str) -> DiagParts {
    let mut lines = rendered.lines();
    let head = lines.next().unwrap_or_default();
    let code = head
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(c, _)| c.to_string())
        .unwrap_or_else(|| "?".to_string());
    let message = head
        .split_once("]: ")
        .map(|(_, m)| m.to_string())
        .unwrap_or_else(|| head.to_string());
    let line = lines
        .find_map(|l| l.trim_start().strip_prefix("--> "))
        .and_then(|loc| {
            let mut it = loc.rsplit(':');
            let _col = it.next()?;
            it.next()?.parse().ok()
        });
    DiagParts {
        code,
        message,
        line,
    }
}

/// Group key: the code plus the message cut at the first quoted token so
/// `unexpected character 'x'` and `... 'y'` land in one bucket.
fn group_key(parts: &DiagParts) -> String {
    let cut = parts
        .message
        .find(['\'', '"'])
        .unwrap_or(parts.message.len());
    let mut prefix = parts.message[..cut]
        .trim_end_matches([' ', ',', ':'])
        .to_string();
    if prefix.len() > 72 {
        let mut end = 72;
        while !prefix.is_char_boundary(end) {
            end -= 1;
        }
        prefix.truncate(end);
        prefix.push('…');
    }
    format!("{}: {}", parts.code, prefix)
}

/// Fold outcomes into a [`Report`].
pub(crate) fn summarize(
    job: &'static str,
    roots: Vec<String>,
    skipped: usize,
    outcomes: &[Outcome],
    top: usize,
    timing: Timing,
) -> Report {
    let mut r = Report {
        job,
        roots,
        files: outcomes.len(),
        skipped,
        expected_ok: 0,
        expected_err: 0,
        ours_ok: 0,
        ours_err: 0,
        panics: 0,
        agree: 0,
        false_rejects: 0,
        false_accepts: 0,
        false_reject_groups: Vec::new(),
        false_reject_group_count: 0,
        false_accept_examples: Vec::new(),
        false_accept_count: 0,
        notes: Vec::new(),
        timing,
    };
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for o in outcomes {
        match o.expected {
            Verdict::Ok => r.expected_ok += 1,
            Verdict::Err(_) => r.expected_err += 1,
        }
        if o.panicked {
            r.panics += 1;
        }
        if let Some(n) = &o.note {
            r.notes.push(format!("{}: {n}", o.path));
        }
        match (&o.expected, &o.ours) {
            (Verdict::Ok, Ok(())) => {
                r.ours_ok += 1;
                r.agree += 1;
            }
            (Verdict::Err(_), Err(_)) => {
                r.ours_err += 1;
                r.agree += 1;
            }
            (Verdict::Ok, Err(diags)) => {
                r.ours_err += 1;
                r.false_rejects += 1;
                let parts = split_rendered(diags.first().map(String::as_str).unwrap_or(""));
                let key = group_key(&parts);
                let g = groups.entry(key.clone()).or_insert_with(|| Group {
                    key,
                    count: 0,
                    examples: Vec::new(),
                });
                g.count += 1;
                if g.examples.len() < 3 {
                    g.examples.push(match parts.line {
                        Some(l) => format!("{}:{l}", o.path),
                        None => o.path.clone(),
                    });
                }
            }
            (Verdict::Err(msg), Ok(())) => {
                r.ours_ok += 1;
                r.false_accepts += 1;
                r.false_accept_examples.push(FalseAccept {
                    file: o.path.clone(),
                    expected: msg.clone(),
                });
            }
        }
    }
    let mut groups: Vec<Group> = groups.into_values().collect();
    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    r.false_reject_group_count = groups.len();
    groups.truncate(top);
    r.false_reject_groups = groups;
    r.false_accept_count = r.false_accept_examples.len();
    r.false_accept_examples.truncate(top);
    r
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn secs(ms: u128) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Human-readable form of a [`Report`].
pub(crate) fn render_text(r: &Report, oracle_name: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{}: {} files in {} root(s), {} skipped",
        r.job,
        r.files,
        r.roots.len(),
        r.skipped
    );
    let _ = writeln!(
        s,
        "  {:<9} ok {:>6}   err {:>6}   (cache {} hits / {} misses, {} cumulative)",
        oracle_name,
        r.expected_ok,
        r.expected_err,
        r.timing.cache_hits,
        r.timing.cache_misses,
        secs(r.timing.oracle_ms)
    );
    let _ = writeln!(
        s,
        "  {:<9} ok {:>6}   err {:>6}   panic {:>4}   ({} cumulative)",
        "rphp",
        r.ours_ok,
        r.ours_err,
        r.panics,
        secs(r.timing.ours_ms)
    );
    let _ = writeln!(
        s,
        "  agree {:>6}   false rejects {:>6}   false accepts {:>6}",
        r.agree, r.false_rejects, r.false_accepts
    );
    if r.false_rejects > 0 {
        let _ = writeln!(
            s,
            "\nfalse rejects ({oracle_name} ok, rphp error) — top {} of {} groups:",
            r.false_reject_groups.len(),
            r.false_reject_group_count
        );
        for g in &r.false_reject_groups {
            let _ = writeln!(s, "  {:>6}  {}", g.count, g.key);
            for e in &g.examples {
                let _ = writeln!(s, "            {e}");
            }
        }
    }
    if r.false_accepts > 0 {
        let _ = writeln!(
            s,
            "\nfalse accepts ({oracle_name} error, rphp ok) — first {} of {}:",
            r.false_accept_examples.len(),
            r.false_accept_count
        );
        for fa in &r.false_accept_examples {
            let _ = writeln!(s, "  {}\n            {}", fa.file, fa.expected);
        }
    }
    if !r.notes.is_empty() {
        let _ = writeln!(s, "\nnotes ({}):", r.notes.len());
        for n in r.notes.iter().take(20) {
            let _ = writeln!(s, "  {n}");
        }
        if r.notes.len() > 20 {
            let _ = writeln!(s, "  … {} more", r.notes.len() - 20);
        }
    }
    let _ = writeln!(
        s,
        "\ntotal {} wall on {} threads (discovery {})",
        secs(r.timing.total_ms),
        r.timing.threads,
        secs(r.timing.discover_ms)
    );
    s
}

/// Print the report in the requested format, write the JSON copy, and turn
/// disagreements into an error unless `allow_failures`.
pub(crate) fn finish(
    r: &Report,
    oracle_name: &str,
    json_stdout: bool,
    json_path: &Path,
    allow_failures: bool,
) -> XtaskResult {
    let json = serde_json::to_string_pretty(r)?;
    if let Some(parent) = json_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(json_path, &json)?;
    if json_stdout {
        println!("{json}");
    } else {
        print!("{}", render_text(r, oracle_name));
        println!("json report: {}", json_path.display());
    }
    let bad = r.false_rejects + r.false_accepts + r.panics;
    if bad > 0 && !allow_failures {
        return Err(format!(
            "{} false rejects, {} false accepts, {} panics (pass --allow-failures to exit 0)",
            r.false_rejects, r.false_accepts, r.panics
        )
        .into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers (paths, discovery, pools)
// ---------------------------------------------------------------------------

/// The repository root (the parent of `xtask/`).
pub(crate) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits directly under the repo root")
        .to_path_buf()
}

/// `$CARGO_TARGET_DIR` or `<repo>/target`.
pub(crate) fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("target"))
}

/// Recursively collect files with extension `ext` under `roots`, sorted.
/// Directories named `tests`/`Tests` are skipped unless `include_tests`;
/// the count of skipped files is returned alongside.
pub(crate) fn discover(roots: &[PathBuf], ext: &str, include_tests: bool) -> (Vec<PathBuf>, usize) {
    let mut files = Vec::new();
    let mut skipped = 0usize;
    for root in roots {
        for entry in WalkDir::new(root)
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some(ext) {
                continue;
            }
            if !include_tests {
                let rel = p.strip_prefix(root).unwrap_or(p);
                let in_tests = rel
                    .parent()
                    .map(|d| {
                        d.components()
                            .any(|c| matches!(c.as_os_str().to_str(), Some("tests" | "Tests")))
                    })
                    .unwrap_or(false);
                if in_tests {
                    skipped += 1;
                    continue;
                }
            }
            files.push(p.to_path_buf());
        }
    }
    files.sort();
    files.dedup();
    (files, skipped)
}

/// A rayon pool with a generous stack (`--stack-mb`, virtual reservation
/// only) so a recursive-descent parser survives pathologically nested input
/// such as `Zend/tests/bug64660.phpt` (34k nested `[`), which php itself
/// rejects with "memory exhausted". A stack overflow aborts the whole process
/// and cannot be caught, so this is the only defence the sweep has.
pub(crate) fn thread_pool(
    opts: &CommonOpts,
) -> Result<rayon::ThreadPool, Box<dyn std::error::Error>> {
    let mut b = rayon::ThreadPoolBuilder::new().stack_size(opts.stack_mb.max(1) << 20);
    if let Some(n) = opts.jobs {
        b = b.num_threads(n.max(1));
    }
    Ok(b.build()?)
}

/// Default worker stack in MiB; enough for the deepest php-src nesting test
/// under a debug build of the M0 parser.
pub(crate) const DEFAULT_STACK_MB: usize = 1024;

/// Common report/selection flags shared by the sweep jobs.
pub(crate) struct CommonOpts {
    pub jobs: Option<usize>,
    pub limit: Option<usize>,
    pub filter: Option<String>,
    pub json: bool,
    pub top: usize,
    pub allow_failures: bool,
    /// Worker thread stack size in MiB.
    pub stack_mb: usize,
}

/// Pull the common flags out of `args` (leaves job-specific ones in place).
pub(crate) fn parse_common(
    args: &mut pico_args::Arguments,
) -> Result<CommonOpts, Box<dyn std::error::Error>> {
    let json = match args.opt_value_from_str::<_, String>("--report")? {
        None => false,
        Some(s) if s == "text" => false,
        Some(s) if s == "json" => true,
        Some(other) => {
            return Err(format!("--report must be `text` or `json`, got `{other}`").into())
        }
    };
    Ok(CommonOpts {
        jobs: args.opt_value_from_str("--jobs")?,
        limit: args.opt_value_from_str("--limit")?,
        filter: args.opt_value_from_str("--filter")?,
        json,
        top: args.opt_value_from_str("--top")?.unwrap_or(20),
        allow_failures: args.contains("--allow-failures"),
        stack_mb: args
            .opt_value_from_str("--stack-mb")?
            .unwrap_or(DEFAULT_STACK_MB),
    })
}

/// Apply `--filter` and `--limit`.
pub(crate) fn select(mut files: Vec<PathBuf>, opts: &CommonOpts) -> Vec<PathBuf> {
    if let Some(f) = &opts.filter {
        files.retain(|p| p.to_string_lossy().contains(f.as_str()));
    }
    if let Some(n) = opts.limit {
        files.truncate(n);
    }
    files
}

// ---------------------------------------------------------------------------
// php -l oracle with content-addressed cache
// ---------------------------------------------------------------------------

struct PhpOracle {
    php: String,
    short_open_tag: bool,
    cache_dir: Option<PathBuf>,
    read_cache: bool,
}

/// `php -l` result plus whether it came from the cache.
struct PhpResult {
    verdict: Verdict,
    cached: bool,
}

impl PhpOracle {
    fn cache_path(&self, contents: &[u8]) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        let sha = Sha256::digest(contents);
        let mut name = String::with_capacity(68);
        for b in sha {
            let _ = write!(name, "{b:02x}");
        }
        if self.short_open_tag {
            name.push_str(".sot");
        }
        Some(dir.join(name))
    }

    fn lint(&self, path: &Path, contents: &[u8]) -> Result<PhpResult, String> {
        let cache = self.cache_path(contents);
        if self.read_cache {
            if let Some(c) = &cache {
                if let Ok(s) = std::fs::read_to_string(c) {
                    if let Some(v) = decode_cache(&s) {
                        return Ok(PhpResult {
                            verdict: v,
                            cached: true,
                        });
                    }
                }
            }
        }
        let out = Command::new(&self.php)
            .arg("-n")
            .arg("-d")
            .arg("display_errors=1")
            .arg("-d")
            .arg("log_errors=0")
            .arg("-d")
            .arg("html_errors=0")
            .arg("-d")
            .arg(format!("short_open_tag={}", u8::from(self.short_open_tag)))
            .arg("-l")
            .arg(path)
            .output()
            .map_err(|e| format!("cannot run `{}`: {e}", self.php))?;
        let verdict = if out.status.success() {
            Verdict::Ok
        } else {
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            Verdict::Err(php_error_line(&text, out.status.code()))
        };
        if let Some(c) = &cache {
            let _ = write_cache(c, &verdict);
        }
        Ok(PhpResult {
            verdict,
            cached: false,
        })
    }
}

/// The interesting line of `php -l` output: the first `Parse error:` /
/// `Fatal error:` (with any `PHP ` prefix stripped), else the first non-empty
/// line, else the exit code.
fn php_error_line(text: &str, code: Option<i32>) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    for l in &lines {
        let l = l.strip_prefix("PHP ").unwrap_or(l);
        if l.starts_with("Parse error:") || l.starts_with("Fatal error:") {
            return l.to_string();
        }
    }
    lines
        .iter()
        .find(|l| !l.starts_with("Errors parsing"))
        .map(|l| (*l).to_string())
        .unwrap_or_else(|| {
            format!(
                "php exited with {}",
                code.map_or("signal".to_string(), |c| c.to_string())
            )
        })
}

fn decode_cache(s: &str) -> Option<Verdict> {
    let s = s.trim_end_matches('\n');
    if s == "ok" {
        Some(Verdict::Ok)
    } else {
        s.strip_prefix("err:").map(|m| Verdict::Err(m.to_string()))
    }
}

fn write_cache(path: &Path, v: &Verdict) -> std::io::Result<()> {
    let dir = path.parent().expect("cache file has a parent");
    let body = match v {
        Verdict::Ok => "ok".to_string(),
        Verdict::Err(m) => format!("err:{m}"),
    };
    // Write-then-rename so a concurrent reader never sees a torn file
    // (two workers may hit the same sha for identical files).
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(tmp.path(), body)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Turn a php `Parse error: ... in <file> on line N` into `<file>:N — msg`
/// shaped text for the false-accept list (keeps the message intact otherwise).
fn strip_php_location(msg: &str) -> String {
    match msg.rfind(" in ") {
        Some(i) if msg[i..].contains(" on line ") => {
            let line = msg[i..].rsplit(" on line ").next().unwrap_or("").trim();
            format!("{} (line {line})", &msg[..i])
        }
        _ => msg.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

struct Opts {
    dirs: Vec<PathBuf>,
    negative: Vec<PathBuf>,
    include_tests: bool,
    no_php_cache: bool,
    short_open_tag: bool,
    php: String,
    common: CommonOpts,
}

fn parse_args(args: &[String]) -> Result<Option<Opts>, Box<dyn std::error::Error>> {
    let mut a = pico_args::Arguments::from_vec(args.iter().map(OsString::from).collect());
    if a.contains(["-h", "--help"]) {
        print!("{HELP}");
        return Ok(None);
    }
    let mut dirs: Vec<PathBuf> = a.values_from_str("--dir")?;
    if dirs.is_empty() {
        if let Some(env) = std::env::var_os("RPHP_CORPUS_DIR") {
            dirs = std::env::split_paths(&env).collect();
        }
    }
    if dirs.is_empty() {
        eprint!("{HELP}");
        return Err("no corpus given: pass --dir <path> or set RPHP_CORPUS_DIR".into());
    }
    let opts = Opts {
        dirs,
        negative: a.values_from_str("--negative")?,
        include_tests: a.contains("--include-tests"),
        no_php_cache: a.contains("--no-php-cache"),
        short_open_tag: a.contains("--short-open-tag"),
        php: a
            .opt_value_from_str("--php")?
            .or_else(|| std::env::var("RPHP_PHP").ok())
            .unwrap_or_else(|| "php".to_string()),
        common: parse_common(&mut a)?,
    };
    let rest = a.finish();
    if !rest.is_empty() {
        eprint!("{HELP}");
        return Err(format!("unexpected arguments: {rest:?}").into());
    }
    for d in opts.dirs.iter().chain(&opts.negative) {
        if !d.is_dir() {
            return Err(format!("not a directory: {}", d.display()).into());
        }
    }
    Ok(Some(opts))
}

/// Run the corpus job.
pub fn run(args: &[String]) -> XtaskResult {
    let Some(opts) = parse_args(args)? else {
        return Ok(());
    };
    let started = Instant::now();

    let (positive, skipped_pos) = discover(&opts.dirs, "php", opts.include_tests);
    let (negative, skipped_neg) = discover(&opts.negative, "php", true);
    let positive = select(positive, &opts.common);
    let negative = select(negative, &opts.common);
    let discover_ms = ms(started.elapsed());
    let skipped = skipped_pos + skipped_neg;

    if positive.is_empty() && negative.is_empty() {
        return Err("no .php files found".into());
    }

    let cache_dir = target_dir().join("corpus-cache");
    std::fs::create_dir_all(&cache_dir)?;
    let oracle = PhpOracle {
        php: opts.php.clone(),
        short_open_tag: opts.short_open_tag,
        cache_dir: Some(cache_dir),
        read_cache: !opts.no_php_cache,
    };

    // Fail early with a clear message when php is unusable at all.
    let probe = Command::new(&opts.php).arg("-n").arg("-v").output();
    match probe {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            return Err(format!(
                "`{} -v` failed: {}",
                opts.php,
                String::from_utf8_lossy(&o.stderr).trim()
            )
            .into())
        }
        Err(e) => {
            return Err(format!("cannot run `{}`: {e} (set --php or RPHP_PHP)", opts.php).into())
        }
    }

    let pool = thread_pool(&opts.common)?;
    let threads = pool.current_num_threads();
    install_quiet_panic_hook();

    struct Row {
        outcome: Outcome,
        oracle: Duration,
        ours: Duration,
        cached: bool,
    }

    let work: Vec<(PathBuf, bool)> = positive
        .iter()
        .map(|p| (p.clone(), false))
        .chain(negative.iter().map(|p| (p.clone(), true)))
        .collect();

    let rows: Vec<Result<Row, String>> = pool.install(|| {
        work.par_iter()
            .map(|(path, is_negative)| {
                let display = path.to_string_lossy().into_owned();
                let contents = std::fs::read(path).map_err(|e| format!("{display}: {e}"))?;
                let t0 = Instant::now();
                let php = oracle
                    .lint(path, &contents)
                    .map_err(|e| format!("{display}: {e}"))?;
                let oracle_t = t0.elapsed();
                let t1 = Instant::now();
                let (ours, panicked) = guarded(|| our_verdict(&contents));
                let ours_t = t1.elapsed();
                let (expected, note) = if *is_negative {
                    let note = match &php.verdict {
                        Verdict::Ok => Some("negative-corpus file is accepted by php".to_string()),
                        Verdict::Err(_) => None,
                    };
                    (
                        Verdict::Err("negative corpus: must be rejected".to_string()),
                        note,
                    )
                } else {
                    (
                        match php.verdict {
                            Verdict::Ok => Verdict::Ok,
                            Verdict::Err(m) => Verdict::Err(strip_php_location(&m)),
                        },
                        None,
                    )
                };
                Ok(Row {
                    outcome: Outcome {
                        path: display,
                        expected,
                        ours,
                        panicked,
                        note,
                    },
                    oracle: oracle_t,
                    ours: ours_t,
                    cached: php.cached,
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
    let (mut oracle_total, mut ours_total) = (Duration::ZERO, Duration::ZERO);
    for row in rows {
        let row = row?;
        oracle_total += row.oracle;
        ours_total += row.ours;
        if row.cached {
            timing.cache_hits += 1;
        } else {
            timing.cache_misses += 1;
        }
        outcomes.push(row.outcome);
    }
    timing.oracle_ms = ms(oracle_total);
    timing.ours_ms = ms(ours_total);
    timing.total_ms = ms(started.elapsed());

    let roots = opts
        .dirs
        .iter()
        .chain(&opts.negative)
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    let report = summarize("corpus", roots, skipped, &outcomes, opts.common.top, timing);
    finish(
        &report,
        "php -l",
        opts.common.json,
        &target_dir().join("corpus-report.json"),
        opts.common.allow_failures,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_verdict_accepts_and_rejects() {
        assert!(our_verdict(b"<?php echo 1;").is_ok());
        let err = our_verdict(b"<?php echo 1 +;").unwrap_err();
        assert!(!err.is_empty());
        assert!(err[0].contains("RPHP_E"), "{}", err[0]);
    }

    #[test]
    fn rendered_diagnostic_splits_into_code_message_line() {
        let p = split_rendered("error[RPHP_E0002]: expected `;`\n  --> x.php:12:5\n   = here");
        assert_eq!(p.code, "RPHP_E0002");
        assert_eq!(p.message, "expected `;`");
        assert_eq!(p.line, Some(12));
        let q = split_rendered("error[PANIC]: boom (at x.rs:1)");
        assert_eq!(q.code, "PANIC");
        assert_eq!(q.line, None);
    }

    #[test]
    fn group_key_cuts_at_first_quote() {
        let p = DiagParts {
            code: "RPHP_E0001".into(),
            message: "unexpected character 'x'".into(),
            line: None,
        };
        assert_eq!(group_key(&p), "RPHP_E0001: unexpected character");
        let q = DiagParts {
            code: "RPHP_E0002".into(),
            message: "expected `;`".into(),
            line: None,
        };
        assert_eq!(group_key(&q), "RPHP_E0002: expected `;`");
    }

    #[test]
    fn php_error_line_prefers_parse_error() {
        let text = "\nParse error: syntax error, unexpected token \";\" in a.php on line 2\nErrors parsing a.php\n";
        assert_eq!(
            php_error_line(text, Some(255)),
            "Parse error: syntax error, unexpected token \";\" in a.php on line 2"
        );
        assert_eq!(
            php_error_line("PHP Fatal error: x\n", Some(255)),
            "Fatal error: x"
        );
        assert_eq!(
            php_error_line("Errors parsing a.php\n", Some(255)),
            "php exited with 255"
        );
    }

    #[test]
    fn strip_php_location_keeps_line() {
        assert_eq!(
            strip_php_location("Parse error: syntax error in /x/a.php on line 7"),
            "Parse error: syntax error (line 7)"
        );
        assert_eq!(strip_php_location("Fatal error: nope"), "Fatal error: nope");
    }

    #[test]
    fn cache_roundtrip() {
        assert_eq!(decode_cache("ok\n"), Some(Verdict::Ok));
        assert_eq!(
            decode_cache("err:Parse error: x"),
            Some(Verdict::Err("Parse error: x".into()))
        );
        assert_eq!(decode_cache("garbage"), None);
    }

    #[test]
    fn summarize_classifies_outcomes() {
        let outcomes = vec![
            Outcome {
                path: "a.php".into(),
                expected: Verdict::Ok,
                ours: Ok(()),
                panicked: false,
                note: None,
            },
            Outcome {
                path: "b.php".into(),
                expected: Verdict::Ok,
                ours: Err(vec![
                    "error[RPHP_E0002]: expected `;`\n  --> <corpus>:3:1".into()
                ]),
                panicked: false,
                note: None,
            },
            Outcome {
                path: "c.php".into(),
                expected: Verdict::Err("Parse error: x".into()),
                ours: Ok(()),
                panicked: false,
                note: None,
            },
            Outcome {
                path: "d.php".into(),
                expected: Verdict::Err("Parse error: y".into()),
                ours: Err(vec!["error[RPHP_E0002]: expected `;`".into()]),
                panicked: false,
                note: None,
            },
        ];
        let r = summarize("corpus", vec![], 0, &outcomes, 20, Timing::default());
        assert_eq!(
            (r.files, r.agree, r.false_rejects, r.false_accepts),
            (4, 2, 1, 1)
        );
        assert_eq!(r.false_reject_groups.len(), 1);
        assert_eq!(
            r.false_reject_groups[0].examples,
            vec!["b.php:3".to_string()]
        );
        assert_eq!(r.false_accept_examples[0].file, "c.php");
        let text = render_text(&r, "php -l");
        assert!(text.contains("false rejects"));
        assert!(text.contains("b.php:3"));
    }

    #[test]
    fn guarded_turns_panics_into_diagnostics() {
        install_quiet_panic_hook();
        let (r, panicked) = guarded(|| panic!("kaboom"));
        assert!(panicked);
        assert!(r.unwrap_err()[0].contains("kaboom"));
        let _ = std::panic::take_hook();
    }
}
