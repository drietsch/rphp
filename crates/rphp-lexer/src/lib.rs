//! Byte-level lexer for the M0 PHP subset.
//!
//! Operates on `&[u8]` (PHP source is bytes). It scans the `<?php ... ?>` code
//! island, skips whitespace and `//`/`#`/`/* */` comments, and emits a flat
//! token stream. Identifiers are interned eagerly into [`IdentId`]. The full
//! lexer (heredoc/nowdoc, interpolation, attributes, `|>`, inline HTML output)
//! is added later; this is the contract the parser builds against.
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
    True,
    False,
    Null,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TokenKind {
    Eof,
    // literals
    Int(i64),
    Float(f64),
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
            } else if c.is_ascii_digit() || (c == b'.' && self.peek_at(1).map_or(false, |d| d.is_ascii_digit())) {
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

    fn lex_number(&mut self, start: usize) {
        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'_' {
                self.pos += 1;
            } else if c == b'.' && !is_float && self.peek_at(1).map_or(true, |d| d != b'.') {
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
            b'-' => one!(TokenKind::Minus),
            b'%' => one!(TokenKind::Percent),
            b'/' => one!(TokenKind::Slash),
            b'(' => one!(TokenKind::LParen),
            b')' => one!(TokenKind::RParen),
            b'{' => one!(TokenKind::LBrace),
            b'}' => one!(TokenKind::RBrace),
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
        b"true" => Kw::True,
        b"false" => Kw::False,
        b"null" => Kw::Null,
        _ => return None,
    })
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
}
