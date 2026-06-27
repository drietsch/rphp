//! Structured diagnostics: stable codes, severity, labelled spans, and a
//! minimal renderer. Errors are data, not strings — the renderer is just one
//! consumer (an LSP/JSON emitter is another). Parser errors are recoverable, so
//! a compilation collects many `Diagnostic`s rather than aborting on the first.
#![forbid(unsafe_code)]

use rphp_source::SourceMap;
use rphp_span::Span;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn tag(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub primary: Option<Label>,
    pub secondary: Vec<Label>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            primary: None,
            secondary: Vec::new(),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, ..Self::error(code, message) }
    }

    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.primary = Some(Label { span, message: message.into() });
        self
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary.push(Label { span, message: message.into() });
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Render a single-line, human-readable form using source positions.
    pub fn render(&self, sources: &SourceMap) -> String {
        let mut out = format!("{}[{}]: {}", self.severity.tag(), self.code, self.message);
        if let Some(label) = &self.primary {
            let file = sources.get(label.span.file);
            let (line, col) = file.line_col(label.span.lo);
            out.push_str(&format!("\n  --> {}:{}:{}", file.name, line, col));
            if !label.message.is_empty() {
                out.push_str(&format!("\n   = {}", label.message));
            }
        }
        out
    }
}

/// Stable diagnostic codes (`RPHP_E####`). Add as the corpus surfaces shapes.
pub mod codes {
    pub const UNEXPECTED_CHAR: &str = "RPHP_E0001";
    pub const UNEXPECTED_TOKEN: &str = "RPHP_E0002";
    pub const UNTERMINATED: &str = "RPHP_E0003";
    pub const UNDEFINED_FUNCTION: &str = "RPHP_E0100";
    pub const WRONG_ARG_COUNT: &str = "RPHP_E0101";
}

#[cfg(test)]
mod tests {
    use super::*;
    use rphp_span::{FileId, Span};

    #[test]
    fn render_points_at_source() {
        let mut sm = SourceMap::new();
        let f = sm.add("t.php", &b"<?php\n$x = ;"[..]);
        let d = Diagnostic::error(codes::UNEXPECTED_TOKEN, "expected expression")
            .with_primary(Span::new(f, 11, 12), "here");
        let r = d.render(&sm);
        assert!(r.contains("RPHP_E0002"));
        assert!(r.contains("t.php:2:6"));
    }

    #[test]
    fn dummy_span_uses_file_zero() {
        // ensure FileId is reachable from this crate's API surface
        let _ = Span::new(FileId(0), 0, 0);
    }
}
