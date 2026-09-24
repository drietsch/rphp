//! ICU's transliterator IDs (`TransliteratorIDParser`): a compound ID is an
//! optional global filter, then single IDs separated by `;`, each
//! `[filter] Source-Target/Variant` with an optional `(inverse)`; the
//! canonical ID the object's `$id` shows is assembled from the pieces as
//! written, and the reverse direction swaps each piece (or takes its
//! registered special inverse: `Lower` ↔ `Upper`, `NFC` ↔ `NFD`, a script
//! target → `Null`, …).

use icu::properties::props::{GeneralCategory, PatternWhiteSpace};
use icu::properties::{CodePointMapData, CodePointSetData};

/// One element of a parsed compound ID.
#[derive(Clone, Debug)]
pub struct Single {
    /// The element's share of the canonical ID.
    pub canon: String,
    /// `Source-Target/Variant` to instantiate; empty for `()` pieces.
    pub basic: String,
    /// The element's filter pattern, as written.
    pub filter: Option<String>,
}

/// A parsed compound ID.
#[derive(Debug)]
pub struct Compound {
    pub canon: String,
    pub global_filter: Option<String>,
    pub list: Vec<Single>,
}

struct Specs {
    source: String,
    target: String,
    variant: String,
    saw_source: bool,
    filter: String,
}

fn is_ws(c: char) -> bool {
    CodePointSetData::new::<PatternWhiteSpace>().contains(c)
}

/// `u_isIDStart`: a letter or a letter number.
fn is_id_start(c: char) -> bool {
    use GeneralCategory as G;
    matches!(
        CodePointMapData::<GeneralCategory>::new().get(c),
        G::UppercaseLetter | G::LowercaseLetter | G::TitlecaseLetter | G::ModifierLetter | G::OtherLetter | G::LetterNumber
    )
}

/// `u_isIDPart`: letters, marks, digits, connectors and the ignorables.
fn is_id_part(c: char) -> bool {
    use GeneralCategory as G;
    let u = c as u32;
    if u <= 8 || (0xe..=0x1b).contains(&u) || (0x7f..=0x9f).contains(&u) {
        return true;
    }
    matches!(
        CodePointMapData::<GeneralCategory>::new().get(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
            | G::LetterNumber
            | G::NonspacingMark
            | G::SpacingMark
            | G::DecimalNumber
            | G::ConnectorPunctuation
            | G::Format
    )
}

struct Cursor<'a> {
    s: &'a [char],
    pos: usize,
}

impl Cursor<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.s.len() && is_ws(self.s[self.pos]) {
            self.pos += 1;
        }
    }

    /// `ICU_Utility::parseChar`: whitespace, then `c`.
    fn parse_char(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.s.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn text(&self, from: usize, to: usize) -> String {
        self.s[from..to].iter().collect()
    }

    /// `UnicodeSet::resemblesPattern`.
    fn resembles_pattern(&self) -> bool {
        let s = self.s;
        let p = self.pos;
        if p + 1 < s.len() && s[p] == '[' {
            return true;
        }
        p + 5 <= s.len() && s[p] == '\\' && matches!(s[p + 1], 'p' | 'P' | 'N')
    }

    /// The end of the set pattern at the cursor (the parser consumes the
    /// whitespace after it), or `None` when it does not close.
    fn scan_set(&self) -> Option<usize> {
        let s = self.s;
        let mut p = self.pos;
        if s[p] == '\\' {
            p += 2;
            if s.get(p) == Some(&'{') {
                while p < s.len() && s[p] != '}' {
                    p += 1;
                }
                if p == s.len() {
                    return None;
                }
            }
            p += 1;
        } else {
            let mut depth = 0usize;
            loop {
                let c = *s.get(p)?;
                match c {
                    '\\' => p += 1,
                    '\'' => {
                        p += 1;
                        while *s.get(p)? != '\'' {
                            p += 1;
                        }
                    }
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            p += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                p += 1;
            }
        }
        while p < s.len() && is_ws(s[p]) {
            p += 1;
        }
        Some(p)
    }

    /// `ICU_Utility::parseUnicodeIdentifier`.
    fn identifier(&mut self) -> String {
        let start = self.pos;
        let mut p = start;
        while p < self.s.len() {
            let c = self.s[p];
            if p == start {
                if !is_id_start(c) {
                    return String::new();
                }
            } else if !is_id_part(c) {
                break;
            }
            p += 1;
        }
        self.pos = p;
        self.text(start, p)
    }

    /// `parseFilterID`.
    fn filter_id(&mut self, allow_filter: bool) -> Option<Specs> {
        let start = self.pos;
        let (mut first, mut target, mut variant, mut filter) = (String::new(), String::new(), String::new(), String::new());
        let mut delimiter: Option<char> = None;
        let mut spec_count = 0;
        loop {
            self.skip_ws();
            if self.pos == self.s.len() {
                break;
            }
            if allow_filter && filter.is_empty() && self.resembles_pattern() {
                let Some(end) = self.scan_set() else {
                    self.pos = start;
                    return None;
                };
                filter = self.text(self.pos, end);
                self.pos = end;
                continue;
            }
            if delimiter.is_none() {
                let c = self.s[self.pos];
                if (c == '-' && target.is_empty()) || (c == '/' && variant.is_empty()) {
                    delimiter = Some(c);
                    self.pos += 1;
                    continue;
                }
            }
            if delimiter.is_none() && spec_count > 0 {
                break;
            }
            let spec = self.identifier();
            if spec.is_empty() {
                break;
            }
            match delimiter {
                None => first = spec,
                Some('-') => target = spec,
                _ => variant = spec,
            }
            spec_count += 1;
            delimiter = None;
        }
        let mut source = String::new();
        if !first.is_empty() {
            if target.is_empty() {
                target = first;
            } else {
                source = first;
            }
        }
        if source.is_empty() && target.is_empty() {
            self.pos = start;
            return None;
        }
        let saw_source = !source.is_empty();
        if source.is_empty() {
            source = "Any".into();
        }
        if target.is_empty() {
            target = "Any".into();
        }
        Some(Specs { source, target, variant, saw_source, filter })
    }

    /// `parseSingleID`.
    fn single_id(&mut self, reverse: bool) -> Option<Single> {
        let start = self.pos;
        let mut a: Option<Specs> = None;
        let mut b: Option<Specs> = None;
        let mut saw_paren = false;
        for pass in 1..=2 {
            if pass == 2 {
                a = self.filter_id(true);
                if a.is_none() {
                    self.pos = start;
                    return None;
                }
            }
            if self.parse_char('(') {
                saw_paren = true;
                if !self.parse_char(')') {
                    b = self.filter_id(true);
                    if b.is_none() || !self.parse_char(')') {
                        self.pos = start;
                        return None;
                    }
                }
                break;
            }
        }
        if saw_paren {
            let (outer, inner) = if reverse { (&b, &a) } else { (&a, &b) };
            let mut single = specs_to_id(outer.as_ref(), false);
            let other = specs_to_id(inner.as_ref(), false);
            single.canon.push('(');
            single.canon.push_str(&other.canon);
            single.canon.push(')');
            single.filter = outer.as_ref().and_then(|s| (!s.filter.is_empty()).then(|| s.filter.clone()));
            return Some(single);
        }
        let a = a?;
        let mut single = if reverse {
            special_inverse(&a).unwrap_or_else(|| specs_to_id(Some(&a), true))
        } else {
            specs_to_id(Some(&a), false)
        };
        single.filter = (!a.filter.is_empty()).then(|| a.filter.clone());
        Some(single)
    }

    /// `parseGlobalFilter`: `with_parens` 0 forbids them, 1 requires them.
    fn global_filter(&mut self, with_parens: bool, reverse: bool, canon: &mut String) -> Option<String> {
        let start = self.pos;
        if with_parens && !self.parse_char('(') {
            self.pos = start;
            return None;
        }
        self.skip_ws();
        if !self.resembles_pattern() {
            // php's ICU leaves the position after the paren here; the
            // caller then fails on it.
            return None;
        }
        let Some(end) = self.scan_set() else {
            self.pos = start;
            return None;
        };
        let mut pattern = self.text(self.pos, end);
        self.pos = end;
        let filter = pattern.clone();
        if with_parens && !self.parse_char(')') {
            self.pos = start;
            return None;
        }
        if reverse != with_parens {
            pattern = format!("({pattern})");
        }
        pattern.push(';');
        if reverse {
            canon.insert_str(0, &pattern);
        } else {
            canon.push_str(&pattern);
        }
        Some(filter)
    }
}

/// `specsToID`.
fn specs_to_id(specs: Option<&Specs>, reverse: bool) -> Single {
    let Some(s) = specs else {
        return Single { canon: String::new(), basic: String::new(), filter: None };
    };
    let mut buf = String::new();
    let mut prefix = String::new();
    if reverse {
        buf.push_str(&s.target);
        buf.push('-');
        buf.push_str(&s.source);
    } else {
        if s.saw_source {
            buf.push_str(&s.source);
            buf.push('-');
        } else {
            prefix = format!("{}-", s.source);
        }
        buf.push_str(&s.target);
    }
    if !s.variant.is_empty() {
        buf.push('/');
        buf.push_str(&s.variant);
    }
    let basic = format!("{prefix}{buf}");
    Single { canon: format!("{}{buf}", s.filter), basic, filter: None }
}

/// The special inverses ICU registers: the case, normalization and null
/// transliterators, and `Null` for every script `Any-X` targets.
fn special_inverse_of(target: &str) -> Option<&'static str> {
    let t = target.to_ascii_lowercase();
    Some(match t.as_str() {
        "null" | "remove" | "any" => "Null",
        "upper" | "title" => "Lower",
        "lower" => "Upper",
        "nfc" | "fcc" => "NFD",
        "nfd" => "NFC",
        "nfkc" => "NFKD",
        "nfkd" => "NFKC",
        "fcd" => "FCD",
        _ => {
            if super::any_targets().iter().any(|a| a.eq_ignore_ascii_case(&t)) {
                "Null"
            } else {
                return None;
            }
        }
    })
}

/// `specsToSpecialInverse`.
fn special_inverse(s: &Specs) -> Option<Single> {
    if !s.source.eq_ignore_ascii_case("any") {
        return None;
    }
    let inv = special_inverse_of(&s.target)?;
    let mut canon = s.filter.clone();
    if s.saw_source {
        canon.push_str("Any-");
    }
    canon.push_str(inv);
    let mut basic = format!("Any-{inv}");
    if !s.variant.is_empty() {
        canon.push('/');
        canon.push_str(&s.variant);
        basic.push('/');
        basic.push_str(&s.variant);
    }
    Some(Single { canon, basic, filter: None })
}

/// `parseCompoundID`: `None` is ICU's `U_INVALID_ID`.
pub fn parse_compound(id: &str, reverse: bool) -> Option<Compound> {
    let chars: Vec<char> = id.chars().collect();
    let mut c = Cursor { s: &chars, pos: 0 };
    let mut canon = String::new();
    let mut global_filter = None;
    if let Some(f) = c.global_filter(false, reverse, &mut canon) {
        if !c.parse_char(';') {
            canon.clear();
            c.pos = 0;
        } else if !reverse {
            global_filter = Some(f);
        }
    } else {
        c.pos = 0;
        canon.clear();
    }
    let mut list: Vec<Single> = Vec::new();
    let mut saw_delimiter = true;
    while let Some(single) = c.single_id(reverse) {
        if reverse {
            list.insert(0, single);
        } else {
            list.push(single);
        }
        if !c.parse_char(';') {
            saw_delimiter = false;
            break;
        }
    }
    if list.is_empty() {
        return None;
    }
    let joined: Vec<&str> = list.iter().map(|s| s.canon.as_str()).collect();
    let joined = joined.join(";");
    canon.push_str(&joined);
    if saw_delimiter {
        if let Some(f) = c.global_filter(true, reverse, &mut canon) {
            c.parse_char(';');
            if reverse {
                global_filter = Some(f);
            }
        }
    }
    c.skip_ws();
    if c.pos != chars.len() {
        return None;
    }
    Some(Compound { canon, global_filter, list })
}

/// A basic ID split into source, target and variant (`STVtoID`'s parts).
pub fn split_basic(basic: &str) -> (String, String, String) {
    let (st, variant) = match basic.split_once('/') {
        Some((a, b)) => (a, b.to_string()),
        None => (basic, String::new()),
    };
    let (source, target) = match st.split_once('-') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => ("Any".to_string(), st.to_string()),
    };
    (source, target, variant)
}
