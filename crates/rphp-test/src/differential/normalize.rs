//! The **closed set** of divergence categories (ADR-008) and the byte-level
//! normalizer each one applies before two outputs are compared.
//!
//! Every category names one legitimately environment-dependent kind of output.
//! Each has a *default* normalizer (a regex rewrite that collapses the volatile
//! span into a placeholder such as `%f` or `#N`) or, where no generic rewrite
//! exists (`Locale`, `PlatformValue`, `Pid`), requires the allowlist entry to
//! spell out its own `normalize = "..."` rule. Both sides of a comparison go
//! through the same normalizer, so a category never hides a divergence in the
//! non-volatile part of the output.
//!
//! The set can only shrink deliberately: an allowlist naming a category that is
//! not listed here is rejected at load time (see [`super::allowlist`]).

use std::path::{Path as StdPath, PathBuf};
use std::sync::OnceLock;

use regex::bytes::Regex;

/// The closed set of divergence categories. Names in the allowlist file may be
/// written either as the variant (`ObjectId`) or in kebab-case (`object-id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Float rendering (`precision`/`serialize_precision`, shortest-round-trip
    /// dtoa). Default: every decimal/exponent literal becomes `%f`.
    FloatFormat,
    /// Locale-sensitive output (`setlocale`, `number_format` separators,
    /// collation). No default; the entry must give an explicit rule.
    Locale,
    /// Error/exception *prose*. The level and the thrown class are kept, the
    /// message text, file/line and stack trace are collapsed. Default: see
    /// [`Category::normalize`].
    ErrorWording,
    /// Hash-order edge cases (unseeded `array_rand`, iteration after
    /// collisions). Default: output lines are sorted.
    HashOrder,
    /// Platform-dependent values (`PHP_OS`, `PHP_INT_SIZE`, `php_uname`). No
    /// default; the entry must give an explicit rule.
    PlatformValue,
    /// Resource handle numbers. Default: `resource(12)` → `resource(N)`,
    /// `Resource id #12` → `Resource id #N`.
    ResourceId,
    /// Object handle numbers in `var_dump`/`debug_zval_dump` output. Default:
    /// `object(Foo)#12` → `object(Foo)#N`. (`spl_object_id` prints a bare
    /// integer and needs an explicit `regex:` rule.)
    ObjectId,
    /// IANA time-zone database version (`timezone_version_get`). Default:
    /// `2025.2` / `2025b` → `%TZDB%`.
    TzdbVersion,
    /// Process ids (`getmypid`). No default: a PID is a bare integer, so the
    /// entry must give an explicit `regex:` rule that targets the right span.
    Pid,
    /// Wall-clock/CPU timings. Default: `12.5 ms`, `0.003s`, `7us` → `%TIMING%`.
    Timing,
    /// Paths returned by `tempnam`/`sys_get_temp_dir`. Default: anything under
    /// the system temp dir, `/tmp` or `/var/folders` → `%TEMPNAM%`.
    TempnamPath,
    /// The script's own absolute path, its directory and the run's working
    /// directory. Default: each of those becomes `%PATH%`.
    Path,
}

impl Category {
    /// Every category, in declaration order.
    pub const ALL: [Category; 12] = [
        Category::FloatFormat,
        Category::Locale,
        Category::ErrorWording,
        Category::HashOrder,
        Category::PlatformValue,
        Category::ResourceId,
        Category::ObjectId,
        Category::TzdbVersion,
        Category::Pid,
        Category::Timing,
        Category::TempnamPath,
        Category::Path,
    ];

    /// The canonical kebab-case name used in `divergences.toml` and reports.
    pub fn name(self) -> &'static str {
        match self {
            Category::FloatFormat => "float-format",
            Category::Locale => "locale",
            Category::ErrorWording => "error-wording",
            Category::HashOrder => "hash-order",
            Category::PlatformValue => "platform-value",
            Category::ResourceId => "resource-id",
            Category::ObjectId => "object-id",
            Category::TzdbVersion => "tzdb-version",
            Category::Pid => "pid",
            Category::Timing => "timing",
            Category::TempnamPath => "tempnam-path",
            Category::Path => "path",
        }
    }

    /// The Rust variant name (`ObjectId`), also accepted in the allowlist.
    pub fn variant_name(self) -> &'static str {
        match self {
            Category::FloatFormat => "FloatFormat",
            Category::Locale => "Locale",
            Category::ErrorWording => "ErrorWording",
            Category::HashOrder => "HashOrder",
            Category::PlatformValue => "PlatformValue",
            Category::ResourceId => "ResourceId",
            Category::ObjectId => "ObjectId",
            Category::TzdbVersion => "TzdbVersion",
            Category::Pid => "Pid",
            Category::Timing => "Timing",
            Category::TempnamPath => "TempnamPath",
            Category::Path => "Path",
        }
    }

    /// Parse a category name; accepts the kebab-case name or the variant name
    /// exactly (no case folding — the set is closed and spelled one way).
    pub fn parse(s: &str) -> Option<Category> {
        Category::ALL
            .iter()
            .copied()
            .find(|c| c.name() == s || c.variant_name() == s)
    }

    /// Whether the category has a built-in normalizer. Categories without one
    /// require an explicit `normalize = "..."` rule on every allowlist entry.
    pub fn has_default(self) -> bool {
        !matches!(self, Category::Locale | Category::PlatformValue | Category::Pid)
    }

    /// Apply the category's default normalizer to `bytes`. Categories without a
    /// default return the input unchanged.
    pub fn normalize(self, bytes: &[u8], ctx: &NormalizeContext) -> Vec<u8> {
        match self {
            Category::FloatFormat => replace(&FLOAT, bytes, b"%f"),
            Category::Locale | Category::PlatformValue | Category::Pid => bytes.to_vec(),
            Category::ErrorWording => normalize_error_wording(bytes),
            Category::HashOrder => sort_lines(bytes),
            Category::ResourceId => {
                let step = replace(&RESOURCE, bytes, b"resource(N)");
                replace(&RESOURCE_ID, &step, b"Resource id #N")
            }
            Category::ObjectId => replace(&OBJECT_ID, bytes, b")#N"),
            Category::TzdbVersion => replace(&TZDB, bytes, b"%TZDB%"),
            Category::Timing => replace(&TIMING, bytes, b"%TIMING%"),
            Category::TempnamPath => tempnam_regex().replace_all(bytes, &b"%TEMPNAM%"[..]).into_owned(),
            Category::Path => normalize_paths(bytes, ctx),
        }
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Per-run facts a normalizer may need: the absolute paths that the `Path`
/// category rewrites to `%PATH%` (the script, its directory, the working
/// directory). Longer paths are replaced first so a directory prefix never
/// clobbers a full path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizeContext {
    /// Absolute paths to collapse into `%PATH%`.
    pub paths: Vec<PathBuf>,
}

impl NormalizeContext {
    /// An empty context: path normalization is a no-op.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one path to collapse.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(path.into());
        self
    }

    /// The context for one run: the script's path, its parent directory and
    /// the working directory the process ran in.
    pub fn for_script(script: &StdPath, cwd: &StdPath) -> Self {
        let mut ctx = Self::new().with_path(script);
        if let Some(dir) = script.parent() {
            ctx.paths.push(dir.to_path_buf());
        }
        ctx.paths.push(cwd.to_path_buf());
        ctx
    }
}

/// A lazily compiled regex (patterns are constants, so compilation cannot fail).
struct Lazy(&'static str, OnceLock<Regex>);

impl Lazy {
    const fn new(pattern: &'static str) -> Self {
        Lazy(pattern, OnceLock::new())
    }

    fn get(&self) -> &Regex {
        self.1.get_or_init(|| Regex::new(self.0).expect("built-in normalizer regex is valid"))
    }
}

fn replace(re: &Lazy, hay: &[u8], with: &[u8]) -> Vec<u8> {
    re.get().replace_all(hay, with).into_owned()
}

static FLOAT: Lazy = Lazy::new(r"-?(?:\d+\.\d+(?:[eE][+-]?\d+)?|\d+[eE][+-]?\d+)");
static RESOURCE: Lazy = Lazy::new(r"resource\(\d+\)");
static RESOURCE_ID: Lazy = Lazy::new(r"Resource id #\d+");
static OBJECT_ID: Lazy = Lazy::new(r"\)#\d+");
static TZDB: Lazy = Lazy::new(r"\b\d{4}(?:\.\d+|[a-z])\b");
static TIMING: Lazy = Lazy::new(r"\b\d+(?:\.\d+)? ?(?:ms|us|µs|ns|s|sec|secs|seconds)\b");
/// `PHP Fatal error:  msg` (log form, two spaces) → `Fatal error: msg`.
static LOG_PREFIX: Lazy =
    Lazy::new(r"(?m)^PHP (Fatal error|Parse error|Warning|Notice|Deprecated|Strict Standards):  ?");
/// `Fatal error: Uncaught Foo\Bar: message in file:12` → `Fatal error: Uncaught Foo\Bar: %MSG%`.
static UNCAUGHT: Lazy = Lazy::new(
    r"(?m)^(Fatal error|Warning|Notice|Deprecated): Uncaught ([A-Za-z_\\][A-Za-z0-9_\\]*)(?::| |$)[^\n]*$",
);
/// `Warning: message in file on line 12` → `Warning: %MSG%`.
static DIAGNOSTIC: Lazy =
    Lazy::new(r"(?m)^(Fatal error|Parse error|Warning|Notice|Deprecated): [^\n]*? in [^\n]* on line \d+$");
/// Stack-trace lines PHP appends to an uncaught exception.
static TRACE: Lazy = Lazy::new(r"(?m)^(?:Stack trace:|#\d+ [^\n]*|  thrown in [^\n]* on line \d+)\n?");

fn normalize_error_wording(bytes: &[u8]) -> Vec<u8> {
    let step = replace(&LOG_PREFIX, bytes, b"$1: ");
    let step = replace(&UNCAUGHT, &step, b"$1: Uncaught $2: %MSG%");
    let step = replace(&DIAGNOSTIC, &step, b"$1: %MSG%");
    replace(&TRACE, &step, b"")
}

/// Sort the lines of `bytes` (byte order), keeping the trailing newline state.
fn sort_lines(bytes: &[u8]) -> Vec<u8> {
    let trailing_newline = bytes.last() == Some(&b'\n');
    let body = if trailing_newline { &bytes[..bytes.len() - 1] } else { bytes };
    if body.is_empty() {
        return bytes.to_vec();
    }
    let mut lines: Vec<&[u8]> = body.split(|&c| c == b'\n').collect();
    lines.sort_unstable();
    let mut out = lines.join(&b'\n');
    if trailing_newline {
        out.push(b'\n');
    }
    out
}

fn tempnam_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let mut alternatives = vec![
            r"/private/var/folders/[^\s'\x22]+".to_string(),
            r"/var/folders/[^\s'\x22]+".to_string(),
            r"/private/tmp/[^\s'\x22]+".to_string(),
            r"/tmp/[^\s'\x22]+".to_string(),
        ];
        let sys = std::env::temp_dir();
        let sys = sys.to_string_lossy();
        let sys = sys.trim_end_matches('/');
        if !sys.is_empty() && sys != "/tmp" {
            alternatives.insert(0, format!(r"{}/[^\s'\x22]+", regex::escape(sys)));
        }
        Regex::new(&format!("(?:{})", alternatives.join("|"))).expect("tempnam regex is valid")
    })
}

fn normalize_paths(bytes: &[u8], ctx: &NormalizeContext) -> Vec<u8> {
    let mut needles: Vec<Vec<u8>> = ctx
        .paths
        .iter()
        .map(|p| path_bytes(p))
        .filter(|p| !p.is_empty() && p != b"/")
        .collect();
    needles.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    needles.dedup();
    let mut out = bytes.to_vec();
    for needle in &needles {
        out = replace_bytes(&out, needle, b"%PATH%");
    }
    out
}

#[cfg(unix)]
fn path_bytes(p: &StdPath) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(p: &StdPath) -> Vec<u8> {
    p.to_string_lossy().into_owned().into_bytes()
}

/// Replace every non-overlapping occurrence of `needle` in `hay` with `with`.
pub fn replace_bytes(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}
