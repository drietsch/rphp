//! Aggregation and reporting: per-slice counts, failure lists with the first
//! differing line, text/markdown/JSON renderings, and the ratcheting
//! [`Baseline`] stored in `tests/phpt/enabled/<slice>.toml`.

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::run::{Outcome, TestResult};

/// Outcome tallies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Every test seen.
    pub total: usize,
    /// PASS.
    pub pass: usize,
    /// FAIL.
    pub fail: usize,
    /// SKIP.
    pub skip: usize,
    /// XFAIL (expected failures).
    pub xfail: usize,
    /// XPASS (unexpected passes).
    pub xpass: usize,
    /// BORK.
    pub bork: usize,
    /// TIMEOUT.
    pub timeout: usize,
    /// CRASH.
    pub crash: usize,
}

impl Counts {
    /// Tally one outcome.
    pub fn add(&mut self, o: &Outcome) {
        self.total += 1;
        match o {
            Outcome::Pass => self.pass += 1,
            Outcome::Fail { .. } => self.fail += 1,
            Outcome::Skip { .. } => self.skip += 1,
            Outcome::XFail { .. } => self.xfail += 1,
            Outcome::XPass => self.xpass += 1,
            Outcome::Borked { .. } => self.bork += 1,
            Outcome::Timeout => self.timeout += 1,
            Outcome::Crash { .. } => self.crash += 1,
        }
    }

    /// Tests that actually ran (total minus skipped).
    pub fn ran(&self) -> usize {
        self.total.saturating_sub(self.skip)
    }

    /// PASS as a percentage of the tests that ran.
    pub fn pass_pct(&self) -> f64 {
        if self.ran() == 0 {
            0.0
        } else {
            100.0 * self.pass as f64 / self.ran() as f64
        }
    }
}

/// One non-passing test in a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// Path relative to the report root.
    pub path: String,
    /// The `--TEST--` text.
    pub name: String,
    /// `FAIL`, `BORK`, `TIMEOUT`, `CRASH`, `XPASS`, `XFAIL`.
    pub label: String,
    /// First differing line (FAIL/XFAIL only).
    pub first_line: Option<usize>,
    /// One-line explanation (diff summary, bork/skip reason, signal).
    pub summary: String,
    /// The rendered diff excerpt, when there is one.
    #[serde(default)]
    pub diff: String,
}

/// The report for one slice (extension directory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliceReport {
    /// Slice name (`ctype`, `standard-strings`, …).
    pub slice: String,
    /// The directory that was run.
    pub dir: String,
    /// Engine label.
    pub engine: String,
    /// Tallies.
    pub counts: Counts,
    /// Everything a human should look at (FAIL/BORK/TIMEOUT/CRASH/XPASS).
    pub failures: Vec<Failure>,
    /// Expected failures, for completeness.
    pub xfails: Vec<Failure>,
    /// Skipped tests with their reasons.
    pub skips: Vec<Failure>,
    /// Wall time of the whole slice.
    pub elapsed_ms: u64,
    /// How many results came from the cache.
    pub cached: usize,
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

impl SliceReport {
    /// Build a report from results; paths are shown relative to `root`.
    pub fn from_results(
        slice: &str,
        dir: &Path,
        engine: &str,
        results: &[TestResult],
        root: &Path,
        elapsed_ms: u64,
    ) -> Self {
        let mut counts = Counts::default();
        let mut failures = Vec::new();
        let mut xfails = Vec::new();
        let mut skips = Vec::new();
        let mut cached = 0;
        for r in results {
            counts.add(&r.outcome);
            if r.cached {
                cached += 1;
            }
            let mut entry = Failure {
                path: rel(&r.path, root),
                name: r.name.clone(),
                label: r.outcome.label().to_string(),
                first_line: None,
                summary: String::new(),
                diff: String::new(),
            };
            match &r.outcome {
                Outcome::Pass => continue,
                Outcome::Fail { diff } => {
                    entry.first_line = Some(diff.line);
                    entry.summary = diff.summary();
                    entry.diff = diff.text.clone();
                    failures.push(entry);
                }
                Outcome::XFail { reason, diff } => {
                    entry.first_line = Some(diff.line);
                    entry.summary = format!("{reason} — {}", diff.summary());
                    entry.diff = diff.text.clone();
                    xfails.push(entry);
                }
                Outcome::XPass => {
                    entry.summary = "XFAIL section but test passes".into();
                    failures.push(entry);
                }
                Outcome::Borked { reason } => {
                    entry.summary = reason.clone();
                    failures.push(entry);
                }
                Outcome::Timeout => {
                    entry.summary = "process timed out".into();
                    failures.push(entry);
                }
                Outcome::Crash { signal, exit } => {
                    entry.summary = match (signal, exit) {
                        (Some(s), _) => format!("killed by signal {s}"),
                        (None, Some(c)) => format!("exit code {c}"),
                        (None, None) => "crashed".into(),
                    };
                    failures.push(entry);
                }
                Outcome::Skip { reason } => {
                    entry.summary = reason.clone();
                    skips.push(entry);
                }
            }
        }
        SliceReport {
            slice: slice.to_string(),
            dir: rel(dir, root),
            engine: engine.to_string(),
            counts,
            failures,
            xfails,
            skips,
            elapsed_ms,
            cached,
        }
    }

    /// One summary line: `ctype: 45/49 pass (91.8%) …`.
    pub fn summary_line(&self) -> String {
        let c = &self.counts;
        format!(
            "{:<20} {:>5}/{:<5} pass ({:5.1}%)  fail {:<4} skip {:<4} xfail {:<3} xpass {:<3} bork {:<3} timeout {:<3} crash {:<3} [{:.1}s, {} cached]",
            self.slice,
            c.pass,
            c.ran(),
            c.pass_pct(),
            c.fail,
            c.skip,
            c.xfail,
            c.xpass,
            c.bork,
            c.timeout,
            c.crash,
            self.elapsed_ms as f64 / 1000.0,
            self.cached
        )
    }

    /// Plain-text rendering (summary plus failures with their first diff line).
    pub fn to_text(&self, list_failures: bool) -> String {
        let mut s = self.summary_line();
        s.push('\n');
        if list_failures {
            for f in &self.failures {
                let _ = writeln!(s, "  {:<7} {} — {}", f.label, f.path, f.summary);
            }
        }
        s
    }

    /// Markdown rendering for `target/phpt-report/<slice>.md`.
    pub fn to_markdown(&self) -> String {
        let c = &self.counts;
        let mut s = String::new();
        let _ = writeln!(s, "# phpt: {}\n", self.slice);
        let _ = writeln!(s, "- directory: `{}`", self.dir);
        let _ = writeln!(s, "- engine: `{}`", self.engine);
        let _ = writeln!(
            s,
            "- elapsed: {:.1}s ({} cached results)\n",
            self.elapsed_ms as f64 / 1000.0,
            self.cached
        );
        let _ = writeln!(
            s,
            "| total | ran | pass | pass % | fail | skip | xfail | xpass | bork | timeout | crash |"
        );
        let _ = writeln!(
            s,
            "|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
        );
        let _ = writeln!(
            s,
            "| {} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} | {} |\n",
            c.total,
            c.ran(),
            c.pass,
            c.pass_pct(),
            c.fail,
            c.skip,
            c.xfail,
            c.xpass,
            c.bork,
            c.timeout,
            c.crash
        );
        if !self.failures.is_empty() {
            let _ = writeln!(s, "## Failures ({})\n", self.failures.len());
            let _ = writeln!(s, "| result | test | name | first diff |");
            let _ = writeln!(s, "|---|---|---|---|");
            for f in &self.failures {
                let _ = writeln!(
                    s,
                    "| {} | `{}` | {} | {} |",
                    f.label,
                    f.path,
                    md_cell(&f.name),
                    md_cell(&f.summary)
                );
            }
            s.push('\n');
        }
        if !self.xfails.is_empty() {
            let _ = writeln!(s, "## Expected failures ({})\n", self.xfails.len());
            for f in &self.xfails {
                let _ = writeln!(s, "- `{}` — {}", f.path, md_cell(&f.summary));
            }
            s.push('\n');
        }
        if !self.skips.is_empty() {
            let _ = writeln!(s, "## Skipped ({})\n", self.skips.len());
            for f in &self.skips {
                let _ = writeln!(s, "- `{}` — {}", f.path, md_cell(&f.summary));
            }
            s.push('\n');
        }
        s
    }

    /// JSON rendering.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// A one-line-per-slice table.
pub fn summary_table(reports: &[SliceReport]) -> String {
    let mut s = String::new();
    let mut total = Counts::default();
    for r in reports {
        s.push_str(&r.summary_line());
        s.push('\n');
        let c = &r.counts;
        total.total += c.total;
        total.pass += c.pass;
        total.fail += c.fail;
        total.skip += c.skip;
        total.xfail += c.xfail;
        total.xpass += c.xpass;
        total.bork += c.bork;
        total.timeout += c.timeout;
        total.crash += c.crash;
    }
    if reports.len() > 1 {
        let _ = writeln!(
            s,
            "{:<20} {:>5}/{:<5} pass ({:5.1}%)  fail {:<4} skip {:<4} xfail {:<3} xpass {:<3} bork {:<3} timeout {:<3} crash {:<3}",
            "TOTAL",
            total.pass,
            total.ran(),
            total.pass_pct(),
            total.fail,
            total.skip,
            total.xfail,
            total.xpass,
            total.bork,
            total.timeout,
            total.crash
        );
    }
    s
}

fn default_true() -> bool {
    true
}

/// `tests/phpt/enabled/<slice>.toml`: a scored directory and the pass count
/// it must never drop below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    /// Directory relative to the php-src checkout (`ext/ctype/tests`).
    pub dir: String,
    /// Gate this slice at all?
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Minimum PASS count; only ever raised.
    #[serde(default)]
    pub baseline: usize,
    /// Number of tests when the baseline was last written (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    /// Date of the last ratchet (`YYYY-MM-DD`, informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// Free text: what blocks the slice, what is deliberately skipped.
    #[serde(default)]
    pub notes: String,
}

/// The result of [`Baseline::ratchet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ratchet {
    /// The pass count went up; the file should be saved.
    Raised {
        /// Old baseline.
        from: usize,
        /// New baseline.
        to: usize,
    },
    /// Same pass count as before.
    Unchanged,
    /// The pass count is below the baseline; nothing was changed.
    Refused {
        /// The recorded baseline.
        baseline: usize,
        /// The observed pass count.
        actual: usize,
    },
}

impl Baseline {
    /// Read a baseline file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }

    /// Write a baseline file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Raise the baseline to `pass` (never lower it).
    pub fn ratchet(&mut self, pass: usize, total: usize) -> Ratchet {
        if pass > self.baseline {
            let from = self.baseline;
            self.baseline = pass;
            self.total = Some(total);
            self.updated = Some(today_utc());
            Ratchet::Raised { from, to: pass }
        } else if pass == self.baseline {
            Ratchet::Unchanged
        } else {
            Ratchet::Refused {
                baseline: self.baseline,
                actual: pass,
            }
        }
    }

    /// `Ok` when `pass` meets the baseline, else the gate message.
    pub fn gate(&self, slice: &str, pass: usize) -> Result<(), String> {
        if !self.enabled || pass >= self.baseline {
            Ok(())
        } else {
            Err(format!(
                "{slice}: {pass} passing tests is below the baseline of {}",
                self.baseline
            ))
        }
    }
}

/// Load every `<name>.toml` in a directory, sorted by name.
pub fn load_enabled(dir: &Path) -> Result<Vec<(String, Baseline)>, String> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push((name, Baseline::load(&path)?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Today's UTC date as `YYYY-MM-DD` (civil-from-days, no chrono needed).
pub fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
