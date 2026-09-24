//! The boundaries of a text for each kind of break iterator, over ICU4X's
//! segmenters (UAX #29 words, sentences and grapheme clusters, UAX #14
//! lines, dictionaries for CJK and South-East Asian scripts), with the
//! rule statuses ICU's rule files attach: word types (`WORD_NUMBER`,
//! `WORD_LETTER`, `WORD_IDEO`), hard line breaks, sentences ended by a
//! separator. `createTitleInstance()`'s legacy rules are applied by hand.

use icu::locale::LanguageIdentifier;
use icu::properties::props::{CaseIgnorable, GeneralCategory, Lowercase, Script, SentenceBreak, Uppercase};
use icu::properties::{CodePointMapData, CodePointSetData};
use icu::segmenter::options::{LineBreakOptions, LineBreakStrictness, SentenceBreakInvariantOptions, SentenceBreakOptions, WordBreakInvariantOptions};
use icu::segmenter::{GraphemeClusterSegmenter, LineSegmenter, SentenceSegmenter, WordSegmenter};
use icu::segmenter::options::WordType;

use super::Kind;

/// A text as ICU's UTF-8 `UText` sees it: each ill-formed byte one
/// U+FFFD. `offsets[i]` is the original byte offset of the `i`th char,
/// with the text's length appended.
pub struct Text {
    pub s: String,
    pub chars: Vec<char>,
    pub offsets: Vec<usize>,
    /// For each byte of `s`, the original offset (valid at char starts).
    map: Vec<usize>,
}

impl Text {
    pub fn new(bytes: &[u8]) -> Text {
        let mut s = String::with_capacity(bytes.len());
        let mut chars = Vec::new();
        let mut offsets = Vec::new();
        let mut map = Vec::with_capacity(bytes.len() + 1);
        let mut i = 0;
        while i < bytes.len() {
            let (c, n) = match std::str::from_utf8(&bytes[i..(i + 4).min(bytes.len())]) {
                Ok(t) => {
                    let c = t.chars().next().unwrap_or('\u{FFFD}');
                    (c, c.len_utf8())
                }
                Err(e) if e.valid_up_to() > 0 => {
                    let t = std::str::from_utf8(&bytes[i..i + e.valid_up_to()]).unwrap_or("\u{FFFD}");
                    let c = t.chars().next().unwrap_or('\u{FFFD}');
                    (c, c.len_utf8())
                }
                Err(_) => ('\u{FFFD}', 1),
            };
            chars.push(c);
            offsets.push(i);
            for _ in 0..c.len_utf8() {
                map.push(i);
            }
            s.push(c);
            i += n;
        }
        offsets.push(bytes.len());
        map.push(bytes.len());
        Text { s, chars, offsets, map }
    }

    /// The original offset of a byte offset into `s`.
    pub fn orig(&self, at: usize) -> usize {
        self.map[at]
    }

    /// `utext_setNativeIndex`: an offset clamped to the text and moved
    /// back to the start of its code point.
    pub fn snap(&self, o: i64) -> usize {
        let len = *self.offsets.last().unwrap_or(&0);
        if o <= 0 {
            return 0;
        }
        let o = (o as usize).min(len);
        match self.offsets.binary_search(&o) {
            Ok(_) => o,
            Err(i) => self.offsets[i - 1],
        }
    }

    /// The index of the char starting at original offset `o`.
    pub fn char_at(&self, o: usize) -> Option<usize> {
        self.offsets.binary_search(&o).ok().filter(|&i| i < self.chars.len())
    }
}

/// How a rule set is run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Config {
    Word,
    Line(LineBreakStrictness, bool),
    Sentence(bool),
    Character,
    Title,
}

/// The rule set ICU loaded (an index into `data::RULES`) as a config.
pub fn config(kind: Kind, rules: usize) -> Config {
    match kind {
        Kind::Word => Config::Word,
        Kind::Line => match rules {
            3 => Config::Line(LineBreakStrictness::Loose, false),
            4 => Config::Line(LineBreakStrictness::Normal, false),
            5 => Config::Line(LineBreakStrictness::Loose, true),
            6 => Config::Line(LineBreakStrictness::Normal, true),
            7 => Config::Line(LineBreakStrictness::Strict, true),
            _ => Config::Line(LineBreakStrictness::Strict, false),
        },
        Kind::Sentence => Config::Sentence(rules == 9),
        Kind::Character => Config::Character,
        Kind::Title => Config::Title,
    }
}

const WORD_NUMBER: i64 = 100;
const WORD_LETTER: i64 = 200;
const WORD_IDEO: i64 = 400;
const LINE_HARD: i64 = 100;
const SENTENCE_SEP: i64 = 100;

fn ideographic(c: char) -> bool {
    matches!(CodePointMapData::<Script>::new().get(c), Script::Han | Script::Hiragana | Script::Katakana)
}

/// The boundaries (original byte offsets, `0` first) and their statuses.
pub fn boundaries(cfg: Config, t: &Text) -> (Vec<usize>, Vec<i64>) {
    let s = t.s.as_str();
    let mut out: Vec<(usize, i64)> = vec![(0, 0)];
    let mut push = |at: usize, st: i64| {
        if at > 0 {
            out.push((at, st));
        }
    };
    match cfg {
        Config::Word => {
            use GeneralCategory as G;
            let gc = CodePointMapData::<GeneralCategory>::new();
            let seg = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
            let mut prev = 0;
            for (at, ty) in seg.segment_str(s).iter_with_word_type() {
                let st = match ty {
                    WordType::Number => WORD_NUMBER,
                    WordType::Letter if s[prev..at].chars().any(ideographic) => WORD_IDEO,
                    WordType::Letter => WORD_LETTER,
                    // ICU4X reports the status of the segment's last rule,
                    // which a trailing Extend (`e\u{301}`) resets; ICU keeps
                    // the word's.
                    _ => match s[prev..at].chars().next().map(|c| gc.get(c)) {
                        Some(G::UppercaseLetter | G::LowercaseLetter | G::TitlecaseLetter | G::ModifierLetter | G::OtherLetter) => {
                            WORD_LETTER
                        }
                        Some(G::DecimalNumber) => WORD_NUMBER,
                        _ => 0,
                    },
                };
                push(at, st);
                prev = at;
            }
        }
        Config::Line(strictness, cj) => {
            let ja: LanguageIdentifier = icu::locale::langid!("ja");
            let mut o = LineBreakOptions::default();
            o.strictness = Some(strictness);
            if cj {
                o.content_locale = Some(&ja);
            }
            let seg = LineSegmenter::new_dictionary(o);
            for at in seg.segment_str(s) {
                // UAX #14's LB7 (no break before a space); ICU4X's
                // dictionary for South-East Asian scripts offers one.
                if s[at..].starts_with([' ', '\u{200B}']) {
                    continue;
                }
                let hard = s[..at].chars().next_back().is_some_and(|c| {
                    matches!(c, '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}')
                });
                push(at, if hard { LINE_HARD } else { 0 });
            }
        }
        Config::Sentence(greek) => {
            let el: LanguageIdentifier = icu::locale::langid!("el");
            let seg = if greek {
                let mut o = SentenceBreakOptions::default();
                o.content_locale = Some(&el);
                SentenceSegmenter::try_new(o).ok()
            } else {
                None
            };
            let fallback = SentenceSegmenter::new(SentenceBreakInvariantOptions::default());
            let bps: Vec<usize> = match &seg {
                Some(seg) => seg.as_borrowed().segment_str(s).collect(),
                None => fallback.segment_str(s).collect(),
            };
            let mut prev = 0;
            for at in bps {
                push(at, sentence_status(&s[prev..at], greek));
                prev = at;
            }
        }
        Config::Character => {
            for at in GraphemeClusterSegmenter::new().segment_str(s) {
                push(at, 0);
            }
        }
        Config::Title => {
            for at in title(s) {
                push(at, 0);
            }
        }
    }
    out.dedup_by_key(|b| b.0);
    let bounds = out.iter().map(|b| t.orig(b.0)).collect();
    let status = out.iter().map(|b| b.1).collect();
    (bounds, status)
}

/// A sentence ended by a terminator is `SENTENCE_TERM`; one ended by a
/// separator, or by the end of the text, `SENTENCE_SEP`.
fn sentence_status(seg: &str, greek: bool) -> i64 {
    let sb = CodePointMapData::<SentenceBreak>::new();
    for c in seg.chars().rev() {
        let b = sb.get(c);
        if b == SentenceBreak::Sep || b == SentenceBreak::CR || b == SentenceBreak::LF {
            return SENTENCE_SEP;
        }
        if matches!(b, SentenceBreak::Sp | SentenceBreak::Close | SentenceBreak::Extend | SentenceBreak::Format) {
            continue;
        }
        if b == SentenceBreak::STerm || b == SentenceBreak::ATerm || (greek && c == ';') {
            return 0;
        }
        return SENTENCE_SEP;
    }
    SENTENCE_SEP
}

/// ICU's title rules: `$CaseIgnorable = [[:Mn:][:Me:][:Cf:][:Lm:][:Sk:]'­’]`,
/// `$Cased = [[:Upper_Case:][:Lower_Case:][:Lt:]-$CaseIgnorable]`, a
/// segment is `$Cased ($Cased|$CaseIgnorable)* ($NotCased|$CaseIgnorable)*`
/// (the text before the first cased letter is one of its own).
fn title(s: &str) -> Vec<usize> {
    use GeneralCategory as G;
    let gc = CodePointMapData::<GeneralCategory>::new();
    let ignorable = |c: char| {
        matches!(gc.get(c), G::NonspacingMark | G::EnclosingMark | G::Format | G::ModifierLetter | G::ModifierSymbol)
            || matches!(c, '\'' | '\u{00AD}' | '\u{2019}')
    };
    let cased = |c: char| {
        !ignorable(c)
            && (CodePointSetData::new::<Uppercase>().contains(c)
                || CodePointSetData::new::<Lowercase>().contains(c)
                || gc.get(c) == G::TitlecaseLetter)
    };
    let _ = CodePointSetData::new::<CaseIgnorable>();
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if cased(chars[i].1) {
            i += 1;
            while i < chars.len() && (cased(chars[i].1) || ignorable(chars[i].1)) {
                i += 1;
            }
            while i < chars.len() && !cased(chars[i].1) {
                i += 1;
            }
        } else {
            while i < chars.len() && !cased(chars[i].1) {
                i += 1;
            }
        }
        out.push(chars.get(i).map_or(s.len(), |c| c.0));
    }
    out
}
