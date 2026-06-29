//! Byte-level lexer for the PHP subset implemented so far.
//!
//! Operates on `&[u8]` (PHP source is bytes). It scans the `<?php ... ?>` code
//! island, skips whitespace and `//`/`#`/`/* */` comments, and emits a flat
//! token stream. Identifiers and string-literal bytes are interned eagerly into
//! [`IdentId`].
//!
//! ## String literals
//! Single-quoted strings (`'...'`, escapes `\\` and `\'`) and double-quoted
//! strings (`"..."`, C-style escapes) become a single [`TokenKind::Str`] holding
//! the interned final bytes. A double-quoted string that contains simple
//! `$var` / `{$var}` interpolation is emitted as a bracketed run —
//! [`TokenKind::DQStrBegin`], then alternating `Str` literal pieces and
//! `Variable` tokens, then [`TokenKind::DQStrEnd`] — which the parser folds into
//! a concatenation. Heredoc/nowdoc and complex interpolation (`{$expr}`,
//! `$a[k]`, `$a->b`) are added later.
#![forbid(unsafe_code)]

use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::{FileId, Span};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kw {
    If,
    Else,
    While,
    Echo,
    Return,
    Function,
    Fn,
    Use,
    True,
    False,
    Null,
    Array,
    Foreach,
    As,
    Class,
    New,
    Public,
    Private,
    Protected,
    Extends,
    Instanceof,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TokenKind {
    Eof,
    // literals
    Int(i64),
    Float(f64),
    Str(IdentId), // 'single' or "double" literal — interned final bytes
    // A double-quoted string with interpolation expands to:
    //   DQStrBegin (Str | Variable)* DQStrEnd
    DQStrBegin,
    DQStrEnd,
    // names
    Variable(IdentId), // $name
    Ident(IdentId),    // bareword (function name, etc.)
    Keyword(Kw),
    // operators / punctuation
    Plus,
    Minus,
    Star,
    StarStar, // **
    Slash,
    Percent,
    Dot, // . (concatenation)
    Assign,       // =
    EqEq,         // ==
    EqEqEq,       // ===
    BangEq,       // !=
    BangEqEq,     // !==
    Lt,
    Le,
    Gt,
    Ge,
    Spaceship, // <=>
    AmpAmp,    // &&
    PipePipe,  // ||
    Bang,      // !
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,    // [
    RBracket,    // ]
    DoubleArrow, // =>
    Arrow,       // -> (object member access)
    DoubleColon, // :: (scope resolution)
    Semicolon,
    Comma,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lex a full source buffer. Requires a `<?php` open tag; bytes before it and
/// after a closing `?>` are ignored in this M0 slice.
pub fn lex(src: &[u8], file: FileId, interner: &mut Interner) -> LexResult {
    let mut lx = Lexer { src, file, interner, pos: 0, tokens: Vec::new(), diags: Vec::new() };
    lx.run();
    LexResult { tokens: lx.tokens, diagnostics: lx.diags }
}

struct Lexer<'a> {
    src: &'a [u8],
    file: FileId,
    interner: &'a mut Interner,
    pos: usize,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn run(&mut self) {
        // Skip to `<?php` (or `<?`). Inline HTML before it is ignored in M0.
        self.skip_to_open_tag();
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.peek() else {
                self.push(TokenKind::Eof, start);
                break;
            };
            // Closing tag ends the code island for M0.
            if c == b'?' && self.peek_at(1) == Some(b'>') {
                self.pos += 2;
                self.push(TokenKind::Eof, start);
                break;
            }
            if c == b'$' {
                self.lex_variable(start);
            } else if c == b'\'' {
                self.lex_single_quoted(start);
            } else if c == b'"' {
                self.lex_double_quoted(start);
            } else if c.is_ascii_digit() || (c == b'.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) {
                self.lex_number(start);
            } else if is_ident_start(c) {
                self.lex_ident_or_keyword(start);
            } else {
                self.lex_operator(start);
            }
        }
    }

    fn skip_to_open_tag(&mut self) {
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'<'
                && self.peek_at(1) == Some(b'?')
            {
                // accept `<?php` or `<?`
                if self.src[self.pos + 2..].starts_with(b"php") {
                    self.pos += 5;
                } else {
                    self.pos += 2;
                }
                return;
            }
            self.pos += 1;
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => self.pos += 1,
                Some(b'#') => self.skip_line_comment(),
                Some(b'/') if self.peek_at(1) == Some(b'/') => self.skip_line_comment(),
                Some(b'/') if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
                _ => return,
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == b'\n' {
                break;
            }
            self.pos += 1;
        }
    }

    fn skip_block_comment(&mut self) {
        self.pos += 2;
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'*' && self.peek_at(1) == Some(b'/') {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
    }

    fn lex_variable(&mut self, start: usize) {
        self.pos += 1; // consume '$'
        let name_start = self.pos;
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let id = self.interner.intern(&self.src[name_start..self.pos]);
        self.push(TokenKind::Variable(id), start);
    }

    /// `'...'` — only `\\` and `\'` are escapes; every other byte (including a
    /// literal newline or a `\n` sequence) is taken verbatim.
    fn lex_single_quoted(&mut self, start: usize) {
        self.pos += 1; // opening quote
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                self.unterminated_string(start);
                break;
            };
            match c {
                b'\'' => {
                    self.pos += 1;
                    break;
                }
                b'\\' => match self.peek_at(1) {
                    Some(b'\'') => {
                        buf.push(b'\'');
                        self.pos += 2;
                    }
                    Some(b'\\') => {
                        buf.push(b'\\');
                        self.pos += 2;
                    }
                    _ => {
                        buf.push(b'\\');
                        self.pos += 1;
                    }
                },
                _ => {
                    buf.push(c);
                    self.pos += 1;
                }
            }
        }
        let id = self.interner.intern(&buf);
        self.push(TokenKind::Str(id), start);
    }

    /// `"..."` — C-style escapes plus simple `$var` / `{$var}` interpolation.
    fn lex_double_quoted(&mut self, start: usize) {
        self.pos += 1; // opening quote
        let mut parts: Vec<DqPart> = Vec::new();
        let mut lit: Vec<u8> = Vec::new();
        let flush = |lit: &mut Vec<u8>, parts: &mut Vec<DqPart>| {
            if !lit.is_empty() {
                parts.push(DqPart::Lit(std::mem::take(lit)));
            }
        };
        loop {
            let Some(c) = self.peek() else {
                self.unterminated_string(start);
                break;
            };
            match c {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\\' => {
                    self.pos += 1; // consume backslash
                    self.lex_dq_escape(&mut lit);
                }
                // Simple `$name` interpolation.
                b'$' if self.peek_at(1).is_some_and(is_ident_start) => {
                    flush(&mut lit, &mut parts);
                    self.pos += 1; // `$`
                    let name_start = self.pos;
                    while self.peek().is_some_and(is_ident_continue) {
                        self.pos += 1;
                    }
                    let id = self.interner.intern(&self.src[name_start..self.pos]);
                    parts.push(DqPart::Var(id));
                }
                // `{$name}` interpolation (simple variable only for now).
                b'{' if self.peek_at(1) == Some(b'$') => {
                    if let Some((id, end)) = self.try_brace_var() {
                        flush(&mut lit, &mut parts);
                        parts.push(DqPart::Var(id));
                        self.pos = end;
                    } else {
                        // Not a simple `{$name}`: keep `{` literal, defer the
                        // complex form. The `$...` after it is handled normally.
                        lit.push(b'{');
                        self.pos += 1;
                    }
                }
                _ => {
                    lit.push(c);
                    self.pos += 1;
                }
            }
        }
        flush(&mut lit, &mut parts);

        let has_var = parts.iter().any(|p| matches!(p, DqPart::Var(_)));
        if !has_var {
            // No interpolation: a single string token (empty `""` included).
            let mut all = Vec::new();
            for p in &parts {
                if let DqPart::Lit(b) = p {
                    all.extend_from_slice(b);
                }
            }
            let id = self.interner.intern(&all);
            self.push(TokenKind::Str(id), start);
        } else {
            self.push(TokenKind::DQStrBegin, start);
            for p in parts {
                match p {
                    DqPart::Lit(b) => {
                        let id = self.interner.intern(&b);
                        self.push(TokenKind::Str(id), start);
                    }
                    DqPart::Var(id) => self.push(TokenKind::Variable(id), start),
                }
            }
            self.push(TokenKind::DQStrEnd, start);
        }
    }

    /// Try to read `{$name}` starting at the current `{` (with `$` next). On
    /// success interns the name and returns it with the index just past the
    /// closing `}`; otherwise `None` (the caller keeps `{` literal). Leaves
    /// `self.pos` unchanged.
    fn try_brace_var(&mut self) -> Option<(IdentId, usize)> {
        let name_start = self.pos + 2; // past `{$`
        let mut j = name_start;
        if !self.src.get(j).copied().is_some_and(is_ident_start) {
            return None;
        }
        while self.src.get(j).copied().is_some_and(is_ident_continue) {
            j += 1;
        }
        if self.src.get(j) != Some(&b'}') {
            return None;
        }
        let id = self.interner.intern(&self.src[name_start..j]);
        Some((id, j + 1))
    }

    /// Decode the escape sequence following a `\` (already consumed) in a
    /// double-quoted string, appending the decoded byte(s) to `out`.
    fn lex_dq_escape(&mut self, out: &mut Vec<u8>) {
        let Some(c) = self.peek() else {
            out.push(b'\\'); // trailing backslash at EOF
            return;
        };
        match c {
            b'n' => self.escape_byte(out, b'\n'),
            b'r' => self.escape_byte(out, b'\r'),
            b't' => self.escape_byte(out, b'\t'),
            b'v' => self.escape_byte(out, 0x0b),
            b'f' => self.escape_byte(out, 0x0c),
            b'e' => self.escape_byte(out, 0x1b),
            b'\\' => self.escape_byte(out, b'\\'),
            b'$' => self.escape_byte(out, b'$'),
            b'"' => self.escape_byte(out, b'"'),
            b'0'..=b'7' => {
                let mut val: u32 = 0;
                let mut count = 0;
                while count < 3 {
                    match self.peek() {
                        Some(d @ b'0'..=b'7') => {
                            val = val * 8 + (d - b'0') as u32;
                            self.pos += 1;
                            count += 1;
                        }
                        _ => break,
                    }
                }
                out.push(val as u8);
            }
            b'x' => {
                self.pos += 1; // consume `x`
                let mut val: u32 = 0;
                let mut count = 0;
                while count < 2 {
                    match self.peek().and_then(hex_val) {
                        Some(h) => {
                            val = val * 16 + h;
                            self.pos += 1;
                            count += 1;
                        }
                        None => break,
                    }
                }
                if count == 0 {
                    out.extend_from_slice(b"\\x"); // not a hex escape after all
                } else {
                    out.push(val as u8);
                }
            }
            b'u' if self.peek_at(1) == Some(b'{') => {
                let mut j = self.pos + 2; // past `u{`
                let mut val: u32 = 0;
                let mut count = 0;
                while let Some(h) = self.src.get(j).copied().and_then(hex_val) {
                    val = val * 16 + h;
                    j += 1;
                    count += 1;
                }
                if count > 0 && self.src.get(j) == Some(&b'}') {
                    self.pos = j + 1;
                    match char::from_u32(val) {
                        Some(ch) => {
                            let mut tmp = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                        }
                        None => {
                            let span = self.span_from(self.pos);
                            self.diags.push(
                                Diagnostic::error(codes::UNEXPECTED_CHAR, "invalid \\u{} code point")
                                    .with_primary(span, "here"),
                            );
                        }
                    }
                } else {
                    // Malformed: keep `\u` literal and reprocess from `{`.
                    out.extend_from_slice(b"\\u");
                    self.pos += 1; // now at `{`
                }
            }
            _ => {
                // Unknown escape: PHP keeps both the backslash and the char.
                out.push(b'\\');
                out.push(c);
                self.pos += 1;
            }
        }
    }

    #[inline]
    fn escape_byte(&mut self, out: &mut Vec<u8>, b: u8) {
        out.push(b);
        self.pos += 1;
    }

    fn unterminated_string(&mut self, start: usize) {
        let span = self.span_from(start);
        self.diags.push(
            Diagnostic::error(codes::UNTERMINATED, "unterminated string literal")
                .with_primary(span, "string starts here"),
        );
    }

    fn lex_number(&mut self, start: usize) {
        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'_' {
                self.pos += 1;
            } else if c == b'.' && !is_float && self.peek_at(1).is_none_or(|d| d != b'.') {
                is_float = true;
                self.pos += 1;
            } else if c == b'e' || c == b'E' {
                is_float = true;
                self.pos += 1;
                if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        let raw: String = self.src[start..self.pos]
            .iter()
            .filter(|&&b| b != b'_')
            .map(|&b| b as char)
            .collect();
        let kind = if is_float {
            TokenKind::Float(raw.parse::<f64>().unwrap_or(0.0))
        } else {
            match raw.parse::<i64>() {
                Ok(i) => TokenKind::Int(i),
                // overflowing int literal becomes float, as PHP does
                Err(_) => TokenKind::Float(raw.parse::<f64>().unwrap_or(0.0)),
            }
        };
        self.push(kind, start);
    }

    fn lex_ident_or_keyword(&mut self, start: usize) {
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let word = &self.src[start..self.pos];
        let kind = match keyword(word) {
            Some(kw) => TokenKind::Keyword(kw),
            None => TokenKind::Ident(self.interner.intern(word)),
        };
        self.push(kind, start);
    }

    fn lex_operator(&mut self, start: usize) {
        let c = self.src[self.pos];
        macro_rules! one {
            ($k:expr) => {{
                self.pos += 1;
                self.push($k, start);
                return;
            }};
        }
        match c {
            b'+' => one!(TokenKind::Plus),
            b'-' => {
                if self.peek_at(1) == Some(b'>') {
                    self.pos += 2;
                    self.push(TokenKind::Arrow, start);
                } else {
                    one!(TokenKind::Minus);
                }
            }
            b'%' => one!(TokenKind::Percent),
            b':' if self.peek_at(1) == Some(b':') => {
                self.pos += 2;
                self.push(TokenKind::DoubleColon, start);
            }
            b'/' => one!(TokenKind::Slash),
            // A `.` adjacent to a digit was already routed to `lex_number`, so a
            // `.` reaching here is always the concatenation operator.
            b'.' => one!(TokenKind::Dot),
            b'(' => one!(TokenKind::LParen),
            b')' => one!(TokenKind::RParen),
            b'{' => one!(TokenKind::LBrace),
            b'}' => one!(TokenKind::RBrace),
            b'[' => one!(TokenKind::LBracket),
            b']' => one!(TokenKind::RBracket),
            b';' => one!(TokenKind::Semicolon),
            b',' => one!(TokenKind::Comma),
            b'*' => {
                if self.peek_at(1) == Some(b'*') {
                    self.pos += 2;
                    self.push(TokenKind::StarStar, start);
                } else {
                    one!(TokenKind::Star);
                }
            }
            b'=' => {
                if self.starts_with(b"===") {
                    self.pos += 3;
                    self.push(TokenKind::EqEqEq, start);
                } else if self.starts_with(b"==") {
                    self.pos += 2;
                    self.push(TokenKind::EqEq, start);
                } else if self.starts_with(b"=>") {
                    self.pos += 2;
                    self.push(TokenKind::DoubleArrow, start);
                } else {
                    one!(TokenKind::Assign);
                }
            }
            b'!' => {
                if self.starts_with(b"!==") {
                    self.pos += 3;
                    self.push(TokenKind::BangEqEq, start);
                } else if self.starts_with(b"!=") {
                    self.pos += 2;
                    self.push(TokenKind::BangEq, start);
                } else {
                    one!(TokenKind::Bang);
                }
            }
            b'<' => {
                if self.starts_with(b"<=>") {
                    self.pos += 3;
                    self.push(TokenKind::Spaceship, start);
                } else if self.starts_with(b"<=") {
                    self.pos += 2;
                    self.push(TokenKind::Le, start);
                } else {
                    one!(TokenKind::Lt);
                }
            }
            b'>' => {
                if self.starts_with(b">=") {
                    self.pos += 2;
                    self.push(TokenKind::Ge, start);
                } else {
                    one!(TokenKind::Gt);
                }
            }
            b'&' if self.peek_at(1) == Some(b'&') => {
                self.pos += 2;
                self.push(TokenKind::AmpAmp, start);
            }
            b'|' if self.peek_at(1) == Some(b'|') => {
                self.pos += 2;
                self.push(TokenKind::PipePipe, start);
            }
            _ => {
                // Unknown byte: emit a diagnostic and skip it (recoverable).
                self.pos += 1;
                let span = self.span_from(start);
                self.diags.push(
                    Diagnostic::error(codes::UNEXPECTED_CHAR, format!("unexpected character {:?}", c as char))
                        .with_primary(span, "here"),
                );
            }
        }
    }

    // ----- helpers -----

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    fn starts_with(&self, pat: &[u8]) -> bool {
        self.src[self.pos..].starts_with(pat)
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(self.file, start as u32, self.pos as u32)
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        let span = self.span_from(start);
        self.tokens.push(Token { kind, span });
    }
}

fn keyword(word: &[u8]) -> Option<Kw> {
    // Keywords are ASCII-case-insensitive in PHP.
    let lower: Vec<u8> = word.iter().map(|b| b.to_ascii_lowercase()).collect();
    Some(match lower.as_slice() {
        b"if" => Kw::If,
        b"else" => Kw::Else,
        b"while" => Kw::While,
        b"echo" => Kw::Echo,
        b"return" => Kw::Return,
        b"function" => Kw::Function,
        b"fn" => Kw::Fn,
        b"use" => Kw::Use,
        b"true" => Kw::True,
        b"false" => Kw::False,
        b"null" => Kw::Null,
        b"array" => Kw::Array,
        b"foreach" => Kw::Foreach,
        b"as" => Kw::As,
        b"class" => Kw::Class,
        b"new" => Kw::New,
        b"public" => Kw::Public,
        b"private" => Kw::Private,
        b"protected" => Kw::Protected,
        b"extends" => Kw::Extends,
        b"instanceof" => Kw::Instanceof,
        _ => return None,
    })
}

/// A piece of a double-quoted string while scanning: either a run of literal
/// (escape-decoded) bytes or an interpolated variable name.
enum DqPart {
    Lit(Vec<u8>),
    Var(IdentId),
}

/// Hex-digit value, or `None` if `c` is not `[0-9a-fA-F]`.
fn hex_val(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic() || c >= 0x80
}

fn is_ident_continue(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &[u8]) -> Vec<TokenKind> {
        let mut i = Interner::new();
        lex(src, FileId(0), &mut i).tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_assignment_and_arith() {
        let k = kinds(b"<?php $x = 1 + 2.5 ** 3;");
        assert!(matches!(k[0], TokenKind::Variable(_)));
        assert!(matches!(k[1], TokenKind::Assign));
        assert!(matches!(k[2], TokenKind::Int(1)));
        assert!(matches!(k[3], TokenKind::Plus));
        assert!(matches!(k[4], TokenKind::Float(_)));
        assert!(matches!(k[5], TokenKind::StarStar));
        assert!(matches!(k[6], TokenKind::Int(3)));
        assert!(matches!(k[7], TokenKind::Semicolon));
        assert!(matches!(k.last(), Some(TokenKind::Eof)));
    }

    #[test]
    fn multichar_operators_and_keywords() {
        let k = kinds(b"<?php if ($a === 1 <=> 2) echo true;");
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::If))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::EqEqEq)));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Spaceship)));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Echo))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::True))));
    }

    #[test]
    fn skips_comments() {
        let k = kinds(b"<?php // line\n# hash\n/* block */ $x;");
        assert!(matches!(k[0], TokenKind::Variable(_)));
    }

    /// Lex `src` and resolve any `Str`/`Variable` token bytes for assertions.
    fn lex_with(src: &[u8]) -> (Vec<TokenKind>, Interner) {
        let mut i = Interner::new();
        let toks = lex(src, FileId(0), &mut i).tokens.into_iter().map(|t| t.kind).collect();
        (toks, i)
    }

    #[test]
    fn single_quoted_string_keeps_escapes_literal() {
        let (k, i) = lex_with(br"<?php 'a\n\'b\\';");
        match k[0] {
            // \n stays literal, \' -> ', \\ -> \
            TokenKind::Str(id) => assert_eq!(i.resolve(id), br"a\n'b\"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn double_quoted_string_decodes_escapes() {
        let (k, i) = lex_with(br#"<?php "tab\there\n\x41\u{2764}";"#);
        match k[0] {
            TokenKind::Str(id) => {
                assert_eq!(i.resolve(id), "tab\there\n\u{41}\u{2764}".as_bytes());
            }
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn double_quoted_without_interp_is_single_token() {
        let (k, _) = lex_with(br#"<?php "plain text";"#);
        assert!(matches!(k[0], TokenKind::Str(_)));
        assert!(matches!(k[1], TokenKind::Semicolon));
    }

    #[test]
    fn interpolation_expands_to_bracketed_run() {
        // "a $x b {$y}" -> Begin, "a ", $x, " b ", $y, End
        let (k, i) = lex_with(br#"<?php "a $x b {$y}";"#);
        assert!(matches!(k[0], TokenKind::DQStrBegin));
        match k[1] {
            TokenKind::Str(id) => assert_eq!(i.resolve(id), b"a "),
            other => panic!("expected literal piece, got {other:?}"),
        }
        match k[2] {
            TokenKind::Variable(id) => assert_eq!(i.resolve(id), b"x"),
            other => panic!("expected $x, got {other:?}"),
        }
        match k[3] {
            TokenKind::Str(id) => assert_eq!(i.resolve(id), b" b "),
            other => panic!("expected literal piece, got {other:?}"),
        }
        match k[4] {
            TokenKind::Variable(id) => assert_eq!(i.resolve(id), b"y"),
            other => panic!("expected {{$y}}, got {other:?}"),
        }
        assert!(matches!(k[5], TokenKind::DQStrEnd));
    }

    #[test]
    fn escaped_dollar_is_not_interpolation() {
        let (k, i) = lex_with(br#"<?php "price \$5";"#);
        match k[0] {
            TokenKind::Str(id) => assert_eq!(i.resolve(id), b"price $5"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn dot_is_concatenation_operator() {
        let k: Vec<_> = lex_with(br#"<?php $x . "y";"#).0;
        assert!(matches!(k[0], TokenKind::Variable(_)));
        assert!(matches!(k[1], TokenKind::Dot));
        assert!(matches!(k[2], TokenKind::Str(_)));
    }

    #[test]
    fn float_dot_still_lexes_as_number() {
        let (k, _) = lex_with(b"<?php 1.5 . 2;");
        assert!(matches!(k[0], TokenKind::Float(_)));
        assert!(matches!(k[1], TokenKind::Dot));
        assert!(matches!(k[2], TokenKind::Int(2)));
    }

    #[test]
    fn unterminated_string_diagnoses() {
        let mut i = Interner::new();
        let r = lex(b"<?php 'oops", FileId(0), &mut i);
        assert!(!r.diagnostics.is_empty());
    }

    #[test]
    fn array_tokens_and_keywords() {
        let k = kinds(b"<?php foreach (array(1) as $k => $v) {} $a[0];");
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Foreach))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Array))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::As))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::DoubleArrow)));
        assert!(k.iter().any(|t| matches!(t, TokenKind::LBracket)));
        assert!(k.iter().any(|t| matches!(t, TokenKind::RBracket)));
    }

    #[test]
    fn double_arrow_vs_equals() {
        let k = kinds(b"<?php $a = 1; $b => 2;");
        assert!(k.iter().any(|t| matches!(t, TokenKind::Assign)));
        assert!(k.iter().any(|t| matches!(t, TokenKind::DoubleArrow)));
    }

    #[test]
    fn class_keywords_and_arrow() {
        let k = kinds(b"<?php class C { public $x; } $o = new C(); $o->x;");
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Class))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Public))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::New))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Arrow)));
    }

    #[test]
    fn inheritance_keywords_and_double_colon() {
        let k = kinds(b"<?php class B extends A { } $x instanceof A; parent::m();");
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Extends))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::Keyword(Kw::Instanceof))));
        assert!(k.iter().any(|t| matches!(t, TokenKind::DoubleColon)));
    }

    #[test]
    fn arrow_is_distinct_from_minus_and_greater() {
        // `->` is one token; `- >` (with a space) is minus then greater-than.
        let arrow = kinds(b"<?php $a->b;");
        assert!(arrow.iter().any(|t| matches!(t, TokenKind::Arrow)));
        assert!(!arrow.iter().any(|t| matches!(t, TokenKind::Minus)));
        let minus = kinds(b"<?php $a - 1;");
        assert!(minus.iter().any(|t| matches!(t, TokenKind::Minus)));
        assert!(!minus.iter().any(|t| matches!(t, TokenKind::Arrow)));
    }
}
