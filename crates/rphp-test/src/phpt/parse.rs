//! `.phpt` file parsing: a port of `TestFile::readFile()` and
//! `TestFile::validateAndProcess()` from php-src's `run-tests.php` (8.5.0).
//!
//! Sections are kept as raw bytes because php-src tests routinely carry
//! non-UTF-8 payloads (Latin-1 strings, binary blobs) in `--FILE--` and the
//! `--EXPECT*--` sections. Text-only sections are read through
//! [`TestFile::section_str`], which is lossy.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use super::sections::{is_file_section, is_known_section};

/// Why a `.phpt` could not be loaded — "BORK" in run-tests terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The file is empty.
    Empty,
    /// The first line does not start with `--TEST--`.
    NoTestHeader,
    /// A `--NAME--` header run-tests.php does not know.
    UnknownSection(String),
    /// The same non-empty section appears twice.
    DuplicateSection(String),
    /// Exactly one of `FILE` / `FILEEOF` / `FILE_EXTERNAL` is required.
    MissingFile,
    /// Exactly one of `EXPECT` / `EXPECTF` / `EXPECTREGEX` is required.
    MissingExpect,
    /// A `*_EXTERNAL` section names a file that does not exist.
    ExternalNotFound {
        /// The `*_EXTERNAL` section name.
        section: String,
        /// The resolved path that was not found.
        path: PathBuf,
    },
    /// The test file itself could not be read.
    Io(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty test"),
            ParseError::NoTestHeader => write!(f, "tests must start with --TEST--"),
            ParseError::UnknownSection(s) => write!(f, "Unknown section \"{s}\""),
            ParseError::DuplicateSection(s) => write!(f, "duplicated {s} section"),
            ParseError::MissingFile => write!(f, "missing section --FILE--"),
            ParseError::MissingExpect => {
                write!(
                    f,
                    "missing section --EXPECT--, --EXPECTF-- or --EXPECTREGEX--"
                )
            }
            ParseError::ExternalNotFound { section, path } => {
                write!(f, "could not load --{section}-- {}", path.display())
            }
            ParseError::Io(e) => write!(f, "cannot read test file: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Which of the three expectation sections a test uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpectKind {
    /// `--EXPECT--`: byte-exact comparison after normalisation.
    Exact,
    /// `--EXPECTF--`: run-tests wildcards (`%s`, `%d`, `%r…%r`, …).
    Format,
    /// `--EXPECTREGEX--`: the section is a regular expression.
    Regex,
}

impl ExpectKind {
    /// The section name this kind reads.
    pub fn section(self) -> &'static str {
        match self {
            ExpectKind::Exact => "EXPECT",
            ExpectKind::Format => "EXPECTF",
            ExpectKind::Regex => "EXPECTREGEX",
        }
    }
}

/// A parsed, validated `.phpt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFile {
    path: PathBuf,
    sections: BTreeMap<String, Vec<u8>>,
}

impl TestFile {
    /// Read and parse a `.phpt` from disk.
    pub fn load(path: &Path) -> Result<Self, ParseError> {
        let bytes = std::fs::read(path).map_err(|e| ParseError::Io(e.to_string()))?;
        Self::parse(path, &bytes)
    }

    /// Parse `bytes` as the contents of the test at `path`. `path` only
    /// matters for `*_EXTERNAL` resolution (relative to its directory) and
    /// for naming the generated files.
    pub fn parse(path: &Path, bytes: &[u8]) -> Result<Self, ParseError> {
        if bytes.is_empty() {
            return Err(ParseError::Empty);
        }
        let mut lines = bytes.split_inclusive(|&b| b == b'\n');
        let first = lines.next().ok_or(ParseError::Empty)?;
        if !first.starts_with(b"--TEST--") {
            return Err(ParseError::NoTestHeader);
        }

        let mut sections: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        sections.insert("TEST".to_string(), Vec::new());
        let mut section = "TEST".to_string();
        let mut secfile = false;
        let mut secdone = false;

        for line in lines {
            if let Some(name) = section_header(line) {
                if sections.get(&name).is_some_and(|v| php_truthy(v)) {
                    return Err(ParseError::DuplicateSection(name));
                }
                if !is_known_section(&name) {
                    return Err(ParseError::UnknownSection(name));
                }
                sections.insert(name.clone(), Vec::new());
                secfile = is_file_section(&name);
                secdone = false;
                section = name;
                continue;
            }
            if !secdone {
                sections
                    .get_mut(&section)
                    .expect("current section exists")
                    .extend_from_slice(line);
            }
            if secfile && is_done_marker(line) {
                secdone = true;
            }
        }

        let mut test = TestFile {
            path: path.to_path_buf(),
            sections,
        };
        test.validate_and_process()?;
        Ok(test)
    }

    /// `validateAndProcess()`: FILEEOF → FILE, `*_EXTERNAL` inlining and the
    /// exactly-one-FILE / exactly-one-EXPECT rules.
    fn validate_and_process(&mut self) -> Result<(), ParseError> {
        // A redirect test carries no FILE/EXPECT of its own.
        if self.has("REDIRECTTEST") {
            return Ok(());
        }
        let n_file = ["FILE", "FILEEOF", "FILE_EXTERNAL"]
            .iter()
            .filter(|s| self.has(s))
            .count();
        if !self.has("PHPDBG") && n_file != 1 {
            return Err(ParseError::MissingFile);
        }
        if let Some(mut eof) = self.sections.remove("FILEEOF") {
            while eof.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
                eof.pop();
            }
            self.sections.insert("FILE".to_string(), eof);
        }
        for prefix in ["FILE", "EXPECT", "EXPECTF", "EXPECTREGEX"] {
            let key = format!("{prefix}_EXTERNAL");
            if let Some(raw) = self.section_str(&key) {
                // Tests may only reach files inside their own directory tree:
                // `..` is stripped and the name is string-concatenated onto
                // the directory (a leading `/` cannot escape it).
                let name = raw.replace("..", "").trim().to_string();
                let path = PathBuf::from(format!("{}/{name}", self.dir().display()));
                match std::fs::read(&path) {
                    Ok(content) => {
                        self.sections.insert(prefix.to_string(), content);
                    }
                    Err(_) => {
                        return Err(ParseError::ExternalNotFound { section: key, path });
                    }
                }
            }
        }
        let n_expect = ["EXPECT", "EXPECTF", "EXPECTREGEX"]
            .iter()
            .filter(|s| self.has(s))
            .count();
        if n_expect != 1 {
            return Err(ParseError::MissingExpect);
        }
        if self.has("PHPDBG") && !self.has("STDIN") {
            let mut stdin = self.sections.get("PHPDBG").cloned().unwrap_or_default();
            stdin.push(b'\n');
            self.sections.insert("STDIN".to_string(), stdin);
        }
        Ok(())
    }

    /// The `.phpt` path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The directory containing the test (where generated files go).
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }

    /// The trimmed `--TEST--` text.
    pub fn name(&self) -> String {
        self.section_str("TEST")
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// Is the section present (even if empty)?
    pub fn has(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    /// Raw section bytes.
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        self.sections.get(name).map(Vec::as_slice)
    }

    /// Section text, lossily decoded.
    pub fn section_str(&self, name: &str) -> Option<Cow<'_, str>> {
        self.sections.get(name).map(|v| String::from_utf8_lossy(v))
    }

    /// PHP's `!empty($sections[$name])`: present, non-empty and not `"0"`.
    pub fn section_not_empty(&self, name: &str) -> bool {
        self.sections.get(name).is_some_and(|v| php_truthy(v))
    }

    /// Replace or add a section (run-tests does this when SKIPIF prints
    /// `xfail`/`xleak`/`flaky`).
    pub fn set_section(&mut self, name: &str, value: &[u8]) {
        self.sections.insert(name.to_string(), value.to_vec());
    }

    /// Section names present, in alphabetical order.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }

    /// `TestFile::isCGI()`: does the test need the CGI SAPI?
    pub fn is_cgi(&self) -> bool {
        self.has("CGI")
            || self.section_not_empty("GET")
            || self.section_not_empty("POST")
            || self.section_not_empty("GZIP_POST")
            || self.section_not_empty("DEFLATE_POST")
            || self.section_not_empty("POST_RAW")
            || self.section_not_empty("PUT")
            || self.section_not_empty("COOKIE")
            || self.section_not_empty("EXPECTHEADERS")
    }

    /// Which expectation section the test uses. Validation guarantees exactly
    /// one for non-redirect tests; a redirect test reports `Exact`.
    pub fn expect_kind(&self) -> ExpectKind {
        if self.has("EXPECTF") {
            ExpectKind::Format
        } else if self.has("EXPECTREGEX") {
            ExpectKind::Regex
        } else {
            ExpectKind::Exact
        }
    }

    /// The raw expectation bytes (before trimming / CRLF normalisation).
    pub fn expected(&self) -> &[u8] {
        self.section(self.expect_kind().section())
            .unwrap_or_default()
    }

    /// The script (`--FILE--`, after FILEEOF / FILE_EXTERNAL resolution).
    pub fn file(&self) -> &[u8] {
        self.section("FILE").unwrap_or_default()
    }
}

/// `preg_match('/^--([_A-Z]+)--/', $line)`: the section a header line opens.
/// Trailing text after the closing `--` is ignored, exactly like run-tests.
fn section_header(line: &[u8]) -> Option<String> {
    let rest = line.strip_prefix(b"--")?;
    let end = rest
        .iter()
        .position(|&b| !(b.is_ascii_uppercase() || b == b'_'))?;
    if end == 0 || !rest[end..].starts_with(b"--") {
        return None;
    }
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// `preg_match('/^===DONE===\s*$/', $line)`.
fn is_done_marker(line: &[u8]) -> bool {
    let trimmed = super::expectf::php_trim_end(line);
    trimmed == b"===DONE==="
}

/// PHP truthiness of a string: anything but `""` and `"0"`.
fn php_truthy(v: &[u8]) -> bool {
    !(v.is_empty() || v == b"0")
}
