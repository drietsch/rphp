//! `cargo xtask ladder` — run the Symfony ladder fixtures (plan T4, ADR-034).
//!
//! Every `fixtures/ladder/<name>/ladder.toml` declares the rungs a Composer
//! fixture serves. For each selected rung the runner creates a **fresh working
//! copy** of the fixture, runs the rung's commands in order under stock `php`
//! (with the pinned oracle ini from `rphp-test`) and under `rphp`, and compares
//! per command: stdout byte-exact, stderr normalized, exit code exact — under
//! the fixture's divergence allowlist — plus the declared artifact files
//! byte-for-byte.
//!
//! ```text
//! cargo xtask ladder --list
//! cargo xtask ladder --rung L1
//! cargo xtask ladder --rung L1 --only php --keep
//! cargo xtask ladder --fixture L1-skeleton --allow-failures --report json
//! ```
//!
//! Working copies live at `target/ladder/<fixture>/<rung>/<side>/`. Both sides
//! run at the *same* path (`…/<rung>/work/`, renamed to `php/` resp. `rphp/`
//! once the side is done) so that path *lengths* inside serialized data and
//! padded console tables cannot differ between the engines; that path is
//! rendered as `%FIXTURE%` in every diff and before artifacts are compared.
//! `vendor/` is hard-linked file by file (PHP resolves `__DIR__` through
//! symlinks, so a symlinked `vendor/` would make Composer's `$baseDir` escape
//! the working copy), `var/` starts empty, `node_modules/` and `.git/` are
//! skipped.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rphp_test::differential::normalize::replace_bytes;
use rphp_test::differential::{
    compare, escape_bytes, find_php, find_rphp, glob_match, php_command, relative_name,
    rphp_command, run_command, scrub_env, unified_diff, Allowlist, Category, Channel,
    NormalizeContext, Policy, RunResult, Verdict, PHP_INI,
};
use serde::{Deserialize, Serialize};

use crate::XtaskResult;

const USAGE: &str = "\
usage: cargo xtask ladder [options]

Runs every command of the selected ladder rungs under stock `php` (pinned oracle
ini) and under `rphp`, each in a fresh working copy of the fixture, and compares
stdout (byte-exact) / stderr (normalized) / exit code per command plus the
rung's artifact files (ADR-034).

Selection (default: every rung of every fixture under fixtures/ladder/):
  --rung <id>           rung to run, e.g. L1 (repeatable)
  --fixture <name>      only this fixture (directory or [fixture] name)
  --list                list fixtures and rungs, run nothing

Execution:
  --only php|rphp       run one side only and keep its outputs under
                        target/ladder/<fixture>/<rung>/<side>/out/<n>.{stdout,stderr,exit}
  --php <path>          stock php (default: $PHP_BIN or `php` on PATH)
  --rphp <path>         rphp binary (default: $RPHP_BIN or target/debug/rphp;
                        never built implicitly — run `cargo build -p rphp`)
  --timeout <s>         per-command timeout (default 120)
  --jobs <n>            rungs run in parallel (default 1; commands stay sequential)
  --keep                keep the working copies (default: only <side>/out/ survives)
  --allow-failures      exit 0 even when commands or artifacts diverge

Reporting:
  --report text|json    stdout format (default text); the JSON report is always
                        written to target/ladder-report.json
  --fixtures-dir <path> fixtures root (default <repo>/fixtures/ladder)
  --target-dir <path>   scratch root (default $CARGO_TARGET_DIR or <repo>/target)

Exit status: 0 all compared commands and artifacts agree (skipped rungs are fine),
1 a divergence or runner error, 2 setup problem (no php, no rphp, vendor/ missing,
bad ladder.toml, unknown rung).
";

/// Default per-command timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Lines of a unified diff shown in the text report.
const DIFF_LINES: usize = 20;
/// Lines of rphp's stderr shown after a failing command.
const STDERR_TAIL: usize = 10;
/// Fixture entries never copied into a working copy (besides `vendor/`, which
/// is hard-linked, and `var/`, which starts empty).
const SKIPPED_ENTRIES: &[&str] = &["node_modules", ".git"];
/// Placeholder for the working-copy path in diffs and artifact comparison.
const FIXTURE_PLACEHOLDER: &[u8] = b"%FIXTURE%";
/// Why HTTP rungs are not run yet.
pub const HTTP_SKIP_NOTE: &str = "served by `php -S` vs `rphp -S`";

// ---------------------------------------------------------------------------
// ladder.toml schema
// ---------------------------------------------------------------------------

/// A `ladder.toml` file as written (see `fixtures/ladder/README.md`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LadderFile {
    /// The `[fixture]` table.
    pub fixture: FixtureMeta,
    /// Every `[[rung]]`, in file order.
    #[serde(default)]
    pub rung: Vec<RungSpec>,
}

/// The `[fixture]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureMeta {
    /// Display name (normally the directory name).
    pub name: String,
    /// The Composer command `tools/fixtures/setup.sh` runs (informational).
    #[serde(default)]
    pub setup: Option<String>,
    /// Minimum stock php version the fixture needs (`8.4.1`); the runner
    /// refuses to run it against an older oracle.
    #[serde(default)]
    pub php_min: Option<String>,
    /// Fixture-relative divergence allowlist (rphp-test format) applied to
    /// every rung that does not name its own.
    #[serde(default)]
    pub allowlist: Option<String>,
    /// Entries of the fixture's `var/` that the working copy keeps (`var/`
    /// otherwise starts empty): build products the pages need that no rung
    /// command produces, such as a compiled stylesheet (`sass`).
    #[serde(default)]
    pub var_keep: Vec<String>,
}

/// One `[[rung]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RungSpec {
    /// Rung id (`L1`, `L6a`); unique within a file, selected with `--rung`.
    pub id: String,
    /// Human-readable title.
    #[serde(default)]
    pub title: String,
    /// Commands run in order in the working copy. Each is either a string
    /// (`"bin/console list --no-ansi"`) or a table (`{ run = "…", expect_exit
    /// = 1, allow = ["timing"], stdin = "…", env = { … } }`).
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    /// Working-copy-relative files (globs allowed) that must be byte-identical
    /// after all commands ran.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Extra environment for every command of the rung.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// stdin fed to every command that does not set its own.
    #[serde(default)]
    pub stdin: Option<String>,
    /// Divergence categories (closed set) applied to every command of the
    /// rung; only categories with a default normalizer are accepted here.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Fixture-relative allowlist file for this rung (overrides `[fixture]`).
    #[serde(default)]
    pub allowlist: Option<String>,
    /// HTTP rung (L7): served by `rphp -S` vs `php -S` once SAPI-3 exists;
    /// parsed and reported as skipped until then.
    #[serde(default)]
    pub http: bool,
    /// Document root for an HTTP rung.
    #[serde(default)]
    pub docroot: Option<String>,
    /// Requests of an HTTP rung (`"GET /"`).
    #[serde(default)]
    pub requests: Vec<String>,
    /// A stateful flow: each side keeps a cookie jar across its requests
    /// (sessions, logins), every non-GET carries a same-origin `Origin`
    /// header (stateless CSRF checks), and a request may quote the previous
    /// response's form fields as `${input:NAME}` (a CSRF token).
    #[serde(default)]
    pub cookies: bool,
}

/// One entry of `commands`: a plain command line or a table with options.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    /// `"relative/script.php arg…"`.
    Line(String),
    /// `{ run = "…", … }`.
    Table(CommandTable),
}

/// The table form of a command.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandTable {
    /// The command line (script relative to the working copy, then arguments).
    pub run: String,
    /// Exit code both sides must produce (default: rphp must match php).
    #[serde(default)]
    pub expect_exit: Option<i32>,
    /// Divergence categories for this command (default normalizers only).
    #[serde(default)]
    pub allow: Vec<String>,
    /// stdin for this command.
    #[serde(default)]
    pub stdin: Option<String>,
    /// Extra environment for this command (on top of the rung's).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// One command with the rung-level defaults folded in and the line split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    /// The command line as written (also the allowlist name's tail).
    pub line: String,
    /// The script, relative to the working copy.
    pub script: String,
    /// Arguments after the script.
    pub args: Vec<String>,
    /// Exit code both sides must produce, if pinned.
    pub expect_exit: Option<i32>,
    /// Inline divergence categories.
    pub allow: Vec<Category>,
    /// stdin bytes, if any.
    pub stdin: Option<Vec<u8>>,
    /// Environment on top of the scrubbed one.
    pub env: BTreeMap<String, String>,
}

impl RungSpec {
    /// Fold rung defaults into each command and split the command lines.
    pub fn resolved_commands(&self) -> Result<Vec<ResolvedCommand>, String> {
        let rung_allow = parse_categories(&self.allow)
            .map_err(|e| format!("rung `{}`: {e}", self.id))?;
        let mut out = Vec::with_capacity(self.commands.len());
        for (i, spec) in self.commands.iter().enumerate() {
            let (line, expect_exit, allow, stdin, env) = match spec {
                CommandSpec::Line(l) => (l.clone(), None, Vec::new(), None, BTreeMap::new()),
                CommandSpec::Table(t) => (
                    t.run.clone(),
                    t.expect_exit,
                    parse_categories(&t.allow)
                        .map_err(|e| format!("rung `{}` command {}: {e}", self.id, i + 1))?,
                    t.stdin.clone(),
                    t.env.clone(),
                ),
            };
            let words = split_command(&line)
                .map_err(|e| format!("rung `{}` command {}: {e}", self.id, i + 1))?;
            let Some((script, args)) = words.split_first() else {
                return Err(format!("rung `{}` command {}: empty command line", self.id, i + 1));
            };
            let mut categories = rung_allow.clone();
            for c in allow {
                if !categories.contains(&c) {
                    categories.push(c);
                }
            }
            let mut merged_env = self.env.clone();
            merged_env.extend(env);
            out.push(ResolvedCommand {
                line: line.trim().to_string(),
                script: script.clone(),
                args: args.to_vec(),
                expect_exit,
                allow: categories,
                stdin: stdin.or_else(|| self.stdin.clone()).map(String::into_bytes),
                env: merged_env,
            });
        }
        Ok(out)
    }
}

/// Parse closed-set category names for an inline `allow` list; categories
/// without a default normalizer must go through an allowlist file instead.
pub fn parse_categories(names: &[String]) -> Result<Vec<Category>, String> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let Some(cat) = Category::parse(name.trim()) else {
            let valid: Vec<&str> = Category::ALL.iter().map(|c| c.name()).collect();
            return Err(format!(
                "unknown divergence category `{name}`; the closed set is: {}",
                valid.join(", ")
            ));
        };
        if !cat.has_default() {
            return Err(format!(
                "category `{cat}` has no default normalizer; put it in an allowlist file with an explicit `normalize` rule"
            ));
        }
        if !out.contains(&cat) {
            out.push(cat);
        }
    }
    Ok(out)
}

/// Split a command line into words: whitespace separated, `'…'` and `"…"`
/// quoting, backslash escapes outside single quotes.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated single quote".to_string()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '\\' | '$' | '`')) => cur.push(e),
                            Some('n') => cur.push('\n'),
                            Some('t') => cur.push('\t'),
                            Some(other) => {
                                cur.push('\\');
                                cur.push(other);
                            }
                            None => return Err("unterminated double quote".to_string()),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated double quote".to_string()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(e) => cur.push(e),
                    None => return Err("trailing backslash".to_string()),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

/// A discovered fixture: its directory and validated `ladder.toml`.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The fixture directory.
    pub dir: PathBuf,
    /// The parsed file.
    pub file: LadderFile,
}

impl Fixture {
    /// Load and validate `<dir>/ladder.toml`.
    pub fn load(dir: &Path) -> Result<Fixture, String> {
        let path = dir.join("ladder.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let file: LadderFile =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let fx = Fixture { dir: dir.to_path_buf(), file };
        fx.validate().map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(fx)
    }

    fn validate(&self) -> Result<(), String> {
        if self.file.fixture.name.trim().is_empty() {
            return Err("[fixture] name must not be empty".to_string());
        }
        if let Some(v) = &self.file.fixture.php_min {
            parse_version(v).ok_or_else(|| format!("[fixture] php_min `{v}` is not a version"))?;
        }
        if let Some(rel) = &self.file.fixture.allowlist {
            let p = self.dir.join(rel);
            if !p.is_file() {
                return Err(format!("[fixture] allowlist `{rel}` does not exist"));
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        for rung in &self.file.rung {
            if rung.id.trim().is_empty() || rung.id.contains('/') || rung.id.contains(char::is_whitespace) {
                return Err(format!("rung id `{}` must be a single non-empty word", rung.id));
            }
            if seen.contains(&rung.id.as_str()) {
                return Err(format!("duplicate rung id `{}`", rung.id));
            }
            seen.push(&rung.id);
            if rung.http {
                if rung.docroot.is_none() {
                    return Err(format!("rung `{}`: http rungs need `docroot`", rung.id));
                }
                if rung.requests.is_empty() {
                    return Err(format!("rung `{}`: http rungs need `requests`", rung.id));
                }
            } else if rung.commands.is_empty() {
                return Err(format!("rung `{}` has no commands", rung.id));
            }
            if let Some(rel) = &rung.allowlist {
                if !self.dir.join(rel).is_file() {
                    return Err(format!("rung `{}`: allowlist `{rel}` does not exist", rung.id));
                }
            }
            rung.resolved_commands()?;
        }
        Ok(())
    }

    /// The `[fixture] name`.
    pub fn name(&self) -> &str {
        &self.file.fixture.name
    }

    /// The directory name (used for the working-copy path).
    pub fn dir_name(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.name().to_string())
    }

    /// Whether the fixture is a Composer project (has `composer.json`).
    pub fn has_composer(&self) -> bool {
        self.dir.join("composer.json").is_file()
    }

    /// Whether `vendor/autoload.php` exists.
    pub fn vendor_installed(&self) -> bool {
        self.dir.join("vendor").join("autoload.php").is_file()
    }

    /// Whether `--fixture <sel>` selects this fixture.
    pub fn matches(&self, sel: &str) -> bool {
        self.name() == sel || self.dir_name() == sel
    }
}

/// Every `<root>/*/ladder.toml`, sorted by directory name.
pub fn discover(root: &Path) -> Result<Vec<Fixture>, String> {
    let entries = std::fs::read_dir(root)
        .map_err(|e| format!("cannot read fixtures root {}: {e}", root.display()))?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("ladder.toml").is_file())
        .collect();
    dirs.sort();
    dirs.iter().map(|d| Fixture::load(d)).collect()
}

/// Parse `major.minor.patch` (extra components / suffixes ignored).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().split(['-', '+', '~']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = it.next()??;
    let minor = it.next().unwrap_or(Some(0))?;
    let patch = it.next().unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

/// Whether version `have` is at least `want`.
pub fn version_at_least(have: &str, want: &str) -> bool {
    match (parse_version(have), parse_version(want)) {
        (Some(h), Some(w)) => h >= w,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// options
// ---------------------------------------------------------------------------

/// Which engine ran a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Stock php (the oracle).
    Php,
    /// rphp.
    Rphp,
}

impl Side {
    /// `php` / `rphp` — also the working-copy directory name.
    pub fn name(self) -> &'static str {
        match self {
            Side::Php => "php",
            Side::Rphp => "rphp",
        }
    }

    /// Parse `--only`.
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "php" => Some(Side::Php),
            "rphp" => Some(Side::Rphp),
            _ => None,
        }
    }
}

/// `--report` format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Human-readable text on stdout.
    Text,
    /// The JSON report on stdout.
    Json,
}

/// Parsed command-line options.
#[derive(Debug, Clone)]
pub struct Options {
    /// Fixtures root.
    pub fixtures_dir: PathBuf,
    /// Scratch root (`<target>/ladder/…`, `<target>/ladder-report.json`).
    pub target_dir: PathBuf,
    /// Selected rung ids (empty = all).
    pub rungs: Vec<String>,
    /// Selected fixture (name or directory name).
    pub fixture: Option<String>,
    /// Keep working copies.
    pub keep: bool,
    /// Run one side only.
    pub only: Option<Side>,
    /// Parallel rungs.
    pub jobs: usize,
    /// stdout format.
    pub report: ReportFormat,
    /// List and exit.
    pub list: bool,
    /// Exit 0 despite divergences.
    pub allow_failures: bool,
    /// Per-command timeout.
    pub timeout: Duration,
    /// Explicit php binary.
    pub php: Option<PathBuf>,
    /// Explicit rphp binary.
    pub rphp: Option<PathBuf>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Options {
    /// Parse `args`; `Ok(None)` when `--help` was given.
    pub fn parse(args: &[String]) -> Result<Option<Options>, Box<dyn std::error::Error>> {
        let mut pargs = pico_args::Arguments::from_vec(args.iter().map(OsString::from).collect());
        if pargs.contains(["-h", "--help"]) {
            return Ok(None);
        }
        let rungs: Vec<String> = pargs.values_from_str("--rung")?;
        let fixture: Option<String> = pargs.opt_value_from_str("--fixture")?;
        let keep = pargs.contains("--keep");
        let only = match pargs.opt_value_from_str::<_, String>("--only")? {
            None => None,
            Some(s) => Some(Side::parse(&s).ok_or_else(|| format!("--only expects php|rphp, got `{s}`"))?),
        };
        let jobs: usize = pargs.opt_value_from_str("--jobs")?.unwrap_or(1);
        let report = match pargs.opt_value_from_str::<_, String>("--report")?.as_deref() {
            None | Some("text") => ReportFormat::Text,
            Some("json") => ReportFormat::Json,
            Some(other) => return Err(format!("--report expects text|json, got `{other}`").into()),
        };
        let list = pargs.contains("--list");
        let allow_failures = pargs.contains("--allow-failures");
        let timeout: u64 = pargs.opt_value_from_str("--timeout")?.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let php: Option<PathBuf> = pargs.opt_value_from_str("--php")?;
        let rphp: Option<PathBuf> = pargs.opt_value_from_str("--rphp")?;
        let fixtures_dir: PathBuf = pargs
            .opt_value_from_str("--fixtures-dir")?
            .unwrap_or_else(|| repo_root().join("fixtures").join("ladder"));
        let target_dir: PathBuf = pargs
            .opt_value_from_str("--target-dir")?
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
            .unwrap_or_else(|| repo_root().join("target"));
        let rest = pargs.finish();
        if !rest.is_empty() {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string_lossy().into_owned()).collect();
            return Err(format!("unexpected argument(s): {}", rest.join(" ")).into());
        }
        Ok(Some(Options {
            fixtures_dir,
            target_dir,
            rungs,
            fixture,
            keep,
            only,
            jobs: jobs.max(1),
            report,
            list,
            allow_failures,
            timeout: Duration::from_secs(timeout.max(1)),
            php,
            rphp,
        }))
    }
}

// ---------------------------------------------------------------------------
// reports
// ---------------------------------------------------------------------------

/// Outcome of a rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RungStatus {
    /// Every compared command and artifact agrees (or, with `--only`, ran).
    Ok,
    /// At least one command or artifact diverged.
    Fail,
    /// Not run (HTTP rung).
    Skipped,
    /// The runner itself failed (copy, spawn, allowlist).
    Error,
}

/// Outcome of a command or artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    /// Both sides agree.
    Ok,
    /// Divergence (or, with `--only`, a pinned exit code / timeout violated).
    Fail,
    /// Ran on one side only (`--only`), nothing to compare.
    Ran,
    /// Not compared (`--only`).
    Skipped,
}

/// What one side produced for one command.
#[derive(Debug, Clone, Serialize)]
pub struct SideOutcome {
    /// Exit code (`128 + signal` when killed).
    pub exit: i32,
    /// Killed after the timeout.
    pub timed_out: bool,
    /// Wall time in milliseconds.
    pub duration_ms: u128,
    /// `<side>/out/<n>` prefix of the dumped `.stdout`/`.stderr`/`.exit`.
    pub out: String,
}

/// One compared command.
#[derive(Debug, Clone, Serialize)]
pub struct CommandReport {
    /// 1-based index within the rung (the `<n>` of `out/<n>.stdout`).
    pub index: usize,
    /// The command line.
    pub line: String,
    /// Outcome.
    pub status: ItemStatus,
    /// The first channel that disagreed.
    pub channel: Option<String>,
    /// Allowlist categories that applied.
    pub categories: Vec<String>,
    /// The php side.
    pub php: Option<SideOutcome>,
    /// The rphp side.
    pub rphp: Option<SideOutcome>,
    /// Full unified diff (or exit explanation) around the first difference.
    pub diff: Option<String>,
    /// Last lines of rphp's stderr when the command failed.
    pub rphp_stderr_tail: Option<String>,
    /// Extra explanation (pinned exit code violated, timeout).
    pub note: Option<String>,
}

/// One artifact pattern / file.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactReport {
    /// The pattern from `ladder.toml`.
    pub pattern: String,
    /// The matched file (working-copy relative) when the row is about one file.
    pub path: Option<String>,
    /// Outcome.
    pub status: ItemStatus,
    /// Diff or explanation.
    pub detail: Option<String>,
}

/// One rung.
#[derive(Debug, Clone, Serialize)]
pub struct RungReport {
    /// Fixture name.
    pub fixture: String,
    /// Rung id.
    pub id: String,
    /// Rung title.
    pub title: String,
    /// Outcome.
    pub status: RungStatus,
    /// Why skipped / what errored.
    pub note: Option<String>,
    /// Commands in order.
    pub commands: Vec<CommandReport>,
    /// Artifact rows.
    pub artifacts: Vec<ArtifactReport>,
    /// Wall time in milliseconds.
    pub duration_ms: u128,
    /// Working-copy directories per side (kept or not).
    pub workdirs: BTreeMap<String, String>,
    /// Whether the working copies were kept.
    pub kept: bool,
}

impl RungReport {
    fn new(fixture: &str, rung: &RungSpec) -> Self {
        RungReport {
            fixture: fixture.to_string(),
            id: rung.id.clone(),
            title: rung.title.clone(),
            status: RungStatus::Ok,
            note: None,
            commands: Vec::new(),
            artifacts: Vec::new(),
            duration_ms: 0,
            workdirs: BTreeMap::new(),
            kept: false,
        }
    }

    fn error(mut self, msg: impl Into<String>) -> Self {
        self.status = RungStatus::Error;
        self.note = Some(msg.into());
        self
    }

    /// Whether the rung counts as a failure for the exit status.
    pub fn failed(&self) -> bool {
        matches!(self.status, RungStatus::Fail | RungStatus::Error)
    }

    /// The first failing command, if any.
    pub fn first_divergence(&self) -> Option<&CommandReport> {
        self.commands.iter().find(|c| c.status == ItemStatus::Fail)
    }
}

/// Totals over every rung.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    /// Rungs selected.
    pub rungs: usize,
    /// Rungs with every item agreeing.
    pub ok: usize,
    /// Rungs with a divergence or runner error.
    pub failed: usize,
    /// Rungs skipped.
    pub skipped: usize,
    /// Commands run (per rung, not per side).
    pub commands: usize,
    /// Commands that diverged.
    pub commands_failed: usize,
    /// Artifact rows.
    pub artifacts: usize,
    /// Artifact rows that diverged.
    pub artifacts_failed: usize,
}

/// The whole run (`target/ladder-report.json`).
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// The php binary used.
    pub php: Option<String>,
    /// The rphp binary used.
    pub rphp: Option<String>,
    /// `--only`.
    pub only: Option<Side>,
    /// Every rung in selection order.
    pub rungs: Vec<RungReport>,
    /// Totals.
    pub summary: Summary,
}

impl Report {
    fn summarize(&mut self) {
        let mut s = Summary { rungs: self.rungs.len(), ..Summary::default() };
        for r in &self.rungs {
            if r.status == RungStatus::Skipped {
                s.skipped += 1;
            } else if r.failed() {
                s.failed += 1;
            } else {
                s.ok += 1;
            }
            s.commands += r.commands.len();
            s.commands_failed += r.commands.iter().filter(|c| c.status == ItemStatus::Fail).count();
            s.artifacts += r.artifacts.len();
            s.artifacts_failed += r.artifacts.iter().filter(|a| a.status == ItemStatus::Fail).count();
        }
        self.summary = s;
    }
}

// ---------------------------------------------------------------------------
// working copies
// ---------------------------------------------------------------------------

/// Create the fresh working copy `dst` of the fixture `src`: every entry is
/// copied except `vendor/` (hard-linked file by file, copied where linking
/// fails), `var/` (created empty but for `var_keep`), `node_modules/` and
/// `.git/` (skipped).
pub fn prepare_workdir(src: &Path, dst: &Path, var_keep: &[String]) -> io::Result<()> {
    if dst.exists() {
        std::fs::remove_dir_all(dst)?;
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let from = entry.path();
        let to = dst.join(&name);
        if SKIPPED_ENTRIES.contains(&name_str.as_ref()) {
            continue;
        }
        match name_str.as_ref() {
            "vendor" => link_tree(&from, &to)?,
            "var" => {
                std::fs::create_dir_all(&to)?;
                for keep in var_keep {
                    let kept = from.join(keep);
                    if kept.exists() {
                        copy_tree(&kept, &to.join(keep))?;
                    }
                }
            }
            _ => copy_tree(&from, &to)?,
        }
    }
    std::fs::create_dir_all(dst.join("var"))?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        return copy_symlink(from, to);
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

fn link_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        return copy_symlink(from, to);
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            link_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(from, to).map(|_| ()),
    }
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    let target = std::fs::read_link(from)?;
    std::os::unix::fs::symlink(target, to)
}

#[cfg(not(unix))]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::copy(from, to).map(|_| ())
}

/// Remove everything in `side_dir` except `out/`.
fn prune_workdir(side_dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(side_dir)? {
        let entry = entry?;
        if entry.file_name() == "out" {
            continue;
        }
        let p = entry.path();
        if std::fs::symlink_metadata(&p)?.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// The byte forms of `work` (as given and canonicalized) that are rewritten
/// to `%FIXTURE%`, longest first.
fn fixture_needles(work: &Path) -> Vec<Vec<u8>> {
    let mut v = vec![path_bytes(work)];
    if let Ok(canon) = work.canonicalize() {
        v.push(path_bytes(&canon));
    }
    v.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    v.dedup();
    v.retain(|p| !p.is_empty() && p != b"/");
    v
}

#[cfg(unix)]
fn path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(p: &Path) -> Vec<u8> {
    p.to_string_lossy().into_owned().into_bytes()
}

/// Rewrite every working-copy path in `bytes` to `%FIXTURE%`.
pub fn normalize_fixture_paths(bytes: &[u8], needles: &[Vec<u8>]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for n in needles {
        out = replace_bytes(&out, n, FIXTURE_PLACEHOLDER);
    }
    out
}

/// One line of an HTTP rung's `requests`: `METHOD /path [body]`, the body
/// sent url-encoded (`application/x-www-form-urlencoded`).
#[derive(Debug, Clone)]
struct HttpRequest {
    line: String,
    method: String,
    target: String,
    body: Option<String>,
}

/// What a `cookies = true` rung carries from one request to the next on
/// one side: the cookie jar and the previous response's body (for
/// `${input:NAME}`).
#[derive(Default)]
struct FlowState {
    jar: Vec<(String, String)>,
    last_body: Vec<u8>,
}

impl FlowState {
    /// Record the `Set-Cookie` headers of a raw response and keep its body.
    fn absorb(&mut self, response: &[u8]) {
        let head_end = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(response.len(), |i| i + 4);
        let head = String::from_utf8_lossy(&response[..head_end]);
        for line in head.lines() {
            let Some(rest) = line
                .get(..11)
                .filter(|p| p.eq_ignore_ascii_case("set-cookie:"))
                .map(|_| line[11..].trim())
            else {
                continue;
            };
            let mut attrs = rest.split(';').map(str::trim);
            let Some((name, value)) = attrs.next().and_then(|nv| nv.split_once('=')) else {
                continue;
            };
            // `Max-Age=0` / a past `expires` deletes the cookie, as php's
            // `setcookie('x', '', 1)` spells it.
            let deleted = value == "deleted"
                || attrs.clone().any(|a| a.eq_ignore_ascii_case("max-age=0"));
            self.jar.retain(|(n, _)| n != name);
            if !deleted {
                self.jar.push((name.to_string(), value.to_string()));
            }
        }
        self.last_body = response[head_end..].to_vec();
    }

    /// The `Cookie:` header value, or `None` with an empty jar.
    fn cookie_header(&self) -> Option<String> {
        if self.jar.is_empty() {
            return None;
        }
        Some(self.jar.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; "))
    }

    /// The request with its `${input:NAME}` placeholders filled from the
    /// previous response's `<input name="NAME" … value="…">` (any attribute
    /// order); an absent field becomes the empty string.
    fn fill(&self, req: &HttpRequest) -> HttpRequest {
        let mut out = req.clone();
        out.target = self.fill_str(&req.target);
        out.body = req.body.as_deref().map(|b| self.fill_str(b));
        out
    }

    fn fill_str(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find("${input:") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 8..];
            let Some(end) = after.find('}') else {
                out.push_str(&rest[i..]);
                return out;
            };
            out.push_str(&self.input_value(&after[..end]));
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    fn input_value(&self, name: &str) -> String {
        let body = String::from_utf8_lossy(&self.last_body);
        let mut rest = body.as_ref();
        while let Some(i) = rest.find("<input") {
            let tag = &rest[i..];
            let end = tag.find('>').unwrap_or(tag.len());
            let tag = &tag[..end];
            if attr(tag, "name").as_deref() == Some(name) {
                return attr(tag, "value").unwrap_or_default();
            }
            rest = &rest[i + 6..];
        }
        String::new()
    }
}

/// The value of attribute `key` in one tag's text (double-quoted).
fn attr(tag: &str, key: &str) -> Option<String> {
    let needle = format!(" {key}=\"");
    let i = tag.find(&needle)?;
    let after = &tag[i + needle.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

impl HttpRequest {
    fn parse(line: &str) -> Result<HttpRequest, String> {
        let mut parts = line.splitn(3, ' ');
        let method = parts.next().filter(|m| !m.is_empty()).ok_or_else(|| format!("request `{line}`: no method"))?;
        let target = parts
            .next()
            .filter(|t| t.starts_with('/'))
            .ok_or_else(|| format!("request `{line}`: expected `METHOD /path`"))?;
        Ok(HttpRequest {
            line: line.to_string(),
            method: method.to_string(),
            target: target.to_string(),
            body: parts.next().map(str::to_string),
        })
    }
}

/// A port nothing listens on right now.
fn free_port() -> io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Poll until the server accepts connections, or it exits / the deadline
/// passes.
fn wait_ready(port: u16, child: &mut std::process::Child, deadline: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("server exited before listening ({status})"));
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if start.elapsed() > deadline {
            return Err(format!("server did not listen within {}s", deadline.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Send one request and read the whole response (the servers close the
/// connection after it). The HTTP status is the `RunResult`'s exit code,
/// the raw response its stdout.
fn http_exchange(port: u16, req: &HttpRequest, timeout: Duration, flow: Option<&mut FlowState>) -> RunResult {
    let failed = |status: i32, msg: String| RunResult {
        stdout: Vec::new(),
        stderr: msg.into_bytes(),
        status,
        timed_out: false,
    };
    let mut stream = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => return failed(-1, format!("connect: {e}")),
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let filled = flow.as_ref().map(|f| f.fill(req));
    let req = filled.as_ref().unwrap_or(req);
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: ladder\r\nAccept: */*\r\nConnection: close\r\n",
        req.method, req.target
    );
    if let Some(flow) = &flow {
        if let Some(cookie) = flow.cookie_header() {
            head.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        if req.method != "GET" {
            head.push_str(&format!("Origin: http://127.0.0.1:{port}\r\n"));
        }
    }
    if let Some(body) = &req.body {
        head.push_str(&format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    if let Err(e) = stream.write_all(head.as_bytes()) {
        return failed(-1, format!("send: {e}"));
    }
    if let Some(body) = &req.body {
        if let Err(e) = stream.write_all(body.as_bytes()) {
            return failed(-1, format!("send: {e}"));
        }
    }
    let mut response = Vec::new();
    let mut timed_out = false;
    if let Err(e) = stream.read_to_end(&mut response) {
        if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
            timed_out = true;
        } else if response.is_empty() {
            return failed(-1, format!("read: {e}"));
        }
    }
    // `HTTP/1.1 404 Not Found` → 404.
    let status = response
        .split(|b| *b == b' ')
        .nth(1)
        .and_then(|s| std::str::from_utf8(s).ok())
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(-1);
    if let Some(flow) = flow {
        flow.absorb(&response);
    }
    RunResult {
        stdout: normalize_http(response, port),
        stderr: Vec::new(),
        status,
        timed_out,
    }
}

/// What every response carries that no engine decides: the port each side
/// happened to get (echoed in `Host:` and any absolute URL) and the `Date`
/// header's clock reading. Both become placeholders before comparison.
fn normalize_http(response: Vec<u8>, port: u16) -> Vec<u8> {
    let with_port = replace_bytes(&response, format!("127.0.0.1:{port}").as_bytes(), b"127.0.0.1:%PORT%");
    let with_port = replace_bytes(&with_port, format!("127.0.0.1%3A{port}").as_bytes(), b"127.0.0.1%3A%PORT%");
    // The head ends at the first blank line; only its `Date:` field is
    // replaced.
    let head_end = with_port
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(with_port.len(), |p| p + 4);
    let (head, body) = with_port.split_at(head_end);
    let mut out = Vec::with_capacity(with_port.len());
    for line in head.split_inclusive(|b| *b == b'\n') {
        if line.len() > 5 && line[..5].eq_ignore_ascii_case(b"date:") {
            out.extend_from_slice(b"Date: %DATE%\r\n");
        } else {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(body);
    out
}

fn normalize_result(res: &RunResult, needles: &[Vec<u8>]) -> RunResult {
    RunResult {
        stdout: normalize_fixture_paths(&res.stdout, needles),
        stderr: normalize_fixture_paths(&res.stderr, needles),
        status: res.status,
        timed_out: res.timed_out,
    }
}

// ---------------------------------------------------------------------------
// processes
// ---------------------------------------------------------------------------

/// Spawn `cmd` with piped stdout/stderr, feed `stdin` (or `/dev/null`), wait
/// up to `timeout`.
fn run_process(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> io::Result<RunResult> {
    let Some(input) = stdin else {
        return run_command(cmd, timeout);
    };
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut in_pipe = child.stdin.take().expect("stdin is piped");
    let input = input.to_vec();
    let in_thread = std::thread::spawn(move || {
        let _ = in_pipe.write_all(&input);
        drop(in_pipe);
    });
    let mut out_pipe = child.stdout.take().expect("stdout is piped");
    let mut err_pipe = child.stderr.take().expect("stderr is piped");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let _ = in_thread.join();
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(RunResult { stdout, stderr, status: exit_code(&status), timed_out })
}

#[cfg(unix)]
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal().map(|s| 128 + s)).unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// Write `<out_dir>/<n>.stdout`, `.stderr`, `.exit`.
fn dump_result(out_dir: &Path, n: usize, res: &RunResult) -> io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join(format!("{n}.stdout")), &res.stdout)?;
    std::fs::write(out_dir.join(format!("{n}.stderr")), &res.stderr)?;
    let exit = if res.timed_out {
        format!("{} (timed out)\n", res.status)
    } else {
        format!("{}\n", res.status)
    };
    std::fs::write(out_dir.join(format!("{n}.exit")), exit)
}

fn query_php_version(php: &Path) -> Result<String, String> {
    let out = Command::new(php)
        .args(["-n", "-r", "echo PHP_VERSION;"])
        .output()
        .map_err(|e| format!("cannot run {}: {e}", php.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} -r 'echo PHP_VERSION;' failed: {}",
            php.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ---------------------------------------------------------------------------
// the runner
// ---------------------------------------------------------------------------

struct TimedRun {
    result: RunResult,
    duration: Duration,
}

struct SideRuns {
    php: Option<Vec<TimedRun>>,
    rphp: Option<Vec<TimedRun>>,
}

/// Runs rungs.
pub struct Runner {
    /// Options.
    pub opts: Options,
    /// The stock php (absent only with `--only rphp`).
    pub php: Option<PathBuf>,
    /// The rphp binary (absent only with `--only php`).
    pub rphp: Option<PathBuf>,
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The allowlist name of a command: `<rung id>/<command line>`.
pub fn command_name(rung_id: &str, line: &str) -> String {
    format!("{rung_id}/{line}")
}

/// The rung's allowlist: the fixture/rung file (if any) plus one synthesized
/// entry per inline `allow` category.
pub fn build_allowlist(fx: &Fixture, rung: &RungSpec, commands: &[ResolvedCommand]) -> Result<Allowlist, String> {
    let mut text = String::new();
    let file = rung.allowlist.as_ref().or(fx.file.fixture.allowlist.as_ref()).map(|rel| fx.dir.join(rel));
    if let Some(path) = &file {
        text.push_str(
            &std::fs::read_to_string(path).map_err(|e| format!("cannot read allowlist {}: {e}", path.display()))?,
        );
        text.push('\n');
    }
    for cmd in commands {
        for cat in &cmd.allow {
            let _ = writeln!(
                text,
                "[[allow]]\nsnippet = {}\ncategory = \"{}\"\nreason = \"inline `allow` in ladder.toml\"\n",
                toml_string(&command_name(&rung.id, &cmd.line)),
                cat.name()
            );
        }
    }
    Allowlist::parse(&text).map_err(|e| match &file {
        Some(p) => format!("{}: {e}", p.display()),
        None => format!("inline allow: {e}"),
    })
}

impl Runner {
    fn side_binary(&self, side: Side) -> &Path {
        match side {
            Side::Php => self.php.as_deref().expect("php present when its side runs"),
            Side::Rphp => self.rphp.as_deref().expect("rphp present when its side runs"),
        }
    }

    fn run_one(&self, side: Side, cmd: &ResolvedCommand, work: &Path) -> io::Result<TimedRun> {
        let bin = self.side_binary(side);
        let script = Path::new(&cmd.script);
        let mut command = match side {
            Side::Php => php_command(bin, script, &cmd.args, work),
            Side::Rphp => rphp_command(bin, script, &cmd.args, work),
        };
        command.envs(&cmd.env);
        let start = Instant::now();
        let result = run_process(command, cmd.stdin.as_deref(), self.opts.timeout)?;
        Ok(TimedRun { result, duration: start.elapsed() })
    }

    /// Run every command of a rung on one side in the prepared `work` copy,
    /// dump the outputs, then move the copy to `<rung_dir>/<side>/`.
    fn run_side(
        &self,
        side: Side,
        commands: &[ResolvedCommand],
        rung_dir: &Path,
        work: &Path,
    ) -> Result<Vec<TimedRun>, String> {
        let out_dir = work.join("out");
        let mut runs = Vec::with_capacity(commands.len());
        for (i, cmd) in commands.iter().enumerate() {
            let run = self
                .run_one(side, cmd, work)
                .map_err(|e| format!("{} `{}`: cannot run: {e}", side.name(), cmd.line))?;
            dump_result(&out_dir, i + 1, &run.result)
                .map_err(|e| format!("cannot write {}: {e}", out_dir.display()))?;
            runs.push(run);
        }
        let side_dir = rung_dir.join(side.name());
        std::fs::rename(work, &side_dir)
            .map_err(|e| format!("cannot move {} to {}: {e}", work.display(), side_dir.display()))?;
        Ok(runs)
    }

    /// Run one rung: fresh copies, both sides, comparison, artifacts, cleanup.
    pub fn run_rung(&self, fx: &Fixture, rung: &RungSpec) -> RungReport {
        let start = Instant::now();
        let mut rep = RungReport::new(fx.name(), rung);
        rep.kept = self.opts.keep;
        if rung.http {
            return self.run_http_rung(fx, rung, rep, start);
        }
        let commands = match rung.resolved_commands() {
            Ok(c) => c,
            Err(e) => return rep.error(e),
        };
        let allowlist = match build_allowlist(fx, rung, &commands) {
            Ok(a) => a,
            Err(e) => return rep.error(e),
        };

        let rung_dir = self.opts.target_dir.join("ladder").join(fx.dir_name()).join(&rung.id);
        if let Err(e) = std::fs::remove_dir_all(&rung_dir) {
            if e.kind() != io::ErrorKind::NotFound {
                return rep.error(format!("cannot clear {}: {e}", rung_dir.display()));
            }
        }
        if let Err(e) = std::fs::create_dir_all(&rung_dir) {
            return rep.error(format!("cannot create {}: {e}", rung_dir.display()));
        }
        let work = rung_dir.join("work");
        let sides: Vec<Side> = match self.opts.only {
            Some(s) => vec![s],
            None => vec![Side::Php, Side::Rphp],
        };

        let mut runs = SideRuns { php: None, rphp: None };
        let mut needles: Vec<Vec<u8>> = Vec::new();
        for &side in &sides {
            if let Err(e) = prepare_workdir(&fx.dir, &work, &fx.file.fixture.var_keep) {
                let msg = format!("cannot create working copy {}: {e}", work.display());
                return self.finish(rep.error(msg), &rung_dir, &sides, start);
            }
            // Compute the needles while `work` exists (canonicalize needs it).
            needles = fixture_needles(&work);
            let outcome = self.run_side(side, &commands, &rung_dir, &work);
            rep.workdirs.insert(side.name().to_string(), rung_dir.join(side.name()).display().to_string());
            match outcome {
                Ok(r) => match side {
                    Side::Php => runs.php = Some(r),
                    Side::Rphp => runs.rphp = Some(r),
                },
                Err(e) => return self.finish(rep.error(e), &rung_dir, &sides, start),
            }
        }

        // Per-command comparison.
        for (i, cmd) in commands.iter().enumerate() {
            let n = i + 1;
            let name = command_name(&rung.id, &cmd.line);
            let categories: Vec<String> = allowlist.categories_for(&name).iter().map(|c| c.name().to_string()).collect();
            let outcome = |side: Side, run: &TimedRun| SideOutcome {
                exit: run.result.status,
                timed_out: run.result.timed_out,
                duration_ms: run.duration.as_millis(),
                out: rung_dir.join(side.name()).join("out").join(n.to_string()).display().to_string(),
            };
            let php_run = runs.php.as_ref().map(|v| &v[i]);
            let rphp_run = runs.rphp.as_ref().map(|v| &v[i]);
            let mut crep = CommandReport {
                index: n,
                line: cmd.line.clone(),
                status: ItemStatus::Ran,
                channel: None,
                categories,
                php: php_run.map(|r| outcome(Side::Php, r)),
                rphp: rphp_run.map(|r| outcome(Side::Rphp, r)),
                diff: None,
                rphp_stderr_tail: None,
                note: None,
            };
            let mut notes: Vec<String> = Vec::new();
            for (side, run) in [(Side::Php, php_run), (Side::Rphp, rphp_run)] {
                let Some(run) = run else { continue };
                if run.result.timed_out {
                    notes.push(format!("{} timed out after {}s", side.name(), self.opts.timeout.as_secs()));
                    crep.status = ItemStatus::Fail;
                }
                if let Some(exp) = cmd.expect_exit {
                    if run.result.status != exp {
                        let who = if side == Side::Php { "php (fixture/oracle problem)" } else { "rphp" };
                        notes.push(format!("{who} exited {}, ladder.toml expects {exp}", run.result.status));
                        crep.status = ItemStatus::Fail;
                        if crep.channel.is_none() {
                            crep.channel = Some(Channel::Exit.to_string());
                        }
                    }
                }
            }
            if let (Some(php_run), Some(rphp_run)) = (php_run, rphp_run) {
                let script_abs = work.join(&cmd.script);
                let mut policy = Policy::new(&name, &allowlist);
                policy.php_ctx = NormalizeContext::for_script(&script_abs, &work);
                policy.rphp_ctx = NormalizeContext::for_script(&script_abs, &work);
                let php_n = normalize_result(&php_run.result, &needles);
                let rphp_n = normalize_result(&rphp_run.result, &needles);
                match compare(&php_n, &rphp_n, &policy) {
                    Verdict::Match => {
                        if crep.status != ItemStatus::Fail {
                            crep.status = ItemStatus::Ok;
                        }
                    }
                    Verdict::Mismatch { channel, first_diff } => {
                        crep.status = ItemStatus::Fail;
                        crep.channel = Some(channel.to_string());
                        crep.diff = Some(first_diff);
                    }
                }
                if crep.status == ItemStatus::Fail && !rphp_n.stderr.is_empty() {
                    crep.rphp_stderr_tail = Some(tail_output(&rphp_n.stderr, STDERR_TAIL));
                }
            }
            if !notes.is_empty() {
                crep.note = Some(notes.join("; "));
            }
            rep.commands.push(crep);
        }

        // Artifacts.
        if runs.php.is_some() && runs.rphp.is_some() {
            let php_dir = rung_dir.join(Side::Php.name());
            let rphp_dir = rung_dir.join(Side::Rphp.name());
            for pattern in &rung.artifacts {
                rep.artifacts.extend(compare_artifacts(pattern, &php_dir, &rphp_dir, &needles));
            }
        } else {
            for pattern in &rung.artifacts {
                rep.artifacts.push(ArtifactReport {
                    pattern: pattern.clone(),
                    path: None,
                    status: ItemStatus::Skipped,
                    detail: Some("not compared (--only)".to_string()),
                });
            }
        }

        if rep.commands.iter().any(|c| c.status == ItemStatus::Fail)
            || rep.artifacts.iter().any(|a| a.status == ItemStatus::Fail)
        {
            rep.status = RungStatus::Fail;
        }
        self.finish(rep, &rung_dir, &sides, start)
    }

    /// An HTTP rung: each side serves its working copy's document root
    /// with its built-in server (`php -n -S` / `rphp -S`, the oracle's ini
    /// pinned on both), every request in `requests` is sent to it, and the
    /// raw responses (status line, headers, body) are compared the way a
    /// command's stdout is — the HTTP status standing in for the exit code.
    fn run_http_rung(&self, fx: &Fixture, rung: &RungSpec, mut rep: RungReport, start: Instant) -> RungReport {
        let Some(docroot) = rung.docroot.clone() else {
            return rep.error("http rung without a docroot".to_string());
        };
        let requests: Vec<HttpRequest> = match rung.requests.iter().map(|l| HttpRequest::parse(l)).collect() {
            Ok(r) => r,
            Err(e) => return rep.error(e),
        };
        let allowlist = match build_allowlist(fx, rung, &[]) {
            Ok(a) => a,
            Err(e) => return rep.error(e),
        };
        let rung_dir = self.opts.target_dir.join("ladder").join(fx.dir_name()).join(&rung.id);
        if let Err(e) = std::fs::remove_dir_all(&rung_dir) {
            if e.kind() != io::ErrorKind::NotFound {
                return rep.error(format!("cannot clear {}: {e}", rung_dir.display()));
            }
        }
        if let Err(e) = std::fs::create_dir_all(&rung_dir) {
            return rep.error(format!("cannot create {}: {e}", rung_dir.display()));
        }
        let work = rung_dir.join("work");
        let sides: Vec<Side> = match self.opts.only {
            Some(s) => vec![s],
            None => vec![Side::Php, Side::Rphp],
        };
        let mut runs = SideRuns { php: None, rphp: None };
        let mut needles: Vec<Vec<u8>> = Vec::new();
        for &side in &sides {
            if let Err(e) = prepare_workdir(&fx.dir, &work, &fx.file.fixture.var_keep) {
                let msg = format!("cannot create working copy {}: {e}", work.display());
                return self.finish(rep.error(msg), &rung_dir, &sides, start);
            }
            needles = fixture_needles(&work);
            let outcome = self.serve_side(side, rung, &docroot, &requests, &rung_dir, &work);
            rep.workdirs.insert(side.name().to_string(), rung_dir.join(side.name()).display().to_string());
            match outcome {
                Ok(r) => match side {
                    Side::Php => runs.php = Some(r),
                    Side::Rphp => runs.rphp = Some(r),
                },
                Err(e) => return self.finish(rep.error(e), &rung_dir, &sides, start),
            }
        }
        for (i, req) in requests.iter().enumerate() {
            let n = i + 1;
            let name = command_name(&rung.id, &req.line);
            let categories: Vec<String> = allowlist.categories_for(&name).iter().map(|c| c.name().to_string()).collect();
            let outcome = |side: Side, run: &TimedRun| SideOutcome {
                exit: run.result.status,
                timed_out: run.result.timed_out,
                duration_ms: run.duration.as_millis(),
                out: rung_dir.join(side.name()).join("out").join(n.to_string()).display().to_string(),
            };
            let php_run = runs.php.as_ref().map(|v| &v[i]);
            let rphp_run = runs.rphp.as_ref().map(|v| &v[i]);
            let mut crep = CommandReport {
                index: n,
                line: req.line.clone(),
                status: ItemStatus::Ran,
                channel: None,
                categories,
                php: php_run.map(|r| outcome(Side::Php, r)),
                rphp: rphp_run.map(|r| outcome(Side::Rphp, r)),
                diff: None,
                rphp_stderr_tail: None,
                note: None,
            };
            let mut notes: Vec<String> = Vec::new();
            for (side, run) in [(Side::Php, php_run), (Side::Rphp, rphp_run)] {
                let Some(run) = run else { continue };
                if run.result.timed_out {
                    notes.push(format!("{} timed out after {}s", side.name(), self.opts.timeout.as_secs()));
                    crep.status = ItemStatus::Fail;
                }
            }
            if let (Some(php_run), Some(rphp_run)) = (php_run, rphp_run) {
                let script_abs = work.join(&docroot).join("index.php");
                let mut policy = Policy::new(&name, &allowlist);
                policy.php_ctx = NormalizeContext::for_script(&script_abs, &work);
                policy.rphp_ctx = NormalizeContext::for_script(&script_abs, &work);
                let php_n = normalize_result(&php_run.result, &needles);
                let rphp_n = normalize_result(&rphp_run.result, &needles);
                match compare(&php_n, &rphp_n, &policy) {
                    Verdict::Match => {
                        if crep.status != ItemStatus::Fail {
                            crep.status = ItemStatus::Ok;
                        }
                    }
                    Verdict::Mismatch { channel, first_diff } => {
                        crep.status = ItemStatus::Fail;
                        crep.channel = Some(channel.to_string());
                        crep.diff = Some(first_diff);
                    }
                }
            }
            if !notes.is_empty() {
                crep.note = Some(notes.join("; "));
            }
            rep.commands.push(crep);
        }
        if rep.commands.iter().any(|c| c.status == ItemStatus::Fail) {
            rep.status = RungStatus::Fail;
        }
        self.finish(rep, &rung_dir, &sides, start)
    }

    /// Serve `work/<docroot>` on one side and collect the responses.
    fn serve_side(
        &self,
        side: Side,
        rung: &RungSpec,
        docroot: &str,
        requests: &[HttpRequest],
        rung_dir: &Path,
        work: &Path,
    ) -> Result<Vec<TimedRun>, String> {
        let out_dir = work.join("out");
        std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
        let port = free_port().map_err(|e| format!("no free port: {e}"))?;
        let bin = self.side_binary(side);
        let mut command = Command::new(bin);
        if side == Side::Php {
            command.arg("-n");
        }
        for (k, v) in PHP_INI {
            command.arg("-d").arg(format!("{k}={v}"));
        }
        command
            .arg("-S")
            .arg(format!("127.0.0.1:{port}"))
            .arg("-t")
            .arg(docroot)
            .current_dir(work);
        scrub_env(&mut command);
        command.envs(&rung.env);
        let log = std::fs::File::create(out_dir.join("server.log"))
            .map_err(|e| format!("cannot create server log: {e}"))?;
        let log2 = log.try_clone().map_err(|e| format!("cannot clone server log: {e}"))?;
        command.stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(log2));
        let mut child = command
            .spawn()
            .map_err(|e| format!("{} `{} -S`: cannot start: {e}", side.name(), bin.display()))?;
        let ready = wait_ready(port, &mut child, Duration::from_secs(20));
        let result = match ready {
            Ok(()) => {
                let mut runs = Vec::with_capacity(requests.len());
                let mut flow = rung.cookies.then(FlowState::default);
                for (i, req) in requests.iter().enumerate() {
                    let started = Instant::now();
                    let result = http_exchange(port, req, self.opts.timeout, flow.as_mut());
                    dump_result(&out_dir, i + 1, &result)
                        .map_err(|e| format!("cannot write {}: {e}", out_dir.display()))?;
                    runs.push(TimedRun { result, duration: started.elapsed() });
                }
                Ok(runs)
            }
            Err(e) => Err(format!("{} `{} -S 127.0.0.1:{port}`: {e}", side.name(), bin.display())),
        };
        let _ = child.kill();
        let _ = child.wait();
        let side_dir = rung_dir.join(side.name());
        std::fs::rename(work, &side_dir)
            .map_err(|e| format!("cannot move {} to {}: {e}", work.display(), side_dir.display()))?;
        result
    }

    fn finish(&self, mut rep: RungReport, rung_dir: &Path, sides: &[Side], start: Instant) -> RungReport {
        if !self.opts.keep {
            let work = rung_dir.join("work");
            if work.exists() {
                let _ = std::fs::remove_dir_all(&work);
            }
            for side in sides {
                let side_dir = rung_dir.join(side.name());
                if side_dir.exists() {
                    if let Err(e) = prune_workdir(&side_dir) {
                        let msg = format!("cleanup of {} failed: {e}", side_dir.display());
                        rep.note = Some(match rep.note.take() {
                            Some(n) => format!("{n}; {msg}"),
                            None => msg,
                        });
                    }
                }
            }
        }
        rep.duration_ms = start.elapsed().as_millis();
        rep
    }
}

/// Working-copy-relative names (forward slashes, sorted) of the files under
/// `root` matching `pattern`. `vendor/` is only searched when the pattern
/// starts with `vendor`; `out/` (the runner's own dumps) never.
pub fn matching_files(root: &Path, pattern: &str) -> Vec<String> {
    let search_vendor = pattern.starts_with("vendor");
    let mut out: Vec<String> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() != 1 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            name != "out" && (search_vendor || name != "vendor")
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| relative_name(Some(root), e.path()))
        .filter(|rel| glob_match(pattern, rel))
        .collect();
    out.sort();
    out
}

/// Whether a pattern's *directory* part carries a wildcard.
fn rel_has_wildcard_dir(pattern: &str) -> bool {
    match pattern.rfind('/') {
        Some(i) => pattern[..i].contains('*'),
        None => false,
    }
}

/// The values a Symfony container dump derives from the working copy's
/// path and the wall clock, which the two sides can never share:
/// `ContainerAbc1234` directory and class names (a hash of the dumped
/// files, which name the path), and the `container.build_hash` /
/// `build_id` / `build_time` parameters. Each is replaced by a placeholder
/// so the rest of the file can be compared.
pub fn normalize_artifact(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let hash = regex::Regex::new(r"Container[A-Za-z0-9_]{7}\b").unwrap();
    let text = hash.replace_all(&text, "Container%HASH%").into_owned();
    let build = regex::Regex::new(r"'container\.build_(hash|id|time)' => [^,\n]+,").unwrap();
    let text = build.replace_all(&text, "'container.build_$1' => %BUILD%,").into_owned();
    text.into_bytes()
}

/// Compare every file matching `pattern` on the php side against the rphp
/// side, byte-for-byte after `%FIXTURE%` normalization.
pub fn compare_artifacts(pattern: &str, php_dir: &Path, rphp_dir: &Path, needles: &[Vec<u8>]) -> Vec<ArtifactReport> {
    let php_files = matching_files(php_dir, pattern);
    let rphp_files = matching_files(rphp_dir, pattern);
    let mut rows = Vec::new();
    if php_files.is_empty() {
        rows.push(ArtifactReport {
            pattern: pattern.to_string(),
            path: None,
            status: ItemStatus::Fail,
            detail: Some(if rphp_files.is_empty() {
                "matched no file on either side (fixture problem?)".to_string()
            } else {
                format!("matched no file on the php side; rphp produced: {}", rphp_files.join(", "))
            }),
        });
        return rows;
    }
    let mut paired: Vec<String> = Vec::new();
    for rel in &php_files {
        let php_path = php_dir.join(rel);
        // A wildcard *directory* in the pattern (`var/cache/dev/Container*/`)
        // is one whose name php derives from the working copy's path — the
        // two sides can never share it — so the rphp file is the one with
        // the same name under the same pattern, when that is unambiguous.
        let rphp_rel = if rel_has_wildcard_dir(pattern) {
            let base = Path::new(rel).file_name().map(|f| f.to_string_lossy().into_owned());
            let candidates: Vec<&String> = rphp_files
                .iter()
                .filter(|r| Path::new(r).file_name().map(|f| f.to_string_lossy().into_owned()) == base)
                .collect();
            match candidates.as_slice() {
                [one] => (*one).clone(),
                _ => rel.clone(),
            }
        } else {
            rel.clone()
        };
        let rphp_path = rphp_dir.join(&rphp_rel);
        paired.push(rphp_rel.clone());
        let mut row = ArtifactReport { pattern: pattern.to_string(), path: Some(rel.clone()), status: ItemStatus::Ok, detail: None };
        let php_bytes = match std::fs::read(&php_path) {
            Ok(b) => normalize_artifact(&normalize_fixture_paths(&b, needles)),
            Err(e) => {
                row.status = ItemStatus::Fail;
                row.detail = Some(format!("cannot read php side: {e}"));
                rows.push(row);
                continue;
            }
        };
        match std::fs::read(&rphp_path) {
            Ok(b) => {
                let rphp_bytes = normalize_artifact(&normalize_fixture_paths(&b, needles));
                if php_bytes != rphp_bytes {
                    row.status = ItemStatus::Fail;
                    row.detail = Some(unified_diff(&php_bytes, &rphp_bytes, "php", "rphp"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                row.status = ItemStatus::Fail;
                row.detail = Some("missing on the rphp side".to_string());
            }
            Err(e) => {
                row.status = ItemStatus::Fail;
                row.detail = Some(format!("cannot read rphp side: {e}"));
            }
        }
        rows.push(row);
    }
    for rel in rphp_files.iter().filter(|r| !php_files.contains(r) && !paired.contains(r)) {
        rows.push(ArtifactReport {
            pattern: pattern.to_string(),
            path: Some(rel.clone()),
            status: ItemStatus::Fail,
            detail: Some("only produced by rphp".to_string()),
        });
    }
    rows
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

/// The first `n` lines of `s` (with a `… (N more lines)` marker).
pub fn head_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        return s.trim_end_matches('\n').to_string();
    }
    let mut out = lines[..n].join("\n");
    let _ = write!(out, "\n… ({} more lines)", lines.len() - n);
    out
}

/// The last `n` lines of raw process output, each rendered with
/// [`escape_bytes`] (split first, so newlines stay newlines).
pub fn tail_output(bytes: &[u8], n: usize) -> String {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let lines: Vec<&[u8]> = body.split(|&c| c == b'\n').collect();
    let start = lines.len().saturating_sub(n);
    let mut out = String::new();
    if start > 0 {
        let _ = writeln!(out, "… ({start} earlier lines)");
    }
    let rendered: Vec<String> = lines[start..].iter().map(|l| escape_bytes(l)).collect();
    out.push_str(&rendered.join("\n"));
    out
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines().map(|l| format!("{prefix}{l}")).collect::<Vec<_>>().join("\n")
}

fn side_summary(label: &str, o: &Option<SideOutcome>) -> Option<String> {
    o.as_ref().map(|o| {
        if o.timed_out {
            format!("{label} TIMEOUT {}ms", o.duration_ms)
        } else {
            format!("{label} exit {} {}ms", o.exit, o.duration_ms)
        }
    })
}

/// Render one rung for the text report.
pub fn render_rung(rep: &RungReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== {} / {} — {}", rep.fixture, rep.id, rep.title);
    if rep.status == RungStatus::Skipped {
        let _ = writeln!(out, "   skipped: {}", rep.note.as_deref().unwrap_or(""));
        return out;
    }
    for c in &rep.commands {
        let status = match c.status {
            ItemStatus::Ok => "ok".to_string(),
            ItemStatus::Fail => format!("FAIL {}", c.channel.as_deref().unwrap_or("")).trim_end().to_string(),
            ItemStatus::Ran => "ran".to_string(),
            ItemStatus::Skipped => "skipped".to_string(),
        };
        let timing: Vec<String> = [side_summary("php", &c.php), side_summary("rphp", &c.rphp)].into_iter().flatten().collect();
        let _ = write!(out, "   [{}] {}  {status}  ({})", c.index, c.line, timing.join(", "));
        if !c.categories.is_empty() {
            let _ = write!(out, "  [allow: {}]", c.categories.join(", "));
        }
        out.push('\n');
        if let Some(note) = &c.note {
            let _ = writeln!(out, "       note: {note}");
        }
        if c.status == ItemStatus::Fail {
            if let Some(diff) = &c.diff {
                let _ = writeln!(out, "{}", indent(&head_lines(diff, DIFF_LINES), "       "));
            }
            if let Some(tail) = &c.rphp_stderr_tail {
                let _ = writeln!(out, "       rphp stderr (last {STDERR_TAIL} lines):");
                let _ = writeln!(out, "{}", indent(tail, "       | "));
            }
            if let (Some(p), Some(r)) = (&c.php, &c.rphp) {
                let _ = writeln!(out, "       outputs: {p}.{{stdout,stderr,exit}} vs {r}.{{stdout,stderr,exit}}", p = p.out, r = r.out);
            }
        }
    }
    for a in &rep.artifacts {
        let what = match &a.path {
            Some(p) => format!("{p} ({})", a.pattern),
            None => a.pattern.clone(),
        };
        let status = match a.status {
            ItemStatus::Ok => "ok",
            ItemStatus::Fail => "FAIL",
            ItemStatus::Ran => "ran",
            ItemStatus::Skipped => "skipped",
        };
        let _ = writeln!(out, "   artifact {what}  {status}");
        if let Some(d) = &a.detail {
            if a.status != ItemStatus::Ok {
                let _ = writeln!(out, "{}", indent(&head_lines(d, DIFF_LINES), "       "));
            }
        }
    }
    let cmd_ok = rep.commands.iter().filter(|c| matches!(c.status, ItemStatus::Ok | ItemStatus::Ran)).count();
    let art_ok = rep.artifacts.iter().filter(|a| a.status == ItemStatus::Ok).count();
    let art_cmp = rep.artifacts.iter().filter(|a| a.status != ItemStatus::Skipped).count();
    let status = match rep.status {
        RungStatus::Ok => "ok",
        RungStatus::Fail => "FAIL",
        RungStatus::Skipped => "skipped",
        RungStatus::Error => "ERROR",
    };
    let _ = write!(
        out,
        "   -- {}: {status} — {cmd_ok}/{} commands, {art_ok}/{art_cmp} artifacts, {:.1}s",
        rep.id,
        rep.commands.len(),
        rep.duration_ms as f64 / 1000.0
    );
    if rep.status == RungStatus::Error {
        let _ = write!(out, " — {}", rep.note.as_deref().unwrap_or("runner error"));
    } else if let Some(n) = &rep.note {
        let _ = write!(out, " — {n}");
    }
    if rep.kept && !rep.workdirs.is_empty() {
        let dirs: Vec<&str> = rep.workdirs.values().map(String::as_str).collect();
        let _ = write!(out, "\n   kept: {}", dirs.join(", "));
    }
    out.push('\n');
    out
}

/// Render `--list`.
pub fn render_list(fixtures: &[Fixture], fixtures_dir: &Path) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "fixtures under {} ({}):", fixtures_dir.display(), fixtures.len());
    for fx in fixtures {
        let vendor = if !fx.has_composer() {
            "no composer.json"
        } else if fx.vendor_installed() {
            "vendor: installed"
        } else {
            "vendor: MISSING (run tools/fixtures/setup.sh)"
        };
        let php_min = fx.file.fixture.php_min.as_deref().map(|v| format!("  php >= {v}")).unwrap_or_default();
        let _ = writeln!(out, "{}  {}  {vendor}{php_min}", fx.name(), fx.dir.display());
        for rung in &fx.file.rung {
            let what = if rung.http {
                format!("http: {} request(s), docroot {}, {HTTP_SKIP_NOTE}", rung.requests.len(), rung.docroot.as_deref().unwrap_or("?"))
            } else {
                let mut s = format!("{} command(s)", rung.commands.len());
                if !rung.artifacts.is_empty() {
                    let _ = write!(s, ", {} artifact pattern(s)", rung.artifacts.len());
                }
                if rung.allowlist.is_some() || fx.file.fixture.allowlist.is_some() {
                    s.push_str(", allowlist");
                }
                s
            };
            let _ = writeln!(out, "  {:<5} {:<52} {what}", rung.id, rung.title);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

fn setup_error(msg: &str) -> ! {
    eprintln!("xtask ladder: {msg}");
    std::process::exit(2);
}

/// Entry point.
pub fn run(args: &[String]) -> XtaskResult {
    let Some(opts) = Options::parse(args)? else {
        print!("{USAGE}");
        return Ok(());
    };

    let fixtures = match discover(&opts.fixtures_dir) {
        Ok(f) => f,
        Err(e) => setup_error(&e),
    };
    if fixtures.is_empty() {
        setup_error(&format!("no fixtures (dirs with ladder.toml) under {}", opts.fixtures_dir.display()));
    }
    let fixtures: Vec<Fixture> = match &opts.fixture {
        Some(sel) => {
            let f: Vec<Fixture> = fixtures.into_iter().filter(|fx| fx.matches(sel)).collect();
            if f.is_empty() {
                setup_error(&format!("no fixture named `{sel}` under {}", opts.fixtures_dir.display()));
            }
            f
        }
        None => fixtures,
    };

    if opts.list {
        print!("{}", render_list(&fixtures, &opts.fixtures_dir));
        return Ok(());
    }

    // Selection: (fixture index, rung index) pairs in file order.
    let mut units: Vec<(usize, usize)> = Vec::new();
    for (fi, fx) in fixtures.iter().enumerate() {
        for (ri, rung) in fx.file.rung.iter().enumerate() {
            if opts.rungs.is_empty() || opts.rungs.iter().any(|r| r == &rung.id) {
                units.push((fi, ri));
            }
        }
    }
    for r in &opts.rungs {
        if !fixtures.iter().any(|fx| fx.file.rung.iter().any(|x| &x.id == r)) {
            let known: Vec<String> = fixtures.iter().flat_map(|fx| fx.file.rung.iter().map(|x| x.id.clone())).collect();
            setup_error(&format!("no rung `{r}` in the selected fixtures (known: {})", known.join(", ")));
        }
    }
    if units.is_empty() {
        setup_error("nothing selected");
    }

    // Binaries.
    let need_php = opts.only != Some(Side::Rphp);
    let need_rphp = opts.only != Some(Side::Php);
    let php = if need_php {
        let p = opts.php.clone().or_else(find_php);
        match p {
            Some(p) if p.is_file() => Some(p),
            Some(p) => setup_error(&format!("php binary {} does not exist", p.display())),
            None => setup_error("no stock php found (set --php or PHP_BIN, or put `php` on PATH)"),
        }
    } else {
        None
    };
    let rphp = if need_rphp {
        let p = opts
            .rphp
            .clone()
            .or_else(find_rphp)
            .unwrap_or_else(|| opts.target_dir.join("debug").join("rphp"));
        if !p.is_file() {
            setup_error(&format!(
                "rphp binary not found at {}\n  build it first (xtask never builds implicitly):\n\n    cargo build -p rphp\n\n  or pass --rphp <path> / set RPHP_BIN",
                p.display()
            ));
        }
        Some(p)
    } else {
        None
    };

    // Fixture preconditions: vendor/ installed, php new enough.
    let php_version = match &php {
        Some(p) => match query_php_version(p) {
            Ok(v) => Some(v),
            Err(e) => setup_error(&e),
        },
        None => None,
    };
    let selected_fixtures: Vec<usize> = {
        let mut v: Vec<usize> = units.iter().map(|(fi, _)| *fi).collect();
        v.dedup();
        v
    };
    for &fi in &selected_fixtures {
        let fx = &fixtures[fi];
        let needs_vendor = fx.has_composer()
            && units.iter().any(|(f, ri)| *f == fi && !fx.file.rung[*ri].http);
        if needs_vendor && !fx.vendor_installed() {
            setup_error(&format!(
                "fixture `{}` has no vendor/ ({})\n  install every fixture's Composer dependencies with the stock php:\n\n    tools/fixtures/setup.sh\n\n  (runs `{}`; never `composer update` — composer.lock is pinned)",
                fx.name(),
                fx.dir.join("vendor").join("autoload.php").display(),
                fx.file.fixture.setup.as_deref().unwrap_or("composer install")
            ));
        }
        if let (Some(min), Some(have)) = (&fx.file.fixture.php_min, &php_version) {
            if !version_at_least(have, min) {
                setup_error(&format!(
                    "fixture `{}` needs php >= {min}, but {} is {have}",
                    fx.name(),
                    php.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
                ));
            }
        }
    }

    let runner = Runner { opts: opts.clone(), php, rphp };
    let stdout_lock = Mutex::new(());
    let text = opts.report == ReportFormat::Text;
    let run_unit = |&(fi, ri): &(usize, usize)| -> RungReport {
        let fx = &fixtures[fi];
        let rung = &fx.file.rung[ri];
        {
            let _g = stdout_lock.lock().unwrap_or_else(|e| e.into_inner());
            eprintln!("ladder: {} / {} …", fx.name(), rung.id);
        }
        let rep = runner.run_rung(fx, rung);
        if text {
            let _g = stdout_lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut so = io::stdout().lock();
            let _ = so.write_all(render_rung(&rep).as_bytes());
            let _ = so.flush();
        }
        rep
    };
    let rung_reports: Vec<RungReport> = if opts.jobs > 1 {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(opts.jobs).build()?;
        pool.install(|| units.par_iter().map(run_unit).collect())
    } else {
        units.iter().map(run_unit).collect()
    };

    let mut report = Report {
        php: runner.php.as_ref().map(|p| p.display().to_string()),
        rphp: runner.rphp.as_ref().map(|p| p.display().to_string()),
        only: opts.only,
        rungs: rung_reports,
        summary: Summary::default(),
    };
    report.summarize();

    let json_path = opts.target_dir.join("ladder-report.json");
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(parent) = json_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&json_path, format!("{json}\n"))?;

    let s = &report.summary;
    match opts.report {
        ReportFormat::Json => println!("{json}"),
        ReportFormat::Text => {
            let verb = if opts.only.is_some() { "ran" } else { "ok" };
            println!(
                "ladder: {} rung(s) — {} ok, {} FAIL, {} skipped; {}/{} commands {verb}, {}/{} artifacts ok; report: {}",
                s.rungs,
                s.ok,
                s.failed,
                s.skipped,
                s.commands - s.commands_failed,
                s.commands,
                s.artifacts - s.artifacts_failed,
                s.artifacts,
                json_path.display()
            );
            if let Some((r, c)) = report.rungs.iter().find_map(|r| r.first_divergence().map(|c| (r, c))) {
                println!(
                    "first divergence: {} / {} [{}] {} ({})",
                    r.fixture,
                    r.id,
                    c.index,
                    c.line,
                    c.channel.as_deref().unwrap_or("note")
                );
            }
        }
    }

    if s.failed > 0 && !opts.allow_failures {
        return Err(format!("{} rung(s) failed", s.failed).into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> PathBuf {
        repo_root().join("fixtures").join("ladder").join("L1-skeleton")
    }

    #[test]
    fn draft_ladder_toml_parses_and_validates() {
        let fx = Fixture::load(&draft()).expect("draft ladder.toml is valid");
        assert_eq!(fx.name(), "L1-skeleton");
        assert_eq!(fx.file.fixture.php_min.as_deref(), Some("8.4.1"));
        let ids: Vec<&str> = fx.file.rung.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["L1", "L3", "L6a", "L7"]);
        let l1 = fx.file.rung.iter().find(|r| r.id == "L1").unwrap();
        let cmds = l1.resolved_commands().unwrap();
        assert_eq!(cmds[0].script, "vendor/composer/platform_check.php");
        assert!(cmds[0].args.is_empty());
        let l6a = fx.file.rung.iter().find(|r| r.id == "L6a").unwrap();
        assert!(!l6a.artifacts.is_empty());
        assert_eq!(l6a.resolved_commands().unwrap()[0].args, ["about", "--no-ansi"]);
        let l7 = fx.file.rung.iter().find(|r| r.id == "L7").unwrap();
        assert!(l7.http);
        assert_eq!(l7.docroot.as_deref(), Some("public"));
        assert_eq!(l7.requests.len(), 4);
        assert!(fx.file.fixture.allowlist.is_some(), "fixture-level allowlist declared");
    }

    #[test]
    fn command_spec_accepts_string_or_table() {
        let src = r#"
[fixture]
name = "x"
[[rung]]
id = "R"
env = { A = "1" }
stdin = "rung"
allow = ["timing"]
commands = [
  "a.php one 'two words' \"three\\\"q\"",
  { run = "b.php", expect_exit = 3, allow = ["path"], stdin = "cmd", env = { B = "2" } },
]
"#;
        let file: LadderFile = toml::from_str(src).unwrap();
        let cmds = file.rung[0].resolved_commands().unwrap();
        assert_eq!(cmds[0].script, "a.php");
        assert_eq!(cmds[0].args, ["one", "two words", "three\"q"]);
        assert_eq!(cmds[0].expect_exit, None);
        assert_eq!(cmds[0].allow, [Category::Timing]);
        assert_eq!(cmds[0].stdin.as_deref(), Some(&b"rung"[..]));
        assert_eq!(cmds[0].env.get("A").map(String::as_str), Some("1"));
        assert_eq!(cmds[1].expect_exit, Some(3));
        assert_eq!(cmds[1].allow, [Category::Timing, Category::Path]);
        assert_eq!(cmds[1].stdin.as_deref(), Some(&b"cmd"[..]));
        assert_eq!(cmds[1].env.get("B").map(String::as_str), Some("2"));
        assert_eq!(cmds[1].env.get("A").map(String::as_str), Some("1"));
    }

    #[test]
    fn inline_allow_rejects_unknown_and_defaultless_categories() {
        let e = parse_categories(&["nonsense".to_string()]).unwrap_err();
        assert!(e.contains("closed set"), "{e}");
        let e = parse_categories(&["platform-value".to_string()]).unwrap_err();
        assert!(e.contains("no default normalizer"), "{e}");
        assert_eq!(parse_categories(&["ObjectId".to_string()]).unwrap(), [Category::ObjectId]);
    }

    #[test]
    fn split_command_handles_quotes_and_escapes() {
        assert_eq!(split_command("  bin/console   list  --no-ansi ").unwrap(), ["bin/console", "list", "--no-ansi"]);
        assert_eq!(split_command(r#"x.php 'a b' "c d" e\ f"#).unwrap(), ["x.php", "a b", "c d", "e f"]);
        assert_eq!(split_command(r#"x.php """#).unwrap(), ["x.php", ""]);
        assert!(split_command("x.php 'oops").is_err());
        assert!(split_command("").unwrap().is_empty());
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_at_least("8.5.0", "8.4.1"));
        assert!(version_at_least("8.4.1", "8.4.1"));
        assert!(!version_at_least("8.4.0", "8.4.1"));
        assert!(version_at_least("8.10.0-dev", "8.9.9"));
        assert!(!version_at_least("garbage", "8.4"));
    }

    #[test]
    fn fixture_paths_become_placeholder() {
        let needles = vec![b"/x/target/ladder/f/L1/work".to_vec()];
        let out = normalize_fixture_paths(b"in /x/target/ladder/f/L1/work/src/Kernel.php on line 3\n", &needles);
        assert_eq!(out, b"in %FIXTURE%/src/Kernel.php on line 3\n");
    }

    #[test]
    fn inline_allow_builds_an_allowlist_named_by_rung_and_line() {
        let fx = Fixture {
            dir: PathBuf::from("/nonexistent"),
            file: LadderFile {
                fixture: FixtureMeta { name: "x".into(), setup: None, php_min: None, allowlist: None, var_keep: Vec::new() },
                rung: vec![],
            },
        };
        let rung: RungSpec = toml::from_str(
            r#"
id = "R"
commands = [{ run = "bin/console list --no-ansi", allow = ["timing", "object-id"] }]
"#,
        )
        .unwrap();
        let cmds = rung.resolved_commands().unwrap();
        let list = build_allowlist(&fx, &rung, &cmds).unwrap();
        let cats = list.categories_for(&command_name("R", "bin/console list --no-ansi"));
        assert_eq!(cats, [Category::Timing, Category::ObjectId]);
        assert!(list.categories_for("R/other.php").is_empty());
    }

    #[test]
    fn fixture_allowlist_file_parses_under_the_rphp_test_loader() {
        let fx = Fixture::load(&draft()).unwrap();
        let rel = fx.file.fixture.allowlist.clone().expect("fixture allowlist");
        let list = Allowlist::load(&fx.dir.join(rel)).expect("fixture allowlist is valid");
        let cats = list.categories_for(&command_name("L6a", "bin/console about --no-ansi"));
        assert!(cats.contains(&Category::PlatformValue), "{cats:?}");
        // The `about` rules keep Symfony's own version line but hide the timestamp.
        let sample = b"  Version              v8.1.7                           \n  Timezone             UTC (2026-09-17T04:23:57+00:00)  \n  OPcache              Enabled                          \n";
        let norm = list.normalize(&command_name("L6a", "bin/console about --no-ansi"), sample, &NormalizeContext::new());
        let norm = String::from_utf8(norm).unwrap();
        assert!(norm.contains("v8.1.7"), "{norm}");
        assert!(!norm.contains("04:23:57"), "{norm}");
        assert!(!norm.contains("OPcache"), "{norm}");
    }

    #[test]
    fn head_and_tail_lines() {
        assert_eq!(head_lines("a\nb\nc\n", 2), "a\nb\n… (1 more lines)");
        assert_eq!(head_lines("a\nb\n", 5), "a\nb");
        assert_eq!(tail_output(b"a\nb\nc\n", 2), "… (1 earlier lines)\nb\nc");
        assert_eq!(tail_output(b"x\x01y\nz", 5), "x\\x01y\nz");
    }

    #[test]
    fn glob_artifacts_match_relative_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("var/cache/dev")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("var/cache/dev/App_KernelDevDebugContainer.php"), "x").unwrap();
        std::fs::write(root.join("var/cache/dev/other.php"), "x").unwrap();
        std::fs::write(root.join("var/cache/dev/x.meta"), "x").unwrap();
        std::fs::write(root.join("out/1.stdout"), "x").unwrap();
        assert_eq!(
            matching_files(root, "var/cache/dev/*Container.php"),
            ["var/cache/dev/App_KernelDevDebugContainer.php"]
        );
        assert_eq!(matching_files(root, "var/**/*.php").len(), 2);
        assert!(matching_files(root, "**/*.stdout").is_empty(), "out/ is never an artifact");
    }
}
