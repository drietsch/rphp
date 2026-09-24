//! The transliterators ICU implements in code rather than rules, plugged
//! into ICU4X's rule engine as custom transliterators: `Any-Title`
//! (`TitlecaseTransliterator`), `Any-Hex` / `Hex-Any` and their variants
//! (`EscapeTransliterator` / `UnescapeTransliterator`), `Any-Name` /
//! `Name-Any`, and `AnyTransliterator` — `Any-Latin` and every other
//! `Any-<script>`: the text split into script runs, each run handed to
//! `<Script>-<target>` (or `<Script>-Latin;Latin-<target>`).

use std::cell::RefCell;
use std::fmt::Write as _;
use std::ops::Range;

use icu::casemap::options::{LeadingAdjustment, TitlecaseOptions};
use icu::casemap::CaseMapperBorrowed;
use icu::locale::LanguageIdentifier;
use icu::properties::props::{CaseIgnorable, Cased, Script};
use icu::properties::{CodePointMapData, CodePointSetData, PropertyNamesShort};
use icu_experimental::transliterate::CustomTransliterator;

use super::Compiled;

/// `ucase_getTypeOrIgnorable() > 0`: cased or case-ignorable.
fn cased_or_ignorable(c: char) -> bool {
    CodePointSetData::new::<Cased>().contains(c) || CodePointSetData::new::<CaseIgnorable>().contains(c)
}

fn cased(c: char) -> bool {
    CodePointSetData::new::<Cased>().contains(c)
}

fn ignorable(c: char) -> bool {
    CodePointSetData::new::<CaseIgnorable>().contains(c)
}

/// `Any-Title`: every character is titlecased after an uncased,
/// non-ignorable one (or the start), lowercased otherwise.
#[derive(Debug)]
pub struct Title;

impl CustomTransliterator for Title {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        let cm = CaseMapperBorrowed::new();
        let root = LanguageIdentifier::UNKNOWN;
        let mut do_title = !input[..range.start].chars().next_back().is_some_and(cased_or_ignorable);
        let chars: Vec<(usize, char)> = input.char_indices().collect();
        let mut out = String::new();
        for (i, &(at, c)) in chars.iter().enumerate() {
            if at < range.start || at >= range.end {
                continue;
            }
            let s = c.to_string();
            if do_title {
                let mut o = TitlecaseOptions::default();
                o.leading_adjustment = Some(LeadingAdjustment::None);
                out.push_str(&cm.titlecase_segment_with_only_case_data_to_string(&s, &root, o));
            } else if c == 'Σ' {
                // Final_Sigma: a cased letter before, none after (both
                // across case-ignorables).
                let before = chars[..i].iter().rev().map(|x| x.1).find(|&x| !ignorable(x)).is_some_and(cased);
                let after = chars[i + 1..].iter().map(|x| x.1).find(|&x| !ignorable(x)).is_some_and(cased);
                out.push(if before && !after { 'ς' } else { 'σ' });
            } else {
                out.push_str(&cm.lowercase_to_string(&s, &root));
            }
            do_title = !cased_or_ignorable(c);
        }
        out
    }
}

/// An `EscapeTransliterator` form: prefix, suffix, radix, minimum digits,
/// whether a supplementary code point is one escape (else two UTF-16
/// escapes), and the prefix/digits for supplementaries (`\U` in C).
#[derive(Debug, Clone, Copy)]
pub struct HexForm {
    prefix: &'static str,
    suffix: &'static str,
    radix: u32,
    min: usize,
    whole: bool,
    supp: Option<(&'static str, usize)>,
}

const UNICODE: HexForm = HexForm { prefix: "U+", suffix: "", radix: 16, min: 4, whole: true, supp: None };
const JAVA: HexForm = HexForm { prefix: "\\u", suffix: "", radix: 16, min: 4, whole: false, supp: None };
const C: HexForm = HexForm { prefix: "\\u", suffix: "", radix: 16, min: 4, whole: true, supp: Some(("\\U", 8)) };
const XML: HexForm = HexForm { prefix: "&#x", suffix: ";", radix: 16, min: 1, whole: true, supp: None };
const XML10: HexForm = HexForm { prefix: "&#", suffix: ";", radix: 10, min: 1, whole: true, supp: None };
const PERL: HexForm = HexForm { prefix: "\\x{", suffix: "}", radix: 16, min: 1, whole: true, supp: None };

/// The escape form of an `Any-Hex/<variant>` (lowercased variant).
pub fn hex_form(variant: &str) -> Option<HexForm> {
    Some(match variant {
        "" | "java" => JAVA,
        "unicode" => UNICODE,
        "c" => C,
        "xml" => XML,
        "xml10" => XML10,
        "perl" => PERL,
        _ => return None,
    })
}

fn digits(v: u32, radix: u32, min: usize) -> String {
    let s = if radix == 10 { v.to_string() } else { format!("{v:X}") };
    format!("{}{s}", "0".repeat(min.saturating_sub(s.len())))
}

/// `Any-Hex/…`.
#[derive(Debug)]
pub struct HexTo(pub HexForm);

impl CustomTransliterator for HexTo {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        let f = self.0;
        let mut out = String::new();
        for c in input[range].chars() {
            let u = c as u32;
            if u > 0xFFFF && !f.whole {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "{}{}{}", f.prefix, digits(*unit as u32, f.radix, f.min), f.suffix);
                }
            } else if let (true, Some((p, min))) = (u > 0xFFFF, f.supp) {
                let _ = write!(out, "{p}{}{}", digits(u, f.radix, min), f.suffix);
            } else {
                let _ = write!(out, "{}{}{}", f.prefix, digits(u, f.radix, f.min), f.suffix);
            }
        }
        out
    }
}

/// An `UnescapeTransliterator` pattern: prefix, suffix, radix, digit
/// count range.
pub struct Unescape {
    prefix: &'static str,
    suffix: &'static str,
    radix: u32,
    min: usize,
    max: usize,
}

const UN_UNICODE: Unescape = Unescape { prefix: "U+", suffix: "", radix: 16, min: 4, max: 6 };
const UN_JAVA: Unescape = Unescape { prefix: "\\u", suffix: "", radix: 16, min: 4, max: 4 };
const UN_C: Unescape = Unescape { prefix: "\\U", suffix: "", radix: 16, min: 8, max: 8 };
const UN_XML: Unescape = Unescape { prefix: "&#x", suffix: ";", radix: 16, min: 1, max: 6 };
const UN_XML10: Unescape = Unescape { prefix: "&#", suffix: ";", radix: 10, min: 1, max: 7 };
const UN_PERL: Unescape = Unescape { prefix: "\\x{", suffix: "}", radix: 16, min: 1, max: 6 };

/// `Hex-Any/…`: the forms a variant recognizes (lowercased variant).
pub fn unescape_forms(variant: &str) -> Option<&'static [Unescape]> {
    static ANY: [Unescape; 6] = [UN_JAVA, UN_C, UN_XML, UN_XML10, UN_PERL, UN_UNICODE];
    static JAVA_: [Unescape; 1] = [UN_JAVA];
    static C_: [Unescape; 2] = [UN_JAVA, UN_C];
    static XML_: [Unescape; 1] = [UN_XML];
    static XML10_: [Unescape; 1] = [UN_XML10];
    static PERL_: [Unescape; 1] = [UN_PERL];
    static UNICODE_: [Unescape; 1] = [UN_UNICODE];
    Some(match variant {
        "" => &ANY,
        "java" => &JAVA_,
        "c" => &C_,
        "xml" => &XML_,
        "xml10" => &XML10_,
        "perl" => &PERL_,
        "unicode" => &UNICODE_,
        _ => return None,
    })
}

/// `Hex-Any/…`.
#[derive(Debug)]
pub struct HexFrom(pub &'static [Unescape]);

impl std::fmt::Debug for Unescape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.prefix)
    }
}

/// One escape at the start of `s`: the code point and the bytes used.
fn unescape_one(forms: &[Unescape], s: &str) -> Option<(u32, usize)> {
    for f in forms {
        let Some(rest) = s.strip_prefix(f.prefix) else { continue };
        let n = rest.chars().take(f.max).take_while(|c| c.is_digit(f.radix)).count();
        if n < f.min {
            continue;
        }
        let Ok(v) = u32::from_str_radix(&rest[..n], f.radix) else { continue };
        let tail = &rest[n..];
        if !tail.starts_with(f.suffix) {
            continue;
        }
        if v > 0x10FFFF {
            continue;
        }
        return Some((v, f.prefix.len() + n + f.suffix.len()));
    }
    None
}

impl CustomTransliterator for HexFrom {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        let s = &input[range];
        let mut out = String::new();
        let mut i = 0;
        let mut pending_high: Option<u32> = None;
        while i < s.len() {
            if let Some((v, used)) = unescape_one(self.0, &s[i..]) {
                i += used;
                if (0xD800..0xDC00).contains(&v) {
                    if let Some(h) = pending_high.replace(v) {
                        out.push_str(&char::from_u32(h).map_or(String::new(), String::from));
                    }
                    continue;
                }
                if let (Some(h), true) = (pending_high, (0xDC00..0xE000).contains(&v)) {
                    pending_high = None;
                    let cp = 0x10000 + ((h - 0xD800) << 10) + (v - 0xDC00);
                    out.extend(char::from_u32(cp));
                    continue;
                }
                pending_high = None;
                out.extend(char::from_u32(v));
                continue;
            }
            pending_high = None;
            let c = s[i..].chars().next().unwrap_or('\0');
            out.push(c);
            i += c.len_utf8();
        }
        out
    }
}

/// `Any-Name`: `\N{NAME}` for every character.
#[derive(Debug)]
pub struct NameTo;

impl CustomTransliterator for NameTo {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        let mut out = String::new();
        for c in input[range].chars() {
            match unicode_names2::name(c) {
                Some(n) => {
                    let _ = write!(out, "\\N{{{n}}}");
                }
                None => out.push(c),
            }
        }
        out
    }
}

/// `Name-Any`: `\N{NAME}` back to the character.
#[derive(Debug)]
pub struct NameFrom;

impl CustomTransliterator for NameFrom {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        let s = &input[range];
        let mut out = String::new();
        let mut rest = s;
        while let Some(at) = rest.find("\\N{") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 3..];
            match after.find('}').and_then(|e| unicode_names2::character(after[..e].trim()).map(|c| (c, e))) {
                Some((c, e)) => {
                    out.push(c);
                    rest = &after[e + 1..];
                }
                None => {
                    out.push_str("\\N{");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// The identity (`Any-BreakInternal`, which leaves php's Thai unbroken).
#[derive(Debug)]
pub struct Identity;

impl CustomTransliterator for Identity {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        input[range].to_string()
    }
}

fn script_of(c: char) -> Script {
    CodePointMapData::<Script>::new().get(c)
}

fn common(s: Script) -> bool {
    s == Script::Common || s == Script::Inherited
}

/// `AnyTransliterator`: `target` is the ID's target (with its variant),
/// `script` its script (`None` when it names none).
#[derive(Debug)]
pub struct AnyScript {
    pub target: String,
    pub script: Option<Script>,
    cache: RefCell<Vec<(Script, Option<std::rc::Rc<Compiled>>)>>,
}

impl AnyScript {
    pub fn new(target: String, script: Option<Script>) -> Self {
        Self { target, script, cache: RefCell::new(Vec::new()) }
    }

    fn for_script(&self, s: Script) -> Option<std::rc::Rc<Compiled>> {
        if Some(s) == self.script {
            return None;
        }
        if let Some((_, t)) = self.cache.borrow().iter().find(|(k, _)| *k == s) {
            return t.clone();
        }
        let short = PropertyNamesShort::<Script>::new().get(s).unwrap_or("Zzzz");
        // ICU 78 no longer pivots through Latin: a script with no direct
        // transliterator to the target is left alone.
        let t = super::build_id(&format!("{short}-{}", self.target), false).map(|(_, c)| std::rc::Rc::new(c));
        self.cache.borrow_mut().push((s, t.clone()));
        t
    }
}

/// A run transliterated with the character before it as context when the
/// transliterator leaves that one alone (ICU4X runs a transliterator on a
/// whole string; ICU lets the rules see the text before the run, which
/// `Han-Latin`'s spacing looks at, and nothing past its end).
fn with_context(t: &Compiled, before: Option<char>, run: &str, after: Option<char>) -> String {
    let keeps = |c: char| t.run(c.to_string()) == c.to_string();
    let before = before.filter(|&c| keeps(c));
    let after = after.filter(|&c| keeps(c));
    if before.is_none() && after.is_none() {
        return t.run(run.to_string());
    }
    let mut s = String::new();
    s.extend(before);
    s.push_str(run);
    s.extend(after);
    let out = t.run(s);
    let mut inner = out.as_str();
    if let Some(b) = before {
        match inner.strip_prefix(b) {
            Some(r) => inner = r,
            None => return t.run(run.to_string()),
        }
    }
    if let Some(a) = after {
        match inner.strip_suffix(a) {
            Some(r) => inner = r,
            None => return t.run(run.to_string()),
        }
    }
    inner.to_string()
}

impl CustomTransliterator for AnyScript {
    fn transliterate(&self, input: &str, range: Range<usize>) -> String {
        let mut text: Vec<char> = input.chars().collect();
        let all_start = input[..range.start].chars().count();
        let mut all_limit = all_start + input[range.clone()].chars().count();
        let mut limit = 0usize;
        while limit < text.len() {
            let mut start = limit;
            while start > 0 && common(script_of(text[start - 1])) {
                start -= 1;
            }
            let mut code: Option<Script> = None;
            while limit < text.len() {
                let s = script_of(text[limit]);
                if !common(s) {
                    match code {
                        None => code = Some(s),
                        Some(c) if c != s => break,
                        _ => {}
                    }
                }
                limit += 1;
            }
            if limit <= all_start {
                continue;
            }
            let Some(t) = code.and_then(|c| self.for_script(c)) else {
                if limit >= all_limit {
                    break;
                }
                continue;
            };
            let s = start.max(all_start);
            let e = limit.min(all_limit);
            let run: String = text[s..e].iter().collect();
            let before = s.checked_sub(1).map(|i| text[i]);
            // ICU matches no rule past the run's limit: only the text before
            // it serves as context.
            let after = None;
            let out: Vec<char> = with_context(&t, before, &run, after).chars().collect();
            let delta = out.len() as isize - (e - s) as isize;
            text.splice(s..e, out);
            all_limit = (all_limit as isize + delta) as usize;
            limit = (limit as isize + delta) as usize;
            if limit >= all_limit {
                break;
            }
        }
        text[all_start..all_limit].iter().collect()
    }
}
