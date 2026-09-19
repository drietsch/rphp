//! The helper surface natives use through [`Ctx`](crate::Ctx) (which derefs
//! to [`Interp`]): output, diagnostics, calling back into PHP, ini,
//! constants, resources, and location queries.

use rphp_value::Value;

use crate::errors::ErrLevel;
use crate::frame::Frame;
use crate::output::{
    ObLevel, PHP_OUTPUT_HANDLER_CLEAN, PHP_OUTPUT_HANDLER_FINAL, PHP_OUTPUT_HANDLER_FLUSH,
    PHP_OUTPUT_HANDLER_START,
};
use crate::registry::{ErrorKind, Unwind};
use crate::Interp;

impl Interp {
    // ---- output ----------------------------------------------------------

    /// The buffer a native appends output to: the top `ob_*` level, or the
    /// level-0 staging buffer that is flushed to the sink when the native
    /// returns.
    pub fn out(&mut self) -> &mut Vec<u8> {
        self.out.buf()
    }

    /// `echo`: to the top `ob_*` level, or straight to the sink.
    pub fn echo(&mut self, bytes: &[u8]) {
        self.out.write(bytes);
    }

    /// `echo $value` (the string conversion is the caller's job for arrays /
    /// objects, which warn / throw).
    pub fn echo_value(&mut self, v: &Value) {
        v.append_php_bytes(self.out.buf());
        self.out.flush_pending();
    }

    // ---- diagnostics -----------------------------------------------------

    /// `E_WARNING`.
    pub fn warn(&mut self, message: &str) -> Result<(), Unwind> {
        self.emit_error(ErrLevel::Warning, message)
    }

    /// `E_NOTICE`.
    pub fn notice(&mut self, message: &str) -> Result<(), Unwind> {
        self.emit_error(ErrLevel::Notice, message)
    }

    /// `E_DEPRECATED`.
    pub fn deprecated(&mut self, message: &str) -> Result<(), Unwind> {
        self.emit_error(ErrLevel::Deprecated, message)
    }

    /// Build the unwind for an engine fault of `kind` (`throw new Kind(msg)`).
    pub fn throw(&self, kind: ErrorKind, message: impl Into<String>) -> Unwind {
        Unwind::Pending(crate::PendingThrow {
            kind,
            message: message.into(),
            site: None,
        })
    }

    // ---- ini ---------------------------------------------------------------

    /// `ini_get`.
    pub fn ini_get(&self, name: &str) -> Option<&str> {
        self.ini.get(name)
    }

    /// `ini_set`: returns the previous value, `None` for an unknown directive.
    /// Directives with engine side effects (`error_reporting`) are applied.
    pub fn ini_set(&mut self, name: &str, value: &str) -> Option<String> {
        let old = self.ini.set(name, value)?;
        if name == "error_reporting" {
            self.error_reporting = parse_error_reporting(value).unwrap_or(0);
        }
        Some(old)
    }

    /// `error_reporting($level)`: set the mask (and mirror it into the ini
    /// entry), returning the previous mask.
    pub fn set_error_reporting(&mut self, level: i64) -> i64 {
        let old = self.error_reporting;
        self.error_reporting = level;
        self.ini.set("error_reporting", &level.to_string());
        old
    }

    // ---- constants -------------------------------------------------------------

    /// `constant($name)` lookup (case-sensitive).
    pub fn constant(&self, name: &[u8]) -> Option<Value> {
        self.constants.get(name).cloned()
    }

    /// `defined($name)`.
    pub fn defined(&self, name: &[u8]) -> bool {
        self.constants.contains_key(name)
    }

    /// `define($name, $value)`: `false` (and unchanged) when already defined.
    pub fn define(&mut self, name: &[u8], value: Value) -> bool {
        if self.constants.contains_key(name) {
            return false;
        }
        self.constants.insert(Box::from(name), value);
        self.user_constants.push(Box::from(name));
        true
    }

    /// The names `define()` added, in definition order — php's `user`
    /// category in `get_defined_constants(true)`.
    pub fn user_constant_names(&self) -> &[Box<[u8]>] {
        &self.user_constants
    }

    /// Every defined constant (`get_defined_constants()`), unordered.
    pub fn constants(&self) -> impl Iterator<Item = (&[u8], &Value)> {
        self.constants.iter().map(|(k, v)| (&**k, v))
    }

    // ---- resources -------------------------------------------------------------

    /// Allocate a resource of `kind` wrapping `payload`.
    pub fn resource_add(&mut self, kind: &'static str, payload: Box<dyn std::any::Any>) -> Value {
        self.resources.add(kind, payload)
    }

    /// The live resource with this id.
    pub fn resource_get(&self, id: u32) -> Option<rphp_value::Resource> {
        self.resources.get(id)
    }

    /// Close a resource value; `false` if it was not live.
    pub fn resource_close(&mut self, v: &Value) -> bool {
        self.resources.close_value(v)
    }

    // ---- location ----------------------------------------------------------------

    /// The source line of the innermost user frame's current op (0 when the
    /// function carries no line table).
    pub fn current_line(&self) -> u32 {
        self.current_user_frame().map_or(0, |f| self.frame_line(f))
    }

    /// The file name diagnostics print for the running unit: the innermost
    /// user frame's file, else the script name.
    pub fn current_file(&self) -> String {
        match self.current_user_frame() {
            Some(f) => self.frame_file(f),
            None => self.script_name.clone(),
        }
    }

    /// The script's `$argv`.
    pub fn argv(&self) -> &[Vec<u8>] {
        &self.argv
    }

    // ---- output buffering (ob_*) -------------------------------------------------

    /// `ob_start($callback, $chunk_size, $flags)`.
    pub fn ob_start(&mut self, callback: Option<Value>, chunk_size: usize, flags: i64) {
        self.out.push(callback, chunk_size, flags);
    }

    /// Run a level's handler for `phase` (adding `START` on its first call)
    /// and return the bytes to pass on: the handler's return, or the raw
    /// buffer when there is no handler / it returned `false`.
    fn ob_run_handler(&mut self, level: &mut ObLevel, phase: i64) -> Result<Vec<u8>, Unwind> {
        let buf = std::mem::take(&mut level.buf);
        let Some(cb) = level.callback.clone() else {
            return Ok(buf);
        };
        let mut phase = phase;
        if !level.started {
            phase |= PHP_OUTPUT_HANDLER_START;
            level.started = true;
        }
        let base = self.frames.len();
        let silence = self.silence;
        self.frames.push(Frame::internal(silence));
        let r = self.call_value(&cb, &[Value::string(&buf), Value::Int(phase)]);
        self.frames.truncate(base);
        match r? {
            Value::Bool(false) => Ok(buf),
            v => Ok(v.to_php_bytes()),
        }
    }

    /// Discard the top level (`ob_end_clean` / `ob_get_clean`): the handler
    /// sees `CLEAN|FINAL`, its output is dropped. Returns the raw contents,
    /// or `None` when no level is active.
    pub fn ob_discard_top(&mut self) -> Result<Option<Vec<u8>>, Unwind> {
        let Some(mut level) = self.out.pop() else {
            return Ok(None);
        };
        let raw = level.buf.clone();
        let _ = self.ob_run_handler(
            &mut level,
            PHP_OUTPUT_HANDLER_CLEAN | PHP_OUTPUT_HANDLER_FINAL,
        )?;
        Ok(Some(raw))
    }

    /// Clear the top level's buffer keeping the level (`ob_clean`): the
    /// handler sees `CLEAN`, its output is dropped. `None` when no level is
    /// active.
    pub fn ob_clean_top(&mut self) -> Result<Option<()>, Unwind> {
        let Some(mut level) = self.out.pop() else {
            return Ok(None);
        };
        let r = self.ob_run_handler(&mut level, PHP_OUTPUT_HANDLER_CLEAN);
        let started = level.started;
        self.out.push(level.callback, level.chunk_size, level.flags);
        if let Some(top) = self.out.top_mut() {
            top.started = started;
        }
        r.map(|_| Some(()))
    }

    /// Flush the top level into the level below (or the sink): with `end`
    /// the level is removed (`ob_end_flush` / `ob_get_flush`, handler phase
    /// `FINAL`), otherwise it stays with an empty buffer (`ob_flush`, phase
    /// `FLUSH`). Returns the raw contents, or `None` when no level is active.
    pub fn ob_flush_top(&mut self, end: bool) -> Result<Option<Vec<u8>>, Unwind> {
        let Some(mut level) = self.out.pop() else {
            return Ok(None);
        };
        let raw = level.buf.clone();
        let phase = if end {
            PHP_OUTPUT_HANDLER_FINAL
        } else {
            PHP_OUTPUT_HANDLER_FLUSH
        };
        let processed = match self.ob_run_handler(&mut level, phase) {
            Ok(p) => p,
            Err(u) => {
                if !end {
                    self.out.push(level.callback, level.chunk_size, level.flags);
                }
                return Err(u);
            }
        };
        self.out.write(&processed);
        if !end {
            let started = level.started;
            self.out.push(level.callback, level.chunk_size, level.flags);
            if let Some(top) = self.out.top_mut() {
                top.started = started;
            }
        }
        Ok(Some(raw))
    }
}

/// Parse an `error_reporting` ini value: an integer, or an expression over
/// `E_*` names with `|`, `&`, `^`, `~` and parentheses (`E_ALL & ~E_NOTICE`).
pub fn parse_error_reporting(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(crate::errors::E_ALL);
    }
    if let Ok(n) = t.parse::<i64>() {
        return Some(n);
    }
    let tokens: Vec<String> = tokenize(t)?;
    let mut pos = 0;
    let v = parse_or(&tokens, &mut pos)?;
    (pos == tokens.len()).then_some(v)
}

fn tokenize(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else if "|&^~()".contains(c) {
            out.push(c.to_string());
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

fn parse_or(t: &[String], pos: &mut usize) -> Option<i64> {
    let mut v = parse_xor(t, pos)?;
    while t.get(*pos).map(String::as_str) == Some("|") {
        *pos += 1;
        v |= parse_xor(t, pos)?;
    }
    Some(v)
}

fn parse_xor(t: &[String], pos: &mut usize) -> Option<i64> {
    let mut v = parse_and(t, pos)?;
    while t.get(*pos).map(String::as_str) == Some("^") {
        *pos += 1;
        v ^= parse_and(t, pos)?;
    }
    Some(v)
}

fn parse_and(t: &[String], pos: &mut usize) -> Option<i64> {
    let mut v = parse_unary(t, pos)?;
    while t.get(*pos).map(String::as_str) == Some("&") {
        *pos += 1;
        v &= parse_unary(t, pos)?;
    }
    Some(v)
}

fn parse_unary(t: &[String], pos: &mut usize) -> Option<i64> {
    match t.get(*pos).map(String::as_str)? {
        "~" => {
            *pos += 1;
            Some(!parse_unary(t, pos)?)
        }
        "(" => {
            *pos += 1;
            let v = parse_or(t, pos)?;
            (t.get(*pos).map(String::as_str) == Some(")")).then(|| {
                *pos += 1;
                v
            })
        }
        tok => {
            *pos += 1;
            if let Ok(n) = tok.parse::<i64>() {
                return Some(n);
            }
            use crate::errors::*;
            Some(match tok {
                "E_ERROR" => E_ERROR,
                "E_WARNING" => E_WARNING,
                "E_PARSE" => E_PARSE,
                "E_NOTICE" => E_NOTICE,
                "E_CORE_ERROR" => E_CORE_ERROR,
                "E_CORE_WARNING" => E_CORE_WARNING,
                "E_COMPILE_ERROR" => E_COMPILE_ERROR,
                "E_COMPILE_WARNING" => E_COMPILE_WARNING,
                "E_USER_ERROR" => E_USER_ERROR,
                "E_USER_WARNING" => E_USER_WARNING,
                "E_USER_NOTICE" => E_USER_NOTICE,
                "E_STRICT" => E_STRICT,
                "E_RECOVERABLE_ERROR" => E_RECOVERABLE_ERROR,
                "E_DEPRECATED" => E_DEPRECATED,
                "E_USER_DEPRECATED" => E_USER_DEPRECATED,
                "E_ALL" => E_ALL,
                _ => return None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_reporting_expressions() {
        assert_eq!(parse_error_reporting("30719"), Some(30719));
        assert_eq!(parse_error_reporting("E_ALL"), Some(30719));
        assert_eq!(parse_error_reporting("E_ALL & ~E_NOTICE"), Some(30719 & !8));
        assert_eq!(parse_error_reporting("(E_ERROR | E_WARNING)"), Some(3));
        assert_eq!(
            parse_error_reporting("E_ALL & ~(E_NOTICE | E_DEPRECATED)"),
            Some(30719 & !(8 | 8192))
        );
        assert_eq!(parse_error_reporting("E_BOGUS"), None);
        assert_eq!(parse_error_reporting(""), Some(30719));
    }
}
