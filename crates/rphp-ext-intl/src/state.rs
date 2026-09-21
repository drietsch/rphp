//! php's intl error conventions (`ext/intl/ERROR_CONVENTIONS.md`): the last
//! error is always stored globally (`intl_get_error_code()`), and also in
//! the object whose method failed (`$obj->getErrorCode()`) unless the fault
//! was in the arguments; `intl.error_level` turns every error into a
//! diagnostic of that level, `intl.use_exceptions` into an `IntlException`.
//! The message php keeps is `func(): <text>`, and `intl_get_error_message()`
//! appends `: U_NAME`.

use rphp_runtime::{Ctx, ErrLevel, Unwind};
use rphp_value::Value;

/// `U_ZERO_ERROR`.
pub const U_ZERO_ERROR: i64 = 0;
/// `U_ILLEGAL_ARGUMENT_ERROR`.
pub const U_ILLEGAL_ARGUMENT_ERROR: i64 = 1;
/// `U_MISSING_RESOURCE_ERROR`.
pub const U_MISSING_RESOURCE_ERROR: i64 = 2;
/// `U_INVALID_FORMAT_ERROR`.
pub const U_INVALID_FORMAT_ERROR: i64 = 3;
/// `U_INTERNAL_PROGRAM_ERROR`.
pub const U_INTERNAL_PROGRAM_ERROR: i64 = 5;
/// `U_MESSAGE_PARSE_ERROR`.
pub const U_MESSAGE_PARSE_ERROR: i64 = 6;
/// `U_MEMORY_ALLOCATION_ERROR`.
pub const U_MEMORY_ALLOCATION_ERROR: i64 = 7;
/// `U_INDEX_OUTOFBOUNDS_ERROR`.
pub const U_INDEX_OUTOFBOUNDS_ERROR: i64 = 8;
/// `U_PARSE_ERROR`.
pub const U_PARSE_ERROR: i64 = 9;
/// `U_INVALID_CHAR_FOUND`.
pub const U_INVALID_CHAR_FOUND: i64 = 10;
/// `U_ILLEGAL_CHAR_FOUND`.
pub const U_ILLEGAL_CHAR_FOUND: i64 = 12;
/// `U_BUFFER_OVERFLOW_ERROR`.
pub const U_BUFFER_OVERFLOW_ERROR: i64 = 15;
/// `U_UNSUPPORTED_ERROR`.
pub const U_UNSUPPORTED_ERROR: i64 = 16;
/// `U_ILLEGAL_ESCAPE_SEQUENCE`.
pub const U_ILLEGAL_ESCAPE_SEQUENCE: i64 = 18;
/// `U_INVALID_STATE_ERROR`.
pub const U_INVALID_STATE_ERROR: i64 = 27;
/// `U_INVALID_ID` (transliterator).
pub const U_INVALID_ID: i64 = 65569;
/// `U_PATTERN_SYNTAX_ERROR`.
pub const U_PATTERN_SYNTAX_ERROR: i64 = 65799;
/// `U_UNSUPPORTED_ATTRIBUTE`.
pub const U_UNSUPPORTED_ATTRIBUTE: i64 = 65803;
/// `U_ARGUMENT_TYPE_MISMATCH`.
pub const U_ARGUMENT_TYPE_MISMATCH: i64 = 65804;
/// `U_USING_FALLBACK_WARNING`.
pub const U_USING_FALLBACK_WARNING: i64 = -128;
/// `U_USING_DEFAULT_WARNING`.
pub const U_USING_DEFAULT_WARNING: i64 = -127;
/// `U_STRING_NOT_TERMINATED_WARNING`.
pub const U_STRING_NOT_TERMINATED_WARNING: i64 = -124;

const STANDARD: &[&str] = &[
    "U_ZERO_ERROR",
    "U_ILLEGAL_ARGUMENT_ERROR",
    "U_MISSING_RESOURCE_ERROR",
    "U_INVALID_FORMAT_ERROR",
    "U_FILE_ACCESS_ERROR",
    "U_INTERNAL_PROGRAM_ERROR",
    "U_MESSAGE_PARSE_ERROR",
    "U_MEMORY_ALLOCATION_ERROR",
    "U_INDEX_OUTOFBOUNDS_ERROR",
    "U_PARSE_ERROR",
    "U_INVALID_CHAR_FOUND",
    "U_TRUNCATED_CHAR_FOUND",
    "U_ILLEGAL_CHAR_FOUND",
    "U_INVALID_TABLE_FORMAT",
    "U_INVALID_TABLE_FILE",
    "U_BUFFER_OVERFLOW_ERROR",
    "U_UNSUPPORTED_ERROR",
    "U_RESOURCE_TYPE_MISMATCH",
    "U_ILLEGAL_ESCAPE_SEQUENCE",
    "U_UNSUPPORTED_ESCAPE_SEQUENCE",
    "U_NO_SPACE_AVAILABLE",
    "U_CE_NOT_FOUND_ERROR",
    "U_PRIMARY_TOO_LONG_ERROR",
    "U_STATE_TOO_OLD_ERROR",
    "U_TOO_MANY_ALIASES_ERROR",
    "U_ENUM_OUT_OF_SYNC_ERROR",
    "U_INVARIANT_CONVERSION_ERROR",
    "U_INVALID_STATE_ERROR",
    "U_COLLATOR_VERSION_MISMATCH",
    "U_USELESS_COLLATOR_ERROR",
    "U_NO_WRITE_PERMISSION",
    "U_INPUT_TOO_LONG_ERROR",
];

const WARNINGS: &[&str] = &[
    "U_USING_FALLBACK_WARNING",
    "U_USING_DEFAULT_WARNING",
    "U_SAFECLONE_ALLOCATED_WARNING",
    "U_STATE_OLD_WARNING",
    "U_STRING_NOT_TERMINATED_WARNING",
    "U_SORT_KEY_TOO_SHORT_WARNING",
    "U_AMBIGUOUS_ALIAS_WARNING",
    "U_DIFFERENT_UCA_VERSION",
    "U_PLUGIN_CHANGED_LEVEL_WARNING",
];

const TRANSLITERATOR: &[&str] = &[
    "U_BAD_VARIABLE_DEFINITION",
    "U_MALFORMED_RULE",
    "U_MALFORMED_SET",
    "U_MALFORMED_SYMBOL_REFERENCE",
    "U_MALFORMED_UNICODE_ESCAPE",
    "U_MALFORMED_VARIABLE_DEFINITION",
    "U_MALFORMED_VARIABLE_REFERENCE",
    "U_MISMATCHED_SEGMENT_DELIMITERS",
    "U_MISPLACED_ANCHOR_START",
    "U_MISPLACED_CURSOR_OFFSET",
    "U_MISPLACED_QUANTIFIER",
    "U_MISSING_OPERATOR",
    "U_MISSING_SEGMENT_CLOSE",
    "U_MULTIPLE_ANTE_CONTEXTS",
    "U_MULTIPLE_CURSORS",
    "U_MULTIPLE_POST_CONTEXTS",
    "U_TRAILING_BACKSLASH",
    "U_UNDEFINED_SEGMENT_REFERENCE",
    "U_UNDEFINED_VARIABLE",
    "U_UNQUOTED_SPECIAL",
    "U_UNTERMINATED_QUOTE",
    "U_RULE_MASK_ERROR",
    "U_MISPLACED_COMPOUND_FILTER",
    "U_MULTIPLE_COMPOUND_FILTERS",
    "U_INVALID_RBT_SYNTAX",
    "U_INVALID_PROPERTY_PATTERN",
    "U_MALFORMED_PRAGMA",
    "U_UNCLOSED_SEGMENT",
    "U_ILLEGAL_CHAR_IN_SEGMENT",
    "U_VARIABLE_RANGE_EXHAUSTED",
    "U_VARIABLE_RANGE_OVERLAP",
    "U_ILLEGAL_CHARACTER",
    "U_INTERNAL_TRANSLITERATOR_ERROR",
    "U_INVALID_ID",
    "U_INVALID_FUNCTION",
];

const FORMATTING: &[&str] = &[
    "U_UNEXPECTED_TOKEN",
    "U_MULTIPLE_DECIMAL_SEPARATORS",
    "U_MULTIPLE_EXPONENTIAL_SYMBOLS",
    "U_MALFORMED_EXPONENTIAL_PATTERN",
    "U_MULTIPLE_PERCENT_SYMBOLS",
    "U_MULTIPLE_PERMILL_SYMBOLS",
    "U_MULTIPLE_PAD_SPECIFIERS",
    "U_PATTERN_SYNTAX_ERROR",
    "U_ILLEGAL_PAD_POSITION",
    "U_UNMATCHED_BRACES",
    "U_UNSUPPORTED_PROPERTY",
    "U_UNSUPPORTED_ATTRIBUTE",
    "U_ARGUMENT_TYPE_MISMATCH",
    "U_DUPLICATE_KEYWORD",
    "U_UNDEFINED_KEYWORD",
    "U_DEFAULT_KEYWORD_MISSING",
    "U_DECIMAL_NUMBER_SYNTAX_ERROR",
    "U_FORMAT_INEXACT_ERROR",
    "U_NUMBER_ARG_OUTOFBOUNDS_ERROR",
    "U_NUMBER_SKELETON_SYNTAX_ERROR",
    "U_MF_UNRESOLVED_VARIABLE_ERROR",
    "U_MF_SYNTAX_ERROR",
    "U_MF_UNKNOWN_FUNCTION_ERROR",
    "U_MF_VARIANT_KEY_MISMATCH_ERROR",
    "U_MF_FORMATTING_ERROR",
    "U_MF_NONEXHAUSTIVE_PATTERN_ERROR",
    "U_MF_DUPLICATE_OPTION_NAME_ERROR",
    "U_MF_SELECTOR_ERROR",
    "U_MF_MISSING_SELECTOR_ANNOTATION_ERROR",
    "U_MF_DUPLICATE_DECLARATION_ERROR",
    "U_MF_OPERAND_MISMATCH_ERROR",
    "U_MF_DUPLICATE_VARIANT_ERROR",
    "U_MF_BAD_OPTION",
];

const BREAK_ITERATOR: &[&str] = &[
    "U_BRK_INTERNAL_ERROR",
    "U_BRK_HEX_DIGITS_EXPECTED",
    "U_BRK_SEMICOLON_EXPECTED",
    "U_BRK_RULE_SYNTAX",
    "U_BRK_UNCLOSED_SET",
    "U_BRK_ASSIGN_ERROR",
    "U_BRK_VARIABLE_REDFINITION",
    "U_BRK_MISMATCHED_PAREN",
    "U_BRK_NEW_LINE_IN_QUOTED_STRING",
    "U_BRK_UNDEFINED_VARIABLE",
    "U_BRK_INIT_ERROR",
    "U_BRK_RULE_EMPTY_SET",
    "U_BRK_UNRECOGNIZED_OPTION",
    "U_BRK_MALFORMED_RULE_TAG",
];

const REGEX: &[&str] = &[
    "U_REGEX_INTERNAL_ERROR",
    "U_REGEX_RULE_SYNTAX",
    "U_REGEX_INVALID_STATE",
    "U_REGEX_BAD_ESCAPE_SEQUENCE",
    "U_REGEX_PROPERTY_SYNTAX",
    "U_REGEX_UNIMPLEMENTED",
    "U_REGEX_MISMATCHED_PAREN",
    "U_REGEX_NUMBER_TOO_BIG",
    "U_REGEX_BAD_INTERVAL",
    "U_REGEX_MAX_LT_MIN",
    "U_REGEX_INVALID_BACK_REF",
    "U_REGEX_INVALID_FLAG",
    "U_REGEX_LOOK_BEHIND_LIMIT",
    "U_REGEX_SET_CONTAINS_STRING",
    "U_REGEX_OCTAL_TOO_BIG",
    "U_REGEX_MISSING_CLOSE_BRACKET",
    "U_REGEX_INVALID_RANGE",
    "U_REGEX_STACK_OVERFLOW",
    "U_REGEX_TIME_OUT",
    "U_REGEX_STOPPED_BY_CALLER",
    "U_REGEX_PATTERN_TOO_BIG",
    "U_REGEX_INVALID_CAPTURE_GROUP_NAME",
];

const IDNA: &[&str] = &[
    "U_IDNA_PROHIBITED_ERROR",
    "U_IDNA_UNASSIGNED_ERROR",
    "U_IDNA_CHECK_BIDI_ERROR",
    "U_IDNA_STD3_ASCII_RULES_ERROR",
    "U_IDNA_ACE_PREFIX_ERROR",
    "U_IDNA_VERIFICATION_ERROR",
    "U_IDNA_LABEL_TOO_LONG_ERROR",
    "U_IDNA_ZERO_LENGTH_LABEL_ERROR",
    "U_IDNA_DOMAIN_NAME_TOO_LONG_ERROR",
];

const PLUGIN: &[&str] = &["U_PLUGIN_TOO_HIGH", "U_PLUGIN_DIDNT_SET_LEVEL"];

/// ICU's `u_errorName()`: the canonical name of an error code, or
/// `[BOGUS UErrorCode]` outside the known ranges.
pub fn error_name(code: i64) -> &'static str {
    let pick = |table: &[&'static str], start: i64| -> Option<&'static str> {
        usize::try_from(code - start).ok().and_then(|i| table.get(i).copied())
    };
    let name = if code < 0 {
        pick(WARNINGS, -128)
    } else if code < 65536 {
        pick(STANDARD, 0)
    } else if code < 65792 {
        pick(TRANSLITERATOR, 65536)
    } else if code < 66048 {
        pick(FORMATTING, 65792)
    } else if code < 66304 {
        pick(BREAK_ITERATOR, 66048)
    } else if code < 66560 {
        pick(REGEX, 66304)
    } else if code < 66816 {
        pick(IDNA, 66560)
    } else {
        pick(PLUGIN, 66816)
    };
    name.unwrap_or("[BOGUS UErrorCode]")
}

/// `U_FAILURE()`: an error, not a warning or success.
pub fn is_failure(code: i64) -> bool {
    code > 0
}

/// The last error, global or an object's own: the code and php's prefixed
/// custom message (`func(): text`).
#[derive(Clone, Debug, Default)]
pub struct IntlError {
    pub code: i64,
    pub msg: Option<String>,
}

impl IntlError {
    /// `intl_error_get_message()`: `custom: U_NAME`, or the name alone.
    pub fn message(&self) -> String {
        match &self.msg {
            Some(m) => format!("{m}: {}", error_name(self.code)),
            None => error_name(self.code).to_string(),
        }
    }

    pub fn reset(&mut self) {
        self.code = U_ZERO_ERROR;
        self.msg = None;
    }
}

/// The request's global error (`ExtState::slot`).
pub fn global(ctx: &mut Ctx) -> &mut IntlError {
    ctx.ext.slot::<IntlError>("intl")
}

/// `intl_error_reset(NULL)`: every intl entry point starts clean.
pub fn reset_global(ctx: &mut Ctx) {
    global(ctx).reset();
}

/// `intl_error_set_code(NULL, code)`.
pub fn set_global_code(ctx: &mut Ctx, code: i64) {
    global(ctx).code = code;
}

/// `intl_error_set(NULL, code, msg)`: the global error, the
/// `intl.error_level` diagnostic and the `intl.use_exceptions` throw.
pub fn set_global(ctx: &mut Ctx, who: &str, code: i64, msg: &str) -> Result<(), Unwind> {
    let prefixed = format!("{who}(): {msg}");
    {
        let g = global(ctx);
        g.code = code;
        g.msg = Some(prefixed.clone());
    }
    diagnose(ctx, msg, &prefixed)
}

/// The `intl.error_level` diagnostic and the `intl.use_exceptions`
/// `IntlException` for a message php has just recorded.
fn diagnose(ctx: &mut Ctx, msg: &str, prefixed: &str) -> Result<(), Unwind> {
    let level = ctx.ini.int("intl.error_level");
    if level != 0 {
        let lvl = match level {
            2 => ErrLevel::Warning,
            8 => ErrLevel::Notice,
            256 => ErrLevel::UserError,
            512 => ErrLevel::UserWarning,
            1024 => ErrLevel::UserNotice,
            8192 => ErrLevel::Deprecated,
            16384 => ErrLevel::UserDeprecated,
            _ => ErrLevel::Warning,
        };
        ctx.emit_error(lvl, prefixed)?;
        let _ = msg;
    }
    if ctx.ini.bool("intl.use_exceptions") {
        return Err(Unwind::exception("IntlException", prefixed.to_string()));
    }
    Ok(())
}

/// `intl_errors_set(&obj->err, code, msg)`: the object's error and the
/// global one together.
pub fn set_both(ctx: &mut Ctx, obj_err: &mut IntlError, who: &str, code: i64, msg: &str) -> Result<(), Unwind> {
    let prefixed = format!("{who}(): {msg}");
    obj_err.code = code;
    obj_err.msg = Some(prefixed.clone());
    {
        let g = global(ctx);
        g.code = code;
        g.msg = Some(prefixed.clone());
    }
    diagnose(ctx, msg, &prefixed)
}

/// `intl_errors_set_code`: the code alone, in both places.
pub fn set_both_code(ctx: &mut Ctx, obj_err: &mut IntlError, code: i64) {
    obj_err.code = code;
    global(ctx).code = code;
}

/// The failure return of an intl function: `false`, after
/// [`set_global`].
pub fn fail(ctx: &mut Ctx, who: &str, code: i64, msg: &str) -> Result<Value, Unwind> {
    set_global(ctx, who, code, msg)?;
    Ok(Value::Bool(false))
}

/// The failure return of a constructor or factory: `null`.
pub fn fail_null(ctx: &mut Ctx, who: &str, code: i64, msg: &str) -> Result<Value, Unwind> {
    set_global(ctx, who, code, msg)?;
    Ok(Value::Null)
}

/// `ini_get('intl.default_locale')`, or ICU's default (`en_US_POSIX` is
/// what `-n` php answers without a locale environment; the engine
/// standardises on `en_US`).
pub fn default_locale(ctx: &Ctx) -> String {
    match ctx.ini.get("intl.default_locale") {
        Some(l) if !l.is_empty() => l.to_string(),
        _ => "en_US".to_string(),
    }
}
