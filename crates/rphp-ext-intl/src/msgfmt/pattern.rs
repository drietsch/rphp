//! ICU's `MessagePattern` (apostrophe mode `DOUBLE_OPTIONAL`) as a tree:
//! the message text with its quoting resolved, `#` in a plural
//! sub-message, and the arguments — none, simple (`number`, `date`, …
//! with the style text as written), `choice`, `plural`, `select` and
//! `selectordinal`. The parser follows `messagepattern.cpp` step for
//! step, so the error codes and the `UParseError` offsets and contexts
//! (in UTF-16 units, 15 of them each side) are the ones php reports.

use crate::state::{U_INDEX_OUTOFBOUNDS_ERROR, U_PATTERN_SYNTAX_ERROR};

/// `U_DEFAULT_KEYWORD_MISSING`.
pub const U_DEFAULT_KEYWORD_MISSING: i64 = 65807;

/// `U_UNMATCHED_BRACES`.
pub const U_UNMATCHED_BRACES: i64 = 65801;

/// ICU's `Part::MAX_LENGTH` and `Part::MAX_VALUE`.
const MAX_LENGTH: usize = 0xffff;
const MAX_VALUE: i64 = 0x7fff;

/// A message: text, `#` and arguments in order.
#[derive(Clone, Debug, Default)]
pub struct Message {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Text(String),
    /// `#` in a plural or selectordinal sub-message.
    Pound,
    Arg(Box<Arg>),
}

/// An argument: its name as written (`0`, `name`) and its number when it
/// is one.
#[derive(Clone, Debug)]
pub struct Arg {
    pub name: String,
    pub number: Option<u32>,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub enum Kind {
    None,
    /// `{n, type[, style]}`: the type as written, the style text raw
    /// (quotes and surrounding white space included).
    Simple { ty: String, style: Option<String>, slot: usize },
    Choice(Vec<ChoiceCase>),
    Plural { ordinal: bool, offset: f64, cases: Vec<(Selector, Message)> },
    Select(Vec<(String, Message)>),
}

#[derive(Clone, Debug)]
pub struct ChoiceCase {
    pub limit: f64,
    /// `<`: the case starts just above the limit (`#` and `≤` include it).
    pub less: bool,
    pub message: Message,
    /// The sub-message as written: `ChoiceFormat::parseArgument` matches
    /// the pattern text itself.
    pub raw: String,
}

#[derive(Clone, Debug)]
pub enum Selector {
    /// `=2`
    Explicit(f64),
    Keyword(String),
}

/// A pattern error: the ICU code, and the parse error position when the
/// parser set one (`setParseError`).
#[derive(Clone, Debug)]
pub struct PatternError {
    pub code: i64,
    pub offset: usize,
    pub pre: String,
    pub post: String,
}

impl PatternError {
    /// php's `intl_parse_error_to_string()` of the position.
    pub fn describe(&self) -> String {
        let mut s = String::from("parse error ");
        s.push_str(&format!("at offset {}", self.offset));
        if !self.pre.is_empty() {
            s.push_str(&format!(", after \"{}\"", self.pre));
        }
        if !self.post.is_empty() {
            s.push_str(&format!(", before or at \"{}\"", self.post));
        }
        s
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    None,
    Choice,
    Plural,
    Select,
    SelectOrdinal,
}

impl Parent {
    fn has_plural_style(self) -> bool {
        matches!(self, Parent::Plural | Parent::SelectOrdinal)
    }
}

const APOS: u16 = b'\'' as u16;
const LBRACE: u16 = b'{' as u16;
const RBRACE: u16 = b'}' as u16;
const PIPE: u16 = b'|' as u16;
const POUND: u16 = b'#' as u16;
const COMMA: u16 = b',' as u16;

/// `PatternProps::isWhiteSpace`.
pub fn is_white(c: u16) -> bool {
    matches!(c, 0x09..=0x0d | 0x20 | 0x85 | 0x200e | 0x200f | 0x2028 | 0x2029)
}

/// `PatternProps::isSyntaxOrWhiteSpace`.
fn is_syntax_or_white(c: u16) -> bool {
    if is_white(c) {
        return true;
    }
    match char::from_u32(u32::from(c)) {
        Some(ch) => icu::properties::CodePointSetData::new::<icu::properties::props::PatternSyntax>().contains(ch),
        None => false,
    }
}

struct Parser<'a> {
    msg: &'a [u16],
    err: Option<PatternError>,
}

type PResult<T> = Result<T, ()>;

impl<'a> Parser<'a> {
    fn len(&self) -> usize {
        self.msg.len()
    }

    fn at(&self, i: usize) -> u16 {
        self.msg.get(i).copied().unwrap_or(0)
    }

    fn text(&self, start: usize, end: usize) -> String {
        String::from_utf16_lossy(&self.msg[start..end])
    }

    /// `setParseError` and the error code.
    fn fail(&mut self, code: i64, index: usize) -> PResult<()> {
        let msg = self.msg;
        let mut len = index;
        if len >= 16 {
            len = 15;
            if len > 0 && (0xdc00..0xe000).contains(&msg[index - len]) {
                len -= 1;
            }
        }
        let pre = String::from_utf16_lossy(&msg[index - len..index]);
        let mut len = msg.len() - index;
        if len >= 16 {
            len = 15;
            if len > 0 && (0xd800..0xdc00).contains(&msg[index + len - 1]) {
                len -= 1;
            }
        }
        let post = String::from_utf16_lossy(&msg[index..index + len]);
        self.err = Some(PatternError { code, offset: index, pre, post });
        Err(())
    }

    fn skip_white(&self, mut i: usize) -> usize {
        while i < self.len() && is_white(self.msg[i]) {
            i += 1;
        }
        i
    }

    fn skip_identifier(&self, mut i: usize) -> usize {
        while i < self.len() && !is_syntax_or_white(self.msg[i]) {
            i += 1;
        }
        i
    }

    fn skip_double(&self, mut i: usize) -> usize {
        while i < self.len() {
            let c = self.msg[i];
            if (c < 0x30 && c != b'+' as u16 && c != b'-' as u16 && c != b'.' as u16) || (c > 0x39 && c != b'e' as u16 && c != b'E' as u16 && c != 0x221e) {
                break;
            }
            i += 1;
        }
        i
    }

    /// `parseDouble`: a small integer, `∞` where allowed, or a double.
    fn parse_double(&mut self, start: usize, limit: usize, allow_infinity: bool) -> PResult<f64> {
        let s = &self.msg[start..limit];
        let mut i = 0;
        let mut negative = false;
        let mut c = s[i];
        i += 1;
        let bad = 'parse: {
            if c == b'-' as u16 || c == b'+' as u16 {
                negative = c == b'-' as u16;
                if i == s.len() {
                    break 'parse true;
                }
                c = s[i];
                i += 1;
            }
            if c == 0x221e {
                if allow_infinity && i == s.len() {
                    return Ok(if negative { f64::NEG_INFINITY } else { f64::INFINITY });
                }
                break 'parse true;
            }
            let _ = c;
            // strtod over the invariant characters
            let text: String = match String::from_utf16(s) {
                Ok(t) => t,
                Err(_) => break 'parse true,
            };
            if !text.is_ascii() {
                break 'parse true;
            }
            match strtod_exact(&text) {
                Some(v) => return Ok(v),
                None => break 'parse true,
            }
        };
        let _ = bad;
        self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
        Ok(0.0)
    }

    fn parse_message(&mut self, mut index: usize, start_len: usize, nesting: usize, parent: Parent) -> PResult<(Message, usize)> {
        let mut out = Message::default();
        let mut text: Vec<u16> = Vec::new();
        index += start_len;
        let flush = |text: &mut Vec<u16>, out: &mut Message| {
            if !text.is_empty() {
                out.items.push(Item::Text(String::from_utf16_lossy(text)));
                text.clear();
            }
        };
        while index < self.len() {
            let c = self.msg[index];
            index += 1;
            if c == APOS {
                if index == self.len() {
                    // a trailing apostrophe is literal
                    text.push(APOS);
                } else {
                    let d = self.msg[index];
                    if d == APOS {
                        // a doubled apostrophe
                        text.push(APOS);
                        index += 1;
                    } else if d == LBRACE || d == RBRACE || (parent == Parent::Choice && d == PIPE) || (parent.has_plural_style() && d == POUND) {
                        // quoted literal text up to the next single
                        // apostrophe (a doubled one inside is one)
                        let mut seg = index;
                        let mut from = index + 1;
                        loop {
                            match self.msg[from.min(self.len())..].iter().position(|&u| u == APOS) {
                                Some(p) => {
                                    let q = from + p;
                                    if self.at(q + 1) == APOS {
                                        text.extend_from_slice(&self.msg[seg..=q]);
                                        seg = q + 2;
                                        from = q + 2;
                                    } else {
                                        text.extend_from_slice(&self.msg[seg..q]);
                                        index = q + 1;
                                        break;
                                    }
                                }
                                None => {
                                    text.extend_from_slice(&self.msg[seg.min(self.len())..]);
                                    index = self.len();
                                    break;
                                }
                            }
                        }
                    } else {
                        text.push(APOS);
                    }
                }
            } else if parent.has_plural_style() && c == POUND {
                flush(&mut text, &mut out);
                out.items.push(Item::Pound);
            } else if c == LBRACE {
                flush(&mut text, &mut out);
                let (arg, next) = self.parse_arg(index - 1, 1, nesting)?;
                out.items.push(Item::Arg(Box::new(arg)));
                index = next;
            } else if (nesting > 0 && c == RBRACE) || (parent == Parent::Choice && c == PIPE) {
                flush(&mut text, &mut out);
                if parent == Parent::Choice {
                    return Ok((out, index - 1));
                }
                return Ok((out, index));
            } else {
                text.push(c);
            }
        }
        if nesting > 0 {
            self.fail(U_UNMATCHED_BRACES, 0)?;
        }
        flush(&mut text, &mut out);
        Ok((out, index))
    }

    fn parse_arg(&mut self, index: usize, start_len: usize, nesting: usize) -> PResult<(Arg, usize)> {
        let name_index = self.skip_white(index + start_len);
        let mut index = name_index;
        if index == self.len() {
            self.fail(U_UNMATCHED_BRACES, 0)?;
        }
        index = self.skip_identifier(index);
        let name = self.text(name_index, index);
        let number = parse_arg_number(&self.msg[name_index..index]);
        let number = match number {
            ArgNumber::Number(n) => {
                if index - name_index > MAX_LENGTH || i64::from(n) > MAX_VALUE {
                    self.fail(U_INDEX_OUTOFBOUNDS_ERROR, name_index)?;
                }
                Some(n)
            }
            ArgNumber::NotNumber => {
                if index - name_index > MAX_LENGTH {
                    self.fail(U_INDEX_OUTOFBOUNDS_ERROR, name_index)?;
                }
                None
            }
            ArgNumber::NotValid => {
                self.fail(U_PATTERN_SYNTAX_ERROR, name_index)?;
                None
            }
        };
        index = self.skip_white(index);
        if index == self.len() {
            self.fail(U_UNMATCHED_BRACES, 0)?;
        }
        let mut kind = Kind::None;
        let c = self.msg[index];
        if c == RBRACE {
            // all done
        } else if c != COMMA {
            self.fail(U_PATTERN_SYNTAX_ERROR, name_index)?;
        } else {
            let type_index = self.skip_white(index + 1);
            index = type_index;
            while index < self.len() && (self.msg[index] as u8 as u16 == self.msg[index]) && (self.msg[index] as u8).is_ascii_alphabetic() {
                index += 1;
            }
            let length = index - type_index;
            index = self.skip_white(index);
            if index == self.len() {
                self.fail(U_UNMATCHED_BRACES, 0)?;
            }
            let c = self.msg[index];
            if length == 0 || (c != COMMA && c != RBRACE) {
                self.fail(U_PATTERN_SYNTAX_ERROR, name_index)?;
            }
            if length > MAX_LENGTH {
                self.fail(U_INDEX_OUTOFBOUNDS_ERROR, name_index)?;
            }
            let ty = self.text(type_index, type_index + length);
            let lower = ty.to_ascii_lowercase();
            let complex = match lower.as_str() {
                "choice" => Some(Parent::Choice),
                "plural" => Some(Parent::Plural),
                "select" => Some(Parent::Select),
                "selectordinal" => Some(Parent::SelectOrdinal),
                _ => None,
            };
            if c == RBRACE {
                if complex.is_some() {
                    self.fail(U_PATTERN_SYNTAX_ERROR, name_index)?;
                }
                kind = Kind::Simple { ty, style: None, slot: usize::MAX };
            } else {
                index += 1;
                match complex {
                    None => {
                        let (style, next) = self.parse_simple_style(index)?;
                        index = next;
                        kind = Kind::Simple { ty, style: Some(style), slot: usize::MAX };
                    }
                    Some(Parent::Choice) => {
                        let (cases, next) = self.parse_choice_style(index, nesting)?;
                        index = next;
                        kind = Kind::Choice(cases);
                    }
                    Some(p) => {
                        let (k, next) = self.parse_plural_or_select_style(p, index, nesting)?;
                        index = next;
                        kind = k;
                    }
                }
            }
        }
        Ok((Arg { name, number, kind }, index + 1))
    }

    fn parse_simple_style(&mut self, start: usize) -> PResult<(String, usize)> {
        let mut index = start;
        let mut nested = 0usize;
        while index < self.len() {
            let c = self.msg[index];
            index += 1;
            if c == APOS {
                match self.msg[index..].iter().position(|&u| u == APOS) {
                    Some(p) => index += p + 1,
                    None => {
                        self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
                    }
                }
            } else if c == LBRACE {
                nested += 1;
            } else if c == RBRACE {
                if nested > 0 {
                    nested -= 1;
                } else {
                    index -= 1;
                    if index - start > MAX_LENGTH {
                        self.fail(U_INDEX_OUTOFBOUNDS_ERROR, start)?;
                    }
                    return Ok((self.text(start, index), index));
                }
            }
        }
        self.fail(U_UNMATCHED_BRACES, 0)?;
        unreachable!()
    }

    fn parse_choice_style(&mut self, start: usize, nesting: usize) -> PResult<(Vec<ChoiceCase>, usize)> {
        let mut index = self.skip_white(start);
        let mut cases = Vec::new();
        if index == self.len() || self.msg[index] == RBRACE {
            self.fail(U_PATTERN_SYNTAX_ERROR, 0)?;
        }
        loop {
            let number_index = index;
            index = self.skip_double(index);
            let length = index - number_index;
            if length == 0 {
                self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
            }
            if length > MAX_LENGTH {
                self.fail(U_INDEX_OUTOFBOUNDS_ERROR, number_index)?;
            }
            let limit = self.parse_double(number_index, index, true)?;
            index = self.skip_white(index);
            if index == self.len() {
                self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
            }
            let c = self.msg[index];
            if !(c == POUND || c == b'<' as u16 || c == 0x2264) {
                self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
            }
            let (message, next) = self.parse_message(index + 1, 0, nesting + 1, Parent::Choice)?;
            let raw = self.text(index + 1, next);
            cases.push(ChoiceCase { limit, less: c == b'<' as u16, message, raw });
            index = next;
            if index == self.len() {
                return Ok((cases, index));
            }
            if self.msg[index] == RBRACE {
                return Ok((cases, index));
            }
            index = self.skip_white(index + 1);
        }
    }

    fn parse_plural_or_select_style(&mut self, ty: Parent, start: usize, nesting: usize) -> PResult<(Kind, usize)> {
        let mut index = start;
        let mut is_empty = true;
        let mut has_other = false;
        let mut offset = 0.0;
        let mut plural_cases: Vec<(Selector, Message)> = Vec::new();
        let mut select_cases: Vec<(String, Message)> = Vec::new();
        loop {
            index = self.skip_white(index);
            let eos = index == self.len();
            if eos || self.msg[index] == RBRACE {
                // inMessageFormatPattern() is always true here
                if eos {
                    self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
                }
                if !has_other {
                    self.fail(U_DEFAULT_KEYWORD_MISSING, 0)?;
                }
                let kind = match ty {
                    Parent::Select => Kind::Select(select_cases),
                    _ => Kind::Plural { ordinal: ty == Parent::SelectOrdinal, offset, cases: plural_cases },
                };
                return Ok((kind, index));
            }
            let selector_index = index;
            let selector = if ty.has_plural_style() && self.msg[selector_index] == b'=' as u16 {
                index = self.skip_double(index + 1);
                let length = index - selector_index;
                if length == 1 {
                    self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
                }
                if length > MAX_LENGTH {
                    self.fail(U_INDEX_OUTOFBOUNDS_ERROR, selector_index)?;
                }
                let v = self.parse_double(selector_index + 1, index, false)?;
                Selector::Explicit(v)
            } else {
                index = self.skip_identifier(index);
                let length = index - selector_index;
                if length == 0 {
                    self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
                }
                let word = self.text(selector_index, index);
                if ty.has_plural_style() && length == 6 && index < self.len() && word == "offset" && self.msg[index] == b':' as u16 {
                    if !is_empty {
                        self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
                    }
                    let value_index = self.skip_white(index + 1);
                    index = self.skip_double(value_index);
                    if index == value_index {
                        self.fail(U_PATTERN_SYNTAX_ERROR, start)?;
                    }
                    if index - value_index > MAX_LENGTH {
                        self.fail(U_INDEX_OUTOFBOUNDS_ERROR, value_index)?;
                    }
                    offset = self.parse_double(value_index, index, false)?;
                    is_empty = false;
                    continue;
                }
                if length > MAX_LENGTH {
                    self.fail(U_INDEX_OUTOFBOUNDS_ERROR, selector_index)?;
                }
                if word == "other" {
                    has_other = true;
                }
                Selector::Keyword(word)
            };
            index = self.skip_white(index);
            if index == self.len() || self.msg[index] != LBRACE {
                self.fail(U_PATTERN_SYNTAX_ERROR, selector_index)?;
            }
            let (message, next) = self.parse_message(index, 1, nesting + 1, ty)?;
            index = next;
            match (ty, selector) {
                (Parent::Select, Selector::Keyword(w)) => select_cases.push((w, message)),
                (_, s) => plural_cases.push((s, message)),
            }
            is_empty = false;
        }
    }
}

enum ArgNumber {
    Number(u32),
    NotNumber,
    NotValid,
}

/// `parseArgNumber`: a number without a leading zero, a name, or
/// neither.
fn parse_arg_number(s: &[u16]) -> ArgNumber {
    if s.is_empty() {
        return ArgNumber::NotValid;
    }
    let mut i = 0;
    let c = s[i];
    i += 1;
    let (mut number, mut bad): (i64, bool);
    if c == b'0' as u16 {
        if i == s.len() {
            return ArgNumber::Number(0);
        }
        number = 0;
        bad = true;
    } else if (b'1' as u16..=b'9' as u16).contains(&c) {
        number = i64::from(c - b'0' as u16);
        bad = false;
    } else {
        return ArgNumber::NotNumber;
    }
    while i < s.len() {
        let c = s[i];
        i += 1;
        if (b'0' as u16..=b'9' as u16).contains(&c) {
            if number >= i64::from(i32::MAX) / 10 {
                bad = true;
            }
            number = number * 10 + i64::from(c - b'0' as u16);
        } else {
            return ArgNumber::NotNumber;
        }
    }
    if bad {
        ArgNumber::NotValid
    } else {
        ArgNumber::Number(number as u32)
    }
}

/// C's `strtod` over the whole text, or `None` when it stops early.
fn strtod_exact(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut ndigits = i - digits_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        ndigits += i - f;
    }
    if ndigits == 0 {
        return None;
    }
    let mantissa_end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let e = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > e {
            i = j;
        }
    }
    if i != b.len() {
        return None;
    }
    let _ = mantissa_end;
    let t = text.strip_prefix('+').unwrap_or(text);
    let t = if t.starts_with('.') || t.starts_with("-.") { t.replacen('.', "0.", 1) } else { t.to_string() };
    let t = if t.ends_with('.') { format!("{t}0") } else { t };
    let t = t.replace(".e", ".0e").replace(".E", ".0E");
    t.parse::<f64>().ok()
}

/// Parse a whole MessageFormat pattern.
pub fn parse(pattern: &str) -> Result<Message, PatternError> {
    let msg: Vec<u16> = pattern.encode_utf16().collect();
    let mut p = Parser { msg: &msg, err: None };
    match p.parse_message(0, 0, 0, Parent::None) {
        Ok((m, _)) => Ok(m),
        Err(()) => Err(p.err.unwrap_or(PatternError { code: U_PATTERN_SYNTAX_ERROR, offset: 0, pre: String::new(), post: String::new() })),
    }
}
