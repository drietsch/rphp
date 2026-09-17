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
    /// Lexer-level: an unexpected or unrecognised byte.
    pub const UNEXPECTED_CHAR: &str = "RPHP_E0001";
    /// Parser-level: an unexpected token (message carries the expected set).
    pub const UNEXPECTED_TOKEN: &str = "RPHP_E0002";
    /// An unterminated string literal.
    pub const UNTERMINATED: &str = "RPHP_E0003";
    /// Unexpected end of file (message carries the expected set).
    pub const UNEXPECTED_EOF: &str = "RPHP_E0004";
    /// The parser's nesting limit was exceeded.
    pub const RECURSION_LIMIT: &str = "RPHP_E0005";
    /// A literal the lexer accepted but PHP rejects at parse time (invalid
    /// numeric literal, invalid `\u{}` escape, heredoc indentation).
    pub const INVALID_LITERAL: &str = "RPHP_E0006";
    /// A syntax node kind the adapter does not map (`unsupported syntax node: <Kind>`).
    pub const UNSUPPORTED_NODE: &str = "RPHP_E0010";
    /// Syntax mago accepts but PHP 8.5 rejects (the conformance pass), with
    /// PHP's own message.
    pub const SUPERSET_REJECTED: &str = "RPHP_E0011";
    /// Write context: the target's shape cannot be written through.
    pub const LVALUE_NOT_WRITABLE: &str = "RPHP_E0020";
    /// Write context: a nullsafe fetch inside a write or reference chain.
    pub const LVALUE_NULLSAFE: &str = "RPHP_E0021";
    /// Write context: `$this` rebound, unset or imported.
    pub const LVALUE_THIS: &str = "RPHP_E0022";
    /// Write context: `$GLOBALS` written as a whole or appended to.
    pub const LVALUE_GLOBALS: &str = "RPHP_E0023";
    /// Write context: `[]` (append) where the chain is read or unset.
    pub const LVALUE_APPEND: &str = "RPHP_E0024";
    /// An ill-formed destructuring pattern.
    pub const LVALUE_DESTRUCTURING: &str = "RPHP_E0025";
    /// `declare(strict_types=...)` placement or value. Shares the code of the
    /// destructuring validation: the roadmap allocates `E0020–E0025` to the
    /// lvalue / strict_types validation group as a whole.
    pub const STRICT_TYPES: &str = "RPHP_E0025";
    pub const UNDEFINED_FUNCTION: &str = "RPHP_E0100";
    pub const WRONG_ARG_COUNT: &str = "RPHP_E0101";
    /// Two unconditional declarations of the same function in one file
    /// (`Cannot redeclare function f() (previously declared in ...)`).
    /// Shared with `rphp-compiler`'s `REDECLARED_FUNCTION`.
    pub const REDECLARED_FUNCTION: &str = "RPHP_E0102";
    /// Two unconditional declarations of the same class-like in one file
    /// (`Cannot redeclare class A (previously declared in ...)`). Shared
    /// with `rphp-compiler`'s `REDECLARED_CLASS`.
    pub const REDECLARED_CLASS: &str = "RPHP_E0106";
    /// Resolution (`rphp-hir`): a `use` import conflicts with another import
    /// or with a declaration in the same file (`Cannot use X as Y because the
    /// name is already in use`, `... (previously declared as local import)`).
    pub const IMPORT_CONFLICT: &str = "RPHP_E0200";
    /// Resolution: a reserved class name (`int`, `self`, ...) declared,
    /// aliased, extended, caught or written fully qualified
    /// (`Cannot use "int" as a class name as it is reserved`).
    pub const RESERVED_CLASS_NAME: &str = "RPHP_E0201";
    /// Validation: `goto` to a label the function body does not define, or a
    /// label defined twice (`'goto' to undefined label 'x'`).
    pub const UNDEFINED_LABEL: &str = "RPHP_E0202";
    /// Validation: an illegal jump — `goto` into a loop/switch or across a
    /// `finally` boundary, `break`/`continue` outside a loop or with too many
    /// levels (`'goto' into loop or switch statement is disallowed`).
    pub const INVALID_JUMP: &str = "RPHP_E0203";
    /// Resolution: `self`/`static` where no class scope is active
    /// (`Cannot use "self" when no class scope is active`).
    pub const NO_CLASS_SCOPE: &str = "RPHP_E0204";
    /// Resolution: `parent` in a class without a parent
    /// (`Cannot use "parent" when current class scope has no parent`).
    pub const NO_PARENT_SCOPE: &str = "RPHP_E0205";
    /// Resolution: braced and unbraced namespace declarations in one file
    /// (`Cannot mix bracketed namespace declarations with unbracketed
    /// namespace declarations`).
    pub const NAMESPACE_MIX: &str = "RPHP_E0206";
    /// A syntactically valid construct the compiler does not lower yet
    /// (`unsupported construct: <what> (not lowered yet)`).
    pub const UNSUPPORTED_CONSTRUCT: &str = "RPHP_E0300";
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
