//! PHP-exact tokenizer: a direct implementation of the Zend scanner's state
//! machine producing lossless raw tokens with php-src token ids, backing
//! `token_get_all()` / `PhpToken::tokenize()` and `rphp --emit=tokens`.
//!
//! The contract is byte-for-byte identity with `token_get_all($src)` of the
//! installed PHP 8.5 in non-`TOKEN_PARSE` mode: the same token ids, the same
//! extents (so `concat(src[lo..hi]) == src`), and the same line numbers,
//! including every quirk of `Zend/zend_language_scanner.l` (one whitespace
//! byte swallowed by `<?php`, one newline swallowed by `?>`, `//` comments
//! stopping at `?>`, the ampersand split by lookahead, heredoc start/end
//! extents, `T_BAD_CHARACTER` for control bytes, the `__halt_compiler()`
//! remainder, …). `crates/rphp-tokenizer/tests/differential.rs` checks this
//! against `php` over an in-repo corpus and, on request, a whole `vendor/` tree.
//!
//! The crate is pure: no I/O, no allocation beyond the output vector (plus a
//! clone of the scanner per heredoc for the scan-ahead), `#![forbid(unsafe_code)]`.
//!
//! Not covered here: `TOKEN_PARSE` reclassification and the `PhpToken` class
//! (both belong to `ext/tokenizer` once the parser adapter and native classes
//! exist).
//!
// TODO(F5/TOKEN_PARSE): `pub fn reclassify(tokens: &mut [RawToken], ident_spans: &[(u32, u32)])`
// — given the parser's `IdentSpans` side table (byte ranges the grammar accepted
// as identifiers, e.g. `Foo::class`, `$o->list`, `enum` used as a name), turn
// the keyword tokens whose extent equals one of those spans into `T_STRING`;
// that is what `token_get_all($src, TOKEN_PARSE)` gets from `zend_lex_tstring`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ids;

mod heredoc;
mod names;
mod scanner;
mod states;

use scanner::Scanner;

/// One raw token: PHP's numeric token id, the half-open byte range it covers
/// and the 1-based line of its first byte, exactly as `token_get_all()` and
/// `PhpToken::tokenize()` report them.
///
/// `id` is a `T_*` constant from [`ids`] (`>= 256`) or, for single-character
/// tokens, the byte value of the character (`;` is 59). The one multi-byte
/// "single-character" token PHP has is the `b"`/`B"` that opens an interpolated
/// binary string: its id is `"` (34) and its extent is two bytes. Zero-length
/// tokens exist too: PHP emits an empty `T_ENCAPSED_AND_WHITESPACE` for an
/// invalid byte inside `"$a[…]"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawToken {
    /// PHP token id (`>= 256`) or the byte value of a single-character token.
    pub id: u16,
    /// Byte offset of the first byte.
    pub lo: u32,
    /// Byte offset one past the last byte.
    pub hi: u32,
    /// 1-based line of the first byte (`CG(zend_lineno)` when scanning began).
    pub line: u32,
}

impl RawToken {
    /// The bytes this token covers in `src`.
    #[inline]
    pub fn text<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        &src[self.lo as usize..self.hi as usize]
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> u32 {
        self.hi - self.lo
    }

    /// True for the zero-length `T_ENCAPSED_AND_WHITESPACE` PHP emits inside a
    /// broken `"$a[…]"` offset.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.hi == self.lo
    }

    /// `token_name()` of this token, `None` for single-character tokens.
    #[inline]
    pub fn name(&self) -> Option<&'static str> {
        token_name(self.id)
    }
}

/// The ini settings the scanner consults.
///
/// `asp_tags` was removed in PHP 7 and is deliberately not modelled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Options {
    /// `short_open_tag`: whether `<?` (and the degenerate `<?phpX`) opens PHP
    /// mode. Defaults to `false`, like `php.ini-production`; note that a bare
    /// `php -n` runs with the built-in default of `true`.
    pub short_open_tag: bool,
}

/// Tokenize `src` exactly like `token_get_all($src)` (without `TOKEN_PARSE`).
///
/// Never fails and never panics: unterminated strings, comments and heredocs
/// yield whatever PHP yields, control bytes become `T_BAD_CHARACTER`, and the
/// result always tiles `src` (`tokens[0].lo == 0`, `tokens[i].hi ==
/// tokens[i + 1].lo`, `tokens.last().hi == src.len()`; an empty input has no
/// tokens).
///
/// # Panics
/// Only if `src` is longer than `u32::MAX` bytes (spans are `u32`).
pub fn tokenize(src: &[u8], opts: Options) -> Vec<RawToken> {
    assert!(
        u32::try_from(src.len()).is_ok(),
        "rphp-tokenizer: source larger than 4 GiB"
    );
    let mut sc = Scanner::new(src, opts.short_open_tag);
    let mut out = Vec::with_capacity(src.len() / 4 + 8);
    // tokenizer.c: token_line is CG(zend_lineno) as it stood *before* the token
    // was scanned; -1 disables the __halt_compiler() countdown.
    let mut token_line = 1u32;
    let mut need_tokens: i32 = -1;
    while let Some(t) = sc.lex() {
        out.push(RawToken {
            id: t.id,
            lo: t.lo as u32,
            hi: t.hi as u32,
            line: token_line,
        });
        if need_tokens != -1 {
            // After T_HALT_COMPILER collect three more non-dropped tokens, then
            // everything left is one T_INLINE_HTML stamped with the *current*
            // token_line (the third token's line), as ext/tokenizer does.
            if !is_ignorable(t.id) {
                need_tokens -= 1;
                if need_tokens == 0 {
                    if sc.pos < src.len() {
                        out.push(RawToken {
                            id: ids::T_INLINE_HTML,
                            lo: sc.pos as u32,
                            hi: src.len() as u32,
                            line: token_line,
                        });
                    }
                    break;
                }
            }
        } else if t.id == ids::T_HALT_COMPILER {
            need_tokens = 3;
        }
        if sc.increment_lineno {
            sc.line += 1;
            sc.increment_lineno = false;
        }
        token_line = sc.line;
    }
    out
}

/// The name PHP's `token_name()` reports for `id` (`"T_LNUMBER"`, …), or `None`
/// for single-character tokens and unassigned ids (where PHP says `"UNKNOWN"`).
#[inline]
pub fn token_name(id: u16) -> Option<&'static str> {
    ids::name_of(id)
}

/// `PhpToken::isIgnorable()`: whitespace, comments and `T_OPEN_TAG`. These are
/// also the tokens ext/tokenizer does not count towards the three that follow
/// `__halt_compiler`.
#[inline]
pub fn is_ignorable(id: u16) -> bool {
    matches!(
        id,
        ids::T_WHITESPACE | ids::T_COMMENT | ids::T_DOC_COMMENT | ids::T_OPEN_TAG
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_of(src: &str) -> Vec<u16> {
        tokenize(src.as_bytes(), Options::default())
            .iter()
            .map(|t| t.id)
            .collect()
    }

    #[test]
    fn hello() {
        assert_eq!(
            ids_of("<?php echo 1;"),
            vec![
                ids::T_OPEN_TAG,
                ids::T_ECHO,
                ids::T_WHITESPACE,
                ids::T_LNUMBER,
                b';' as u16
            ]
        );
    }

    #[test]
    fn empty_and_html_only() {
        assert!(tokenize(b"", Options::default()).is_empty());
        let t = tokenize(b"<html>\n", Options::default());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, ids::T_INLINE_HTML);
        assert_eq!((t[0].lo, t[0].hi, t[0].line), (0, 7, 1));
    }

    #[test]
    fn names_and_ignorable() {
        assert_eq!(token_name(ids::T_LNUMBER), Some("T_LNUMBER"));
        assert_eq!(
            token_name(ids::T_PAAMAYIM_NEKUDOTAYIM),
            Some("T_DOUBLE_COLON")
        );
        assert_eq!(token_name(b';' as u16), None);
        assert!(is_ignorable(ids::T_DOC_COMMENT));
        assert!(!is_ignorable(ids::T_CLOSE_TAG));
    }

    #[test]
    fn halt_compiler_remainder_takes_third_tokens_line() {
        let src = b"<?php __halt_compiler (\n)\n;\n\nrest";
        let t = tokenize(src, Options::default());
        let last = t.last().unwrap();
        assert_eq!(last.id, ids::T_INLINE_HTML);
        assert_eq!(last.text(src), b"\n\nrest");
        assert_eq!(last.line, 3);
    }

    #[test]
    fn short_open_tag_option() {
        assert_eq!(ids_of("<? 1"), vec![ids::T_INLINE_HTML]);
        let t = tokenize(
            b"<? 1",
            Options {
                short_open_tag: true,
            },
        );
        assert_eq!(t[0].id, ids::T_OPEN_TAG);
        assert_eq!(t[0].hi, 2);
    }
}
