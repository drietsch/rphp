//! Heredoc / nowdoc rules: the `b?"<<<"…{NEWLINE}` start rule with its
//! look-at-next-line and scan-ahead logic, the `ST_HEREDOC` / `ST_NOWDOC`
//! `{ANY_CHAR}` body rules with flexible closing-label detection, and
//! `ST_END_HEREDOC`.
//!
//! Extents worth knowing (all verified against php 8.5):
//! * `T_START_HEREDOC` spans `b?<<<`, optional spaces/tabs, the (optionally
//!   quoted) label and the newline.
//! * A body chunk ending at the closing label *includes* the newline before the
//!   label but not the label's indentation.
//! * `T_END_HEREDOC` spans the closing indentation plus the label.
//! * A closing label needs at least one byte after it (`length < YYLIMIT - YYCURSOR`
//!   is strict), so `<<<EOT\nEOT` at end of file leaves `EOT` as body text.
//!
//! The closing indentation used for `T_END_HEREDOC` of a *heredoc* comes from a
//! scan-ahead pass over the body (a nowdoc records it directly). That pass runs
//! the ordinary lexer and stops early when a rule throws, which in tokenizer
//! mode still happens for `018`-style octal literals, unterminated `/*` and
//! mixed tab/space indentation inside the body; the outer `T_END_HEREDOC` then
//! uses whatever indentation the pass had recorded so far (usually 0), which is
//! why `heredoc_scan_ahead` is emulated rather than approximated.

use crate::ids::*;
use crate::names::{is_label_char, is_label_start};
use crate::scanner::{tok, Scanner, Tok};
use crate::states::State;

const USING_SPACES: u8 = 1;
const USING_TABS: u8 = 2;

/// One entry of `SCNG(heredoc_label_stack)`: the label's bytes (as a range of
/// the source) and the closing indentation once known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeredocLabel {
    pub lo: usize,
    pub hi: usize,
    pub indentation: usize,
}

impl HeredocLabel {
    #[inline]
    fn len(&self) -> usize {
        self.hi - self.lo
    }
}

impl Scanner<'_> {
    /// `<ST_IN_SCRIPTING>b?"<<<"{TABS_AND_SPACES}({LABEL}|([']{LABEL}['])|(["]{LABEL}["])){NEWLINE}`.
    /// `start` is the position of `b`/`<`; `bprefix` is 0 or 1. Returns `None`
    /// when the pattern does not match (then `<<` and `<` lex separately).
    pub(crate) fn try_start_heredoc(&mut self, start: usize, bprefix: usize) -> Option<Tok> {
        let s = self.src;
        let len = s.len();
        let mut p = start + bprefix;
        if !self.starts_with(p, b"<<<") {
            return None;
        }
        p += 3;
        while matches!(self.at(p), b' ' | b'\t') {
            p += 1;
        }
        let (quote, label_lo) = match self.at(p) {
            b'\'' => (b'\'', p + 1),
            b'"' => (b'"', p + 1),
            _ => (0u8, p),
        };
        let label_hi = self.scan_label(label_lo);
        if label_hi == label_lo {
            return None;
        }
        let mut q = label_hi;
        if quote != 0 {
            if self.at(q) != quote {
                return None;
            }
            q += 1;
        }
        let body = match self.at(q) {
            b'\n' => q + 1,
            b'\r' => {
                if self.at(q + 1) == b'\n' {
                    q + 2
                } else {
                    q + 1
                }
            }
            _ => return None,
        };
        // The rule matched: CG(zend_lineno)++ for the newline in the token.
        self.line += 1;
        let is_heredoc = quote != b'\'';
        self.heredocs.push(HeredocLabel {
            lo: label_lo,
            hi: label_hi,
            indentation: 0,
        });
        self.state = if is_heredoc {
            State::Heredoc
        } else {
            State::Nowdoc
        };
        let label_len = label_hi - label_lo;

        // Is the closing label right on the next line?
        let mut c = body;
        let mut indentation = 0usize;
        let mut spacing = 0u8;
        while c < len && (s[c] == b' ' || s[c] == b'\t') {
            spacing |= if s[c] == b'\t' {
                USING_TABS
            } else {
                USING_SPACES
            };
            c += 1;
            indentation += 1;
        }
        if c == len {
            return Some(tok(T_START_HEREDOC, start, body));
        }
        if label_len < len - c
            && s[c..c + label_len] == s[label_lo..label_hi]
            && !is_label_char(s[c + label_len])
        {
            if spacing == USING_SPACES | USING_TABS {
                self.exception = true; // "Invalid indentation - tabs and spaces cannot be mixed"
            }
            if let Some(l) = self.heredocs.last_mut() {
                l.indentation = indentation;
            }
            self.state = State::EndHeredoc;
            return Some(tok(T_START_HEREDOC, start, body));
        }

        if is_heredoc && !self.scan_ahead {
            let indentation = self.heredoc_scan_ahead(body);
            if let Some(l) = self.heredocs.last_mut() {
                l.indentation = indentation;
            }
        }
        Some(tok(T_START_HEREDOC, start, body))
    }

    /// The scan-ahead: lex a clone forward from `body` until the matching
    /// `T_END_HEREDOC` (nesting-aware), the end of input, or the first rule
    /// that throws; report `SCNG(heredoc_indentation)` as it stood then.
    fn heredoc_scan_ahead(&self, body: usize) -> usize {
        let mut sc = self.clone();
        sc.pos = body;
        sc.scan_ahead = true;
        sc.scan_ahead_indent = 0;
        sc.exception = false;
        sc.increment_lineno = false;
        let mut nesting = 1i32;
        while nesting > 0 {
            let Some(t) = sc.lex() else { break };
            if sc.exception {
                break;
            }
            match t.id {
                T_START_HEREDOC => nesting += 1,
                T_END_HEREDOC => nesting -= 1,
                _ => {}
            }
        }
        sc.scan_ahead_indent
    }

    /// `<ST_HEREDOC>{ANY_CHAR}`: a literal run up to the next interpolation or
    /// the closing label.
    pub(crate) fn lex_heredoc_body(&mut self, start: usize) -> Tok {
        self.lex_doc_body(start, false)
    }

    /// `<ST_NOWDOC>{ANY_CHAR}`: the whole body up to the closing label.
    pub(crate) fn lex_nowdoc_body(&mut self, start: usize) -> Tok {
        self.lex_doc_body(start, true)
    }

    fn lex_doc_body(&mut self, start: usize, nowdoc: bool) -> Tok {
        let s = self.src;
        let len = s.len();
        let Some(label) = self.heredocs.last().copied() else {
            // Unreachable by construction; degrade to a literal run to the end.
            self.line += self.count_newlines(start, len);
            return tok(T_ENCAPSED_AND_WHITESPACE, start, len);
        };
        let label_len = label.len();
        let mut q = start;
        let mut newline = 0usize;
        while q < len {
            let c = s[q];
            q += 1;
            match c {
                b'\r' | b'\n' => {
                    if c == b'\r' && q < len && s[q] == b'\n' {
                        q += 1;
                    }
                    let mut indentation = 0usize;
                    let mut spacing = 0u8;
                    while q < len && (s[q] == b' ' || s[q] == b'\t') {
                        spacing |= if s[q] == b'\t' {
                            USING_TABS
                        } else {
                            USING_SPACES
                        };
                        q += 1;
                        indentation += 1;
                    }
                    if q == len {
                        self.line += self.count_newlines(start, q);
                        return tok(T_ENCAPSED_AND_WHITESPACE, start, q);
                    }
                    if is_label_start(s[q])
                        && label_len < len - q
                        && s[q..q + label_len] == s[label.lo..label.hi]
                    {
                        if is_label_char(s[q + label_len]) {
                            continue;
                        }
                        if spacing == USING_SPACES | USING_TABS {
                            self.exception = true; // "Invalid indentation - tabs and spaces cannot be mixed"
                        }
                        // The newline before the label stays in the token's extent
                        // (but not in its value); its line increment is deferred.
                        let nl_pos = q - indentation;
                        newline = if nl_pos >= 2 && s[nl_pos - 2] == b'\r' && s[nl_pos - 1] == b'\n'
                        {
                            2
                        } else {
                            1
                        };
                        self.increment_lineno = true;
                        if nowdoc {
                            q -= indentation;
                            if let Some(l) = self.heredocs.last_mut() {
                                l.indentation = indentation;
                            }
                        } else if self.scan_ahead {
                            self.scan_ahead_indent = indentation;
                        } else {
                            q -= indentation;
                        }
                        self.state = State::EndHeredoc;
                        break;
                    }
                }
                b'$' if !nowdoc => {
                    if is_label_start(self.at(q)) || self.at(q) == b'{' {
                        q -= 1;
                        break;
                    }
                }
                b'{' if !nowdoc => {
                    if self.at(q) == b'$' {
                        q -= 1;
                        break;
                    }
                }
                b'\\' if !nowdoc && q < len && s[q] != b'\n' && s[q] != b'\r' => q += 1,
                _ => {}
            }
        }
        self.line += self.count_newlines(start, q - newline);
        tok(T_ENCAPSED_AND_WHITESPACE, start, q)
    }

    /// `<ST_END_HEREDOC>{ANY_CHAR}`: pop the label; the token is the recorded
    /// indentation plus the label length. (When an aborted scan-ahead recorded
    /// a different indentation than the real one, PHP emits those bytes anyway;
    /// the extent is clamped to the input so the stream stays lossless.)
    pub(crate) fn lex_end_heredoc(&mut self, start: usize) -> Option<Tok> {
        self.state = State::Scripting;
        let label = self.heredocs.pop()?;
        let hi = (start + label.indentation + label.len()).min(self.src.len());
        Some(tok(T_END_HEREDOC, start, hi))
    }
}
