//! php-src `ext/mbstring/php_mbregex.c` (`mb_ereg*`, `mb_split`,
//! `mb_regex_*`, the `mb_ereg_search_*` family) plus the two remaining
//! `mbstring.c` functions that were cataloged with it: `mb_convert_kana`
//! (`libmbfl/filters/mbfilter_tl_jisx0201_jisx0208.c`) and `mb_send_mail`.
//!
//! **The engine.** php runs these on Oniguruma; rphp has no Oniguruma, so
//! each pattern is *translated* from Oniguruma's syntax (Ruby by default,
//! or the syntax `mb_regex_set_options` picked) into a PCRE2 pattern with
//! the same meaning, and run on the PCRE2 the `pcre` extension links
//! ([`Translator`]). Where the two engines agree the translation is a
//! respelling: Ruby's `\h` is a hex digit, `(?m)` is dot-all, `^`/`$` are
//! line anchors unless the `s` option makes them buffer anchors, an
//! isolated `(?i)` scopes over the rest of its group *including* later
//! alternatives, `{,n}` is `{0,n}`, `{n,m}+` and `a**` are nested repeats,
//! `[a-z&&[^aeiou]]` is an intersection (rendered with lookaheads), named
//! groups switch plain groups to non-capturing, `\k<-1>` is relative, and
//! so on. The parse also reports Oniguruma's own compile errors ("end
//! pattern with unmatched parenthesis", "target of repeat operator is not
//! specified", "invalid backref number/name", …) with its texts. Constructs
//! PCRE2 cannot express faithfully are refused with a compile error instead
//! of matching differently: grapheme boundaries `\y` `\Y`, backrefs with a
//! nesting level other than 0, the `l` (find-longest) option, and the
//! absent-operator forms with `|`.
//!
//! **Encodings.** The regex encoding (`mb_regex_encoding`) is one of the
//! encodings Oniguruma has; subjects and patterns are checked in it, then
//! converted to UTF-8 for PCRE2, and every offset is mapped back to the
//! caller's bytes. Replacement strings are scanned in the regex encoding
//! byte by byte exactly as php's `mb_regex_substitute` does.
//!
//! **State.** The default options/syntax, the explicitly selected regex
//! encoding, the compiled-pattern cache and the `mb_ereg_search_*` cursor
//! are request state in the `Interp` ext slot [`SLOT`].
//!
//! Group names come out of `mb_ereg()` in the order Oniguruma's name table
//! (an old-style `st` hash with 11 bins) iterates, which [`name_order`]
//! reproduces.

use std::collections::HashMap;
use std::rc::Rc;

use rphp_pcre2::{Code, MatchContext, MatchData, MatchResult, UNSET};
use rphp_runtime::{Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use crate::libmbfl::{self as mbfl, ConvertBuf, Encoding, Id, BAD_INPUT};

/// A [`NativeFn`] row with parameter names (php's arginfo).
const fn row(
    name: &'static str,
    min: u8,
    max: u8,
    by_ref: u32,
    params: &'static [&'static str],
    handler: rphp_runtime::NativeHandler,
) -> NativeFn {
    NativeFn { name, min_args: min, max_args: Some(max), by_ref, params, flags: rphp_runtime::FnFlags::EMPTY, handler }
}

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    row("mb_regex_encoding", 0, 1, 0, &["encoding"], mb_regex_encoding),
    row("mb_ereg", 2, 3, 0b100, &["pattern", "string", "matches"], mb_ereg),
    row("mb_eregi", 2, 3, 0b100, &["pattern", "string", "matches"], mb_eregi),
    row("mb_ereg_replace", 3, 4, 0, &["pattern", "replacement", "string", "options"], mb_ereg_replace),
    row("mb_eregi_replace", 3, 4, 0, &["pattern", "replacement", "string", "options"], mb_eregi_replace),
    row(
        "mb_ereg_replace_callback",
        3,
        4,
        0,
        &["pattern", "callback", "string", "options"],
        mb_ereg_replace_callback,
    ),
    row("mb_split", 2, 3, 0, &["pattern", "string", "limit"], mb_split),
    row("mb_ereg_match", 2, 3, 0, &["pattern", "string", "options"], mb_ereg_match),
    row("mb_ereg_search", 0, 2, 0, &["pattern", "options"], mb_ereg_search),
    row("mb_ereg_search_pos", 0, 2, 0, &["pattern", "options"], mb_ereg_search_pos),
    row("mb_ereg_search_regs", 0, 2, 0, &["pattern", "options"], mb_ereg_search_regs),
    row("mb_ereg_search_init", 1, 3, 0, &["string", "pattern", "options"], mb_ereg_search_init),
    row("mb_ereg_search_getregs", 0, 0, 0, &[], mb_ereg_search_getregs),
    row("mb_ereg_search_getpos", 0, 0, 0, &[], mb_ereg_search_getpos),
    row("mb_ereg_search_setpos", 1, 1, 0, &["offset"], mb_ereg_search_setpos),
    row("mb_regex_set_options", 0, 1, 0, &["options"], mb_regex_set_options),
    row("mb_convert_kana", 1, 3, 0, &["string", "mode", "encoding"], mb_convert_kana),
    row(
        "mb_send_mail",
        3,
        5,
        0,
        &["to", "subject", "message", "additional_headers", "additional_params"],
        mb_send_mail,
    ),
];

#[allow(dead_code)]
pub(crate) fn register_classes(_r: &mut Registry) {}

#[allow(dead_code)]
pub(crate) fn register_constants(_r: &mut Registry) {}

// ---- options and syntaxes -------------------------------------------------------------

/// `ONIG_OPTION_*` bits php stores.
const OPT_IGNORECASE: u32 = 1;
const OPT_EXTEND: u32 = 2;
const OPT_MULTILINE: u32 = 4;
const OPT_SINGLELINE: u32 = 8;
const OPT_FIND_LONGEST: u32 = 16;
const OPT_FIND_NOT_EMPTY: u32 = 32;

/// PCRE2 bits `rphp-pcre2` does not name.
const PCRE2_MATCH_INVALID_UTF: u32 = 0x0400_0000;
const PCRE2_NOTEMPTY: u32 = 0x0000_0004;

/// The Oniguruma syntaxes php can select (`ONIG_SYNTAX_*`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Syntax {
    Java,
    GnuRegex,
    Grep,
    Emacs,
    Ruby,
    Perl,
    PosixBasic,
    PosixExtended,
}

impl Syntax {
    /// The option letter `_php_mb_regex_get_option_string` prints.
    fn letter(self) -> char {
        match self {
            Syntax::Java => 'j',
            Syntax::GnuRegex => 'u',
            Syntax::Grep => 'g',
            Syntax::Emacs => 'c',
            Syntax::Ruby => 'r',
            Syntax::Perl => 'z',
            Syntax::PosixBasic => 'b',
            Syntax::PosixExtended => 'd',
        }
    }

    /// `syntax->options`, or-ed into every pattern by `onig_new`.
    fn base_options(self) -> u32 {
        match self {
            Syntax::Perl | Syntax::Java => OPT_SINGLELINE,
            Syntax::PosixBasic | Syntax::PosixExtended => OPT_SINGLELINE | OPT_MULTILINE,
            _ => 0,
        }
    }

    fn feat(self) -> &'static Feat {
        match self {
            Syntax::Ruby => &RUBY,
            Syntax::Perl => &PERL,
            Syntax::Java => &JAVA,
            Syntax::GnuRegex => &GNU,
            Syntax::PosixExtended => &POSIX_EXT,
            Syntax::Grep => &GREP,
            Syntax::Emacs => &EMACS,
            Syntax::PosixBasic => &POSIX_BASIC,
        }
    }
}

/// The syntax operators (`ONIG_SYN_OP_*`, `ONIG_SYN_OP2_*`, the behaviour
/// bits) the translator consults, per syntax.
struct Feat {
    /// `+` and `?` repeat.
    plus_qmark: bool,
    /// `\+` and `\?` repeat (grep).
    esc_plus_qmark: bool,
    /// `|` alternates.
    vbar: bool,
    /// `\|` alternates.
    esc_vbar: bool,
    /// `(…)` groups.
    paren: bool,
    /// `\(…\)` groups.
    esc_paren: bool,
    /// `{n,m}` intervals.
    brace: bool,
    /// `\{n,m\}` intervals.
    esc_brace: bool,
    /// `(?…)` extended groups.
    qmark_group: bool,
    /// `(?<name>…)` / `(?'name'…)`, `\k<…>`, `\g<…>`.
    named: bool,
    /// Ruby's inline options (`m` is dot-all; no `s`).
    ruby_opts: bool,
    /// `\h` / `\H` are hex digits.
    esc_h: bool,
    /// `\Q…\E`.
    quote: bool,
    /// `\w \W \b \B`.
    esc_w: bool,
    /// `\d \D \s \S`.
    esc_d: bool,
    /// `[[:alpha:]]`.
    posix_bracket: bool,
    /// `\p{…}`.
    prop: bool,
    /// `{,n}`.
    low_abbrev: bool,
    /// `*+ ++ ?+` are possessive.
    possessive: bool,
    /// `{n,m}+` is possessive.
    possessive_interval: bool,
    /// `{n}?` is `(?:{n})?`.
    fixed_greedy_only: bool,
    /// `\<` `\>`.
    ltgt: bool,
    /// ``\` `` `\'`.
    backtick: bool,
    /// `\uHHHH`.
    esc_u: bool,
    /// `[a&&b]`, nested classes.
    class_set: bool,
    /// `\x{…}` and `\o{…}`.
    esc_brace_code: bool,
    /// `\A \z \Z \G`.
    esc_az: bool,
    /// `\v` is VT.
    esc_v: bool,
    /// A repeat with nothing to repeat is an error (else a literal).
    ctx_indep_repeat: bool,
    /// An invalid `{` is a literal (else an error).
    invalid_interval_literal: bool,
    /// `{3,1}` is a possessive `{1,3}` (else an error).
    swap_interval: bool,
    /// `(?~…)`.
    absent: bool,
    /// `\K \R \X \N`.
    esc_k: bool,
    /// `\C-x`, `\M-x`.
    ruby_meta: bool,
}

const fn feat_base() -> Feat {
    Feat {
        plus_qmark: false,
        esc_plus_qmark: false,
        vbar: false,
        esc_vbar: false,
        paren: false,
        esc_paren: false,
        brace: false,
        esc_brace: false,
        qmark_group: false,
        named: false,
        ruby_opts: false,
        esc_h: false,
        quote: false,
        esc_w: false,
        esc_d: false,
        posix_bracket: false,
        prop: false,
        low_abbrev: false,
        possessive: false,
        possessive_interval: false,
        fixed_greedy_only: false,
        ltgt: false,
        backtick: false,
        esc_u: false,
        class_set: false,
        esc_brace_code: false,
        esc_az: false,
        esc_v: false,
        ctx_indep_repeat: false,
        invalid_interval_literal: true,
        swap_interval: false,
        absent: false,
        esc_k: false,
        ruby_meta: false,
    }
}

static RUBY: Feat = Feat {
    plus_qmark: true,
    vbar: true,
    paren: true,
    brace: true,
    qmark_group: true,
    named: true,
    ruby_opts: true,
    esc_h: true,
    esc_w: true,
    esc_d: true,
    posix_bracket: true,
    prop: true,
    low_abbrev: true,
    possessive: true,
    fixed_greedy_only: true,
    esc_u: true,
    class_set: true,
    esc_brace_code: true,
    esc_az: true,
    esc_v: true,
    ctx_indep_repeat: true,
    swap_interval: true,
    absent: true,
    esc_k: true,
    ruby_meta: true,
    ..feat_base()
};

static PERL: Feat = Feat {
    plus_qmark: true,
    vbar: true,
    paren: true,
    brace: true,
    qmark_group: true,
    quote: true,
    esc_w: true,
    esc_d: true,
    posix_bracket: true,
    prop: true,
    possessive: true,
    possessive_interval: true,
    esc_brace_code: true,
    esc_az: true,
    ctx_indep_repeat: true,
    absent: true,
    esc_k: true,
    ..feat_base()
};

static JAVA: Feat = Feat {
    plus_qmark: true,
    vbar: true,
    paren: true,
    brace: true,
    qmark_group: true,
    quote: true,
    esc_w: true,
    esc_d: true,
    prop: true,
    possessive: true,
    possessive_interval: true,
    esc_u: true,
    class_set: true,
    esc_az: true,
    esc_v: true,
    ctx_indep_repeat: true,
    ..feat_base()
};

static GNU: Feat = Feat {
    plus_qmark: true,
    vbar: true,
    paren: true,
    brace: true,
    esc_w: true,
    esc_d: true,
    posix_bracket: true,
    ltgt: true,
    esc_az: true,
    ctx_indep_repeat: true,
    swap_interval: true,
    ..feat_base()
};

static POSIX_EXT: Feat = Feat {
    plus_qmark: true,
    vbar: true,
    paren: true,
    brace: true,
    posix_bracket: true,
    ctx_indep_repeat: true,
    invalid_interval_literal: false,
    swap_interval: true,
    ..feat_base()
};

static GREP: Feat = Feat {
    esc_plus_qmark: true,
    esc_vbar: true,
    esc_paren: true,
    esc_brace: true,
    esc_w: true,
    posix_bracket: true,
    ltgt: true,
    ..feat_base()
};

static EMACS: Feat = Feat {
    plus_qmark: true,
    esc_vbar: true,
    esc_paren: true,
    esc_brace: true,
    backtick: true,
    ..feat_base()
};

static POSIX_BASIC: Feat = Feat {
    esc_paren: true,
    esc_brace: true,
    posix_bracket: true,
    ..feat_base()
};

/// `_php_mb_regex_init_options`: the option bits and syntax an option
/// string selects (the syntax is Ruby unless a syntax letter says
/// otherwise). `Err` carries the unsupported letter, together with what
/// was parsed before it (php keeps using that).
fn parse_options(s: &[u8]) -> Result<(u32, Syntax), (u8, u32, Syntax)> {
    let mut opt = 0;
    let mut syn = Syntax::Ruby;
    for &c in s {
        match c {
            b'i' => opt |= OPT_IGNORECASE,
            b'x' => opt |= OPT_EXTEND,
            b'm' => opt |= OPT_MULTILINE,
            b's' => opt |= OPT_SINGLELINE,
            b'p' => opt |= OPT_MULTILINE | OPT_SINGLELINE,
            b'l' => opt |= OPT_FIND_LONGEST,
            b'n' => opt |= OPT_FIND_NOT_EMPTY,
            b'j' => syn = Syntax::Java,
            b'u' => syn = Syntax::GnuRegex,
            b'g' => syn = Syntax::Grep,
            b'c' => syn = Syntax::Emacs,
            b'r' => syn = Syntax::Ruby,
            b'z' => syn = Syntax::Perl,
            b'b' => syn = Syntax::PosixBasic,
            b'd' => syn = Syntax::PosixExtended,
            _ => return Err((c, 0, syn)),
        }
    }
    Ok((opt, syn))
}

fn unsupported_option(c: u8) -> Unwind {
    Unwind::value_error(format!("Option \"{}\" is not supported", c as char))
}

/// `_php_mb_regex_get_option_string`.
fn option_string(opt: u32, syn: Syntax) -> String {
    let mut s = String::new();
    if opt & OPT_IGNORECASE != 0 {
        s.push('i');
    }
    if opt & OPT_EXTEND != 0 {
        s.push('x');
    }
    if opt & (OPT_MULTILINE | OPT_SINGLELINE) == OPT_MULTILINE | OPT_SINGLELINE {
        s.push('p');
    } else {
        if opt & OPT_MULTILINE != 0 {
            s.push('m');
        }
        if opt & OPT_SINGLELINE != 0 {
            s.push('s');
        }
    }
    if opt & OPT_FIND_LONGEST != 0 {
        s.push('l');
    }
    if opt & OPT_FIND_NOT_EMPTY != 0 {
        s.push('n');
    }
    s.push(syn.letter());
    s
}

// ---- regex encodings ------------------------------------------------------------------

/// php's `enc_name_map`: the encodings Oniguruma has, by the names
/// `mb_regex_encoding()` accepts (case-insensitively). The first name is
/// the one the getter reports.
static ENC_MAP: &[&[&str]] = &[
    &["EUC-JP", "EUCJP", "X-EUC-JP", "UJIS", "EUCJP", "EUCJP-WIN"],
    &["UTF-8", "UTF8"],
    &["UTF-16", "UTF-16BE"],
    &["UTF-16LE"],
    &["UCS-4", "UTF-32", "UTF-32BE"],
    &["UCS-4LE", "UTF-32LE"],
    &["SJIS", "CP932", "MS932", "SHIFT_JIS", "SJIS-WIN", "WINDOWS-31J"],
    &["BIG5", "BIG-5", "BIGFIVE", "CN-BIG5", "BIG-FIVE"],
    &["EUC-CN", "EUCCN", "EUC_CN", "GB-2312", "GB2312"],
    &["EUC-TW", "EUCTW", "EUC_TW"],
    &["EUC-KR", "EUCKR", "EUC_KR"],
    &["KOI8R", "KOI8-R", "KOI-8R"],
    &["ISO-8859-1", "ISO8859-1"],
    &["ISO-8859-2", "ISO8859-2"],
    &["ISO-8859-3", "ISO8859-3"],
    &["ISO-8859-4", "ISO8859-4"],
    &["ISO-8859-5", "ISO8859-5"],
    &["ISO-8859-6", "ISO8859-6"],
    &["ISO-8859-7", "ISO8859-7"],
    &["ISO-8859-8", "ISO8859-8"],
    &["ISO-8859-9", "ISO8859-9"],
    &["ISO-8859-10", "ISO8859-10"],
    &["ISO-8859-11", "ISO8859-11"],
    &["ISO-8859-13", "ISO8859-13"],
    &["ISO-8859-14", "ISO8859-14"],
    &["ISO-8859-15", "ISO8859-15"],
    &["ISO-8859-16", "ISO8859-16"],
    &["ASCII", "US-ASCII", "US_ASCII", "ISO646"],
];

/// `ENC_MAP` index of UTF-8.
const ENC_UTF8: usize = 1;

/// `_php_mb_regex_name2mbctype`.
fn enc_index(name: &[u8]) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    ENC_MAP.iter().position(|names| names.iter().any(|n| n.as_bytes().eq_ignore_ascii_case(name)))
}

/// Whether the Oniguruma encoding is a non-Unicode multibyte one.
fn enc_mb_ctype(idx: usize) -> bool {
    matches!(idx, 0 | 6..=10)
}

/// Whether the Oniguruma encoding is a Unicode one (`\x{…}` is a code
/// point rather than an encoded code value).
fn enc_is_unicode(idx: usize) -> bool {
    (1..=5).contains(&idx)
}

/// The regex encoding in effect: `(ENC_MAP index, libmbfl encoding)`.
#[derive(Clone, Copy)]
struct RegexEnc {
    idx: usize,
    mbfl: &'static Encoding,
}

impl RegexEnc {
    fn name(&self) -> &'static str {
        ENC_MAP[self.idx][0]
    }
    fn is_utf8(&self) -> bool {
        self.idx == ENC_UTF8
    }
}

/// The libmbfl encoding behind a name, UTF-8 for names libmbfl lacks.
fn mbfl_for(name: &[u8]) -> &'static Encoding {
    mbfl::name2encoding(name).unwrap_or(&mbfl::UTF8)
}

// ---- request state --------------------------------------------------------------------

/// The `Interp` ext-slot key.
const SLOT: &str = "mbregex";

/// php's `MBREX(…)` globals.
struct State {
    /// `MBREX(regex_default_options)`.
    options: u32,
    /// `MBREX(regex_default_syntax)`.
    syntax: Syntax,
    /// `mb_regex_encoding($x)`; `None` follows the internal-encoding ini.
    enc: Option<RegexEnc>,
    /// `MBREX(ht_rc)`.
    cache: HashMap<(Vec<u8>, u32, Syntax, usize), Rc<Compiled>>,
    /// `MBREX(search_str)`.
    search_str: Option<Vec<u8>>,
    /// `MBREX(search_pos)`.
    search_pos: usize,
    /// `MBREX(search_re)`.
    search_re: Option<Rc<Compiled>>,
    /// `MBREX(search_regs)` (byte offsets into `search_str`).
    search_regs: Option<Region>,
}

impl Default for State {
    fn default() -> Self {
        State {
            options: OPT_MULTILINE | OPT_SINGLELINE,
            syntax: Syntax::Ruby,
            enc: None,
            cache: HashMap::new(),
            search_str: None,
            search_pos: 0,
            search_re: None,
            search_regs: None,
        }
    }
}

fn state<'c>(ctx: &'c mut Ctx<'_>) -> &'c mut State {
    ctx.ext.slot::<State>(SLOT)
}

/// The current regex encoding: an explicit `mb_regex_encoding()`, else the
/// one the internal-encoding directives name (php's ini handler sets it),
/// UTF-8 when that is not an Oniguruma encoding.
fn regex_enc(ctx: &mut Ctx) -> RegexEnc {
    if let Some(e) = state(ctx).enc {
        return e;
    }
    let mut name = ctx.ini_get("mbstring.internal_encoding").unwrap_or("").to_string();
    if name.is_empty() {
        name = ctx.ini_get("internal_encoding").unwrap_or("").to_string();
    }
    if name.is_empty() {
        name = ctx.ini_get("default_charset").unwrap_or("").to_string();
    }
    match enc_index(name.as_bytes()) {
        Some(idx) => RegexEnc { idx, mbfl: mbfl_for(name.as_bytes()) },
        None => RegexEnc { idx: ENC_UTF8, mbfl: &mbfl::UTF8 },
    }
}

// ---- argument helpers -----------------------------------------------------------------

fn str_arg(args: &[Value], i: usize, func: &str, name: &str) -> Result<Vec<u8>, Unwind> {
    let v = args.get(i).cloned().unwrap_or(Value::Null);
    match v.deref().as_ref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{} (${name}) must be of type string, {} given",
            i + 1,
            rphp_runtime::value_name(&v)
        ))),
        _ => Ok(v.to_php_bytes()),
    }
}

fn opt_str_arg(args: &[Value], i: usize, func: &str, name: &str) -> Result<Option<Vec<u8>>, Unwind> {
    match args.get(i).map(|v| v.deref()) {
        None => Ok(None),
        Some(v) if matches!(v.as_ref(), Value::Null) => Ok(None),
        Some(_) => str_arg(args, i, func, name).map(Some),
    }
}

fn str_value(b: &[u8]) -> Value {
    Value::string(b)
}

// ---- Oniguruma errors ------------------------------------------------------------------

const E_END_AT_ESCAPE: &str = "end pattern at escape";
const E_END_AT_META: &str = "end pattern at meta";
const E_END_AT_CONTROL: &str = "end pattern at control";
const E_META_SYNTAX: &str = "invalid meta-code syntax";
const E_CONTROL_SYNTAX: &str = "invalid control-code syntax";
const E_PREMATURE_CC: &str = "premature end of char-class";
const E_EMPTY_CC: &str = "empty char-class";
const E_CC_END_OF_RANGE: &str = "char-class value at end of range";
const E_UNMATCHED_RANGE: &str = "unmatched range specifier in char-class";
const E_EMPTY_RANGE: &str = "empty range in char class";
const E_TARGET_NOT_SPECIFIED: &str = "target of repeat operator is not specified";
const E_TARGET_INVALID: &str = "target of repeat operator is invalid";
const E_UNMATCHED_CLOSE: &str = "unmatched close parenthesis";
const E_UNMATCHED_PAREN: &str = "end pattern with unmatched parenthesis";
const E_END_IN_GROUP: &str = "end pattern in group";
const E_GROUP_OPTION: &str = "undefined group option";
const E_POSIX_BRACKET: &str = "invalid POSIX bracket type";
const E_TOO_BIG_REPEAT: &str = "too big number for repeat range";
const E_UPPER_SMALLER: &str = "upper is smaller than lower in repeat range";
const E_INVALID_REPEAT: &str = "invalid repeat range {lower,upper}";
const E_TOO_SHORT_MB: &str = "too short multibyte code string";
const E_INVALID_BACKREF: &str = "invalid backref number/name";
const E_NUMBERED_REF: &str = "numbered backref/call is not allowed. (use name)";
const E_INVALID_CODE_POINT: &str = "invalid code point value";
const E_EMPTY_GROUP_NAME: &str = "group name is empty";
const E_TOO_BIG_NUMBER: &str = "too big number";
const E_LOOKBEHIND: &str = "invalid pattern in look-behind";

fn err_name(what: &str, name: &[u8]) -> String {
    format!("{what} <{}>", String::from_utf8_lossy(name))
}

// ---- the translator ---------------------------------------------------------------------

/// What `Translator` produces: a PCRE2 pattern and the capture layout.
struct Translated {
    pattern: Vec<u8>,
    /// Capturing groups (Oniguruma's `num_mem`).
    ncaps: usize,
    /// Group names (UTF-8) with their group numbers, in definition order.
    names: Vec<(Vec<u8>, Vec<usize>)>,
}

/// The inline options in effect.
#[derive(Clone, Copy)]
struct Opts {
    icase: bool,
    extend: bool,
    dotall: bool,
    singleline: bool,
}

/// What a group does when it closes.
struct Frame {
    /// Text emitted at `)`.
    close: &'static str,
    /// The `(?:` wrappers opened for isolated option switches inside.
    extra: usize,
    /// The options to restore.
    saved: Opts,
    /// Output offset where the group starts (the repeat target).
    start: usize,
    /// Lookarounds cannot be repeated.
    assertion: bool,
    /// `(?~…)` renders as a repeat already.
    repeated: bool,
    /// Whether this frame was opened by `\(` (grep/emacs/basic).
    escaped: bool,
}

/// One class element.
enum CItem {
    Char(u32),
    Range(u32, u32),
    /// A PCRE2 class fragment (`\w`, `[:alpha:]`, `\p{L}`, `0-9a-f`).
    Text(String),
    Nested(ClassExpr),
    /// Matches nothing (an undecodable raw byte).
    Never,
}

/// `[…]`: the `&&`-separated terms, each a union of items.
struct ClassExpr {
    neg: bool,
    terms: Vec<Vec<CItem>>,
}

/// A deferred reference check (`numbered_ref_check`, `setup_call`,
/// `setup_tree` run after the parse).
enum Check {
    NumberedRef,
    Backref(usize),
    Call(usize),
}

struct Translator<'a> {
    p: &'a [u8],
    i: usize,
    out: Vec<u8>,
    f: &'static Feat,
    ruby: bool,
    enc: RegexEnc,
    opts: Opts,
    frames: Vec<Frame>,
    /// The start of the current repeat target, if there is one.
    atom: Option<usize>,
    /// The target is an anchor or lookaround.
    atom_invalid: bool,
    /// The target already carries a repeat.
    quantified: bool,
    /// Named groups exist (plain groups do not capture).
    names_mode: bool,
    ncaps: usize,
    names: Vec<(Vec<u8>, Vec<usize>)>,
    checks: Vec<Check>,
    /// Isolated option switches at the top level (closed at the end).
    top_extra: usize,
}

fn is_space(c: u32) -> bool {
    matches!(c, 0x09..=0x0D | 0x20 | 0x85 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000)
}

/// Decode the UTF-8 character at `p[i..]` (the pattern is valid UTF-8).
fn utf8_at(p: &[u8], i: usize) -> (u32, usize) {
    let b = p[i];
    let n = match b {
        0x00..=0x7F => return (b as u32, 1),
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    };
    let end = (i + n).min(p.len());
    match std::str::from_utf8(&p[i..end]).ok().and_then(|s| s.chars().next()) {
        Some(c) => (c as u32, c.len_utf8()),
        None => (b as u32, 1),
    }
}

fn push_utf8(out: &mut Vec<u8>, c: u32) {
    let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
    let mut b = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
}

/// A literal code point, escaped for PCRE2 (outside or inside a class).
fn push_literal(out: &mut Vec<u8>, c: u32) {
    if c < 0x80 {
        let b = c as u8;
        if b.is_ascii_alphanumeric() || b == b'_' {
            out.push(b);
        } else if b.is_ascii_graphic() || b == b' ' {
            out.push(b'\\');
            out.push(b);
        } else {
            out.extend_from_slice(format!("\\x{{{c:x}}}").as_bytes());
        }
    } else {
        push_utf8(out, c);
    }
}

/// The POSIX bracket names Oniguruma knows.
static POSIX_NAMES: &[&str] =
    &["alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space", "upper", "xdigit", "word", "ascii"];

/// The Unicode blocks `\p{In_…}` is translated for (PCRE2 has no blocks).
static BLOCKS: &[(&str, u32, u32)] = &[
    ("basiclatin", 0x0000, 0x007F),
    ("latin1supplement", 0x0080, 0x00FF),
    ("latinextendeda", 0x0100, 0x017F),
    ("latinextendedb", 0x0180, 0x024F),
    ("ipaextensions", 0x0250, 0x02AF),
    ("greekandcoptic", 0x0370, 0x03FF),
    ("cyrillic", 0x0400, 0x04FF),
    ("armenian", 0x0530, 0x058F),
    ("hebrew", 0x0590, 0x05FF),
    ("arabic", 0x0600, 0x06FF),
    ("devanagari", 0x0900, 0x097F),
    ("thai", 0x0E00, 0x0E7F),
    ("hangul jamo", 0x1100, 0x11FF),
    ("hanguljamo", 0x1100, 0x11FF),
    ("generalpunctuation", 0x2000, 0x206F),
    ("currencysymbols", 0x20A0, 0x20CF),
    ("letterlikesymbols", 0x2100, 0x214F),
    ("numberforms", 0x2150, 0x218F),
    ("arrows", 0x2190, 0x21FF),
    ("mathematicaloperators", 0x2200, 0x22FF),
    ("boxdrawing", 0x2500, 0x257F),
    ("geometricshapes", 0x25A0, 0x25FF),
    ("miscellaneoussymbols", 0x2600, 0x26FF),
    ("cjkradicalssupplement", 0x2E80, 0x2EFF),
    ("cjksymbolsandpunctuation", 0x3000, 0x303F),
    ("hiragana", 0x3040, 0x309F),
    ("katakana", 0x30A0, 0x30FF),
    ("bopomofo", 0x3100, 0x312F),
    ("hangulcompatibilityjamo", 0x3130, 0x318F),
    ("katakanaphoneticextensions", 0x31F0, 0x31FF),
    ("enclosedcjklettersandmonths", 0x3200, 0x32FF),
    ("cjkcompatibility", 0x3300, 0x33FF),
    ("cjkunifiedideographsextensiona", 0x3400, 0x4DBF),
    ("cjkunifiedideographs", 0x4E00, 0x9FFF),
    ("hangulsyllables", 0xAC00, 0xD7AF),
    ("privateusearea", 0xE000, 0xF8FF),
    ("cjkcompatibilityideographs", 0xF900, 0xFAFF),
    ("halfwidthandfullwidthforms", 0xFF00, 0xFFEF),
    ("specials", 0xFFF0, 0xFFFF),
];

/// Oniguruma's property-name normalisation: case-insensitive, spaces,
/// underscores and hyphens ignored.
fn norm_prop(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, ' ' | '_' | '-')).flat_map(|c| c.to_lowercase()).collect()
}

/// Whether PCRE2 knows `\p{name}`.
fn pcre_has_prop(name: &str) -> bool {
    if name.is_empty() || name.contains(['{', '}', '\\', '&', '=', ':']) {
        return false;
    }
    let pat = format!("\\p{{{name}}}");
    Code::compile(pat.as_bytes(), rphp_pcre2::opt::UTF | rphp_pcre2::opt::UCP, 0).is_ok()
}

/// A property as a class fragment (for use inside `[…]`), with `neg`.
fn prop_fragment(name: &str, neg: bool) -> Option<String> {
    let n = norm_prop(name);
    if n == "xdigit" {
        // PCRE2's UCP `[:xdigit:]` takes the fullwidth digits too.
        return if neg { None } else { Some(String::from("0-9A-Fa-f")) };
    }
    if POSIX_NAMES.contains(&n.as_str()) {
        return Some(format!("[:{}{n}:]", if neg { "^" } else { "" }));
    }
    match n.as_str() {
        "any" => return Some(if neg { String::from("\\P{Any}") } else { String::from("\\p{Any}") }),
        "assigned" => return Some(if neg { String::from("\\p{Cn}") } else { String::from("\\P{Cn}") }),
        _ => {}
    }
    if let Some(block) = n.strip_prefix("in") {
        if let Some(&(_, lo, hi)) = BLOCKS.iter().find(|b| b.0 == block) {
            if neg {
                return None;
            }
            return Some(format!("\\x{{{lo:x}}}-\\x{{{hi:x}}}"));
        }
    }
    if pcre_has_prop(name) {
        return Some(format!("\\{}{{{name}}}", if neg { 'P' } else { 'p' }));
    }
    None
}

/// Onig's `EncLen_UTF8`.
fn utf8_enclen(b: u8) -> usize {
    match b {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        0xF8..=0xFB => 5,
        0xFC..=0xFD => 6,
        _ => 1,
    }
}

impl<'a> Translator<'a> {
    fn new(p: &'a [u8], opt: u32, syn: Syntax, enc: RegexEnc) -> Translator<'a> {
        let opt = opt | syn.base_options();
        Translator {
            p,
            i: 0,
            out: Vec::with_capacity(p.len() * 2 + 16),
            f: syn.feat(),
            ruby: syn == Syntax::Ruby,
            enc,
            opts: Opts {
                icase: opt & OPT_IGNORECASE != 0,
                extend: opt & OPT_EXTEND != 0,
                dotall: opt & OPT_MULTILINE != 0,
                singleline: opt & OPT_SINGLELINE != 0,
            },
            frames: Vec::new(),
            atom: None,
            atom_invalid: false,
            quantified: false,
            names_mode: false,
            ncaps: 0,
            names: Vec::new(),
            checks: Vec::new(),
            top_extra: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.p.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<u8> {
        self.p.get(self.i + k).copied()
    }

    fn eof(&self) -> bool {
        self.i >= self.p.len()
    }

    /// The next character (code point, byte length) without consuming it.
    fn peek_char(&self) -> Option<(u32, usize)> {
        if self.eof() {
            None
        } else {
            Some(utf8_at(self.p, self.i))
        }
    }

    fn next_char(&mut self) -> Option<u32> {
        let (c, n) = self.peek_char()?;
        self.i += n;
        Some(c)
    }

    /// Begin a repeatable atom at the current output position.
    fn begin_atom(&mut self) {
        self.atom = Some(self.out.len());
        self.atom_invalid = false;
        self.quantified = false;
    }

    /// Emit an anchor (a repeat after it is "invalid").
    fn anchor(&mut self, text: &str) {
        self.atom = Some(self.out.len());
        self.atom_invalid = true;
        self.quantified = false;
        self.out.extend_from_slice(text.as_bytes());
    }

    fn lit(&mut self, c: u32) {
        self.begin_atom();
        // Oniguruma's case folding includes the one-to-many folds.
        let multi = match c {
            0xDF | 0x1E9E => Some("ss"),
            0xFB00 => Some("ff"),
            0xFB01 => Some("fi"),
            0xFB02 => Some("fl"),
            0xFB03 => Some("ffi"),
            0xFB04 => Some("ffl"),
            0xFB05 | 0xFB06 => Some("st"),
            _ => None,
        };
        if let (true, Some(m)) = (self.opts.icase, multi) {
            self.out.extend_from_slice(b"(?:");
            push_literal(&mut self.out, c);
            self.out.push(b'|');
            self.out.extend_from_slice(m.as_bytes());
            self.out.push(b')');
            return;
        }
        push_literal(&mut self.out, c);
    }

    fn atom_text(&mut self, text: &str) {
        self.begin_atom();
        self.out.extend_from_slice(text.as_bytes());
    }

    /// A multibyte non-Unicode regex encoding (EUC-JP, SJIS, BIG5,
    /// EUC-CN/TW/KR): Oniguruma's ctypes are ASCII there, except that
    /// every non-ASCII character is a word / graph / print character.
    fn mb_ctype(&self) -> bool {
        enc_mb_ctype(self.enc.idx)
    }

    /// Pre-scan: does the pattern define a named group? (Ruby's
    /// `ONIG_SYN_CAPTURE_ONLY_NAMED_GROUP` makes plain groups
    /// non-capturing then.)
    fn prescan_names(&self) -> bool {
        if !self.f.named {
            return false;
        }
        let p = self.p;
        let mut i = 0;
        let mut in_class = 0usize;
        while i < p.len() {
            match p[i] {
                b'\\' => i += 2,
                b'[' => {
                    in_class += 1;
                    i += 1;
                }
                b']' if in_class > 0 => {
                    in_class -= 1;
                    i += 1;
                }
                b'(' if in_class == 0 && p.get(i + 1) == Some(&b'?') => {
                    match (p.get(i + 2), p.get(i + 3)) {
                        (Some(b'<'), Some(c)) if *c != b'=' && *c != b'!' => return true,
                        (Some(b'\''), _) => return true,
                        _ => {}
                    }
                    i += 2;
                }
                _ => i += 1,
            }
        }
        false
    }

    fn translate(mut self) -> Result<Translated, String> {
        self.names_mode = self.prescan_names();
        self.out.extend_from_slice(b"(*LF)");
        if self.opts.icase {
            self.out.extend_from_slice(b"(?i)");
        }
        self.parse_seq()?;
        if !self.frames.is_empty() {
            return Err(E_UNMATCHED_PAREN.into());
        }
        for _ in 0..self.top_extra {
            self.out.push(b')');
        }
        // The deferred checks, in Oniguruma's order.
        if self.names_mode && self.checks.iter().any(|c| matches!(c, Check::NumberedRef)) {
            return Err(E_NUMBERED_REF.into());
        }
        for c in &self.checks {
            if let Check::Call(n) = c {
                if *n > self.ncaps {
                    return Err(format!("undefined group <{n}> reference"));
                }
            }
        }
        for c in &self.checks {
            if let Check::Backref(n) = c {
                if *n > self.ncaps || *n == 0 {
                    return Err(E_INVALID_BACKREF.into());
                }
            }
        }
        Ok(Translated { pattern: self.out, ncaps: self.ncaps, names: self.names })
    }

    /// Skip whitespace and `#` comments under the `x` option.
    fn skip_extended(&mut self) {
        while self.opts.extend {
            match self.peek_char() {
                Some((c, n)) if is_space(c) => self.i += n,
                Some((0x23, _)) => {
                    while let Some(b) = self.peek() {
                        self.i += 1;
                        if b == b'\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// The main loop: until the end of the pattern (or the group's `)`,
    /// which the caller handles).
    fn parse_seq(&mut self) -> Result<(), String> {
        loop {
            self.skip_extended();
            let Some(b) = self.peek() else {
                // Close the isolated-option wrappers of the top level.
                return Ok(());
            };
            match b {
                b'(' if self.f.paren => {
                    self.i += 1;
                    self.open_group(false)?;
                }
                b')' if self.f.paren => {
                    self.i += 1;
                    self.close_group(false)?;
                }
                b'|' if self.f.vbar => {
                    self.i += 1;
                    self.out.push(b'|');
                    self.atom = None;
                    self.quantified = false;
                }
                b'[' => {
                    self.i += 1;
                    let start = self.out.len();
                    let cls = self.parse_class()?;
                    self.atom = Some(start);
                    self.atom_invalid = false;
                    self.quantified = false;
                    let text = render_class(&cls);
                    self.out.extend_from_slice(text.as_bytes());
                }
                b'.' => {
                    self.i += 1;
                    self.atom_text(if self.opts.dotall { "(?s:.)" } else { "\\N" });
                }
                b'^' => {
                    self.i += 1;
                    self.anchor(if self.opts.singleline { "\\A" } else { "(?m:^)" });
                }
                b'$' => {
                    self.i += 1;
                    self.anchor(if self.opts.singleline { "\\Z" } else { "(?m:$)" });
                }
                b'*' => self.repeat(0, None, 1)?,
                b'+' if self.f.plus_qmark => self.repeat(1, None, 1)?,
                b'?' if self.f.plus_qmark => self.repeat(0, Some(1), 1)?,
                b'{' if self.f.brace => {
                    if let Some((lo, hi, len)) = self.interval(self.i + 1, false)? {
                        self.repeat_interval(lo, hi, len)?;
                    } else {
                        self.i += 1;
                        self.lit(u32::from(b'{'));
                    }
                }
                b'\\' => self.escape()?,
                _ => {
                    let c = self.next_char().unwrap_or(0);
                    self.lit(c);
                }
            }
        }
    }

    /// A repeat operator of `len` bytes at `self.i`: `{lo, hi}` (`None` =
    /// unbounded).
    fn repeat(&mut self, lo: u32, hi: Option<u32>, len: usize) -> Result<(), String> {
        let text = match (lo, hi) {
            (0, None) => "*".to_string(),
            (1, None) => "+".to_string(),
            (0, Some(1)) => "?".to_string(),
            (l, None) => format!("{{{l},}}"),
            (l, Some(h)) if l == h => format!("{{{l}}}"),
            (l, Some(h)) => format!("{{{l},{h}}}"),
        };
        let Some(start) = self.atom else {
            if !self.f.ctx_indep_repeat {
                let c = self.p[self.i];
                self.i += len;
                self.lit(u32::from(c));
                return Ok(());
            }
            return Err(E_TARGET_NOT_SPECIFIED.into());
        };
        if self.atom_invalid {
            return Err(E_TARGET_INVALID.into());
        }
        self.i += len;
        if self.quantified {
            self.out.splice(start..start, b"(?:".iter().copied());
            self.out.push(b')');
        }
        self.out.extend_from_slice(text.as_bytes());
        self.quantified = true;
        // Lazy / possessive suffixes.
        match self.peek() {
            Some(b'?') if self.f.plus_qmark || self.f.esc_plus_qmark => {
                self.i += 1;
                self.out.push(b'?');
            }
            Some(b'+') if self.f.possessive => {
                self.i += 1;
                self.out.push(b'+');
            }
            _ => {}
        }
        Ok(())
    }

    /// An interval `{…}` whose body starts at `at`: `Some((lo, hi, bytes
    /// consumed from the '{'))`, `None` when it is not an interval (a
    /// literal `{`). `esc` = the `\{…\}` spelling.
    fn interval(&self, at: usize, esc: bool) -> Result<Option<(u32, Option<u32>, usize)>, String> {
        let p = self.p;
        let mut j = at;
        let num = |j: &mut usize| -> Result<Option<u32>, String> {
            let s = *j;
            let mut v: u64 = 0;
            while *j < p.len() && p[*j].is_ascii_digit() {
                v = v * 10 + u64::from(p[*j] - b'0');
                if v > 100_000 {
                    return Err(E_TOO_BIG_REPEAT.into());
                }
                *j += 1;
            }
            Ok(if *j > s { Some(v as u32) } else { None })
        };
        let invalid = || -> Result<Option<(u32, Option<u32>, usize)>, String> {
            if self.f.invalid_interval_literal {
                Ok(None)
            } else {
                Err(E_INVALID_REPEAT.into())
            }
        };
        let lo = num(&mut j)?;
        let (lo, hi) = if p.get(j) == Some(&b',') {
            j += 1;
            let hi = num(&mut j)?;
            match (lo, hi) {
                (None, None) => return invalid(),
                (None, Some(h)) => {
                    if !self.f.low_abbrev {
                        return invalid();
                    }
                    (0, Some(h))
                }
                (Some(l), h) => (l, h),
            }
        } else {
            match lo {
                Some(l) => (l, Some(l)),
                None => return invalid(),
            }
        };
        if esc {
            if p.get(j) != Some(&b'\\') {
                return invalid();
            }
            j += 1;
        }
        if p.get(j) != Some(&b'}') {
            return invalid();
        }
        j += 1;
        Ok(Some((lo, hi, j - self.i)))
    }

    fn repeat_interval(&mut self, lo: u32, hi: Option<u32>, len: usize) -> Result<(), String> {
        let Some(start) = self.atom else {
            if !self.f.ctx_indep_repeat {
                self.i += 1;
                self.lit(u32::from(b'{'));
                return Ok(());
            }
            return Err(E_TARGET_NOT_SPECIFIED.into());
        };
        if self.atom_invalid {
            return Err(E_TARGET_INVALID.into());
        }
        let fixed = hi == Some(lo);
        let (lo, hi, possessive) = match hi {
            Some(h) if h < lo => {
                if !self.f.swap_interval {
                    return Err(E_UPPER_SMALLER.into());
                }
                (h, Some(lo), true)
            }
            _ => (lo, hi, false),
        };
        if lo > 65535 || hi.is_some_and(|h| h > 65535) {
            return Err(E_TOO_BIG_REPEAT.into());
        }
        self.i += len;
        if self.quantified {
            self.out.splice(start..start, b"(?:".iter().copied());
            self.out.push(b')');
        }
        let text = match hi {
            None => format!("{{{lo},}}"),
            Some(h) if h == lo => format!("{{{lo}}}"),
            Some(h) => format!("{{{lo},{h}}}"),
        };
        self.out.extend_from_slice(text.as_bytes());
        self.quantified = true;
        if possessive {
            self.out.push(b'+');
            return Ok(());
        }
        match self.peek() {
            Some(b'?') if !(fixed && self.f.fixed_greedy_only) && (self.f.plus_qmark || self.f.esc_plus_qmark) => {
                self.i += 1;
                self.out.push(b'?');
            }
            Some(b'+') if self.f.possessive_interval => {
                self.i += 1;
                self.out.push(b'+');
            }
            _ => {}
        }
        Ok(())
    }

    /// After `(`.
    fn open_group(&mut self, escaped: bool) -> Result<(), String> {
        let start = self.out.len();
        let saved = self.opts;
        let mut frame = Frame { close: ")", extra: 0, saved, start, assertion: false, repeated: false, escaped };
        if !escaped && self.f.qmark_group && self.peek() == Some(b'?') {
            self.i += 1;
            let Some(c) = self.peek() else {
                return Err(E_END_IN_GROUP.into());
            };
            match c {
                b'#' => {
                    // A comment: skip to `)`.
                    while let Some(b) = self.peek() {
                        self.i += 1;
                        if b == b')' {
                            return Ok(());
                        }
                        if b == b'\\' {
                            self.i += 1;
                        }
                    }
                    return Err(E_END_IN_GROUP.into());
                }
                b':' => {
                    self.i += 1;
                    self.out.extend_from_slice(b"(?:");
                }
                b'=' | b'!' => {
                    self.i += 1;
                    self.out.extend_from_slice(if c == b'=' { b"(?=" } else { b"(?!" });
                    frame.assertion = true;
                }
                b'>' => {
                    self.i += 1;
                    self.out.extend_from_slice(b"(?>");
                }
                b'~' if self.f.absent => {
                    self.i += 1;
                    if self.peek() == Some(b'|') {
                        return Err(String::from("absent operator with '|' is not supported by rphp"));
                    }
                    self.out.extend_from_slice(b"(?:(?!");
                    frame.close = ")(?s:.))*";
                    frame.repeated = true;
                }
                b'<' if matches!(self.peek_at(1), Some(b'=') | Some(b'!')) => {
                    let neg = self.peek_at(1) == Some(b'!');
                    self.i += 2;
                    self.out.extend_from_slice(if neg { b"(?<!" } else { b"(?<=" });
                    frame.assertion = true;
                }
                b'<' | b'\'' if self.f.named => {
                    self.i += 1;
                    let close = if c == b'<' { b'>' } else { b'\'' };
                    let name = self.group_name(close, true)?;
                    self.ncaps += 1;
                    let n = self.ncaps;
                    match self.names.iter_mut().find(|(k, _)| *k == name) {
                        Some((_, nums)) => nums.push(n),
                        None => self.names.push((name, vec![n])),
                    }
                    self.out.push(b'(');
                }
                b'(' if self.ruby => {
                    // A conditional `(?(cond)yes|no)`.
                    self.i += 1;
                    let n = self.condition()?;
                    self.out.extend_from_slice(format!("(?({n})").as_bytes());
                }
                _ => {
                    // Options `(?imx-imx)` / `(?imx-imx:…)`.
                    if c == b')' {
                        return Err(E_GROUP_OPTION.into());
                    }
                    let mut on = true;
                    let mut o = self.opts;
                    loop {
                        let Some(c) = self.peek() else {
                            return Err(E_END_IN_GROUP.into());
                        };
                        self.i += 1;
                        match c {
                            b'-' => on = false,
                            b'i' => o.icase = on,
                            b'x' => o.extend = on,
                            b'm' if self.f.ruby_opts => o.dotall = on,
                            b'm' => o.singleline = !on,
                            b's' if !self.f.ruby_opts => o.dotall = on,
                            b')' => {
                                // Isolated: scopes over the rest of the
                                // enclosing group, alternatives included.
                                self.push_option_wrapper(o);
                                return Ok(());
                            }
                            b':' => {
                                self.out.extend_from_slice(Self::option_open(self.opts, o).as_bytes());
                                self.opts = o;
                                break;
                            }
                            _ => return Err(E_GROUP_OPTION.into()),
                        }
                    }
                }
            }
        } else if self.names_mode {
            self.out.extend_from_slice(b"(?:");
        } else {
            self.ncaps += 1;
            self.out.push(b'(');
        }
        self.frames.push(frame);
        self.atom = None;
        self.quantified = false;
        Ok(())
    }

    /// `(?i:` / `(?-i:` / `(?:` for switching from `from` to `to`.
    fn option_open(from: Opts, to: Opts) -> String {
        if from.icase == to.icase {
            String::from("(?:")
        } else if to.icase {
            String::from("(?i:")
        } else {
            String::from("(?-i:")
        }
    }

    fn push_option_wrapper(&mut self, o: Opts) {
        let open = Self::option_open(self.opts, o);
        self.out.extend_from_slice(open.as_bytes());
        self.opts = o;
        match self.frames.last_mut() {
            Some(f) => f.extra += 1,
            None => self.top_extra += 1,
        }
        self.atom = None;
        self.quantified = false;
    }

    fn close_group(&mut self, escaped: bool) -> Result<(), String> {
        match self.frames.last() {
            Some(f) if f.escaped == escaped => {}
            _ => return Err(E_UNMATCHED_CLOSE.into()),
        }
        let Some(frame) = self.frames.pop() else {
            return Err(E_UNMATCHED_CLOSE.into());
        };
        for _ in 0..frame.extra {
            self.out.push(b')');
        }
        self.out.extend_from_slice(frame.close.as_bytes());
        self.opts = frame.saved;
        self.atom = Some(frame.start);
        self.atom_invalid = frame.assertion;
        self.quantified = frame.repeated;
        Ok(())
    }

    /// A group name up to `close` (after `(?<` / `\k<` …). `definition`:
    /// a group name (must not start with a digit).
    fn group_name(&mut self, close: u8, definition: bool) -> Result<Vec<u8>, String> {
        let start = self.i;
        while let Some(b) = self.peek() {
            if b == close {
                break;
            }
            if definition && b == b')' {
                break;
            }
            let (_, n) = utf8_at(self.p, self.i);
            self.i += n;
        }
        let name = self.p[start..self.i].to_vec();
        if self.peek() != Some(close) {
            if name.is_empty() {
                return Err(E_EMPTY_GROUP_NAME.into());
            }
            return Err(err_name("invalid group name", &name));
        }
        self.i += 1;
        if name.is_empty() {
            return Err(E_EMPTY_GROUP_NAME.into());
        }
        if definition && name[0].is_ascii_digit() {
            return Err(err_name("invalid group name", &name));
        }
        if definition && name.iter().any(|&b| b < 0x80 && !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
            return Err(err_name("invalid char in group name", &name));
        }
        Ok(name)
    }

    /// The condition of `(?(…)`, up to and including its `)`: the group
    /// number it tests.
    fn condition(&mut self) -> Result<usize, String> {
        let n = match self.peek() {
            Some(b'<') | Some(b'\'') => {
                let close = if self.peek() == Some(b'<') { b'>' } else { b'\'' };
                self.i += 1;
                let name = self.group_name(close, false)?;
                self.resolve_ref(&name)?.last().copied().unwrap_or(0)
            }
            Some(b) if b.is_ascii_digit() => {
                let mut v = 0usize;
                while let Some(d) = self.peek().filter(u8::is_ascii_digit) {
                    v = v.saturating_mul(10).saturating_add(usize::from(d - b'0'));
                    self.i += 1;
                }
                self.checks.push(Check::NumberedRef);
                self.checks.push(Check::Backref(v));
                v
            }
            _ => return Err(String::from("invalid conditional pattern")),
        };
        if self.peek() != Some(b')') {
            return Err(String::from("invalid conditional pattern"));
        }
        self.i += 1;
        Ok(n)
    }

    /// A `\k<…>` / `\g<…>` reference body: the group numbers it names
    /// (a name must already be defined; numbers are checked at the end).
    fn resolve_ref(&mut self, name: &[u8]) -> Result<Vec<usize>, String> {
        let (sign, digits) = match name[0] {
            b'-' | b'+' => (Some(name[0]), &name[1..]),
            _ => (None, name),
        };
        if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
            let v: usize = std::str::from_utf8(digits).ok().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
            self.checks.push(Check::NumberedRef);
            let n = match sign {
                Some(b'-') => {
                    if v == 0 || v > self.ncaps {
                        return Err(E_INVALID_BACKREF.into());
                    }
                    self.ncaps + 1 - v
                }
                Some(_) => self.ncaps + v,
                None => v,
            };
            return Ok(vec![n]);
        }
        match self.names.iter().find(|(k, _)| k.as_slice() == name) {
            Some((_, nums)) => Ok(nums.clone()),
            None => Err(err_name("undefined name", name) + " reference"),
        }
    }

    /// After `\` outside a class.
    fn escape(&mut self) -> Result<(), String> {
        self.i += 1;
        let Some((c, n)) = self.peek_char() else {
            return Err(E_END_AT_ESCAPE.into());
        };
        if c >= 0x80 {
            self.i += n;
            self.lit(c);
            return Ok(());
        }
        let b = c as u8;
        let f = self.f;
        match b {
            b'(' if f.esc_paren => {
                self.i += 1;
                return self.open_group(true);
            }
            b')' if f.esc_paren => {
                self.i += 1;
                return self.close_group(true);
            }
            b'|' if f.esc_vbar => {
                self.i += 1;
                self.out.push(b'|');
                self.atom = None;
                self.quantified = false;
                return Ok(());
            }
            b'{' if f.esc_brace => {
                if let Some((lo, hi, len)) = self.interval(self.i + 1, true)? {
                    self.i -= 1;
                    return self.repeat_interval(lo, hi, len + 1);
                }
                self.i += 1;
                self.lit(u32::from(b'{'));
                return Ok(());
            }
            b'+' if f.esc_plus_qmark => {
                self.i -= 1;
                return self.repeat(1, None, 2);
            }
            b'?' if f.esc_plus_qmark => {
                self.i -= 1;
                return self.repeat(0, Some(1), 2);
            }
            _ => {}
        }
        self.i += 1;
        match b {
            b'w' | b'W' | b'b' | b'B' if f.esc_w => {
                let t = format!("\\{}", b as char);
                if b == b'b' || b == b'B' {
                    self.anchor(&t);
                } else if self.mb_ctype() {
                    self.atom_text(if b == b'w' { "[\\w\\x{80}-\\x{10ffff}]" } else { "[^\\w\\x{80}-\\x{10ffff}]" });
                } else {
                    self.atom_text(&t);
                }
            }
            b'd' | b'D' | b's' | b'S' if f.esc_d => self.atom_text(&format!("\\{}", b as char)),
            b'h' if f.esc_h => self.atom_text("[0-9a-fA-F]"),
            b'H' if f.esc_h => self.atom_text("[^0-9a-fA-F]"),
            b'A' | b'z' | b'Z' | b'G' if f.esc_az => self.anchor(&format!("\\{}", b as char)),
            b'`' if f.backtick => self.anchor("\\A"),
            b'\'' if f.backtick => self.anchor("\\z"),
            b'<' if f.ltgt => self.anchor("\\b(?=\\w)"),
            b'>' if f.ltgt => self.anchor("\\b(?<=\\w)"),
            b'K' if f.esc_k => self.anchor("\\K"),
            b'R' | b'X' | b'N' if f.esc_k => self.atom_text(&format!("\\{}", b as char)),
            b'y' | b'Y' if f.esc_k => {
                return Err(String::from("text segment boundaries (\\y, \\Y) are not supported by rphp"));
            }
            b'Q' if f.quote => {
                // Literal text up to `\E`.
                while !self.eof() {
                    if self.peek() == Some(b'\\') && self.peek_at(1) == Some(b'E') {
                        self.i += 2;
                        break;
                    }
                    let c = self.next_char().unwrap_or(0);
                    self.lit(c);
                }
            }
            b'p' | b'P' if f.prop && self.peek() == Some(b'{') => {
                let (frag, neg) = self.property(b == b'P')?;
                self.atom_text(&format!("[{}{frag}]", if neg { "^" } else { "" }));
            }
            b'k' if f.named && matches!(self.peek(), Some(b'<') | Some(b'\'')) => {
                let close = if self.peek() == Some(b'<') { b'>' } else { b'\'' };
                self.i += 1;
                let mut name = self.group_name(close, false)?;
                // A nesting level: only level 0 has the plain meaning.
                if let Some(pos) = name.iter().skip(1).position(|&c| c == b'+' || c == b'-').map(|p| p + 1) {
                    let level = &name[pos + 1..];
                    if !level.is_empty() && level.iter().all(|&c| c == b'0') {
                        name.truncate(pos);
                    } else {
                        return Err(String::from("backreferences with a nesting level are not supported by rphp"));
                    }
                }
                let nums = self.resolve_ref(&name)?;
                for &n in &nums {
                    self.checks.push(Check::Backref(n));
                }
                self.begin_atom();
                if nums.len() == 1 {
                    self.out.extend_from_slice(format!("\\g{{{}}}", nums[0]).as_bytes());
                } else {
                    let alts: Vec<String> = nums.iter().rev().map(|n| format!("\\g{{{n}}}")).collect();
                    self.out.extend_from_slice(format!("(?:{})", alts.join("|")).as_bytes());
                }
            }
            b'g' if f.named && matches!(self.peek(), Some(b'<') | Some(b'\'')) => {
                let close = if self.peek() == Some(b'<') { b'>' } else { b'\'' };
                self.i += 1;
                let name = self.group_name(close, false)?;
                let n = if name == b"0" {
                    0
                } else {
                    let nums = self.resolve_ref(&name)?;
                    if nums.len() > 1 {
                        return Err(err_name("multiplex definition name", &name) + " call");
                    }
                    self.checks.push(Check::Call(nums[0]));
                    nums[0]
                };
                self.atom_text(&format!("(?{n})"));
            }
            b'1'..=b'9' => self.digit_escape(b)?,
            b'0' => {
                let v = self.octal(2);
                self.raw_byte(v)?;
            }
            b'x' => self.hex_escape()?,
            b'u' if f.esc_u => {
                let mut v = 0u32;
                for _ in 0..4 {
                    match self.peek().and_then(|d| (d as char).to_digit(16)) {
                        Some(d) => {
                            v = v * 16 + d;
                            self.i += 1;
                        }
                        None => return Err(E_INVALID_CODE_POINT.into()),
                    }
                }
                self.code_value(v)?;
            }
            b'o' if f.esc_brace_code && self.peek() == Some(b'{') => {
                self.i += 1;
                let mut v: u64 = 0;
                let mut any = false;
                while let Some(d) = self.peek().filter(|d| (b'0'..=b'7').contains(d)) {
                    v = v * 8 + u64::from(d - b'0');
                    any = true;
                    self.i += 1;
                    if v > 0x7FFF_FFFF {
                        return Err(E_TOO_BIG_NUMBER.into());
                    }
                }
                if self.peek() != Some(b'}') {
                    if !any {
                        self.atom_text("(?!)");
                        return Ok(());
                    }
                    return Err(E_INVALID_CODE_POINT.into());
                }
                self.i += 1;
                if any {
                    self.code_value(v as u32)?;
                } else {
                    self.atom_text("(?!)");
                }
            }
            b't' => self.lit(0x09),
            b'n' => self.lit(0x0A),
            b'r' => self.lit(0x0D),
            b'f' => self.lit(0x0C),
            b'a' => self.lit(0x07),
            b'e' => self.lit(0x1B),
            b'v' if f.esc_v => self.lit(0x0B),
            b'c' => {
                let v = self.control()?;
                self.raw_byte(v)?;
            }
            b'C' if f.ruby_meta => {
                if self.eof() {
                    return Err(E_END_AT_CONTROL.into());
                }
                if self.peek() != Some(b'-') {
                    return Err(E_CONTROL_SYNTAX.into());
                }
                self.i += 1;
                let v = self.control()?;
                self.raw_byte(v)?;
            }
            b'M' if f.ruby_meta => {
                let v = self.meta()?;
                self.lit(u32::from(v));
            }
            _ => self.lit(c),
        }
        Ok(())
    }

    /// `\cX` (after the `c`): the control code of the next character.
    fn control(&mut self) -> Result<u8, String> {
        let Some(c) = self.peek() else {
            return Err(E_END_AT_CONTROL.into());
        };
        self.i += 1;
        if c == b'\\' {
            // `\c\M-x` and friends are rare; take the escaped char.
            let Some(d) = self.peek() else {
                return Err(E_END_AT_ESCAPE.into());
            };
            self.i += 1;
            return Ok(d & 0x9F);
        }
        Ok(if c == b'?' { 0x7F } else { c & 0x9F })
    }

    /// `\M-x` (after the `M`).
    fn meta(&mut self) -> Result<u8, String> {
        if self.eof() {
            return Err(E_END_AT_META.into());
        }
        if self.peek() != Some(b'-') {
            return Err(E_META_SYNTAX.into());
        }
        self.i += 1;
        let Some(c) = self.peek() else {
            return Err(E_END_AT_META.into());
        };
        self.i += 1;
        if c == b'\\' {
            match self.peek() {
                Some(b'C') => {
                    self.i += 1;
                    if self.peek() != Some(b'-') {
                        return Err(E_CONTROL_SYNTAX.into());
                    }
                    self.i += 1;
                    return Ok(self.control()? | 0x80);
                }
                Some(b'c') => {
                    self.i += 1;
                    return Ok(self.control()? | 0x80);
                }
                Some(d) => {
                    self.i += 1;
                    return Ok(d | 0x80);
                }
                None => return Err(E_END_AT_ESCAPE.into()),
            }
        }
        Ok(c | 0x80)
    }

    /// Up to `max` octal digits.
    fn octal(&mut self, max: usize) -> u8 {
        let mut v: u32 = 0;
        for _ in 0..max {
            match self.peek().filter(|d| (b'0'..=b'7').contains(d)) {
                Some(d) => {
                    v = v * 8 + u32::from(d - b'0');
                    self.i += 1;
                }
                None => break,
            }
        }
        v as u8
    }

    /// `\1`…`\9…` (after the first digit): a backreference, else an octal
    /// raw byte, else a literal digit (Oniguruma's `fetch_token`).
    fn digit_escape(&mut self, first: u8) -> Result<(), String> {
        let start = self.i - 1;
        let mut j = start;
        let mut num: u64 = 0;
        while j < self.p.len() && self.p[j].is_ascii_digit() {
            num = num.saturating_mul(10).saturating_add(u64::from(self.p[j] - b'0'));
            j += 1;
        }
        if num <= 1000 && (num as usize <= self.ncaps || num <= 9) {
            self.i = j;
            let n = num as usize;
            self.checks.push(Check::NumberedRef);
            self.checks.push(Check::Backref(n));
            self.atom_text(&format!("\\g{{{n}}}"));
            return Ok(());
        }
        if first == b'8' || first == b'9' {
            self.i = start + 1;
            self.lit(u32::from(first));
            return Ok(());
        }
        self.i = start;
        let v = self.octal(3);
        self.raw_byte(v)
    }

    /// `\x` (after the `x`).
    fn hex_escape(&mut self) -> Result<(), String> {
        if self.f.esc_brace_code && self.peek() == Some(b'{') {
            let save = self.i;
            self.i += 1;
            let mut v: u64 = 0;
            let mut digits = 0;
            while let Some(d) = self.peek().and_then(|d| (d as char).to_digit(16)) {
                v = v * 16 + u64::from(d);
                digits += 1;
                self.i += 1;
                if digits > 8 {
                    return Err(String::from("too long wide-char value"));
                }
            }
            if self.peek() == Some(b'}') {
                self.i += 1;
                if digits == 0 {
                    self.atom_text("(?!)");
                    return Ok(());
                }
                if v > 0x7FFF_FFFF {
                    return Err(String::from("too big wide-char value"));
                }
                return self.code_value(v as u32);
            }
            if digits > 0 {
                return Err(E_INVALID_CODE_POINT.into());
            }
            // `\x{` with nothing after: a literal `x`.
            self.i = save;
            self.lit(u32::from(b'x'));
            return Ok(());
        }
        if self.eof() {
            self.lit(u32::from(b'x'));
            return Ok(());
        }
        let mut v: u32 = 0;
        for _ in 0..2 {
            match self.peek().and_then(|d| (d as char).to_digit(16)) {
                Some(d) => {
                    v = v * 16 + d;
                    self.i += 1;
                }
                None => break,
            }
        }
        self.raw_byte(v as u8)
    }

    /// The next raw-byte escape (`\xHH`, `\0oo`, `\ooo`), if one follows.
    fn next_raw_byte(&mut self) -> Option<u8> {
        if self.peek() != Some(b'\\') {
            return None;
        }
        let save = self.i;
        self.i += 1;
        match self.peek() {
            Some(b'x') if self.peek_at(1).is_some_and(|d| d.is_ascii_hexdigit()) => {
                self.i += 1;
                let mut v: u32 = 0;
                for _ in 0..2 {
                    match self.peek().and_then(|d| (d as char).to_digit(16)) {
                        Some(d) => {
                            v = v * 16 + d;
                            self.i += 1;
                        }
                        None => break,
                    }
                }
                Some(v as u8)
            }
            Some(b'0'..=b'7') => {
                let d0 = self.peek().unwrap_or(b'0');
                if d0 == b'0' {
                    self.i += 1;
                    Some(self.octal(2))
                } else {
                    // Only a raw byte when it is not a backreference.
                    let mut j = self.i;
                    let mut num = 0u64;
                    while j < self.p.len() && self.p[j].is_ascii_digit() {
                        num = num.saturating_mul(10).saturating_add(u64::from(self.p[j] - b'0'));
                        j += 1;
                    }
                    if num as usize <= self.ncaps || num <= 9 {
                        self.i = save;
                        return None;
                    }
                    Some(self.octal(3))
                }
            }
            _ => {
                self.i = save;
                None
            }
        }
    }

    /// The character a raw byte starts (collecting the continuation raw
    /// bytes a multibyte character needs): `Ok(None)` when the bytes do
    /// not decode (the pattern then matches nothing there).
    fn raw_char(&mut self, first: u8) -> Result<Option<u32>, String> {
        let mut bytes = vec![first];
        let need = if self.enc.is_utf8() {
            utf8_enclen(first)
        } else {
            enc_char_len(self.enc.mbfl, &[first], 0).max(1)
        };
        while bytes.len() < need {
            match self.next_raw_byte() {
                Some(b) => bytes.push(b),
                None => return Err(E_TOO_SHORT_MB.into()),
            }
        }
        let w = self.enc.mbfl.decode(&bytes);
        Ok(match w.as_slice() {
            [c] if *c != BAD_INPUT => Some(*c),
            _ => None,
        })
    }

    fn raw_byte(&mut self, first: u8) -> Result<(), String> {
        match self.raw_char(first)? {
            Some(c) => self.lit(c),
            None => self.atom_text("(?!)"),
        }
        Ok(())
    }

    /// A `\x{…}` / `\uHHHH` value: a code point in the Unicode encodings,
    /// the encoded code value (big-endian bytes) in the others.
    fn code_value(&mut self, v: u32) -> Result<(), String> {
        match self.code_to_char(v)? {
            Some(c) => self.lit(c),
            None => self.atom_text("(?!)"),
        }
        Ok(())
    }

    fn code_to_char(&self, v: u32) -> Result<Option<u32>, String> {
        if enc_is_unicode(self.enc.idx) {
            if v > 0x10FFFF {
                return Ok(None);
            }
            if (0xD800..=0xDFFF).contains(&v) {
                return Err(E_INVALID_CODE_POINT.into());
            }
            return Ok(Some(v));
        }
        let bytes: Vec<u8> = v.to_be_bytes().iter().copied().skip_while(|&b| b == 0).collect();
        let bytes = if bytes.is_empty() { vec![0] } else { bytes };
        let w = self.enc.mbfl.decode(&bytes);
        Ok(match w.as_slice() {
            [c] if *c != BAD_INPUT => Some(*c),
            _ => None,
        })
    }

    /// `\p{…}` (at the `{`): a class fragment, and whether the set it
    /// names must be negated around it (blocks). `upper` = `\P`.
    fn property(&mut self, upper: bool) -> Result<(String, bool), String> {
        self.i += 1;
        let mut neg = upper;
        if self.peek() == Some(b'^') {
            self.i += 1;
            neg = !neg;
        }
        let start = self.i;
        while let Some(b) = self.peek() {
            if b == b'}' {
                break;
            }
            self.i += 1;
        }
        if self.eof() {
            return Err(E_UNMATCHED_PAREN.into());
        }
        let name = String::from_utf8_lossy(&self.p[start..self.i]).into_owned();
        self.i += 1;
        match prop_fragment(&name, neg) {
            Some(f) => Ok((f, false)),
            None if neg => match prop_fragment(&name, false) {
                Some(f) => Ok((f, true)),
                None => Err(format!("invalid character property name {{{name}}}")),
            },
            None => Err(format!("invalid character property name {{{name}}}")),
        }
    }

    // ---- character classes ----

    /// After `[`.
    fn parse_class(&mut self) -> Result<ClassExpr, String> {
        let mut expr = ClassExpr { neg: false, terms: vec![Vec::new()] };
        if self.peek() == Some(b'^') {
            self.i += 1;
            expr.neg = true;
        }
        // A `]` first is a literal when another `]` follows somewhere.
        if self.peek() == Some(b']') {
            if !self.p[self.i + 1..].contains(&b']') {
                return Err(E_EMPTY_CC.into());
            }
            self.i += 1;
            expr.terms[0].push(CItem::Char(u32::from(b']')));
        }
        // State: the last single value (for a range), whether it is a
        // class-like item, whether a `-` is pending.
        #[derive(PartialEq)]
        enum St {
            Start,
            Value,
            Class,
            Range,
            Complete,
        }
        let mut st = if expr.terms[0].is_empty() { St::Start } else { St::Value };
        loop {
            let Some(b) = self.peek() else {
                return Err(E_PREMATURE_CC.into());
            };
            let term = expr.terms.last_mut().expect("a term");
            match b {
                b']' => {
                    self.i += 1;
                    if st == St::Range {
                        // `[a-]`: the `-` is a literal.
                        term.push(CItem::Char(u32::from(b'-')));
                    }
                    return Ok(expr);
                }
                b'&' if self.f.class_set && self.peek_at(1) == Some(b'&') => {
                    self.i += 2;
                    if st == St::Range {
                        term.push(CItem::Char(u32::from(b'-')));
                    }
                    expr.terms.push(Vec::new());
                    st = St::Start;
                    continue;
                }
                b'-' => {
                    match st {
                        St::Start | St::Complete => {
                            self.i += 1;
                            if st == St::Complete && self.peek() != Some(b']') && !self.ruby {
                                return Err(E_UNMATCHED_RANGE.into());
                            }
                            self.push_item(&mut expr, CItem::Char(u32::from(b'-')));
                            st = St::Value;
                        }
                        St::Value => {
                            self.i += 1;
                            st = St::Range;
                        }
                        St::Class => {
                            self.i += 1;
                            if self.peek() == Some(b']') {
                                self.push_item(&mut expr, CItem::Char(u32::from(b'-')));
                                st = St::Value;
                            } else {
                                return Err(E_UNMATCHED_RANGE.into());
                            }
                        }
                        St::Range => {
                            // `[!--x]`: the `-` ends the range.
                            self.i += 1;
                            self.finish_range(&mut expr, u32::from(b'-'))?;
                            st = St::Complete;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            // One element.
            let item = self.class_item()?;
            match item {
                CItem::Char(c) => {
                    if st == St::Range {
                        self.finish_range(&mut expr, c)?;
                        st = St::Complete;
                    } else {
                        self.push_item(&mut expr, CItem::Char(c));
                        st = St::Value;
                    }
                }
                other => {
                    if st == St::Range {
                        return Err(E_CC_END_OF_RANGE.into());
                    }
                    self.push_item(&mut expr, other);
                    st = St::Class;
                }
            }
        }
    }

    fn push_item(&self, expr: &mut ClassExpr, item: CItem) {
        if let Some(t) = expr.terms.last_mut() {
            t.push(item);
        }
    }

    /// Close a pending range whose start is the term's last `Char`.
    fn finish_range(&self, expr: &mut ClassExpr, hi: u32) -> Result<(), String> {
        let term = expr.terms.last_mut().expect("a term");
        let lo = match term.pop() {
            Some(CItem::Char(c)) => c,
            _ => return Err(E_CC_END_OF_RANGE.into()),
        };
        if lo > hi {
            return Err(E_EMPTY_RANGE.into());
        }
        term.push(CItem::Range(lo, hi));
        Ok(())
    }

    /// One class element at `self.i` (not `]`, `-`, `&&`).
    fn class_item(&mut self) -> Result<CItem, String> {
        let b = self.peek().unwrap_or(0);
        if b == b'[' {
            if self.f.posix_bracket && self.peek_at(1) == Some(b':') {
                if let Some(item) = self.posix_bracket()? {
                    return Ok(item);
                }
            }
            if self.f.class_set {
                self.i += 1;
                return Ok(CItem::Nested(self.parse_class()?));
            }
            self.i += 1;
            return Ok(CItem::Char(u32::from(b'[')));
        }
        if b != b'\\' {
            let c = self.next_char().unwrap_or(0);
            return Ok(CItem::Char(c));
        }
        self.i += 1;
        let Some((c, n)) = self.peek_char() else {
            return Err(E_PREMATURE_CC.into());
        };
        self.i += n;
        if c >= 0x80 {
            return Ok(CItem::Char(c));
        }
        let e = c as u8;
        let f = self.f;
        Ok(match e {
            b'w' if f.esc_w && self.mb_ctype() => CItem::Text(String::from("\\w\\x{80}-\\x{10ffff}")),
            b'W' if f.esc_w && self.mb_ctype() => CItem::Nested(ClassExpr {
                neg: true,
                terms: vec![vec![CItem::Text(String::from("\\w\\x{80}-\\x{10ffff}"))]],
            }),
            b'w' | b'W' if f.esc_w => CItem::Text(format!("\\{}", e as char)),
            b'd' | b'D' | b's' | b'S' if f.esc_d => CItem::Text(format!("\\{}", e as char)),
            b'h' if f.esc_h => CItem::Text(String::from("0-9a-fA-F")),
            b'H' if f.esc_h => CItem::Nested(ClassExpr {
                neg: true,
                terms: vec![vec![CItem::Text(String::from("0-9a-fA-F"))]],
            }),
            b'p' | b'P' if f.prop && self.peek() == Some(b'{') => {
                let (frag, neg) = self.property(e == b'P')?;
                if neg {
                    CItem::Nested(ClassExpr { neg: true, terms: vec![vec![CItem::Text(frag)]] })
                } else {
                    CItem::Text(frag)
                }
            }
            b'x' => {
                if f.esc_brace_code && self.peek() == Some(b'{') {
                    self.i += 1;
                    let mut v: u64 = 0;
                    let mut digits = 0;
                    while let Some(d) = self.peek().and_then(|d| (d as char).to_digit(16)) {
                        v = v * 16 + u64::from(d);
                        digits += 1;
                        self.i += 1;
                        if digits > 8 {
                            return Err(String::from("too long wide-char value"));
                        }
                    }
                    if self.peek() != Some(b'}') {
                        return Err(E_INVALID_CODE_POINT.into());
                    }
                    self.i += 1;
                    if digits == 0 {
                        return Ok(CItem::Never);
                    }
                    match self.code_to_char(v as u32)? {
                        Some(c) => CItem::Char(c),
                        None => CItem::Never,
                    }
                } else {
                    let mut v: u32 = 0;
                    for _ in 0..2 {
                        match self.peek().and_then(|d| (d as char).to_digit(16)) {
                            Some(d) => {
                                v = v * 16 + d;
                                self.i += 1;
                            }
                            None => break,
                        }
                    }
                    self.class_raw(v as u8)?
                }
            }
            b'u' if f.esc_u => {
                let mut v = 0u32;
                for _ in 0..4 {
                    match self.peek().and_then(|d| (d as char).to_digit(16)) {
                        Some(d) => {
                            v = v * 16 + d;
                            self.i += 1;
                        }
                        None => return Err(E_INVALID_CODE_POINT.into()),
                    }
                }
                match self.code_to_char(v)? {
                    Some(c) => CItem::Char(c),
                    None => CItem::Never,
                }
            }
            b'0'..=b'7' => {
                self.i -= 1;
                let v = self.octal(3);
                self.class_raw(v)?
            }
            b't' => CItem::Char(0x09),
            b'n' => CItem::Char(0x0A),
            b'r' => CItem::Char(0x0D),
            b'f' => CItem::Char(0x0C),
            b'a' => CItem::Char(0x07),
            b'e' => CItem::Char(0x1B),
            b'b' => CItem::Char(0x08),
            b'v' if f.esc_v => CItem::Char(0x0B),
            b'c' => {
                let v = self.control()?;
                self.class_raw(v)?
            }
            b'C' if f.ruby_meta => {
                if self.eof() {
                    return Err(E_END_AT_CONTROL.into());
                }
                if self.peek() != Some(b'-') {
                    return Err(E_CONTROL_SYNTAX.into());
                }
                self.i += 1;
                let v = self.control()?;
                self.class_raw(v)?
            }
            b'M' if f.ruby_meta => CItem::Char(u32::from(self.meta()?)),
            _ => CItem::Char(c),
        })
    }

    fn class_raw(&mut self, first: u8) -> Result<CItem, String> {
        Ok(match self.raw_char(first)? {
            Some(c) => CItem::Char(c),
            None => CItem::Never,
        })
    }

    /// `[:name:]` at `[` (Oniguruma's `parse_posix_bracket`): `Ok(None)`
    /// when it is not one (the `[` then opens a nested class).
    fn posix_bracket(&mut self) -> Result<Option<CItem>, String> {
        let mut j = self.i + 2;
        let mut neg = false;
        if self.p.get(j) == Some(&b'^') {
            neg = true;
            j += 1;
        }
        for name in POSIX_NAMES {
            let nb = name.as_bytes();
            if self.p[j..].starts_with(nb) && self.p[j + nb.len()..].starts_with(b":]") {
                self.i = j + nb.len() + 2;
                if self.mb_ctype() && matches!(*name, "word" | "graph" | "print") {
                    let t = CItem::Text(format!("[:{name}:]\\x{{80}}-\\x{{10ffff}}"));
                    return Ok(Some(if neg { CItem::Nested(ClassExpr { neg: true, terms: vec![vec![t]] }) } else { t }));
                }
                if *name == "xdigit" {
                    let t = CItem::Text(String::from("0-9A-Fa-f"));
                    return Ok(Some(if neg { CItem::Nested(ClassExpr { neg: true, terms: vec![vec![t]] }) } else { t }));
                }
                return Ok(Some(CItem::Text(format!("[:{}{name}:]", if neg { "^" } else { "" }))));
            }
        }
        // Not a known name: an error when it is shaped like one.
        let mut k = j;
        let mut n = 0;
        while k < self.p.len() && self.p[k] != b':' && self.p[k] != b']' {
            n += 1;
            if n > 20 {
                break;
            }
            k += 1;
        }
        if k < self.p.len() && self.p[k] == b':' && self.p.get(k + 1) == Some(&b']') {
            return Err(E_POSIX_BRACKET.into());
        }
        Ok(None)
    }
}

/// A class element as class-body text, when it is not nested.
fn simple_item(item: &CItem, out: &mut Vec<u8>) -> bool {
    match item {
        CItem::Char(c) => push_literal(out, *c),
        CItem::Range(a, b) => {
            push_literal(out, *a);
            out.push(b'-');
            push_literal(out, *b);
        }
        CItem::Text(t) => out.extend_from_slice(t.as_bytes()),
        CItem::Never => {}
        CItem::Nested(_) => return false,
    }
    true
}

/// One `&&` term: a union.
fn render_term(items: &[CItem]) -> String {
    let mut simple = Vec::new();
    let mut nested = Vec::new();
    for it in items {
        if let CItem::Nested(n) = it {
            nested.push(render_class(n));
        } else {
            simple_item(it, &mut simple);
        }
    }
    let simple = if simple.is_empty() { None } else { Some(format!("[{}]", String::from_utf8_lossy(&simple))) };
    match (simple, nested.is_empty()) {
        (Some(s), true) => s,
        (None, true) => String::from("(?!)"),
        (s, false) => {
            let mut alts: Vec<String> = s.into_iter().collect();
            alts.extend(nested);
            format!("(?:{})", alts.join("|"))
        }
    }
}

/// A class as a PCRE2 fragment that matches exactly one character.
fn render_class(c: &ClassExpr) -> String {
    let simple_only = c.terms.len() == 1 && c.terms[0].iter().all(|i| !matches!(i, CItem::Nested(_)));
    if simple_only {
        let mut body = Vec::new();
        for it in &c.terms[0] {
            simple_item(it, &mut body);
        }
        return match (c.neg, body.is_empty()) {
            (false, true) => String::from("(?!)"),
            (true, true) => String::from("(?s:.)"),
            (false, false) => format!("[{}]", String::from_utf8_lossy(&body)),
            (true, false) => format!("[^{}]", String::from_utf8_lossy(&body)),
        };
    }
    let terms: Vec<String> = c.terms.iter().map(|t| render_term(t)).collect();
    let positive = if terms.len() == 1 {
        terms[0].clone()
    } else {
        let (last, init) = terms.split_last().expect("terms");
        let mut s = String::from("(?:");
        for t in init {
            s.push_str(&format!("(?={t})"));
        }
        s.push_str(last);
        s.push(')');
        s
    };
    if c.neg {
        format!("(?:(?!{positive})(?s:.))")
    } else {
        positive
    }
}

// ---- compiled patterns and matching -----------------------------------------------------

/// A compiled pattern (`php_mb_regex_t`).
struct Compiled {
    code: Code,
    ncaps: usize,
    /// Names in Oniguruma's `onig_foreach_name` order, as bytes in the
    /// regex encoding, with their group numbers.
    names: Vec<(Vec<u8>, Vec<usize>)>,
    /// `onig_noname_group_capture_is_active`.
    noname_capture: bool,
    not_empty: bool,
}

/// `onig_region`: per group, the byte span in the subject (caller's
/// encoding) or `None`.
type Region = Vec<Option<(usize, usize)>>;

/// Oniguruma's `strend_hash`.
fn st_hash(s: &[u8]) -> u32 {
    let mut val: u32 = 0;
    for &b in s {
        val = val.wrapping_mul(997).wrapping_add(u32::from(b));
    }
    val.wrapping_add(val >> 5)
}

/// The old `st.c` `new_size`.
fn st_new_size(size: usize) -> usize {
    const PRIMES: [usize; 29] = [
        8 + 3,
        16 + 3,
        32 + 5,
        64 + 3,
        128 + 3,
        256 + 27,
        512 + 9,
        1024 + 9,
        2048 + 5,
        4096 + 3,
        8192 + 27,
        16384 + 43,
        32768 + 3,
        65536 + 45,
        131072 + 29,
        262144 + 3,
        524288 + 21,
        1048576 + 7,
        2097152 + 17,
        4194304 + 15,
        8388608 + 9,
        16777216 + 43,
        33554432 + 35,
        67108864 + 15,
        134217728 + 29,
        268435456 + 3,
        536870912 + 11,
        1073741824 + 85,
        0,
    ];
    let mut newsize = 8;
    for p in PRIMES {
        if newsize > size {
            return p;
        }
        newsize <<= 1;
    }
    usize::MAX
}

/// The order `onig_foreach_name` visits names inserted in `defs` order:
/// an `st` table with chained bins (new entries at the head), 11 bins to
/// start with, rehashed past a density of 5.
fn name_order(defs: &[Vec<u8>]) -> Vec<usize> {
    let mut bins: Vec<Vec<usize>> = vec![Vec::new(); st_new_size(5)];
    for (idx, name) in defs.iter().enumerate() {
        // `idx` = the entries so far.
        if idx / bins.len() > 5 {
            let new_len = st_new_size(bins.len() + 1);
            let mut nb: Vec<Vec<usize>> = vec![Vec::new(); new_len];
            for bin in &bins {
                // `bin` is head-first; re-inserting at heads walks it in
                // that order.
                for &e in bin {
                    let h = st_hash(&defs[e]) as usize % new_len;
                    nb[h].insert(0, e);
                }
            }
            bins = nb;
        }
        let h = st_hash(name) as usize % bins.len();
        bins[h].insert(0, idx);
    }
    bins.into_iter().flatten().collect()
}

/// A subject converted for PCRE2: UTF-8 text and the offset maps.
struct Subject<'a> {
    orig: &'a [u8],
    utf8: std::borrow::Cow<'a, [u8]>,
    /// UTF-8 offset → caller offset (character starts and the end);
    /// `None` for UTF-8 subjects.
    u2o: Option<Vec<usize>>,
    /// Caller offset → UTF-8 offset (a position inside a character maps
    /// to the next character).
    o2u: Option<Vec<usize>>,
}

/// The byte length of the character at `s[i..]` in `enc` (for the regex
/// encodings: table-driven multibyte, UTF-16, UCS-4, single bytes).
fn enc_char_len(enc: &Encoding, s: &[u8], i: usize) -> usize {
    let b = s[i];
    if let Some(t) = enc.mblen_table {
        return usize::from(t[usize::from(b)]).max(1);
    }
    match enc.id {
        Id::Utf16 | Id::Utf16Be => {
            if (0xD8..=0xDB).contains(&b) {
                4
            } else {
                2
            }
        }
        Id::Utf16Le => {
            if s.get(i + 1).is_some_and(|&h| (0xD8..=0xDB).contains(&h)) {
                4
            } else {
                2
            }
        }
        _ => enc.fixed_width().unwrap_or(1),
    }
}

impl<'a> Subject<'a> {
    fn new(orig: &'a [u8], enc: RegexEnc) -> Subject<'a> {
        if enc.is_utf8() {
            return Subject { orig, utf8: std::borrow::Cow::Borrowed(orig), u2o: None, o2u: None };
        }
        let mut utf8 = Vec::with_capacity(orig.len() * 2);
        let mut u2o = Vec::with_capacity(orig.len() * 2 + 1);
        let mut o2u = vec![0usize; orig.len() + 1];
        let mut i = 0;
        while i < orig.len() {
            let n = enc_char_len(enc.mbfl, orig, i).min(orig.len() - i);
            let w = enc.mbfl.decode(&orig[i..i + n]);
            let c = match w.as_slice() {
                [c] if *c != BAD_INPUT => *c,
                [] => 0xFEFF,
                _ => 0xFFFD,
            };
            let at = utf8.len();
            for k in 0..n {
                o2u[i + k] = at;
            }
            push_utf8(&mut utf8, c);
            while u2o.len() < utf8.len() {
                u2o.push(i);
            }
            i += n;
        }
        // Positions inside a character map to the next one.
        let mut i = orig.len();
        o2u[i] = utf8.len();
        let mut next = utf8.len();
        while i > 0 {
            i -= 1;
            let start = u2o.get(o2u[i]).copied() == Some(i);
            if start {
                next = o2u[i];
            } else {
                o2u[i] = next;
            }
        }
        u2o.push(orig.len());
        Subject { orig, utf8: std::borrow::Cow::Owned(utf8), u2o: Some(u2o), o2u: Some(o2u) }
    }

    fn to_utf8(&self, o: usize) -> usize {
        match &self.o2u {
            None => o,
            Some(m) => m[o.min(m.len() - 1)],
        }
    }

    fn to_orig(&self, u: usize) -> usize {
        match &self.u2o {
            None => u,
            Some(m) => m[u.min(m.len() - 1)],
        }
    }
}

/// A failed search (Oniguruma's `ONIGERR_*` texts).
fn search_error(code: i32) -> &'static str {
    if code == rphp_pcre2::err::MATCHLIMIT {
        "retry-limit-in-match over"
    } else if code == rphp_pcre2::err::DEPTHLIMIT || code == rphp_pcre2::err::NOMEMORY || code == -63 {
        "match-stack limit over"
    } else {
        "internal parser error (bug)"
    }
}

/// The match limits from `mbstring.regex_retry_limit` /
/// `mbstring.regex_stack_limit`.
fn limits(ctx: &Ctx) -> (Option<u32>, Option<u32>) {
    let get = |name: &str| -> Option<u32> {
        let v: i64 = ctx.ini_get(name).unwrap_or("0").trim().parse().unwrap_or(0);
        u32::try_from(v).ok().filter(|&v| v > 0)
    };
    (get("mbstring.regex_retry_limit"), get("mbstring.regex_stack_limit"))
}

impl Compiled {
    /// `onig_search` from caller offset `start` (or `onig_match` there
    /// when `anchored`): `Ok(None)` for no match, `Err` for an engine
    /// failure.
    fn search(&self, subj: &Subject, start: usize, anchored: bool, lim: (Option<u32>, Option<u32>)) -> Result<Option<Region>, i32> {
        if start > subj.orig.len() {
            return Ok(None);
        }
        let ustart = subj.to_utf8(start);
        let mut mctx = MatchContext::new();
        if let Some(l) = lim.0 {
            mctx.set_match_limit(l);
        }
        if let Some(l) = lim.1 {
            mctx.set_depth_limit(l);
        }
        let mut md = MatchData::for_code(&self.code);
        let mut o = 0;
        if anchored {
            o |= rphp_pcre2::mopt::ANCHORED;
        }
        if self.not_empty {
            o |= PCRE2_NOTEMPTY;
        }
        // A start inside a UTF-8 character (php advances one byte past an
        // empty match): Oniguruma steps through the continuation bytes as
        // one-byte characters, where only an empty match can happen.
        if subj.u2o.is_none() && !anchored && (0x80..=0xBF).contains(&subj.utf8.get(ustart).copied().unwrap_or(0)) {
            let mut at = ustart;
            while (0x80..=0xBF).contains(&subj.utf8.get(at).copied().unwrap_or(0)) {
                // PCRE2 cannot start inside a character: stand the stray
                // byte in with U+FFFE and ask for an empty match before it.
                let skip = subj.utf8[at..].iter().take_while(|b| (0x80..=0xBF).contains(*b)).count();
                let mut buf = "\u{FFFE}".as_bytes().to_vec();
                buf.extend_from_slice(&subj.utf8[at + skip..]);
                if let MatchResult::Match(_) = self.code.exec(&mut md, &buf, 0, o | rphp_pcre2::mopt::ANCHORED, &mctx) {
                    let ov = md.ovector();
                    if ov[0] == 0 && ov[1] == 0 {
                        let mut region = vec![None; self.ncaps + 1];
                        region[0] = Some((at, at));
                        return Ok(Some(region));
                    }
                }
                at += 1;
            }
        }
        match self.code.exec(&mut md, &subj.utf8, ustart, o, &mctx) {
            MatchResult::NoMatch => Ok(None),
            MatchResult::Error(e) => Err(e),
            MatchResult::Match(_) => {
                let ov = md.ovector();
                let mut region = Vec::with_capacity(self.ncaps + 1);
                for g in 0..=self.ncaps {
                    let (s, e) = (ov.get(2 * g).copied().unwrap_or(UNSET), ov.get(2 * g + 1).copied().unwrap_or(UNSET));
                    if s == UNSET || e == UNSET {
                        region.push(None);
                    } else {
                        region.push(Some((subj.to_orig(s), subj.to_orig(e))));
                    }
                }
                Ok(Some(region))
            }
        }
    }

    /// `onig_name_to_backref_number`: of a name's groups, the last one
    /// that participated, else the last.
    fn backref_number(&self, nums: &[usize], region: &Region) -> usize {
        nums.iter().rev().find(|&&n| region.get(n).is_some_and(|r| r.is_some())).copied().unwrap_or(*nums.last().unwrap_or(&0))
    }
}

/// Translate and compile `pattern` (in the regex encoding) — Oniguruma's
/// `onig_new` — through the request cache. `Ok(None)` after php's warning.
fn compile(ctx: &mut Ctx, func: &str, pattern: &[u8], opt: u32, syn: Syntax) -> Result<Option<Rc<Compiled>>, Unwind> {
    let enc = regex_enc(ctx);
    if !enc.mbfl.check(pattern) {
        ctx.warn(&format!("{func}(): Pattern is not valid under {} encoding", enc.name()))?;
        return Ok(None);
    }
    let key = (pattern.to_vec(), opt, syn, enc.idx);
    if let Some(c) = state(ctx).cache.get(&key) {
        return Ok(Some(c.clone()));
    }
    match build(pattern, opt, syn, enc) {
        Ok(c) => {
            let c = Rc::new(c);
            let cache = &mut state(ctx).cache;
            if cache.len() > 4096 {
                cache.clear();
            }
            cache.insert(key, c.clone());
            Ok(Some(c))
        }
        Err(msg) => {
            ctx.warn(&format!("{func}(): mbregex compile err: {msg}"))?;
            Ok(None)
        }
    }
}

fn build(pattern: &[u8], opt: u32, syn: Syntax, enc: RegexEnc) -> Result<Compiled, String> {
    if opt & OPT_FIND_LONGEST != 0 {
        return Err(String::from("the find-longest option (l) is not supported by rphp"));
    }
    let utf8: Vec<u8> = if enc.is_utf8() {
        pattern.to_vec()
    } else {
        let w = enc.mbfl.decode(pattern);
        let mut out = Vec::with_capacity(pattern.len() * 2);
        for c in w {
            push_utf8(&mut out, c);
        }
        out
    };
    let t = Translator::new(&utf8, opt, syn, enc).translate()?;
    let ucp = if enc_mb_ctype(enc.idx) { 0 } else { rphp_pcre2::opt::UCP };
    let flags = rphp_pcre2::opt::UTF | ucp | PCRE2_MATCH_INVALID_UTF;
    let code = match Code::compile(&t.pattern, flags, 0) {
        Ok(c) => c,
        Err(e) => {
            let m = e.message.to_ascii_lowercase();
            return Err(if m.contains("lookbehind") {
                String::from(E_LOOKBEHIND)
            } else if m.contains("too large") || m.contains("too big") {
                String::from(E_TOO_BIG_REPEAT)
            } else if m.contains("property") {
                String::from("invalid character property name")
            } else {
                e.message
            });
        }
    };
    // Names back in the regex encoding, in Oniguruma's iteration order.
    let names_enc: Vec<Vec<u8>> = t
        .names
        .iter()
        .map(|(n, _)| {
            if enc.is_utf8() {
                n.clone()
            } else {
                let w: Vec<u32> = std::str::from_utf8(n).map(|s| s.chars().map(|c| c as u32).collect()).unwrap_or_default();
                let mut buf = ConvertBuf::default_subst();
                enc.mbfl.encode(&w, &mut buf, true);
                buf.out
            }
        })
        .collect();
    let order = name_order(&names_enc);
    let names = order.into_iter().map(|i| (names_enc[i].clone(), t.names[i].1.clone())).collect::<Vec<_>>();
    let noname_capture = names.is_empty() || syn != Syntax::Ruby;
    Ok(Compiled { code, ncaps: t.ncaps, names, noname_capture, not_empty: opt & OPT_FIND_NOT_EMPTY != 0 })
}


// ---- building match arrays ------------------------------------------------------------------

/// `mb_regex_groups_iter`: each name under its key, the text of the
/// group `onig_name_to_backref_number` picks (non-empty), else `false`.
fn add_names(out: &mut Array, re: &Compiled, subject: &[u8], region: &Region) {
    for (name, nums) in &re.names {
        let gn = re.backref_number(nums, region);
        let v = match region.get(gn).copied().flatten() {
            Some((b, e)) if b < e && e <= subject.len() => str_value(&subject[b..e]),
            _ => Value::Bool(false),
        };
        out.set(ArrayKey::str(name), v);
    }
}

/// The match array `mb_ereg`/`mb_ereg_search_regs` build: numbered groups
/// (`allow_empty` = the search functions' `beg <= end`), then names.
fn regs_array(re: &Compiled, subject: &[u8], region: &Region, allow_empty: bool) -> Array {
    let mut out = Array::new();
    for (i, g) in region.iter().enumerate() {
        let v = match *g {
            Some((b, e)) if (b < e || (allow_empty && b == e)) && e <= subject.len() => str_value(&subject[b..e]),
            _ => Value::Bool(false),
        };
        out.set(ArrayKey::Int(i as i64), v);
    }
    if !re.names.is_empty() {
        add_names(&mut out, re, subject, region);
    }
    out
}

// ---- mb_regex_encoding / mb_regex_set_options ------------------------------------------------

/// `mb_regex_encoding(?string $encoding = null): string|bool`.
fn mb_regex_encoding(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "mb_regex_encoding";
    match opt_str_arg(args, 0, func, "encoding")? {
        None => Ok(str_value(regex_enc(ctx).name().as_bytes())),
        Some(name) => {
            let Some(idx) = enc_index(&name) else {
                return Err(Unwind::value_error(format!(
                    "{func}(): Argument #1 ($encoding) must be a valid encoding, \"{}\" given",
                    String::from_utf8_lossy(&name)
                )));
            };
            let mbfl = mbfl::name2encoding(&name).or_else(|| mbfl::name2encoding(ENC_MAP[idx][0].as_bytes())).unwrap_or(&mbfl::UTF8);
            state(ctx).enc = Some(RegexEnc { idx, mbfl });
            Ok(Value::Bool(true))
        }
    }
}

/// `mb_regex_set_options(?string $options = null): string` — the previous
/// setting when changing it.
fn mb_regex_set_options(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = opt_str_arg(args, 0, "mb_regex_set_options", "options")?;
    let st = state(ctx);
    let (opt, syn) = (st.options, st.syntax);
    if let Some(s) = s {
        let (o, y) = parse_options(&s).map_err(|(c, _, _)| unsupported_option(c))?;
        st.options = o;
        st.syntax = y;
    }
    Ok(str_value(option_string(opt, syn).as_bytes()))
}

// ---- mb_ereg / mb_eregi -------------------------------------------------------------------------

fn ereg_common(ctx: &mut Ctx, args: &mut [Value], icase: bool) -> NativeResult {
    let func = if icase { "mb_eregi" } else { "mb_ereg" };
    let pattern = str_arg(args, 0, func, "pattern")?;
    let string = str_arg(args, 1, func, "string")?;
    if pattern.is_empty() {
        return Err(Unwind::value_error(format!("{func}(): Argument #1 ($pattern) must not be empty")));
    }
    let want = args.len() > 2;
    if want {
        args[2] = Value::Array(Array::new());
    }
    let enc = regex_enc(ctx);
    if !enc.mbfl.check(&string) {
        return Ok(Value::Bool(false));
    }
    let st = state(ctx);
    let opt = st.options | if icase { OPT_IGNORECASE } else { 0 };
    let syn = st.syntax;
    let Some(re) = compile(ctx, func, &pattern, opt, syn)? else {
        return Ok(Value::Bool(false));
    };
    let subj = Subject::new(&string, enc);
    let Ok(Some(region)) = re.search(&subj, 0, false, limits(ctx)) else {
        return Ok(Value::Bool(false));
    };
    if want {
        args[2] = Value::Array(regs_array(&re, &string, &region, false));
    }
    Ok(Value::Bool(true))
}

/// `mb_ereg(string $pattern, string $string, &$matches = null): bool`.
fn mb_ereg(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ereg_common(ctx, args, false)
}

/// `mb_eregi(…)`: `mb_ereg` with `i`.
fn mb_eregi(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ereg_common(ctx, args, true)
}

// ---- replacement ----------------------------------------------------------------------------------

/// `php_mb_mbchar_bytes`.
fn mbchar_bytes(enc: &Encoding, s: &[u8], i: usize) -> usize {
    let b = s.get(i).copied().unwrap_or(0);
    if let Some(t) = enc.mblen_table {
        return usize::from(t[usize::from(b)]).max(1);
    }
    match enc.flags & (mbfl::FLAG_WCS2 | mbfl::FLAG_WCS4) {
        mbfl::FLAG_WCS2 => 2,
        mbfl::FLAG_WCS4 => 4,
        _ => 1,
    }
}

/// Append `s[from..to]` as php's `smart_str_appendl(sp, p - sp)` does,
/// where `to` may run one past the end (the string's NUL terminator).
fn append_span(out: &mut Vec<u8>, s: &[u8], from: usize, to: usize) {
    if to <= s.len() {
        out.extend_from_slice(&s[from..to]);
    } else {
        out.extend_from_slice(&s[from.min(s.len())..]);
        out.extend(std::iter::repeat_n(0u8, to - s.len().max(from)));
    }
}

/// `mb_regex_substitute`: expand `\0`–`\9`, `\k<name>` / `\k'name'` in
/// `replace` (scanned in the regex encoding) for one match.
fn substitute(out: &mut Vec<u8>, subject: &[u8], replace: &[u8], re: &Compiled, region: &Region, enc: &Encoding) {
    let eos = replace.len();
    let clen = |i: usize| if i >= eos { 1 } else { mbchar_bytes(enc, replace, i) };
    let at = |i: usize| replace.get(i).copied().unwrap_or(0);
    let mut p = 0;
    while p < eos {
        let cl = clen(p);
        if cl != 1 || at(p) != b'\\' {
            append_span(out, replace, p, (p + cl).min(eos));
            p += cl;
            continue;
        }
        let sp = p;
        p += 1;
        if clen(p) != 1 || p == eos {
            out.push(b'\\');
            continue;
        }
        let mut no: i64 = -1;
        match at(p) {
            b'0' => {
                no = 0;
                p += 1;
            }
            d @ b'1'..=b'9' => {
                if !re.noname_capture {
                    p += 1;
                    append_span(out, replace, sp, p);
                    continue;
                }
                no = i64::from(d - b'0');
                p += 1;
            }
            b'k' => {
                p += 1;
                let cl = clen(p);
                if cl != 1 || p == eos || (at(p) != b'<' && at(p) != b'\'') {
                    p += cl;
                    append_span(out, replace, sp, p);
                    continue;
                }
                let delim = if at(p) == b'<' { b'>' } else { b'\'' };
                let name = p + 1;
                let mut ne = name;
                let mut maybe_num = true;
                while ne < eos {
                    let cl = clen(ne);
                    if cl != 1 {
                        ne += cl;
                        maybe_num = false;
                        continue;
                    }
                    if at(ne) == delim {
                        break;
                    }
                    if maybe_num && !at(ne).is_ascii_digit() {
                        maybe_num = false;
                    }
                    ne += 1;
                }
                p = ne + 1;
                if ne <= name || ne >= eos {
                    append_span(out, replace, sp, p.min(eos + 1));
                    continue;
                }
                if maybe_num {
                    if !re.noname_capture {
                        append_span(out, replace, sp, p);
                        continue;
                    }
                    if ne - name == 1 {
                        no = i64::from(at(name) - b'0');
                    } else if at(name) != b'0' {
                        no = std::str::from_utf8(&replace[name..ne]).ok().and_then(|s| s.parse().ok()).unwrap_or(i64::MAX);
                    }
                } else {
                    let key = &replace[name..ne];
                    no = match re.names.iter().find(|(n, _)| n.as_slice() == key) {
                        Some((_, nums)) => re.backref_number(nums, region) as i64,
                        None => -1,
                    };
                }
            }
            _ => {
                append_span(out, replace, sp, p);
                continue;
            }
        }
        if no < 0 || no as usize >= region.len() {
            append_span(out, replace, sp, p);
            continue;
        }
        if let Some((b, e)) = region[no as usize] {
            if b < e && e <= subject.len() {
                out.extend_from_slice(&subject[b..e]);
            }
        }
    }
}

/// php's reason text for an invalid callback.
fn callable_problem(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("function \"{}\" not found or invalid function name", String::from_utf8_lossy(s.as_bytes())),
        Value::Array(a) if a.len() != 2 => "array callback must have exactly two members".to_string(),
        Value::Array(_) => "first array member is not a valid class name or object".to_string(),
        _ => "no array or string given".to_string(),
    }
}

/// `_php_mb_regex_ereg_replace_exec`.
fn replace_common(ctx: &mut Ctx, args: &mut [Value], func: &'static str, base: u32, callback: bool) -> NativeResult {
    let pattern = str_arg(args, 0, func, "pattern")?;
    let mut replace = Vec::new();
    let mut cb = Value::Null;
    if callback {
        cb = args.get(1).cloned().unwrap_or(Value::Null);
        if !ctx.is_callable(&cb) {
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #2 ($callback) must be a valid callback, {}",
                callable_problem(&cb)
            )));
        }
    } else {
        replace = str_arg(args, 1, func, "replacement")?;
    }
    let string = str_arg(args, 2, func, "string")?;
    let option_str = opt_str_arg(args, 3, func, "options")?;
    let enc = regex_enc(ctx);
    if !enc.mbfl.check(&string) {
        return Ok(Value::Null);
    }
    let (opt, syn) = match option_str {
        Some(s) => {
            let (o, y) = parse_options(&s).map_err(|(c, _, _)| unsupported_option(c))?;
            (base | o, y)
        }
        None => {
            let st = state(ctx);
            (base | st.options, st.syntax)
        }
    };
    let Some(re) = compile(ctx, func, &pattern, opt, syn)? else {
        return Ok(Value::Bool(false));
    };
    let lim = limits(ctx);
    let subj = Subject::new(&string, enc);
    let len = string.len();
    let mut out = Vec::with_capacity(len);
    let mut pos = 0usize;
    loop {
        match re.search(&subj, pos, false, lim) {
            Err(code) => {
                ctx.warn(&format!("{func}(): mbregex search failure in php_mbereg_replace_exec(): {}", search_error(code)))?;
                return Ok(Value::Bool(false));
            }
            Ok(None) => {
                if pos < len {
                    out.extend_from_slice(&string[pos..]);
                }
                break;
            }
            Ok(Some(region)) => {
                let (b0, e0) = region[0].unwrap_or((pos, pos));
                if b0 > pos {
                    out.extend_from_slice(&string[pos..b0]);
                }
                if callback {
                    let mut subpats = Array::new();
                    for g in &region {
                        subpats.push(match *g {
                            Some((b, e)) if b <= e && e <= len => str_value(&string[b..e]),
                            _ => str_value(b""),
                        });
                    }
                    if !re.names.is_empty() {
                        add_names(&mut subpats, &re, &string, &region);
                    }
                    let r = ctx.call_value(&cb, &[Value::Array(subpats)])?;
                    out.extend_from_slice(&r.to_php_bytes());
                } else {
                    substitute(&mut out, &string, &replace, &re, &region, enc.mbfl);
                }
                if pos < e0 {
                    pos = e0;
                } else {
                    if pos < len {
                        out.push(string[pos]);
                    }
                    pos += 1;
                }
            }
        }
    }
    Ok(Value::string(&out))
}

/// `mb_ereg_replace(string $pattern, string $replacement, string $string,
/// ?string $options = null): string|false|null`.
fn mb_ereg_replace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    replace_common(ctx, args, "mb_ereg_replace", 0, false)
}

/// `mb_eregi_replace(…)`: always case-insensitive.
fn mb_eregi_replace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    replace_common(ctx, args, "mb_eregi_replace", OPT_IGNORECASE, false)
}

/// `mb_ereg_replace_callback(string $pattern, callable $callback, string
/// $string, ?string $options = null): string|false|null`.
fn mb_ereg_replace_callback(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    replace_common(ctx, args, "mb_ereg_replace_callback", 0, true)
}

// ---- mb_split / mb_ereg_match -------------------------------------------------------------------

/// `mb_split(string $pattern, string $string, int $limit = -1): array|false`.
fn mb_split(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "mb_split";
    let pattern = str_arg(args, 0, func, "pattern")?;
    let string = str_arg(args, 1, func, "string")?;
    let mut count = args.get(2).map(|v| v.to_int()).unwrap_or(-1);
    if count > 0 {
        count -= 1;
    }
    let enc = regex_enc(ctx);
    if !enc.mbfl.check(&string) {
        return Ok(Value::Bool(false));
    }
    let st = state(ctx);
    let (opt, syn) = (st.options, st.syntax);
    let Some(re) = compile(ctx, func, &pattern, opt, syn)? else {
        return Ok(Value::Bool(false));
    };
    let lim = limits(ctx);
    let subj = Subject::new(&string, enc);
    let len = string.len();
    let mut out = Array::new();
    let (mut chunk, mut pos) = (0usize, 0usize);
    let mut err: Option<&'static str> = None;
    while count != 0 && pos < len {
        let region = match re.search(&subj, pos, false, lim) {
            Err(code) => {
                err = Some(search_error(code));
                break;
            }
            Ok(None) => break,
            Ok(Some(r)) => r,
        };
        let (beg, end) = region[0].unwrap_or((pos, pos));
        if pos < end {
            if beg < len && beg >= chunk {
                out.push(str_value(&string[chunk..beg]));
                count -= 1;
            } else {
                err = Some("no support in this configuration");
                break;
            }
            chunk = end;
            pos = end;
        } else {
            pos += 1;
        }
    }
    if let Some(e) = err {
        ctx.warn(&format!("{func}(): mbregex search failure in mbsplit(): {e}"))?;
        return Ok(Value::Bool(false));
    }
    out.push(str_value(if chunk < len { &string[chunk..] } else { b"" }));
    Ok(Value::Array(out))
}

/// `mb_ereg_match(string $pattern, string $string, ?string $options =
/// null): bool` — anchored at the start (`onig_match`).
fn mb_ereg_match(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "mb_ereg_match";
    let pattern = str_arg(args, 0, func, "pattern")?;
    let string = str_arg(args, 1, func, "string")?;
    let (opt, syn) = match opt_str_arg(args, 2, func, "options")? {
        Some(s) => parse_options(&s).map_err(|(c, _, _)| unsupported_option(c))?,
        None => {
            let st = state(ctx);
            (st.options, st.syntax)
        }
    };
    let enc = regex_enc(ctx);
    if !enc.mbfl.check(&string) {
        return Ok(Value::Bool(false));
    }
    let Some(re) = compile(ctx, func, &pattern, opt, syn)? else {
        return Ok(Value::Bool(false));
    };
    let subj = Subject::new(&string, enc);
    Ok(Value::Bool(matches!(re.search(&subj, 0, true, limits(ctx)), Ok(Some(_)))))
}

// ---- the mb_ereg_search_* cursor ------------------------------------------------------------------

/// Options for the search functions: php keeps going after an invalid
/// letter (with the options parsed so far dropped); the `ValueError`
/// surfaces when the function returns.
fn search_options(ctx: &mut Ctx, s: Option<Vec<u8>>) -> (u32, Syntax, Option<Unwind>) {
    match s {
        Some(s) => match parse_options(&s) {
            Ok((o, y)) => (o, y, None),
            Err((c, o, y)) => (o, y, Some(unsupported_option(c))),
        },
        None => {
            let st = state(ctx);
            (st.options, st.syntax, None)
        }
    }
}

/// `_php_mb_regex_ereg_search_exec`: mode 0 bool, 1 position, 2 groups.
fn search_exec(ctx: &mut Ctx, args: &mut [Value], func: &'static str, mode: u8) -> NativeResult {
    let pattern = opt_str_arg(args, 0, func, "pattern")?;
    let options = opt_str_arg(args, 1, func, "options")?;
    let (opt, syn, pending) = search_options(ctx, options);
    let result = search_run(ctx, func, mode, pattern, opt, syn);
    match pending {
        Some(e) => Err(e),
        None => result,
    }
}

fn search_run(ctx: &mut Ctx, func: &'static str, mode: u8, pattern: Option<Vec<u8>>, opt: u32, syn: Syntax) -> NativeResult {
    state(ctx).search_regs = None;
    if let Some(p) = pattern {
        let re = compile(ctx, func, &p, opt, syn)?;
        let failed = re.is_none();
        state(ctx).search_re = re;
        if failed {
            return Ok(Value::Bool(false));
        }
    }
    let st = state(ctx);
    let pos = st.search_pos;
    let Some(re) = st.search_re.clone() else {
        return Err(Unwind::error("No pattern was provided"));
    };
    let Some(string) = st.search_str.clone() else {
        return Err(Unwind::error("No string was provided"));
    };
    let enc = regex_enc(ctx);
    let subj = Subject::new(&string, enc);
    let len = string.len();
    match re.search(&subj, pos, false, limits(ctx)) {
        Ok(None) => {
            state(ctx).search_pos = len;
            Ok(Value::Bool(false))
        }
        Err(code) => {
            ctx.warn(&format!("{func}(): mbregex search failure in mbregex_search(): {}", search_error(code)))?;
            Ok(Value::Bool(false))
        }
        Ok(Some(region)) => {
            let (beg, end) = region[0].unwrap_or((pos, pos));
            let v = match mode {
                1 => {
                    let mut a = Array::new();
                    a.push(Value::Int(beg as i64));
                    a.push(Value::Int(end as i64 - beg as i64));
                    Value::Array(a)
                }
                2 => Value::Array(regs_array(&re, &string, &region, true)),
                _ => Value::Bool(true),
            };
            let st = state(ctx);
            st.search_pos = if pos <= end { end } else { pos + 1 };
            st.search_regs = Some(region);
            Ok(v)
        }
    }
}

/// `mb_ereg_search(?string $pattern = null, ?string $options = null): bool`.
fn mb_ereg_search(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    search_exec(ctx, args, "mb_ereg_search", 0)
}

/// `mb_ereg_search_pos(…): array|false` — `[offset, length]`.
fn mb_ereg_search_pos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    search_exec(ctx, args, "mb_ereg_search_pos", 1)
}

/// `mb_ereg_search_regs(…): array|false` — the groups.
fn mb_ereg_search_regs(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    search_exec(ctx, args, "mb_ereg_search_regs", 2)
}

/// `mb_ereg_search_init(string $string, ?string $pattern = null, ?string
/// $options = null): bool`.
fn mb_ereg_search_init(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "mb_ereg_search_init";
    let string = str_arg(args, 0, func, "string")?;
    let pattern = opt_str_arg(args, 1, func, "pattern")?;
    let options = opt_str_arg(args, 2, func, "options")?;
    if pattern.as_ref().is_some_and(|p| p.is_empty()) {
        return Err(Unwind::value_error(format!("{func}(): Argument #2 ($pattern) must not be empty")));
    }
    let (opt, syn, pending) = search_options(ctx, options);
    let result = search_init_run(ctx, func, string, pattern, opt, syn);
    match pending {
        Some(e) => Err(e),
        None => result,
    }
}

fn search_init_run(ctx: &mut Ctx, func: &str, string: Vec<u8>, pattern: Option<Vec<u8>>, opt: u32, syn: Syntax) -> NativeResult {
    if let Some(p) = pattern {
        let re = compile(ctx, func, &p, opt, syn)?;
        let failed = re.is_none();
        state(ctx).search_re = re;
        if failed {
            return Ok(Value::Bool(false));
        }
    }
    let enc = regex_enc(ctx);
    let ok = enc.mbfl.check(&string);
    let st = state(ctx);
    st.search_pos = if ok { 0 } else { string.len() };
    st.search_str = Some(string);
    st.search_regs = None;
    Ok(Value::Bool(ok))
}

/// `mb_ereg_search_getregs(): array|false` — the last match's groups.
fn mb_ereg_search_getregs(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let st = state(ctx);
    match (&st.search_regs, &st.search_str, &st.search_re) {
        (Some(region), Some(s), Some(re)) => Ok(Value::Array(regs_array(re, s, region, true))),
        _ => Ok(Value::Bool(false)),
    }
}

/// `mb_ereg_search_getpos(): int`.
fn mb_ereg_search_getpos(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(ctx).search_pos as i64))
}

/// `mb_ereg_search_setpos(int $offset): bool`.
fn mb_ereg_search_setpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut position = args.first().map(|v| v.to_int()).unwrap_or(0);
    let st = state(ctx);
    let len = st.search_str.as_ref().map(|s| s.len() as i64);
    if position < 0 {
        if let Some(l) = len {
            position += l;
        }
    }
    if position < 0 || len.is_some_and(|l| position > l) {
        return Err(Unwind::value_error("mb_ereg_search_setpos(): Argument #1 ($offset) is out of range"));
    }
    st.search_pos = position as usize;
    Ok(Value::Bool(true))
}

// ---- mb_convert_kana ------------------------------------------------------------------------------

const HAN2ZEN_ALL: u32 = 0x1;
const HAN2ZEN_ALPHA: u32 = 0x2;
const HAN2ZEN_NUMERIC: u32 = 0x4;
const HAN2ZEN_SPACE: u32 = 0x8;
const ZEN2HAN_ALL: u32 = 0x10;
const ZEN2HAN_ALPHA: u32 = 0x20;
const ZEN2HAN_NUMERIC: u32 = 0x40;
const ZEN2HAN_SPACE: u32 = 0x80;
const HAN2ZEN_KATAKANA: u32 = 0x100;
const HAN2ZEN_HIRAGANA: u32 = 0x200;
const HAN2ZEN_GLUE: u32 = 0x800;
const ZEN2HAN_KATAKANA: u32 = 0x1000;
const ZEN2HAN_HIRAGANA: u32 = 0x2000;
const HIRA2KATA: u32 = 0x10000;
const KATA2HIRA: u32 = 0x20000;
const HAN2ZEN_COMPAT1: u32 = 0x100000;
const ZEN2HAN_COMPAT1: u32 = 0x200000;

/// Halfwidth katakana U+FF60+n → fullwidth katakana U+3000+x.
static HANKANA2ZENKANA: [u8; 64] = [
    0x00, 0x02, 0x0C, 0x0D, 0x01, 0xFB, 0xF2, 0xA1, 0xA3, 0xA5, 0xA7, 0xA9, 0xE3, 0xE5, 0xE7, 0xC3, 0xFC, 0xA2, 0xA4,
    0xA6, 0xA8, 0xAA, 0xAB, 0xAD, 0xAF, 0xB1, 0xB3, 0xB5, 0xB7, 0xB9, 0xBB, 0xBD, 0xBF, 0xC1, 0xC4, 0xC6, 0xC8, 0xCA,
    0xCB, 0xCC, 0xCD, 0xCE, 0xCF, 0xD2, 0xD5, 0xD8, 0xDB, 0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE4, 0xE6, 0xE8, 0xE9, 0xEA,
    0xEB, 0xEC, 0xED, 0xEF, 0xF3, 0x9B, 0x9C,
];

/// Halfwidth katakana U+FF60+n → hiragana U+3000+x.
static HANKANA2ZENHIRA: [u8; 64] = [
    0x00, 0x02, 0x0C, 0x0D, 0x01, 0xFB, 0x92, 0x41, 0x43, 0x45, 0x47, 0x49, 0x83, 0x85, 0x87, 0x63, 0xFC, 0x42, 0x44,
    0x46, 0x48, 0x4A, 0x4B, 0x4D, 0x4F, 0x51, 0x53, 0x55, 0x57, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x64, 0x66, 0x68, 0x6A,
    0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x72, 0x75, 0x78, 0x7B, 0x7E, 0x7F, 0x80, 0x81, 0x82, 0x84, 0x86, 0x88, 0x89, 0x8A,
    0x8B, 0x8C, 0x8D, 0x8F, 0x93, 0x9B, 0x9C,
];

/// Fullwidth katakana U+30A1+n (and hiragana U+3041+n) → one or two
/// halfwidth katakana U+FF00+x (a voiced mark second).
static ZENKANA2HANKANA: [[u8; 2]; 84] = [
    [0x67, 0x00], [0x71, 0x00], [0x68, 0x00], [0x72, 0x00], [0x69, 0x00], [0x73, 0x00], [0x6A, 0x00], [0x74, 0x00],
    [0x6B, 0x00], [0x75, 0x00], [0x76, 0x00], [0x76, 0x9E], [0x77, 0x00], [0x77, 0x9E], [0x78, 0x00], [0x78, 0x9E],
    [0x79, 0x00], [0x79, 0x9E], [0x7A, 0x00], [0x7A, 0x9E], [0x7B, 0x00], [0x7B, 0x9E], [0x7C, 0x00], [0x7C, 0x9E],
    [0x7D, 0x00], [0x7D, 0x9E], [0x7E, 0x00], [0x7E, 0x9E], [0x7F, 0x00], [0x7F, 0x9E], [0x80, 0x00], [0x80, 0x9E],
    [0x81, 0x00], [0x81, 0x9E], [0x6F, 0x00], [0x82, 0x00], [0x82, 0x9E], [0x83, 0x00], [0x83, 0x9E], [0x84, 0x00],
    [0x84, 0x9E], [0x85, 0x00], [0x86, 0x00], [0x87, 0x00], [0x88, 0x00], [0x89, 0x00], [0x8A, 0x00], [0x8A, 0x9E],
    [0x8A, 0x9F], [0x8B, 0x00], [0x8B, 0x9E], [0x8B, 0x9F], [0x8C, 0x00], [0x8C, 0x9E], [0x8C, 0x9F], [0x8D, 0x00],
    [0x8D, 0x9E], [0x8D, 0x9F], [0x8E, 0x00], [0x8E, 0x9E], [0x8E, 0x9F], [0x8F, 0x00], [0x90, 0x00], [0x91, 0x00],
    [0x92, 0x00], [0x93, 0x00], [0x6C, 0x00], [0x94, 0x00], [0x6D, 0x00], [0x95, 0x00], [0x6E, 0x00], [0x96, 0x00],
    [0x97, 0x00], [0x98, 0x00], [0x99, 0x00], [0x9A, 0x00], [0x9B, 0x00], [0x9C, 0x00], [0x9C, 0x00], [0x72, 0x00],
    [0x74, 0x00], [0x66, 0x00], [0x9D, 0x00], [0x73, 0x9E],
];

/// `mb_convert_kana_codepoint`: one code point (`next` is the one after
/// it, for the voiced-mark glue) → `(result, next consumed, second)`.
fn kana_codepoint(c: u32, next: u32, mode: u32) -> (u32, bool, Option<u32>) {
    if mode & HAN2ZEN_ALL != 0 && (0x21..=0x7D).contains(&c) && c != 0x22 && c != 0x27 && c != 0x5C {
        return (c + 0xFEE0, false, None);
    }
    if mode & HAN2ZEN_ALPHA != 0 && (c as u8 as u32 == c) && (c as u8).is_ascii_alphabetic() {
        return (c + 0xFEE0, false, None);
    }
    if mode & HAN2ZEN_NUMERIC != 0 && (0x30..=0x39).contains(&c) {
        return (c + 0xFEE0, false, None);
    }
    if mode & HAN2ZEN_SPACE != 0 && c == 0x20 {
        return (0x3000, false, None);
    }
    if mode & (HAN2ZEN_KATAKANA | HAN2ZEN_HIRAGANA) != 0 && (0xFF61..=0xFF9F).contains(&c) {
        let n = (c - 0xFF60) as usize;
        let table = if mode & HAN2ZEN_KATAKANA != 0 { &HANKANA2ZENKANA } else { &HANKANA2ZENHIRA };
        if mode & HAN2ZEN_GLUE != 0 && (0xFF61..=0xFF9F).contains(&next) {
            if next == 0xFF9E && ((22..=36).contains(&n) || (42..=46).contains(&n)) {
                return (0x3001 + u32::from(table[n]), true, None);
            }
            if next == 0xFF9E && n == 19 && mode & HAN2ZEN_KATAKANA != 0 {
                return (0x30F4, true, None);
            }
            if next == 0xFF9F && (42..=46).contains(&n) {
                return (0x3002 + u32::from(table[n]), true, None);
            }
        }
        return (0x3000 + u32::from(table[n]), false, None);
    }
    if mode & HAN2ZEN_COMPAT1 != 0 {
        match c {
            0x5C | 0xA5 => return (0xFFE5, false, None),
            0x7E | 0x203E => return (0xFFE3, false, None),
            0x27 => return (0x2019, false, None),
            0x22 => return (0x201D, false, None),
            _ => {}
        }
    }
    if mode & (ZEN2HAN_ALL | ZEN2HAN_ALPHA | ZEN2HAN_NUMERIC | ZEN2HAN_SPACE) != 0 {
        if mode & ZEN2HAN_ALL != 0 && (0xFF01..=0xFF5D).contains(&c) && c != 0xFF02 && c != 0xFF07 && c != 0xFF3C {
            return (c - 0xFEE0, false, None);
        }
        if mode & ZEN2HAN_ALPHA != 0 && ((0xFF21..=0xFF3A).contains(&c) || (0xFF41..=0xFF5A).contains(&c)) {
            return (c - 0xFEE0, false, None);
        }
        if mode & ZEN2HAN_NUMERIC != 0 && (0xFF10..=0xFF19).contains(&c) {
            return (c - 0xFEE0, false, None);
        }
        if mode & ZEN2HAN_SPACE != 0 && c == 0x3000 {
            return (0x20, false, None);
        }
        if mode & ZEN2HAN_ALL != 0 && c == 0x2212 {
            return (0x2D, false, None);
        }
    }
    if mode & (ZEN2HAN_KATAKANA | ZEN2HAN_HIRAGANA) != 0 {
        let half = |n: usize| {
            let [a, b] = ZENKANA2HANKANA[n];
            (0xFF00 + u32::from(a), false, if b != 0 { Some(0xFF00 + u32::from(b)) } else { None })
        };
        if mode & ZEN2HAN_KATAKANA != 0 && (0x30A1..=0x30F4).contains(&c) {
            return half((c - 0x30A1) as usize);
        }
        if mode & ZEN2HAN_HIRAGANA != 0 && (0x3041..=0x3093).contains(&c) {
            return half((c - 0x3041) as usize);
        }
        match c {
            0x3001 => return (0xFF64, false, None),
            0x3002 => return (0xFF61, false, None),
            0x300C => return (0xFF62, false, None),
            0x300D => return (0xFF63, false, None),
            0x309B => return (0xFF9E, false, None),
            0x309C => return (0xFF9F, false, None),
            0x30FC => return (0xFF70, false, None),
            0x30FB => return (0xFF65, false, None),
            _ => {}
        }
    } else if mode & (HIRA2KATA | KATA2HIRA) != 0 {
        if mode & KATA2HIRA != 0 && ((0x30A1..=0x30F3).contains(&c) || c == 0x30FD || c == 0x30FE) {
            return (c - 0x60, false, None);
        }
        if mode & HIRA2KATA != 0 && ((0x3041..=0x3093).contains(&c) || c == 0x309D || c == 0x309E) {
            return (c + 0x60, false, None);
        }
    }
    if mode & ZEN2HAN_COMPAT1 != 0 {
        match c {
            0xFFE5 | 0xFF3C => return (0x5C, false, None),
            0xFFE3 | 0x203E => return (0x7E, false, None),
            0x2018 | 0x2019 => return (0x27, false, None),
            0x201C | 0x201D => return (0x22, false, None),
            _ => {}
        }
    }
    (c, false, None)
}

/// The mode letters (`mb_convert_kana`'s flag parse and conflict checks).
fn kana_mode(mode: &[u8]) -> Result<u32, Unwind> {
    let func = "mb_convert_kana";
    let mut opt = 0;
    for &c in mode {
        opt |= match c {
            b'A' => HAN2ZEN_ALL,
            b'a' => ZEN2HAN_ALL,
            b'R' => HAN2ZEN_ALPHA,
            b'r' => ZEN2HAN_ALPHA,
            b'N' => HAN2ZEN_NUMERIC,
            b'n' => ZEN2HAN_NUMERIC,
            b'S' => HAN2ZEN_SPACE,
            b's' => ZEN2HAN_SPACE,
            b'K' => HAN2ZEN_KATAKANA,
            b'k' => ZEN2HAN_KATAKANA,
            b'H' => HAN2ZEN_HIRAGANA,
            b'h' => ZEN2HAN_HIRAGANA,
            b'V' => HAN2ZEN_GLUE,
            b'C' => HIRA2KATA,
            b'c' => KATA2HIRA,
            b'M' => HAN2ZEN_COMPAT1,
            b'm' => ZEN2HAN_COMPAT1,
            _ => {
                // php formats with `%c`: a NUL ends the message.
                let mut msg = format!("{func}(): Argument #2 ($mode) contains invalid flag: '").into_bytes();
                if c != 0 {
                    msg.push(c);
                    msg.push(b'\'');
                }
                return Err(Unwind::value_error(String::from_utf8_lossy(&msg).into_owned()));
            }
        };
    }
    const CONFLICTS: &[(u32, u32, char, char)] = &[
        (HAN2ZEN_ALL, ZEN2HAN_ALL, 'A', 'a'),
        (HAN2ZEN_ALL, ZEN2HAN_ALPHA, 'A', 'r'),
        (HAN2ZEN_ALL, ZEN2HAN_NUMERIC, 'A', 'n'),
        (HAN2ZEN_ALPHA, ZEN2HAN_ALL, 'R', 'a'),
        (HAN2ZEN_ALPHA, ZEN2HAN_ALPHA, 'R', 'r'),
        (HAN2ZEN_NUMERIC, ZEN2HAN_ALL, 'N', 'a'),
        (HAN2ZEN_NUMERIC, ZEN2HAN_NUMERIC, 'N', 'n'),
        (HAN2ZEN_SPACE, ZEN2HAN_SPACE, 'S', 's'),
        (HAN2ZEN_KATAKANA, ZEN2HAN_KATAKANA, 'K', 'k'),
        (HAN2ZEN_HIRAGANA, ZEN2HAN_HIRAGANA, 'H', 'h'),
        (HAN2ZEN_COMPAT1, ZEN2HAN_COMPAT1, 'M', 'm'),
        (HIRA2KATA, KATA2HIRA, 'C', 'c'),
        (HAN2ZEN_HIRAGANA, HAN2ZEN_KATAKANA, 'H', 'K'),
        (ZEN2HAN_HIRAGANA, HIRA2KATA, 'h', 'C'),
        (ZEN2HAN_KATAKANA, HIRA2KATA, 'k', 'C'),
        (ZEN2HAN_HIRAGANA, KATA2HIRA, 'h', 'c'),
        (ZEN2HAN_KATAKANA, KATA2HIRA, 'k', 'c'),
    ];
    for &(x, y, a, b) in CONFLICTS {
        if opt & x != 0 && opt & y != 0 {
            return Err(Unwind::value_error(format!("{func}(): Argument #2 ($mode) must not combine '{a}' and '{b}' flags")));
        }
    }
    Ok(opt)
}

/// `mb_convert_kana(string $string, string $mode = "KV", ?string
/// $encoding = null): string`.
fn mb_convert_kana(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "mb_convert_kana";
    let string = str_arg(args, 0, func, "string")?;
    let mode = match args.get(1) {
        Some(_) => str_arg(args, 1, func, "mode")?,
        None => b"KV".to_vec(),
    };
    let opt = kana_mode(&mode)?;
    let encname = opt_str_arg(args, 2, func, "encoding")?;
    let enc = crate::mbstring::get_encoding(ctx, encname.as_deref(), func, 3, "encoding")?;
    let (emode, subst) = crate::mbstring::substitute(ctx);
    let w = enc.decode(&string);
    let mut out = Vec::with_capacity(w.len());
    let mut i = 0;
    while i < w.len() {
        let c = w[i];
        if c == BAD_INPUT {
            out.push(c);
            i += 1;
            continue;
        }
        let next = w.get(i + 1).copied().filter(|&n| n != BAD_INPUT).unwrap_or(0);
        let (r, consumed, second) = kana_codepoint(c, next, opt);
        out.push(r);
        if let Some(s) = second {
            out.push(s);
        }
        i += if consumed { 2 } else { 1 };
    }
    let mut buf = ConvertBuf::new(subst, emode);
    enc.encode(&out, &mut buf, true);
    Ok(Value::string(&buf.out))
}

// ---- mb_send_mail ---------------------------------------------------------------------------------

/// `_php_mbstr_parse_mail_headers`: lower-cased header names → values
/// (continuation lines folded in, surrounding space trimmed).
fn parse_mail_headers(s: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut cur: Option<(Vec<u8>, Vec<u8>)> = None;
    for line in s.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if matches!(line.first(), Some(b' ') | Some(b'\t')) {
            if let Some((_, v)) = cur.as_mut() {
                v.push(b' ');
                v.extend_from_slice(trim_ws(line));
            }
            continue;
        }
        if let Some(h) = cur.take() {
            out.push(h);
        }
        if let Some(colon) = line.iter().position(|&b| b == b':') {
            let name = trim_ws(&line[..colon]).to_ascii_lowercase();
            cur = Some((name, trim_ws(&line[colon + 1..]).to_vec()));
        }
    }
    if let Some(h) = cur.take() {
        out.push(h);
    }
    out
}

fn trim_ws(s: &[u8]) -> &[u8] {
    let is = |b: &u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let start = s.iter().position(|b| !is(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !is(b)).map(|e| e + 1).unwrap_or(start);
    &s[start..end.max(start)]
}

fn header<'h>(hs: &'h [(Vec<u8>, Vec<u8>)], name: &[u8]) -> Option<&'h [u8]> {
    hs.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_slice())
}

/// `php_mb_get_encoding_or_pass`.
fn encoding_or_pass(name: &[u8]) -> Option<&'static Encoding> {
    if name == b"pass" {
        return Some(&mbfl::PASS);
    }
    mbfl::name2encoding(name)
}

fn no_nul(v: &[u8], n: usize, name: &str) -> Result<(), Unwind> {
    if v.contains(&0) {
        return Err(Unwind::value_error(format!("mb_send_mail(): Argument #{n} (${name}) must not contain any null bytes")));
    }
    Ok(())
}

/// `php_mail_build_headers_elem`: one `Name: value` line, after php's
/// value checks (folding `CRLF` + space is allowed).
fn header_elem(out: &mut Vec<u8>, key: &[u8], val: &[u8]) -> Result<(), Unwind> {
    let k = String::from_utf8_lossy(key);
    let mut i = 0;
    while i < val.len() {
        match val[i] {
            b'\r' => {
                if val.len() - i >= 3 && val[i + 1] == b'\n' && matches!(val[i + 2], b' ' | b'\t') {
                    i += 3;
                    continue;
                }
                return Err(Unwind::value_error(if val.get(i + 1) == Some(&b'\n') {
                    format!("Header \"{k}\" contains CRLF characters that are used as a line separator and are not allowed in the header")
                } else {
                    format!("Header \"{k}\" contains CR character that is not allowed in the header")
                }));
            }
            b'\n' => return Err(Unwind::value_error(format!("Header \"{k}\" contains LF character that is not allowed in the header"))),
            0 => return Err(Unwind::value_error(format!("Header \"{k}\" contains NULL character that is not allowed in the header"))),
            _ => i += 1,
        }
    }
    out.extend_from_slice(key);
    out.extend_from_slice(b": ");
    out.extend_from_slice(val);
    out.extend_from_slice(b"\r\n");
    Ok(())
}

/// `php_mail_build_headers`: `Name: value` lines from an array, with
/// php's checks (the RFC 2822 single headers take one string; `To` and
/// `Subject` are refused).
fn build_headers(a: &Array) -> Result<Vec<u8>, Unwind> {
    const SINGLE: &[&str] = &["orig-date", "from", "sender", "reply-to", "cc", "bcc", "message-id", "references", "in-reply-to"];
    let mut out = Vec::new();
    for (k, v) in a.iter() {
        let key = match k {
            ArrayKey::Int(i) => return Err(Unwind::type_error(format!("Header name cannot be numeric, {i} given"))),
            ArrayKey::Str(s) => s.as_bytes().to_vec(),
        };
        let kname = String::from_utf8_lossy(&key).into_owned();
        let lower = kname.to_ascii_lowercase();
        let v = v.deref();
        let type_err = |v: &Value| {
            Unwind::type_error(format!("Header \"{kname}\" must be of type array|string, {} given", rphp_runtime::value_name(v)))
        };
        if lower == "to" {
            return Err(Unwind::value_error("The additional headers cannot contain the \"To\" header"));
        }
        if lower == "subject" {
            return Err(Unwind::value_error("The additional headers cannot contain the \"Subject\" header"));
        }
        if let Some(target) = SINGLE.iter().find(|t| **t == lower) {
            match v.as_ref() {
                Value::Str(s) => header_elem(&mut out, &key, s.as_bytes())?,
                Value::Array(_) => {
                    return Err(Unwind::type_error(format!("Header \"{target}\" must be of type string, array given")))
                }
                other => return Err(type_err(other)),
            }
            continue;
        }
        if key.iter().any(|&b| !(33..=126).contains(&b) || b == b':') {
            return Err(Unwind::value_error(format!("Header name \"{kname}\" contains invalid characters")));
        }
        match v.as_ref() {
            Value::Str(s) => header_elem(&mut out, &key, s.as_bytes())?,
            Value::Array(list) => {
                for (_, item) in list.iter() {
                    match item.deref().as_ref() {
                        Value::Str(s) => header_elem(&mut out, &key, s.as_bytes())?,
                        other => {
                            return Err(Unwind::type_error(format!(
                                "Header \"{kname}\" must only contain values of type string, {} found",
                                rphp_runtime::value_name(other)
                            )))
                        }
                    }
                }
            }
            other => return Err(type_err(other)),
        }
    }
    Ok(out)
}

/// `php_mail_detect_multiple_crlf`.
fn multiple_crlf(h: &[u8]) -> bool {
    if h.is_empty() {
        return false;
    }
    if h[0] < 33 || h[0] > 126 || h[0] == b':' {
        return true;
    }
    let at = |i: usize| h.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while i < h.len() {
        match h[i] {
            b'\r' => {
                if matches!(at(i + 1), 0 | b'\r') || (at(i + 1) == b'\n' && matches!(at(i + 2), 0 | b'\n' | b'\r')) {
                    return true;
                }
                i += 2;
            }
            b'\n' => {
                if matches!(at(i + 1), 0 | b'\r' | b'\n') {
                    return true;
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    false
}

/// `mb_send_mail(string $to, string $subject, string $message, array|string
/// $additional_headers = [], ?string $additional_params = null): bool`.
fn mb_send_mail(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "mb_send_mail";
    let to = str_arg(args, 0, func, "to")?;
    no_nul(&to, 1, "to")?;
    let subject = str_arg(args, 1, func, "subject")?;
    no_nul(&subject, 2, "subject")?;
    let message = str_arg(args, 2, func, "message")?;
    no_nul(&message, 3, "message")?;
    let headers: Option<Vec<u8>> = match args.get(3).map(|v| v.deref()) {
        None => None,
        Some(v) => match v.as_ref() {
            Value::Array(a) => Some(build_headers(a)?),
            _ => {
                let h = str_arg(args, 3, func, "additional_headers")?;
                no_nul(&h, 4, "additional_headers")?;
                Some(trim_ws(&h).to_vec())
            }
        },
    };
    let extra = opt_str_arg(args, 4, func, "additional_params")?;
    if let Some(e) = &extra {
        no_nul(e, 5, "additional_params")?;
    }

    let lang = crate::mbstring::language(ctx);
    let mut tran_cs = mbfl::by_id(lang.mail_charset);
    let head_enc = mbfl::by_id(lang.mail_header);
    let mut body_enc = mbfl::by_id(lang.mail_body);
    let parsed = headers.as_deref().map(parse_mail_headers).unwrap_or_default();

    let mut suppress_type = false;
    if let Some(ct) = header(&parsed, b"content-type") {
        if let Some(semi) = ct.iter().position(|&b| b == b';') {
            let rest = &ct[semi + 1..];
            let rest = &rest[rest.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(rest.len())..];
            let mut toks = rest.split(|&b| b == b'=' || b == b' ').filter(|t| !t.is_empty());
            if let Some(param) = toks.next() {
                if param.eq_ignore_ascii_case(b"charset") {
                    let after = &rest[param.len()..];
                    let cs: Option<Vec<u8>> = after
                        .split(|&b| b == b'=' || b == b' ' || b == b'"')
                        .find(|t| !t.is_empty())
                        .map(<[u8]>::to_vec);
                    let found = cs.as_deref().and_then(encoding_or_pass);
                    tran_cs = match (found, cs) {
                        (Some(e), _) => e,
                        (None, Some(cs)) => {
                            ctx.warn(&format!(
                                "{func}(): Unsupported charset \"{}\" - will be regarded as ascii",
                                String::from_utf8_lossy(&cs)
                            ))?;
                            mbfl::name2encoding(b"ASCII").unwrap_or(&mbfl::UTF8)
                        }
                        (None, None) => tran_cs,
                    };
                }
            }
        }
        suppress_type = true;
    }
    let mut suppress_cte = false;
    if let Some(cte) = header(&parsed, b"content-transfer-encoding") {
        match encoding_or_pass(cte) {
            Some(e) if matches!(e.id, Id::Base64 | Id::SevenBit | Id::EightBit) => body_enc = e,
            _ => {
                ctx.warn(&format!(
                    "{func}(): Unsupported transfer encoding \"{}\" - will be regarded as 8bit",
                    String::from_utf8_lossy(cte)
                ))?;
                body_enc = &mbfl::EIGHT_BIT;
            }
        }
        suppress_cte = true;
    }

    // To: trailing space dropped, control characters (bar folding) blanked.
    let mut to_r = to.clone();
    while to_r.last().is_some_and(|b| b.is_ascii_whitespace()) {
        to_r.pop();
    }
    let mut i = 0;
    while i < to_r.len() {
        if to_r[i] < 0x20 || to_r[i] == 0x7F {
            if to_r[i] == b'\r' && to_r.get(i + 1) == Some(&b'\n') && matches!(to_r.get(i + 2), Some(b' ') | Some(b'\t')) {
                i += 2;
                while matches!(to_r.get(i + 1), Some(b' ') | Some(b'\t')) {
                    i += 1;
                }
                i += 1;
                continue;
            }
            to_r[i] = b' ';
        }
        i += 1;
    }

    let mixed = ctx.ini.bool("mail.mixed_lf_and_crlf");
    let line_sep: &[u8] = if mixed { b"\n" } else { b"\r\n" };
    let mut enc = crate::mbstring::internal_encoding(ctx);
    if enc.id == Id::Pass {
        let order = crate::mbstring::current_detect_order(ctx);
        let strict = ctx.ini.bool("mbstring.strict_detection");
        enc = mbfl::detect::guess(&[&subject], &order, strict, false).unwrap_or(&mbfl::UTF8);
    }
    let subject_enc = crate::mbstring::mime_header_encode(
        &subject,
        enc,
        tran_cs,
        head_enc.id == Id::Base64,
        line_sep,
        ("Subject: [PHP-jp nnnnnnnn]".len() + line_sep.len()) as i64,
    );
    let mut msg_enc = crate::mbstring::internal_encoding(ctx);
    if msg_enc.id == Id::Pass {
        let order = crate::mbstring::current_detect_order(ctx);
        let strict = ctx.ini.bool("mbstring.strict_detection");
        msg_enc = mbfl::detect::guess(&[&message], &order, strict, false).unwrap_or(&mbfl::UTF8);
    }
    let (tmp, _) = mbfl::convert(&message, msg_enc, tran_cs, u32::from(b'?'), mbfl::ErrorMode::Char);
    let (body, _) = mbfl::convert(&tmp, &mbfl::EIGHT_BIT, body_enc, u32::from(b'?'), mbfl::ErrorMode::Char);

    let mut hdr: Vec<u8> = Vec::new();
    let mut empty = true;
    if let Some(h) = headers.as_deref().filter(|h| !h.is_empty()) {
        let mut len = h.len();
        if h[len - 1] == b'\n' {
            len -= 1;
        }
        if len > 0 && h[len - 1] == b'\r' {
            len -= 1;
        }
        hdr.extend_from_slice(&h[..len]);
        empty = false;
    }
    if header(&parsed, b"mime-version").is_none() {
        if !empty {
            hdr.extend_from_slice(line_sep);
        }
        hdr.extend_from_slice(b"MIME-Version: 1.0");
        empty = false;
    }
    if !suppress_type {
        if !empty {
            hdr.extend_from_slice(line_sep);
        }
        hdr.extend_from_slice(b"Content-Type: text/plain");
        if let Some(p) = mbfl::preferred_mime_name(tran_cs) {
            hdr.extend_from_slice(b"; charset=");
            hdr.extend_from_slice(p.as_bytes());
        }
        empty = false;
    }
    if !suppress_cte {
        if !empty {
            hdr.extend_from_slice(line_sep);
        }
        hdr.extend_from_slice(b"Content-Transfer-Encoding: ");
        hdr.extend_from_slice(mbfl::preferred_mime_name(body_enc).unwrap_or("7bit").as_bytes());
    }

    let force = ctx.ini_get("mail.force_extra_parameters").unwrap_or("").as_bytes().to_vec();
    let extra = if !force.is_empty() { Some(force) } else { extra };
    let extra = match extra {
        Some(e) => {
            let v = ctx.call_value(&Value::string(b"escapeshellcmd"), &[Value::string(&e)])?;
            Some(v.to_php_bytes())
        }
        None => None,
    };
    php_mail(ctx, func, &to_r, &subject_enc, &body, &hdr, extra.as_deref(), line_sep)
}

/// `php_mail`: pipe the message into `sendmail_path` (plus the extra
/// parameters) through `/bin/sh -c`.
#[allow(clippy::too_many_arguments)]
fn php_mail(
    ctx: &mut Ctx,
    func: &'static str,
    to: &[u8],
    subject: &[u8],
    message: &[u8],
    headers: &[u8],
    extra: Option<&[u8]>,
    sep: &[u8],
) -> NativeResult {
    use std::io::Write;
    if multiple_crlf(headers) {
        ctx.warn(&format!("{func}(): Multiple or malformed newlines found in additional_header"))?;
        return Ok(Value::Bool(false));
    }
    let path = ctx.ini_get("sendmail_path").unwrap_or("").as_bytes().to_vec();
    if path.is_empty() {
        return Ok(Value::Bool(false));
    }
    let mut cmd = path.clone();
    if let Some(e) = extra {
        cmd.push(b' ');
        cmd.extend_from_slice(e);
    }
    let mut hdr = Vec::new();
    if ctx.ini.bool("mail.add_x_header") {
        let file = ctx.current_file();
        let base = std::path::Path::new(&file).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let uid = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&file).map(|m| m.uid()).unwrap_or(0)
        };
        hdr.extend_from_slice(format!("X-PHP-Originating-Script: {uid}:{base}").as_bytes());
        if !headers.is_empty() {
            hdr.extend_from_slice(sep);
        }
    }
    hdr.extend_from_slice(headers);
    let argv = vec![b"/bin/sh".to_vec(), b"-c".to_vec(), cmd.clone()];
    let verdict = match ctx.ext.slots.get(crate::exec::POLICY_SLOT).and_then(|p| p.downcast_ref::<crate::exec::SpawnPolicy>()) {
        Some(policy) => policy(&crate::exec::SpawnRequest { function: func, argv: &argv, cwd: None }),
        None => Ok(()),
    };
    let path_s = String::from_utf8_lossy(&path).into_owned();
    if verdict.is_err() {
        ctx.warn(&format!("{func}(): Could not execute mail delivery program '{path_s}'"))?;
        return Ok(Value::Bool(false));
    }
    let child = {
        use std::os::unix::ffi::OsStrExt;
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(std::ffi::OsStr::from_bytes(&cmd))
            .stdin(std::process::Stdio::piped())
            .spawn()
    };
    let Ok(mut child) = child else {
        ctx.warn(&format!("{func}(): Could not execute mail delivery program '{path_s}'"))?;
        return Ok(Value::Bool(false));
    };
    let mut data = Vec::new();
    data.extend_from_slice(b"To: ");
    data.extend_from_slice(to);
    data.extend_from_slice(sep);
    data.extend_from_slice(b"Subject: ");
    data.extend_from_slice(subject);
    data.extend_from_slice(sep);
    if !hdr.is_empty() {
        data.extend_from_slice(&hdr);
        data.extend_from_slice(sep);
    }
    data.extend_from_slice(sep);
    data.extend_from_slice(message);
    data.extend_from_slice(sep);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&data);
    }
    let status = child.wait();
    crate::filestat::clear_stat_cache(ctx);
    let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
    // EX_OK / EX_TEMPFAIL count as sent.
    Ok(Value::Bool(code == 0 || code == 75))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf8() -> RegexEnc {
        RegexEnc { idx: ENC_UTF8, mbfl: &mbfl::UTF8 }
    }

    fn tr(p: &str, opt: u32) -> Result<String, String> {
        Translator::new(p.as_bytes(), opt, Syntax::Ruby, utf8())
            .translate()
            .map(|t| String::from_utf8(t.pattern).unwrap())
    }

    #[test]
    fn names_iterate_in_oniguruma_hash_order() {
        let defs: Vec<Vec<u8>> = ["z", "a", "m"].iter().map(|s| s.as_bytes().to_vec()).collect();
        assert_eq!(name_order(&defs), vec![1, 2, 0]);
        let defs: Vec<Vec<u8>> = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"].iter().map(|s| s.as_bytes().to_vec()).collect();
        let order = name_order(&defs);
        assert_eq!(&order[..3], &[10, 11, 0]);
    }

    #[test]
    fn ruby_constructs_translate() {
        assert_eq!(tr("a(?i)b|c", 0).unwrap(), "(*LF)a(?i:b|c)");
        assert_eq!(tr("\\h+", 0).unwrap(), "(*LF)[0-9a-fA-F]+");
        assert_eq!(tr("^a$", OPT_SINGLELINE).unwrap(), "(*LF)\\Aa\\Z");
        assert_eq!(tr("a{,2}", 0).unwrap(), "(*LF)a{0,2}");
        assert_eq!(tr("a**", 0).unwrap(), "(*LF)(?:a*)*");
        assert_eq!(tr("(?<n>a)(b)", 0).unwrap(), "(*LF)(a)(?:b)");
    }

    #[test]
    fn oniguruma_errors_are_reported() {
        assert_eq!(tr("(", 0).unwrap_err(), E_UNMATCHED_PAREN);
        assert_eq!(tr("*a", 0).unwrap_err(), E_TARGET_NOT_SPECIFIED);
        assert_eq!(tr("[b-a]", 0).unwrap_err(), E_EMPTY_RANGE);
        assert_eq!(tr("(?<n>a)\\1", 0).unwrap_err(), E_NUMBERED_REF);
        assert_eq!(tr("\\8", 0).unwrap_err(), E_INVALID_BACKREF);
    }

    #[test]
    fn kana_modes() {
        assert!(kana_mode(b"Aa").is_err());
        assert_eq!(kana_codepoint(0xFF76, 0xFF9E, HAN2ZEN_KATAKANA | HAN2ZEN_GLUE), (0x30AC, true, None));
        assert_eq!(kana_codepoint(0x30AC, 0, ZEN2HAN_KATAKANA), (0xFF76, false, Some(0xFF9E)));
    }
}
