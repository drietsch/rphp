//! Labels, keywords and casts: the parts of the `ST_IN_SCRIPTING` rules that
//! are plain table lookups.
//!
//! The Zend scanner is generated with re2c `--case-inverted`, which makes every
//! double-quoted rule literal case-insensitive, so `EXIT`, `Die`, `(INT)` and
//! `__class__` all lex like their lowercase spellings. Bytes `>= 0x80` are
//! label characters (PHP identifiers are byte strings).

use crate::ids::*;

/// `[a-zA-Z_\x80-\xff]` — the first byte of a label (`IS_LABEL_START`).
#[inline]
pub(crate) fn is_label_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

/// `[a-zA-Z0-9_\x80-\xff]` — any later byte of a label (`IS_LABEL_SUCCESSOR`).
#[inline]
pub(crate) fn is_label_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// Length of the longest keyword (`__halt_compiler`).
const MAX_KEYWORD_LEN: usize = 15;

/// Reserved words that lex as their own token when a label matches them
/// exactly (case-insensitively). `enum` and `yield from` need lookahead and are
/// handled in the scanner; `public(set)` & co. are checked by the scanner after
/// this lookup returns `T_PUBLIC` / `T_PROTECTED` / `T_PRIVATE`.
pub(crate) fn keyword(label: &[u8]) -> Option<u16> {
    if label.len() > MAX_KEYWORD_LEN {
        return None;
    }
    let mut buf = [0u8; MAX_KEYWORD_LEN];
    for (d, s) in buf.iter_mut().zip(label) {
        *d = s.to_ascii_lowercase();
    }
    let id = match &buf[..label.len()] {
        b"exit" | b"die" => T_EXIT,
        b"fn" => T_FN,
        b"function" => T_FUNCTION,
        b"const" => T_CONST,
        b"return" => T_RETURN,
        b"yield" => T_YIELD,
        b"try" => T_TRY,
        b"catch" => T_CATCH,
        b"finally" => T_FINALLY,
        b"throw" => T_THROW,
        b"if" => T_IF,
        b"elseif" => T_ELSEIF,
        b"endif" => T_ENDIF,
        b"else" => T_ELSE,
        b"while" => T_WHILE,
        b"endwhile" => T_ENDWHILE,
        b"do" => T_DO,
        b"for" => T_FOR,
        b"endfor" => T_ENDFOR,
        b"foreach" => T_FOREACH,
        b"endforeach" => T_ENDFOREACH,
        b"declare" => T_DECLARE,
        b"enddeclare" => T_ENDDECLARE,
        b"instanceof" => T_INSTANCEOF,
        b"as" => T_AS,
        b"switch" => T_SWITCH,
        b"match" => T_MATCH,
        b"endswitch" => T_ENDSWITCH,
        b"case" => T_CASE,
        b"default" => T_DEFAULT,
        b"break" => T_BREAK,
        b"continue" => T_CONTINUE,
        b"goto" => T_GOTO,
        b"echo" => T_ECHO,
        b"print" => T_PRINT,
        b"class" => T_CLASS,
        b"interface" => T_INTERFACE,
        b"trait" => T_TRAIT,
        b"extends" => T_EXTENDS,
        b"implements" => T_IMPLEMENTS,
        b"new" => T_NEW,
        b"clone" => T_CLONE,
        b"var" => T_VAR,
        b"eval" => T_EVAL,
        b"include" => T_INCLUDE,
        b"include_once" => T_INCLUDE_ONCE,
        b"require" => T_REQUIRE,
        b"require_once" => T_REQUIRE_ONCE,
        b"namespace" => T_NAMESPACE,
        b"use" => T_USE,
        b"insteadof" => T_INSTEADOF,
        b"global" => T_GLOBAL,
        b"isset" => T_ISSET,
        b"empty" => T_EMPTY,
        b"__halt_compiler" => T_HALT_COMPILER,
        b"static" => T_STATIC,
        b"abstract" => T_ABSTRACT,
        b"final" => T_FINAL,
        b"private" => T_PRIVATE,
        b"protected" => T_PROTECTED,
        b"public" => T_PUBLIC,
        b"readonly" => T_READONLY,
        b"unset" => T_UNSET,
        b"list" => T_LIST,
        b"array" => T_ARRAY,
        b"callable" => T_CALLABLE,
        b"or" => T_LOGICAL_OR,
        b"and" => T_LOGICAL_AND,
        b"xor" => T_LOGICAL_XOR,
        b"__class__" => T_CLASS_C,
        b"__trait__" => T_TRAIT_C,
        b"__function__" => T_FUNC_C,
        b"__property__" => T_PROPERTY_C,
        b"__method__" => T_METHOD_C,
        b"__line__" => T_LINE,
        b"__file__" => T_FILE,
        b"__dir__" => T_DIR,
        b"__namespace__" => T_NS_C,
        _ => return None,
    };
    Some(id)
}

/// Length of the longest cast keyword (`integer`, `boolean`).
const MAX_CAST_LEN: usize = 7;

/// The words accepted between `(` and `)` (with optional tabs/spaces on either
/// side) as a cast operator. PHP 8.5 still *lexes* `(real)`, `(integer)`,
/// `(double)`, `(boolean)` and `(binary)` as casts; only the parser rejects or
/// deprecates them, which `token_get_all()` never runs.
pub(crate) fn cast(word: &[u8]) -> Option<u16> {
    if word.is_empty() || word.len() > MAX_CAST_LEN {
        return None;
    }
    let mut buf = [0u8; MAX_CAST_LEN];
    for (d, s) in buf.iter_mut().zip(word) {
        *d = s.to_ascii_lowercase();
    }
    let id = match &buf[..word.len()] {
        b"int" | b"integer" => T_INT_CAST,
        b"float" | b"double" | b"real" => T_DOUBLE_CAST,
        b"string" | b"binary" => T_STRING_CAST,
        b"array" => T_ARRAY_CAST,
        b"object" => T_OBJECT_CAST,
        b"bool" | b"boolean" => T_BOOL_CAST,
        b"unset" => T_UNSET_CAST,
        b"void" => T_VOID_CAST,
        _ => return None,
    };
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_case_insensitive() {
        assert_eq!(keyword(b"EXIT"), Some(T_EXIT));
        assert_eq!(keyword(b"Die"), Some(T_EXIT));
        assert_eq!(keyword(b"__CLASS__"), Some(T_CLASS_C));
        assert_eq!(keyword(b"__class__"), Some(T_CLASS_C));
        assert_eq!(keyword(b"enum"), None);
        assert_eq!(keyword(b"exitx"), None);
        assert_eq!(keyword(b"__halt_compiler_"), None);
    }

    #[test]
    fn casts() {
        assert_eq!(cast(b"INT"), Some(T_INT_CAST));
        assert_eq!(cast(b"real"), Some(T_DOUBLE_CAST));
        assert_eq!(cast(b"binary"), Some(T_STRING_CAST));
        assert_eq!(cast(b"void"), Some(T_VOID_CAST));
        assert_eq!(cast(b"intx"), None);
        assert_eq!(cast(b""), None);
    }
}
