//! Recursive-descent + Pratt parser: tokens -> `rphp-ast` for the M0 subset.
//!
//! The parser calls [`rphp_lexer::lex`] to obtain a flat token stream, then
//! builds a typed [`Program`]. Expression parsing uses precedence climbing
//! (a Pratt-style loop) for the binary operators, with `**` and the prefix
//! unary operators handled by dedicated rungs so that `**` binds tighter than
//! unary minus (`-2 ** 2` == `-(2 ** 2)`) and stays right-associative.
//!
//! Parsing is recoverable: an unexpected token pushes a [`Diagnostic`]
//! (`codes::UNEXPECTED_TOKEN`) at the offending span and the parser
//! synchronizes to the next `;` or `}` rather than aborting. The lexer's own
//! diagnostics are forwarded too, so the returned `Vec<Diagnostic>` is the full
//! picture and the returned `Program` may be partial.
#![forbid(unsafe_code)]

use rphp_ast::{BinOp, Expr, Func, Param, Program, Stmt, UnOp};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_lexer::{lex, Kw, LexResult, Token, TokenKind};
use rphp_span::{FileId, Span};

/// Parse a full source buffer into a [`Program`] plus any diagnostics.
/// Recoverable: returns a (possibly partial) program even when diagnostics
/// are non-empty.
pub fn parse(src: &[u8], file: FileId, interner: &mut Interner) -> (Program, Vec<Diagnostic>) {
    let LexResult { tokens, diagnostics } = lex(src, file, interner);
    let mut parser = Parser { tokens, pos: 0, diags: diagnostics, interner };
    let program = parser.parse_program();
    (program, parser.diags)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
    interner: &'a mut Interner,
}

impl<'a> Parser<'a> {
    // ----- token cursor helpers -----

    fn peek(&self) -> TokenKind {
        self.tokens[self.pos].kind
    }

    fn peek_tok(&self) -> Token {
        self.tokens[self.pos]
    }

    fn cur_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    /// Span of the most recently consumed token (or the first token's span).
    fn prev_span(&self) -> Span {
        let i = self.pos.saturating_sub(1);
        self.tokens[i].span
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek() == kind
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    /// Consume and return the current token, clamping at `Eof`.
    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos];
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    /// Consume the current token if it matches `kind`.
    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume `kind` or emit a diagnostic (without consuming) and return false.
    fn expect(&mut self, kind: TokenKind, msg: &str) -> bool {
        if self.eat(kind) {
            true
        } else {
            let span = self.cur_span();
            self.error(span, msg);
            false
        }
    }

    // ----- diagnostics / recovery -----

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(codes::UNEXPECTED_TOKEN, msg).with_primary(span, "here"));
    }

    /// Skip tokens until just past the next `;`, or up to (but not consuming) a
    /// `}` / `Eof`. Used to resynchronize after a malformed statement.
    fn synchronize(&mut self) {
        while !self.at_eof() {
            match self.peek() {
                TokenKind::Semicolon => {
                    self.advance();
                    return;
                }
                TokenKind::RBrace => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// Expect a statement-terminating `;`, recovering on mismatch.
    fn expect_semi(&mut self) {
        if !self.eat(TokenKind::Semicolon) {
            let span = self.cur_span();
            self.error(span, "expected `;`");
            self.synchronize();
        }
    }

    /// An interned placeholder identifier for error recovery (malformed names).
    fn error_ident(&mut self) -> IdentId {
        self.interner.intern(b"")
    }

    // ----- top level -----

    fn parse_program(&mut self) -> Program {
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            if let Some(stmt) = self.parse_stmt() {
                items.push(stmt);
            }
            if self.pos == before {
                // No progress: a stray token (e.g. `}`) at the top level.
                let span = self.cur_span();
                self.error(span, "unexpected token");
                self.advance();
            }
        }
        Program { items }
    }

    // ----- statements -----

    fn parse_stmt(&mut self) -> Option<Stmt> {
        let tok = self.peek_tok();
        match tok.kind {
            TokenKind::Keyword(Kw::Echo) => Some(self.parse_echo(tok.span)),
            TokenKind::Keyword(Kw::If) => Some(self.parse_if(tok.span)),
            TokenKind::Keyword(Kw::While) => Some(self.parse_while(tok.span)),
            TokenKind::Keyword(Kw::Return) => Some(self.parse_return(tok.span)),
            TokenKind::Keyword(Kw::Function) => Some(self.parse_function(tok.span)),
            // Empty statement.
            TokenKind::Semicolon => {
                self.advance();
                None
            }
            // Delimiters handled by the caller (block / program loop).
            TokenKind::RBrace | TokenKind::Eof => None,
            // Otherwise an expression statement.
            _ => {
                let expr = self.parse_expr();
                self.expect_semi();
                Some(Stmt::Expr(expr))
            }
        }
    }

    /// `{ stmt* }`
    fn parse_block(&mut self) -> Vec<Stmt> {
        self.expect(TokenKind::LBrace, "expected `{`");
        let mut stmts = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            if let Some(stmt) = self.parse_stmt() {
                stmts.push(stmt);
            }
            if self.pos == before {
                // Guarantee forward progress on unparseable input.
                self.advance();
            }
        }
        self.expect(TokenKind::RBrace, "expected `}`");
        stmts
    }

    /// A brace block, or a single statement for braceless `if`/`while` bodies.
    fn parse_block_or_stmt(&mut self) -> Vec<Stmt> {
        if self.at(TokenKind::LBrace) {
            self.parse_block()
        } else {
            match self.parse_stmt() {
                Some(stmt) => vec![stmt],
                None => Vec::new(),
            }
        }
    }

    fn parse_echo(&mut self, kw_span: Span) -> Stmt {
        self.advance(); // `echo`
        let mut args = vec![self.parse_expr()];
        while self.eat(TokenKind::Comma) {
            args.push(self.parse_expr());
        }
        let end = args.last().map(|e| e.span()).unwrap_or(kw_span);
        self.expect_semi();
        Stmt::Echo { args, span: kw_span.to(end) }
    }

    fn parse_return(&mut self, kw_span: Span) -> Stmt {
        self.advance(); // `return`
        let value = if self.at(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr())
        };
        let end = value.as_ref().map(|e| e.span()).unwrap_or(kw_span);
        self.expect_semi();
        Stmt::Return { value, span: kw_span.to(end) }
    }

    fn parse_if(&mut self, kw_span: Span) -> Stmt {
        self.advance(); // `if`
        self.expect(TokenKind::LParen, "expected `(` after `if`");
        let cond = self.parse_expr();
        self.expect(TokenKind::RParen, "expected `)` after condition");
        let then_branch = self.parse_block_or_stmt();

        let mut else_branch = Vec::new();
        if self.at(TokenKind::Keyword(Kw::Else)) {
            self.advance(); // `else`
            if self.at(TokenKind::Keyword(Kw::If)) {
                // `else if` -> a nested `if` statement.
                let nested_span = self.cur_span();
                else_branch = vec![self.parse_if(nested_span)];
            } else {
                else_branch = self.parse_block_or_stmt();
            }
        }

        let span = kw_span.to(self.prev_span());
        Stmt::If { cond, then_branch, else_branch, span }
    }

    fn parse_while(&mut self, kw_span: Span) -> Stmt {
        self.advance(); // `while`
        self.expect(TokenKind::LParen, "expected `(` after `while`");
        let cond = self.parse_expr();
        self.expect(TokenKind::RParen, "expected `)` after condition");
        let body = self.parse_block_or_stmt();
        let span = kw_span.to(self.prev_span());
        Stmt::While { cond, body, span }
    }

    fn parse_function(&mut self, kw_span: Span) -> Stmt {
        self.advance(); // `function`
        let name = if let TokenKind::Ident(id) = self.peek() {
            self.advance();
            id
        } else {
            let span = self.cur_span();
            self.error(span, "expected function name");
            self.error_ident()
        };
        let params = self.parse_params();
        let body = self.parse_block();
        let span = kw_span.to(self.prev_span());
        Stmt::Func(Func { name, params, body, span })
    }

    /// `( $a, $b, ... )` — plain variables only in M0 (no types/defaults).
    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        self.expect(TokenKind::LParen, "expected `(`");
        if !self.at(TokenKind::RParen) {
            loop {
                let tok = self.peek_tok();
                match tok.kind {
                    TokenKind::Variable(id) => {
                        self.advance();
                        params.push(Param { name: id, span: tok.span });
                    }
                    _ => {
                        self.error(tok.span, "expected parameter variable");
                        break;
                    }
                }
                if self.eat(TokenKind::Comma) {
                    if self.at(TokenKind::RParen) {
                        break; // trailing comma
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RParen, "expected `)`");
        params
    }

    // ----- expressions -----

    fn parse_expr(&mut self) -> Expr {
        self.parse_assignment()
    }

    /// Assignment is the lowest-precedence operator and is right-associative;
    /// it is only valid when the left-hand side is a plain `$var`.
    fn parse_assignment(&mut self) -> Expr {
        let lhs = self.parse_binary(0);
        if self.at(TokenKind::Assign) {
            let eq_span = self.cur_span();
            self.advance(); // `=`
            let value = self.parse_assignment(); // right assoc
            match lhs {
                Expr::Var(id, var_span) => {
                    let span = var_span.to(value.span());
                    Expr::Assign { target: id, value: Box::new(value), span }
                }
                other => {
                    self.error(eq_span, "invalid assignment target (expected a variable)");
                    other // recover by keeping the parsed left-hand side
                }
            }
        } else {
            lhs
        }
    }

    /// Precedence-climbing loop over the left-associative binary operators.
    /// `**` and the prefix unary operators are handled below `parse_unary`.
    fn parse_binary(&mut self, min_bp: u8) -> Expr {
        let mut lhs = self.parse_unary();
        while let Some((op, prec)) = bin_op(self.peek()) {
            if prec < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_binary(prec + 1); // +1 => left associative
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        lhs
    }

    /// Prefix unary `-` / `!`. The operand is itself a unary expression, which
    /// (via `parse_pow`) lets `**` bind tighter than the unary operator.
    fn parse_unary(&mut self) -> Expr {
        let tok = self.peek_tok();
        let op = match tok.kind {
            TokenKind::Minus => UnOp::Neg,
            TokenKind::Bang => UnOp::Not,
            _ => return self.parse_pow(),
        };
        self.advance();
        let operand = self.parse_unary();
        let span = tok.span.to(operand.span());
        Expr::Unary { op, expr: Box::new(operand), span }
    }

    /// `**` — right-associative and tighter than unary minus. The exponent is
    /// parsed as a unary expression so `2 ** -3` and `2 ** 3 ** 2` both work.
    fn parse_pow(&mut self) -> Expr {
        let base = self.parse_primary();
        if self.at(TokenKind::StarStar) {
            self.advance();
            let exp = self.parse_unary();
            let span = base.span().to(exp.span());
            Expr::Binary { op: BinOp::Pow, lhs: Box::new(base), rhs: Box::new(exp), span }
        } else {
            base
        }
    }

    fn parse_primary(&mut self) -> Expr {
        let tok = self.peek_tok();
        match tok.kind {
            TokenKind::Int(v) => {
                self.advance();
                Expr::Int(v, tok.span)
            }
            TokenKind::Float(v) => {
                self.advance();
                Expr::Float(v, tok.span)
            }
            TokenKind::Str(id) => {
                self.advance();
                Expr::Str(id, tok.span)
            }
            TokenKind::DQStrBegin => self.parse_interpolated_string(),
            TokenKind::Keyword(Kw::Null) => {
                self.advance();
                Expr::Null(tok.span)
            }
            TokenKind::Keyword(Kw::True) => {
                self.advance();
                Expr::Bool(true, tok.span)
            }
            TokenKind::Keyword(Kw::False) => {
                self.advance();
                Expr::Bool(false, tok.span)
            }
            TokenKind::Variable(id) => {
                self.advance();
                Expr::Var(id, tok.span)
            }
            TokenKind::Ident(id) => {
                self.advance();
                self.parse_call(id, tok.span)
            }
            TokenKind::LParen => {
                self.advance(); // `(`
                let inner = self.parse_expr();
                if !self.eat(TokenKind::RParen) {
                    let span = self.cur_span();
                    self.error(span, "expected `)`");
                }
                inner
            }
            _ => {
                // Cannot start an expression: report and return a placeholder
                // without consuming, leaving recovery to the statement layer.
                self.error(tok.span, "expected expression");
                Expr::Null(tok.span)
            }
        }
    }

    /// A double-quoted string with interpolation: `DQStrBegin (Str|Variable)*
    /// DQStrEnd`. The pieces are folded into a left-associative concatenation,
    /// seeded with an empty string so the result is always a string (a lone
    /// `"$x"` is `(string)$x`, not the raw value of `$x`).
    fn parse_interpolated_string(&mut self) -> Expr {
        let begin = self.advance(); // DQStrBegin
        let mut acc = Expr::Str(self.interner.intern(b""), begin.span);
        loop {
            let tok = self.peek_tok();
            let piece = match tok.kind {
                TokenKind::Str(id) => Expr::Str(id, tok.span),
                TokenKind::Variable(id) => Expr::Var(id, tok.span),
                TokenKind::DQStrEnd => {
                    self.advance();
                    break;
                }
                TokenKind::Eof => {
                    self.error(tok.span, "unterminated interpolated string");
                    break;
                }
                _ => {
                    // The lexer only emits Str/Variable between the brackets.
                    self.error(tok.span, "unexpected token in interpolated string");
                    self.advance();
                    continue;
                }
            };
            self.advance();
            let span = acc.span().to(piece.span());
            acc = Expr::Binary {
                op: BinOp::Concat,
                lhs: Box::new(acc),
                rhs: Box::new(piece),
                span,
            };
        }
        acc
    }

    /// `name ( arg, arg, ... )`. A bareword not followed by `(` has no node in
    /// the M0 AST (bare constants are deferred), so it is reported.
    fn parse_call(&mut self, name: IdentId, name_span: Span) -> Expr {
        if !self.at(TokenKind::LParen) {
            self.error(name_span, "expected `(` after name (bare constants unsupported in M0)");
            return Expr::Null(name_span);
        }
        self.advance(); // `(`
        let mut args = Vec::new();
        if !self.at(TokenKind::RParen) {
            loop {
                args.push(self.parse_expr());
                if self.eat(TokenKind::Comma) {
                    if self.at(TokenKind::RParen) {
                        break; // trailing comma
                    }
                    continue;
                }
                break;
            }
        }
        let end_span = if self.at(TokenKind::RParen) {
            self.advance().span
        } else {
            let span = self.cur_span();
            self.error(span, "expected `)`");
            span
        };
        Expr::Call { name, args, span: name_span.to(end_span) }
    }
}

/// Binary operator table for the precedence-climbing loop. Higher numbers bind
/// tighter. `**` (handled in `parse_pow`) and `=` (handled in
/// `parse_assignment`) are intentionally absent.
fn bin_op(kind: TokenKind) -> Option<(BinOp, u8)> {
    use TokenKind as T;
    // PHP 8 precedence (low → high). Concatenation `.` sits below `+ -` and
    // above the comparison operators (the 8.0 change), so `"x" . 1 + 2` is
    // `"x" . (1 + 2)` and `1 . 2 < 3` parses the concat first.
    Some(match kind {
        T::PipePipe => (BinOp::Or, 1),
        T::AmpAmp => (BinOp::And, 2),
        T::EqEq => (BinOp::Eq, 3),
        T::BangEq => (BinOp::Ne, 3),
        T::EqEqEq => (BinOp::Identical, 3),
        T::BangEqEq => (BinOp::NotIdentical, 3),
        T::Spaceship => (BinOp::Spaceship, 3),
        T::Lt => (BinOp::Lt, 4),
        T::Le => (BinOp::Le, 4),
        T::Gt => (BinOp::Gt, 4),
        T::Ge => (BinOp::Ge, 4),
        T::Dot => (BinOp::Concat, 5),
        T::Plus => (BinOp::Add, 6),
        T::Minus => (BinOp::Sub, 6),
        T::Star => (BinOp::Mul, 7),
        T::Slash => (BinOp::Div, 7),
        T::Percent => (BinOp::Mod, 7),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a source buffer, returning the program and diagnostics.
    fn parse_src(src: &[u8]) -> (Program, Vec<Diagnostic>) {
        let mut interner = Interner::new();
        parse(src, FileId(0), &mut interner)
    }

    /// Parse, asserting no diagnostics, and return the statement list.
    fn parse_ok(src: &[u8]) -> Vec<Stmt> {
        let (program, diags) = parse_src(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        program.items
    }

    /// The single expression of a one-statement program.
    fn one_expr(src: &[u8]) -> Expr {
        let items = parse_ok(src);
        assert_eq!(items.len(), 1, "expected exactly one statement");
        match items.into_iter().next().unwrap() {
            Stmt::Expr(e) => e,
            other => panic!("expected expression statement, got {other:?}"),
        }
    }

    #[test]
    fn literals() {
        assert!(matches!(one_expr(b"<?php null;"), Expr::Null(_)));
        assert!(matches!(one_expr(b"<?php true;"), Expr::Bool(true, _)));
        assert!(matches!(one_expr(b"<?php false;"), Expr::Bool(false, _)));
        assert!(matches!(one_expr(b"<?php 42;"), Expr::Int(42, _)));
        assert!(matches!(one_expr(b"<?php 3.5;"), Expr::Float(_, _)));
        assert!(matches!(one_expr(b"<?php $x;"), Expr::Var(_, _)));
    }

    #[test]
    fn precedence_mul_over_add() {
        // 1 + 2 * 3  =>  Add(1, Mul(2, 3))
        match one_expr(b"<?php 1 + 2 * 3;") {
            Expr::Binary { op: BinOp::Add, lhs, rhs, .. } => {
                assert!(matches!(*lhs, Expr::Int(1, _)));
                match *rhs {
                    Expr::Binary { op: BinOp::Mul, lhs, rhs, .. } => {
                        assert!(matches!(*lhs, Expr::Int(2, _)));
                        assert!(matches!(*rhs, Expr::Int(3, _)));
                    }
                    other => panic!("expected Mul, got {other:?}"),
                }
            }
            other => panic!("expected Add, got {other:?}"),
        }
    }

    #[test]
    fn pow_binds_tighter_than_unary_minus() {
        // -2 ** 2  =>  Neg(Pow(2, 2))
        match one_expr(b"<?php -2 ** 2;") {
            Expr::Unary { op: UnOp::Neg, expr, .. } => match *expr {
                Expr::Binary { op: BinOp::Pow, lhs, rhs, .. } => {
                    assert!(matches!(*lhs, Expr::Int(2, _)));
                    assert!(matches!(*rhs, Expr::Int(2, _)));
                }
                other => panic!("expected Pow, got {other:?}"),
            },
            other => panic!("expected Neg, got {other:?}"),
        }
    }

    #[test]
    fn pow_is_right_associative() {
        // 2 ** 3 ** 2  =>  Pow(2, Pow(3, 2))
        match one_expr(b"<?php 2 ** 3 ** 2;") {
            Expr::Binary { op: BinOp::Pow, lhs, rhs, .. } => {
                assert!(matches!(*lhs, Expr::Int(2, _)));
                assert!(matches!(*rhs, Expr::Binary { op: BinOp::Pow, .. }));
            }
            other => panic!("expected Pow, got {other:?}"),
        }
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // $a && $b || $c  =>  Or(And($a, $b), $c)
        match one_expr(b"<?php $a && $b || $c;") {
            Expr::Binary { op: BinOp::Or, lhs, rhs, .. } => {
                assert!(matches!(*lhs, Expr::Binary { op: BinOp::And, .. }));
                assert!(matches!(*rhs, Expr::Var(_, _)));
            }
            other => panic!("expected Or, got {other:?}"),
        }
    }

    #[test]
    fn comparison_and_equality() {
        // $a == $b < $c  =>  Eq($a, Lt($b, $c))  (comparison binds tighter)
        match one_expr(b"<?php $a == $b < $c;") {
            Expr::Binary { op: BinOp::Eq, rhs, .. } => {
                assert!(matches!(*rhs, Expr::Binary { op: BinOp::Lt, .. }));
            }
            other => panic!("expected Eq, got {other:?}"),
        }
        assert!(matches!(
            one_expr(b"<?php 1 === 2;"),
            Expr::Binary { op: BinOp::Identical, .. }
        ));
        assert!(matches!(
            one_expr(b"<?php 1 !== 2;"),
            Expr::Binary { op: BinOp::NotIdentical, .. }
        ));
        assert!(matches!(
            one_expr(b"<?php 1 <=> 2;"),
            Expr::Binary { op: BinOp::Spaceship, .. }
        ));
    }

    #[test]
    fn parenthesized_overrides_precedence() {
        // (1 + 2) * 3  =>  Mul(Add(1, 2), 3)
        match one_expr(b"<?php (1 + 2) * 3;") {
            Expr::Binary { op: BinOp::Mul, lhs, .. } => {
                assert!(matches!(*lhs, Expr::Binary { op: BinOp::Add, .. }));
            }
            other => panic!("expected Mul, got {other:?}"),
        }
    }

    #[test]
    fn unary_not() {
        assert!(matches!(
            one_expr(b"<?php !$x;"),
            Expr::Unary { op: UnOp::Not, .. }
        ));
    }

    #[test]
    fn assignment_is_right_associative() {
        // $x = $y = 1  =>  Assign($x, Assign($y, 1))
        match one_expr(b"<?php $x = $y = 1;") {
            Expr::Assign { value, .. } => {
                assert!(matches!(*value, Expr::Assign { .. }));
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn assignment_below_binary() {
        // $x = 1 + 2  =>  Assign($x, Add(1, 2))
        match one_expr(b"<?php $x = 1 + 2;") {
            Expr::Assign { value, .. } => {
                assert!(matches!(*value, Expr::Binary { op: BinOp::Add, .. }));
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn echo_multiple_args() {
        let items = parse_ok(b"<?php echo 1, 2, 3;");
        match &items[0] {
            Stmt::Echo { args, .. } => assert_eq!(args.len(), 3),
            other => panic!("expected Echo, got {other:?}"),
        }
    }

    #[test]
    fn if_else() {
        let items = parse_ok(b"<?php if ($x) { echo 1; } else { echo 2; }");
        match &items[0] {
            Stmt::If { then_branch, else_branch, .. } => {
                assert_eq!(then_branch.len(), 1);
                assert_eq!(else_branch.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_without_else() {
        let items = parse_ok(b"<?php if ($x) { $y = 1; }");
        match &items[0] {
            Stmt::If { else_branch, .. } => assert!(else_branch.is_empty()),
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn else_if_chain() {
        let items = parse_ok(b"<?php if ($a) { echo 1; } else if ($b) { echo 2; } else { echo 3; }");
        match &items[0] {
            Stmt::If { else_branch, .. } => {
                assert_eq!(else_branch.len(), 1);
                // The `else if` becomes a nested `if` with its own else.
                match &else_branch[0] {
                    Stmt::If { else_branch: inner, .. } => assert_eq!(inner.len(), 1),
                    other => panic!("expected nested If, got {other:?}"),
                }
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn while_loop() {
        let items = parse_ok(b"<?php while ($x) { $x = 0; }");
        match &items[0] {
            Stmt::While { body, .. } => assert_eq!(body.len(), 1),
            other => panic!("expected While, got {other:?}"),
        }
    }

    #[test]
    fn return_with_and_without_value() {
        let items = parse_ok(b"<?php return 1; return;");
        assert!(matches!(items[0], Stmt::Return { value: Some(_), .. }));
        assert!(matches!(items[1], Stmt::Return { value: None, .. }));
    }

    #[test]
    fn function_decl_and_call() {
        let mut interner = Interner::new();
        let (program, diags) =
            parse(b"<?php function add($a, $b) { return $a + $b; } add(1, 2);", FileId(0), &mut interner);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(program.items.len(), 2);

        match &program.items[0] {
            Stmt::Func(f) => {
                assert_eq!(interner.resolve(f.name), b"add");
                assert_eq!(f.params.len(), 2);
                assert_eq!(f.body.len(), 1);
                assert!(matches!(f.body[0], Stmt::Return { value: Some(_), .. }));
            }
            other => panic!("expected Func, got {other:?}"),
        }
        match &program.items[1] {
            Stmt::Expr(Expr::Call { name, args, .. }) => {
                assert_eq!(interner.resolve(*name), b"add");
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected call expression, got {other:?}"),
        }
    }

    #[test]
    fn call_with_no_args() {
        match one_expr(b"<?php now();") {
            Expr::Call { args, .. } => assert!(args.is_empty()),
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn nested_calls_as_args() {
        // f(g(1), 2)
        match one_expr(b"<?php f(g(1), 2);") {
            Expr::Call { args, .. } => {
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0], Expr::Call { .. }));
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn error_recovery_missing_expr() {
        // `$x = ;` is malformed but must not panic and must report a diagnostic.
        let (program, diags) = parse_src(b"<?php $x = ;");
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        // Recovered: still produced a (partial) statement.
        assert_eq!(program.items.len(), 1);
    }

    #[test]
    fn error_recovery_then_continues() {
        // A broken statement is recovered from, and the following one parses.
        let (program, diags) = parse_src(b"<?php $x = ; echo 1;");
        assert!(!diags.is_empty());
        // The trailing `echo 1;` is parsed despite the earlier error.
        assert!(program.items.iter().any(|s| matches!(s, Stmt::Echo { .. })));
    }

    #[test]
    fn error_invalid_assignment_target() {
        let (_program, diags) = parse_src(b"<?php 1 = 2;");
        assert!(!diags.is_empty(), "expected a diagnostic for bad assign target");
    }

    #[test]
    fn lexer_diagnostics_are_forwarded() {
        // `@` is an unknown character; the lexer's diagnostic must surface.
        let (_program, diags) = parse_src(b"<?php @ ;");
        assert!(!diags.is_empty(), "expected forwarded lexer diagnostic");
    }

    #[test]
    fn empty_program() {
        let (program, diags) = parse_src(b"<?php");
        assert!(program.items.is_empty());
        assert!(diags.is_empty());
    }

    #[test]
    fn string_literal() {
        let mut interner = Interner::new();
        let (program, diags) = parse(br#"<?php "hi";"#, FileId(0), &mut interner);
        assert!(diags.is_empty(), "{diags:?}");
        match &program.items[0] {
            Stmt::Expr(Expr::Str(id, _)) => assert_eq!(interner.resolve(*id), b"hi"),
            other => panic!("expected string literal, got {other:?}"),
        }
    }

    #[test]
    fn concat_binds_below_addition() {
        // "x" . 1 + 2  =>  Concat("x", Add(1, 2))   (PHP 8 precedence)
        match one_expr(br#"<?php "x" . 1 + 2;"#) {
            Expr::Binary { op: BinOp::Concat, rhs, .. } => {
                assert!(matches!(*rhs, Expr::Binary { op: BinOp::Add, .. }));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn concat_binds_above_comparison() {
        // 1 . 2 < 3  =>  Lt(Concat(1, 2), 3)
        match one_expr(b"<?php 1 . 2 < 3;") {
            Expr::Binary { op: BinOp::Lt, lhs, .. } => {
                assert!(matches!(*lhs, Expr::Binary { op: BinOp::Concat, .. }));
            }
            other => panic!("expected Lt, got {other:?}"),
        }
    }

    #[test]
    fn concat_is_left_associative() {
        // $a . $b . $c  =>  Concat(Concat($a, $b), $c)
        match one_expr(b"<?php $a . $b . $c;") {
            Expr::Binary { op: BinOp::Concat, lhs, .. } => {
                assert!(matches!(*lhs, Expr::Binary { op: BinOp::Concat, .. }));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn interpolation_folds_into_concat() {
        // "a $x" => Concat(Concat("", "a "), $x)  — seeded with an empty string.
        match one_expr(br#"<?php "a $x";"#) {
            Expr::Binary { op: BinOp::Concat, lhs, rhs, .. } => {
                assert!(matches!(*rhs, Expr::Var(_, _)));
                assert!(matches!(*lhs, Expr::Binary { op: BinOp::Concat, .. }));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }
}
