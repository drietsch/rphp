//! `SimpleDateFormat::parse`: the pattern walked field by field over the
//! text, each numeric field taking its digits and each named field its
//! longest matching name, then the fields resolved into an instant the
//! way `Calendar::computeTime` does (out-of-range values roll over in
//! lenient mode; in strict mode a field outside its range, a name that
//! needed padding or a literal that did not line up fails the parse).

use super::civil::{self, Fields};
use super::render::Item;
use super::{Calendar, DateState, ZoneRef};
use crate::data::datenames;

/// What a parse accumulated.
#[derive(Default)]
struct Slots {
    era: Option<i64>,
    year: Option<i64>,
    /// A two-digit year, which the 80/20 rule maps into a century.
    year_is_two_digit: bool,
    month: Option<i64>,
    day: Option<i64>,
    day_of_year: Option<i64>,
    hour: Option<i64>,
    /// The hour field's kind: 'h', 'H', 'K', 'k'.
    hour_kind: char,
    minute: Option<i64>,
    second: Option<i64>,
    millis: Option<i64>,
    am_pm: Option<i64>,
    /// A zone the text named: its offset, and its id when it has one.
    zone: Option<(i32, Option<String>)>,
    week_year: Option<i64>,
    week: Option<i64>,
    weekday: Option<i64>,
}

struct Parser<'a> {
    st: &'a DateState,
    chars: &'a [char],
    i: usize,
    strict: bool,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self.i < self.chars.len() && self.chars[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    /// A run of digits, at most `max` of them; `None` when none follow.
    fn number(&mut self, max: usize) -> Option<i64> {
        if !self.strict {
            self.skip_space();
        }
        let mut v: i64 = 0;
        let mut n = 0;
        let mut negative = false;
        if self.chars.get(self.i) == Some(&'-') {
            negative = true;
            self.i += 1;
        }
        while n < max {
            let Some(c) = self.chars.get(self.i) else { break };
            let d = self.st.digits.iter().position(|x| x == c).map(|d| d as i64).or_else(|| c.to_digit(10).map(i64::from)).or_else(|| crate::uchar::decimal_digit(*c as u32).map(i64::from));
            match d {
                Some(d) => {
                    v = v.saturating_mul(10).saturating_add(d);
                    self.i += 1;
                    n += 1;
                }
                None => break,
            }
        }
        if n == 0 {
            if negative {
                self.i -= 1;
            }
            return None;
        }
        Some(if negative { -v } else { v })
    }

    /// The index of the longest name of `names` that the text starts with.
    fn name_in(&mut self, names: &[&str]) -> Option<usize> {
        if !self.strict {
            self.skip_space();
        }
        let mut best: Option<(usize, usize)> = None;
        for (k, name) in names.iter().enumerate() {
            if name.is_empty() {
                continue;
            }
            let n: Vec<char> = name.chars().collect();
            if self.chars.len() - self.i < n.len() {
                continue;
            }
            let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
            if self.chars[self.i..self.i + n.len()].iter().zip(n.iter()).all(|(a, b)| a == b || fold(*a) == fold(*b)) && best.is_none_or(|(len, _)| n.len() > len) {
                best = Some((n.len(), k));
            }
        }
        let (len, k) = best?;
        self.i += len;
        Some(k)
    }

    /// A literal run: matched exactly in strict mode, whitespace-elastic
    /// otherwise.
    fn literal(&mut self, text: &str) -> bool {
        for c in text.chars() {
            if c.is_whitespace() {
                if self.strict {
                    if self.chars.get(self.i) != Some(&c) {
                        return false;
                    }
                    self.i += 1;
                } else {
                    self.skip_space();
                }
                continue;
            }
            if !self.strict {
                self.skip_space();
            }
            match self.chars.get(self.i) {
                Some(x) if *x == c => self.i += 1,
                // ICU's lenient parse lets a missing literal pass
                _ if !self.strict => {}
                _ => return false,
            }
        }
        true
    }

    /// A zone the text names: `GMT±hh:mm`, an ISO offset, or a name CLDR
    /// spells for one of the zones this locale knows.
    fn zone(&mut self, n: usize, kind: char) -> Option<(i32, Option<String>)> {
        if !self.strict {
            self.skip_space();
        }
        let rest: String = self.chars[self.i..].iter().collect();
        // an ISO offset
        if matches!(kind, 'Z' | 'X' | 'x') || rest.starts_with(['+', '-']) || rest.starts_with('Z') {
            if let Some((secs, len)) = iso_offset(&rest) {
                self.i += len;
                return Some((secs, None));
            }
        }
        // `GMT±h[:mm]` / `UTC` / `GMT`
        for prefix in ["GMT", "UTC", "UT"] {
            if rest.starts_with(prefix) {
                let after: String = rest[prefix.len()..].to_string();
                if let Some((secs, len)) = iso_offset(&after) {
                    self.i += prefix.len() + len;
                    return Some((secs, None));
                }
                self.i += prefix.len();
                return Some((0, Some("GMT".to_string())));
            }
        }
        // a CLDR name of a zone: the formatter's own zone first, then
        // every zone the pattern could name
        let _ = n;
        let candidates = zone_candidates(self.st);
        let mut best: Option<(usize, i32, String)> = None;
        for (name, id, offset) in &candidates {
            let len = name.chars().count();
            if len == 0 || self.chars.len() - self.i < len {
                continue;
            }
            let matches = self.chars[self.i..self.i + len].iter().copied().eq(name.chars());
            if matches && best.as_ref().is_none_or(|(l, _, _)| len > *l) {
                best = Some((len, *offset, id.clone()));
            }
        }
        let (len, offset, id) = best?;
        self.i += len;
        Some((offset, Some(id)))
    }
}

/// `+01:00`, `-0500`, `Z`, `+5`: an ISO or GMT offset and its length in
/// characters.
fn iso_offset(text: &str) -> Option<(i32, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.first() == Some(&'Z') {
        return Some((0, 1));
    }
    let sign = match chars.first() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return None,
    };
    let mut i = 1;
    let mut digits = String::new();
    let mut colons = 0;
    while i < chars.len() && digits.len() < 6 {
        if chars[i] == ':' && colons < 2 && !digits.is_empty() {
            colons += 1;
            i += 1;
            continue;
        }
        if chars[i].is_ascii_digit() {
            digits.push(chars[i]);
            i += 1;
            continue;
        }
        break;
    }
    if digits.is_empty() {
        return None;
    }
    let (h, m, s) = match digits.len() {
        1 | 2 => (digits.parse::<i32>().ok()?, 0, 0),
        3 => (digits[..1].parse().ok()?, digits[1..].parse().ok()?, 0),
        4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?, 0),
        5 => (digits[..1].parse().ok()?, digits[1..3].parse().ok()?, digits[3..].parse().ok()?),
        _ => (digits[..2].parse().ok()?, digits[2..4].parse().ok()?, digits[4..].parse().ok()?),
    };
    Some((sign * (h * 3600 + m * 60 + s), i))
}

impl DateState {
    /// Parse `text` from `start`: the fields, the instant and where the
    /// parse stopped.
    pub(super) fn parse(&self, text: &str, start: usize) -> Option<(Fields, i64, usize)> {
        let chars: Vec<char> = text.chars().collect();
        let items = match (&self.relative, &self.items) {
            // a relative formatter parses its plain combined pattern
            (Some((date, time, glue)), _) => {
                let combined = if time.is_empty() { date.clone() } else { glue.replace("{1}", date).replace("{0}", time) };
                super::render::items(&combined)?
            }
            (None, Some(items)) => items.clone(),
            (None, None) => return None,
        };
        let mut p = Parser { st: self, chars: &chars, i: start.min(chars.len()), strict: !self.lenient };
        let mut s = Slots { hour_kind: 'H', ..Default::default() };
        for item in &items {
            match item {
                Item::Literal(t) => {
                    if !p.literal(t) {
                        return None;
                    }
                }
                Item::Field(c, n) => {
                    if !field(&mut p, &mut s, *c, *n) {
                        return None;
                    }
                }
            }
        }
        let end = p.i;
        let (fields, ts) = self.resolve(&s)?;
        Some((fields, ts, end))
    }

    /// The fields resolved into an instant.
    fn resolve(&self, s: &Slots) -> Option<(Fields, i64)> {
        let strict = !self.lenient;
        // the year
        let mut year = s.year.unwrap_or(1970);
        if s.year_is_two_digit {
            // ICU's 80/20 rule around the current year
            let now = rphp_stdlib::date_bridge::now_seconds();
            let this_year = civil::fields_at(now, 0, 0).ext_year;
            let base = this_year - 80;
            year = base + (year - base).rem_euclid(100);
        }
        if self.calendar == Calendar::Buddhist {
            year -= 543;
        }
        if s.era == Some(0) {
            year = 1 - year;
        }
        let month = s.month.unwrap_or(1);
        let day = s.day.unwrap_or(1);
        if strict && (!(1..=12).contains(&month) || !(1..=31).contains(&day)) {
            return None;
        }
        // months roll over into years
        let (year, month) = (year + (month - 1).div_euclid(12), (month - 1).rem_euclid(12) + 1);
        let mut days = civil::days_from_civil(year, month as u32, 1) + day - 1;
        if let Some(doy) = s.day_of_year {
            days = civil::days_from_civil(year, 1, 1) + doy - 1;
        }
        // the clock
        let raw_hour = s.hour.unwrap_or(0);
        let mut hour = match s.hour_kind {
            'h' => raw_hour % 12,
            'k' => raw_hour % 24,
            _ => raw_hour,
        };
        if let Some(pm) = s.am_pm {
            if matches!(s.hour_kind, 'h' | 'K') {
                hour = hour % 12 + pm * 12;
            }
        }
        let minute = s.minute.unwrap_or(0);
        let second = s.second.unwrap_or(0);
        if strict && (raw_hour > 24 || minute > 59 || second > 59) {
            return None;
        }
        let local = days * 86_400 + hour * 3600 + minute * 60 + second;
        // the zone: the one the text named, else the formatter's
        let ts = match &s.zone {
            Some((offset, _)) => local - i64::from(*offset),
            None => {
                let guess = local - i64::from(self.zone.offset_at(local));
                local - i64::from(self.zone.offset_at(guess))
            }
        };
        let offset = match &s.zone {
            Some((offset, _)) => *offset,
            None => self.zone.offset_at(ts),
        };
        let millis = s.millis.unwrap_or(0).clamp(0, 999) as u32;
        let f = self.calendar.apply(civil::fields_at(ts, millis * 1000, offset));
        Some((f, ts))
    }
}

/// Parse one field; `false` when the text does not carry it.
fn field(p: &mut Parser, s: &mut Slots, c: char, n: usize) -> bool {
    let names = p.st.names;
    let slice = |from: usize, len: usize| -> Vec<&'static str> { (0..len).map(|i| names[from + i]).collect() };
    let wide = |n: usize| if p.strict { n } else { 10 };
    match c {
        'G' => match p.name_in(&[names[128], names[129], names[130], names[131], names[132], names[133]]) {
            Some(k) => {
                s.era = Some(i64::from(k % 2 == 0));
                true
            }
            None => false,
        },
        'y' | 'Y' | 'u' | 'U' | 'r' => {
            let max = if n == 2 && p.strict { 2 } else { 10 };
            match p.number(max) {
                Some(v) => {
                    // a two-digit year in a two-letter field
                    if n == 2 && (p.strict || v < 100) {
                        s.year_is_two_digit = true;
                    }
                    if c == 'Y' {
                        s.week_year = Some(v);
                    }
                    s.year = Some(v);
                    true
                }
                None => false,
            }
        }
        'M' | 'L' => {
            if n <= 2 {
                match p.number(wide(2)) {
                    Some(v) => {
                        s.month = Some(v);
                        return true;
                    }
                    None => {
                        if p.strict {
                            return false;
                        }
                    }
                }
            }
            let mut all: Vec<&'static str> = Vec::new();
            for base in [0usize, 12, 24, 36, 48, 60] {
                all.extend(slice(base, 12));
            }
            match p.name_in(&all) {
                Some(k) => {
                    s.month = Some((k % 12) as i64 + 1);
                    true
                }
                None => !p.strict && p.number(2).map(|v| s.month = Some(v)).is_some(),
            }
        }
        'd' => match p.number(wide(n.max(2))) {
            Some(v) => {
                s.day = Some(v);
                true
            }
            None => false,
        },
        'D' => match p.number(wide(3)) {
            Some(v) => {
                s.day_of_year = Some(v);
                true
            }
            None => false,
        },
        'E' | 'c' | 'e' => {
            if (c == 'e' || c == 'c') && n <= 2 {
                return p.number(wide(2)).map(|v| s.weekday = Some(v)).is_some();
            }
            let mut all: Vec<&'static str> = Vec::new();
            for base in [72usize, 79, 86, 93, 100, 107, 114, 121] {
                all.extend(slice(base, 7));
            }
            match p.name_in(&all) {
                Some(k) => {
                    s.weekday = Some((k % 7) as i64);
                    true
                }
                None => false,
            }
        }
        'a' | 'b' => {
            let all: Vec<&'static str> = (134..146).map(|i| names[i]).collect();
            match p.name_in(&all) {
                Some(k) => {
                    // the midnight/noon names sit after the six am/pm ones
                    if k < 6 {
                        s.am_pm = Some((k % 2) as i64);
                    } else {
                        s.am_pm = Some(i64::from((k - 6) % 2 == 1));
                        if (k - 6) % 2 == 0 {
                            s.hour = Some(0);
                        } else {
                            s.hour = Some(12);
                            s.hour_kind = 'H';
                        }
                    }
                    true
                }
                None => false,
            }
        }
        'B' => {
            let all: Vec<&'static str> = (146..290).map(|i| names[i]).collect();
            p.name_in(&all).is_some()
        }
        'h' | 'H' | 'K' | 'k' => match p.number(wide(2)) {
            Some(v) => {
                s.hour = Some(v);
                s.hour_kind = c;
                true
            }
            None => false,
        },
        'm' => match p.number(wide(2)) {
            Some(v) => {
                s.minute = Some(v);
                true
            }
            None => false,
        },
        's' => match p.number(wide(2)) {
            Some(v) => {
                s.second = Some(v);
                true
            }
            None => false,
        },
        'S' => match p.number(wide(n)) {
            Some(v) => {
                // the digits are a fraction of a second
                let mut text = v.to_string();
                while text.len() < 3 {
                    text.push('0');
                }
                s.millis = text[..3].parse().ok();
                true
            }
            None => false,
        },
        'A' => match p.number(wide(9)) {
            Some(v) => {
                s.hour = Some(v / 3_600_000);
                s.hour_kind = 'H';
                s.minute = Some(v % 3_600_000 / 60_000);
                s.second = Some(v % 60_000 / 1000);
                s.millis = Some(v % 1000);
                true
            }
            None => false,
        },
        'z' | 'Z' | 'v' | 'V' | 'O' | 'X' | 'x' => match p.zone(n, c) {
            Some(z) => {
                s.zone = Some(z);
                true
            }
            None => false,
        },
        'w' => p.number(wide(2)).map(|v| s.week = Some(v)).is_some(),
        'W' | 'F' | 'g' | 'Q' | 'q' => {
            if matches!(c, 'Q' | 'q') && n >= 3 {
                let all: Vec<&'static str> = (290..306).map(|i| names[i]).collect();
                return p.name_in(&all).is_some();
            }
            p.number(wide(10)).is_some()
        }
        _ => true,
    }
}

/// The names a `z`/`v`/`V` field may carry, with the zone they mean.
pub(super) fn zone_candidates(st: &DateState) -> Vec<(String, String, i32)> {
    use super::zone::{name, Style};
    let mut out: Vec<(String, String, i32)> = Vec::new();
    let ts = rphp_stdlib::date_bridge::now_seconds();
    let mut push = |zone: &ZoneRef| {
        if zone.iana.is_empty() {
            return;
        }
        for (secs, _dst) in [(zone.offset_at(ts), false)] {
            for style in [Style::SpecificLong, Style::SpecificShort, Style::GenericLong, Style::GenericShort, Style::Location, Style::ExemplarCity] {
                if let Some(text) = name(&st.icu_locale, &zone.iana, secs, ts, style) {
                    if !text.is_empty() {
                        out.push((text, zone.id.clone(), secs));
                    }
                }
            }
        }
        out.push((zone.id.clone(), zone.id.clone(), zone.offset_at(ts)));
        out.push((zone.iana.clone(), zone.id.clone(), zone.offset_at(ts)));
    };
    push(&st.zone);
    // ICU also knows the names of every other zone; the formatter's own
    // and the handful a document is likely to carry are enough here
    for id in ["UTC", "America/New_York", "America/Chicago", "America/Denver", "America/Los_Angeles", "Europe/London", "Europe/Paris", "Europe/Berlin", "Asia/Tokyo", "Asia/Shanghai", "Asia/Kolkata", "Australia/Sydney"] {
        if let Some(z) = ZoneRef::parse(id) {
            push(&z);
        }
    }
    let _ = datenames::N;
    out
}
