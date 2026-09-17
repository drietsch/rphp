//! The re2c start conditions of `Zend/zend_language_scanner.l`.
//!
//! `SHEBANG` is intentionally absent: `token_get_all()` starts the scanner in
//! `INITIAL` (only the CLI SAPI strips a `#!` line), so a shebang is plain
//! `T_INLINE_HTML`. `__halt_compiler()` is not a scanner state either; the
//! "three more tokens, then the rest is `T_INLINE_HTML`" countdown that follows
//! it lives in the driver loop in `lib.rs`, exactly as in ext/tokenizer.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// `INITIAL`: outside `<?php … ?>`, producing `T_INLINE_HTML`.
    Initial,
    /// `ST_IN_SCRIPTING`: PHP code.
    Scripting,
    /// `ST_LOOKING_FOR_PROPERTY`: right after `->` / `?->`; the next label is a
    /// `T_STRING` even when it spells a keyword.
    LookingForProperty,
    /// `ST_DOUBLE_QUOTES`: inside an interpolated `"…"` string.
    DoubleQuotes,
    /// `ST_HEREDOC`: inside a heredoc body.
    Heredoc,
    /// `ST_NOWDOC`: inside a nowdoc body (no interpolation at all).
    Nowdoc,
    /// `ST_BACKQUOTE`: inside `` `…` ``.
    Backquote,
    /// `ST_VAR_OFFSET`: inside the `[…]` of a simple `"$a[…]"` interpolation.
    VarOffset,
    /// `ST_LOOKING_FOR_VARNAME`: right after `${` inside a string.
    LookingForVarname,
    /// `ST_END_HEREDOC`: positioned on the closing label of a heredoc/nowdoc.
    EndHeredoc,
}
