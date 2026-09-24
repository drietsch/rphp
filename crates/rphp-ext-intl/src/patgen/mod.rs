//! `IntlDatePatternGenerator`: ICU's `DateTimePatternGenerator`
//! (`dtptngen.cpp`) — the skeleton matcher with ICU's field type table and
//! distance, the pattern map fed the way ICU feeds it (the canonical items,
//! the locale's date and time style patterns and its short-time `mm:ss`
//! hack, the calendar's `availableFormats`), the field-length adjustment,
//! the appending of missing fields through `appendItems`, and the date-time
//! glue. The data is the oracle's (`data.rs`, `tools/intl-data/dump-patgen.php`).

mod data;

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

use crate::shape::{register_class, MethodImpl};
use crate::state::{self, IntlError};
use crate::{generated, locale};

// fields
const ERA: usize = 0;
const YEAR: usize = 1;
const MONTH: usize = 3;
const WEEKDAY: usize = 6;
const DAYPERIOD: usize = 10;
const HOUR: usize = 11;
const MINUTE: usize = 12;
const SECOND: usize = 13;
const FRACTIONAL_SECOND: usize = 14;
const FIELD_COUNT: usize = 16;

const DT_NARROW: i32 = -0x101;
const DT_SHORTER: i32 = -0x102;
const DT_SHORT: i32 = -0x103;
const DT_LONG: i32 = -0x104;
const DT_NUMERIC: i32 = 0x100;
const DT_DELTA: i32 = 0x10;

const EXTRA_FIELD: i32 = 0x10000;
const MISSING_FIELD: i32 = 0x1000;

const FLAG_CAP_J: u32 = 1;
const FLAG_FIX_FRACTIONAL: u32 = 2;

/// `dtTypes`: pattern char, field, type, minimum length.
static DT_TYPES: &[(char, usize, i32, usize)] = &[
    ('G', ERA, DT_SHORT, 1),
    ('G', ERA, DT_LONG, 4),
    ('G', ERA, DT_NARROW, 5),
    ('y', YEAR, DT_NUMERIC, 1),
    ('Y', YEAR, DT_NUMERIC + DT_DELTA, 1),
    ('u', YEAR, DT_NUMERIC + 2 * DT_DELTA, 1),
    ('r', YEAR, DT_NUMERIC + 3 * DT_DELTA, 1),
    ('U', YEAR, DT_SHORT, 1),
    ('U', YEAR, DT_LONG, 4),
    ('U', YEAR, DT_NARROW, 5),
    ('Q', 2, DT_NUMERIC, 1),
    ('Q', 2, DT_SHORT, 3),
    ('Q', 2, DT_LONG, 4),
    ('Q', 2, DT_NARROW, 5),
    ('q', 2, DT_NUMERIC + DT_DELTA, 1),
    ('q', 2, DT_SHORT - DT_DELTA, 3),
    ('q', 2, DT_LONG - DT_DELTA, 4),
    ('q', 2, DT_NARROW - DT_DELTA, 5),
    ('M', MONTH, DT_NUMERIC, 1),
    ('M', MONTH, DT_SHORT, 3),
    ('M', MONTH, DT_LONG, 4),
    ('M', MONTH, DT_NARROW, 5),
    ('L', MONTH, DT_NUMERIC + DT_DELTA, 1),
    ('L', MONTH, DT_SHORT - DT_DELTA, 3),
    ('L', MONTH, DT_LONG - DT_DELTA, 4),
    ('L', MONTH, DT_NARROW - DT_DELTA, 5),
    ('l', MONTH, DT_NUMERIC + DT_DELTA, 1),
    ('w', 4, DT_NUMERIC, 1),
    ('W', 5, DT_NUMERIC, 1),
    ('E', WEEKDAY, DT_SHORT, 1),
    ('E', WEEKDAY, DT_LONG, 4),
    ('E', WEEKDAY, DT_NARROW, 5),
    ('E', WEEKDAY, DT_SHORTER, 6),
    ('c', WEEKDAY, DT_NUMERIC + 2 * DT_DELTA, 1),
    ('c', WEEKDAY, DT_SHORT - 2 * DT_DELTA, 3),
    ('c', WEEKDAY, DT_LONG - 2 * DT_DELTA, 4),
    ('c', WEEKDAY, DT_NARROW - 2 * DT_DELTA, 5),
    ('c', WEEKDAY, DT_SHORTER - 2 * DT_DELTA, 6),
    ('e', WEEKDAY, DT_NUMERIC + DT_DELTA, 1),
    ('e', WEEKDAY, DT_SHORT - DT_DELTA, 3),
    ('e', WEEKDAY, DT_LONG - DT_DELTA, 4),
    ('e', WEEKDAY, DT_NARROW - DT_DELTA, 5),
    ('e', WEEKDAY, DT_SHORTER - DT_DELTA, 6),
    ('d', 9, DT_NUMERIC, 1),
    ('g', 9, DT_NUMERIC + DT_DELTA, 1),
    ('D', 7, DT_NUMERIC, 1),
    ('F', 8, DT_NUMERIC, 1),
    ('a', DAYPERIOD, DT_SHORT, 1),
    ('a', DAYPERIOD, DT_LONG, 4),
    ('a', DAYPERIOD, DT_NARROW, 5),
    ('b', DAYPERIOD, DT_SHORT - DT_DELTA, 1),
    ('b', DAYPERIOD, DT_LONG - DT_DELTA, 4),
    ('b', DAYPERIOD, DT_NARROW - DT_DELTA, 5),
    ('B', DAYPERIOD, DT_SHORT - 3 * DT_DELTA, 1),
    ('B', DAYPERIOD, DT_LONG - 3 * DT_DELTA, 4),
    ('B', DAYPERIOD, DT_NARROW - 3 * DT_DELTA, 5),
    ('H', HOUR, DT_NUMERIC + 10 * DT_DELTA, 1),
    ('k', HOUR, DT_NUMERIC + 11 * DT_DELTA, 1),
    ('h', HOUR, DT_NUMERIC, 1),
    ('K', HOUR, DT_NUMERIC + DT_DELTA, 1),
    ('J', HOUR, DT_NUMERIC + 5 * DT_DELTA, 1),
    ('j', HOUR, DT_NUMERIC + 6 * DT_DELTA, 1),
    ('C', HOUR, DT_NUMERIC + 7 * DT_DELTA, 1),
    ('m', MINUTE, DT_NUMERIC, 1),
    ('s', SECOND, DT_NUMERIC, 1),
    ('A', SECOND, DT_NUMERIC + DT_DELTA, 1),
    ('S', FRACTIONAL_SECOND, DT_NUMERIC, 1),
    ('v', 15, DT_SHORT - 2 * DT_DELTA, 1),
    ('v', 15, DT_LONG - 2 * DT_DELTA, 4),
    ('z', 15, DT_SHORT, 1),
    ('z', 15, DT_LONG, 4),
    ('Z', 15, DT_NARROW - DT_DELTA, 1),
    ('Z', 15, DT_LONG - DT_DELTA, 4),
    ('Z', 15, DT_SHORT - DT_DELTA, 5),
    ('O', 15, DT_SHORT - 2 * DT_DELTA, 1),
    ('O', 15, DT_LONG - 2 * DT_DELTA, 4),
    ('V', 15, DT_SHORT - 2 * DT_DELTA, 1),
    ('V', 15, DT_LONG - 2 * DT_DELTA, 2),
    ('V', 15, DT_LONG - 1 - 2 * DT_DELTA, 3),
    ('V', 15, DT_LONG - 2 - 2 * DT_DELTA, 4),
    ('X', 15, DT_NARROW - DT_DELTA, 1),
    ('X', 15, DT_SHORT - DT_DELTA, 2),
    ('X', 15, DT_LONG - DT_DELTA, 4),
    ('x', 15, DT_NARROW - DT_DELTA, 1),
    ('x', 15, DT_SHORT - DT_DELTA, 2),
    ('x', 15, DT_LONG - DT_DELTA, 4),
];

/// `FormatParser::getCanonicalIndex` (strict).
fn canonical_index(item: &[char]) -> Option<usize> {
    let ch = *item.first()?;
    if item.iter().any(|&c| c != ch) {
        return None;
    }
    let len = item.len();
    let mut i = 0;
    while i < DT_TYPES.len() {
        if DT_TYPES[i].0 != ch {
            i += 1;
            continue;
        }
        if DT_TYPES.get(i + 1).is_none_or(|n| n.0 != ch) {
            return Some(i);
        }
        if DT_TYPES[i + 1].3 <= len {
            i += 1;
            continue;
        }
        return Some(i);
    }
    None
}

/// `FormatParser::set`: runs of one ASCII letter, every other character
/// on its own.
fn tokens(pattern: &str) -> Vec<Vec<char>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out: Vec<Vec<char>> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphabetic() {
            let mut j = i + 1;
            while j < chars.len() && chars[j] == c {
                j += 1;
            }
            out.push(chars[i..j].to_vec());
            i = j;
        } else {
            out.push(vec![c]);
            i += 1;
        }
    }
    out
}

/// `getQuoteLiteral`: from a quote item to its closing quote.
fn quote_literal(items: &[Vec<char>], i: &mut usize) -> String {
    let mut quote = String::new();
    if items[*i].first() == Some(&'\'') {
        quote.extend(&items[*i]);
        *i += 1;
    }
    while *i < items.len() {
        if items[*i].first() == Some(&'\'') {
            if *i + 1 < items.len() && items[*i + 1].first() == Some(&'\'') {
                quote.extend(&items[*i]);
                *i += 1;
                quote.extend(&items[*i]);
                *i += 1;
                continue;
            }
            quote.extend(&items[*i]);
            break;
        }
        quote.extend(&items[*i]);
        *i += 1;
    }
    quote
}

/// `SkeletonFields`: a character and a length per field.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Fields {
    chars: [char; FIELD_COUNT],
    lens: [u8; FIELD_COUNT],
}

impl Fields {
    fn empty() -> Fields {
        Fields { chars: ['\0'; FIELD_COUNT], lens: [0; FIELD_COUNT] }
    }

    fn populate(&mut self, field: usize, ch: char, len: usize) {
        self.chars[field] = ch;
        self.lens[field] = len.min(255) as u8;
    }

    fn is_empty(&self, field: usize) -> bool {
        self.lens[field] == 0
    }

    fn clear(&mut self, field: usize) {
        self.chars[field] = '\0';
        self.lens[field] = 0;
    }

    fn text(&self) -> String {
        let mut s = String::new();
        for f in 0..FIELD_COUNT {
            for _ in 0..self.lens[f] {
                s.push(self.chars[f]);
            }
        }
        s
    }

    fn first_char(&self) -> Option<char> {
        (0..FIELD_COUNT).find(|&f| self.lens[f] > 0).map(|f| self.chars[f])
    }
}

/// `PtnSkeleton`.
#[derive(Clone, Debug)]
struct Skeleton {
    ty: [i32; FIELD_COUNT],
    original: Fields,
    base: Fields,
    added_default_day_period: bool,
}

impl Skeleton {
    /// `DateTimeMatcher::set`.
    fn of(pattern: &str) -> Skeleton {
        let mut s = Skeleton { ty: [0; FIELD_COUNT], original: Fields::empty(), base: Fields::empty(), added_default_day_period: false };
        let items = tokens(pattern);
        let mut i = 0;
        while i < items.len() {
            let value = &items[i];
            if value.first() == Some(&'\'') {
                quote_literal(&items, &mut i);
                i += 1;
                continue;
            }
            if let Some(ci) = canonical_index(value) {
                let (_, field, ty, min) = DT_TYPES[ci];
                s.original.populate(field, value[0], value.len());
                s.base.populate(field, DT_TYPES[ci].0, min);
                s.ty[field] = if ty > 0 { ty + value.len() as i32 } else { ty };
            }
            i += 1;
        }
        // minutes and fractional seconds without seconds: add seconds
        if !s.original.is_empty(FRACTIONAL_SECOND) && !s.original.is_empty(MINUTE) && s.original.is_empty(SECOND) {
            s.original.populate(SECOND, 's', 1);
            s.base.populate(SECOND, 's', 1);
            s.ty[SECOND] = DT_NUMERIC + 1;
        }
        if !s.original.is_empty(HOUR) {
            let hc = s.original.chars[HOUR];
            if hc == 'h' || hc == 'K' {
                if s.original.is_empty(DAYPERIOD) {
                    s.original.populate(DAYPERIOD, 'a', 1);
                    s.base.populate(DAYPERIOD, 'a', 1);
                    s.ty[DAYPERIOD] = DT_SHORT;
                    s.added_default_day_period = true;
                }
            } else if !s.original.is_empty(DAYPERIOD) {
                s.original.clear(DAYPERIOD);
                s.base.clear(DAYPERIOD);
                s.ty[DAYPERIOD] = 0;
            }
        }
        s
    }

    /// `getSkeleton()`: without a day period `set` added.
    fn skeleton_text(&self) -> String {
        let mut t = self.original.text();
        if self.added_default_day_period {
            if let Some(p) = t.find('a') {
                t.remove(p);
            }
        }
        t
    }

    fn field_mask(&self) -> u32 {
        (0..FIELD_COUNT).filter(|&f| self.ty[f] != 0).fold(0, |m, f| m | (1 << f))
    }

    /// `getDistance`: the distance and the missing and extra field masks.
    fn distance(&self, other: &Skeleton, include: u32) -> (i32, u32, u32) {
        let mut result = 0;
        let (mut missing, mut extra) = (0u32, 0u32);
        for i in 0..FIELD_COUNT {
            let my = if include & (1 << i) == 0 { 0 } else { self.ty[i] };
            let ot = other.ty[i];
            if my == ot {
                continue;
            }
            if my == 0 {
                result += EXTRA_FIELD;
                extra |= 1 << i;
            } else if ot == 0 {
                result += MISSING_FIELD;
                missing |= 1 << i;
            } else {
                result += (my - ot).abs();
            }
        }
        (result, missing, extra)
    }
}

/// A pattern map entry (`PtnElem`).
#[derive(Clone, Debug)]
struct Elem {
    base: String,
    skeleton: Skeleton,
    pattern: String,
    specified: bool,
}

/// A generator's state.
#[derive(Clone, Debug)]
pub struct Generator {
    /// By first base character, `A`–`Z` then `a`–`z`, in insertion order.
    boot: Vec<Vec<Elem>>,
    append: [String; FIELD_COUNT],
    names: [String; FIELD_COUNT],
    glue: [String; 4],
    default_hour: char,
    decimal: String,
    available_set: Vec<String>,
}

fn boot_index(c: char) -> Option<usize> {
    match c {
        'A'..='Z' => Some(c as usize - 'A' as usize),
        'a'..='z' => Some(26 + c as usize - 'a' as usize),
        _ => None,
    }
}

impl Generator {
    fn new(canonical: &str) -> Generator {
        let (_, i) = crate::numfmt_resolve(data::LOCALES, canonical);
        let row = &data::ROWS[i];
        let hours: Vec<char> = row.hours.chars().collect();
        let mut g = Generator {
            boot: vec![Vec::new(); 52],
            append: std::array::from_fn(|f| if row.append[f].is_empty() { "{0} \u{251C}{2}: {1}\u{2524}".to_string() } else { row.append[f].to_string() }),
            names: std::array::from_fn(|f| {
                if row.names[f].is_empty() {
                    if f < 10 {
                        format!("F{f}")
                    } else {
                        format!("F1{}", f - 10)
                    }
                } else {
                    row.names[f].to_string()
                }
            }),
            glue: std::array::from_fn(|k| row.glue[k].to_string()),
            default_hour: hours.first().copied().unwrap_or('h'),
            decimal: row.decimal.to_string(),
            available_set: Vec::new(),
        };
        let _ = hours;
        // the canonical items
        for c in "GyQMwWEDFdaHmsSv".chars() {
            g.add_pattern(&c.to_string(), None, false);
        }
        // the style patterns: date then time, full to short
        for k in 0..4 {
            g.add_pattern(row.styles[4 + k], None, false);
            g.add_pattern(row.styles[k], None, false);
        }
        g.hack_times(row.styles[3]);
        // the calendar's availableFormats
        for (skel, pattern, root) in row.avail {
            if g.available_set.iter().any(|s| s == skel) {
                continue;
            }
            g.available_set.push((*skel).to_string());
            let _ = root;
            g.add_pattern(pattern, Some(skel), true);
        }
        g
    }

    /// The hour and day-period characters `C` maps to.
    fn c_chars(canonical: &str) -> (char, char) {
        let (_, i) = crate::numfmt_resolve(data::LOCALES, canonical);
        let h: Vec<char> = data::ROWS[i].hours.chars().collect();
        (h.get(1).copied().unwrap_or('h'), h.get(2).copied().unwrap_or('a'))
    }

    fn find_by_base(&self, base: &str) -> Option<&Elem> {
        let b = boot_index(base.chars().next()?)?;
        self.boot[b].iter().find(|e| e.base == base)
    }

    fn find_by_skeleton(&self, s: &Skeleton) -> Option<&Elem> {
        let b = boot_index(s.base.first_char()?)?;
        self.boot[b].iter().find(|e| e.skeleton.original == s.original)
    }

    /// `addPatternWithSkeleton`.
    fn add_pattern(&mut self, pattern: &str, skeleton_to_use: Option<&str>, overriding: bool) {
        let skeleton = Skeleton::of(skeleton_to_use.unwrap_or(pattern));
        let base = skeleton.base.text();
        if let Some(dup) = self.find_by_base(&base) {
            if (!dup.specified || (skeleton_to_use.is_some() && !overriding)) && !overriding {
                return;
            }
        }
        if let Some(dup) = self.find_by_skeleton(&skeleton) {
            if !overriding || (skeleton_to_use.is_some() && dup.specified) {
                return;
            }
        }
        let Some(b) = base.chars().next().and_then(boot_index) else {
            return;
        };
        let list = &mut self.boot[b];
        match list.iter_mut().find(|e| e.base == base && e.skeleton.ty == skeleton.ty) {
            Some(e) => {
                e.pattern = pattern.to_string();
                e.specified = skeleton_to_use.is_some();
            }
            None => list.push(Elem { base, skeleton, pattern: pattern.to_string(), specified: skeleton_to_use.is_some() }),
        }
    }

    /// `hackTimes`: the short time pattern's `mm:ss` part.
    fn hack_times(&mut self, pattern: &str) {
        let items = tokens(pattern);
        let mut mmss = String::new();
        let mut got_mm = false;
        let mut i = 0;
        while i < items.len() {
            let field = &items[i];
            if field.first() == Some(&'\'') {
                if got_mm {
                    let q = quote_literal(&items, &mut i);
                    mmss.push_str(&q);
                }
            } else if is_separator(field) && got_mm {
                mmss.extend(field);
            } else {
                let ch = field[0];
                if ch == 'm' {
                    got_mm = true;
                    mmss.extend(field);
                } else if ch == 's' {
                    if !got_mm {
                        break;
                    }
                    mmss.extend(field);
                    self.add_pattern(&mmss.clone(), None, false);
                    break;
                } else if got_mm || matches!(ch, 'z' | 'Z' | 'v' | 'V') {
                    break;
                }
            }
            i += 1;
        }
    }

    /// `getBestRaw`: the closest pattern, its specified skeleton, and the
    /// missing and extra field masks.
    fn best_raw(&self, source: &Skeleton, include: u32) -> Option<(&Elem, u32, u32)> {
        let mut best_distance = i32::MAX;
        let mut best_missing: i64 = -1;
        let mut best: Option<(&Elem, u32, u32)> = None;
        'outer: for list in &self.boot {
            for e in list {
                let (d, missing, extra) = source.distance(&e.skeleton, include);
                if d < best_distance || (d == best_distance && best_missing < i64::from(missing)) {
                    best_distance = d;
                    best_missing = i64::from(missing);
                    best = Some((e, missing, extra));
                    if d == 0 {
                        break 'outer;
                    }
                }
            }
        }
        best
    }

    /// `adjustFieldTypes`.
    fn adjust(&self, pattern: &str, specified: Option<&Skeleton>, req: &Skeleton, flags: u32) -> String {
        let items = tokens(pattern);
        let mut out = String::new();
        let mut i = 0;
        while i < items.len() {
            let field = &items[i];
            if field.first() == Some(&'\'') {
                out.push_str(&quote_literal(&items, &mut i));
                i += 1;
                continue;
            }
            let Some(ci) = canonical_index(field) else {
                out.extend(field);
                i += 1;
                continue;
            };
            let (_, ty_field, row_ty, _) = DT_TYPES[ci];
            if flags & FLAG_FIX_FRACTIONAL != 0 && ty_field == SECOND {
                out.extend(field);
                out.push_str(&self.decimal);
                for _ in 0..req.original.lens[FRACTIONAL_SECOND] {
                    out.push(req.original.chars[FRACTIONAL_SECOND]);
                }
            } else if req.ty[ty_field] != 0 {
                let req_char = req.original.chars[ty_field];
                let mut req_len = usize::from(req.original.lens[ty_field]);
                if req_char == 'E' && req_len < 3 {
                    req_len = 3;
                }
                let mut adj_len = req_len;
                if matches!(ty_field, HOUR | MINUTE | SECOND) {
                    adj_len = field.len();
                } else if let Some(sk) = specified {
                    if req_char != 'c' && req_char != 'e' {
                        let skel_len = usize::from(sk.original.lens[ty_field]);
                        let pat_numeric = row_ty > 0;
                        let skel_numeric = sk.ty[ty_field] > 0;
                        let req_numeric = req.ty[ty_field] > 0;
                        if skel_len == req_len || pat_numeric != skel_numeric || pat_numeric != req_numeric {
                            adj_len = field.len();
                        }
                    }
                }
                let mut c = if ty_field != HOUR && ty_field != MONTH && ty_field != WEEKDAY && (ty_field != YEAR || req_char == 'Y') { req_char } else { field[0] };
                if c == 'E' && adj_len < 3 {
                    c = 'e';
                }
                if ty_field == HOUR {
                    let d = self.default_hour;
                    if flags & FLAG_CAP_J != 0 || req_char == d {
                        c = d;
                    } else if req_char == 'h' && d == 'K' {
                        c = 'K';
                    } else if req_char == 'H' && d == 'k' {
                        c = 'k';
                    } else if req_char == 'k' && d == 'H' {
                        c = 'H';
                    } else if req_char == 'K' && d == 'h' {
                        c = 'h';
                    }
                }
                for _ in 0..adj_len {
                    out.push(c);
                }
            } else {
                out.extend(field);
            }
            i += 1;
        }
        out
    }

    /// `getBestAppending`.
    fn best_appending(&self, req: &Skeleton, missing_fields: u32, flags: u32) -> String {
        if missing_fields == 0 {
            return String::new();
        }
        let Some((e, mut missing, _)) = self.best_raw(req, missing_fields) else {
            return String::new();
        };
        let mut specified = e.specified.then_some(&e.skeleton);
        let mut result = self.adjust(&e.pattern, specified, req, flags);
        let mut last_missing = 0u32;
        const FRACTIONAL_MASK: u32 = 1 << FRACTIONAL_SECOND;
        const SECOND_AND_FRACTIONAL: u32 = (1 << SECOND) | FRACTIONAL_MASK;
        while missing != 0 {
            if last_missing == missing {
                break;
            }
            if missing & SECOND_AND_FRACTIONAL == FRACTIONAL_MASK && missing_fields & SECOND_AND_FRACTIONAL == SECOND_AND_FRACTIONAL {
                result = self.adjust(&result, specified, req, flags | FLAG_FIX_FRACTIONAL);
                missing &= !FRACTIONAL_MASK;
                continue;
            }
            let starting = missing;
            let Some((e2, m2, _)) = self.best_raw(req, missing) else {
                break;
            };
            missing = m2;
            specified = e2.specified.then_some(&e2.skeleton);
            let temp = self.adjust(&e2.pattern, specified, req, flags);
            let found = starting & !missing;
            if found != 0 {
                let top = 31 - found.leading_zeros() as usize;
                let fmt = &self.append[top];
                if !fmt.is_empty() {
                    let name = format!("'{}'", self.names[top]);
                    result = compose(fmt, &[&result, &temp, &name]);
                }
            }
            last_missing = missing;
        }
        result
    }

    /// `getBestPattern`.
    fn best_pattern(&self, skeleton: &str, c_chars: (char, char)) -> String {
        let (mapped, flags) = self.map_meta(skeleton, c_chars);
        let req = Skeleton::of(&mapped);
        let Some((e, missing, extra)) = self.best_raw(&req, u32::MAX) else {
            return String::new();
        };
        if missing == 0 && extra == 0 {
            return self.adjust(&e.pattern, e.specified.then_some(&e.skeleton), &req, flags);
        }
        let needed = req.field_mask();
        let date_mask = (1u32 << DAYPERIOD) - 1;
        let time_mask = ((1u32 << FIELD_COUNT) - 1) & !date_mask;
        let date = self.best_appending(&req, needed & date_mask, flags);
        let time = self.best_appending(&req, needed & time_mask, flags);
        if date.is_empty() {
            return time;
        }
        if time.is_empty() {
            return date;
        }
        let month_len = req.base.lens[MONTH];
        let style = if month_len == 4 {
            if req.base.lens[WEEKDAY] > 0 {
                0
            } else {
                1
            }
        } else if month_len == 3 {
            2
        } else {
            3
        };
        compose(&self.glue[style], &[&time, &date])
    }

    /// `mapSkeletonMetacharacters`: `j`, `C`, `J`.
    fn map_meta(&self, skeleton: &str, c_chars: (char, char)) -> (String, u32) {
        let chars: Vec<char> = skeleton.chars().collect();
        let mut out = String::new();
        let mut flags = 0;
        let mut quoted = false;
        let mut p = 0;
        while p < chars.len() {
            let c = chars[p];
            if c == '\'' {
                quoted = !quoted;
            } else if !quoted {
                if c == 'j' || c == 'C' {
                    let mut extra = 0;
                    while p + 1 < chars.len() && chars[p + 1] == c {
                        extra += 1;
                        p += 1;
                    }
                    let hour_len = 1 + (extra & 1);
                    let mut period_len = if extra < 2 { 1 } else { 3 + (extra >> 1) };
                    let (hour_char, period_char) = if c == 'j' { (self.default_hour, 'a') } else { c_chars };
                    if hour_char == 'H' || hour_char == 'k' {
                        period_len = 0;
                    }
                    for _ in 0..period_len {
                        out.push(period_char);
                    }
                    for _ in 0..hour_len {
                        out.push(hour_char);
                    }
                } else if c == 'J' {
                    out.push('H');
                    flags |= FLAG_CAP_J;
                } else {
                    out.push(c);
                }
            }
            p += 1;
        }
        (out, flags)
    }
}

/// `FormatParser::isPatternSeparator` over one item.
fn is_separator(field: &[char]) -> bool {
    field.iter().all(|&c| matches!(c, '\'' | '\\' | ' ' | ':' | '"' | ',' | '-' | '.'))
}

/// `SimpleFormatter::format`: `{n}` replaced by the n-th value, quoting
/// (`''`, `'…'`) as SimpleFormatter reads it.
fn compose(pattern: &str, values: &[&str]) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut quoted = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            // a quote starts a literal only before `{`, `}` or inside one
            if quoted || matches!(chars.get(i + 1), Some('{') | Some('}')) {
                quoted = !quoted;
                i += 1;
                continue;
            }
            out.push('\'');
            i += 1;
            continue;
        }
        if !quoted && c == '{' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '}') {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                if let Ok(n) = inner.parse::<usize>() {
                    if let Some(v) = values.get(n) {
                        out.push_str(v);
                        i += end + 2;
                        continue;
                    }
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// The best pattern for a skeleton in a locale (`DateFormat::
/// createInstanceForSkeleton`, MessageFormat's `{d, date, ::skeleton}`).
pub(crate) fn best_pattern_for(locale: &str, skeleton: &str) -> Result<String, i64> {
    let canonical = locale::canonical(locale);
    let g = Generator::new(&canonical);
    Ok(g.best_pattern(skeleton, Generator::c_chars(&canonical)))
}

// ---- the class --------------------------------------------------------------------------------

/// The object's state: the generator and its locale.
pub struct PatGenState {
    gen: Generator,
    c_chars: (char, char),
    err: IntlError,
}

/// `ULOC_FULLNAME_CAPACITY - 1`.
const MAX_LOCALE_LEN: usize = 156;

fn build(ctx: &mut Ctx, args: &[Value]) -> Result<PatGenState, (i64, String)> {
    state::reset_global(ctx);
    let bytes = crate::opt_arg(args, 0).map(Value::to_php_bytes).unwrap_or_default();
    if bytes.len() > MAX_LOCALE_LEN {
        return Err((state::U_ILLEGAL_ARGUMENT_ERROR, format!("Locale string too long, should be no longer than {MAX_LOCALE_LEN} characters")));
    }
    let requested = if bytes.is_empty() { state::default_locale(ctx) } else { String::from_utf8_lossy(&bytes).into_owned() };
    let canonical = locale::canonical(&requested);
    let lang = canonical.split(['_', '@']).next().unwrap_or("");
    // ICU's bundle fallback: the default locale for a language it lacks
    let canonical = if lang.is_empty() || crate::data::numfmt::LANGUAGES.binary_search(&lang).is_ok() { canonical } else { locale::canonical(&state::default_locale(ctx)) };
    Ok(PatGenState { gen: Generator::new(&canonical), c_chars: Generator::c_chars(&canonical), err: IntlError::default() })
}

fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = o.ok_or_else(|| Unwind::error("Non-static method called statically"))?;
    if o.with_payload::<PatGenState, _>(|_| ()).is_some() {
        let who = ctx.active_function_name();
        let prefixed = format!("{who}(): Cannot call constructor twice");
        let g = state::global(ctx);
        g.code = state::U_ILLEGAL_ARGUMENT_ERROR;
        g.msg = Some(prefixed.clone());
        return Err(Unwind::exception("IntlException", prefixed));
    }
    match build(ctx, args) {
        Ok(st) => {
            o.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Null)
        }
        Err((code, msg)) => {
            let who = ctx.active_function_name();
            let prefixed = format!("{who}(): {msg}");
            let g = state::global(ctx);
            g.code = code;
            g.msg = Some(prefixed.clone());
            Err(Unwind::exception("IntlException", prefixed))
        }
    }
}

fn create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    match build(ctx, args) {
        Ok(st) => {
            let cid = ctx.lookup_class_or_error(b"IntlDatePatternGenerator")?;
            let obj = ctx.instantiate(cid);
            obj.set_payload(Payload::Native(Box::new(st)));
            Ok(Value::Object(obj))
        }
        Err((code, msg)) => {
            let who = ctx.active_function_name();
            state::set_global(ctx, &who, code, &msg)?;
            Ok(Value::Null)
        }
    }
}

fn get_best_pattern(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = o.ok_or_else(|| Unwind::error("Non-static method called statically"))?;
    state::reset_global(ctx);
    let bytes = args.first().map(Value::to_php_bytes).unwrap_or_default();
    let Some(()) = o.with_payload::<PatGenState, _>(|st| st.err.reset()) else {
        return Err(Unwind::error("Found unconstructed IntlDatePatternGenerator"));
    };
    let Ok(skeleton) = String::from_utf8(bytes) else {
        let who = ctx.active_function_name();
        let mut err = IntlError::default();
        state::set_both(ctx, &mut err, &who, state::U_INVALID_CHAR_FOUND, "Skeleton is not a valid UTF-8 string")?;
        o.with_payload::<PatGenState, _>(|st| st.err = err);
        return Ok(Value::Bool(false));
    };
    let r = o
        .with_payload::<PatGenState, _>(|st| {
            // getSkeleton() first, then the best pattern
            let cleaned = Skeleton::of(&skeleton).skeleton_text();
            st.gen.best_pattern(&cleaned, st.c_chars)
        })
        .unwrap_or_default();
    Ok(Value::string(r.as_bytes()))
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    match src.with_payload::<PatGenState, _>(|s| PatGenState { gen: s.gen.clone(), c_chars: s.c_chars, err: IntlError::default() }) {
        Some(st) => {
            dst.set_payload(Payload::Native(Box::new(st)));
            Ok(())
        }
        None => Err(Unwind::error("Cannot clone uninitialized IntlDatePatternGenerator")),
    }
}

static METHODS: &[MethodImpl] = &[("__construct", construct), ("create", create), ("getBestPattern", get_best_pattern)];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLDATEPATTERNGENERATOR, METHODS, |b| b.payload_clone(payload_clone));
}
