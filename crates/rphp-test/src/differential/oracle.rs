//! Locating and invoking the two processes under comparison: the pinned stock
//! `php` oracle and an `rphp` binary. Both run with a scrubbed environment, a
//! caller-chosen working directory, no stdin, captured stdout/stderr and a
//! timeout.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The pinned oracle ini: every `-d` setting stock php is started with. Fixed
/// so that the comparison does not depend on the machine's `php.ini`
/// (`-n` is passed as well).
pub const PHP_INI: &[(&str, &str)] = &[
    ("display_errors", "1"),
    ("log_errors", "0"),
    ("error_reporting", "E_ALL"),
    ("html_errors", "0"),
    ("date.timezone", "UTC"),
    ("precision", "14"),
    ("serialize_precision", "-1"),
    ("memory_limit", "-1"),
    ("output_buffering", "0"),
    ("implicit_flush", "1"),
    ("short_open_tag", "0"),
    ("zend.assertions", "-1"),
];

/// Environment variables inherited by both processes; everything else is
/// dropped and `LC_ALL=C`, `TZ=UTC` are set.
pub const KEPT_ENV: &[&str] = &["PATH", "HOME", "TMPDIR"];

/// Default per-process timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// What one process run produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunResult {
    /// Raw stdout bytes.
    pub stdout: Vec<u8>,
    /// Raw stderr bytes.
    pub stderr: Vec<u8>,
    /// Exit code (`128 + signal` when killed by a signal on unix, `-1` when
    /// the code is unknown).
    pub status: i32,
    /// The process exceeded the timeout and was killed.
    pub timed_out: bool,
}

impl RunResult {
    /// A result with the given stdout/stderr/status (for synthetic tests).
    pub fn new(stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>, status: i32) -> Self {
        RunResult { stdout: stdout.into(), stderr: stderr.into(), status, timed_out: false }
    }

    /// stdout as (lossy) text.
    pub fn stdout_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    /// stderr as (lossy) text.
    pub fn stderr_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }
}

/// The stock php to compare against: `$PHP_BIN` if set, else `php` found on
/// `$PATH`. `None` when neither exists (callers skip, not fail).
pub fn find_php() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("PHP_BIN") {
        let bin = PathBuf::from(bin);
        return if bin.is_file() { Some(bin) } else { None };
    }
    find_on_path("php")
}

/// An `rphp` override: `$RPHP_BIN` if set and it exists. Harnesses normally
/// use their own freshly built binary instead.
pub fn find_rphp() -> Option<PathBuf> {
    let bin = PathBuf::from(std::env::var_os("RPHP_BIN")?);
    bin.is_file().then_some(bin)
}

/// Look `name` up on `$PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The argument vector the oracle is started with: `-n`, every [`PHP_INI`]
/// setting as `-d key=value`, the script and its arguments.
pub fn php_args(script: &Path, args: &[String]) -> Vec<OsString> {
    let mut out: Vec<OsString> = vec!["-n".into()];
    for (k, v) in PHP_INI {
        out.push("-d".into());
        out.push(format!("{k}={v}").into());
    }
    out.push(script.as_os_str().to_owned());
    out.extend(args.iter().map(OsString::from));
    out
}

/// The command that runs `script` under stock php in `cwd`.
pub fn php_command(php: &Path, script: &Path, args: &[String], cwd: &Path) -> Command {
    let mut cmd = Command::new(php);
    cmd.args(php_args(script, args)).current_dir(cwd);
    scrub_env(&mut cmd);
    cmd
}

/// The command that runs `script` under `rphp` in `cwd`.
pub fn rphp_command(rphp: &Path, script: &Path, args: &[String], cwd: &Path) -> Command {
    let mut cmd = Command::new(rphp);
    cmd.arg(script).args(args).current_dir(cwd);
    scrub_env(&mut cmd);
    cmd
}

/// Run `script` under stock php.
pub fn run_php(
    php: &Path,
    script: &Path,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> std::io::Result<RunResult> {
    run_command(php_command(php, script, args, cwd), timeout)
}

/// Run `script` under `rphp`.
pub fn run_rphp(
    rphp: &Path,
    script: &Path,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> std::io::Result<RunResult> {
    run_command(rphp_command(rphp, script, args, cwd), timeout)
}

/// Drop the inherited environment except [`KEPT_ENV`], then pin `LC_ALL=C`
/// and `TZ=UTC`.
pub fn scrub_env(cmd: &mut Command) {
    cmd.env_clear();
    for key in KEPT_ENV {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("LC_ALL", "C");
    cmd.env("TZ", "UTC");
}

/// Spawn `cmd` with no stdin and piped stdout/stderr, wait up to `timeout`
/// (killing the process when it is exceeded) and collect the result.
pub fn run_command(mut cmd: Command, timeout: Duration) -> std::io::Result<RunResult> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
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

    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(RunResult { stdout, stderr, status: exit_code(&status), timed_out })
}

#[cfg(unix)]
fn exit_code(status: &ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal().map(|s| 128 + s)).unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: &ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}
