//! `parse_ini_string` / `parse_ini_file` (ext/standard `basic_functions.c`)
//! over a transcription of php's INI scanner and parser
//! (`Zend/zend_ini_scanner.l`, `Zend/zend_ini_parser.y`).
//!
//! The scanner is the re2c one rule for rule: per start condition the
//! longest match wins, the earlier rule on a tie, and — php's `YYFILL` —
//! a match that runs past the NUL after the input makes the scanner
//! return "end of file" on the spot. The parser is bison's LALR(1)
//! automaton for php's grammar: the tables below are what bison builds
//! from `zend_ini_parser.y`, driven by the bison skeleton's loop, so the
//! lookahead is read exactly when php reads it (the scanner's line number
//! in a message depends on that) and a syntax error names the unexpected
//! token and, when there are at most four, the expected ones — php's
//! `syntax error, unexpected '=' in Unknown on line 1`.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{array_key, numeric_string, Array, ArrayKey, Value};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("parse_ini_string", 1, Some(3), parse_ini_string),
    nf!("parse_ini_file", 1, Some(3), parse_ini_file),
];

#[allow(dead_code)]
pub(crate) fn register_classes(_r: &mut Registry) {}

/// The `INI_SCANNER_*` constants are registered by `info.rs`.
#[allow(dead_code)]
pub(crate) fn register_constants(_r: &mut Registry) {}

// ---- the functions -----------------------------------------------------------

const SCANNER_NORMAL: i64 = 0;
const SCANNER_RAW: i64 = 1;
const SCANNER_TYPED: i64 = 2;

/// `parse_ini_string(string $ini_string, bool $process_sections = false,
/// int $scanner_mode = INI_SCANNER_NORMAL): array|false`
fn parse_ini_string(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut text = args[0].to_php_bytes();
    // php hands the scanner a C string: a NUL ends it.
    if let Some(n) = text.iter().position(|&c| c == 0) {
        text.truncate(n);
    }
    let sections = args.get(1).is_some_and(Value::to_bool);
    let mode = args.get(2).map_or(SCANNER_NORMAL, Value::to_int);
    run(ctx, &text, None, sections, mode)
}

/// `parse_ini_file(string $filename, bool $process_sections = false,
/// int $scanner_mode = INI_SCANNER_NORMAL): array|false`
fn parse_ini_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    if name.is_empty() {
        return Err(Unwind::value_error("parse_ini_file(): Argument #1 ($filename) must not be empty"));
    }
    if name.contains(&0) {
        return Err(Unwind::value_error(
            "parse_ini_file(): Argument #1 ($filename) must not contain any null bytes",
        ));
    }
    let sections = args.get(1).is_some_and(Value::to_bool);
    let mode = args.get(2).map_or(SCANNER_NORMAL, Value::to_int);
    let path = resolve(ctx, &name);
    let shown = String::from_utf8_lossy(&name).into_owned();
    let text = if path.is_dir() {
        // php opens the directory, then fails reading it — under the
        // resolved path.
        let full = std::fs::canonicalize(&path).unwrap_or(path);
        ctx.warn(&format!(
            "parse_ini_file({}): Failed to open stream: No such file or directory",
            full.display()
        ))?;
        return Ok(Value::Bool(false));
    } else {
        match std::fs::read(&path) {
            Ok(t) => t,
            Err(e) => {
                ctx.warn(&format!(
                    "parse_ini_file({shown}): Failed to open stream: {}",
                    crate::filestat::io_text(&e)
                ))?;
                return Ok(Value::Bool(false));
            }
        }
    };
    run(ctx, &text, Some(&shown), sections, mode)
}

/// Where php's `zend_stream_open` finds `name`: an absolute or `./`
/// path as it is; otherwise the `include_path` entries, then the running
/// script's directory, then the cwd.
fn resolve(ctx: &Ctx, name: &[u8]) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(String::from_utf8_lossy(name).into_owned());
    if p.is_absolute() {
        return p;
    }
    let direct = ctx.cwd.join(&p);
    if name.starts_with(b"./") || name.starts_with(b"../") {
        return direct;
    }
    let include_path = ctx.ini_get("include_path").unwrap_or(".").to_string();
    let mut candidates: Vec<std::path::PathBuf> = include_path
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|dir| if dir == "." { ctx.cwd.join(&p) } else { ctx.cwd.join(dir).join(&p) })
        .collect();
    if let Some(dir) = std::path::Path::new(&ctx.current_file()).parent() {
        candidates.push(dir.join(&p));
    }
    candidates.into_iter().find(|c| c.is_file()).unwrap_or(direct)
}

/// Parse `text` and build php's result, or report the error and `false`.
fn run(ctx: &mut Ctx, text: &[u8], file: Option<&str>, sections: bool, mode: i64) -> NativeResult {
    if !matches!(mode, SCANNER_NORMAL | SCANNER_RAW | SCANNER_TYPED) {
        ctx.warn("Invalid scanner mode")?;
        return Ok(Value::Bool(false));
    }
    let r = {
        // `zend_get_constant` knows `true`/`false`/`null` in any case.
        let constant = |name: &[u8]| match name.to_ascii_lowercase().as_slice() {
            b"true" => Some(b"1".to_vec()),
            b"false" | b"null" => Some(Vec::new()),
            _ => ctx.constant(name).map(|v| v.to_php_bytes()),
        };
        let var = |name: &[u8]| env_var(ctx, name);
        parse(text, mode, sections, &constant, &var)
    };
    match r {
        Ok(a) => Ok(Value::Array(a)),
        Err((msg, line)) => {
            ctx.warn(&format!("{msg} in {} on line {line}\n", file.unwrap_or("Unknown")))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `${name}`: php's configuration directive of that name, else the
/// environment. rphp keeps no configuration hash (`get_cfg_var()` is
/// always `false`, as under `php -n`), so this is the environment: the
/// request's under a CGI SAPI, then the process's.
fn env_var(ctx: &Ctx, name: &[u8]) -> Option<Vec<u8>> {
    let name_s = String::from_utf8_lossy(name);
    if let Some(vars) = &ctx.request_env {
        if let Some((_, v)) = vars.iter().find(|(k, _)| *k == name_s) {
            return Some(v.as_bytes().to_vec());
        }
    }
    if name.is_empty() || name.contains(&b'=') {
        return None;
    }
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    std::env::var_os(std::ffi::OsStr::from_bytes(name)).map(OsStringExt::into_vec)
}

// ---- values --------------------------------------------------------------------

/// A semantic value: php's zval, with the parser's `Z_EXTRA` "is a number"
/// mark on strings (`INI_ZVAL_IS_NUMBER`).
#[derive(Clone, Debug, PartialEq)]
enum V {
    Str(Vec<u8>, bool),
    Bool(bool),
    Null,
    Long(i64),
    Double(f64),
}

impl V {
    fn empty() -> V {
        V::Str(Vec::new(), false)
    }

    fn str(b: &[u8]) -> V {
        V::Str(b.to_vec(), false)
    }

    fn bytes(&self) -> Vec<u8> {
        match self {
            V::Str(s, _) => s.clone(),
            V::Bool(true) => b"1".to_vec(),
            V::Bool(false) | V::Null => Vec::new(),
            V::Long(i) => i.to_string().into_bytes(),
            V::Double(d) => Value::Float(*d).to_php_bytes(),
        }
    }

    fn into_value(self) -> Value {
        match self {
            V::Str(s, _) => Value::string(&s),
            V::Bool(b) => Value::Bool(b),
            V::Null => Value::Null,
            V::Long(i) => Value::Int(i),
            V::Double(d) => Value::Float(d),
        }
    }
}

/// `zend_ini_add_string`: the concatenation, keeping the left side's mark.
fn concat(a: V, b: V) -> V {
    let mark = matches!(a, V::Str(_, true));
    let mut s = a.bytes();
    s.extend_from_slice(&b.bytes());
    V::Str(s, mark)
}

/// C's `atoi` (as macOS has it: `strtol` narrowed to `int`).
fn atoi(s: &[u8]) -> i32 {
    let mut i = 0;
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        i += 1;
    }
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut v: i128 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = (v * 10 + i128::from(s[i] - b'0')).min(i128::from(i64::MAX) + 1);
        i += 1;
    }
    let v = if neg { -v } else { v };
    let v = v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    v as i32
}

/// `get_int_val`.
fn int_val(v: &V) -> i32 {
    match v {
        V::Long(i) => *i as i32,
        V::Double(d) => *d as i32,
        V::Str(s, _) => atoi(s),
        V::Bool(b) => i32::from(*b),
        V::Null => 0,
    }
}

/// `zend_ini_do_op`.
fn do_op(op: u8, a: &V, b: Option<&V>, mode: i64) -> V {
    let x = int_val(a);
    let y = b.map_or(0, int_val);
    let r = match op {
        b'|' => x | y,
        b'&' => x & y,
        b'^' => x ^ y,
        b'~' => !x,
        b'!' => i32::from(x == 0),
        _ => 0,
    };
    if mode == SCANNER_TYPED {
        V::Long(i64::from(r))
    } else {
        V::Str(r.to_string().into_bytes(), false)
    }
}

/// `convert_to_number`: an integer, or a float that did not come from an
/// overflowing integer.
fn to_number(s: &[u8]) -> Option<V> {
    match numeric_string(s)? {
        Value::Int(i) => Some(V::Long(i)),
        Value::Float(f) => {
            let t: Vec<u8> = s
                .iter()
                .copied()
                .filter(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
                .collect();
            if t.iter().any(|c| matches!(c, b'.' | b'e' | b'E')) {
                Some(V::Double(f))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `normalize_value`: under `INI_SCANNER_TYPED`, a value that starts with
/// a number becomes that number when the whole of it is one.
fn normalize(v: V, mode: i64) -> V {
    if mode != SCANNER_TYPED {
        return v;
    }
    match &v {
        V::Str(s, true) => to_number(s).unwrap_or(v),
        _ => v,
    }
}

// ---- the scanner ---------------------------------------------------------------

const END: i32 = 0;
const TC_SECTION: i32 = 258;
const TC_RAW: i32 = 259;
const TC_CONSTANT: i32 = 260;
const TC_NUMBER: i32 = 261;
const TC_STRING: i32 = 262;
const TC_WHITESPACE: i32 = 263;
const TC_LABEL: i32 = 264;
const TC_OFFSET: i32 = 265;
const TC_DOLLAR_CURLY: i32 = 266;
const TC_VARNAME: i32 = 267;
const TC_QUOTED_STRING: i32 = 268;
const TC_FALLBACK: i32 = 269;
const BOOL_TRUE: i32 = 270;
const BOOL_FALSE: i32 = 271;
const NULL_NULL: i32 = 272;
const END_OF_LINE: i32 = 273;

/// The scanner's start conditions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cond {
    Initial,
    Offset,
    SectionValue,
    Value,
    SectionRaw,
    DoubleQuotes,
    VarFallback,
    VarName,
    Raw,
}

struct Scanner<'a> {
    buf: &'a [u8],
    pos: usize,
    cond: Cond,
    stack: Vec<Cond>,
    lineno: i32,
    mode: i64,
    /// A match ran past the NUL after the input (php's `YYFILL`).
    abort: std::cell::Cell<bool>,
}

fn is_ts(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// `LABEL_CHAR`: `[^=\n\r\t;&|^$~(){}!"[\]\x00]`.
fn label_char(c: u8) -> bool {
    !matches!(
        c,
        b'=' | b'\n' | b'\r' | b'\t' | b';' | b'&' | b'|' | b'^' | b'$' | b'~' | b'(' | b')' | b'{' | b'}' | b'!'
            | b'"' | b'[' | b']' | 0
    )
}

/// `VALUE_CHARS` without the `$` alternative.
fn value_plain(c: u8) -> bool {
    !matches!(
        c,
        b'$' | b'=' | b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&' | b'|' | b'^' | b'~' | b'(' | b')' | b'!' | b'"'
            | b'\'' | 0
    )
}

/// `SECTION_VALUE_CHARS` without the `$` and `\` alternatives.
fn section_plain(c: u8) -> bool {
    !matches!(c, b'$' | b'\n' | b'\r' | b';' | b'"' | b'\'' | b']' | b'\\')
}

/// `FALLBACK_CHARS` without the `$` and `\` alternatives.
fn fallback_plain(c: u8) -> bool {
    !matches!(c, b'$' | b'\n' | b'\r' | b';' | b'"' | b'\'' | b'}' | b'\\')
}

/// `TOKENS`, the single characters `INITIAL` returns as themselves.
fn token_char(c: u8) -> bool {
    b":,.[]\"'()&|^+-/*=%$!~<>?@{}".contains(&c)
}

/// `EAT_LEADING_WHITESPACE` / `EAT_TRAILING_WHITESPACE_EX`.
fn trim(mut s: &[u8], extra: Option<u8>) -> &[u8] {
    while let [c, rest @ ..] = s {
        if is_ts(*c) {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., c] = s {
        if Some(*c) == extra || matches!(c, b'\n' | b'\r' | b'\t' | b' ') {
            s = rest;
        } else {
            break;
        }
    }
    s
}

impl<'a> Scanner<'a> {
    fn new(buf: &'a [u8], mode: i64) -> Scanner<'a> {
        Scanner {
            buf,
            pos: 0,
            cond: Cond::Initial,
            stack: Vec::new(),
            lineno: 1,
            mode,
            abort: std::cell::Cell::new(false),
        }
    }

    /// The byte the DFA reads at `i`: the input, then the NUL php keeps
    /// after it; reading further is `YYFILL`'s "end of file".
    fn b(&self, i: usize) -> u8 {
        if i < self.buf.len() {
            self.buf[i]
        } else {
            if i > self.buf.len() {
                self.abort.set(true);
            }
            0
        }
    }

    /// A byte an action reads by hand (the zero padding past the end).
    fn raw(&self, i: usize) -> u8 {
        self.buf.get(i).copied().unwrap_or(0)
    }

    fn push(&mut self, c: Cond) {
        self.stack.push(self.cond);
        self.cond = c;
    }

    fn pop(&mut self) {
        if let Some(c) = self.stack.pop() {
            self.cond = c;
        }
    }

    // -- rule patterns: the length each matches at `p` ---------------------

    fn ts_star(&self, mut q: usize) -> usize {
        while is_ts(self.b(q)) {
            q += 1;
        }
        q
    }

    fn newline(&self, q: usize) -> Option<usize> {
        match self.b(q) {
            b'\r' if self.b(q + 1) == b'\n' => Some(q + 2),
            b'\r' | b'\n' => Some(q + 1),
            _ => None,
        }
    }

    /// `"'"{SINGLE_QUOTED_CHARS}+"'"`
    fn m_quoted(&self, p: usize) -> Option<usize> {
        if self.b(p) != b'\'' {
            return None;
        }
        let mut q = p + 1;
        loop {
            let c = self.b(q);
            if self.abort.get() {
                return None;
            }
            if c == b'\'' {
                break;
            }
            q += 1;
        }
        (q > p + 1).then_some(q + 1 - p)
    }

    /// `("w1"|"w2"|…){TABS_AND_SPACES}*`, case-insensitively.
    fn m_words(&self, p: usize, words: &[&[u8]]) -> Option<usize> {
        let mut best = None;
        for w in words {
            if (0..w.len()).all(|i| self.b(p + i).to_ascii_lowercase() == w[i]) {
                let q = self.ts_star(p + w.len());
                best = best.max(Some(q - p));
            }
        }
        best
    }

    fn label_run(&self, p: usize) -> usize {
        let mut q = p;
        while label_char(self.b(q)) {
            q += 1;
        }
        q - p
    }

    /// `{TABS_AND_SPACES}*{NEWLINE}`
    fn m_ts_newline(&self, p: usize) -> Option<usize> {
        self.newline(self.ts_star(p)).map(|q| q - p)
    }

    /// `{TABS_AND_SPACES}*[;][^\r\n]*{NEWLINE}`
    fn m_comment(&self, p: usize) -> Option<usize> {
        let mut q = self.ts_star(p);
        if self.b(q) != b';' {
            return None;
        }
        q += 1;
        loop {
            let c = self.b(q);
            if self.abort.get() {
                return None;
            }
            if c == b'\r' || c == b'\n' {
                break;
            }
            q += 1;
        }
        self.newline(q).map(|q| q - p)
    }

    /// `{CONSTANT}`
    fn m_constant(&self, p: usize) -> Option<usize> {
        let c = self.b(p);
        if !(c.is_ascii_alphabetic() || c == b'_') {
            return None;
        }
        let mut q = p + 1;
        while self.b(q).is_ascii_alphanumeric() || self.b(q) == b'_' {
            q += 1;
        }
        Some(q - p)
    }

    /// `{NUMBER}`: `[-]?{LNUM}|{DNUM}`.
    fn m_number(&self, p: usize) -> Option<usize> {
        let digits = |mut q: usize| {
            while self.b(q).is_ascii_digit() {
                q += 1;
            }
            q
        };
        let mut best = None;
        let s = if self.b(p) == b'-' { p + 1 } else { p };
        let e = digits(s);
        if e > s {
            best = Some(e - p);
        }
        // DNUM: [0-9]*[.][0-9]+ | [0-9]+[.][0-9]*
        let e = digits(p);
        if self.b(e) == b'.' {
            let f = digits(e + 1);
            if f > e + 1 || e > p {
                best = best.max(Some(f - p));
            }
        }
        best
    }

    /// `X+` for `VALUE_CHARS` (`backslash = false`), `SECTION_VALUE_CHARS`
    /// or `FALLBACK_CHARS`: a plain class, `"\\"{ANY_CHAR}` (the latter two)
    /// and `LITERAL_DOLLAR` = `"$"([^{\000]|"\\"{ANY_CHAR})`.
    fn m_chars(&self, p: usize, plain: fn(u8) -> bool, backslash: bool) -> Option<usize> {
        let n = self.buf.len();
        let mut reach = vec![false; n + 4];
        reach[p] = true;
        let mut best = None;
        let mut q = p;
        let mut far = p;
        while q <= far {
            if reach[q] {
                if q > n {
                    // The loop would read past the NUL.
                    self.abort.set(true);
                    return None;
                }
                let c = self.b(q);
                let mut mark = |to: usize, far: &mut usize, best: &mut Option<usize>| {
                    reach[to] = true;
                    *far = (*far).max(to);
                    *best = (*best).max(Some(to - p));
                };
                if plain(c) {
                    mark(q + 1, &mut far, &mut best);
                }
                if c == b'$' {
                    let d = self.b(q + 1);
                    if d != b'{' && d != 0 {
                        mark(q + 2, &mut far, &mut best);
                    }
                    if d == b'\\' {
                        self.b(q + 2);
                        mark(q + 3, &mut far, &mut best);
                    }
                }
                if backslash && c == b'\\' {
                    self.b(q + 1);
                    mark(q + 2, &mut far, &mut best);
                }
                if self.abort.get() {
                    return None;
                }
            }
            q += 1;
        }
        best
    }

    /// `{SECTION_RAW_CHARS}+`: `[^\]\n\r]+`.
    fn m_section_raw(&self, p: usize) -> Option<usize> {
        let mut q = p;
        loop {
            let c = self.b(q);
            if self.abort.get() {
                return None;
            }
            if matches!(c, b']' | b'\n' | b'\r') {
                break;
            }
            q += 1;
        }
        (q > p).then_some(q - p)
    }

    // -- the lexer -------------------------------------------------------------

    fn text(&self, p: usize, len: usize) -> &'a [u8] {
        &self.buf[p..(p + len).min(self.buf.len())]
    }

    /// `RETURN_TOKEN`: the token's value, typed under `INI_SCANNER_TYPED`
    /// in a value.
    fn value(&self, kind: i32, s: &[u8]) -> V {
        if self.mode == SCANNER_TYPED && matches!(self.cond, Cond::Value | Cond::Raw) {
            match kind {
                BOOL_TRUE => return V::Bool(true),
                BOOL_FALSE => return V::Bool(false),
                NULL_NULL => return V::Null,
                _ => {}
            }
        }
        V::str(s)
    }

    /// The longest match among `rules` (the earlier on a tie), or `None`
    /// when nothing matched; `Err` when the DFA ran past the input.
    fn pick(&self, cands: &[Option<usize>]) -> Result<Option<(usize, usize)>, ()> {
        if self.abort.get() {
            return Err(());
        }
        let mut best: Option<(usize, usize)> = None;
        for (i, c) in cands.iter().enumerate() {
            if let Some(l) = *c {
                if best.is_none_or(|(_, bl)| l > bl) {
                    best = Some((i, l));
                }
            }
        }
        Ok(best)
    }

    /// `ini_lex`: the next token and its value.
    fn lex(&mut self) -> (i32, V) {
        loop {
            let p = self.pos;
            self.abort.set(false);
            if p >= self.buf.len() {
                if matches!(self.cond, Cond::Value | Cond::Raw) {
                    self.cond = Cond::Initial;
                    return (END_OF_LINE, V::empty());
                }
                return (END, V::empty());
            }
            if p == 0 && p + 3 < self.buf.len() && self.buf.starts_with(b"\xef\xbb\xbf") {
                self.pos = 3;
                continue;
            }
            let r = match self.cond {
                Cond::Initial => self.lex_initial(p),
                Cond::Value => self.lex_value(p),
                Cond::Raw => self.lex_raw(p),
                Cond::SectionValue | Cond::Offset => self.lex_section_value(p),
                Cond::SectionRaw => self.lex_section_raw(p),
                Cond::DoubleQuotes => self.lex_double_quotes(p),
                Cond::VarName => self.lex_varname(p),
                Cond::VarFallback => self.lex_fallback(p),
            };
            match r {
                Some(t) => return t,
                None => continue,
            }
        }
    }

    fn lex_initial(&mut self, p: usize) -> Option<(i32, V)> {
        let run = self.label_run(p);
        let offset = (run > 0 && self.b(p + run) == b'[').then(|| self.ts_star(p + run + 1) - p);
        let eq = {
            let q = self.ts_star(p);
            (self.b(q) == b'=').then(|| self.ts_star(q + 1) - p)
        };
        let tsp = self.ts_star(p) - p;
        let cands = [
            (self.b(p) == b'[').then_some(1),
            offset,
            self.m_words(p, &[b"true", b"on", b"yes"]),
            self.m_words(p, &[b"false", b"off", b"no", b"none"]),
            self.m_words(p, &[b"null"]),
            (run > 0).then_some(run),
            eq,
            token_char(self.b(p)).then_some(1),
            (tsp > 0).then_some(tsp),
            self.m_ts_newline(p),
            self.m_comment(p),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        let t = self.text(p, len);
        Some(match rule {
            0 => {
                self.cond = if self.mode == SCANNER_RAW { Cond::SectionRaw } else { Cond::SectionValue };
                (TC_SECTION, V::empty())
            }
            1 => {
                self.cond = Cond::Offset;
                (TC_OFFSET, V::str(trim(t, Some(b'['))))
            }
            2 => (BOOL_TRUE, self.value(BOOL_TRUE, b"1")),
            3 => (BOOL_FALSE, self.value(BOOL_FALSE, b"")),
            4 => (NULL_NULL, self.value(NULL_NULL, b"")),
            5 => (TC_LABEL, V::str(trim(t, None))),
            6 => {
                self.cond = if self.mode == SCANNER_RAW { Cond::Raw } else { Cond::Value };
                (i32::from(b'='), V::empty())
            }
            7 => (i32::from(t[0]), V::empty()),
            8 => return None,
            9 => {
                self.lineno += 1;
                (END_OF_LINE, V::empty())
            }
            10 => {
                self.cond = Cond::Initial;
                self.lineno += 1;
                (END_OF_LINE, V::empty())
            }
            _ => (END, V::empty()),
        })
    }

    fn lex_value(&mut self, p: usize) -> Option<(i32, V)> {
        let c = self.b(p);
        let tsp = self.ts_star(p) - p;
        let cands = [
            self.m_quoted(p),
            (c == b'$' && self.b(p + 1) == b'{').then_some(2),
            self.m_words(p, &[b"true", b"on", b"yes"]),
            self.m_words(p, &[b"false", b"off", b"no", b"none"]),
            self.m_words(p, &[b"null"]),
            self.m_ts_newline(p),
            self.m_constant(p),
            self.m_number(p),
            b"&|^~()!".contains(&c).then(|| self.ts_star(p + 1) - p),
            (c == b'=').then_some(1),
            self.m_chars(p, value_plain, false),
            (self.b(p + tsp) == b'"').then_some(tsp + 1),
            (tsp > 0).then_some(tsp),
            self.m_comment(p),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        let t = self.text(p, len);
        Some(match rule {
            0 => (TC_RAW, self.value(TC_RAW, &t[1..t.len() - 1])),
            1 => {
                self.push(Cond::VarName);
                (TC_DOLLAR_CURLY, V::empty())
            }
            2 => (BOOL_TRUE, self.value(BOOL_TRUE, b"1")),
            3 => (BOOL_FALSE, self.value(BOOL_FALSE, b"")),
            4 => (NULL_NULL, self.value(NULL_NULL, b"")),
            5 => {
                self.cond = Cond::Initial;
                self.lineno += 1;
                (END_OF_LINE, V::empty())
            }
            6 => (TC_CONSTANT, self.value(TC_CONSTANT, t)),
            7 => (TC_NUMBER, self.value(TC_NUMBER, t)),
            8 => (i32::from(t[0]), V::empty()),
            9 => {
                // `yyless(0)`: the `=` is scanned again, in INITIAL.
                self.pos = p;
                self.cond = Cond::Initial;
                (END_OF_LINE, V::empty())
            }
            10 => (TC_STRING, self.value(TC_STRING, t)),
            11 => {
                self.push(Cond::DoubleQuotes);
                (i32::from(b'"'), V::empty())
            }
            12 => (TC_WHITESPACE, self.value(TC_WHITESPACE, t)),
            13 => {
                self.cond = Cond::Initial;
                self.lineno += 1;
                (END_OF_LINE, V::empty())
            }
            _ => {
                self.cond = Cond::Initial;
                (END_OF_LINE, V::empty())
            }
        })
    }

    fn lex_raw(&mut self, p: usize) -> Option<(i32, V)> {
        let c = self.b(p);
        let tsp = self.ts_star(p) - p;
        let cands = [
            (!matches!(c, b'\n' | b'\r' | b';' | 0)).then_some(1),
            self.m_ts_newline(p),
            (tsp > 0).then_some(tsp),
            self.m_comment(p),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        Some(match rule {
            0 => {
                let n = self.buf.len();
                // `EAT_LEADING_WHITESPACE` over the one matched byte.
                let t = if is_ts(self.raw(p)) { p + 1 } else { p };
                let mut cur = p + 1;
                let mut sc: Option<usize> = None;
                while cur < n {
                    match self.buf[cur] {
                        b'\n' | b'\r' => break,
                        b';' => {
                            sc.get_or_insert(cur);
                            cur += 1;
                        }
                        b'"' => {
                            if self.raw(t) == b'"' {
                                sc = None;
                            }
                            cur += 1;
                        }
                        _ => cur += 1,
                    }
                }
                let end = sc.unwrap_or(cur).max(t);
                self.pos = cur;
                let mut s = &self.buf[t..end];
                while let [rest @ .., c] = s {
                    if matches!(c, b'\n' | b'\r' | b'\t' | b' ') {
                        s = rest;
                    } else {
                        break;
                    }
                }
                if s.len() > 1 && s[0] == b'"' && s[s.len() - 1] == b'"' {
                    s = &s[1..s.len() - 1];
                }
                (TC_RAW, self.value(TC_RAW, s))
            }
            1 => {
                self.cond = Cond::Initial;
                self.lineno += 1;
                (END_OF_LINE, V::empty())
            }
            2 => return None,
            3 => {
                self.cond = Cond::Initial;
                self.lineno += 1;
                (END_OF_LINE, V::empty())
            }
            _ => {
                self.cond = Cond::Initial;
                (END_OF_LINE, V::empty())
            }
        })
    }

    /// `ST_SECTION_VALUE` and `ST_OFFSET`, which differ in how `]` ends.
    fn lex_section_value(&mut self, p: usize) -> Option<(i32, V)> {
        let c = self.b(p);
        let tsp = self.ts_star(p) - p;
        let close = if self.cond == Cond::SectionValue {
            (c == b']').then(|| {
                let q = self.ts_star(p + 1);
                self.newline(q).unwrap_or(q) - p
            })
        } else {
            (self.b(p + tsp) == b']').then_some(tsp + 1)
        };
        let cands = [
            self.m_quoted(p),
            close,
            (c == b'$' && self.b(p + 1) == b'{').then_some(2),
            self.m_constant(p),
            self.m_number(p),
            self.m_chars(p, section_plain, true),
            (self.b(p + tsp) == b'"').then_some(tsp + 1),
            (tsp > 0).then_some(tsp),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        let t = self.text(p, len);
        Some(match rule {
            0 => (TC_RAW, V::str(&t[1..t.len() - 1])),
            1 => {
                if self.cond == Cond::SectionValue {
                    self.lineno += 1;
                }
                self.cond = Cond::Initial;
                (i32::from(b']'), V::empty())
            }
            2 => {
                self.push(Cond::VarName);
                (TC_DOLLAR_CURLY, V::empty())
            }
            3 => (TC_CONSTANT, V::str(t)),
            4 => (TC_NUMBER, V::str(t)),
            5 => (TC_STRING, V::str(t)),
            6 => {
                self.push(Cond::DoubleQuotes);
                (i32::from(b'"'), V::empty())
            }
            7 => (TC_WHITESPACE, V::str(t)),
            _ => (END, V::empty()),
        })
    }

    fn lex_section_raw(&mut self, p: usize) -> Option<(i32, V)> {
        let close = (self.b(p) == b']').then(|| {
            let q = self.ts_star(p + 1);
            self.newline(q).unwrap_or(q) - p
        });
        let cands = [close, self.m_section_raw(p), Some(1)];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        Some(match rule {
            0 => {
                self.cond = Cond::Initial;
                self.lineno += 1;
                (i32::from(b']'), V::empty())
            }
            1 => (TC_RAW, V::str(self.text(p, len))),
            _ => (END, V::empty()),
        })
    }

    fn lex_double_quotes(&mut self, p: usize) -> Option<(i32, V)> {
        let c = self.b(p);
        let cands = [
            (c == b'$' && self.b(p + 1) == b'{').then_some(2),
            (c == b'"').then(|| self.ts_star(p + 1) - p),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        Some(match rule {
            0 => {
                self.push(Cond::VarName);
                (TC_DOLLAR_CURLY, V::empty())
            }
            1 => {
                self.pop();
                (i32::from(b'"'), V::empty())
            }
            _ => {
                let n = self.buf.len();
                let mut s = p;
                while s < n {
                    let ch = self.buf[s];
                    s += 1;
                    let stop = match ch {
                        b'"' => true,
                        b'$' => s < n && self.buf[s] == b'{',
                        b'\\' if s < n => {
                            let esc = self.buf[s];
                            s += 1;
                            esc == b'"' && (s >= n || self.buf[s] == b'\n' || self.buf[s] == b'\r')
                        }
                        _ => false,
                    };
                    if stop {
                        s -= 1;
                        break;
                    }
                }
                self.pos = s;
                let v = self.escape(p, s);
                (TC_QUOTED_STRING, V::Str(v, false))
            }
        })
    }

    /// `zend_ini_escape_string` over `buf[from..to]`, counting its lines.
    fn escape(&mut self, from: usize, to: usize) -> Vec<u8> {
        let src = &self.buf[from..to];
        let n = src.len();
        let mut out = Vec::with_capacity(n);
        let mut s = 0;
        while s < n {
            if src[s] == b'\\' {
                s += 1;
                if s >= n {
                    out.push(b'\\');
                    continue;
                }
                match src[s] {
                    b'"' | b'\\' | b'$' => out.push(src[s]),
                    c => {
                        out.push(b'\\');
                        out.push(c);
                    }
                }
            } else {
                out.push(src[s]);
            }
            if src[s] == b'\n' || (src[s] == b'\r' && src.get(s + 1).copied().unwrap_or(0) != b'\n') {
                self.lineno += 1;
            }
            s += 1;
        }
        out
    }

    fn lex_varname(&mut self, p: usize) -> Option<(i32, V)> {
        let c = self.b(p);
        let cands = [
            (c == b':' && self.b(p + 1) == b'-').then_some(2),
            label_char(c).then_some(1),
            (c == b'}').then_some(1),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        Some(match rule {
            0 => self.fallback_start(),
            1 => {
                let n = self.buf.len();
                let mut cur = p + 1;
                if self.raw(cur) == b':' && self.raw(cur + 1) == b'-' {
                    self.pos = cur + 1;
                    return Some(self.fallback_start());
                }
                let mut yyleng = 1;
                while cur < n {
                    let ch = self.buf[cur];
                    cur += 1;
                    let stop = match ch {
                        b'=' | b'\n' | b'\r' | b'\t' | b';' | b'&' | b'|' | b'^' | b'$' | b'~' | b'(' | b')'
                        | b'{' | b'}' | b'!' | b'"' | b'[' | b']' => true,
                        b':' => self.raw(cur) == b'-',
                        _ => false,
                    };
                    if stop {
                        cur -= 1;
                        yyleng = cur - p;
                        break;
                    }
                }
                self.pos = cur;
                (TC_VARNAME, V::str(trim(self.text(p, yyleng), None)))
            }
            2 => {
                self.pop();
                (i32::from(b'}'), V::empty())
            }
            _ => (END, V::empty()),
        })
    }

    fn fallback_start(&mut self) -> (i32, V) {
        self.pop();
        self.push(Cond::VarFallback);
        (TC_FALLBACK, V::empty())
    }

    fn lex_fallback(&mut self, p: usize) -> Option<(i32, V)> {
        let c = self.b(p);
        let tsp = self.ts_star(p) - p;
        let cands = [
            (c == b'$' && self.b(p + 1) == b'{').then_some(2),
            (c == b'}').then_some(1),
            self.m_constant(p),
            self.m_number(p),
            self.m_chars(p, fallback_plain, true),
            (self.b(p + tsp) == b'"').then_some(tsp + 1),
            (tsp > 0).then_some(tsp),
            Some(1),
        ];
        let Ok(Some((rule, len))) = self.pick(&cands) else {
            return Some((END, V::empty()));
        };
        self.pos = p + len;
        let t = self.text(p, len);
        Some(match rule {
            0 => {
                self.push(Cond::VarName);
                (TC_DOLLAR_CURLY, V::empty())
            }
            1 => {
                self.pop();
                (i32::from(b'}'), V::empty())
            }
            2 => (TC_CONSTANT, V::str(t)),
            3 => (TC_NUMBER, V::str(t)),
            4 => (TC_STRING, V::str(t)),
            5 => {
                self.push(Cond::DoubleQuotes);
                (i32::from(b'"'), V::empty())
            }
            6 => (TC_WHITESPACE, V::str(t)),
            _ => (END, V::empty()),
        })
    }
}

// ---- the parser ------------------------------------------------------------------
//
// bison's tables for php's `zend_ini_parser.y` (rules numbered as bison
// numbers them: 1 is `$accept`, 2–3 `statement_list`, 4–8 `statement`,
// 9–10 `section_string_or_value`, 11–15 `string_or_value`, 16–17
// `option_offset`, 18–20 `encapsed_list`, 21–26
// `var_string_list_section`, 27–32 `var_string_list`, 33–39 `expr`,
// 40–41 `cfg_var_ref`, 42–43 `fallback`, 44–48 `constant_literal`,
// 49–53 `constant_string`).

const TRANSLATE: [u8; 274] = [
    0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 41, 23, 2, 31, 30, 40, 24, 43, 44, 29, 26, 21, 27, 22, 28,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 20, 2, 33, 19, 34, 35,
    36, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 42, 25, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 37, 39, 38, 32, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    17, 18,
];
const R1: [u8; 54] = [
    0, 45, 46, 46, 47, 47, 47, 47, 47, 48, 48, 49, 49, 49, 49, 49,
    50, 50, 51, 51, 51, 52, 52, 52, 52, 52, 52, 53, 53, 53, 53, 53,
    53, 54, 54, 54, 54, 54, 54, 54, 55, 55, 56, 56, 57, 57, 57, 57,
    57, 58, 58, 58, 58, 58,
];
const R2: [u8; 54] = [
    0, 2, 2, 0, 3, 3, 5, 1, 1, 1, 0, 1, 1, 1, 1, 1,
    1, 0, 2, 2, 0, 1, 1, 3, 2, 2, 4, 1, 1, 3, 2, 2,
    4, 1, 3, 3, 3, 2, 2, 3, 3, 5, 1, 0, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1,
];
const DEFACT: [u8; 76] = [
    3, 0, 1, 10, 7, 17, 8, 2, 45, 44, 46, 47, 48, 0, 20, 0,
    9, 21, 22, 0, 50, 49, 51, 52, 53, 20, 0, 16, 27, 28, 0, 0,
    4, 20, 24, 25, 12, 13, 14, 15, 0, 0, 0, 5, 33, 11, 0, 0,
    20, 30, 31, 43, 40, 19, 23, 18, 0, 37, 38, 0, 0, 0, 0, 29,
    0, 0, 42, 0, 26, 39, 36, 34, 35, 6, 32, 41,
];
const DEFGOTO: [i16; 14] = [-1, 1, 7, 15, 43, 26, 31, 16, 44, 45, 28, 67, 18, 29];
const PACT: [i16; 76] = [
    -46, 118, -46, 73, -17, 81, -46, -46, -46, -46, -46, -46, -46, 0, -46, -34,
    94, -46, -46, -1, -46, -46, -46, -46, -46, -46, -31, 102, -46, -46, 6, 59,
    -46, -46, -46, -46, -46, -46, -46, -46, 28, 28, 28, -46, 102, 25, 80, 2,
    -46, -46, -46, 81, -46, -46, -46, -46, 109, -46, -46, 72, 28, 28, 28, -46,
    -1, 120, 102, -20, -46, -46, -46, -46, -46, -46, -46, -46,
];
const PGOTO: [i16; 14] = [-46, -46, -46, -46, -45, -46, 4, -46, -4, 14, -3, -46, 7, -18];
const TABLE: [i16; 144] = [
    17, 27, 19, 20, 21, 22, 23, 24, 32, 50, 13, 47, 30, 34, 36, 37,
    38, 39, 75, 73, 51, 64, 25, 35, 49, 0, 50, 0, 55, 46, 0, 40,
    20, 21, 22, 23, 24, 56, 0, 13, 41, 49, 42, 55, 52, 0, 0, 66,
    50, 0, 60, 25, 65, 55, 57, 58, 59, 0, 0, 0, 40, 0, 55, 49,
    61, 62, 0, 0, 0, 41, 13, 42, 53, 0, 70, 71, 72, 8, 9, 10,
    11, 12, 54, 0, 13, 20, 21, 22, 23, 24, 0, 13, 13, 53, 0, 0,
    14, 60, 8, 9, 10, 11, 12, 63, 25, 13, 20, 21, 22, 23, 24, 61,
    62, 13, 0, 0, 69, 33, 2, 0, 13, 3, 53, 0, 0, 48, 0, 4,
    5, 0, 0, 13, 68, 53, 0, 0, 6, 0, 0, 0, 0, 0, 0, 74,
];
const CHECK: [i16; 144] = [
    3, 5, 19, 4, 5, 6, 7, 8, 42, 27, 11, 42, 12, 16, 15, 16,
    17, 18, 38, 64, 14, 19, 23, 16, 27, -1, 44, -1, 31, 25, -1, 32,
    4, 5, 6, 7, 8, 33, -1, 11, 41, 44, 43, 46, 38, -1, -1, 51,
    66, -1, 25, 23, 48, 56, 40, 41, 42, -1, -1, -1, 32, -1, 65, 66,
    39, 40, -1, -1, -1, 41, 11, 43, 13, -1, 60, 61, 62, 4, 5, 6,
    7, 8, 23, -1, 11, 4, 5, 6, 7, 8, -1, 11, 11, 13, -1, -1,
    23, 25, 4, 5, 6, 7, 8, 23, 23, 11, 4, 5, 6, 7, 8, 39,
    40, 11, -1, -1, 44, 23, 0, -1, 11, 3, 13, -1, -1, 23, -1, 9,
    10, -1, -1, 11, 23, 13, -1, -1, 18, -1, -1, -1, -1, -1, -1, 23,
];
/// The terminals' names as php's messages spell them.
const TNAME: [&str; 45] = [
    "end of file", "error", "invalid token", "TC_SECTION", "TC_RAW", "TC_CONSTANT", "TC_NUMBER", "TC_STRING",
    "TC_WHITESPACE", "TC_LABEL", "TC_OFFSET", "TC_DOLLAR_CURLY", "TC_VARNAME", "TC_QUOTED_STRING", "TC_FALLBACK",
    "BOOL_TRUE", "BOOL_FALSE", "NULL_NULL", "END_OF_LINE", "'='", "':'", "','", "'.'", "'\"'", "'''", "'^'", "'+'",
    "'-'", "'/'", "'*'", "'%'", "'$'", "'~'", "'<'", "'>'", "'?'", "'@'", "'{'", "'}'", "'|'", "'&'", "'!'", "']'",
    "'('", "')'",
];
const FINAL: i32 = 2;
const LAST: i32 = 143;
const NTOKENS: i32 = 45;
const PACT_NINF: i32 = -46;

fn translate(tok: i32) -> i32 {
    if tok <= 0 {
        0
    } else {
        TRANSLATE.get(tok as usize).map_or(2, |&t| i32::from(t))
    }
}

/// php's `syntax error, unexpected X, expecting A or B` for the parser in
/// `state` with lookahead symbol `sym` (bison's `yysyntax_error`).
fn syntax_error(state: i32, sym: i32) -> String {
    let mut msg = format!("syntax error, unexpected {}", TNAME[sym as usize]);
    let n = i32::from(PACT[state as usize]);
    if n != PACT_NINF {
        let begin = if n < 0 { -n } else { 0 };
        let end = (LAST - n + 1).min(NTOKENS);
        let mut expected = Vec::new();
        for x in begin..end {
            if i32::from(CHECK[(x + n) as usize]) == x && x != 1 {
                expected.push(x);
            }
        }
        if !expected.is_empty() && expected.len() <= 4 {
            for (i, x) in expected.iter().enumerate() {
                msg.push_str(if i == 0 { ", expecting " } else { " or " });
                msg.push_str(TNAME[*x as usize]);
            }
        }
    }
    msg
}

/// The result being built: php's two parser callbacks.
struct Builder {
    arr: Array,
    sections: bool,
    /// The key of the section being filled (`BG(active_ini_file_section)`).
    section: Option<ArrayKey>,
}

/// `zend_symtable_update`'s key.
fn sym_key(s: &[u8]) -> ArrayKey {
    array_key(&Value::string(s)).unwrap_or_else(|| ArrayKey::str(s))
}

impl Builder {
    fn target(&mut self) -> &mut Array {
        if let Some(k) = &self.section {
            if let Some(Value::Array(a)) = self.arr.get_mut(k) {
                return a;
            }
        }
        &mut self.arr
    }

    fn section(&mut self, name: &[u8]) {
        if self.sections {
            let k = sym_key(name);
            self.arr.set(k.clone(), Value::Array(Array::new()));
            self.section = Some(k);
        }
    }

    fn entry(&mut self, key: &[u8], value: V) {
        self.target().set(sym_key(key), value.into_value());
    }

    /// `key[offset] = value`.
    fn pop_entry(&mut self, key: &[u8], value: V, offset: &[u8]) {
        let arr = self.target();
        let int_key = !(key.len() > 1 && key[0] == b'0') && matches!(numeric_string(key), Some(Value::Int(_)));
        let k = if int_key {
            ArrayKey::Int(strtoul(key) as i64)
        } else {
            ArrayKey::str(key)
        };
        let mut inner = match arr.get(&k).map(|v| v.deref().into_owned()) {
            Some(Value::Array(a)) => a,
            _ => Array::new(),
        };
        // Drop the outer array's handle so the inner one is written in place.
        arr.set(k.clone(), Value::Null);
        if offset.is_empty() {
            inner.push(value.into_value());
        } else {
            inner.set(sym_key(offset), value.into_value());
        }
        arr.set(k, Value::Array(inner));
    }
}

/// C's `strtoul(s, NULL, 0)` of a decimal integer string.
fn strtoul(s: &[u8]) -> u64 {
    let t = trim(s, None);
    let (neg, digits) = match t.first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: u64 = 0;
    for &c in digits.iter().take_while(|c| c.is_ascii_digit()) {
        v = v.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
    }
    if neg {
        v.wrapping_neg()
    } else {
        v
    }
}

/// Parse an INI text as php's scanner in `mode` does: the array, or the
/// syntax error's message and the scanner's line at it.
fn parse(
    text: &[u8],
    mode: i64,
    sections: bool,
    constant: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    var: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Array, (String, i32)> {
    let mut sc = Scanner::new(text, mode);
    let mut out = Builder {
        arr: Array::new(),
        sections,
        section: None,
    };
    let mut states: Vec<i32> = vec![0];
    let mut vals: Vec<V> = vec![V::empty()];
    let mut look: Option<(i32, V)> = None;
    let mut state: i32 = 0;
    loop {
        // yybackup
        let n = i32::from(PACT[state as usize]);
        let mut rule: i32 = 0;
        let mut shifted = false;
        if n != PACT_NINF {
            if look.is_none() {
                look = Some(sc.lex());
            }
            let sym = translate(look.as_ref().map_or(0, |l| l.0));
            let idx = n + sym;
            if (0..=LAST).contains(&idx) && i32::from(CHECK[idx as usize]) == sym {
                let act = i32::from(TABLE[idx as usize]);
                if act <= 0 {
                    return Err((syntax_error(state, sym), sc.lineno));
                }
                if act == FINAL {
                    return Ok(out.arr);
                }
                let (_, v) = look.take().unwrap_or((0, V::empty()));
                state = act;
                states.push(state);
                vals.push(v);
                shifted = true;
            }
        }
        if shifted {
            continue;
        }
        // yydefault
        if rule == 0 {
            rule = i32::from(DEFACT[state as usize]);
            if rule == 0 {
                let sym = translate(look.as_ref().map_or(0, |l| l.0));
                return Err((syntax_error(state, sym), sc.lineno));
            }
        }
        // yyreduce
        let len = R2[rule as usize] as usize;
        let at = vals.len() - len;
        let rhs: Vec<V> = vals.split_off(at);
        states.truncate(states.len() - len);
        let arg = |i: usize| rhs[i - 1].clone();
        let result = match rule {
            4 => {
                out.section(&arg(2).bytes());
                V::empty()
            }
            5 => {
                out.entry(&arg(1).bytes(), arg(3));
                V::empty()
            }
            6 => {
                out.pop_entry(&arg(1).bytes(), arg(5), &arg(2).bytes());
                V::empty()
            }
            10 | 15 | 17 | 20 | 43 => V::empty(),
            11 => normalize(arg(1), mode),
            18 | 19 | 24 | 25 | 30 | 31 => concat(arg(1), arg(2)),
            23 | 29 | 39 => arg(2),
            26 | 32 => concat(arg(1), arg(3)),
            34 => do_op(b'|', &arg(1), Some(&arg(3)), mode),
            35 => do_op(b'&', &arg(1), Some(&arg(3)), mode),
            36 => do_op(b'^', &arg(1), Some(&arg(3)), mode),
            37 => do_op(b'~', &arg(2), None, mode),
            38 => do_op(b'!', &arg(2), None, mode),
            40 | 41 => {
                let name = arg(2).bytes();
                match var(&name) {
                    Some(v) => V::Str(v, false),
                    None if rule == 41 => V::Str(arg(4).bytes(), false),
                    None => V::empty(),
                }
            }
            49 => {
                let name = arg(1).bytes();
                match (!name.contains(&b':')).then(|| constant(&name)).flatten() {
                    Some(v) => V::Str(v, false),
                    None => arg(1),
                }
            }
            51 => V::Str(arg(1).bytes(), true),
            _ if len > 0 => arg(1),
            _ => V::empty(),
        };
        let lhs = i32::from(R1[rule as usize]) - NTOKENS;
        let top = *states.last().unwrap_or(&0);
        let g = i32::from(PGOTO[lhs as usize]) + top;
        state = if (0..=LAST).contains(&g) && i32::from(CHECK[g as usize]) == top {
            i32::from(TABLE[g as usize])
        } else {
            i32::from(DEFGOTO[lhs as usize])
        };
        states.push(state);
        vals.push(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str, mode: i64) -> Result<Vec<(String, String)>, String> {
        let none = |_: &[u8]| None;
        parse(s.as_bytes(), mode, false, &none, &none).map_err(|e| e.0).map(|a| {
            a.iter()
                .map(|(k, v)| {
                    (String::from_utf8_lossy(&k.to_value().to_php_bytes()).into_owned(), String::from_utf8_lossy(&v.to_php_bytes()).into_owned())
                })
                .collect()
        })
    }

    #[test]
    fn plain_values() {
        assert_eq!(p("a = 1\nb = \"x y\"\n", 0).unwrap(), vec![("a".into(), "1".into()), ("b".into(), "x y".into())]);
        assert_eq!(p("a = 1 | 6", 0).unwrap(), vec![("a".into(), "7".into())]);
    }

    #[test]
    fn errors_name_tokens() {
        assert_eq!(p("a = b = c", 0).unwrap_err(), "syntax error, unexpected '='");
        assert_eq!(p("yes = 1", 0).unwrap_err(), "syntax error, unexpected BOOL_TRUE");
        assert_eq!(
            p("a = (1", 0).unwrap_err(),
            "syntax error, unexpected END_OF_LINE, expecting '^' or '|' or '&' or ')'"
        );
        assert_eq!(
            p("a = \"x", 0).unwrap_err(),
            "syntax error, unexpected end of file, expecting TC_DOLLAR_CURLY or TC_QUOTED_STRING or '\"'"
        );
    }
}
