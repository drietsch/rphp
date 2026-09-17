//! The Zend scanner state machine (`Zend/zend_language_scanner.l`), rule for
//! rule, in tokenizer (non-parser) mode.
//!
//! Every [`Scanner::lex`] call yields the next raw token exactly as `lex_scan()`
//! would: the same extents, the same state transitions and the same line
//! accounting. re2c semantics apply throughout — the longest match wins and
//! earlier rules break ties — and the scanner is compiled with
//! `--case-inverted`, so every quoted literal in the rules is case-insensitive.
//! Reads past the end of input see a NUL byte (`at()`), which is what the
//! zero-padded Zend buffer does; explicit `pos < len` checks are used where the
//! C code compares against `YYLIMIT` rather than reading a byte.
//!
//! Heredoc rules live in `heredoc.rs`; this file owns everything else.

use crate::heredoc::HeredocLabel;
use crate::ids::*;
use crate::names::{self, is_label_char, is_label_start};
use crate::states::State;

/// A raw token as the scanner produces it: id plus byte extent. The line is
/// attached by the driver in `lib.rs`, mirroring `token_line` in tokenizer.c.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tok {
    pub id: u16,
    pub lo: usize,
    pub hi: usize,
}

#[inline]
pub(crate) fn tok(id: u16, lo: usize, hi: usize) -> Tok {
    Tok { id, lo, hi }
}

/// The token id of a single-character token is its byte value.
#[inline]
const fn ch(c: u8) -> u16 {
    c as u16
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

/// Scanner state: the re2c condition, the `yy_push_state` stack, the heredoc
/// label stack and the line counter (`CG(zend_lineno)`).
///
/// `Clone` is what makes the heredoc scan-ahead cheap: the start-of-heredoc rule
/// clones the scanner, runs it forward in `scan_ahead` mode until the matching
/// `T_END_HEREDOC`, and throws the clone away (`zend_save_lexical_state` /
/// `zend_restore_lexical_state` in C).
#[derive(Clone)]
pub(crate) struct Scanner<'a> {
    pub(crate) src: &'a [u8],
    /// `YYCURSOR`.
    pub(crate) pos: usize,
    /// The current re2c condition.
    pub(crate) state: State,
    /// `SCNG(state_stack)`.
    pub(crate) stack: Vec<State>,
    /// `SCNG(heredoc_label_stack)`.
    pub(crate) heredocs: Vec<HeredocLabel>,
    /// `CG(zend_lineno)`: incremented while scanning, read by the driver after
    /// each token to stamp the *next* token.
    pub(crate) line: u32,
    /// `CG(increment_lineno)`: a deferred `+1` applied by the driver after the
    /// token that set it (`?>\n`, and the newline before a closing heredoc label).
    pub(crate) increment_lineno: bool,
    /// `CG(short_tags)`.
    pub(crate) short_open_tag: bool,
    /// `SCNG(heredoc_scan_ahead)`: true inside the clone that pre-scans a heredoc body.
    pub(crate) scan_ahead: bool,
    /// `SCNG(heredoc_indentation)`: the closing indentation found by the scan-ahead.
    pub(crate) scan_ahead_indent: usize,
    /// `EG(exception)`: set by the rules that call `zend_throw_exception()` even
    /// in tokenizer mode. `token_get_all()` clears it afterwards, so its only
    /// observable effect is aborting a heredoc scan-ahead early.
    pub(crate) exception: bool,
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(src: &'a [u8], short_open_tag: bool) -> Self {
        Scanner {
            src,
            pos: 0,
            state: State::Initial,
            stack: Vec::new(),
            heredocs: Vec::new(),
            line: 1,
            increment_lineno: false,
            short_open_tag,
            scan_ahead: false,
            scan_ahead_indent: 0,
            exception: false,
        }
    }

    // ----- primitive helpers --------------------------------------------------

    /// The byte at `i`, or NUL past the end (the Zend buffer is zero-padded).
    #[inline]
    pub(crate) fn at(&self, i: usize) -> u8 {
        self.src.get(i).copied().unwrap_or(0)
    }

    #[inline]
    pub(crate) fn starts_with(&self, i: usize, pat: &[u8]) -> bool {
        self.src.get(i..i + pat.len()) == Some(pat)
    }

    #[inline]
    pub(crate) fn starts_with_ci(&self, i: usize, pat: &[u8]) -> bool {
        self.src
            .get(i..i + pat.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(pat))
    }

    /// End of the `{LABEL}` starting at `i`, or `i` if none starts there.
    #[inline]
    pub(crate) fn scan_label(&self, i: usize) -> usize {
        if !is_label_start(self.at(i)) {
            return i;
        }
        let mut e = i + 1;
        while is_label_char(self.at(e)) {
            e += 1;
        }
        e
    }

    /// `HANDLE_NEWLINES(s, l)`: `\n` counts, `\r` counts unless a `\n` follows —
    /// and the byte *after* the range is consulted for that, like the C macro.
    pub(crate) fn count_newlines(&self, lo: usize, hi: usize) -> u32 {
        let hi = hi.min(self.src.len());
        let mut n = 0;
        let mut i = lo;
        while i < hi {
            match self.src[i] {
                b'\n' => n += 1,
                b'\r' if self.at(i + 1) != b'\n' => n += 1,
                _ => {}
            }
            i += 1;
        }
        n
    }

    /// `yy_push_state`: remember the current condition, switch to `s`.
    #[inline]
    pub(crate) fn push_state(&mut self, s: State) {
        self.stack.push(self.state);
        self.state = s;
    }

    /// `yy_pop_state`.
    #[inline]
    pub(crate) fn pop_state(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    // ----- the lex_scan() loop ----------------------------------------------

    /// The next token, or `None` at end of input (`END`). Rules that consume
    /// nothing and `goto restart` are looped here.
    pub(crate) fn lex(&mut self) -> Option<Tok> {
        loop {
            if self.pos >= self.src.len() {
                return None;
            }
            let start = self.pos;
            let t = match self.state {
                State::Initial => Some(self.lex_initial(start)),
                State::Scripting => Some(self.lex_scripting(start)),
                State::LookingForProperty => self.lex_looking_for_property(start),
                State::LookingForVarname => self.lex_looking_for_varname(start),
                State::VarOffset => Some(self.lex_var_offset(start)),
                State::DoubleQuotes => Some(self.lex_string_state(start, b'"')),
                State::Backquote => Some(self.lex_string_state(start, b'`')),
                State::Heredoc => Some(self.lex_string_state(start, 0)),
                State::Nowdoc => Some(self.lex_nowdoc_body(start)),
                State::EndHeredoc => self.lex_end_heredoc(start),
            };
            if let Some(t) = t {
                debug_assert!(t.lo == start && t.hi >= t.lo && t.hi <= self.src.len());
                self.pos = t.hi;
                return Some(t);
            }
        }
    }

    // ----- INITIAL ------------------------------------------------------------

    fn lex_initial(&mut self, start: usize) -> Tok {
        let len = self.src.len();
        if self.starts_with(start, b"<?=") {
            self.state = State::Scripting;
            return tok(T_OPEN_TAG_WITH_ECHO, start, start + 3);
        }
        if self.starts_with_ci(start, b"<?php") {
            let p = start + 5;
            if p == len {
                // "<?php" at end of file is an open tag.
                self.state = State::Scripting;
                return tok(T_OPEN_TAG, start, p);
            }
            match self.src[p] {
                // "<?php"([ \t]|{NEWLINE}) — one whitespace character is part of the tag.
                b' ' | b'\t' => {
                    self.state = State::Scripting;
                    return tok(T_OPEN_TAG, start, p + 1);
                }
                b'\n' => {
                    self.line += 1;
                    self.state = State::Scripting;
                    return tok(T_OPEN_TAG, start, p + 1);
                }
                b'\r' => {
                    self.line += 1;
                    self.state = State::Scripting;
                    let e = if self.at(p + 1) == b'\n' {
                        p + 2
                    } else {
                        p + 1
                    };
                    return tok(T_OPEN_TAG, start, e);
                }
                _ => {
                    // Degenerate "<?phpX": with short tags it is "<?" + "phpX".
                    if self.short_open_tag {
                        self.state = State::Scripting;
                        return tok(T_OPEN_TAG, start, start + 2);
                    }
                }
            }
        } else if self.short_open_tag && self.starts_with(start, b"<?") {
            self.state = State::Scripting;
            return tok(T_OPEN_TAG, start, start + 2);
        }
        self.lex_inline_html(start)
    }

    /// `inline_char_handler`: everything up to the next real open tag.
    fn lex_inline_html(&mut self, start: usize) -> Tok {
        let s = self.src;
        let len = s.len();
        let mut p = start + 1;
        let end = loop {
            let Some(off) = memchr::memchr(b'<', &s[p.min(len)..]) else {
                break len;
            };
            let i = p + off;
            if i + 1 >= len {
                break len;
            }
            if s[i + 1] == b'?'
                && (self.short_open_tag
                    || self.at(i + 2) == b'='
                    || (self.starts_with_ci(i + 2, b"php")
                        && (i + 5 == len || is_ws(self.at(i + 5)))))
            {
                break i;
            }
            p = i + 1;
        };
        self.line += self.count_newlines(start, end);
        tok(T_INLINE_HTML, start, end)
    }

    // ----- ST_IN_SCRIPTING ---------------------------------------------------

    fn lex_scripting(&mut self, start: usize) -> Tok {
        let c = self.src[start];
        let n1 = self.at(start + 1);
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => self.whitespace(start),
            b'#' => {
                if n1 == b'[' {
                    tok(T_ATTRIBUTE, start, start + 2)
                } else {
                    self.line_comment(start, 1)
                }
            }
            b'/' => match n1 {
                b'*' => self.block_comment(start),
                b'/' => self.line_comment(start, 2),
                b'=' => tok(T_DIV_EQUAL, start, start + 2),
                _ => tok(ch(b'/'), start, start + 1),
            },
            b'?' => {
                if self.starts_with(start + 1, b"->") {
                    self.push_state(State::LookingForProperty);
                    tok(T_NULLSAFE_OBJECT_OPERATOR, start, start + 3)
                } else if n1 == b'?' {
                    if self.at(start + 2) == b'=' {
                        tok(T_COALESCE_EQUAL, start, start + 3)
                    } else {
                        tok(T_COALESCE, start, start + 2)
                    }
                } else if n1 == b'>' {
                    self.close_tag(start)
                } else {
                    tok(ch(b'?'), start, start + 1)
                }
            }
            b'-' => match n1 {
                b'>' => {
                    self.push_state(State::LookingForProperty);
                    tok(T_OBJECT_OPERATOR, start, start + 2)
                }
                b'-' => tok(T_DEC, start, start + 2),
                b'=' => tok(T_MINUS_EQUAL, start, start + 2),
                _ => tok(ch(b'-'), start, start + 1),
            },
            b'=' => {
                if self.starts_with(start + 1, b"==") {
                    tok(T_IS_IDENTICAL, start, start + 3)
                } else {
                    match n1 {
                        b'=' => tok(T_IS_EQUAL, start, start + 2),
                        b'>' => tok(T_DOUBLE_ARROW, start, start + 2),
                        _ => tok(ch(b'='), start, start + 1),
                    }
                }
            }
            b'!' => {
                if self.starts_with(start + 1, b"==") {
                    tok(T_IS_NOT_IDENTICAL, start, start + 3)
                } else if n1 == b'=' {
                    tok(T_IS_NOT_EQUAL, start, start + 2)
                } else {
                    tok(ch(b'!'), start, start + 1)
                }
            }
            b'<' => {
                if let Some(t) = self.try_start_heredoc(start, 0) {
                    t
                } else if self.starts_with(start + 1, b"=>") {
                    tok(T_SPACESHIP, start, start + 3)
                } else if self.starts_with(start + 1, b"<=") {
                    tok(T_SL_EQUAL, start, start + 3)
                } else {
                    match n1 {
                        b'<' => tok(T_SL, start, start + 2),
                        b'=' => tok(T_IS_SMALLER_OR_EQUAL, start, start + 2),
                        b'>' => tok(T_IS_NOT_EQUAL, start, start + 2),
                        _ => tok(ch(b'<'), start, start + 1),
                    }
                }
            }
            b'>' => {
                if self.starts_with(start + 1, b">=") {
                    tok(T_SR_EQUAL, start, start + 3)
                } else {
                    match n1 {
                        b'>' => tok(T_SR, start, start + 2),
                        b'=' => tok(T_IS_GREATER_OR_EQUAL, start, start + 2),
                        _ => tok(ch(b'>'), start, start + 1),
                    }
                }
            }
            b'+' => match n1 {
                b'+' => tok(T_INC, start, start + 2),
                b'=' => tok(T_PLUS_EQUAL, start, start + 2),
                _ => tok(ch(b'+'), start, start + 1),
            },
            b'*' => {
                if self.starts_with(start + 1, b"*=") {
                    tok(T_POW_EQUAL, start, start + 3)
                } else {
                    match n1 {
                        b'*' => tok(T_POW, start, start + 2),
                        b'=' => tok(T_MUL_EQUAL, start, start + 2),
                        _ => tok(ch(b'*'), start, start + 1),
                    }
                }
            }
            b'.' => {
                if n1.is_ascii_digit() {
                    self.number(start)
                } else if self.starts_with(start + 1, b"..") {
                    tok(T_ELLIPSIS, start, start + 3)
                } else if n1 == b'=' {
                    tok(T_CONCAT_EQUAL, start, start + 2)
                } else {
                    tok(ch(b'.'), start, start + 1)
                }
            }
            b'%' => {
                if n1 == b'=' {
                    tok(T_MOD_EQUAL, start, start + 2)
                } else {
                    tok(ch(b'%'), start, start + 1)
                }
            }
            b'&' => match n1 {
                b'&' => tok(T_BOOLEAN_AND, start, start + 2),
                b'=' => tok(T_AND_EQUAL, start, start + 2),
                _ => self.ampersand(start),
            },
            b'|' => match n1 {
                b'|' => tok(T_BOOLEAN_OR, start, start + 2),
                b'=' => tok(T_OR_EQUAL, start, start + 2),
                b'>' => tok(T_PIPE, start, start + 2),
                _ => tok(ch(b'|'), start, start + 1),
            },
            b'^' => {
                if n1 == b'=' {
                    tok(T_XOR_EQUAL, start, start + 2)
                } else {
                    tok(ch(b'^'), start, start + 1)
                }
            }
            b':' => {
                if n1 == b':' {
                    tok(T_DOUBLE_COLON, start, start + 2)
                } else {
                    tok(ch(b':'), start, start + 1)
                }
            }
            b'$' => {
                let e = self.scan_label(start + 1);
                if e > start + 1 {
                    tok(T_VARIABLE, start, e)
                } else {
                    tok(ch(b'$'), start, start + 1)
                }
            }
            b'(' => self
                .cast(start)
                .unwrap_or_else(|| tok(ch(b'('), start, start + 1)),
            b'{' => {
                self.push_state(State::Scripting);
                tok(ch(b'{'), start, start + 1)
            }
            b'}' => {
                // RESET_DOC_COMMENT(); pop only when something was pushed.
                if !self.stack.is_empty() {
                    self.pop_state();
                }
                tok(ch(b'}'), start, start + 1)
            }
            b'"' => self.double_quoted(start, 0),
            b'\'' => self.single_quoted(start, 0),
            b'`' => {
                self.state = State::Backquote;
                tok(ch(b'`'), start, start + 1)
            }
            b'\\' => {
                let e = self.scan_qualified_tail(start);
                if e > start {
                    tok(T_NAME_FULLY_QUALIFIED, start, e)
                } else {
                    tok(T_NS_SEPARATOR, start, start + 1)
                }
            }
            b'0'..=b'9' => self.number(start),
            b')' | b']' | b'[' | b';' | b',' | b'~' | b'@' => tok(ch(c), start, start + 1),
            _ if is_label_start(c) => self.label(start),
            _ => tok(T_BAD_CHARACTER, start, start + 1),
        }
    }

    /// `{WHITESPACE}+` (shared by `ST_IN_SCRIPTING` and `ST_LOOKING_FOR_PROPERTY`).
    fn whitespace(&mut self, start: usize) -> Tok {
        let mut e = start + 1;
        while e < self.src.len() && is_ws(self.src[e]) {
            e += 1;
        }
        self.line += self.count_newlines(start, e);
        tok(T_WHITESPACE, start, e)
    }

    /// `"#"|"//"`: up to (not including) the newline or a `?>`.
    fn line_comment(&mut self, start: usize, prefix: usize) -> Tok {
        let s = self.src;
        let len = s.len();
        let mut q = start + prefix;
        while q < len {
            match s[q] {
                b'\r' | b'\n' => break,
                b'?' if self.at(q + 1) == b'>' => break,
                _ => q += 1,
            }
        }
        tok(T_COMMENT, start, q)
    }

    /// `"/*"|"/**"{WHITESPACE}`: a doc comment needs a whitespace byte right
    /// after `/**`, so `/**/` and `/***/` are plain comments. An unterminated
    /// comment runs to the end of input (and throws — see `exception`).
    fn block_comment(&mut self, start: usize) -> Tok {
        let s = self.src;
        let len = s.len();
        let doc = self.at(start + 2) == b'*' && is_ws(self.at(start + 3));
        let from = if doc { start + 4 } else { start + 2 };
        let end = match memchr::memmem::find(&s[from.min(len)..], b"*/") {
            Some(off) => from + off + 2,
            None => {
                self.exception = true; // "Unterminated comment starting line %d"
                len
            }
        };
        self.line += self.count_newlines(start, end);
        tok(if doc { T_DOC_COMMENT } else { T_COMMENT }, start, end)
    }

    /// `"?>"{NEWLINE}?` — the newline is part of the token but its line
    /// increment is deferred (`CG(increment_lineno)`).
    fn close_tag(&mut self, start: usize) -> Tok {
        let mut e = start + 2;
        match self.at(e) {
            b'\n' => {
                e += 1;
                self.increment_lineno = true;
            }
            b'\r' => {
                e += 1;
                if self.at(e) == b'\n' {
                    e += 1;
                }
                self.increment_lineno = true;
            }
            _ => {}
        }
        self.state = State::Initial;
        tok(T_CLOSE_TAG, start, e)
    }

    /// `("\\"{LABEL})*` starting at `i` (which must be a backslash to match at
    /// all); returns `i` when no segment matches.
    fn scan_qualified_tail(&self, i: usize) -> usize {
        let mut e = i;
        while self.at(e) == b'\\' && is_label_start(self.at(e + 1)) {
            e = self.scan_label(e + 1);
        }
        e
    }

    /// `"("{TABS_AND_SPACES}<word>{TABS_AND_SPACES}")"` cast operators.
    fn cast(&self, start: usize) -> Option<Tok> {
        let mut p = start + 1;
        while matches!(self.at(p), b' ' | b'\t') {
            p += 1;
        }
        let mut e = p;
        while self.at(e).is_ascii_alphabetic() {
            e += 1;
        }
        let id = names::cast(&self.src[p..e])?;
        let mut q = e;
        while matches!(self.at(q), b' ' | b'\t') {
            q += 1;
        }
        (self.at(q) == b')').then(|| tok(id, start, q + 1))
    }

    /// `"&"{OPTIONAL_WHITESPACE_OR_COMMENTS}("$"|"...")` versus plain `"&"`;
    /// both yield a one-byte token, only the id differs.
    fn ampersand(&self, start: usize) -> Tok {
        let mut ends = self.ws_or_comment_ends(start + 1);
        ends.push(start + 1);
        let followed = ends
            .iter()
            .any(|&q| self.at(q) == b'$' || self.starts_with(q, b"..."));
        let id = if followed {
            T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
        } else {
            T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG
        };
        tok(id, start, start + 1)
    }

    /// Every position reachable from `start` by consuming one or more items of
    /// `{WHITESPACE_OR_COMMENTS}` — the regex the `yield from`, `enum` and `&`
    /// rules look ahead with. It is a *set*, not a single position, because
    /// `HASH_COMMENT` is `"#"(([^[\x00][^\x00\n\r]*[\n\r])|[\n\r])` and its
    /// first alternative may spend a newline as the "any" character, swallowing
    /// the whole next line; re2c then keeps whichever path gives the longest
    /// overall match, so callers examine all of them. Note the regexes reject
    /// NUL bytes and require line comments to end in a newline.
    pub(crate) fn ws_or_comment_ends(&self, start: usize) -> Vec<usize> {
        let s = self.src;
        let len = s.len();
        let mut ends: Vec<usize> = Vec::new();
        let mut work = vec![start];
        while let Some(p) = work.pop() {
            let mut next = [None, None];
            match self.at(p) {
                b' ' | b'\t' | b'\n' | b'\r' => {
                    // Intermediate positions inside a whitespace run cannot start
                    // a comment or any lookahead tail, so the run is one step.
                    let mut e = p + 1;
                    while is_ws(self.at(e)) {
                        e += 1;
                    }
                    next[0] = Some(e);
                }
                b'/' if self.at(p + 1) == b'*' => {
                    if let Some(off) = memchr::memmem::find(&s[(p + 2).min(len)..], b"*/") {
                        let k = p + 2 + off;
                        if !s[p + 2..k].contains(&0) {
                            next[0] = Some(k + 2);
                        }
                    }
                }
                b'/' if self.at(p + 1) == b'/' => next[0] = self.regex_line_comment_end(p + 2),
                b'#' => {
                    let c1 = self.at(p + 1);
                    if c1 != 0 && c1 != b'[' {
                        if c1 == b'\n' || c1 == b'\r' {
                            next[0] = Some(p + 2);
                        }
                        next[1] = self.regex_line_comment_end(p + 2);
                    }
                }
                _ => {}
            }
            for n in next.into_iter().flatten() {
                if !ends.contains(&n) {
                    ends.push(n);
                    work.push(n);
                }
            }
        }
        ends
    }

    /// `[^\x00\n\r]*[\n\r]` from `p`: the position after the newline, or `None`
    /// when a NUL byte or the end of input comes first.
    fn regex_line_comment_end(&self, mut p: usize) -> Option<usize> {
        loop {
            match *self.src.get(p)? {
                0 => return None,
                b'\n' | b'\r' => return Some(p + 1),
                _ => p += 1,
            }
        }
    }

    /// A label in `ST_IN_SCRIPTING`: `b"…"`/`b'…'`/`b<<<` prefixes, qualified
    /// names, `yield from`, `enum`, `public(set)`, keywords, or `T_STRING`.
    fn label(&mut self, start: usize) -> Tok {
        let s = self.src;
        if s[start] == b'b' || s[start] == b'B' {
            match self.at(start + 1) {
                b'"' => return self.double_quoted(start, 1),
                b'\'' => return self.single_quoted(start, 1),
                b'<' => {
                    if let Some(t) = self.try_start_heredoc(start, 1) {
                        return t;
                    }
                }
                _ => {}
            }
        }
        let e = self.scan_label(start);
        // {LABEL}("\\"{LABEL})+ and "namespace"("\\"{LABEL})+ are longer than any keyword.
        let qe = self.scan_qualified_tail(e);
        if qe > e {
            let id = if s[start..e].eq_ignore_ascii_case(b"namespace") {
                T_NAME_RELATIVE
            } else {
                T_NAME_QUALIFIED
            };
            return tok(id, start, qe);
        }
        let label = &s[start..e];
        if label.eq_ignore_ascii_case(b"yield") {
            // "yield"{WHITESPACE_OR_COMMENTS}"from"[^a-zA-Z0-9_\x80-\xff], then yyless(1).
            let mut best: Option<usize> = None;
            for q in self.ws_or_comment_ends(e) {
                if self.starts_with_ci(q, b"from") && !is_label_char(self.at(q + 4)) {
                    best = Some(best.map_or(q + 4, |b| b.max(q + 4)));
                }
            }
            if let Some(hi) = best {
                self.line += self.count_newlines(start, hi);
                return tok(T_YIELD_FROM, start, hi);
            }
            return tok(T_YIELD, start, e);
        }
        if label.eq_ignore_ascii_case(b"enum") {
            // "enum"{WS_OR_COMMENTS}("extends"|"implements") -> T_STRING, else
            // "enum"{WS_OR_COMMENTS}[a-zA-Z_\x80-\xff] -> T_ENUM; longest overall
            // match wins, the first rule wins ties, and both yyless(4).
            let mut best: Option<(usize, u16)> = None;
            for q in self.ws_or_comment_ends(e) {
                let mut cands = [None, None, None];
                if self.starts_with_ci(q, b"extends") {
                    cands[0] = Some((q + 7, T_STRING));
                }
                if self.starts_with_ci(q, b"implements") {
                    cands[1] = Some((q + 10, T_STRING));
                }
                if is_label_start(self.at(q)) {
                    cands[2] = Some((q + 1, T_ENUM));
                }
                for (end, id) in cands.into_iter().flatten() {
                    best = Some(match best {
                        None => (end, id),
                        Some((be, bid)) => {
                            if end > be || (end == be && bid == T_ENUM && id == T_STRING) {
                                (end, id)
                            } else {
                                (be, bid)
                            }
                        }
                    });
                }
            }
            return tok(best.map_or(T_STRING, |(_, id)| id), start, e);
        }
        if let Some(id) = names::keyword(label) {
            if matches!(id, T_PUBLIC | T_PROTECTED | T_PRIVATE) && self.starts_with_ci(e, b"(set)")
            {
                let id = match id {
                    T_PUBLIC => T_PUBLIC_SET,
                    T_PROTECTED => T_PROTECTED_SET,
                    _ => T_PRIVATE_SET,
                };
                return tok(id, start, e + 5);
            }
            return tok(id, start, e);
        }
        tok(T_STRING, start, e)
    }

    // ----- numbers ------------------------------------------------------------

    /// `[0-9]+(_[0-9]+)*` starting at `p` (a digit).
    fn lnum_end(&self, p: usize) -> usize {
        self.radix_end(p, |c| c.is_ascii_digit())
    }

    /// `d+(_d+)*` for the digit class `ok`, starting at `p` (which satisfies `ok`).
    fn radix_end(&self, p: usize, ok: impl Fn(u8) -> bool) -> usize {
        let mut e = p;
        while ok(self.at(e)) {
            e += 1;
        }
        while self.at(e) == b'_' && ok(self.at(e + 1)) {
            e += 1;
            while ok(self.at(e)) {
                e += 1;
            }
        }
        e
    }

    /// `{BNUM}`, `{ONUM}`, `{HNUM}`, `{LNUM}`, `{DNUM}`, `{EXPONENT_DNUM}` —
    /// longest match — plus the `T_LNUMBER` / `T_DNUMBER` overflow decision.
    fn number(&mut self, start: usize) -> Tok {
        let s = self.src;
        if s[start] == b'0' {
            let d = self.at(start + 2);
            match self.at(start + 1) {
                b'b' | b'B' if d == b'0' || d == b'1' => {
                    let e = self.radix_end(start + 2, |c| c == b'0' || c == b'1');
                    return tok(self.classify_bin(start + 2, e), start, e);
                }
                b'o' | b'O' if (b'0'..=b'7').contains(&d) => {
                    let e = self.radix_end(start + 2, |c| (b'0'..=b'7').contains(&c));
                    return tok(self.classify_oct(start + 2, e), start, e);
                }
                b'x' | b'X' if d.is_ascii_hexdigit() => {
                    let e = self.radix_end(start + 2, |c| c.is_ascii_hexdigit());
                    return tok(self.classify_hex(start + 2, e), start, e);
                }
                _ => {}
            }
        }
        let mut is_float = s[start] == b'.';
        let mut e = if is_float {
            self.lnum_end(start + 1)
        } else {
            self.lnum_end(start)
        };
        if !is_float && self.at(e) == b'.' {
            // {LNUM}"."{LNUM}? — "1." is a float even with nothing after the dot.
            is_float = true;
            e += 1;
            if self.at(e).is_ascii_digit() {
                e = self.lnum_end(e);
            }
        }
        if self.at(e) == b'e' || self.at(e) == b'E' {
            let mut q = e + 1;
            if self.at(q) == b'+' || self.at(q) == b'-' {
                q += 1;
            }
            if self.at(q).is_ascii_digit() {
                e = self.lnum_end(q);
                is_float = true;
            }
        }
        if is_float {
            tok(T_DNUMBER, start, e)
        } else {
            let id = self.classify_lnum(start, e);
            tok(id, start, e)
        }
    }

    /// `{LNUM}`: decimal, or octal with a leading `0`. Digits `8`/`9` cut an
    /// octal literal short (and throw "Invalid numeric literal"); a value that
    /// does not fit a signed 64-bit integer becomes `T_DNUMBER`.
    fn classify_lnum(&mut self, lo: usize, hi: usize) -> u16 {
        let s = self.src;
        let octal = s[lo] == b'0';
        let base: i64 = if octal { 8 } else { 10 };
        let mut v: i64 = 0;
        let mut overflow = false;
        for &d in &s[lo..hi] {
            if d == b'_' {
                continue;
            }
            if octal && d >= b'8' {
                self.exception = true;
                break;
            }
            if !overflow {
                match v
                    .checked_mul(base)
                    .and_then(|x| x.checked_add(i64::from(d - b'0')))
                {
                    Some(x) => v = x,
                    None => overflow = true,
                }
            }
        }
        if overflow {
            T_DNUMBER
        } else {
            T_LNUMBER
        }
    }

    /// Significant digits of a prefixed literal: underscores dropped, leading
    /// zeros skipped (the C code skips leading `0`/`_` and strips the rest).
    fn significant_digits(&self, lo: usize, hi: usize) -> (usize, u8) {
        let mut it = self.src[lo..hi]
            .iter()
            .copied()
            .filter(|&c| c != b'_')
            .skip_while(|&c| c == b'0');
        match it.next() {
            None => (0, 0),
            Some(first) => (1 + it.count(), first),
        }
    }

    /// `{HNUM}`: 16 hex digits fit when the first is `0..=7`.
    fn classify_hex(&self, lo: usize, hi: usize) -> u16 {
        let (n, first) = self.significant_digits(lo, hi);
        if n < 16 || (n == 16 && first <= b'7') {
            T_LNUMBER
        } else {
            T_DNUMBER
        }
    }

    /// `{BNUM}`: up to 63 significant bits fit.
    fn classify_bin(&self, lo: usize, hi: usize) -> u16 {
        let (n, _) = self.significant_digits(lo, hi);
        if n < 64 {
            T_LNUMBER
        } else {
            T_DNUMBER
        }
    }

    /// `{ONUM}`: `strtol` overflow, which is exactly "more than 21 significant
    /// octal digits" (21 sevens are `i64::MAX`).
    fn classify_oct(&self, lo: usize, hi: usize) -> u16 {
        let (n, _) = self.significant_digits(lo, hi);
        if n <= 21 {
            T_LNUMBER
        } else {
            T_DNUMBER
        }
    }

    /// `ST_VAR_OFFSET`: `[0]|([1-9][0-9]*)` and `{LNUM}|{HNUM}|{BNUM}|{ONUM}` both
    /// give `T_NUM_STRING`; only the (longest) extent matters.
    fn num_string_end(&self, start: usize) -> usize {
        let mut e = self.lnum_end(start);
        if self.src[start] == b'0' {
            let d = self.at(start + 2);
            let alt = match self.at(start + 1) {
                b'b' | b'B' if d == b'0' || d == b'1' => {
                    self.radix_end(start + 2, |c| c == b'0' || c == b'1')
                }
                b'o' | b'O' if (b'0'..=b'7').contains(&d) => {
                    self.radix_end(start + 2, |c| (b'0'..=b'7').contains(&c))
                }
                b'x' | b'X' if d.is_ascii_hexdigit() => {
                    self.radix_end(start + 2, |c| c.is_ascii_hexdigit())
                }
                _ => e,
            };
            e = e.max(alt);
        }
        e
    }

    // ----- quoted strings -----------------------------------------------------

    /// `b?[']`: `\` escapes any one byte; an unterminated string becomes a single
    /// `T_ENCAPSED_AND_WHITESPACE` running to the end of input.
    fn single_quoted(&mut self, start: usize, bprefix: usize) -> Tok {
        let s = self.src;
        let len = s.len();
        let content_lo = start + bprefix + 1;
        let mut q = content_lo;
        loop {
            if q >= len {
                return tok(T_ENCAPSED_AND_WHITESPACE, start, len);
            }
            match s[q] {
                b'\'' => {
                    q += 1;
                    break;
                }
                b'\\' => {
                    q += 1;
                    if q < len {
                        q += 1;
                    }
                }
                _ => q += 1,
            }
        }
        self.line += self.count_newlines(content_lo, q - 1);
        tok(T_CONSTANT_ENCAPSED_STRING, start, q)
    }

    /// `b?["]`: a `T_CONSTANT_ENCAPSED_STRING` when nothing interpolates,
    /// otherwise just the opening quote (`"` or `b"`, id `"`) and a switch to
    /// `ST_DOUBLE_QUOTES`, which rescans the body piecewise.
    fn double_quoted(&mut self, start: usize, bprefix: usize) -> Tok {
        let s = self.src;
        let len = s.len();
        let content_lo = start + bprefix + 1;
        let mut q = content_lo;
        while q < len {
            let c = s[q];
            q += 1;
            match c {
                b'"' => {
                    let (nl, exc) = self.escape_string_newlines(content_lo, q - 1);
                    self.line += nl;
                    self.exception |= exc;
                    return tok(T_CONSTANT_ENCAPSED_STRING, start, q);
                }
                b'$' => {
                    if is_label_start(self.at(q)) || self.at(q) == b'{' {
                        break;
                    }
                }
                b'{' => {
                    if self.at(q) == b'$' {
                        break;
                    }
                }
                b'\\' if q < len => q += 1,
                _ => {}
            }
        }
        self.state = State::DoubleQuotes;
        tok(ch(b'"'), start, content_lo)
    }

    /// Line accounting of `zend_scan_escape_string()`: plain newline counting,
    /// except that an invalid `\u{…}` escape throws and returns early, so
    /// newlines after it are *not* counted. Returns `(newlines, threw)`.
    pub(crate) fn escape_string_newlines(&self, lo: usize, hi: usize) -> (u32, bool) {
        let s = self.src;
        if hi.saturating_sub(lo) <= 1 {
            return (self.count_newlines(lo, hi), false);
        }
        let mut n = 0;
        let mut i = lo;
        while i < hi {
            let c = s[i];
            if c == b'\\' {
                i += 1;
                if i >= hi {
                    break;
                }
                let e = s[i];
                if e == b'u' && self.at(i + 1) == b'{' {
                    // \u{hex+}: the C loop scans hex digits until '}' or a
                    // non-hex byte, reading past the string if need be.
                    let mut j = i + 2;
                    let mut value: u64 = 0;
                    let mut digits = 0usize;
                    let valid = loop {
                        let d = self.at(j);
                        if d == b'}' {
                            break digits > 0;
                        }
                        if !d.is_ascii_hexdigit() {
                            break false;
                        }
                        let dv = (d as char).to_digit(16).unwrap_or(0) as u64;
                        value = value.saturating_mul(16).saturating_add(dv);
                        digits += 1;
                        j += 1;
                    };
                    if !valid || value > 0x10FFFF {
                        return (n, true);
                    }
                    i = j + 1;
                    continue;
                }
                if e == b'\n' || (e == b'\r' && self.at(i + 1) != b'\n') {
                    n += 1;
                }
                i += 1;
                continue;
            }
            if c == b'\n' || (c == b'\r' && self.at(i + 1) != b'\n') {
                n += 1;
            }
            i += 1;
        }
        (n, false)
    }

    // ----- ST_DOUBLE_QUOTES / ST_BACKQUOTE / ST_HEREDOC -----------------------

    /// The interpolation rules shared by the three string states, then the
    /// state's `{ANY_CHAR}` literal-run rule. `quote` is `"`, `` ` `` or 0 for
    /// heredoc (whose literal runs are scanned in `heredoc.rs`).
    fn lex_string_state(&mut self, start: usize, quote: u8) -> Tok {
        let s = self.src;
        let len = s.len();
        let c = s[start];
        if quote != 0 && c == quote {
            self.state = State::Scripting;
            return tok(ch(quote), start, start + 1);
        }
        if c == b'$' {
            if self.at(start + 1) == b'{' {
                self.push_state(State::LookingForVarname);
                return tok(T_DOLLAR_OPEN_CURLY_BRACES, start, start + 2);
            }
            let e = self.scan_label(start + 1);
            if e > start + 1 {
                // "$"{LABEL}"->"[a-zA-Z_\x80-\xff] and the "?->" twin: the operator and the
                // property are lexed next, in ST_LOOKING_FOR_PROPERTY.
                if (self.starts_with(e, b"->") && is_label_start(self.at(e + 2)))
                    || (self.starts_with(e, b"?->") && is_label_start(self.at(e + 3)))
                {
                    self.push_state(State::LookingForProperty);
                } else if self.at(e) == b'[' {
                    self.push_state(State::VarOffset);
                }
                return tok(T_VARIABLE, start, e);
            }
        }
        if c == b'{' && self.at(start + 1) == b'$' {
            self.push_state(State::Scripting);
            return tok(T_CURLY_OPEN, start, start + 1);
        }
        if quote == 0 {
            return self.lex_heredoc_body(start);
        }
        // {ANY_CHAR}: a literal run up to the closing quote or the next interpolation.
        let mut q = start + 1;
        if c == b'\\' && q < len {
            q += 1;
        }
        while q < len {
            let d = s[q];
            q += 1;
            if d == quote {
                q -= 1;
                break;
            }
            match d {
                b'$' => {
                    if is_label_start(self.at(q)) || self.at(q) == b'{' {
                        q -= 1;
                        break;
                    }
                }
                b'{' => {
                    if self.at(q) == b'$' {
                        q -= 1;
                        break;
                    }
                }
                b'\\' if q < len => q += 1,
                _ => {}
            }
        }
        let (nl, exc) = self.escape_string_newlines(start, q);
        self.line += nl;
        self.exception |= exc;
        tok(T_ENCAPSED_AND_WHITESPACE, start, q)
    }

    // ----- ST_LOOKING_FOR_PROPERTY / ST_LOOKING_FOR_VARNAME / ST_VAR_OFFSET ---

    /// After `->` / `?->`: whitespace and comments are allowed, a label is a
    /// `T_STRING`, anything else pops the state and is rescanned.
    fn lex_looking_for_property(&mut self, start: usize) -> Option<Tok> {
        let c = self.src[start];
        let n1 = self.at(start + 1);
        let t = match c {
            b' ' | b'\t' | b'\n' | b'\r' => self.whitespace(start),
            b'-' if n1 == b'>' => tok(T_OBJECT_OPERATOR, start, start + 2),
            b'?' if self.starts_with(start + 1, b"->") => {
                tok(T_NULLSAFE_OBJECT_OPERATOR, start, start + 3)
            }
            b'#' => self.line_comment(start, 1),
            b'/' if n1 == b'/' => self.line_comment(start, 2),
            b'/' if n1 == b'*' => self.block_comment(start),
            _ if is_label_start(c) => {
                let e = self.scan_label(start);
                self.pop_state();
                tok(T_STRING, start, e)
            }
            _ => {
                self.pop_state();
                return None;
            }
        };
        Some(t)
    }

    /// After `${`: `{LABEL}[[}]` is a `T_STRING_VARNAME`; either way the
    /// string state is left on the stack and scripting resumes.
    fn lex_looking_for_varname(&mut self, start: usize) -> Option<Tok> {
        self.pop_state();
        self.push_state(State::Scripting);
        if is_label_start(self.src[start]) {
            let e = self.scan_label(start);
            if matches!(self.at(e), b'[' | b'}') {
                return Some(tok(T_STRING_VARNAME, start, e));
            }
        }
        None
    }

    /// Inside `"$a[…]"`.
    fn lex_var_offset(&mut self, start: usize) -> Tok {
        let c = self.src[start];
        match c {
            b']' => {
                self.pop_state();
                tok(ch(b']'), start, start + 1)
            }
            b'$' => {
                let e = self.scan_label(start + 1);
                if e > start + 1 {
                    tok(T_VARIABLE, start, e)
                } else {
                    tok(ch(b'$'), start, start + 1)
                }
            }
            b'0'..=b'9' => {
                let e = self.num_string_end(start);
                tok(T_NUM_STRING, start, e)
            }
            _ if is_label_start(c) => {
                let e = self.scan_label(start);
                tok(T_STRING, start, e)
            }
            // "Invalid rule to return a more explicit parse error": a zero-length
            // T_ENCAPSED_AND_WHITESPACE, then back to the enclosing string state.
            b' ' | b'\n' | b'\r' | b'\t' | b'\\' | b'\'' | b'#' => {
                self.pop_state();
                tok(T_ENCAPSED_AND_WHITESPACE, start, start)
            }
            b';' | b':' | b',' | b'.' | b'|' | b'^' | b'&' | b'+' | b'-' | b'/' | b'*' | b'='
            | b'%' | b'!' | b'~' | b'<' | b'>' | b'?' | b'@' | b'[' | b'(' | b')' | b'{' | b'}'
            | b'"' | b'`' => tok(ch(c), start, start + 1),
            _ => tok(T_BAD_CHARACTER, start, start + 1),
        }
    }
}
