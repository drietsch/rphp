//! Running one `.phpt` under an engine binary — the port of `run_test()` in
//! php-src's run-tests.php: SKIPIF, EXTENSIONS, INI/ENV/ARGS/STDIN, CGI
//! environment shaping, CAPTURE_STDIO, CLEAN, XFAIL/FLAKY, timeouts and the
//! output normalisation. One real process per test, no shared state, so
//! tests can run on a rayon pool.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::expectf::{self, Diff};
use super::parse::TestFile;
use super::sections::{
    expand_ini_placeholders, parse_env_section, parse_extensions, parse_headers, shape_request,
    split_args, CaptureStdio, EnvMap, IniSettings, ShapeError,
};

/// run-tests.php's `$ini_overwrites` (8.5.0). Applied on top of `-n` for the
/// stock php engine so tests see the environment run-tests gives them.
pub const RUN_TESTS_INI_OVERWRITES: &[&str] = &[
    "output_handler=",
    "open_basedir=",
    "disable_functions=",
    "output_buffering=Off",
    // The constant, not a number: E_ALL is 30719 on 8.5 (no E_STRICT bit)
    // and run-tests lets the PHP under test evaluate it.
    "error_reporting=E_ALL",
    "fatal_error_backtraces=Off",
    "display_errors=1",
    "display_startup_errors=1",
    "log_errors=0",
    "html_errors=0",
    "track_errors=0",
    "report_zend_debug=0",
    "docref_root=",
    "docref_ext=.html",
    "error_prepend_string=",
    "error_append_string=",
    "auto_prepend_file=",
    "auto_append_file=",
    "ignore_repeated_errors=0",
    "precision=14",
    "serialize_precision=-1",
    "memory_limit=128M",
    "opcache.fast_shutdown=0",
    "opcache.file_update_protection=0",
    "opcache.revalidate_freq=0",
    "opcache.jit_hot_loop=1",
    "opcache.jit_hot_func=1",
    "opcache.jit_hot_return=1",
    "opcache.jit_hot_side_exit=1",
    "opcache.jit_max_root_traces=100000",
    "opcache.jit_max_side_traces=100000",
    "opcache.jit_max_exit_counters=100000",
    "opcache.protect_memory=1",
    "zend.assertions=1",
    "zend.exception_ignore_args=0",
    "zend.exception_string_param_max_len=15",
    "short_open_tag=0",
];

/// What run-tests learns about a php binary once: its extension directory
/// and the loaded extensions (lower-cased, `Zend OPcache` → `opcache`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpProbe {
    /// `ini_get('extension_dir')`.
    pub extension_dir: PathBuf,
    /// `get_loaded_extensions()`, normalised.
    pub loaded: Vec<String>,
}

/// The binary that runs the tests.
#[derive(Debug)]
pub struct Engine {
    /// The CLI binary (`rphp` or `php`).
    pub binary: PathBuf,
    /// Arguments placed before everything else (`-n` for stock php).
    pub extra_args: Vec<String>,
    /// Stock php semantics: `-f`, `-q`, `-d` are understood, extensions can
    /// be probed and loaded, `php-cgi` may sit next to the binary.
    pub is_php: bool,
    /// A CGI binary for GET/POST/COOKIE tests; without one, stock php skips
    /// them ("CGI not available") and rphp runs them with the CLI binary.
    pub cgi_binary: Option<PathBuf>,
    /// `name=value` INI overwrites applied to every test (run-tests' list for
    /// php, nothing for rphp whose defaults are the `-n` equivalent).
    pub ini_overwrites: Vec<String>,
    probe: OnceLock<Option<PhpProbe>>,
    fingerprint: OnceLock<String>,
}

impl Engine {
    /// An `rphp` engine. Test `--INI--` entries are passed as `-d` (the flag
    /// arrives with the CLI SAPI work); no overwrites, no CGI binary.
    pub fn rphp(binary: impl Into<PathBuf>) -> Self {
        Engine {
            binary: binary.into(),
            extra_args: Vec::new(),
            is_php: false,
            cgi_binary: None,
            ini_overwrites: Vec::new(),
            probe: OnceLock::new(),
            fingerprint: OnceLock::new(),
        }
    }

    /// A stock `php` engine: `-n` plus run-tests' INI overwrites, and
    /// `php-cgi` next to the binary when present.
    pub fn php(binary: impl Into<PathBuf>) -> Self {
        let binary: PathBuf = binary.into();
        let cgi = binary
            .parent()
            .map(|d| d.join("php-cgi"))
            .filter(|p| p.is_file());
        Engine {
            binary,
            extra_args: vec!["-n".to_string()],
            is_php: true,
            cgi_binary: cgi,
            ini_overwrites: RUN_TESTS_INI_OVERWRITES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            probe: OnceLock::new(),
            fingerprint: OnceLock::new(),
        }
    }

    /// `$RPHP_BIN`, else `<workspace>/target/debug/rphp`.
    pub fn default_rphp_binary() -> PathBuf {
        if let Some(p) = std::env::var_os("RPHP_BIN") {
            return PathBuf::from(p);
        }
        super::workspace_root()
            .join("target")
            .join("debug")
            .join("rphp")
    }

    /// Resolve a program name through `PATH` (or return it unchanged when it
    /// already has a directory component).
    pub fn find_in_path(name: &str) -> Option<PathBuf> {
        let p = Path::new(name);
        if p.components().count() > 1 {
            return p.is_file().then(|| p.to_path_buf());
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|c| c.is_file())
    }

    /// Probe extension information (stock php only; cached).
    pub fn probe(&self) -> Option<&PhpProbe> {
        self.probe
            .get_or_init(|| {
                if !self.is_php {
                    return None;
                }
                let out = Command::new(&self.binary)
                    .args(&self.extra_args)
                    .arg("-d")
                    .arg("display_errors=0")
                    .arg("-r")
                    .arg("echo ini_get('extension_dir'), \"\\n\", implode(',', get_loaded_extensions());")
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()?;
                let text = String::from_utf8_lossy(&out.stdout);
                let mut lines = text.lines();
                let extension_dir = PathBuf::from(lines.next()?.trim());
                let loaded = lines
                    .next()
                    .unwrap_or("")
                    .split(',')
                    .filter(|s| !s.is_empty() && *s != "Core")
                    .map(|s| {
                        if s == "Zend OPcache" {
                            "opcache".to_string()
                        } else {
                            s.to_ascii_lowercase()
                        }
                    })
                    .collect();
                Some(PhpProbe {
                    extension_dir,
                    loaded,
                })
            })
            .as_ref()
    }

    /// A digest of everything that changes test outcomes on the engine side:
    /// binary contents and mtime, extra args, INI overwrites, CGI binary.
    pub fn fingerprint(&self) -> &str {
        self.fingerprint.get_or_init(|| {
            let mut h = Sha256::new();
            h.update(self.binary.display().to_string());
            h.update(b"\0");
            if let Ok(bytes) = fs::read(&self.binary) {
                h.update(&bytes);
            }
            if let Ok(meta) = fs::metadata(&self.binary) {
                if let Ok(m) = meta.modified() {
                    if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                        h.update(d.as_secs().to_le_bytes());
                    }
                }
            }
            for a in &self.extra_args {
                h.update(a);
                h.update(b"\0");
            }
            for i in &self.ini_overwrites {
                h.update(i);
                h.update(b"\0");
            }
            h.update([self.is_php as u8]);
            if let Some(c) = &self.cgi_binary {
                h.update(c.display().to_string());
            }
            hex(&h.finalize())
        })
    }

    /// A short label for reports (`php`/`rphp` plus the binary path).
    pub fn label(&self) -> String {
        format!(
            "{} ({})",
            if self.is_php { "php" } else { "rphp" },
            self.binary.display()
        )
    }
}

/// Lower-case hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Per-run knobs.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Kill a test process after this long (run-tests: 60 s).
    pub timeout: Duration,
    /// Working directory for the spawned processes; run-tests uses the
    /// php-src root. `None` = the test's own directory.
    pub cwd: Option<PathBuf>,
    /// Leave the generated `<name>.php` beside a failing test for debugging.
    pub keep_files: bool,
    /// Re-run a failing test once when run-tests would (FLAKY section,
    /// timing functions in FILE, network-ish error text in the output).
    pub retry_flaky: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            timeout: Duration::from_secs(60),
            cwd: None,
            keep_files: false,
            retry_flaky: true,
        }
    }
}

/// The verdict for one test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Output matched.
    Pass,
    /// Output did not match.
    Fail {
        /// Where and how.
        diff: Diff,
    },
    /// SKIPIF (or the runner) decided not to run it.
    Skip {
        /// The `skip …` text, or the runner's reason.
        reason: String,
    },
    /// Failed, and an XFAIL section said it would.
    XFail {
        /// The XFAIL text.
        reason: String,
        /// Where and how it failed.
        diff: Diff,
    },
    /// Passed although an XFAIL section said it would not.
    XPass,
    /// The test file is broken, SKIPIF printed garbage, or CLEAN printed
    /// something.
    Borked {
        /// What went wrong.
        reason: String,
    },
    /// Killed after the timeout.
    Timeout,
    /// The process died from a signal.
    Crash {
        /// The signal number (unix).
        signal: Option<i32>,
        /// The exit code, when there was one.
        exit: Option<i32>,
    },
}

impl Outcome {
    /// run-tests' upper-case label.
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail { .. } => "FAIL",
            Outcome::Skip { .. } => "SKIP",
            Outcome::XFail { .. } => "XFAIL",
            Outcome::XPass => "XPASS",
            Outcome::Borked { .. } => "BORK",
            Outcome::Timeout => "TIMEOUT",
            Outcome::Crash { .. } => "CRASH",
        }
    }

    /// Counts towards the baseline?
    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Pass)
    }

    /// Something a human should look at (everything but PASS/SKIP/XFAIL).
    pub fn is_failure(&self) -> bool {
        !matches!(
            self,
            Outcome::Pass | Outcome::Skip { .. } | Outcome::XFail { .. }
        )
    }
}

/// One executed (or cached) test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// The `.phpt` path as given.
    pub path: PathBuf,
    /// The `--TEST--` text.
    pub name: String,
    /// The verdict.
    pub outcome: Outcome,
    /// Wall time of the run (0 for cache hits).
    pub duration_ms: u64,
    /// SKIPIF `info`/`warn` lines, retry notes, regex errors.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Came from the result cache.
    #[serde(default)]
    pub cached: bool,
}

/// Where stdin comes from.
#[derive(Debug, Clone, Copy)]
pub enum StdinMode<'a> {
    /// `/dev/null`.
    Null,
    /// These bytes, then EOF.
    Bytes(&'a [u8]),
    /// The runner's own stdin.
    Inherit,
}

/// A finished (or killed) process.
#[derive(Debug)]
pub struct Exec {
    /// Captured output (stdout and stderr interleaved when both captured).
    pub output: Vec<u8>,
    /// Exit status; `None` after a timeout kill.
    pub status: Option<ExitStatus>,
    /// Killed by the runner.
    pub timed_out: bool,
}

/// Spawn `cmd`, feed stdin, collect the captured streams into one buffer
/// (sharing a single file description when both stdout and stderr are
/// captured, which is what `2>&1` does), and kill it after `timeout`.
pub fn exec(
    cmd: &mut Command,
    stdin: StdinMode<'_>,
    capture: CaptureStdio,
    timeout: Duration,
) -> io::Result<Exec> {
    let mut outfile: Option<fs::File> = None;
    if capture.stdout || capture.stderr {
        let f = tempfile::tempfile()?;
        if capture.stdout {
            cmd.stdout(Stdio::from(f.try_clone()?));
        }
        if capture.stderr {
            cmd.stderr(Stdio::from(f.try_clone()?));
        }
        outfile = Some(f);
    }
    match stdin {
        StdinMode::Null => cmd.stdin(Stdio::null()),
        StdinMode::Bytes(_) => cmd.stdin(Stdio::piped()),
        StdinMode::Inherit => cmd.stdin(Stdio::inherit()),
    };
    let mut child = cmd.spawn()?;
    if let StdinMode::Bytes(b) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            let data = b.to_vec();
            // A separate writer so a child that never reads cannot wedge us.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
    }
    let start = Instant::now();
    let mut nap = Duration::from_micros(200);
    let (status, timed_out) = loop {
        if let Some(st) = child.try_wait()? {
            break (Some(st), false);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        std::thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(10));
    };
    let mut output = Vec::new();
    if let Some(mut f) = outfile {
        f.seek(SeekFrom::Start(0))?;
        f.read_to_end(&mut output)?;
    }
    Ok(Exec {
        output,
        status,
        timed_out,
    })
}

/// The signal that killed a process, if any.
pub fn signal_of(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

/// Apply environment overrides: empty values unset (proc_open semantics).
fn apply_env(cmd: &mut Command, env: &EnvMap) {
    for (k, v) in env {
        if v.is_empty() {
            cmd.env_remove(k);
        } else {
            cmd.env(k, v);
        }
    }
}

/// The files run-tests writes beside a test: `<name>.php`, `<name>.skip.php`,
/// `<name>.clean.php` (the `.phpt` suffix is replaced, so `x.phpt` → `x.php`).
#[derive(Debug, Clone)]
pub struct GeneratedFiles {
    /// The script.
    pub php: PathBuf,
    /// The SKIPIF script.
    pub skip: PathBuf,
    /// The CLEAN script.
    pub clean: PathBuf,
}

impl GeneratedFiles {
    /// Paths for the test at `path`.
    pub fn for_test(path: &Path) -> Self {
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = file.strip_suffix("phpt").unwrap_or(&file).to_string();
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        GeneratedFiles {
            php: dir.join(format!("{base}php")),
            skip: dir.join(format!("{base}skip.php")),
            clean: dir.join(format!("{base}clean.php")),
        }
    }

    /// Remove all three (ignoring missing ones).
    pub fn remove_all(&self) {
        for p in [&self.php, &self.skip, &self.clean] {
            let _ = fs::remove_file(p);
        }
    }

    /// Is `file` (a name in the same directory) one of ours?
    pub fn is_generated_name(name: &str, has_phpt: impl Fn(&str) -> bool) -> bool {
        for suffix in [".php", ".skip.php", ".clean.php"] {
            if let Some(stem) = name.strip_suffix(suffix) {
                if has_phpt(&format!("{stem}.phpt")) {
                    return true;
                }
            }
        }
        for suffix in [
            ".diff", ".out", ".exp", ".log", ".sh", ".mem", ".stdin", ".post",
        ] {
            if let Some(stem) = name.strip_suffix(suffix) {
                if has_phpt(&format!("{stem}.phpt")) {
                    return true;
                }
            }
        }
        false
    }
}

/// Run one test file end to end.
pub fn run_test(engine: &Engine, opts: &RunOptions, path: &Path) -> TestResult {
    let start = Instant::now();
    let mut notes = Vec::new();
    let (name, outcome) = match TestFile::load(path) {
        Err(e) => (
            String::new(),
            Outcome::Borked {
                reason: e.to_string(),
            },
        ),
        Ok(test) => {
            let name = test.name();
            let outcome = run_parsed(engine, opts, test, &mut notes);
            (name, outcome)
        }
    };
    TestResult {
        path: path.to_path_buf(),
        name,
        outcome,
        duration_ms: start.elapsed().as_millis() as u64,
        notes,
        cached: false,
    }
}

/// `is_flaky()`: tests run-tests retries once on failure.
fn is_flaky(test: &TestFile) -> bool {
    if test.has("FLAKY") {
        return true;
    }
    if test
        .section_str("SKIPIF")
        .is_some_and(|s| s.contains("SKIP_PERF_SENSITIVE"))
    {
        return true;
    }
    static RE: OnceLock<regex::bytes::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::bytes::RegexBuilder::new(r"\b(disk_free_space|hrtime|microtime|sleep|usleep)\(")
            .case_insensitive(true)
            .unicode(false)
            .build()
            .expect("static regex")
    });
    re.is_match(test.file())
}

/// `is_flaky_output()`: output that smells like a transient failure.
fn is_flaky_output(output: &[u8]) -> bool {
    static RE: OnceLock<regex::bytes::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::bytes::RegexBuilder::new(
            r"\b(404: page not found|address already in use|connection refused|deadlock|mailbox already exists|timed out)\b",
        )
        .case_insensitive(true)
        .unicode(false)
        .build()
        .expect("static regex")
    });
    re.is_match(output)
}

/// Strip the header block a CGI binary prints: `^(.*?)\r?\n\r?\n(.*)`.
fn split_cgi_output(raw: &[u8]) -> (Vec<u8>, Vec<(String, String)>) {
    let mut i = 0;
    while i < raw.len() {
        let mut j = i;
        if raw.get(j) == Some(&b'\r') {
            j += 1;
        }
        if raw.get(j) == Some(&b'\n') {
            j += 1;
            let mut k = j;
            if raw.get(k) == Some(&b'\r') {
                k += 1;
            }
            if raw.get(k) == Some(&b'\n') {
                let headers = parse_headers(&String::from_utf8_lossy(&raw[..i]));
                let body = expectf::php_trim(&raw[k + 1..]).to_vec();
                return (body, headers);
            }
        }
        i += 1;
    }
    (expectf::normalize_output(raw), Vec::new())
}

/// The body of [`run_test`] once the file parsed. `notes` collects
/// SKIPIF info/warn text and retry notes.
pub fn run_parsed(
    engine: &Engine,
    opts: &RunOptions,
    mut test: TestFile,
    notes: &mut Vec<String>,
) -> Outcome {
    let skip = |reason: &str| Outcome::Skip {
        reason: reason.to_string(),
    };
    let bork = |reason: String| Outcome::Borked { reason };

    if test.has("PHPDBG") {
        return skip("phpdbg tests are not supported by this runner");
    }
    if test.has("REDIRECTTEST") {
        return skip("REDIRECTTEST is not supported by this runner");
    }

    let capture = CaptureStdio::parse(test.section_str("CAPTURE_STDIO").as_deref());
    let uses_cgi = test.is_cgi() && engine.cgi_binary.is_some();
    if test.is_cgi() && !uses_cgi && engine.is_php {
        return skip("CGI not available");
    }
    let program: &Path = if uses_cgi {
        engine.cgi_binary.as_deref().expect("checked above")
    } else {
        &engine.binary
    };

    let test_dir = test.dir().to_path_buf();
    let cwd = opts.cwd.clone().unwrap_or_else(|| test_dir.clone());
    let files = GeneratedFiles::for_test(test.path());
    files.remove_all();
    let cleanup = |failed: bool| {
        if !(failed && opts.keep_files) {
            files.remove_all();
        }
    };

    // Environment: run-tests resets the CGI variables and TZ, then applies ENV.
    let mut env: EnvMap = BTreeMap::new();
    for k in [
        "REDIRECT_STATUS",
        "QUERY_STRING",
        "PATH_TRANSLATED",
        "SCRIPT_FILENAME",
        "REQUEST_METHOD",
        "CONTENT_TYPE",
        "CONTENT_LENGTH",
        "TZ",
    ] {
        env.insert(k.to_string(), String::new());
    }
    let binary_str = engine.binary.display().to_string();
    env.insert("TEST_PHP_EXECUTABLE".into(), binary_str.clone());
    env.insert(
        "TEST_PHP_EXECUTABLE_ESCAPED".into(),
        format!("'{}'", binary_str.replace('\'', "'\\''")),
    );
    if let Some(cgi) = &engine.cgi_binary {
        env.insert("TEST_PHP_CGI_EXECUTABLE".into(), cgi.display().to_string());
    }
    if test.section_not_empty("ENV") {
        let text = test.section_str("ENV").unwrap_or_default().into_owned();
        for (k, v) in parse_env_section(&text, &test_dir) {
            env.insert(k, v);
        }
    }

    // INI: extensions first, then the overwrites (= `$orig_ini_settings`),
    // then the test's own --INI-- block.
    let mut ini = IniSettings::new();
    if test.has("EXTENSIONS") {
        let wanted = parse_extensions(&test.section_str("EXTENSIONS").unwrap_or_default());
        if let Some(probe) = engine.probe() {
            let mut missing = Vec::new();
            for ext in wanted {
                let key = ext.to_ascii_lowercase();
                if probe.loaded.contains(&key) {
                    continue;
                }
                let file = probe.extension_dir.join(format!("{ext}.so"));
                let directive = if key == "opcache" || key == "xdebug" {
                    "zend_extension"
                } else {
                    "extension"
                };
                ini.set(directive, &file.display().to_string());
                if !file.is_file() {
                    missing.push(ext);
                }
            }
            if !missing.is_empty() {
                return skip(&format!(
                    "Required extension{} missing: {}",
                    if missing.len() > 1 { "s" } else { "" },
                    missing.join(", ")
                ));
            }
        }
        // Without a probe (rphp) the test itself tells us whether the
        // functions exist; that is the signal the burn-down wants.
    }
    ini.add_lines(engine.ini_overwrites.iter().map(String::as_str));
    let orig_ini = ini.clone();
    if test.has("INI") {
        let raw = test.section_str("INI").unwrap_or_default().into_owned();
        match expand_ini_placeholders(&raw, &test_dir) {
            Ok(text) => ini.add_text(&text),
            Err(reason) => return skip(&reason),
        }
    }
    {
        let mut extra: Vec<String> = engine.extra_args.clone();
        extra.extend(ini.to_args());
        env.insert("TEST_PHP_EXTRA_ARGS".into(), extra.join(" "));
    }

    // SKIPIF.
    if test.section_not_empty("SKIPIF") {
        let code = test.section("SKIPIF").unwrap_or_default().to_vec();
        if let Err(e) = fs::write(&files.skip, &code) {
            return bork(format!("cannot write {}: {e}", files.skip.display()));
        }
        let mut cmd = Command::new(program);
        cmd.args(&engine.extra_args);
        if uses_cgi {
            cmd.arg("-C");
        }
        if engine.is_php {
            cmd.arg("-q");
        }
        cmd.args(orig_ini.to_args());
        if engine.is_php {
            cmd.args(["-d", "display_errors=1", "-d", "display_startup_errors=0"]);
        }
        cmd.arg(&files.skip);
        let mut skip_env = env.clone();
        for k in [
            "REQUEST_METHOD",
            "QUERY_STRING",
            "PATH_TRANSLATED",
            "SCRIPT_FILENAME",
        ] {
            skip_env.insert(k.to_string(), String::new());
        }
        apply_env(&mut cmd, &skip_env);
        cmd.current_dir(&cwd);
        let capture_skip = CaptureStdio {
            stdin: true,
            stdout: true,
            stderr: false,
        };
        cmd.stderr(Stdio::null());
        let ex = match exec(&mut cmd, StdinMode::Null, capture_skip, opts.timeout) {
            Ok(ex) => ex,
            Err(e) => {
                cleanup(false);
                return bork(format!("cannot run SKIPIF: {e}"));
            }
        };
        let _ = fs::remove_file(&files.skip);
        if ex.timed_out {
            cleanup(false);
            return bork("SKIPIF timed out".to_string());
        }
        let out = String::from_utf8_lossy(expectf::php_trim(&ex.output)).into_owned();
        let lower = out.to_ascii_lowercase();
        if lower.starts_with("skip") {
            cleanup(false);
            let reason = out[4..].trim_start();
            return Outcome::Skip {
                reason: reason.to_string(),
            };
        } else if lower.starts_with("info") {
            let m = out[4..].trim_start();
            if !m.is_empty() {
                notes.push(format!("info: {m}"));
            }
        } else if lower.starts_with("warn") {
            let m = out[4..].trim_start();
            if !m.is_empty() {
                notes.push(format!("warn: {m}"));
            }
        } else if lower.starts_with("xfail") {
            test.set_section("XFAIL", out[5..].trim_start().as_bytes());
        } else if lower.starts_with("xleak") {
            test.set_section("XLEAK", out[5..].trim_start().as_bytes());
        } else if lower.starts_with("flaky") {
            test.set_section("FLAKY", out[5..].trim_start().as_bytes());
        } else if lower.starts_with("nocache") || out.is_empty() {
            // Nothing to do.
        } else {
            cleanup(false);
            return bork(format!("invalid output from SKIPIF: {out}"));
        }
    }

    // The script.
    if let Err(e) = fs::write(&files.php, test.file()) {
        return bork(format!("cannot write {}: {e}", files.php.display()));
    }
    let script_abs = fs::canonicalize(&files.php).unwrap_or_else(|_| files.php.clone());

    // CGI variables.
    let query = test
        .section("GET")
        .map(|s| String::from_utf8_lossy(expectf::php_trim(s)).into_owned())
        .unwrap_or_default();
    env.insert("REDIRECT_STATUS".into(), "1".into());
    if env.get("QUERY_STRING").is_none_or(|v| v.is_empty()) {
        env.insert("QUERY_STRING".into(), query);
    }
    for k in ["PATH_TRANSLATED", "SCRIPT_FILENAME"] {
        if env.get(k).is_none_or(|v| v.is_empty()) {
            env.insert(k.to_string(), script_abs.display().to_string());
        }
    }
    let cookie = test
        .section("COOKIE")
        .map(|s| String::from_utf8_lossy(expectf::php_trim(s)).into_owned())
        .unwrap_or_default();
    env.insert("HTTP_COOKIE".into(), cookie);

    let args: Vec<String> = test
        .section_str("ARGS")
        .map(|a| split_args(&a))
        .unwrap_or_default();

    let body = match shape_request(&test, &mut env) {
        Ok(b) => b,
        Err(ShapeError::Bork(r)) => {
            cleanup(false);
            return bork(r);
        }
        Err(ShapeError::Skip(r)) => {
            cleanup(false);
            return skip(&r);
        }
    };
    let stdin_bytes: Option<Vec<u8>> = body.or_else(|| test.section("STDIN").map(<[u8]>::to_vec));

    let flaky = is_flaky(&test);
    let kind = test.expect_kind();
    let wanted = expectf::normalize_expected(test.expected());
    let mut retried = false;

    loop {
        let mut cmd = Command::new(program);
        cmd.args(&engine.extra_args);
        if uses_cgi {
            cmd.arg("-C");
        }
        cmd.args(ini.to_args());
        if engine.is_php {
            cmd.arg("-f").arg(&files.php);
            if !args.is_empty() {
                cmd.arg("--").args(&args);
            }
        } else {
            cmd.arg(&files.php).args(&args);
        }
        apply_env(&mut cmd, &env);
        cmd.current_dir(&cwd);
        let stdin_mode = match (&stdin_bytes, capture.stdin) {
            (Some(b), _) => StdinMode::Bytes(b.as_slice()),
            (None, true) => StdinMode::Null,
            (None, false) => StdinMode::Null,
        };
        let ex = match exec(&mut cmd, stdin_mode, capture, opts.timeout) {
            Ok(ex) => ex,
            Err(e) => {
                cleanup(false);
                return bork(format!("cannot run {}: {e}", program.display()));
            }
        };
        if ex.timed_out {
            cleanup(true);
            return Outcome::Timeout;
        }

        // CLEAN (runs with the CLI binary, without the CGI variables).
        let clean_output: Option<String> = if test.section_not_empty("CLEAN") {
            let code = expectf::php_trim(test.section("CLEAN").unwrap_or_default()).to_vec();
            if fs::write(&files.clean, &code).is_ok() {
                let mut c = Command::new(&engine.binary);
                c.args(&engine.extra_args);
                if engine.is_php {
                    c.arg("-q");
                }
                c.args(orig_ini.to_args());
                c.arg(&files.clean);
                let mut clean_env = env.clone();
                for k in [
                    "REQUEST_METHOD",
                    "QUERY_STRING",
                    "PATH_TRANSLATED",
                    "SCRIPT_FILENAME",
                ] {
                    clean_env.insert(k.to_string(), String::new());
                }
                apply_env(&mut c, &clean_env);
                c.current_dir(&cwd);
                c.stderr(Stdio::null());
                let out = exec(
                    &mut c,
                    StdinMode::Null,
                    CaptureStdio {
                        stdin: true,
                        stdout: true,
                        stderr: false,
                    },
                    opts.timeout,
                )
                .map(|e| String::from_utf8_lossy(expectf::php_trim(&e.output)).into_owned())
                .unwrap_or_default();
                let _ = fs::remove_file(&files.clean);
                Some(out)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(st) = &ex.status {
            if let Some(sig) = signal_of(st) {
                cleanup(true);
                return Outcome::Crash {
                    signal: Some(sig),
                    exit: st.code(),
                };
            }
        }

        let (output, headers) = if uses_cgi {
            split_cgi_output(&ex.output)
        } else {
            (expectf::normalize_output(&ex.output), Vec::new())
        };

        let mut header_failure: Option<String> = None;
        if test.has("EXPECTHEADERS") {
            let want = parse_headers(&test.section_str("EXPECTHEADERS").unwrap_or_default());
            let mut problems = Vec::new();
            for (k, v) in &want {
                match headers.iter().find(|(hk, _)| hk == k) {
                    Some((_, hv)) if hv == v => {}
                    Some((_, hv)) => problems.push(format!("{k}: {hv} (expected {v})")),
                    None => problems.push(format!("{k}: <missing> (expected {v})")),
                }
            }
            if !problems.is_empty() {
                header_failure = Some(problems.join("; "));
            }
        }

        let matched = match expectf::matches(kind, &wanted, &output) {
            Ok(m) => m,
            Err(e) => {
                notes.push(format!("regex error: {e}"));
                false
            }
        };
        let passed = matched && header_failure.is_none();

        if !passed && !retried && opts.retry_flaky && (flaky || is_flaky_output(&output)) {
            retried = true;
            continue;
        }

        if passed {
            cleanup(false);
            if let Some(c) = clean_output {
                if !c.is_empty() {
                    return bork(format!("invalid output from CLEAN: {c}"));
                }
            }
            if test.has("XFAIL") {
                return Outcome::XPass;
            }
            if retried {
                notes.push("passed on retry".to_string());
            }
            return Outcome::Pass;
        }

        cleanup(true);
        let mut diff = expectf::diff(kind, &wanted, &output);
        if let Some(h) = header_failure {
            diff.text = format!("headers: {h}\n{}", diff.text);
            if matched {
                diff.expected = format!("headers: {h}");
                diff.actual = String::new();
            }
        }
        if test.has("XFAIL") {
            let reason = test
                .section_str("XFAIL")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            return Outcome::XFail { reason, diff };
        }
        return Outcome::Fail { diff };
    }
}
