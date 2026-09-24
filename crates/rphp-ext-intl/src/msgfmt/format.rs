//! `MessageFormat::format()` and `parse()` over a compiled pattern.

use rphp_value::Value;

use super::pattern::{ChoiceCase, Item, Kind, Message, Selector};
use super::plurals::{self, Operands};
use super::{Compiled, Fmt, Sub, U_MESSAGE_PARSE_ERROR};
use crate::datefmt::MsgDate;
use crate::numfmt::{MsgNumber, MsgStyle};
use crate::state::{U_ARGUMENT_TYPE_MISMATCH, U_ILLEGAL_ARGUMENT_ERROR};

/// `U_INVALID_FORMAT_ERROR`.
const U_INVALID_FORMAT_ERROR: i64 = 3;

/// The plural sub-message being formatted: `number - offset` and its text
/// in the default number format (`#`).
struct PluralCtx {
    text: String,
}

/// What one `format()` call builds on demand.
struct Run<'a> {
    c: &'a Compiled,
    locale: &'a str,
    values: &'a [(String, Fmt)],
    default_number: Option<MsgNumber>,
    default_date: Option<MsgDate>,
}

impl<'a> Run<'a> {
    fn value(&self, name: &str) -> Option<&'a Fmt> {
        self.values.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    fn default_number(&mut self) -> Result<&MsgNumber, i64> {
        if self.default_number.is_none() {
            self.default_number = Some(MsgNumber::new(self.locale, MsgStyle::Decimal)?);
        }
        Ok(self.default_number.as_ref().expect("built"))
    }

    fn default_date(&mut self) -> Result<&MsgDate, i64> {
        if self.default_date.is_none() {
            // DateFormat::createDateTimeInstance(kShort, kShort)
            let zone = crate::datefmt::ZoneRef::utc();
            self.default_date = Some(MsgDate::new(self.locale, 3, 3, None, zone)?);
        }
        Ok(self.default_date.as_ref().expect("built"))
    }

    fn format_number(&mut self, v: &Fmt) -> Result<String, i64> {
        let n = self.default_number()?;
        let r = match v {
            Fmt::Int(i) => n.format(&Value::Int(*i)),
            Fmt::Double(d) => n.format(&Value::Float(*d)),
            _ => None,
        };
        r.ok_or(U_ILLEGAL_ARGUMENT_ERROR)
    }

    fn message(&mut self, m: &Message, plural: Option<&PluralCtx>, out: &mut String) -> Result<(), i64> {
        for item in &m.items {
            match item {
                Item::Text(t) => out.push_str(t),
                Item::Pound => match plural {
                    Some(p) => out.push_str(&p.text),
                    None => out.push('#'),
                },
                Item::Arg(arg) => {
                    let Some(v) = self.value(&arg.name) else {
                        out.push('{');
                        out.push_str(&arg.name);
                        out.push('}');
                        continue;
                    };
                    match &arg.kind {
                        Kind::None => match v {
                            Fmt::Str(s) => out.push_str(s),
                            Fmt::Date(ms) => {
                                let ms = *ms;
                                let s = self.default_date()?.format_millis(ms).ok_or(U_ILLEGAL_ARGUMENT_ERROR)?;
                                out.push_str(&s);
                            }
                            other => {
                                let s = self.format_number(other)?;
                                out.push_str(&s);
                            }
                        },
                        Kind::Simple { slot, .. } => {
                            let s = self.simple(*slot, v)?;
                            out.push_str(&s);
                        }
                        Kind::Choice(cases) => {
                            if !v.is_numeric() {
                                return Err(U_ILLEGAL_ARGUMENT_ERROR);
                            }
                            let m = choose(cases, v.as_f64());
                            self.message(m, None, out)?;
                        }
                        Kind::Plural { ordinal, offset, cases } => {
                            if !v.is_numeric() {
                                return Err(U_ILLEGAL_ARGUMENT_ERROR);
                            }
                            let n = v.as_f64();
                            let (sub, ctx) = self.plural(&arg.name, *ordinal, *offset, cases, n)?;
                            if let Some(m) = sub {
                                self.message(m, Some(&ctx), out)?;
                            }
                        }
                        Kind::Select(cases) => {
                            let Fmt::Str(key) = v else {
                                return Err(U_ILLEGAL_ARGUMENT_ERROR);
                            };
                            let mut chosen: Option<&Message> = None;
                            for (k, m) in cases {
                                if k == key {
                                    chosen = Some(m);
                                    break;
                                }
                                if chosen.is_none() && k == "other" {
                                    chosen = Some(m);
                                }
                            }
                            // the first `other` unless a key matched
                            let exact = cases.iter().find(|(k, _)| k == key).map(|(_, m)| m);
                            if let Some(m) = exact.or(chosen) {
                                self.message(m, None, out)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn simple(&mut self, slot: usize, v: &Fmt) -> Result<String, i64> {
        let sub = self.c.subs.get(slot).ok_or(U_ILLEGAL_ARGUMENT_ERROR)?;
        match sub {
            Sub::Number(n) => {
                let r = match v {
                    Fmt::Int(i) => n.format(&Value::Int(*i)),
                    Fmt::Double(d) => n.format(&Value::Float(*d)),
                    _ => return Err(U_ILLEGAL_ARGUMENT_ERROR),
                };
                r.ok_or(U_ILLEGAL_ARGUMENT_ERROR)
            }
            Sub::Date(d) => match v {
                Fmt::Str(_) => Err(U_ILLEGAL_ARGUMENT_ERROR),
                other => d.format_millis(other.as_f64()).ok_or(U_INVALID_FORMAT_ERROR),
            },
            Sub::Rbnf(r) => match v {
                Fmt::Int(i) => Ok(r.format_i64(*i)),
                Fmt::Double(d) => Ok(r.format_f64(*d)),
                _ => Err(U_ILLEGAL_ARGUMENT_ERROR),
            },
        }
    }

    /// `PluralFormat::findSubMessage` with MessageFormat's selector
    /// context.
    fn plural<'m>(&mut self, name: &str, ordinal: bool, offset: f64, cases: &'m [(Selector, Message)], n: f64) -> Result<(Option<&'m Message>, PluralCtx), i64> {
        let number = n - offset;
        let (text, operands) = self.select_operands(name, cases, number)?;
        let ctx = PluralCtx { text };
        let mut keyword: Option<&'static str> = None;
        let mut have_keyword_match = false;
        let mut msg_start: Option<&'m Message> = None;
        for (sel, m) in cases {
            match sel {
                Selector::Explicit(v) => {
                    if n == *v {
                        return Ok((Some(m), ctx));
                    }
                }
                Selector::Keyword(k) => {
                    if have_keyword_match {
                        continue;
                    }
                    if k == "other" {
                        if msg_start.is_none() {
                            msg_start = Some(m);
                            if keyword == Some("other") {
                                have_keyword_match = true;
                            }
                        }
                    } else {
                        if keyword.is_none() {
                            let kw = plurals::select(self.locale, ordinal, &operands);
                            keyword = Some(kw);
                            if msg_start.is_some() && kw == "other" {
                                have_keyword_match = true;
                            }
                        }
                        if !have_keyword_match && Some(k.as_str()) == keyword {
                            msg_start = Some(m);
                            have_keyword_match = true;
                        }
                    }
                }
            }
        }
        Ok((msg_start, ctx))
    }

    /// The text of `number` in the default format (for `#`) and the
    /// operands the selection reads: those of a `{name, number, …}` the
    /// `other` sub-message starts with, else of the default format.
    fn select_operands(&mut self, name: &str, cases: &[(Selector, Message)], number: f64) -> Result<(String, Operands), i64> {
        let (text, shown) = self.default_number()?.format_operands(number).ok_or(U_ILLEGAL_ARGUMENT_ERROR)?;
        let mut ops = shown.map_or_else(|| Operands::from_f64(number), |t| Operands::from_text(&t));
        let other = cases.iter().find(|(s, _)| matches!(s, Selector::Keyword(k) if k == "other")).map(|(_, m)| m);
        if let Some(other) = other {
            for item in &other.items {
                match item {
                    Item::Pound => break,
                    Item::Arg(a) => {
                        if a.name == name {
                            if let Kind::Simple { slot, .. } = &a.kind {
                                match self.c.subs.get(*slot) {
                                    Some(Sub::Number(nf)) => {
                                        if let Some((_, Some(t))) = nf.format_operands(number) {
                                            ops = Operands::from_text(&t);
                                        }
                                    }
                                    Some(_) => ops = Operands::from_f64(number),
                                    None => {}
                                }
                            }
                            break;
                        }
                    }
                    Item::Text(_) => {}
                }
            }
        }
        Ok((text, ops))
    }
}

/// `ChoiceFormat::findSubMessage` (the negated comparisons catch NaN).
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn choose(cases: &[ChoiceCase], number: f64) -> &Message {
    let mut chosen = &cases[0].message;
    for c in &cases[1..] {
        let inside = if c.less { !(number > c.limit) } else { !(number >= c.limit) };
        if inside {
            break;
        }
        chosen = &c.message;
    }
    chosen
}

/// Format a compiled pattern with its converted arguments.
pub(super) fn format(c: &Compiled, locale: &str, values: &[(String, Fmt)]) -> Result<String, i64> {
    let mut run = Run { c, locale, values, default_number: None, default_date: None };
    let mut out = String::new();
    run.message(&c.root, None, &mut out)?;
    Ok(out)
}

// ---- parse -------------------------------------------------------------------------------------

/// `MessageFormat::parse(source, count, status)`: the top-level message
/// matched against the text, one value per argument number.
pub(super) fn parse(c: &Compiled, source: &str) -> Result<Vec<Value>, i64> {
    if c.named {
        return Err(U_ARGUMENT_TYPE_MISMATCH);
    }
    let src: Vec<char> = source.chars().collect();
    let mut results: Vec<Option<Value>> = Vec::new();
    let mut pos = 0usize;
    let items = &c.root.items;
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Text(t) => {
                let t: Vec<char> = t.chars().collect();
                if src.len() < pos + t.len() || src[pos..pos + t.len()] != t[..] {
                    return Err(U_MESSAGE_PARSE_ERROR);
                }
                pos += t.len();
            }
            Item::Pound => return Err(U_MESSAGE_PARSE_ERROR),
            Item::Arg(arg) => {
                let number = arg.number.unwrap_or(0) as usize;
                let value: Option<Value> = match &arg.kind {
                    Kind::Simple { slot, .. } => {
                        let (v, end) = parse_simple(c, *slot, &src, pos).ok_or(U_MESSAGE_PARSE_ERROR)?;
                        if end == pos {
                            return Err(U_MESSAGE_PARSE_ERROR);
                        }
                        pos = end;
                        Some(v)
                    }
                    Kind::None => {
                        // up to the literal text that follows, or the end
                        let after: Vec<char> = match items.get(i + 1) {
                            Some(Item::Text(t)) => t.chars().collect(),
                            _ => Vec::new(),
                        };
                        let next = if after.is_empty() {
                            Some(src.len())
                        } else {
                            (pos..=src.len().saturating_sub(after.len())).find(|&k| src[k..k + after.len()] == after[..])
                        };
                        let Some(next) = next else {
                            return Err(U_MESSAGE_PARSE_ERROR);
                        };
                        let s: String = src[pos..next].iter().collect();
                        pos = next;
                        if s == format!("{{{number}}}") {
                            None
                        } else {
                            Some(Value::string(s.as_bytes()))
                        }
                    }
                    Kind::Choice(cases) => {
                        let (v, end) = parse_choice(cases, &src, pos);
                        if end == pos {
                            return Err(U_MESSAGE_PARSE_ERROR);
                        }
                        pos = end;
                        Some(Value::Float(v))
                    }
                    // ICU cannot parse these; the public parse() reports it as a parse error
                    Kind::Plural { .. } | Kind::Select(_) => return Err(U_MESSAGE_PARSE_ERROR),
                };
                if let Some(v) = value {
                    if results.len() <= number {
                        results.resize(number + 1, None);
                    }
                    results[number] = Some(v);
                }
            }
        }
    }
    if pos == 0 {
        return Err(U_MESSAGE_PARSE_ERROR);
    }
    Ok(results.into_iter().map(|v| v.unwrap_or(Value::Int(0))).collect())
}

fn parse_simple(c: &Compiled, slot: usize, src: &[char], pos: usize) -> Option<(Value, usize)> {
    let text: String = src.iter().collect();
    match c.subs.get(slot)? {
        Sub::Number(n) => n.parse_at(&text, pos),
        Sub::Date(d) => d.parse_at(&text, pos).map(|(ts, end)| (Value::Float(ts as f64), end)),
        Sub::Rbnf(r) => r.parse_at(&text, pos),
    }
}

/// `ChoiceFormat::parseArgument`: the limit of the case whose text
/// matches the longest.
fn parse_choice(cases: &[ChoiceCase], src: &[char], start: usize) -> (f64, usize) {
    let mut furthest = start;
    let mut best = f64::NAN;
    for c in cases {
        let raw: Vec<char> = c.raw.chars().collect();
        if src.len() >= start + raw.len() && src[start..start + raw.len()] == raw[..] {
            let end = start + raw.len();
            if end > furthest {
                furthest = end;
                best = c.limit;
                if furthest == src.len() {
                    break;
                }
            }
        }
    }
    (best, furthest)
}
