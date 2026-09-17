//! The curated divergence allowlist (`examples/tier-a/divergences.toml`).
//!
//! ```toml
//! [[allow]]
//! snippet   = "math/basics.php"          # relative path or glob (`*`, `**`, `?`)
//! category  = "float-format"             # one of the closed set, see `Category`
//! reason    = "why this legitimately diverges"
//! normalize = ""                         # "" = the category default,
//!                                        # "drop-lines:<regex>" or
//!                                        # "regex:<pattern> => <replacement>"
//! ```
//!
//! Loading is strict: an unknown key, an unknown category, an empty reason, an
//! invalid regex, or a category without a default normalizer combined with an
//! empty `normalize` are all errors. The allowlist can only shrink deliberately.

use std::fmt;
use std::path::{Path, PathBuf};

use regex::bytes::Regex;
use serde::Deserialize;

use super::normalize::{Category, NormalizeContext};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    allow: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    snippet: String,
    category: String,
    reason: String,
    #[serde(default)]
    normalize: String,
}

/// How an entry rewrites output before comparison.
#[derive(Debug, Clone)]
pub enum Rule {
    /// Apply the category's built-in normalizer.
    CategoryDefault,
    /// Drop every line (without its newline) that matches the regex.
    DropLines(Regex),
    /// `regex::bytes::Regex::replace_all` with the given replacement (`$1`
    /// style capture references are expanded).
    Replace {
        /// The pattern to search for.
        pattern: Regex,
        /// The replacement text.
        replacement: String,
    },
}

impl Rule {
    /// Parse the `normalize` field of an entry.
    pub fn parse(spec: &str) -> Result<Rule, String> {
        let spec = spec.trim_start();
        if spec.is_empty() {
            return Ok(Rule::CategoryDefault);
        }
        if let Some(re) = spec.strip_prefix("drop-lines:") {
            let re = Regex::new(re).map_err(|e| format!("invalid drop-lines regex: {e}"))?;
            return Ok(Rule::DropLines(re));
        }
        if let Some(rest) = spec.strip_prefix("regex:") {
            let Some((pattern, replacement)) = rest.split_once(" => ") else {
                return Err("regex rule must be `regex:<pattern> => <replacement>`".to_string());
            };
            let pattern = Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;
            return Ok(Rule::Replace { pattern, replacement: replacement.to_string() });
        }
        Err(format!(
            "unknown normalize rule `{spec}` (expected \"\", \"drop-lines:<regex>\" or \"regex:<pattern> => <replacement>\")"
        ))
    }

    /// Apply the rule for `category` to `bytes`.
    pub fn apply(&self, category: Category, bytes: &[u8], ctx: &NormalizeContext) -> Vec<u8> {
        match self {
            Rule::CategoryDefault => category.normalize(bytes, ctx),
            Rule::DropLines(re) => {
                let mut out = Vec::with_capacity(bytes.len());
                for line in bytes.split_inclusive(|&c| c == b'\n') {
                    let bare = line.strip_suffix(b"\n").unwrap_or(line);
                    if !re.is_match(bare) {
                        out.extend_from_slice(line);
                    }
                }
                out
            }
            Rule::Replace { pattern, replacement } => {
                pattern.replace_all(bytes, replacement.as_bytes()).into_owned()
            }
        }
    }
}

/// One validated allowlist entry.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Relative snippet path or glob, forward slashes.
    pub snippet: String,
    /// The divergence category.
    pub category: Category,
    /// Why the output legitimately diverges.
    pub reason: String,
    /// The normalizer to apply to both sides.
    pub rule: Rule,
}

impl Entry {
    /// Whether this entry applies to the snippet at `rel` (a path relative to
    /// the snippet root, forward slashes).
    pub fn matches(&self, rel: &str) -> bool {
        glob_match(&self.snippet, rel)
    }

    /// Apply this entry's normalizer.
    pub fn apply(&self, bytes: &[u8], ctx: &NormalizeContext) -> Vec<u8> {
        self.rule.apply(self.category, bytes, ctx)
    }
}

/// Errors from loading or validating an allowlist file.
#[derive(Debug)]
pub enum AllowlistError {
    /// The file could not be read.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The TOML did not parse or had unknown keys.
    Toml(String),
    /// `category` is not in the closed set.
    UnknownCategory {
        /// Zero-based entry index.
        index: usize,
        /// The entry's snippet field.
        snippet: String,
        /// The offending name.
        category: String,
    },
    /// `reason` is empty.
    EmptyReason {
        /// Zero-based entry index.
        index: usize,
        /// The entry's snippet field.
        snippet: String,
    },
    /// `snippet` is empty.
    EmptySnippet {
        /// Zero-based entry index.
        index: usize,
    },
    /// `normalize` could not be parsed.
    BadRule {
        /// Zero-based entry index.
        index: usize,
        /// The entry's snippet field.
        snippet: String,
        /// What went wrong.
        message: String,
    },
    /// The category has no default normalizer and `normalize` is empty.
    NoDefaultNormalizer {
        /// Zero-based entry index.
        index: usize,
        /// The entry's snippet field.
        snippet: String,
        /// The category.
        category: Category,
    },
}

impl fmt::Display for AllowlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllowlistError::Io { path, source } => {
                write!(f, "cannot read allowlist {}: {source}", path.display())
            }
            AllowlistError::Toml(msg) => write!(f, "allowlist is not valid TOML: {msg}"),
            AllowlistError::UnknownCategory { index, snippet, category } => {
                let valid: Vec<&str> = Category::ALL.iter().map(|c| c.name()).collect();
                write!(
                    f,
                    "allow[{index}] ({snippet}): unknown category `{category}`; the closed set is: {}",
                    valid.join(", ")
                )
            }
            AllowlistError::EmptyReason { index, snippet } => {
                write!(f, "allow[{index}] ({snippet}): `reason` must say why the output diverges")
            }
            AllowlistError::EmptySnippet { index } => {
                write!(f, "allow[{index}]: `snippet` must name a snippet or glob")
            }
            AllowlistError::BadRule { index, snippet, message } => {
                write!(f, "allow[{index}] ({snippet}): {message}")
            }
            AllowlistError::NoDefaultNormalizer { index, snippet, category } => write!(
                f,
                "allow[{index}] ({snippet}): category `{category}` has no default normalizer; give an explicit `normalize` rule"
            ),
        }
    }
}

impl std::error::Error for AllowlistError {}

/// The loaded, validated allowlist.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    entries: Vec<Entry>,
    root: Option<PathBuf>,
}

impl Allowlist {
    /// An allowlist with no entries: every comparison is exact.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse and validate TOML text.
    pub fn parse(src: &str) -> Result<Self, AllowlistError> {
        let raw: RawFile = toml::from_str(src).map_err(|e| AllowlistError::Toml(e.to_string()))?;
        let mut entries = Vec::with_capacity(raw.allow.len());
        for (index, e) in raw.allow.into_iter().enumerate() {
            if e.snippet.trim().is_empty() {
                return Err(AllowlistError::EmptySnippet { index });
            }
            let snippet = e.snippet.trim().to_string();
            let Some(category) = Category::parse(e.category.trim()) else {
                return Err(AllowlistError::UnknownCategory { index, snippet, category: e.category });
            };
            if e.reason.trim().is_empty() {
                return Err(AllowlistError::EmptyReason { index, snippet });
            }
            let rule = Rule::parse(&e.normalize).map_err(|message| AllowlistError::BadRule {
                index,
                snippet: snippet.clone(),
                message,
            })?;
            if matches!(rule, Rule::CategoryDefault) && !category.has_default() {
                return Err(AllowlistError::NoDefaultNormalizer { index, snippet, category });
            }
            entries.push(Entry { snippet, category, reason: e.reason.trim().to_string(), rule });
        }
        Ok(Allowlist { entries, root: None })
    }

    /// Load and validate `path`; the file's directory becomes the snippet root
    /// that entry globs are relative to.
    pub fn load(path: &Path) -> Result<Self, AllowlistError> {
        let src = std::fs::read_to_string(path)
            .map_err(|source| AllowlistError::Io { path: path.to_path_buf(), source })?;
        let mut list = Self::parse(&src)?;
        list.root = path.parent().map(Path::to_path_buf);
        Ok(list)
    }

    /// Set the snippet root that entry globs are relative to.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// The snippet root, if known.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// All entries in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entries that apply to the snippet at `rel`, in file order.
    pub fn entries_for(&self, rel: &str) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.matches(rel)).collect()
    }

    /// The categories that apply to `rel` (deduplicated, file order).
    pub fn categories_for(&self, rel: &str) -> Vec<Category> {
        let mut out: Vec<Category> = Vec::new();
        for e in self.entries_for(rel) {
            if !out.contains(&e.category) {
                out.push(e.category);
            }
        }
        out
    }

    /// Run `bytes` through every entry that applies to `rel`, in file order.
    pub fn normalize(&self, rel: &str, bytes: &[u8], ctx: &NormalizeContext) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for e in self.entries_for(rel) {
            out = e.apply(&out, ctx);
        }
        out
    }

    /// The name a snippet path has inside this allowlist: relative to the root
    /// when it is under it, otherwise its file name; always forward slashes.
    pub fn relative_name(&self, path: &Path) -> String {
        relative_name(self.root.as_deref(), path)
    }

    /// Entries that match none of the given snippet names — stale entries that
    /// should be deleted rather than kept around.
    pub fn unmatched<'a>(&'a self, names: &[String]) -> Vec<&'a Entry> {
        self.entries.iter().filter(|e| !names.iter().any(|n| e.matches(n))).collect()
    }
}

/// The name a snippet has relative to `root` (or its file name when it is not
/// under `root`), with forward slashes.
pub fn relative_name(root: Option<&Path>, path: &Path) -> String {
    let rel = root
        .and_then(|r| path.strip_prefix(r).ok())
        .map(Path::to_path_buf)
        .or_else(|| path.file_name().map(PathBuf::from))
        .unwrap_or_else(|| path.to_path_buf());
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Minimal glob matching over `/`-separated names: `*` matches within one
/// path segment, `**` matches across segments (`**/x` also matches `x`), `?`
/// matches one non-separator character; everything else is literal.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            Some(b'*') => (0..=s.len())
                .take_while(|&i| i == 0 || s[i - 1] != b'/')
                .any(|i| go(&p[1..], &s[i..])),
            Some(b'?') => !s.is_empty() && s[0] != b'/' && go(&p[1..], &s[1..]),
            Some(&c) => !s.is_empty() && s[0] == c && go(&p[1..], &s[1..]),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}
