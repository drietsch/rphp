//! The comparison policy: stdout byte-exact by default (or against an
//! `EXPECTF` template when a `.expectf` sidecar exists), stderr after
//! normalization and after stripping php's `PHP `-prefixed log duplicates,
//! exit codes exact. Allowlist entries for the snippet run their normalizer
//! over *both* sides of every channel before the comparison.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use regex::bytes::Regex;

use super::allowlist::Allowlist;
use super::normalize::{Category, NormalizeContext};
use super::oracle::{run_php, run_rphp, RunResult, DEFAULT_TIMEOUT};
use super::sidecar;

/// Which channel a mismatch was found on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
    /// The exit code (or a timeout).
    Exit,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Channel::Stdout => "stdout",
            Channel::Stderr => "stderr",
            Channel::Exit => "exit",
        })
    }
}

/// The outcome of comparing one snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every channel agrees.
    Match,
    /// The first channel that disagrees, with a diff around the first
    /// difference (non-UTF-8 bytes escaped as `\xNN`).
    Mismatch {
        /// The channel that disagrees.
        channel: Channel,
        /// Human-readable diff or explanation.
        first_diff: String,
    },
}

impl Verdict {
    /// `true` for [`Verdict::Match`].
    pub fn is_match(&self) -> bool {
        matches!(self, Verdict::Match)
    }
}

/// Everything the comparison needs besides the two results.
#[derive(Debug, Clone)]
pub struct Policy<'a> {
    /// The snippet's allowlist-relative name (forward slashes).
    pub snippet: &'a str,
    /// The allowlist whose matching entries normalize both sides.
    pub allowlist: &'a Allowlist,
    /// An `EXPECTF` template both stdouts must match instead of each other.
    pub expectf: Option<&'a [u8]>,
    /// Path context for the php side.
    pub php_ctx: NormalizeContext,
    /// Path context for the rphp side.
    pub rphp_ctx: NormalizeContext,
}

impl<'a> Policy<'a> {
    /// A policy with no template and empty path contexts.
    pub fn new(snippet: &'a str, allowlist: &'a Allowlist) -> Self {
        Policy {
            snippet,
            allowlist,
            expectf: None,
            php_ctx: NormalizeContext::new(),
            rphp_ctx: NormalizeContext::new(),
        }
    }

    /// Set the `EXPECTF` template.
    pub fn with_expectf(mut self, template: &'a [u8]) -> Self {
        self.expectf = Some(template);
        self
    }
}

/// Compare a php run against an rphp run under `policy`.
pub fn compare(php: &RunResult, rphp: &RunResult, policy: &Policy<'_>) -> Verdict {
    if php.timed_out || rphp.timed_out {
        let who = match (php.timed_out, rphp.timed_out) {
            (true, true) => "both php and rphp",
            (true, false) => "php",
            _ => "rphp",
        };
        return Verdict::Mismatch { channel: Channel::Exit, first_diff: format!("{who} timed out") };
    }

    let list = policy.allowlist;
    let php_out = list.normalize(policy.snippet, &php.stdout, &policy.php_ctx);
    let rphp_out = list.normalize(policy.snippet, &rphp.stdout, &policy.rphp_ctx);
    if let Some(template) = policy.expectf {
        if !expectf_matches(template, &php_out) {
            return Verdict::Mismatch {
                channel: Channel::Stdout,
                first_diff: format!(
                    "stock php stdout does not match the .expectf template\n{}",
                    unified_diff(template, &php_out, "expectf", "php")
                ),
            };
        }
        if !expectf_matches(template, &rphp_out) {
            return Verdict::Mismatch {
                channel: Channel::Stdout,
                first_diff: format!(
                    "rphp stdout does not match the .expectf template\n{}",
                    unified_diff(template, &rphp_out, "expectf", "rphp")
                ),
            };
        }
    } else if php_out != rphp_out {
        return Verdict::Mismatch {
            channel: Channel::Stdout,
            first_diff: unified_diff(&php_out, &rphp_out, "php", "rphp"),
        };
    }

    let php_err = strip_log_duplicates(&php.stderr, &php.stdout);
    let rphp_err = strip_log_duplicates(&rphp.stderr, &rphp.stdout);
    let php_err = list.normalize(policy.snippet, &php_err, &policy.php_ctx);
    let rphp_err = list.normalize(policy.snippet, &rphp_err, &policy.rphp_ctx);
    if php_err != rphp_err {
        return Verdict::Mismatch {
            channel: Channel::Stderr,
            first_diff: unified_diff(&php_err, &rphp_err, "php", "rphp"),
        };
    }

    if php.status != rphp.status {
        return Verdict::Mismatch {
            channel: Channel::Exit,
            first_diff: format!("exit code differs: php {} vs rphp {}", php.status, rphp.status),
        };
    }
    Verdict::Match
}

/// Drop stderr lines that are php's `log_errors` copy of a message already
/// displayed on stdout (`PHP Fatal error:  X` on stderr next to
/// `Fatal error: X` on stdout), including the stack-trace continuation lines
/// that follow such a message.
pub fn strip_log_duplicates(stderr: &[u8], stdout: &[u8]) -> Vec<u8> {
    let shown: HashSet<&[u8]> = stdout.split(|&c| c == b'\n').collect();
    let mut out = Vec::with_capacity(stderr.len());
    let mut in_block = false;
    for line in stderr.split_inclusive(|&c| c == b'\n') {
        let bare = line.strip_suffix(b"\n").unwrap_or(line);
        if let Some(rest) = bare.strip_prefix(b"PHP ") {
            if shown.contains(collapse_log_colon(rest).as_slice()) {
                in_block = true;
                continue;
            }
        }
        if in_block && shown.contains(bare) {
            continue;
        }
        in_block = false;
        out.extend_from_slice(line);
    }
    out
}

/// `Fatal error:  msg` (log form) → `Fatal error: msg` (display form).
fn collapse_log_colon(line: &[u8]) -> Vec<u8> {
    if let Some(pos) = line.windows(3).position(|w| w == b":  ") {
        let mut v = Vec::with_capacity(line.len() - 1);
        v.extend_from_slice(&line[..pos + 2]);
        v.extend_from_slice(&line[pos + 3..]);
        v
    } else {
        line.to_vec()
    }
}

/// Translate a run-tests `EXPECTF` template into an anchored byte regex.
///
/// Supported wildcards: `%e` (directory separator), `%s`/`%S` (a non-newline
/// run, `+`/`*`), `%a`/`%A` (any run incl. newlines), `%w` (whitespace),
/// `%i` (signed int), `%d` (unsigned int), `%x` (hex), `%f` (float), `%c`
/// (one char), `%r...%r` (a raw regex). Everything else is literal.
pub fn expectf_regex(template: &[u8]) -> Result<Regex, regex::Error> {
    let mut pat = String::from("(?s-u)^");
    let mut i = 0;
    while i < template.len() {
        let c = template[i];
        if c == b'%' && i + 1 < template.len() {
            let piece: Option<&str> = match template[i + 1] {
                b'e' => Some(r"/"),
                b's' => Some(r"[^\r\n]+"),
                b'S' => Some(r"[^\r\n]*"),
                b'a' => Some(r".+"),
                b'A' => Some(r".*"),
                b'w' => Some(r"\s*"),
                b'i' => Some(r"[+-]?\d+"),
                b'd' => Some(r"\d+"),
                b'x' => Some(r"[0-9a-fA-F]+"),
                b'f' => Some(r"[+-]?\.?\d+\.?\d*(?:[Ee][+-]?\d+)?"),
                b'c' => Some(r"."),
                b'r' => {
                    let body = &template[i + 2..];
                    if let Some(end) = body.windows(2).position(|w| w == b"%r") {
                        pat.push_str("(?:");
                        pat.push_str(&String::from_utf8_lossy(&body[..end]));
                        pat.push(')');
                        i += 2 + end + 2;
                        continue;
                    }
                    None
                }
                _ => None,
            };
            if let Some(p) = piece {
                pat.push_str(p);
                i += 2;
                continue;
            }
        }
        push_literal_byte(&mut pat, c);
        i += 1;
    }
    pat.push('$');
    Regex::new(&pat)
}

fn push_literal_byte(pat: &mut String, c: u8) {
    match c {
        b'\\' | b'.' | b'+' | b'*' | b'?' | b'(' | b')' | b'|' | b'[' | b']' | b'{' | b'}' | b'^'
        | b'$' | b'#' | b'&' | b'-' | b'~' => {
            pat.push('\\');
            pat.push(c as char);
        }
        b'\n' => pat.push_str(r"\n"),
        0x20..=0x7e => pat.push(c as char),
        _ => pat.push_str(&format!(r"\x{c:02X}")),
    }
}

/// Whether `actual` matches the `EXPECTF` `template`. As in run-tests.php,
/// both are trimmed of surrounding whitespace and `\r\n` is folded to `\n`
/// before the anchored match.
pub fn expectf_matches(template: &[u8], actual: &[u8]) -> bool {
    let template = trim_bytes(&fold_crlf(template));
    let actual = trim_bytes(&fold_crlf(actual));
    match expectf_regex(&template) {
        Ok(re) => re.is_match(&actual),
        Err(_) => template == actual,
    }
}

fn fold_crlf(b: &[u8]) -> Vec<u8> {
    super::normalize::replace_bytes(b, b"\r\n", b"\n")
}

fn trim_bytes(b: &[u8]) -> Vec<u8> {
    let start = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(start, |e| e + 1);
    b[start..end.max(start)].to_vec()
}

/// A compact unified diff of `left` vs `right` around the first differing
/// line: three lines of context, then up to seven removed and seven added
/// lines. Non-UTF-8 and control bytes are escaped as `\xNN`.
pub fn unified_diff(left: &[u8], right: &[u8], left_label: &str, right_label: &str) -> String {
    const CONTEXT: usize = 3;
    const MAX_SIDE: usize = 7;

    let mut out = String::new();
    if left == right {
        out.push_str("(identical)\n");
        return out;
    }
    let l: Vec<&[u8]> = left.split_inclusive(|&c| c == b'\n').collect();
    let r: Vec<&[u8]> = right.split_inclusive(|&c| c == b'\n').collect();
    let prefix = l.iter().zip(&r).take_while(|(a, b)| a == b).count();
    let max_suffix = l.len().min(r.len()) - prefix;
    let suffix = l
        .iter()
        .rev()
        .zip(r.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let byte_offset = left.iter().zip(right).position(|(a, b)| a != b).unwrap_or(left.len().min(right.len()));

    out.push_str(&format!("--- {left_label}\n+++ {right_label}\n"));
    out.push_str(&format!(
        "@@ first difference at line {} (byte offset {byte_offset}); {left_label} {} bytes, {right_label} {} bytes @@\n",
        prefix + 1,
        left.len(),
        right.len()
    ));
    for line in &l[prefix.saturating_sub(CONTEXT)..prefix] {
        out.push(' ');
        out.push_str(&render_line(line));
        out.push('\n');
    }
    let removed = &l[prefix..l.len() - suffix];
    let added = &r[prefix..r.len() - suffix];
    push_side(&mut out, '-', removed, MAX_SIDE, left, l.len(), prefix);
    push_side(&mut out, '+', added, MAX_SIDE, right, r.len(), prefix);
    if removed.len() <= MAX_SIDE && added.len() <= MAX_SIDE {
        for line in l[l.len() - suffix..].iter().take(CONTEXT) {
            out.push(' ');
            out.push_str(&render_line(line));
            out.push('\n');
        }
    }
    out
}

fn push_side(
    out: &mut String,
    marker: char,
    lines: &[&[u8]],
    max: usize,
    whole: &[u8],
    total_lines: usize,
    prefix: usize,
) {
    for (k, line) in lines.iter().enumerate() {
        if k == max {
            out.push_str(&format!("{marker}… ({} more lines)\n", lines.len() - max));
            return;
        }
        out.push(marker);
        out.push_str(&render_line(line));
        out.push('\n');
        let is_last_of_whole = prefix + k + 1 == total_lines;
        if is_last_of_whole && !whole.ends_with(b"\n") {
            out.push_str("\\ No newline at end of file\n");
        }
    }
}

fn render_line(line: &[u8]) -> String {
    let bare = line.strip_suffix(b"\n").unwrap_or(line);
    escape_bytes(bare)
}

/// Render bytes for a report: valid UTF-8 passes through, invalid bytes and
/// control characters (other than tab) become `\xNN`.
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                push_escaped_str(&mut out, s);
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                push_escaped_str(&mut out, std::str::from_utf8(&rest[..valid]).unwrap_or(""));
                let bad = e.error_len().unwrap_or(rest.len() - valid);
                for b in &rest[valid..valid + bad] {
                    out.push_str(&format!("\\x{b:02X}"));
                }
                rest = &rest[valid + bad..];
            }
        }
    }
    out
}

fn push_escaped_str(out: &mut String, s: &str) {
    for ch in s.chars() {
        if ch.is_control() && ch != '\t' {
            out.push_str(&format!("\\x{:02X}", ch as u32));
        } else {
            out.push(ch);
        }
    }
}

/// One full snippet run: both results, the verdict and what shaped it.
#[derive(Debug, Clone)]
pub struct SnippetRun {
    /// The stock php result.
    pub php: RunResult,
    /// The rphp result.
    pub rphp: RunResult,
    /// The comparison outcome.
    pub verdict: Verdict,
    /// Allowlist categories that applied to this snippet.
    pub categories: Vec<Category>,
    /// Whether a `.expectf` template was used instead of byte equality.
    pub expectf: bool,
}

/// Run `snippet` under both binaries (each in its own fresh temporary working
/// directory) and compare under the allowlist and any `.expectf` sidecar.
pub fn run_snippet_full(
    php: &Path,
    rphp: &Path,
    snippet: &Path,
    allowlist: &Allowlist,
    timeout: Duration,
) -> std::io::Result<SnippetRun> {
    let script = snippet.canonicalize()?;
    let tmp = tempfile::tempdir()?;
    let php_cwd = tmp.path().join("php");
    let rphp_cwd = tmp.path().join("rphp");
    std::fs::create_dir_all(&php_cwd)?;
    std::fs::create_dir_all(&rphp_cwd)?;

    let php_res = run_php(php, &script, &[], &php_cwd, timeout)?;
    let rphp_res = run_rphp(rphp, &script, &[], &rphp_cwd, timeout)?;

    let name = allowlist.relative_name(snippet);
    let template = match std::fs::read(sidecar(snippet, "expectf")) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let mut policy = Policy::new(&name, allowlist);
    policy.php_ctx = NormalizeContext::for_script(&script, &php_cwd);
    policy.rphp_ctx = NormalizeContext::for_script(&script, &rphp_cwd);
    if let Some(t) = template.as_deref() {
        policy = policy.with_expectf(t);
    }
    let verdict = compare(&php_res, &rphp_res, &policy);
    Ok(SnippetRun {
        php: php_res,
        rphp: rphp_res,
        verdict,
        categories: allowlist.categories_for(&name),
        expectf: template.is_some(),
    })
}

/// Run `snippet` under `rphp` alone, in a fresh temporary working directory
/// (the php-independent smoke path: "does it run and exit as declared?").
pub fn run_rphp_isolated(rphp: &Path, snippet: &Path, timeout: Duration) -> std::io::Result<RunResult> {
    let script = snippet.canonicalize()?;
    let tmp = tempfile::tempdir()?;
    run_rphp(rphp, &script, &[], tmp.path(), timeout)
}

/// Convenience over [`run_snippet_full`] with the default timeout; a harness
/// error (binary missing, unreadable snippet) is reported as an `Exit`
/// mismatch so it is never mistaken for a pass.
pub fn run_snippet(php: &Path, rphp: &Path, snippet: &Path, allowlist: &Allowlist) -> Verdict {
    match run_snippet_full(php, rphp, snippet, allowlist, DEFAULT_TIMEOUT) {
        Ok(run) => run.verdict,
        Err(e) => Verdict::Mismatch { channel: Channel::Exit, first_diff: format!("harness error: {e}") },
    }
}
