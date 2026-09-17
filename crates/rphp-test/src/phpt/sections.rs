//! Section-level semantics shared by the parser and the runner.
//!
//! Everything here is a port of a small piece of php-src's `run-tests.php`
//! (8.5.0): the allowed section list (`TestFile::ALLOWED_SECTIONS`), INI
//! handling (`settings2array` / `settings2params`), the `--ENV--` format,
//! `--CAPTURE_STDIO--`, `--ARGS--` splitting, `--EXTENSIONS--` /
//! `--CONFLICTS--` lists, and the CGI request shaping that turns
//! `--GET--`/`--POST--`/`--POST_RAW--`/`--PUT--`/`--COOKIE--` into the
//! environment variables and stdin body a CGI binary expects.

use std::collections::BTreeMap;

use super::parse::TestFile;

/// Environment overrides for a spawned engine process. An empty value means
/// "unset": `proc_open()` drops empty variables, and run-tests.php relies on
/// that to clear `TZ`, `REQUEST_METHOD` and friends between tests.
pub type EnvMap = BTreeMap<String, String>;

/// Every section name run-tests.php 8.5 accepts after the leading `--TEST--`.
/// Anything else makes the test "BORKED" (`Unknown section`).
pub const ALLOWED_SECTIONS: &[&str] = &[
    "EXPECT",
    "EXPECTF",
    "EXPECTREGEX",
    "EXPECTREGEX_EXTERNAL",
    "EXPECT_EXTERNAL",
    "EXPECTF_EXTERNAL",
    "EXPECTHEADERS",
    "POST",
    "POST_RAW",
    "GZIP_POST",
    "DEFLATE_POST",
    "PUT",
    "GET",
    "COOKIE",
    "ARGS",
    "FILE",
    "FILEEOF",
    "FILE_EXTERNAL",
    "REDIRECTTEST",
    "CAPTURE_STDIO",
    "STDIN",
    "CGI",
    "PHPDBG",
    "INI",
    "ENV",
    "EXTENSIONS",
    "SKIPIF",
    "XFAIL",
    "XLEAK",
    "CLEAN",
    "CREDITS",
    "DESCRIPTION",
    "CONFLICTS",
    "WHITESPACE_SENSITIVE",
    "FLAKY",
];

/// Is `name` one of the sections run-tests.php accepts (excluding the implicit
/// leading `TEST`)?
pub fn is_known_section(name: &str) -> bool {
    ALLOWED_SECTIONS.contains(&name)
}

/// Sections that carry the test script; `===DONE===` terminates them.
pub fn is_file_section(name: &str) -> bool {
    matches!(name, "FILE" | "FILEEOF" | "FILE_EXTERNAL")
}

/// One `name=value` INI entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniEntry {
    /// Directive name (trimmed).
    pub name: String,
    /// Directive value (trimmed, no quoting applied).
    pub value: String,
}

/// An ordered set of INI directives with `settings2array` semantics: setting a
/// name that already exists replaces it in place, except `extension` and
/// `zend_extension`, which accumulate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IniSettings {
    entries: Vec<IniEntry>,
}

impl IniSettings {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a set from `name=value` lines.
    pub fn from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut s = Self::new();
        s.add_lines(lines);
        s
    }

    /// Apply `name=value` lines (`settings2array`): lines without `=` are
    /// ignored, both halves are trimmed.
    pub fn add_lines<'a>(&mut self, lines: impl IntoIterator<Item = &'a str>) {
        for line in lines {
            if let Some((name, value)) = line.split_once('=') {
                self.set(name.trim(), value.trim());
            }
        }
    }

    /// Apply a whole `--INI--` block: run-tests splits it on `[\n\r]+`.
    pub fn add_text(&mut self, text: &str) {
        self.add_lines(text.split(['\n', '\r']).filter(|l| !l.is_empty()));
    }

    /// Set one directive (replace in place, or append for the accumulating
    /// `extension` / `zend_extension` names).
    pub fn set(&mut self, name: &str, value: &str) {
        let accumulates = name == "extension" || name == "zend_extension";
        if !accumulates {
            if let Some(e) = self.entries.iter_mut().find(|e| e.name == name) {
                e.value = value.to_string();
                return;
            }
        }
        self.entries.push(IniEntry {
            name: name.to_string(),
            value: value.to_string(),
        });
    }

    /// The (last) value of a directive.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.name == name)
            .map(|e| e.value.as_str())
    }

    /// All entries in order.
    pub fn entries(&self) -> &[IniEntry] {
        &self.entries
    }

    /// No directives at all?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `settings2params`: the `-d name=value` argument pairs. No shell
    /// quoting is applied because the process is spawned directly.
    pub fn to_args(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.entries.len() * 2);
        for e in &self.entries {
            out.push("-d".to_string());
            out.push(format!("{}={}", e.name, e.value));
        }
        out
    }
}

/// Expand the placeholders run-tests.php supports inside `--INI--`:
/// `{PWD}` (the test's directory), `{TMP}` (the system temp dir),
/// `{MAIL:file}` (a `sendmail_path` replacement that appends to `file`) and
/// `{ENV:NAME}`. A missing environment variable is a skip, returned as `Err`
/// with the exact run-tests reason.
pub fn expand_ini_placeholders(text: &str, test_dir: &std::path::Path) -> Result<String, String> {
    let mut out = text.replace("{PWD}", &test_dir.display().to_string());
    out = out.replace("{TMP}", &std::env::temp_dir().display().to_string());
    let mail = regex::Regex::new(r"\{MAIL:(\S+)\}").expect("static regex");
    out = mail.replace_all(&out, "tee $1 >/dev/null").into_owned();
    let env = regex::Regex::new(r"\{ENV:(\S+)\}").expect("static regex");
    let mut skip: Option<String> = None;
    let expanded = env
        .replace_all(&out, |caps: &regex::Captures<'_>| {
            let name = &caps[1];
            match std::env::var(name) {
                Ok(v) => v,
                Err(_) => {
                    skip.get_or_insert_with(|| format!("Environment variable {name} is not set"));
                    String::new()
                }
            }
        })
        .into_owned();
    match skip {
        Some(reason) => Err(reason),
        None => Ok(expanded),
    }
}

/// Parse an `--ENV--` block: `{PWD}` is replaced by the test directory, each
/// trimmed line is split at the first `=`, lines without `=` or with an empty
/// name are ignored (PHP's `!empty($e[0])` also drops a name of `0`).
pub fn parse_env_section(text: &str, test_dir: &std::path::Path) -> Vec<(String, String)> {
    let text = text.replace("{PWD}", &test_dir.display().to_string());
    text.split('\n')
        .filter_map(|line| {
            let (k, v) = line.trim().split_once('=')?;
            if k.is_empty() || k == "0" {
                return None;
            }
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Which standard streams the runner captures (`--CAPTURE_STDIO--`). When
/// stdout and stderr are both captured they share one pipe, exactly like the
/// `2>&1` run-tests appends to the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureStdio {
    /// Feed stdin from the runner (STDIN / POST body, else nothing).
    pub stdin: bool,
    /// Capture stdout into the compared output.
    pub stdout: bool,
    /// Capture stderr into the compared output.
    pub stderr: bool,
}

impl CaptureStdio {
    /// Everything captured — the default without a `--CAPTURE_STDIO--` section.
    pub const ALL: Self = Self {
        stdin: true,
        stdout: true,
        stderr: true,
    };

    /// Parse the section text (case-insensitive substring test per stream);
    /// `None` means the section is absent.
    pub fn parse(section: Option<&str>) -> Self {
        match section {
            None => Self::ALL,
            Some(s) => {
                let up = s.to_ascii_uppercase();
                Self {
                    stdin: up.contains("STDIN"),
                    stdout: up.contains("STDOUT"),
                    stderr: up.contains("STDERR"),
                }
            }
        }
    }
}

/// Split an `--ARGS--` section the way the shell would when run-tests appends
/// it to the command line: whitespace-separated words, single quotes literal,
/// double quotes with `\"` and `\\` escapes, backslash escapes outside quotes.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    args.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    cur.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(n @ ('"' | '\\' | '$' | '`')) => cur.push(n),
                            Some(n) => {
                                cur.push('\\');
                                cur.push(n);
                            }
                            None => cur.push('\\'),
                        },
                        c => cur.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        args.push(cur);
    }
    args
}

/// Parse an `--EXTENSIONS--` block: one extension per line.
pub fn parse_extensions(text: &str) -> Vec<String> {
    text.trim()
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a `--CONFLICTS--` block or a per-directory `CONFLICTS` file:
/// `#` comments are stripped, one key per line, `all` means "run alone".
pub fn parse_conflicts(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse `Name: value` header lines (for `--EXPECTHEADERS--` and the header
/// block a CGI binary prints); lines without `:` are ignored.
pub fn parse_headers(text: &str) -> Vec<(String, String)> {
    text.split(['\n', '\r'])
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Why a request could not be shaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    /// The test is broken (empty `POST_RAW`/`PUT` body).
    Bork(String),
    /// The test needs something this runner does not provide.
    Skip(String),
}

/// Shape the CGI request of a test: sets `REQUEST_METHOD`, `CONTENT_TYPE` and
/// `CONTENT_LENGTH` in `env` and returns the body to feed on stdin, mirroring
/// the `POST_RAW` / `PUT` / `POST` / `GZIP_POST` / `DEFLATE_POST` / `GET`
/// branches of run-tests.php. `QUERY_STRING`, `HTTP_COOKIE`, `REDIRECT_STATUS`
/// and the script paths are the caller's business.
pub fn shape_request(test: &TestFile, env: &mut EnvMap) -> Result<Option<Vec<u8>>, ShapeError> {
    fn env_empty(env: &EnvMap, k: &str) -> bool {
        env.get(k).is_none_or(|v| v.is_empty() || v == "0")
    }
    fn raw_body(env: &mut EnvMap, section: &[u8]) -> Vec<u8> {
        let trimmed = super::expectf::php_trim(section);
        let mut request: Vec<u8> = Vec::new();
        let mut started = false;
        for line in trimmed.split(|&b| b == b'\n') {
            if env_empty(env, "CONTENT_TYPE") {
                let lower: Vec<u8> = line.iter().map(u8::to_ascii_lowercase).collect();
                if lower.starts_with(b"content-type:") {
                    let value = String::from_utf8_lossy(&line[b"content-type:".len()..])
                        .replace('\r', "")
                        .trim()
                        .to_string();
                    env.insert("CONTENT_TYPE".into(), value);
                    continue;
                }
            }
            if started {
                request.push(b'\n');
            }
            started = true;
            request.extend_from_slice(line);
        }
        request
    }

    if test.section_not_empty("POST_RAW") {
        let request = raw_body(env, test.section("POST_RAW").unwrap_or_default());
        env.insert("CONTENT_LENGTH".into(), request.len().to_string());
        if env_empty(env, "REQUEST_METHOD") {
            env.insert("REQUEST_METHOD".into(), "POST".into());
        }
        if request.is_empty() {
            return Err(ShapeError::Bork("empty $request".into()));
        }
        return Ok(Some(request));
    }
    if test.section_not_empty("PUT") {
        let request = raw_body(env, test.section("PUT").unwrap_or_default());
        env.insert("CONTENT_LENGTH".into(), request.len().to_string());
        env.insert("REQUEST_METHOD".into(), "PUT".into());
        if request.is_empty() {
            return Err(ShapeError::Bork("empty $request".into()));
        }
        return Ok(Some(request));
    }
    if test.section_not_empty("POST") {
        let post = super::expectf::php_trim(test.section("POST").unwrap_or_default()).to_vec();
        env.insert("REQUEST_METHOD".into(), "POST".into());
        if env_empty(env, "CONTENT_TYPE") {
            env.insert(
                "CONTENT_TYPE".into(),
                "application/x-www-form-urlencoded".into(),
            );
        }
        if env_empty(env, "CONTENT_LENGTH") {
            env.insert("CONTENT_LENGTH".into(), post.len().to_string());
        }
        return Ok(Some(post));
    }
    if test.section_not_empty("GZIP_POST") || test.section_not_empty("DEFLATE_POST") {
        return Err(ShapeError::Skip(
            "GZIP_POST/DEFLATE_POST need zlib compression in the runner".into(),
        ));
    }
    env.insert("REQUEST_METHOD".into(), "GET".into());
    env.insert("CONTENT_TYPE".into(), String::new());
    env.insert("CONTENT_LENGTH".into(), String::new());
    Ok(None)
}
