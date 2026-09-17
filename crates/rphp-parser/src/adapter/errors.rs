//! mago's parse errors → `rphp_diagnostics::Diagnostic` (`RPHP_E0001..E0005`).
//!
//! The message text is mago's own (`Display`), which already appends the
//! expected-token set for `UnexpectedToken` / `UnexpectedEndOfFile`.

use mago_span::HasSpan;
use mago_syntax::error::{ParseError, SyntaxError};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_span::{FileId, Span};

/// The diagnostic code for a mago error kind.
pub(crate) fn code_for(err: &ParseError) -> &'static str {
    match err {
        ParseError::SyntaxError(SyntaxError::UnexpectedToken(..))
        | ParseError::SyntaxError(SyntaxError::UnrecognizedToken(..)) => codes::UNEXPECTED_CHAR,
        ParseError::SyntaxError(SyntaxError::UnexpectedEndOfFile(..))
        | ParseError::UnexpectedEndOfFile(..) => codes::UNEXPECTED_EOF,
        ParseError::SyntaxError(SyntaxError::RecursionLimitExceeded(..))
        | ParseError::RecursionLimitExceeded(_) => codes::RECURSION_LIMIT,
        ParseError::UnexpectedToken(..) => codes::UNEXPECTED_TOKEN,
        ParseError::UnclosedLiteralString(..) => codes::UNTERMINATED,
    }
}

/// Convert one mago error into a diagnostic in `file`.
pub(crate) fn diagnostic(err: &ParseError, file: FileId) -> Diagnostic {
    let s = err.span();
    let span = Span::new(file, s.start.offset, s.end.offset);
    Diagnostic::error(code_for(err), err.to_string()).with_primary(span, "")
}
