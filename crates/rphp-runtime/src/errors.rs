//! The diagnostics channel (ADR-022, php-src `main/main.c: php_error_cb`).
//!
//! [`Interp::emit_error`] is the single path for every non-fatal diagnostic
//! and for fatal error rendering: it honours the user handler stack
//! (`set_error_handler` — the handler may throw, and a `false` return falls
//! through), records `error_get_last()`, applies `error_reporting` and the `@`
//! silence depth, logs (`log_errors`, `PHP Warning:  …` on stderr) and
//! displays (`display_errors`) exactly as php-cli does:
//! `\nWarning: <msg> in <file> on line <N>\n` on **stdout**.

use rphp_value::Value;

use crate::{Interp, PendingThrow, Unwind};

/// `E_ERROR`.
pub const E_ERROR: i64 = 1;
/// `E_WARNING`.
pub const E_WARNING: i64 = 2;
/// `E_PARSE`.
pub const E_PARSE: i64 = 4;
/// `E_NOTICE`.
pub const E_NOTICE: i64 = 8;
/// `E_CORE_ERROR`.
pub const E_CORE_ERROR: i64 = 16;
/// `E_CORE_WARNING`.
pub const E_CORE_WARNING: i64 = 32;
/// `E_COMPILE_ERROR`.
pub const E_COMPILE_ERROR: i64 = 64;
/// `E_COMPILE_WARNING`.
pub const E_COMPILE_WARNING: i64 = 128;
/// `E_USER_ERROR`.
pub const E_USER_ERROR: i64 = 256;
/// `E_USER_WARNING`.
pub const E_USER_WARNING: i64 = 512;
/// `E_USER_NOTICE`.
pub const E_USER_NOTICE: i64 = 1024;
/// `E_STRICT` (still defined in 8.5, deprecated, no longer part of `E_ALL`).
pub const E_STRICT: i64 = 2048;
/// `E_RECOVERABLE_ERROR`.
pub const E_RECOVERABLE_ERROR: i64 = 4096;
/// `E_DEPRECATED`.
pub const E_DEPRECATED: i64 = 8192;
/// `E_USER_DEPRECATED`.
pub const E_USER_DEPRECATED: i64 = 16384;
/// `E_ALL` as PHP 8.5 defines it (every level except `E_STRICT`).
pub const E_ALL: i64 = 30719;
/// What `error_reporting()` reports inside an `@`-silenced expression: the
/// levels `@` cannot silence (fatal ones).
pub const SILENCE_MASK: i64 =
    E_ERROR | E_CORE_ERROR | E_COMPILE_ERROR | E_USER_ERROR | E_RECOVERABLE_ERROR | E_PARSE;

/// A diagnostic level (`E_*`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ErrLevel {
    Error,
    Warning,
    Parse,
    Notice,
    CoreError,
    CoreWarning,
    CompileError,
    CompileWarning,
    UserError,
    UserWarning,
    UserNotice,
    Strict,
    RecoverableError,
    Deprecated,
    UserDeprecated,
}

impl ErrLevel {
    /// The `E_*` value.
    pub const fn code(self) -> i64 {
        match self {
            ErrLevel::Error => E_ERROR,
            ErrLevel::Warning => E_WARNING,
            ErrLevel::Parse => E_PARSE,
            ErrLevel::Notice => E_NOTICE,
            ErrLevel::CoreError => E_CORE_ERROR,
            ErrLevel::CoreWarning => E_CORE_WARNING,
            ErrLevel::CompileError => E_COMPILE_ERROR,
            ErrLevel::CompileWarning => E_COMPILE_WARNING,
            ErrLevel::UserError => E_USER_ERROR,
            ErrLevel::UserWarning => E_USER_WARNING,
            ErrLevel::UserNotice => E_USER_NOTICE,
            ErrLevel::Strict => E_STRICT,
            ErrLevel::RecoverableError => E_RECOVERABLE_ERROR,
            ErrLevel::Deprecated => E_DEPRECATED,
            ErrLevel::UserDeprecated => E_USER_DEPRECATED,
        }
    }

    /// The level for an `E_*` value.
    pub fn from_code(code: i64) -> Option<ErrLevel> {
        Some(match code {
            E_ERROR => ErrLevel::Error,
            E_WARNING => ErrLevel::Warning,
            E_PARSE => ErrLevel::Parse,
            E_NOTICE => ErrLevel::Notice,
            E_CORE_ERROR => ErrLevel::CoreError,
            E_CORE_WARNING => ErrLevel::CoreWarning,
            E_COMPILE_ERROR => ErrLevel::CompileError,
            E_COMPILE_WARNING => ErrLevel::CompileWarning,
            E_USER_ERROR => ErrLevel::UserError,
            E_USER_WARNING => ErrLevel::UserWarning,
            E_USER_NOTICE => ErrLevel::UserNotice,
            E_STRICT => ErrLevel::Strict,
            E_RECOVERABLE_ERROR => ErrLevel::RecoverableError,
            E_DEPRECATED => ErrLevel::Deprecated,
            E_USER_DEPRECATED => ErrLevel::UserDeprecated,
            _ => return None,
        })
    }

    /// The label php prints (`Warning`, `Fatal error`, …).
    pub const fn label(self) -> &'static str {
        match self {
            ErrLevel::Error
            | ErrLevel::CoreError
            | ErrLevel::CompileError
            | ErrLevel::UserError
            | ErrLevel::RecoverableError => "Fatal error",
            ErrLevel::Warning
            | ErrLevel::CoreWarning
            | ErrLevel::CompileWarning
            | ErrLevel::UserWarning => "Warning",
            ErrLevel::Parse => "Parse error",
            ErrLevel::Notice | ErrLevel::UserNotice => "Notice",
            ErrLevel::Strict => "Strict Standards",
            ErrLevel::Deprecated | ErrLevel::UserDeprecated => "Deprecated",
        }
    }

    /// Whether the level ends the request (exit 255) after display.
    pub const fn is_fatal(self) -> bool {
        matches!(
            self,
            ErrLevel::Error
                | ErrLevel::CoreError
                | ErrLevel::CompileError
                | ErrLevel::UserError
                | ErrLevel::RecoverableError
                | ErrLevel::Parse
        )
    }

    /// Whether a `set_error_handler` callback may receive this level
    /// (php excludes `E_ERROR`, `E_PARSE`, `E_CORE_*`, `E_COMPILE_*`).
    pub const fn user_handleable(self) -> bool {
        !matches!(
            self,
            ErrLevel::Error
                | ErrLevel::Parse
                | ErrLevel::CoreError
                | ErrLevel::CoreWarning
                | ErrLevel::CompileError
                | ErrLevel::CompileWarning
        )
    }
}

/// The `error_get_last()` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastError {
    /// The `E_*` value.
    pub kind: i64,
    pub message: String,
    pub file: String,
    pub line: u32,
}

/// Where `display_errors` sends diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DisplayMode {
    Off,
    /// Through the output layer (stdout on the CLI).
    Stdout,
    /// Directly to stderr (`display_errors=stderr`).
    Stderr,
}

impl DisplayMode {
    /// Parse the ini value (`1`/`On`/`stdout` ⇒ stdout, `stderr` ⇒ stderr).
    pub fn parse(v: &str) -> DisplayMode {
        let t = v.trim();
        if t.eq_ignore_ascii_case("stderr") {
            DisplayMode::Stderr
        } else if t.eq_ignore_ascii_case("stdout") || crate::ini::parse_bool(t) {
            DisplayMode::Stdout
        } else {
            DisplayMode::Off
        }
    }
}

impl Interp {
    /// php's `php_log_err`: the `error_log` ini file when set (a
    /// `[20-Sep-2026 17:03:37 UTC] ` stamp in `date.timezone` ahead), else
    /// the SAPI's logger (the built-in server's log, FastCGI's stderr
    /// stream), else the process's stderr.
    pub fn log_message(&mut self, entry: &str) {
        if let Some(path) = self.ini.get("error_log").filter(|p| !p.is_empty()).map(str::to_string) {
            let zone = self.ini.get("date.timezone").filter(|z| !z.is_empty()).unwrap_or("UTC").to_string();
            let stamp = match jiff::tz::TimeZone::get(&zone) {
                Ok(tz) => jiff::Timestamp::now().to_zoned(tz).strftime("%d-%b-%Y %H:%M:%S").to_string(),
                Err(_) => jiff::Timestamp::now().strftime("%d-%b-%Y %H:%M:%S").to_string(),
            };
            let line = format!("[{stamp} {zone}] {entry}\n");
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                use std::io::Write;
                let _ = f.write_all(line.as_bytes());
                return;
            }
        }
        match &mut self.error_log {
            Some(log) => log(entry),
            None => eprintln!("{entry}"),
        }
    }
}

impl Interp {
    /// The `error_reporting` mask in effect: the configured mask, reduced to
    /// [`SILENCE_MASK`] inside an `@` bracket.
    pub fn effective_error_reporting(&self) -> i64 {
        if self.silence > 0 {
            self.error_reporting & SILENCE_MASK
        } else {
            self.error_reporting
        }
    }

    /// The active user error handler (`set_error_handler`) and its level mask.
    pub fn current_error_handler(&self) -> Option<(Value, i64)> {
        match self.error_handler.last() {
            Some((h, mask)) if !matches!(h, Value::Null) => Some((h.clone(), *mask)),
            _ => None,
        }
    }

    /// Emit a diagnostic at the current file/line (see the module docs).
    /// `Err` when a user handler throws, or when the level is fatal
    /// (`Unwind::Exit(255)` after display).
    pub fn emit_error(&mut self, level: ErrLevel, message: &str) -> Result<(), Unwind> {
        if self.light_native {
            return Err(Unwind::Retry);
        }
        let file = self.current_file().to_string();
        let line = self.current_line();
        self.emit_error_at(level, message, &file, line)
    }

    /// [`Interp::emit_error`] with an explicit location.
    pub fn emit_error_at(
        &mut self,
        level: ErrLevel,
        message: &str,
        file: &str,
        line: u32,
    ) -> Result<(), Unwind> {
        if self.light_native {
            return Err(Unwind::Retry);
        }
        self.emit_error_full(level, message, file, line, true)
    }

    /// [`Interp::emit_error_at`]; `with_trace = false` never appends the
    /// `fatal_error_backtraces` block (an uncaught exception carries its
    /// own `Stack trace:` inside the message, or none when a user
    /// `__toString` produced it).
    pub fn emit_error_full(
        &mut self,
        level: ErrLevel,
        message: &str,
        file: &str,
        line: u32,
        with_trace: bool,
    ) -> Result<(), Unwind> {
        // 1. The user handler — called regardless of error_reporting / `@`
        //    (php ≥ 8.0); it sees the masked value through error_reporting().
        if level.user_handleable() && !self.in_error_handler {
            if let Some((handler, mask)) = self.current_error_handler() {
                if mask & level.code() != 0 {
                    // Errors raised inside the handler go to the standard path.
                    self.in_error_handler = true;
                    let r = self.call_value(
                        &handler,
                        &[
                            Value::Int(level.code()),
                            Value::string(message.as_bytes()),
                            Value::string(file.as_bytes()),
                            Value::Int(line as i64),
                        ],
                    );
                    self.in_error_handler = false;
                    match r {
                        Err(u) => return Err(u),
                        Ok(v) if matches!(*v.deref(), Value::Bool(false)) => {} // fall through
                        Ok(_) => return Ok(()),
                    }
                }
            }
        }

        // 2. The standard path: record, then log and display if reported.
        self.last_error = Some(LastError {
            kind: level.code(),
            message: message.to_string(),
            file: file.to_string(),
            line,
        });
        let reported = self.effective_error_reporting() & level.code() != 0;
        if reported {
            // php ≥ 8.5 (`fatal_error_backtraces=1`) appends a backtrace to a
            // fatal error after the location; an uncaught-exception message
            // already carries its own `Stack trace:` block.
            let trace = if with_trace
                && level.is_fatal()
                && self.ini.bool("fatal_error_backtraces")
                && !message.contains("Stack trace:")
            {
                format!("\nStack trace:\n{}", self.render_trace())
            } else {
                String::new()
            };
            let label = level.label();
            if self.ini.bool("log_errors") {
                let entry = format!("PHP {label}:  {message} in {file} on line {line}{trace}");
                self.log_message(&entry);
            }
            let prepend = self
                .ini
                .get("error_prepend_string")
                .unwrap_or("")
                .to_string();
            let append = self
                .ini
                .get("error_append_string")
                .unwrap_or("")
                .to_string();
            // `html_errors=1` (every SAPI but the CLI) wraps the label and
            // the location in `<b>`; the message itself is not escaped.
            let text = if self.ini.bool("html_errors") {
                // php escapes the message of a fatal or parse error only.
                let message = if matches!(level, ErrLevel::Error | ErrLevel::Parse) {
                    html_escape(message)
                } else {
                    message.to_string()
                };
                format!(
                    "{prepend}<br />\n<b>{label}</b>:  {message} in <b>{file}</b> on line <b>{line}</b>{trace}<br />\n{append}"
                )
            } else {
                format!("{prepend}\n{label}: {message} in {file} on line {line}{trace}\n{append}")
            };
            match DisplayMode::parse(self.ini.get("display_errors").unwrap_or("")) {
                DisplayMode::Stdout => self.echo(text.as_bytes()),
                DisplayMode::Stderr => eprint!("{text}"),
                DisplayMode::Off => {}
            }
        }
        if level.is_fatal() {
            return Err(Unwind::Exit(255));
        }
        Ok(())
    }

    /// Emit a fatal error (`Fatal error: <msg> in <file> on line <N>` plus
    /// the backtrace) and return the unwind that ends the request.
    pub fn fatal(&mut self, message: &str) -> Unwind {
        match self.emit_error(ErrLevel::Error, message) {
            Err(u) => u,
            Ok(()) => Unwind::Exit(255),
        }
    }

    /// [`Interp::fatal`] at an explicit location.
    pub fn fatal_at(&mut self, message: &str, file: &str, line: u32) -> Unwind {
        match self.emit_error_at(ErrLevel::Error, message, file, line) {
            Err(u) => u,
            Ok(()) => Unwind::Exit(255),
        }
    }

    /// A parse error in a unit compiled on demand (`include`, `eval`).
    ///
    /// php throws a **catchable** `ParseError` here — `try { include $f; }
    /// catch (ParseError $e)` works, and so does the `eval` form — so this
    /// records the error for `error_get_last()` and then throws, rather than
    /// rendering `Parse error:` and exiting. An uncaught one renders through
    /// the ordinary uncaught-throwable path.
    pub fn parse_error(&mut self, message: &str, file: &str, line: u32) -> Unwind {
        self.last_error = Some(LastError {
            kind: E_PARSE,
            message: message.to_string(),
            file: file.to_string(),
            line,
        });
        self.throw_at(crate::registry::ErrorKind::Exception("ParseError"), message, file, line)
    }


    /// Render an uncaught fault exactly like php-cli:
    /// `Fatal error: Uncaught <Class>: <msg> in <file>:<line>\nStack trace:\n
    /// #0 {main}\n  thrown in <file> on line <line>` (through the standard
    /// display path, so `error_get_last()` records it and `log_errors`
    /// duplicates it on stderr).
    pub fn render_uncaught(&mut self, p: &PendingThrow) {
        let (file, line, trace) = match &p.site {
            Some(site) => (site.file.clone(), site.line, self.trace_to_string(&site.trace)),
            None => (self.current_file(), self.current_line(), self.render_trace()),
        };
        // php reads an uncaught TypeError's `, called in X on line N` as a
        // sentence and finishes it: `… and defined in <file>:<line>`.
        let class = p.kind.class_name();
        let text = if class == "TypeError" && p.message.contains(", called in ") {
            format!("{} and defined", p.message)
        } else {
            p.message.clone()
        };
        let message = format!(
            "Uncaught {class}: {text} in {file}:{line}\nStack trace:\n{trace}\n  thrown"
        );
        let _ = self.emit_error_at(ErrLevel::Error, &message, &file, line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_labels_and_codes_round_trip() {
        for level in [
            ErrLevel::Error,
            ErrLevel::Warning,
            ErrLevel::Notice,
            ErrLevel::Deprecated,
            ErrLevel::UserError,
            ErrLevel::UserDeprecated,
            ErrLevel::Strict,
        ] {
            assert_eq!(ErrLevel::from_code(level.code()), Some(level));
        }
        assert_eq!(ErrLevel::Warning.label(), "Warning");
        assert_eq!(ErrLevel::UserError.label(), "Fatal error");
        assert_eq!(ErrLevel::Deprecated.label(), "Deprecated");
        assert!(ErrLevel::UserError.is_fatal());
        assert!(ErrLevel::UserError.user_handleable());
        assert!(!ErrLevel::Error.user_handleable());
        assert_eq!(E_ALL, 30719);
        assert_eq!(SILENCE_MASK, 4437);
        assert_eq!(DisplayMode::parse("stderr"), DisplayMode::Stderr);
        assert_eq!(DisplayMode::parse("On"), DisplayMode::Stdout);
        assert_eq!(DisplayMode::parse("0"), DisplayMode::Off);
    }
}

/// `htmlspecialchars` with `ENT_QUOTES`, for `html_errors`.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            c => out.push(c),
        }
    }
    out
}
