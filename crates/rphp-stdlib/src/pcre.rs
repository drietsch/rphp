//! `pcre` extension — `preg_*` over PCRE2 through `rphp-pcre2` (ADR-032: the
//! engine php-src links, so pattern semantics match byte for byte). This
//! module is a transcription of `ext/pcre/php_pcre.c`: the delimited-pattern
//! parser with php's warnings, the modifier table, the per-request compiled
//! pattern cache, the `pcre2_match` loop with Perl's empty-match retry
//! (`NOTEMPTY_ATSTART|ANCHORED`), `populate_subpat_array` (named groups with
//! dual keys, `PREG_OFFSET_CAPTURE`, `PREG_UNMATCHED_AS_NULL`, `(*MARK)`),
//! the replacement-reference grammar (`$1 \1 ${1}`), and the error-code
//! slot behind `preg_last_error()` / `preg_last_error_msg()`.
//!
//! Everything operates on bytes: PHP strings (patterns, subjects,
//! replacements) are byte strings, and PCRE2 validates UTF-8 itself when a
//! pattern carries the `u` modifier.
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use rphp_pcre2::{err, mopt, opt, Code, MatchContext, MatchData, MatchResult, UNSET};
use rphp_value::{Array, ArrayKey, Str, Value};

use rphp_runtime::{Ctx, NativeFn, NativeResult, Registry, Unwind};

/// A [`NativeFn`] row with parameter names (php's arginfo), so named
/// arguments and `Argument #3 ($matches) could not be passed by reference`
/// render as in php.
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

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    row("preg_match", 2, 5, 0b100, &["pattern", "subject", "matches", "flags", "offset"], preg_match),
    row("preg_match_all", 2, 5, 0b100, &["pattern", "subject", "matches", "flags", "offset"], preg_match_all),
    row("preg_replace", 3, 5, 0b10000, &["pattern", "replacement", "subject", "limit", "count"], preg_replace),
    row("preg_filter", 3, 5, 0b10000, &["pattern", "replacement", "subject", "limit", "count"], preg_filter),
    row(
        "preg_replace_callback",
        3,
        6,
        0b10000,
        &["pattern", "callback", "subject", "limit", "count", "flags"],
        preg_replace_callback,
    ),
    row(
        "preg_replace_callback_array",
        2,
        5,
        0b1000,
        &["pattern", "subject", "limit", "count", "flags"],
        preg_replace_callback_array,
    ),
    row("preg_split", 2, 4, 0, &["pattern", "subject", "limit", "flags"], preg_split),
    row("preg_quote", 1, 2, 0, &["str", "delimiter"], preg_quote),
    row("preg_grep", 2, 3, 0, &["pattern", "array", "flags"], preg_grep),
    row("preg_last_error", 0, 0, 0, &[], preg_last_error),
    row("preg_last_error_msg", 0, 0, 0, &[], preg_last_error_msg),
];

// ---- constants --------------------------------------------------------------

const PREG_PATTERN_ORDER: i64 = 1;
const PREG_SET_ORDER: i64 = 2;
const PREG_OFFSET_CAPTURE: i64 = 256;
const PREG_UNMATCHED_AS_NULL: i64 = 512;
const PREG_SPLIT_NO_EMPTY: i64 = 1;
const PREG_SPLIT_DELIM_CAPTURE: i64 = 2;
const PREG_SPLIT_OFFSET_CAPTURE: i64 = 4;
const PREG_GREP_INVERT: i64 = 1;

const PREG_NO_ERROR: i64 = 0;
const PREG_INTERNAL_ERROR: i64 = 1;
const PREG_BACKTRACK_LIMIT_ERROR: i64 = 2;
const PREG_RECURSION_LIMIT_ERROR: i64 = 3;
const PREG_BAD_UTF8_ERROR: i64 = 4;
const PREG_BAD_UTF8_OFFSET_ERROR: i64 = 5;
const PREG_JIT_STACKLIMIT_ERROR: i64 = 6;

/// php's compiled-pattern cache size (`PCRE_CACHE_SIZE`).
const CACHE_SIZE: usize = 4096;
/// php's JIT stack bounds (`PCRE_JIT_STACK_MIN_SIZE` / `_MAX_SIZE`).
const JIT_STACK_MIN: usize = 32 * 1024;
const JIT_STACK_MAX: usize = 192 * 1024;

/// `PREG_*` and `PCRE_*` constants (see `lib.rs`).
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("PREG_PATTERN_ORDER", PREG_PATTERN_ORDER),
        ("PREG_SET_ORDER", PREG_SET_ORDER),
        ("PREG_OFFSET_CAPTURE", PREG_OFFSET_CAPTURE),
        ("PREG_UNMATCHED_AS_NULL", PREG_UNMATCHED_AS_NULL),
        ("PREG_SPLIT_NO_EMPTY", PREG_SPLIT_NO_EMPTY),
        ("PREG_SPLIT_DELIM_CAPTURE", PREG_SPLIT_DELIM_CAPTURE),
        ("PREG_SPLIT_OFFSET_CAPTURE", PREG_SPLIT_OFFSET_CAPTURE),
        ("PREG_GREP_INVERT", PREG_GREP_INVERT),
        ("PREG_NO_ERROR", PREG_NO_ERROR),
        ("PREG_INTERNAL_ERROR", PREG_INTERNAL_ERROR),
        ("PREG_BACKTRACK_LIMIT_ERROR", PREG_BACKTRACK_LIMIT_ERROR),
        ("PREG_RECURSION_LIMIT_ERROR", PREG_RECURSION_LIMIT_ERROR),
        ("PREG_BAD_UTF8_ERROR", PREG_BAD_UTF8_ERROR),
        ("PREG_BAD_UTF8_OFFSET_ERROR", PREG_BAD_UTF8_OFFSET_ERROR),
        ("PREG_JIT_STACKLIMIT_ERROR", PREG_JIT_STACKLIMIT_ERROR),
    ] {
        r.constant(name, Value::Int(v));
    }
    let version = rphp_pcre2::version();
    let (major, minor) = version
        .split_whitespace()
        .next()
        .and_then(|v| v.split_once('.'))
        .map(|(a, b)| (a.parse().unwrap_or(0), b.parse().unwrap_or(0)))
        .unwrap_or((0, 0));
    r.constant("PCRE_VERSION", Value::string(version.as_bytes()));
    r.constant("PCRE_VERSION_MAJOR", Value::Int(major));
    r.constant("PCRE_VERSION_MINOR", Value::Int(minor));
    r.constant("PCRE_JIT_SUPPORT", Value::Bool(rphp_pcre2::jit_available()));
}

// ---- error slot ---------------------------------------------------------------

/// php's `php_pcre_get_error_msg`.
fn error_text(code: i64) -> &'static str {
    match code {
        PREG_NO_ERROR => "No error",
        PREG_INTERNAL_ERROR => "Internal error",
        PREG_BACKTRACK_LIMIT_ERROR => "Backtrack limit exhausted",
        PREG_RECURSION_LIMIT_ERROR => "Recursion limit exhausted",
        PREG_BAD_UTF8_ERROR => "Malformed UTF-8 characters, possibly incorrectly encoded",
        PREG_BAD_UTF8_OFFSET_ERROR => {
            "The offset did not correspond to the beginning of a valid UTF-8 code point"
        }
        PREG_JIT_STACKLIMIT_ERROR => "JIT stack limit exhausted",
        _ => "Unknown error",
    }
}

/// Record `code` as the request's last preg error.
fn set_error(ctx: &mut Ctx, code: i64) {
    ctx.ext.preg_last_error = code;
    ctx.ext.preg_last_error_msg = error_text(code).to_string();
}

/// php's `pcre_handle_exec_error`: map a PCRE2 return code to `PREG_*_ERROR`
/// and record it.
fn handle_exec_error(ctx: &mut Ctx, rc: i32) {
    let code = match rc {
        err::MATCHLIMIT => PREG_BACKTRACK_LIMIT_ERROR,
        err::DEPTHLIMIT => PREG_RECURSION_LIMIT_ERROR,
        err::BADUTFOFFSET => PREG_BAD_UTF8_OFFSET_ERROR,
        err::JIT_STACKLIMIT => PREG_JIT_STACKLIMIT_ERROR,
        c if (err::UTF8_ERR21..=err::UTF8_ERR1).contains(&c) => PREG_BAD_UTF8_ERROR,
        _ => PREG_INTERNAL_ERROR,
    };
    set_error(ctx, code);
}

/// `preg_last_error()`.
pub(crate) fn preg_last_error(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(ctx.ext.preg_last_error))
}

/// `preg_last_error_msg()`.
pub(crate) fn preg_last_error_msg(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(error_text(ctx.ext.preg_last_error).as_bytes()))
}

// ---- argument coercion ----------------------------------------------------------

/// zpp `string` coercion for parameter `n` (`$name`) of `func`: scalars
/// convert, `null` converts with php's 8.1 deprecation, arrays are a
/// `TypeError`, objects go through `__toString`. The result shares the
/// argument's buffer when it already is a string.
fn str_arg(ctx: &mut Ctx, func: &str, n: usize, name: &str, v: &Value) -> Result<Str, Unwind> {
    str_arg_typed(ctx, func, n, name, "string", v)
}

/// [`str_arg`] with the declared type php names in the `TypeError`
/// (`string` or `array|string`).
fn str_arg_typed(ctx: &mut Ctx, func: &str, n: usize, name: &str, ty: &str, v: &Value) -> Result<Str, Unwind> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{func}(): Passing null to parameter #{n} (${name}) of type {ty} is deprecated"
            ))?;
            Ok(Str::new(b""))
        }
        Value::Array(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type {ty}, array given"
        ))),
        Value::Object(_) | Value::Closure(_) => match ctx.to_string(v) {
            Ok(s) => Ok(s),
            Err(_) => Err(Unwind::type_error(format!(
                "{func}(): Argument #{n} (${name}) must be of type {ty}, {} given",
                rphp_runtime::value_name(v)
            ))),
        },
        Value::Ref(r) => {
            let inner = r.get();
            str_arg_typed(ctx, func, n, name, ty, &inner)
        }
        other => Ok(Str::from_vec(other.to_php_bytes())),
    }
}

/// zpp `int` coercion (`$limit`, `$flags`, `$offset`).
fn int_arg(ctx: &mut Ctx, func: &str, n: usize, name: &str, v: &Value) -> Result<i64, Unwind> {
    match v {
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{func}(): Passing null to parameter #{n} (${name}) of type int is deprecated"
            ))?;
            Ok(0)
        }
        Value::Array(_) | Value::Object(_) | Value::Closure(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type int, {} given",
            rphp_runtime::value_name(v)
        ))),
        Value::Str(_) if !v.is_numeric() => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type int, string given"
        ))),
        Value::Ref(r) => {
            let inner = r.get();
            int_arg(ctx, func, n, name, &inner)
        }
        other => Ok(other.to_int()),
    }
}

/// `zval_get_string` for an array element (subjects, patterns, replacements
/// taken out of arrays): arrays warn and become `Array`, objects use
/// `__toString`.
fn elem_string(ctx: &mut Ctx, v: &Value) -> Result<Str, Unwind> {
    ctx.to_string(v)
}

fn str_value(bytes: Vec<u8>) -> Value {
    Value::Str(Str::from_vec(bytes))
}

fn substr_value(subject: &[u8], start: usize, end: usize) -> Value {
    Value::string(&subject[start..end])
}

// ---- pattern parsing --------------------------------------------------------------

/// A parsed PHP pattern: the body between the delimiters and the PCRE2
/// options its modifiers map to.
struct Parsed {
    body: Vec<u8>,
    options: u32,
    extra: u32,
}

/// Why a delimited pattern was rejected (php's warning texts).
#[derive(Debug)]
enum PatternError {
    Empty,
    BadDelimiter,
    NoEnding(u8),
    NoEndingMatching(u8),
    UnknownModifier(u8),
    NulModifier,
}

impl PatternError {
    fn message(&self) -> String {
        match self {
            PatternError::Empty => "Empty regular expression".to_string(),
            PatternError::BadDelimiter => {
                "Delimiter must not be alphanumeric, backslash, or NUL byte".to_string()
            }
            PatternError::NoEnding(d) => format!("No ending delimiter '{}' found", *d as char),
            PatternError::NoEndingMatching(d) => {
                format!("No ending matching delimiter '{}' found", *d as char)
            }
            PatternError::UnknownModifier(m) => {
                format!("Unknown modifier '{}'", String::from_utf8_lossy(&[*m]))
            }
            PatternError::NulModifier => "NUL byte is not a valid modifier".to_string(),
        }
    }
}

/// php's `isspace` under the C locale.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Split `/body/mods` into its body and PCRE2 options, exactly as
/// `pcre_get_compiled_regex_cache_ex`: leading whitespace is skipped, the
/// delimiter must not be alphanumeric, a backslash or NUL, a bracket
/// delimiter (`( [ { <`) closes with its mate and nests, any other closes
/// on its next unescaped occurrence, and trailing letters are modifiers
/// (`i m n s x A D S U X u J r`; space, `\n` and `\r` are ignored).
fn parse_pattern(regex: &[u8]) -> Result<Parsed, PatternError> {
    let mut p = 0;
    while p < regex.len() && is_c_space(regex[p]) {
        p += 1;
    }
    if p >= regex.len() {
        return Err(PatternError::Empty);
    }
    let start_delim = regex[p];
    p += 1;
    if start_delim.is_ascii_alphanumeric() || start_delim == b'\\' || start_delim == 0 {
        return Err(PatternError::BadDelimiter);
    }
    let end_delim = match start_delim {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        d => d,
    };
    let body_start = p;
    let mut pp = p;
    let n = regex.len();
    if start_delim == end_delim {
        while pp < n {
            if regex[pp] == b'\\' && pp + 1 < n {
                pp += 1;
            } else if regex[pp] == end_delim {
                break;
            }
            pp += 1;
        }
    } else {
        let mut brackets = 1i32;
        while pp < n {
            if regex[pp] == b'\\' && pp + 1 < n {
                pp += 1;
            } else if regex[pp] == end_delim {
                brackets -= 1;
                if brackets <= 0 {
                    break;
                }
            } else if regex[pp] == start_delim {
                brackets += 1;
            }
            pp += 1;
        }
    }
    if pp >= n {
        return Err(if start_delim == end_delim {
            PatternError::NoEnding(end_delim)
        } else {
            PatternError::NoEndingMatching(end_delim)
        });
    }
    let body = regex[body_start..pp].to_vec();
    pp += 1;
    let mut options = 0u32;
    let mut extra = 0u32;
    while pp < n {
        let m = regex[pp];
        pp += 1;
        match m {
            b'i' => options |= opt::CASELESS,
            b'm' => options |= opt::MULTILINE,
            b'n' => options |= opt::NO_AUTO_CAPTURE,
            b's' => options |= opt::DOTALL,
            b'x' => options |= opt::EXTENDED,
            b'A' => options |= opt::ANCHORED,
            b'D' => options |= opt::DOLLAR_ENDONLY,
            b'S' | b'X' => {}
            b'U' => options |= opt::UNGREEDY,
            b'u' => options |= opt::UTF | opt::UCP,
            b'J' => options |= opt::DUPNAMES,
            b'r' => extra |= opt::EXTRA_CASELESS_RESTRICT,
            b' ' | b'\n' | b'\r' => {}
            0 => return Err(PatternError::NulModifier),
            other => return Err(PatternError::UnknownModifier(other)),
        }
    }
    Ok(Parsed { body, options, extra })
}

// ---- compiled-pattern cache -------------------------------------------------------

/// A compiled pattern plus what the match loops need to know about it.
struct Compiled {
    code: Code,
    /// The `u` modifier: subjects are UTF-8 checked, units are code points.
    utf: bool,
    /// Group index → name (`None` for unnamed groups), `capture_count + 1`
    /// entries; empty when the pattern has no named groups.
    names: Vec<Option<Vec<u8>>>,
}

impl Compiled {
    /// Group count including group 0 (`num_subpats`).
    fn num_subpats(&self) -> usize {
        self.code.capture_count() as usize + 1
    }

    /// php's `calculate_unit_length`: how far to step past an empty match at
    /// `pos` — one UTF-8 character under `u`, else one byte.
    fn unit_len(&self, subject: &[u8], pos: usize) -> usize {
        if !self.utf {
            return 1;
        }
        let mut end = pos + 1;
        while end < subject.len() && subject[end] & 0xC0 == 0x80 {
            end += 1;
        }
        end - pos
    }
}

/// php's per-request pattern cache (`PCRE_G(pcre_cache)`): full regex string
/// → compiled entry, at most [`CACHE_SIZE`] entries, the oldest evicted first.
#[derive(Default)]
struct PatternCache {
    map: HashMap<Vec<u8>, Rc<Compiled>>,
    order: VecDeque<Vec<u8>>,
}

thread_local! {
    /// One interpreter runs per thread, so a thread-local is the request slot.
    static CACHE: RefCell<PatternCache> = RefCell::new(PatternCache::default());
    /// The request's match context (limits, JIT stack).
    static MCTX: RefCell<MatchContext> = RefCell::new(MatchContext::new());
    /// Subjects a `u` match already validated as UTF-8 (php's
    /// `IS_STR_VALID_UTF8` string flag, which keeps `preg_match(.., $offset)`
    /// loops linear): the handles are kept alive so a pointer cannot be reused.
    static VALID_UTF8: RefCell<Vec<Str>> = const { RefCell::new(Vec::new()) };
}

/// How many validated subjects are remembered.
const VALID_UTF8_MEMO: usize = 8;

/// php's `is_known_valid_utf8`: the subject was validated by an earlier
/// match and `start` does not point into the middle of a character.
fn known_valid_utf8(subject: &Str, start: usize) -> bool {
    if !VALID_UTF8.with(|m| m.borrow().iter().any(|s| s.ptr_eq(subject))) {
        return false;
    }
    let b = subject.as_bytes();
    start >= b.len() || b[start] & 0xC0 != 0x80
}

/// Remember that `subject` is valid UTF-8.
fn remember_valid_utf8(subject: &Str) {
    VALID_UTF8.with(|m| {
        let mut m = m.borrow_mut();
        if m.iter().any(|s| s.ptr_eq(subject)) {
            return;
        }
        if m.len() >= VALID_UTF8_MEMO {
            m.remove(0);
        }
        m.push(subject.clone());
    });
}

/// The compiled form of `regex` (from the cache or freshly compiled),
/// or `None` after php's warning for a malformed / uncompilable pattern.
fn compile(ctx: &mut Ctx, func: &str, regex: &[u8]) -> Result<Option<Rc<Compiled>>, Unwind> {
    if let Some(hit) = CACHE.with(|c| c.borrow().map.get(regex).cloned()) {
        return Ok(Some(hit));
    }
    let parsed = match parse_pattern(regex) {
        Ok(p) => p,
        Err(e) => {
            ctx.warn(&format!("{func}(): {}", e.message()))?;
            set_error(ctx, PREG_INTERNAL_ERROR);
            return Ok(None);
        }
    };
    let mut code = match Code::compile(&parsed.body, parsed.options, parsed.extra) {
        Ok(c) => c,
        Err(e) => {
            ctx.warn(&format!("{func}(): Compilation failed: {} at offset {}", e.message, e.offset))?;
            set_error(ctx, PREG_INTERNAL_ERROR);
            return Ok(None);
        }
    };
    if ctx.ini.bool("pcre.jit") {
        match code.jit_compile() {
            Ok(_) => {}
            Err(err::NOMEMORY) => {
                ctx.warn(&format!(
                    "{func}(): Allocation of JIT memory failed, PCRE JIT will be disabled. \
                     This is likely caused by security restrictions. \
                     Either grant PHP permission to allocate executable memory, or set pcre.jit=0"
                ))?;
                ctx.ini.set("pcre.jit", "0");
            }
            Err(rc) => {
                ctx.warn(&format!("{func}(): JIT compilation failed: {}", rphp_pcre2::error_message(rc)))?;
                set_error(ctx, PREG_INTERNAL_ERROR);
                return Ok(None);
            }
        }
    }
    let mut names = Vec::new();
    if !code.names().is_empty() {
        names = vec![None; code.capture_count() as usize + 1];
        for (idx, name) in code.names() {
            if let Some(slot) = names.get_mut(*idx as usize) {
                *slot = Some(name.clone());
            }
        }
    }
    let utf = parsed.options & opt::UTF != 0;
    let entry = Rc::new(Compiled { code, utf, names });
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.map.len() >= CACHE_SIZE {
            if let Some(old) = c.order.pop_front() {
                c.map.remove(&old);
            }
        }
        c.order.push_back(regex.to_vec());
        c.map.insert(regex.to_vec(), entry.clone());
    });
    Ok(Some(entry))
}

/// Apply the `pcre.*` ini limits (and a JIT stack when the pattern is
/// JIT-compiled) to the request's match context before a match loop.
fn configure_match_context(ctx: &Ctx, re: &Compiled) {
    let backtrack = ctx.ini.int("pcre.backtrack_limit") as u32;
    let recursion = ctx.ini.int("pcre.recursion_limit") as u32;
    MCTX.with(|m| {
        let mut m = m.borrow_mut();
        m.set_match_limit(backtrack);
        m.set_depth_limit(recursion);
        if re.code.jit() {
            m.ensure_jit_stack(JIT_STACK_MIN, JIT_STACK_MAX);
        }
    });
}

// ---- the match loop -----------------------------------------------------------------

/// One subject's global-match cursor: php's `pcre2_match` loop including
/// Perl's `/g` treatment of empty matches (retry at the same offset with
/// `NOTEMPTY_ATSTART|ANCHORED`, then step one unit). After a successful
/// [`Matcher::next`] the ovector is in `md`; call [`Matcher::advance`]
/// before the next call.
struct Matcher<'a> {
    re: &'a Compiled,
    subject_str: &'a Str,
    subject: &'a [u8],
    md: MatchData,
    start: usize,
    /// Options for the next ordinary match: UTF-8 is checked once, on the
    /// first call, and skipped afterwards (`PCRE2_NO_UTF_CHECK`).
    options: u32,
    /// The previous match was empty at `start`.
    retry_empty: bool,
}

impl<'a> Matcher<'a> {
    fn new(re: &'a Compiled, subject_str: &'a Str, start: usize) -> Matcher<'a> {
        let check = re.utf && !known_valid_utf8(subject_str, start);
        Matcher {
            re,
            subject_str,
            subject: subject_str.as_bytes(),
            md: MatchData::for_code(&re.code),
            start,
            options: if check { 0 } else { mopt::NO_UTF_CHECK },
            retry_empty: false,
        }
    }

    /// After the loop: a `u` match that ended without error has validated
    /// the whole subject.
    fn remember_valid_utf8(&self, no_error: bool) {
        if self.re.utf && no_error {
            remember_valid_utf8(self.subject_str);
        }
    }

    fn exec(&mut self, options: u32) -> MatchResult {
        MCTX.with(|m| self.re.code.exec(&mut self.md, self.subject, self.start, options, &m.borrow()))
    }

    /// The next match: `Ok(Some(count))` (with `count` meaningful ovector
    /// pairs), `Ok(None)` when there is none, `Err(rc)` on a PCRE2 error.
    fn next(&mut self) -> Result<Option<usize>, i32> {
        if self.retry_empty {
            self.retry_empty = false;
            match self.exec(mopt::NO_UTF_CHECK | mopt::NOTEMPTY_ATSTART | mopt::ANCHORED) {
                MatchResult::Match(c) => return Ok(Some(c)),
                MatchResult::NoMatch => {
                    if self.start < self.subject.len() {
                        self.start += self.re.unit_len(self.subject, self.start);
                    } else {
                        return Ok(None);
                    }
                }
                MatchResult::Error(rc) => return Err(rc),
            }
        }
        match self.exec(self.options) {
            MatchResult::Match(c) => {
                self.options = mopt::NO_UTF_CHECK;
                Ok(Some(c))
            }
            MatchResult::NoMatch => Ok(None),
            MatchResult::Error(rc) => Err(rc),
        }
    }

    /// Move past the match in `md`.
    fn advance(&mut self) {
        let ov = self.md.ovector();
        let (s, e) = (ov[0], ov[1]);
        self.start = e;
        self.retry_empty = s == e;
    }
}

// ---- populating $matches -------------------------------------------------------------

/// php's `add_offset_pair`: `[text, offset]`, with an unset group as
/// `["", -1]` (or `[null, -1]` under `PREG_UNMATCHED_AS_NULL`).
fn offset_pair(subject: &[u8], start: usize, end: usize, unmatched_as_null: bool) -> Value {
    let mut pair = Array::new();
    if start == UNSET {
        pair.push(if unmatched_as_null { Value::Null } else { Value::string(b"") });
        pair.push(Value::Int(-1));
    } else {
        pair.push(substr_value(subject, start, end));
        pair.push(Value::Int(start as i64));
    }
    Value::Array(pair)
}

/// php's `populate_match_value`: the group's text, `""`/`null` when unset.
fn match_value(subject: &[u8], start: usize, end: usize, unmatched_as_null: bool) -> Value {
    if start == UNSET {
        if unmatched_as_null {
            Value::Null
        } else {
            Value::string(b"")
        }
    } else {
        substr_value(subject, start, end)
    }
}

/// Insert a group value under its name (if any) and its index. php's
/// `add_named`: with `(?J)` duplicate names the group that matched wins —
/// an unmatched group only claims the name when nothing has yet.
fn add_group(out: &mut Array, name: Option<&Vec<u8>>, v: Value, unmatched: bool) {
    if let Some(n) = name {
        let key = ArrayKey::str(n);
        if !unmatched || !out.contains_key(&key) {
            out.set(key, v.clone());
        }
    }
    out.push(v);
}

/// php's `populate_subpat_array`: one match's groups (`count` participating,
/// padded to `num_subpats` with nulls under `PREG_UNMATCHED_AS_NULL`), named
/// groups under both their name and index, plus `MARK`.
#[allow(clippy::too_many_arguments)]
fn populate_subpats(
    out: &mut Array,
    re: &Compiled,
    subject: &[u8],
    ov: &[usize],
    count: usize,
    mark: Option<&[u8]>,
    offset_capture: bool,
    unmatched_as_null: bool,
) {
    let num = re.num_subpats();
    let name = |i: usize| re.names.get(i).and_then(|n| n.as_ref());
    for i in 0..count {
        let v = if offset_capture {
            offset_pair(subject, ov[2 * i], ov[2 * i + 1], unmatched_as_null)
        } else {
            match_value(subject, ov[2 * i], ov[2 * i + 1], unmatched_as_null)
        };
        add_group(out, name(i), v, ov[2 * i] == UNSET);
    }
    if unmatched_as_null {
        for i in count..num {
            let v = if offset_capture { offset_pair(subject, UNSET, UNSET, true) } else { Value::Null };
            add_group(out, name(i), v, true);
        }
    }
    if let Some(m) = mark {
        out.set(ArrayKey::str(b"MARK"), Value::string(m));
    }
}

// ---- preg_match / preg_match_all --------------------------------------------------------

/// Resolve `$offset` against the subject (negative counts from the end); php
/// reports an offset past the end as `PCRE2_ERROR_BADOFFSET`.
fn resolve_offset(offset: i64, len: usize) -> Option<usize> {
    let start = if offset < 0 {
        let back = offset.unsigned_abs() as usize;
        len.saturating_sub(back)
    } else {
        offset as usize
    };
    (start <= len).then_some(start)
}

/// The shared body of `preg_match` / `preg_match_all` (`php_pcre_match_impl`).
fn match_impl(ctx: &mut Ctx, args: &mut [Value], global: bool) -> NativeResult {
    let func = if global { "preg_match_all" } else { "preg_match" };
    let regex = str_arg(ctx, func, 1, "pattern", &args[0])?;
    let subject_str = str_arg(ctx, func, 2, "subject", &args[1])?;
    let subject = subject_str.as_bytes();
    let flags = match args.get(3) {
        Some(v) => int_arg(ctx, func, 4, "flags", v)?,
        None => 0,
    };
    let offset = match args.get(4) {
        Some(v) => int_arg(ctx, func, 5, "offset", v)?,
        None => 0,
    };
    if offset == i64::MIN {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #5 ($offset) must be greater than {}",
            i64::MIN
        )));
    }
    let Some(re) = compile(ctx, func, regex.as_bytes())? else {
        return Ok(Value::Bool(false));
    };
    // $matches is reset to [] before anything else can fail.
    let want_subpats = args.len() >= 3;
    if want_subpats {
        args[2] = Value::Array(Array::new());
    }
    let mut subpats_order = if global { PREG_PATTERN_ORDER } else { 0 };
    let offset_capture = flags & PREG_OFFSET_CAPTURE != 0;
    let unmatched_as_null = flags & PREG_UNMATCHED_AS_NULL != 0;
    if flags != 0 {
        if flags & 0xff != 0 {
            subpats_order = flags & 0xff;
        }
        if (global && !(PREG_PATTERN_ORDER..=PREG_SET_ORDER).contains(&subpats_order))
            || (!global && subpats_order != 0)
        {
            return Err(Unwind::value_error(format!(
                "{func}(): Argument #4 ($flags) must be a PREG_* constant"
            )));
        }
    }
    let Some(start) = resolve_offset(offset, subject.len()) else {
        handle_exec_error(ctx, err::BADOFFSET);
        return Ok(Value::Bool(false));
    };
    set_error(ctx, PREG_NO_ERROR);
    configure_match_context(ctx, &re);
    let num = re.num_subpats();
    // PATTERN_ORDER accumulates one list per group, plus the marks.
    let mut match_sets: Vec<Array> = if global && subpats_order == PREG_PATTERN_ORDER {
        vec![Array::new(); num]
    } else {
        Vec::new()
    };
    let mut marks: Option<Array> = None;
    let mut result = Array::new();
    let mut matched = 0i64;
    let mut m = Matcher::new(&re, &subject_str, start);
    loop {
        match m.next() {
            Ok(Some(count)) => {
                matched += 1;
                let mark = m.md.mark();
                let ov = m.md.ovector().to_vec();
                if !global {
                    if want_subpats {
                        populate_subpats(&mut result, &re, subject, &ov, count, mark.as_deref(), offset_capture, unmatched_as_null);
                    }
                    break;
                }
                if subpats_order == PREG_PATTERN_ORDER {
                    for i in 0..count {
                        let v = if offset_capture {
                            offset_pair(subject, ov[2 * i], ov[2 * i + 1], unmatched_as_null)
                        } else {
                            match_value(subject, ov[2 * i], ov[2 * i + 1], unmatched_as_null)
                        };
                        match_sets[i].push(v);
                    }
                    // Groups past the last participating one are padded.
                    for set in match_sets.iter_mut().take(num).skip(count) {
                        let v = if offset_capture {
                            offset_pair(subject, UNSET, UNSET, unmatched_as_null)
                        } else if unmatched_as_null {
                            Value::Null
                        } else {
                            Value::string(b"")
                        };
                        set.push(v);
                    }
                    if let Some(mk) = &mark {
                        // Keyed by the match number (`add_index_string`).
                        marks.get_or_insert_with(Array::new).set(ArrayKey::Int(matched - 1), Value::string(mk));
                    }
                } else {
                    let mut set = Array::new();
                    populate_subpats(&mut set, &re, subject, &ov, count, mark.as_deref(), offset_capture, unmatched_as_null);
                    result.push(Value::Array(set));
                }
                m.advance();
            }
            Ok(None) => break,
            Err(rc) => {
                handle_exec_error(ctx, rc);
                break;
            }
        }
    }
    m.remember_valid_utf8(ctx.ext.preg_last_error == PREG_NO_ERROR);
    if global && subpats_order == PREG_PATTERN_ORDER {
        for (i, set) in match_sets.into_iter().enumerate() {
            add_group(&mut result, re.names.get(i).and_then(|n| n.as_ref()), Value::Array(set), false);
        }
        if let Some(mk) = marks {
            result.set(ArrayKey::str(b"MARK"), Value::Array(mk));
        }
    }
    if want_subpats {
        args[2] = Value::Array(result);
    }
    if ctx.ext.preg_last_error == PREG_NO_ERROR {
        Ok(Value::Int(matched))
    } else {
        Ok(Value::Bool(false))
    }
}

/// `preg_match($pattern, $subject, &$matches = null, $flags = 0, $offset = 0)`.
pub(crate) fn preg_match(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match_impl(ctx, args, false)
}

/// `preg_match_all($pattern, $subject, &$matches = null, $flags = 0, $offset = 0)`.
pub(crate) fn preg_match_all(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match_impl(ctx, args, true)
}

// ---- replacement -------------------------------------------------------------------------

/// How one match is turned into replacement text.
enum Replacer<'a> {
    /// A replacement string with `$n`/`\n`/`${n}` references.
    Text(&'a [u8]),
    /// `preg_replace_callback`: the callable and the `$flags` for its
    /// `$matches` argument.
    Callback { callback: &'a Value, flags: i64 },
}

/// php's `preg_get_backref`: parse a `$n`, `${n}` or `\n` reference at the
/// start of `s` (whose first byte is `$` or `\`), reading at most two
/// digits. Returns `(group, bytes consumed)`.
fn parse_backref(s: &[u8]) -> Option<(usize, usize)> {
    if s.len() < 2 {
        return None;
    }
    let mut w = 1;
    let in_brace = s[0] == b'$' && s[1] == b'{';
    if in_brace {
        w += 1;
    }
    let d0 = *s.get(w)?;
    if !d0.is_ascii_digit() {
        return None;
    }
    let mut group = (d0 - b'0') as usize;
    w += 1;
    if let Some(&d1) = s.get(w) {
        if d1.is_ascii_digit() {
            group = group * 10 + (d1 - b'0') as usize;
            w += 1;
        }
    }
    if in_brace {
        if s.get(w) == Some(&b'}') {
            w += 1;
        } else {
            return None;
        }
    }
    Some((group, w))
}

/// Expand a replacement string against one match (`php_pcre_replace_impl`'s
/// copy loop): a `\` or `$` that starts a reference to a participating
/// group (`< count`) is replaced by that group's text; a `\` right before a
/// `\` or `$` escapes it; everything else is copied verbatim.
fn expand_replacement(repl: &[u8], subject: &[u8], ov: &[usize], count: usize, out: &mut Vec<u8>) {
    let mut walk_last = 0u8;
    let mut i = 0;
    while i < repl.len() {
        let c = repl[i];
        if c == b'\\' || c == b'$' {
            if walk_last == b'\\' {
                if let Some(last) = out.last_mut() {
                    *last = c;
                }
                walk_last = 0;
                i += 1;
                continue;
            }
            if let Some((group, consumed)) = parse_backref(&repl[i..]) {
                if group < count && ov[2 * group] != UNSET {
                    out.extend_from_slice(&subject[ov[2 * group]..ov[2 * group + 1]]);
                }
                i += consumed;
                continue;
            }
        }
        out.push(c);
        walk_last = c;
        i += 1;
    }
}

/// `php_pcre_replace_impl` / `php_pcre_replace_func_impl`: replace up to
/// `limit` matches (`None` = unlimited) of `re` in `subject`, bumping
/// `count`. `Ok(None)` when a match error occurred (php returns `null`).
fn replace_impl(
    ctx: &mut Ctx,
    re: &Compiled,
    subject_str: &Str,
    replacer: &Replacer<'_>,
    limit: Option<usize>,
    count: &mut i64,
) -> Result<Option<Str>, Unwind> {
    set_error(ctx, PREG_NO_ERROR);
    configure_match_context(ctx, re);
    let subject = subject_str.as_bytes();
    let mut out = Vec::with_capacity(subject.len());
    let mut last_end = 0usize;
    let mut remaining = limit;
    let mut m = Matcher::new(re, subject_str, 0);
    let mut had_match = false;
    loop {
        if remaining == Some(0) {
            break;
        }
        match m.next() {
            Ok(Some(n)) => {
                had_match = true;
                let ov = m.md.ovector().to_vec();
                let mark = m.md.mark();
                out.extend_from_slice(&subject[last_end..ov[0]]);
                match replacer {
                    Replacer::Text(repl) => expand_replacement(repl, subject, &ov, n, &mut out),
                    Replacer::Callback { callback, flags } => {
                        let mut matches = Array::new();
                        populate_subpats(
                            &mut matches,
                            re,
                            subject,
                            &ov,
                            n,
                            mark.as_deref(),
                            flags & PREG_OFFSET_CAPTURE != 0,
                            flags & PREG_UNMATCHED_AS_NULL != 0,
                        );
                        let r = ctx.call_value(callback, &[Value::Array(matches)])?;
                        let s = ctx.to_string(&r)?;
                        out.extend_from_slice(s.as_bytes());
                    }
                }
                *count += 1;
                if let Some(r) = remaining.as_mut() {
                    *r -= 1;
                }
                last_end = ov[1];
                m.advance();
            }
            Ok(None) => break,
            Err(rc) => {
                handle_exec_error(ctx, rc);
                return Ok(None);
            }
        }
    }
    m.remember_valid_utf8(true);
    if !had_match {
        return Ok(Some(subject_str.clone()));
    }
    out.extend_from_slice(&subject[last_end..]);
    Ok(Some(Str::from_vec(out)))
}

/// The `$limit` argument as php's `size_t`: `-1` (or any negative) is
/// unlimited.
fn limit_arg(limit: i64) -> Option<usize> {
    if limit < 0 { None } else { Some(limit as usize) }
}

/// The pattern / replacement operands of `preg_replace` & co, after php's
/// array-shape checks.
enum Patterns {
    One(Str),
    Many(Vec<Str>),
}

/// php's `php_replace_in_subject`: run every pattern (each with the same
/// `limit`) over one subject, threading the result through. `None` on error.
fn replace_in_subject(
    ctx: &mut Ctx,
    func: &str,
    patterns: &Patterns,
    replacements: Option<&[Str]>,
    replacement: &Replacer<'_>,
    subject: Str,
    limit: Option<usize>,
    count: &mut i64,
) -> Result<Option<Str>, Unwind> {
    match patterns {
        Patterns::One(p) => {
            let Some(re) = compile(ctx, func, p.as_bytes())? else {
                return Ok(None);
            };
            replace_impl(ctx, &re, &subject, replacement, limit, count)
        }
        Patterns::Many(ps) => {
            let mut subject = subject;
            for (i, p) in ps.iter().enumerate() {
                let Some(re) = compile(ctx, func, p.as_bytes())? else {
                    return Ok(None);
                };
                // A replacement array is consumed positionally; once it runs
                // out the replacement is the empty string.
                let text_repl: Option<&[u8]> = replacements.map(|r| r.get(i).map_or(&b""[..], |s| s.as_bytes()));
                let r = match (text_repl, replacement) {
                    (Some(t), _) => Replacer::Text(t),
                    (None, Replacer::Text(t)) => Replacer::Text(t),
                    (None, Replacer::Callback { callback, flags }) => Replacer::Callback { callback, flags: *flags },
                };
                match replace_impl(ctx, &re, &subject, &r, limit, count)? {
                    Some(s) => subject = s,
                    None => return Ok(None),
                }
            }
            Ok(Some(subject))
        }
    }
}

/// `preg_replace_common`: string or array subject, `is_filter` keeps only
/// subjects that had a replacement.
#[allow(clippy::too_many_arguments)]
fn replace_common(
    ctx: &mut Ctx,
    func: &str,
    patterns: &Patterns,
    replacements: Option<&[Str]>,
    replacement: &Replacer<'_>,
    subject: &Value,
    subject_arg: usize,
    limit: Option<usize>,
    is_filter: bool,
    count: &mut i64,
) -> NativeResult {
    if let Value::Array(subjects) = subject {
        let mut out = Array::new();
        for (k, v) in subjects.iter() {
            let before = *count;
            let s = elem_string(ctx, v)?;
            if let Some(r) = replace_in_subject(ctx, func, patterns, replacements, replacement, s, limit, count)? {
                if !is_filter || *count > before {
                    out.set(k.clone(), Value::Str(r));
                }
            }
        }
        return Ok(Value::Array(out));
    }
    let s = str_arg_typed(ctx, func, subject_arg, "subject", "array|string", subject)?;
    match replace_in_subject(ctx, func, patterns, replacements, replacement, s, limit, count)? {
        Some(r) if !is_filter || *count > 0 => Ok(Value::Str(r)),
        _ => Ok(Value::Null),
    }
}

/// Collect the `$pattern` operand: a string, or an array of strings.
fn pattern_operand(ctx: &mut Ctx, func: &str, v: &Value) -> Result<Patterns, Unwind> {
    match v {
        Value::Array(a) => {
            let mut ps = Vec::with_capacity(a.len());
            for (_, p) in a.iter() {
                ps.push(elem_string(ctx, p)?);
            }
            Ok(Patterns::Many(ps))
        }
        other => Ok(Patterns::One(str_arg_typed(ctx, func, 1, "pattern", "array|string", other)?)),
    }
}

/// The shared body of `preg_replace` / `preg_filter`.
fn preg_replace_common(ctx: &mut Ctx, args: &mut [Value], is_filter: bool) -> NativeResult {
    let func = if is_filter { "preg_filter" } else { "preg_replace" };
    let limit = match args.get(3) {
        Some(v) => int_arg(ctx, func, 4, "limit", v)?,
        None => -1,
    };
    let patterns = pattern_operand(ctx, func, &args[0])?;
    let mut replacements: Option<Vec<Str>> = None;
    let replacement_text: Str;
    match &args[1] {
        Value::Array(a) => {
            if matches!(patterns, Patterns::One(_)) {
                return Err(Unwind::type_error(format!(
                    "{func}(): Argument #1 ($pattern) must be of type array when argument #2 ($replacement) is an array, string given"
                )));
            }
            let mut rs = Vec::with_capacity(a.len());
            for (_, r) in a.iter() {
                rs.push(elem_string(ctx, r)?);
            }
            replacements = Some(rs);
            replacement_text = Str::new(b"");
        }
        other => replacement_text = str_arg_typed(ctx, func, 2, "replacement", "array|string", other)?,
    }
    let subject = args[2].clone();
    let mut count = 0i64;
    let result = replace_common(
        ctx,
        func,
        &patterns,
        replacements.as_deref(),
        &Replacer::Text(replacement_text.as_bytes()),
        &subject,
        3,
        limit_arg(limit),
        is_filter,
        &mut count,
    )?;
    if args.len() >= 5 {
        args[4] = Value::Int(count);
    }
    Ok(result)
}

/// `preg_replace($pattern, $replacement, $subject, $limit = -1, &$count = null)`.
pub(crate) fn preg_replace(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    preg_replace_common(ctx, args, false)
}

/// `preg_filter(...)`: like `preg_replace`, returning only the subjects in
/// which a replacement happened (`null` for a string subject without one).
pub(crate) fn preg_filter(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    preg_replace_common(ctx, args, true)
}

/// `preg_replace_callback($pattern, $callback, $subject, $limit = -1,
/// &$count = null, $flags = 0)`: the callback receives the match array
/// (shaped by `$flags`) and returns the replacement text.
pub(crate) fn preg_replace_callback(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "preg_replace_callback";
    if !ctx.is_callable(&args[1]) {
        return Err(Unwind::type_error(format!(
            "{func}(): Argument #2 ($callback) must be a valid callback, {}",
            callable_problem(ctx, &args[1])
        )));
    }
    let limit = match args.get(3) {
        Some(v) => int_arg(ctx, func, 4, "limit", v)?,
        None => -1,
    };
    let flags = match args.get(5) {
        Some(v) => int_arg(ctx, func, 6, "flags", v)?,
        None => 0,
    };
    let patterns = pattern_operand(ctx, func, &args[0])?;
    let callback = args[1].clone();
    let subject = args[2].clone();
    let mut count = 0i64;
    let result = replace_common(
        ctx,
        func,
        &patterns,
        None,
        &Replacer::Callback { callback: &callback, flags },
        &subject,
        3,
        limit_arg(limit),
        false,
        &mut count,
    )?;
    if args.len() >= 5 {
        args[4] = Value::Int(count);
    }
    Ok(result)
}

/// php's reason text for an invalid callback (zpp's `Z_PARAM_FUNC`
/// wording for the common shapes).
fn callable_problem(ctx: &Ctx, v: &Value) -> String {
    match v {
        Value::Str(s) => format!(
            "function \"{}\" not found or invalid function name",
            String::from_utf8_lossy(s.as_bytes())
        ),
        Value::Array(a) if a.len() != 2 => "array callback must have exactly two members".to_string(),
        Value::Array(a) => match (a.get_deref(&ArrayKey::Int(0)), a.get_deref(&ArrayKey::Int(1))) {
            (Some(Value::Object(o)), Some(Value::Str(m))) => format!(
                "class {} does not have a method \"{}\"",
                ctx.class_name_of(&o),
                String::from_utf8_lossy(m.as_bytes())
            ),
            (Some(Value::Str(c)), Some(Value::Str(m))) => {
                if ctx.class_exists(c.as_bytes()) {
                    format!(
                        "class {} does not have a method \"{}\"",
                        String::from_utf8_lossy(c.as_bytes()),
                        String::from_utf8_lossy(m.as_bytes())
                    )
                } else {
                    format!("class \"{}\" not found", String::from_utf8_lossy(c.as_bytes()))
                }
            }
            _ => "first array member is not a valid class name or object".to_string(),
        },
        _ => "no array or string given".to_string(),
    }
}

/// `preg_replace_callback_array($pattern, $subject, $limit = -1, &$count = null,
/// $flags = 0)`: `[pattern => callback, …]` applied in order.
pub(crate) fn preg_replace_callback_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "preg_replace_callback_array";
    let map = match &args[0] {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #1 ($pattern) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    let limit = match args.get(2) {
        Some(v) => int_arg(ctx, func, 3, "limit", v)?,
        None => -1,
    };
    let flags = match args.get(4) {
        Some(v) => int_arg(ctx, func, 5, "flags", v)?,
        None => 0,
    };
    let mut subject = args[1].clone();
    if !matches!(subject, Value::Array(_)) {
        subject = Value::Str(str_arg_typed(ctx, func, 2, "subject", "array|string", &subject)?);
    }
    let mut count = 0i64;
    let mut result = subject;
    for (k, cb) in map.iter() {
        let cb = cb.deref().into_owned();
        let regex = match k {
            ArrayKey::Str(s) => Str::new(s),
            ArrayKey::Int(_) => {
                return Err(Unwind::type_error(format!(
                    "{func}(): Argument #1 ($pattern) must contain only string patterns as keys"
                )))
            }
        };
        if !ctx.is_callable(&cb) {
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #1 ($pattern) must contain only valid callbacks"
            )));
        }
        let patterns = Patterns::One(regex);
        let r = replace_common(
            ctx,
            func,
            &patterns,
            None,
            &Replacer::Callback { callback: &cb, flags },
            &result,
            2,
            limit_arg(limit),
            false,
            &mut count,
        )?;
        if matches!(r, Value::Null) {
            if args.len() >= 4 {
                args[3] = Value::Int(count);
            }
            return Ok(Value::Null);
        }
        result = r;
    }
    if args.len() >= 4 {
        args[3] = Value::Int(count);
    }
    Ok(result)
}

// ---- preg_split ----------------------------------------------------------------------------

/// `preg_split($pattern, $subject, $limit = -1, $flags = 0)`
/// (`php_pcre_split_impl`): `$limit` 0/-1 is unlimited, 1 (or any other
/// negative value) yields the whole subject; `PREG_SPLIT_NO_EMPTY` drops
/// empty pieces (they do not count against the limit), `DELIM_CAPTURE` adds
/// the participating groups after each piece, `OFFSET_CAPTURE` wraps pieces
/// as `[text, offset]`.
pub(crate) fn preg_split(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "preg_split";
    let regex = str_arg(ctx, func, 1, "pattern", &args[0])?;
    let subject_str = str_arg(ctx, func, 2, "subject", &args[1])?;
    let subject = subject_str.as_bytes();
    let mut limit = match args.get(2) {
        Some(v) => int_arg(ctx, func, 3, "limit", v)?,
        None => -1,
    };
    let flags = match args.get(3) {
        Some(v) => int_arg(ctx, func, 4, "flags", v)?,
        None => 0,
    };
    let Some(re) = compile(ctx, func, regex.as_bytes())? else {
        return Ok(Value::Bool(false));
    };
    set_error(ctx, PREG_NO_ERROR);
    configure_match_context(ctx, &re);
    let no_empty = flags & PREG_SPLIT_NO_EMPTY != 0;
    let delim_capture = flags & PREG_SPLIT_DELIM_CAPTURE != 0;
    let offset_capture = flags & PREG_SPLIT_OFFSET_CAPTURE != 0;
    if limit == 0 {
        limit = -1;
    }
    let piece = |start: usize, end: usize| {
        if offset_capture {
            offset_pair(subject, start, end, false)
        } else {
            match_value(subject, start, end, false)
        }
    };
    let mut out = Array::new();
    let mut last_match = 0usize;
    let mut m = Matcher::new(&re, &subject_str, 0);
    while limit == -1 || limit > 1 {
        match m.next() {
            Ok(Some(count)) => {
                let ov = m.md.ovector().to_vec();
                if !no_empty || ov[0] != last_match {
                    out.push(piece(last_match, ov[0]));
                    if limit != -1 {
                        limit -= 1;
                    }
                }
                if delim_capture {
                    for i in 1..count {
                        if !no_empty || ov[2 * i] != ov[2 * i + 1] {
                            out.push(piece(ov[2 * i], ov[2 * i + 1]));
                        }
                    }
                }
                last_match = ov[1];
                m.advance();
            }
            Ok(None) => break,
            Err(rc) => {
                handle_exec_error(ctx, rc);
                break;
            }
        }
    }
    m.remember_valid_utf8(ctx.ext.preg_last_error == PREG_NO_ERROR);
    if ctx.ext.preg_last_error != PREG_NO_ERROR {
        return Ok(Value::Bool(false));
    }
    if !no_empty || last_match < subject.len() {
        out.push(piece(last_match, subject.len()));
    }
    Ok(Value::Array(out))
}

// ---- preg_quote / preg_grep --------------------------------------------------------------------

/// `preg_quote($str, $delimiter = null)`: backslash-escape every PCRE
/// metacharacter (`. \ + * ? [ ^ ] $ ( ) { } = ! < > | : - #`) and the
/// delimiter's first byte; a NUL byte becomes `\000`.
pub(crate) fn preg_quote(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(ctx, "preg_quote", 1, "str", &args[0])?;
    let delim = match args.get(1) {
        Some(Value::Null) | None => None,
        Some(v) => str_arg(ctx, "preg_quote", 2, "delimiter", v)?.as_bytes().first().copied().filter(|&d| d != 0),
    };
    let mut out = Vec::with_capacity(s.len() + 8);
    for &b in s.as_bytes() {
        match b {
            b'.' | b'\\' | b'+' | b'*' | b'?' | b'[' | b'^' | b']' | b'$' | b'(' | b')' | b'{' | b'}'
            | b'=' | b'!' | b'>' | b'<' | b'|' | b':' | b'-' | b'#' => {
                out.push(b'\\');
                out.push(b);
            }
            0 => out.extend_from_slice(b"\\000"),
            _ if Some(b) == delim => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    Ok(str_value(out))
}

/// `preg_grep($pattern, $array, $flags = 0)`: the entries whose string form
/// matches (or, with `PREG_GREP_INVERT`, does not), keys preserved; `false`
/// on a pattern or match error.
pub(crate) fn preg_grep(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "preg_grep";
    let regex = str_arg(ctx, func, 1, "pattern", &args[0])?;
    let arr = match &args[1] {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #2 ($array) must be of type array, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    let flags = match args.get(2) {
        Some(v) => int_arg(ctx, func, 3, "flags", v)?,
        None => 0,
    };
    let Some(re) = compile(ctx, func, regex.as_bytes())? else {
        return Ok(Value::Bool(false));
    };
    set_error(ctx, PREG_NO_ERROR);
    configure_match_context(ctx, &re);
    let invert = flags & PREG_GREP_INVERT != 0;
    let mut out = Array::new();
    let mut md = MatchData::for_code(&re.code);
    let options = if re.utf { 0 } else { mopt::NO_UTF_CHECK };
    for (k, v) in arr.iter() {
        let hay = elem_string(ctx, v)?;
        let r = MCTX.with(|m| re.code.exec(&mut md, hay.as_bytes(), 0, options, &m.borrow()));
        match r {
            MatchResult::Match(_) => {
                if !invert {
                    out.set(k.clone(), v.clone());
                }
            }
            MatchResult::NoMatch => {
                if invert {
                    out.set(k.clone(), v.clone());
                }
            }
            MatchResult::Error(rc) => {
                handle_exec_error(ctx, rc);
                break;
            }
        }
    }
    if ctx.ext.preg_last_error != PREG_NO_ERROR {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_parser_follows_php_pcre() {
        let p = parse_pattern(b"  /a\\/b/ims").unwrap();
        assert_eq!(p.body, b"a\\/b");
        assert_eq!(p.options, opt::CASELESS | opt::MULTILINE | opt::DOTALL);
        let p = parse_pattern(b"[a[b]]u").unwrap();
        assert_eq!(p.body, b"a[b]");
        assert_eq!(p.options, opt::UTF | opt::UCP);
        assert!(matches!(parse_pattern(b""), Err(PatternError::Empty)));
        assert!(matches!(parse_pattern(b"abc"), Err(PatternError::BadDelimiter)));
        assert!(matches!(parse_pattern(b"/abc"), Err(PatternError::NoEnding(b'/'))));
        assert!(matches!(parse_pattern(b"(abc"), Err(PatternError::NoEndingMatching(b')'))));
        assert!(matches!(parse_pattern(b"/a/z"), Err(PatternError::UnknownModifier(b'z'))));
        assert!(matches!(parse_pattern(b"/a/\0"), Err(PatternError::NulModifier)));
        assert_eq!(parse_pattern(b"/a/r").unwrap().extra, opt::EXTRA_CASELESS_RESTRICT);
    }

    #[test]
    fn backrefs_read_at_most_two_digits() {
        assert_eq!(parse_backref(b"$12x"), Some((12, 3)));
        assert_eq!(parse_backref(b"${1}"), Some((1, 4)));
        assert_eq!(parse_backref(b"${1"), None);
        assert_eq!(parse_backref(b"\\0"), Some((0, 2)));
        assert_eq!(parse_backref(b"$x"), None);
        assert_eq!(parse_backref(b"$"), None);
    }

    #[test]
    fn offsets_resolve_like_php() {
        assert_eq!(resolve_offset(-1, 3), Some(2));
        assert_eq!(resolve_offset(-5, 3), Some(0));
        assert_eq!(resolve_offset(3, 3), Some(3));
        assert_eq!(resolve_offset(4, 3), None);
    }
}
