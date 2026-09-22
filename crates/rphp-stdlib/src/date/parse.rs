//! php's date/time string parser — `strtotime()`, `date_parse()` and the
//! string half of the `DateTime` family.
//!
//! php does not parse dates with one grammar; it runs a longest-match scanner
//! over a list of independent rules ("ISO 8601 date", "American date",
//! "relative text", "timezone", …) and each rule that fires *mutates* one
//! half-built time. That is why `strtotime("3pm tomorrow")` is midnight while
//! `strtotime("tomorrow 3pm")` is 15:00: `tomorrow` clears the time, and the
//! order the rules fire in is the order they appear in the string.
//!
//! The scanner here works the same way. At each position every rule is tried
//! and the longest match wins, ties going to the earlier rule — the tie-break
//! a generated lexer gives for free, and one the input depends on:
//! `01.07.08` is the *time* 01:07:08 because the time rule is listed before
//! the `d.m.y` date rule, while `1.7.2008` is a date because the date rule
//! matches two characters more.
//!
//! Unparsable input does not abort the scan: the position is recorded as an
//! error and one character is skipped, because `date_parse()` reports every
//! error it met. `strtotime()` then answers `false` for any input that
//! produced at least one error — which is why `strtotime(" ")` is the base
//! timestamp but `strtotime("")` is `false`.
//!
//! Resolution is deliberately *civil*: relative amounts are added to the
//! broken-down fields and the result is converted to an instant exactly once,
//! at the end. `2021-03-27 12:00 +24 hours` in `Europe/Berlin` is therefore
//! 12:00 the next day (23 real hours), not 13:00.

use super::civil;
use super::tz::{self, Tz};

/// The relative part of a parse: what `+1 month`, `next monday`,
/// `first day of` and `3 weekdays` contribute.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rel {
    pub(crate) y: i64,
    pub(crate) m: i64,
    pub(crate) d: i64,
    pub(crate) h: i64,
    pub(crate) i: i64,
    pub(crate) s: i64,
    pub(crate) us: i64,
    /// The weekday a plain `monday` / `next monday` walks to. Negative after
    /// `ago`, which flips the walk to the past (`0` becomes `-7`, since `-0`
    /// would be indistinguishable from Sunday).
    pub(crate) weekday: Option<i64>,
    /// 0 for `next`/`last`/an ordinal, 1 for a bare weekday and `this`,
    /// 2 for `this week` and friends.
    pub(crate) weekday_behavior: i64,
    /// `first day of` (1) / `last day of` (2).
    pub(crate) first_last_day_of: u8,
    /// `<ordinal> <weekday> of`: the ordinal (`-1` for `last`, `0` for
    /// `this`) and the weekday.
    pub(crate) nth_weekday: Option<(i64, i64)>,
    /// `weekday` / `weekdays`: a count of business days.
    pub(crate) special_weekday: Option<i64>,
}

/// One scanned string: the fields it pinned down, the relative amounts it
/// carries, and every error and warning met on the way.
#[derive(Clone, Debug, Default)]
pub(crate) struct Parsed {
    pub(crate) y: Option<i64>,
    pub(crate) m: Option<i64>,
    pub(crate) d: Option<i64>,
    pub(crate) h: Option<i64>,
    pub(crate) i: Option<i64>,
    pub(crate) s: Option<i64>,
    pub(crate) us: Option<i64>,
    pub(crate) have_date: bool,
    /// How many clock readings the string carried. php counts rather than
    /// flags them, because a second bare four-digit group after a time is
    /// read as a *year* (`0030 0030` is 00:30 in the year 30) and only a
    /// third is an error.
    pub(crate) have_time: u8,
    pub(crate) have_relative: bool,
    pub(crate) have_zone: bool,
    pub(crate) zone: Option<Tz>,
    pub(crate) rel: Rel,
    /// `(byte offset, message)`, in the order they were met.
    pub(crate) errors: Vec<(usize, &'static str)>,
    pub(crate) warnings: Vec<(usize, &'static str)>,
}

impl Parsed {
    /// Whether any field was pinned down or any relative amount collected —
    /// php's `is_localtime` is about the zone, but `date_parse` reports the
    /// `relative` key only when this is true.
    pub(crate) fn has_relative(&self) -> bool {
        self.have_relative
    }
}

// ---- cursor ------------------------------------------------------------------

/// A byte cursor over the rest of the input. Every rule matcher is written
/// against this, and a matcher that returns `None` leaves no trace because the
/// caller works on its own copy.
#[derive(Clone, Copy)]
struct Cur<'a> {
    s: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a [u8]) -> Cur<'a> {
        Cur { s, p: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.p).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.s.get(self.p + n).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.p += 1;
            true
        } else {
            false
        }
    }

    /// One byte out of `set`.
    fn eat_any(&mut self, set: &[u8]) -> bool {
        match self.peek() {
            Some(c) if set.contains(&c) => {
                self.p += 1;
                true
            }
            _ => false,
        }
    }

    /// As many bytes out of `set` as there are.
    fn eat_while(&mut self, set: &[u8]) -> usize {
        let start = self.p;
        while self.eat_any(set) {}
        self.p - start
    }

    /// `[ \t]*`.
    fn spaces(&mut self) -> usize {
        self.eat_while(b" \t")
    }

    /// A case-insensitive literal.
    fn eat_word(&mut self, lit: &[u8]) -> bool {
        if self.s.len() - self.p.min(self.s.len()) < lit.len() {
            return false;
        }
        for (k, w) in lit.iter().enumerate() {
            match self.s.get(self.p + k) {
                Some(c) if c.eq_ignore_ascii_case(w) => {}
                _ => return false,
            }
        }
        self.p += lit.len();
        true
    }

    /// Between `min` and `max` digits, as a number and a digit count. php's
    /// `timelib_get_nr`.
    fn digits(&mut self, min: usize, max: usize) -> Option<(i64, usize)> {
        let start = self.p;
        let mut v: i64 = 0;
        while self.p - start < max {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    v = v.saturating_mul(10).saturating_add((c - b'0') as i64);
                    self.p += 1;
                }
                _ => break,
            }
        }
        let n = self.p - start;
        if n < min {
            self.p = start;
            return None;
        }
        Some((v, n))
    }

    /// A fixed-width run of digits.
    fn fixed(&mut self, n: usize) -> Option<i64> {
        self.digits(n, n).map(|(v, _)| v)
    }
}

// ---- field matchers ----------------------------------------------------------

/// One or two digits, where the two-digit reading is only taken when php's
/// own character class would take it: `[01]?[0-9] | "2"[0-4]` reads `25` as
/// the single digit `2`, not as an out-of-range hour, and that shorter match
/// is what makes `+25:00` a `+02:05` offset followed by three stray
/// characters.
fn nr2(c: &mut Cur, ok2: fn(i64) -> bool, ok1: fn(i64) -> bool) -> Option<i64> {
    let d0 = c.peek().filter(u8::is_ascii_digit)? - b'0';
    if let Some(d1) = c.peek_at(1).filter(u8::is_ascii_digit) {
        let v = (d0 as i64) * 10 + (d1 - b'0') as i64;
        if ok2(v) {
            c.p += 2;
            return Some(v);
        }
    }
    if ok1(d0 as i64) {
        c.p += 1;
        return Some(d0 as i64);
    }
    None
}

/// Exactly `n` digits, accepted only when `ok` takes the value.
fn nrlz(c: &mut Cur, n: usize, ok: fn(i64) -> bool) -> Option<i64> {
    let save = c.p;
    let v = c.digits(n, n).map(|(v, _)| v)?;
    if ok(v) {
        Some(v)
    } else {
        c.p = save;
        None
    }
}

/// `hour24`.
fn hour24(c: &mut Cur) -> Option<i64> {
    nr2(c, |v| v <= 24, |_| true)
}

/// `hour24lz`.
fn hour24lz(c: &mut Cur) -> Option<i64> {
    nrlz(c, 2, |v| v <= 24)
}

/// `hour12`.
fn hour12(c: &mut Cur) -> Option<i64> {
    nr2(c, |v| (1..=12).contains(&v), |v| v >= 1)
}

/// `minute`.
fn minute(c: &mut Cur) -> Option<i64> {
    nr2(c, |v| v <= 59, |_| true)
}

/// `minutelz`.
fn minutelz(c: &mut Cur) -> Option<i64> {
    nrlz(c, 2, |v| v <= 59)
}

/// `second` — `60` is a leap second php accepts.
fn second(c: &mut Cur) -> Option<i64> {
    nr2(c, |v| v <= 60, |_| true)
}

/// `secondlz`.
fn secondlz(c: &mut Cur) -> Option<i64> {
    nrlz(c, 2, |v| v <= 60)
}

/// `month`.
fn month_nr(c: &mut Cur) -> Option<i64> {
    nr2(c, |v| v <= 12, |_| true)
}

/// `monthlz`.
fn monthlz(c: &mut Cur) -> Option<i64> {
    nrlz(c, 2, |v| v <= 12)
}

/// `day` with its optional English ordinal suffix (`1st`, `22nd`).
fn day_nr(c: &mut Cur) -> Option<i64> {
    let v = nr2(c, |v| v <= 31, |_| true)?;
    let suf = c.p;
    if !(c.eat_word(b"st") || c.eat_word(b"nd") || c.eat_word(b"rd") || c.eat_word(b"th")) {
        c.p = suf;
    }
    Some(v)
}

/// `daylz`.
fn daylz(c: &mut Cur) -> Option<i64> {
    nrlz(c, 2, |v| v <= 31)
}

/// `weekofyear` — `01`–`53`.
fn week_of_year(c: &mut Cur) -> Option<i64> {
    nrlz(c, 2, |v| (1..=53).contains(&v))
}

/// `dayofyear` — `001`–`366`.
fn day_of_year_nr(c: &mut Cur) -> Option<i64> {
    nrlz(c, 3, |v| (1..=366).contains(&v))
}

/// `year` — one to four digits, with the two-digit window php applies to any
/// literal shorter than four digits.
fn year_nr(c: &mut Cur) -> Option<(i64, usize)> {
    c.digits(1, 4)
}

/// `year4`.
fn year4(c: &mut Cur) -> Option<i64> {
    c.fixed(4)
}

/// `year4withsign`.
fn year4_signed(c: &mut Cur) -> Option<i64> {
    let save = c.p;
    let neg = if c.eat(b'-') {
        true
    } else {
        c.eat(b'+');
        false
    };
    match c.fixed(4) {
        Some(v) => Some(if neg { -v } else { v }),
        None => {
            c.p = save;
            None
        }
    }
}

/// `.` followed by digits, truncated to microseconds the way php truncates.
fn fraction(c: &mut Cur) -> Option<i64> {
    let save = c.p;
    if !c.eat(b'.') {
        return None;
    }
    let start = c.p;
    let mut us: i64 = 0;
    let mut n = 0;
    while let Some(ch) = c.peek() {
        if !ch.is_ascii_digit() {
            break;
        }
        if n < 6 {
            us = us * 10 + (ch - b'0') as i64;
        }
        n += 1;
        c.p += 1;
    }
    if c.p == start {
        c.p = save;
        return None;
    }
    let mut k = n;
    while k < 6 {
        us *= 10;
        k += 1;
    }
    Some(us)
}

/// `meridian`: `am`, `p.m.`, … php requires the run to be followed by a
/// space, a tab or the end of the string, which is why `3pm,` does not parse.
fn meridian(c: &mut Cur) -> Option<bool> {
    let save = c.p;
    let pm = match c.peek() {
        Some(b'a') | Some(b'A') => false,
        Some(b'p') | Some(b'P') => true,
        _ => return None,
    };
    c.p += 1;
    c.eat(b'.');
    if !(c.eat(b'm') || c.eat(b'M')) {
        c.p = save;
        return None;
    }
    c.eat(b'.');
    match c.peek() {
        None => Some(pm),
        Some(b' ') | Some(b'\t') => {
            c.p += 1;
            Some(pm)
        }
        _ => {
            c.p = save;
            None
        }
    }
}

/// Apply a meridian to a 12-hour clock reading (`12am` is hour 0).
fn apply_meridian(h: i64, pm: bool) -> i64 {
    (h % 12) + if pm { 12 } else { 0 }
}

// ---- word tables -------------------------------------------------------------

/// The full month names, in php's order.
const MONTH_FULL: [&[u8]; 12] = [
    b"january",
    b"february",
    b"march",
    b"april",
    b"may",
    b"june",
    b"july",
    b"august",
    b"september",
    b"october",
    b"november",
    b"december",
];

/// The month abbreviations php accepts, `sept` among them.
const MONTH_ABBR: [(&[u8], i64); 13] = [
    (b"sept", 9),
    (b"jan", 1),
    (b"feb", 2),
    (b"mar", 3),
    (b"apr", 4),
    (b"may", 5),
    (b"jun", 6),
    (b"jul", 7),
    (b"aug", 8),
    (b"sep", 9),
    (b"oct", 10),
    (b"nov", 11),
    (b"dec", 12),
];

/// The roman month numerals. php accepts these in **upper case only**:
/// `13.VI.2021` is June, `13.vi.2021` does not parse at all.
const MONTH_ROMAN: [(&[u8], i64); 12] = [
    (b"VIII", 8),
    (b"VII", 7),
    (b"XII", 12),
    (b"III", 3),
    (b"VI", 6),
    (b"IV", 4),
    (b"IX", 9),
    (b"XI", 11),
    (b"II", 2),
    (b"V", 5),
    (b"X", 10),
    (b"I", 1),
];

/// Which spellings of a month a rule takes. php is not uniform about it:
/// `2021-Jan-13` is a date but `2021-january-13` is not, and a roman numeral
/// is a month only inside a longer date — on its own, `I` is the military
/// timezone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MonthSpelling {
    Any,
    NoRoman,
    AbbrOnly,
}

/// `monthtext`. The longest spelling wins, so `september` never stops at
/// `sep`.
fn month_text_as(c: &mut Cur, how: MonthSpelling) -> Option<i64> {
    let mut best: Option<(usize, i64)> = None;
    if how != MonthSpelling::AbbrOnly {
        for (idx, name) in MONTH_FULL.iter().enumerate() {
            let mut t = *c;
            if t.eat_word(name) && best.is_none_or(|(len, _)| t.p > len) {
                best = Some((t.p, idx as i64 + 1));
            }
        }
    }
    for (name, n) in MONTH_ABBR.iter() {
        let mut t = *c;
        if t.eat_word(name) {
            // An abbreviation may carry a trailing dot (`Jan.`).
            t.eat(b'.');
            if best.is_none_or(|(len, _)| t.p > len) {
                best = Some((t.p, *n));
            }
        }
    }
    if how == MonthSpelling::Any {
        for (name, n) in MONTH_ROMAN.iter() {
            if c.s[c.p..].starts_with(name) {
                let end = c.p + name.len();
                if best.is_none_or(|(len, _)| end > len) {
                    best = Some((end, *n));
                }
            }
        }
    }
    let (end, n) = best?;
    c.p = end;
    Some(n)
}

/// `monthtext` in its widest reading.
fn month_text(c: &mut Cur) -> Option<i64> {
    month_text_as(c, MonthSpelling::Any)
}

/// The full weekday names, `0` = Sunday.
const DAY_FULL: [&[u8]; 7] = [
    b"sunday",
    b"monday",
    b"tuesday",
    b"wednesday",
    b"thursday",
    b"friday",
    b"saturday",
];

/// The three-letter weekday abbreviations. php takes no others: `tues`,
/// `thur` and `weds` end up read as (unknown) timezone abbreviations,
/// because the timezone rule matches one character more and the longest
/// match wins.
const DAY_ABBR: [&[u8]; 7] = [b"sun", b"mon", b"tue", b"wed", b"thu", b"fri", b"sat"];

/// `dayfull 's'? | dayabbr`. Only the full names take the plural, which is
/// why `mondays` is a weekday but `sats` is an unknown timezone.
fn day_text(c: &mut Cur) -> Option<i64> {
    let mut best: Option<(usize, i64)> = None;
    for (idx, name) in DAY_FULL.iter().enumerate() {
        let mut t = *c;
        if t.eat_word(name) {
            let mut t2 = t;
            if t2.eat(b's') || t2.eat(b'S') {
                t = t2;
            }
            if best.is_none_or(|(len, _)| t.p > len) {
                best = Some((t.p, idx as i64));
            }
        }
    }
    for (idx, name) in DAY_ABBR.iter().enumerate() {
        let mut t = *c;
        if t.eat_word(name) && best.is_none_or(|(len, _)| t.p > len) {
            best = Some((t.p, idx as i64));
        }
    }
    let (end, n) = best?;
    c.p = end;
    Some(n)
}

/// The ordinal words php accepts in front of a unit, with their amount and
/// the `weekday_behavior` they imply (`1` only for `this`).
const REL_TEXT: [(&[u8], i64, i64); 17] = [
    (b"previous", -1, 0),
    (b"eleventh", 11, 0),
    (b"twelfth", 12, 0),
    (b"seventh", 7, 0),
    (b"eighth", 8, 0),
    (b"fourth", 4, 0),
    (b"second", 2, 0),
    (b"eight", 8, 0),
    (b"fifth", 5, 0),
    (b"first", 1, 0),
    (b"ninth", 9, 0),
    (b"sixth", 6, 0),
    (b"tenth", 10, 0),
    (b"third", 3, 0),
    (b"last", -1, 0),
    (b"next", 1, 0),
    (b"this", 0, 1),
];

/// `reltextnumber | reltexttext`.
fn rel_text(c: &mut Cur) -> Option<(i64, i64)> {
    let mut best: Option<(usize, i64, i64)> = None;
    for (name, amount, behavior) in REL_TEXT.iter() {
        let mut t = *c;
        if t.eat_word(name) && best.is_none_or(|(len, _, _)| t.p > len) {
            best = Some((t.p, *amount, *behavior));
        }
    }
    let (end, a, b) = best?;
    c.p = end;
    Some((a, b))
}

/// The unit a relative amount counts in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Unit {
    Us,
    Sec,
    Min,
    Hour,
    Day,
    Week,
    Fortnight,
    Month,
    Year,
    /// `weekday` / `weekdays` — business days.
    BusinessDay,
    /// A named weekday; the payload is `0` = Sunday … `6` = Saturday.
    Weekday(i64),
}

/// `reltextunit`. `msec` / `millisecond` are milliseconds, which php folds
/// into the microsecond field.
const REL_UNITS: [(&[u8], Unit, i64); 32] = [
    (b"microseconds", Unit::Us, 1),
    (b"milliseconds", Unit::Us, 1000),
    (b"microsecond", Unit::Us, 1),
    (b"millisecond", Unit::Us, 1000),
    (b"forthnights", Unit::Fortnight, 1),
    (b"forthnight", Unit::Fortnight, 1),
    (b"fortnights", Unit::Fortnight, 1),
    (b"fortnight", Unit::Fortnight, 1),
    (b"weekdays", Unit::BusinessDay, 1),
    (b"weekday", Unit::BusinessDay, 1),
    (b"seconds", Unit::Sec, 1),
    (b"minutes", Unit::Min, 1),
    (b"second", Unit::Sec, 1),
    (b"minute", Unit::Min, 1),
    (b"months", Unit::Month, 1),
    (b"usecs", Unit::Us, 1),
    (b"msecs", Unit::Us, 1000),
    (b"hours", Unit::Hour, 1),
    (b"weeks", Unit::Week, 1),
    (b"years", Unit::Year, 1),
    (b"month", Unit::Month, 1),
    (b"usec", Unit::Us, 1),
    (b"msec", Unit::Us, 1000),
    (b"secs", Unit::Sec, 1),
    (b"mins", Unit::Min, 1),
    (b"days", Unit::Day, 1),
    (b"hour", Unit::Hour, 1),
    (b"week", Unit::Week, 1),
    (b"year", Unit::Year, 1),
    (b"sec", Unit::Sec, 1),
    (b"min", Unit::Min, 1),
    (b"day", Unit::Day, 1),
];

/// `reltextunit`: a unit word or a weekday name. The singular `week` is not
/// one of them — php spells that rule out separately, so `+1 week` works but
/// `first week` does not parse at all (`first weeks` does).
fn rel_unit(c: &mut Cur, bare_week: bool) -> Option<(Unit, i64)> {
    let mut best: Option<(usize, Unit, i64)> = None;
    for (name, unit, mult) in REL_UNITS.iter() {
        if !bare_week && *name == b"week" {
            continue;
        }
        let mut t = *c;
        if t.eat_word(name) && best.is_none_or(|(len, _, _)| t.p > len) {
            best = Some((t.p, *unit, *mult));
        }
    }
    let mut t = *c;
    if let Some(w) = day_text(&mut t) {
        if best.is_none_or(|(len, _, _)| t.p > len) {
            best = Some((t.p, Unit::Weekday(w), 1));
        }
    }
    let (end, unit, mult) = best?;
    c.p = end;
    Some((unit, mult))
}

/// `relnumber`: a signed integer with php's forgiving sign run — `+-1` is
/// `-1`, `--1` is `+1` — and spaces allowed anywhere before the digits.
fn rel_number(c: &mut Cur) -> Option<i64> {
    let save = c.p;
    let mut sign = 1i64;
    loop {
        c.spaces();
        if c.eat(b'+') {
            continue;
        }
        if c.eat(b'-') {
            sign = -sign;
            continue;
        }
        break;
    }
    match c.digits(1, 19) {
        Some((v, _)) => Some(sign.saturating_mul(v)),
        None => {
            c.p = save;
            None
        }
    }
}

// ---- tokens ------------------------------------------------------------------

/// What one matched rule contributes. A rule may produce several tokens
/// (`Feb 13 23:31` is one match but a date *and* a time).
#[derive(Clone, Debug)]
enum Tok {
    /// A separator, or `now`: matched and ignored.
    Nop,
    /// `@<seconds>` — an instant rather than a wall clock.
    Stamp(i64, i64),
    /// A calendar date; every part the rule did not carry stays `None`.
    /// The year keeps its digit count, because php widens a year literal of
    /// fewer than four digits (`23` is 2023, `0069` stays year 69).
    Date {
        y: Option<(i64, usize)>,
        m: Option<i64>,
        d: Option<i64>,
    },
    /// A wall-clock time. `us` is `None` for the readings php leaves the
    /// microsecond field unset for, which `date_parse()` reports as
    /// `fraction => false`.
    Time {
        h: i64,
        i: i64,
        s: i64,
        us: Option<i64>,
    },
    /// A bare four-digit clock reading (`1230`), which php re-reads as a
    /// year once a time is already known.
    GnuNoColon {
        h: i64,
        i: i64,
    },
    /// A timezone, still as written: an offset resolves immediately, a name
    /// is looked up when the token is applied.
    Offset(i32),
    Name(Vec<u8>),
    /// A relative amount in one unit.
    Rel {
        unit: Unit,
        amount: i64,
        behavior: i64,
        keep_time: bool,
    },
    /// `<ordinal> <weekday> of` — the n-th weekday of the month.
    NthWeekdayOf {
        n: i64,
        weekday: i64,
    },
    /// `first day of` (1) / `last day of` (2).
    FirstLastDayOf(u8),
    /// A bare four-digit year, which php does not count as a date.
    Year(i64),
    /// `ago`: flip every relative amount collected so far.
    Ago,
    /// `today` / `midnight` — drop the time. With `Some(h)` (`noon`) the
    /// hour is then set and the time counts as specified, so a second time
    /// in the string is a double specification.
    ResetTime(Option<i64>),
    /// `tomorrow` / `yesterday`: a day offset that also drops the time.
    RelDays(i64),
    /// `this week` and friends: `n` weeks, walking to Monday of that week.
    Week(i64),
}

// ---- rules -------------------------------------------------------------------

/// `[ \t.-]*`, the filler php allows inside textual dates.
fn date_seps(c: &mut Cur) -> usize {
    c.eat_while(b" \t.-")
}

/// `[ .,\t]+` and the line endings php treats as whitespace.
fn r_separator(c: &mut Cur) -> Option<Vec<Tok>> {
    let n = c.eat_while(b" \t.,\r\n");
    (n > 0).then(|| vec![Tok::Nop])
}

/// `now`.
fn r_now(c: &mut Cur) -> Option<Vec<Tok>> {
    c.eat_word(b"now").then(|| vec![Tok::Nop])
}

/// `noon`.
fn r_noon(c: &mut Cur) -> Option<Vec<Tok>> {
    c.eat_word(b"noon").then(|| vec![Tok::ResetTime(Some(12))])
}

/// `midnight` / `today`.
fn r_midnight_today(c: &mut Cur) -> Option<Vec<Tok>> {
    (c.eat_word(b"midnight") || c.eat_word(b"today")).then(|| vec![Tok::ResetTime(None)])
}

/// `tomorrow`.
fn r_tomorrow(c: &mut Cur) -> Option<Vec<Tok>> {
    c.eat_word(b"tomorrow").then(|| vec![Tok::RelDays(1)])
}

/// `yesterday`.
fn r_yesterday(c: &mut Cur) -> Option<Vec<Tok>> {
    c.eat_word(b"yesterday").then(|| vec![Tok::RelDays(-1)])
}

/// `ago`.
fn r_ago(c: &mut Cur) -> Option<Vec<Tok>> {
    c.eat_word(b"ago").then(|| vec![Tok::Ago])
}

/// `@<seconds>` with an optional fractional part.
fn r_timestamp(c: &mut Cur) -> Option<Vec<Tok>> {
    if !c.eat(b'@') {
        return None;
    }
    let neg = c.eat(b'-');
    let (v, _) = c.digits(1, 19)?;
    let us = fraction(c).unwrap_or(0);
    let secs = if neg { -v } else { v };
    Some(vec![Tok::Stamp(secs, if neg { -us } else { us })])
}

/// `first day of` / `last day of`.
fn r_firstlastdayof(c: &mut Cur) -> Option<Vec<Tok>> {
    let which = if c.eat_word(b"first") {
        1
    } else if c.eat_word(b"last") {
        2
    } else {
        return None;
    };
    if c.spaces() == 0 || !c.eat_word(b"day") {
        return None;
    }
    if c.spaces() == 0 || !c.eat_word(b"of") {
        return None;
    }
    c.spaces();
    Some(vec![Tok::FirstLastDayOf(which)])
}

/// `<ordinal> <weekday> of` — `first monday of january 2021`. php has no
/// numeric form of this: `1 monday of january 2021` does not parse.
fn r_nth_weekday_of(c: &mut Cur) -> Option<Vec<Tok>> {
    let (n, _) = rel_text(c)?;
    if c.spaces() == 0 {
        return None;
    }
    let weekday = day_text(c)?;
    if c.spaces() == 0 || !c.eat_word(b"of") {
        return None;
    }
    c.spaces();
    Some(vec![Tok::NthWeekdayOf { n, weekday }])
}

/// `back of <hour>` / `front of <hour>` — a quarter past and a quarter to.
fn r_backfrontof(c: &mut Cur) -> Option<Vec<Tok>> {
    let back = if c.eat_word(b"back") {
        true
    } else if c.eat_word(b"front") {
        false
    } else {
        return None;
    };
    if c.spaces() == 0 || !c.eat_word(b"of") || c.spaces() == 0 {
        return None;
    }
    let h = hour24(c)?;
    let save = c.p;
    c.spaces();
    let h = match meridian(c) {
        Some(pm) => apply_meridian(h, pm),
        None => {
            c.p = save;
            h
        }
    };
    let (h, i) = if back { (h, 15) } else { (h - 1, 45) };
    Some(vec![Tok::Time {
        h,
        i,
        s: 0,
        us: Some(0),
    }])
}

/// `this week` / `next week` / `last week`: Monday of that week, with the
/// time kept.
fn r_relativetextweek(c: &mut Cur) -> Option<Vec<Tok>> {
    // Only the four positional words take this form; `first week` is an
    // ordinary `+1 week`.
    let n = if c.eat_word(b"previous") {
        -1
    } else if c.eat_word(b"last") {
        -1
    } else if c.eat_word(b"next") {
        1
    } else if c.eat_word(b"this") {
        0
    } else {
        return None;
    };
    if c.spaces() == 0 || !c.eat_word(b"week") {
        return None;
    }
    Some(vec![Tok::Week(n)])
}

/// `next monday`, `last month`, `third friday` — an ordinal word and a unit.
fn r_relativetext(c: &mut Cur) -> Option<Vec<Tok>> {
    let (n, behavior) = rel_text(c)?;
    if c.spaces() == 0 {
        return None;
    }
    let (unit, mult) = rel_unit(c, false)?;
    Some(vec![Tok::Rel {
        unit,
        amount: n * mult,
        behavior,
        keep_time: false,
    }])
}

/// `+1 week`, `3 days`, `2 weekdays` — a signed number and a unit. The
/// numeric form keeps the time, which is why `+1 weekday` moves the day but
/// not the clock.
fn r_relative(c: &mut Cur) -> Option<Vec<Tok>> {
    let n = rel_number(c)?;
    c.spaces();
    let (unit, mult) = rel_unit(c, true)?;
    Some(vec![Tok::Rel {
        unit,
        amount: n * mult,
        // The numeric form walks like a bare weekday name, not like `next`:
        // `next friday` on a Friday is a week on, `1 friday` is today.
        behavior: 1,
        keep_time: true,
    }])
}

/// A bare weekday name.
fn r_daytext(c: &mut Cur) -> Option<Vec<Tok>> {
    let w = day_text(c)?;
    Some(vec![Tok::Rel {
        unit: Unit::Weekday(w),
        amount: 0,
        behavior: 1,
        keep_time: false,
    }])
}

/// A bare `weekday` / `weekdays`. php reads these through the same table as
/// the weekday names and then uses the table's *multiplier* as the weekday
/// number, so a bare `weekday` walks to the next **Monday** rather than
/// stepping one business day: from a Tuesday it is the following Monday,
/// where `next weekday` is the Wednesday.
fn r_dayspecial(c: &mut Cur) -> Option<Vec<Tok>> {
    (c.eat_word(b"weekdays") || c.eat_word(b"weekday")).then(|| {
        vec![Tok::Rel {
            unit: Unit::Weekday(1),
            amount: 0,
            behavior: 1,
            keep_time: false,
        }]
    })
}

// ---- time rules --------------------------------------------------------------

/// Every clock shape php knows, as one rule: the longest reading wins, so
/// `12:30:45.5` is a fractional time rather than `12:30:45` with a stray
/// `.5`. The optional leading `t` is ISO 8601's date/time separator, which
/// php also accepts on a bare time (`T12:30`).
fn match_time(c: &Cur, need_sep: bool) -> Option<(usize, Tok)> {
    let mut best: Option<(usize, Tok)> = None;
    let keep = |t: &Cur, tok: Tok, best: &mut Option<(usize, Tok)>| {
        if best.as_ref().is_none_or(|(len, _)| t.p > *len) {
            *best = Some((t.p, tok));
        }
    };

    // `hour12:minutelz:secondlz[:.]<digits> <meridian>` — SQL Server.
    let mut t = *c;
    if let Some(h) = hour12(&mut t) {
        if t.eat(b':') {
            if let Some(i) = minutelz(&mut t) {
                if t.eat(b':') {
                    if let Some(s) = secondlz(&mut t) {
                        if t.eat_any(b":.") {
                            let start = t.p;
                            let mut us = 0i64;
                            let mut n = 0;
                            while t.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                                if n < 6 {
                                    us = us * 10 + (t.peek().unwrap() - b'0') as i64;
                                }
                                n += 1;
                                t.p += 1;
                            }
                            if t.p > start {
                                let mut k = n;
                                while k < 6 {
                                    us *= 10;
                                    k += 1;
                                }
                                t.spaces();
                                if let Some(pm) = meridian(&mut t) {
                                    keep(
                                        &t,
                                        Tok::Time {
                                            h: apply_meridian(h, pm),
                                            i,
                                            s,
                                            us: Some(us),
                                        },
                                        &mut best,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 12-hour clocks: `3pm`, `1:30 pm`, `11:59:59 PM`.
    let mut t = *c;
    if let Some(h) = hour12(&mut t) {
        let after_hour = t;
        let mut t2 = t;
        t2.spaces();
        if !need_sep {
            if let Some(pm) = meridian(&mut t2) {
                keep(
                    &t2,
                    Tok::Time {
                        h: apply_meridian(h, pm),
                        i: 0,
                        s: 0,
                        us: Some(0),
                    },
                    &mut best,
                );
            }
        }
        let mut t = after_hour;
        if t.eat_any(b":.") {
            if let Some(i) = minutelz(&mut t) {
                let after_min = t;
                let mut t2 = t;
                t2.spaces();
                if let Some(pm) = meridian(&mut t2) {
                    keep(
                        &t2,
                        Tok::Time {
                            h: apply_meridian(h, pm),
                            i,
                            s: 0,
                            us: Some(0),
                        },
                        &mut best,
                    );
                }
                let mut t = after_min;
                if t.eat_any(b":.") {
                    if let Some(s) = secondlz(&mut t) {
                        t.spaces();
                        if let Some(pm) = meridian(&mut t) {
                            keep(
                                &t,
                                Tok::Time {
                                    h: apply_meridian(h, pm),
                                    i,
                                    s,
                                    us: Some(0),
                                },
                                &mut best,
                            );
                        }
                    }
                }
            }
        }
    }

    // 24-hour clocks, with and without separators. Both alternatives below
    // restart from the cursor past an ISO `T`, so it is saved once.
    let mut after_t = *c;
    let _ = after_t.eat(b'T') || after_t.eat(b't');

    let mut t = after_t;
    if let Some(h) = hour24(&mut t) {
        if t.eat_any(b":.") {
            if let Some(i) = minute(&mut t) {
                keep(
                    &t,
                    Tok::Time {
                        h,
                        i,
                        s: 0,
                        us: Some(0),
                    },
                    &mut best,
                );
                if t.eat_any(b":.") {
                    if let Some(s) = second(&mut t) {
                        keep(
                            &t,
                            Tok::Time {
                                h,
                                i,
                                s,
                                us: Some(0),
                            },
                            &mut best,
                        );
                        if let Some(us) = fraction(&mut t) {
                            keep(
                                &t,
                                Tok::Time {
                                    h,
                                    i,
                                    s,
                                    us: Some(us),
                                },
                                &mut best,
                            );
                        }
                    }
                }
            }
        }
    }

    if !need_sep {
        let mut t = after_t;
        if let Some(h) = hour24lz(&mut t) {
            if let Some(i) = minutelz(&mut t) {
                keep(&t, Tok::GnuNoColon { h, i }, &mut best);
                if let Some(s) = secondlz(&mut t) {
                    keep(
                        &t,
                        Tok::Time {
                            h,
                            i,
                            s,
                            us: Some(0),
                        },
                        &mut best,
                    );
                }
            }
        }
    }

    best
}

/// The time rules as one scanner rule.
fn r_time(c: &mut Cur) -> Option<Vec<Tok>> {
    let (end, tok) = match_time(c, false)?;
    c.p = end;
    Some(vec![tok])
}

// ---- date rules --------------------------------------------------------------

/// `year4-monthlz-daylz`, with the leading sign ISO 8601 allows.
fn r_iso8601date4(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4_signed(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let m = monthlz(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let d = daylz(c)?;
    Some(vec![Tok::Date {
        y: Some((y, 4)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `year-month-day` with any digit count (`8-7-1`, `2009-2-13`).
fn r_gnudateshort(c: &mut Cur) -> Option<Vec<Tok>> {
    let (y, n) = year_nr(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let m = month_nr(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let d = day_nr(c)?;
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `year4-month` — the day becomes the first.
fn r_gnudateshorter(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let m = month_nr(c)?;
    Some(vec![Tok::Date {
        y: Some((y, 4)),
        m: Some(m),
        d: Some(1),
    }])
}

/// `year4/monthlz/daylz` and `year4/month/day`.
fn r_dateslash(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    if !c.eat(b'/') {
        return None;
    }
    let m = month_nr(c)?;
    if !c.eat(b'/') {
        return None;
    }
    let d = day_nr(c)?;
    c.eat(b'/');
    Some(vec![Tok::Date {
        y: Some((y, 4)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `month/day/year` — the American order.
fn r_american(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_nr(c)?;
    if !c.eat(b'/') {
        return None;
    }
    let d = day_nr(c)?;
    if !c.eat(b'/') {
        return None;
    }
    let (y, n) = year_nr(c)?;
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `month/day` — the year comes from the base time.
fn r_americanshort(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_nr(c)?;
    if !c.eat(b'/') {
        return None;
    }
    let d = day_nr(c)?;
    Some(vec![Tok::Date {
        y: None,
        m: Some(m),
        d: Some(d),
    }])
}

/// `day.month.year` and `day-month-year` — the European order.
fn r_pointeddate(c: &mut Cur) -> Option<Vec<Tok>> {
    let d = day_nr(c)?;
    let sep1 = c.peek();
    if !c.eat_any(b".\t-") {
        return None;
    }
    let m = month_nr(c)?;
    let sep2 = c.peek();
    if !c.eat_any(b".-") {
        return None;
    }
    // A four-digit year takes dashes as well (`1-7-2008`); a two-digit one
    // does not, which is why `31.01.70` is a date but `31-01-70` is not.
    let (y, n) = match c.digits(4, 4) {
        Some(v) => v,
        None => {
            if sep1 == Some(b'-') || sep2 == Some(b'-') {
                return None;
            }
            c.digits(2, 2)?
        }
    };
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `day monthtext year` — `13 February 2009`, `13-Feb-2009`.
fn r_datefull(c: &mut Cur) -> Option<Vec<Tok>> {
    let d = day_nr(c)?;
    date_seps(c);
    let m = month_text(c)?;
    date_seps(c);
    let (y, n) = year_nr(c)?;
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `day monthtext` — the year comes from the base time.
fn r_datenoyearrev(c: &mut Cur) -> Option<Vec<Tok>> {
    let d = day_nr(c)?;
    date_seps(c);
    let m = month_text(c)?;
    Some(vec![Tok::Date {
        y: None,
        m: Some(m),
        d: Some(d),
    }])
}

/// `monthtext day year` — `February 13, 2009`, `Feb 13th 2009`.
fn r_datetextual(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_text(c)?;
    date_seps(c);
    let d = day_nr(c)?;
    if c.eat_while(b",.stndrh\t ") == 0 {
        return None;
    }
    let (y, n) = year_nr(c)?;
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `monthtext day` — `Feb 13`, `July 1st`.
fn r_datenoyear(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_text(c)?;
    date_seps(c);
    let d = day_nr(c)?;
    Some(vec![Tok::Date {
        y: None,
        m: Some(m),
        d: Some(d),
    }])
}

/// `monthtext day <time>` — php scans these together so that `Feb 13 23:31`
/// is a time rather than the year 23 followed by a stray `:31`.
fn r_datenoyear_time(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_text(c)?;
    date_seps(c);
    let d = day_nr(c)?;
    if c.eat_while(b",.stndrh\t ") == 0 {
        return None;
    }
    // Only the separated clock shapes join a date this way: `Feb 13 1230` is
    // the year 1230, not half past twelve.
    let (end, time) = match_time(c, true)?;
    c.p = end;
    Some(vec![
        Tok::Date {
            y: None,
            m: Some(m),
            d: Some(d),
        },
        time,
    ])
}

/// `monthtext year4` — `July 2008`.
fn r_datenoday(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_text(c)?;
    date_seps(c);
    let y = year4(c)?;
    Some(vec![Tok::Date {
        y: Some((y, 4)),
        m: Some(m),
        d: Some(1),
    }])
}

/// `year4 monthtext` — `2008 July`.
fn r_datenodayrev(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    date_seps(c);
    let m = month_text(c)?;
    Some(vec![Tok::Date {
        y: Some((y, 4)),
        m: Some(m),
        d: Some(1),
    }])
}

/// A bare four-digit year. php sets the year *without* marking the date as
/// given, so `1999` keeps the base time and the base month and day — unlike
/// `2009`, which the clock rules read as 20:09 and win by being listed first.
fn r_year4(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    Some(vec![Tok::Year(y)])
}

/// A bare month name — the day and year come from the base time.
fn r_monthtext(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_text_as(c, MonthSpelling::NoRoman)?;
    Some(vec![Tok::Date {
        y: None,
        m: Some(m),
        d: None,
    }])
}

/// `year4monthlzdaylz` — `20080701`.
fn r_datenocolon(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    let m = monthlz(c)?;
    let d = daylz(c)?;
    Some(vec![Tok::Date {
        y: Some((y, 4)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `year4[.]dayofyear` — PostgreSQL's ordinal date (`2008182`, `2008.182`).
fn r_pgydotd(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    c.eat(b'.');
    let doy = day_of_year_nr(c)?;
    let (yy, mm, dd) = civil::civil_from_days(civil::days_from_civil(y, 1, 1) + doy - 1);
    Some(vec![Tok::Date {
        y: Some((yy, 4)),
        m: Some(mm as i64),
        d: Some(dd as i64),
    }])
}

/// `monthabbr-daylz-year` — PostgreSQL's `Jul-01-2008`.
fn r_pgtextshort(c: &mut Cur) -> Option<Vec<Tok>> {
    let m = month_text_as(c, MonthSpelling::AbbrOnly)?;
    if !c.eat(b'-') {
        return None;
    }
    let d = daylz(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let (y, n) = year_nr(c)?;
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `year-monthabbr-daylz` — PostgreSQL's `2008-Jul-01`.
fn r_pgtextreverse(c: &mut Cur) -> Option<Vec<Tok>> {
    let (y, n) = year_nr(c)?;
    if !c.eat(b'-') {
        return None;
    }
    let m = month_text_as(c, MonthSpelling::AbbrOnly)?;
    if !c.eat(b'-') {
        return None;
    }
    let d = daylz(c)?;
    Some(vec![Tok::Date {
        y: Some((y, n)),
        m: Some(m),
        d: Some(d),
    }])
}

/// `year4:monthlz:daylz hour:minute:second` — the EXIF timestamp.
fn r_exif(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    if !c.eat(b':') {
        return None;
    }
    let m = monthlz(c)?;
    if !c.eat(b':') {
        return None;
    }
    let d = daylz(c)?;
    if !c.eat(b' ') {
        return None;
    }
    let h = hour24(c)?;
    if !c.eat(b':') {
        return None;
    }
    let i = minutelz(c)?;
    if !c.eat(b':') {
        return None;
    }
    let s = secondlz(c)?;
    Some(vec![
        Tok::Date {
            y: Some((y, 4)),
            m: Some(m),
            d: Some(d),
        },
        Tok::Time {
            h,
            i,
            s,
            us: Some(0),
        },
    ])
}

/// `year4[-]W<week>[-]<dow>` — the ISO 8601 week date. The day part is
/// `1`–`7` with Monday first; php also takes `0`, which lands on the Sunday
/// *before* the week.
fn r_isoweek(c: &mut Cur) -> Option<Vec<Tok>> {
    let y = year4(c)?;
    c.eat(b'-');
    if !(c.eat(b'W') || c.eat(b'w')) {
        return None;
    }
    let week = week_of_year(c)?;
    let save = c.p;
    c.eat(b'-');
    let dow = match c.peek() {
        Some(ch) if (b'0'..=b'7').contains(&ch) => {
            c.p += 1;
            (ch - b'0') as i64
        }
        _ => {
            c.p = save;
            1
        }
    };
    // Monday of ISO week 1 is the Monday of the week holding 4 January.
    let jan4 = civil::days_from_civil(y, 1, 4);
    let iso_dow = (jan4 + 3).rem_euclid(7) + 1; // 1 = Monday
    let monday = jan4 - (iso_dow - 1) + (week - 1) * 7;
    let (yy, mm, dd) = civil::civil_from_days(monday + dow - 1);
    Some(vec![Tok::Date {
        y: Some((yy, 4)),
        m: Some(mm as i64),
        d: Some(dd as i64),
    }])
}

// ---- timezone rules ----------------------------------------------------------

/// `[GMT]±hh[:]mm` — a numeric offset. php takes the `GMT` prefix in upper
/// case only, and reads `+25:00` as `+02:05` plus three stray characters
/// because the hour class stops at `24`.
fn r_tzcorrection(c: &mut Cur) -> Option<Vec<Tok>> {
    let save = c.p;
    if c.peek() == Some(b'G') {
        if !c.eat_word(b"GMT") {
            return None;
        }
        if c.s[save..c.p] != *b"GMT" {
            c.p = save;
            return None;
        }
    }
    let sign: i32 = if c.eat(b'+') {
        1
    } else if c.eat(b'-') {
        -1
    } else {
        c.p = save;
        return None;
    };
    let Some(h) = hour24(c) else {
        c.p = save;
        return None;
    };
    c.eat(b':');
    let m = minute(c).unwrap_or(0);
    Some(vec![Tok::Offset(sign * (h as i32 * 3600 + m as i32 * 60))])
}

/// `tz` — a bracketed or bare abbreviation of up to six letters, or an IANA
/// identifier (`Europe/Berlin`, `America/Argentina/Buenos_Aires`).
fn r_tz(c: &mut Cur) -> Option<Vec<Tok>> {
    let mut best: Option<(usize, Vec<u8>)> = None;

    // `[A-Z][a-z]+([_/-][A-Za-z]+)+`
    let mut t = *c;
    if t.peek().is_some_and(|ch| ch.is_ascii_uppercase()) {
        t.p += 1;
        let lower = t.eat_while(b"abcdefghijklmnopqrstuvwxyz");
        if lower > 0 {
            let mut parts = 0;
            loop {
                let mark = t.p;
                if !t.eat_any(b"_/-") {
                    break;
                }
                let n = t.eat_while(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz");
                if n == 0 {
                    t.p = mark;
                    break;
                }
                parts += 1;
            }
            if parts > 0 {
                best = Some((t.p, c.s[c.p..t.p].to_vec()));
            }
        }
    }

    // `"("? [A-Za-z]{1,6} ")"?`
    let mut t = *c;
    let open = t.eat(b'(');
    let start = t.p;
    let mut n = 0;
    while n < 6 && t.peek().is_some_and(|ch| ch.is_ascii_alphabetic()) {
        t.p += 1;
        n += 1;
    }
    if n > 0 {
        let name = c.s[start..t.p].to_vec();
        if open {
            t.eat(b')');
        }
        if best.as_ref().is_none_or(|(len, _)| t.p > *len) {
            best = Some((t.p, name));
        }
    }

    let (end, name) = best?;
    c.p = end;
    Some(vec![Tok::Name(name)])
}

// ---- the rule table ----------------------------------------------------------

/// Every rule, in the order ties are broken. Two constraints in here are
/// load-bearing: the time rules come before the date rules, so `01.07.08` is
/// 01:07:08 rather than 1 July 2008; and the word rules come before the
/// timezone rule, so `sat` is Saturday rather than an unknown abbreviation
/// while `sats`, one character longer, is the unknown abbreviation.
type Rule = fn(&mut Cur) -> Option<Vec<Tok>>;

static RULES: &[Rule] = &[
    r_timestamp,
    r_firstlastdayof,
    r_nth_weekday_of,
    r_backfrontof,
    r_now,
    r_noon,
    r_midnight_today,
    r_tomorrow,
    r_yesterday,
    r_ago,
    r_relativetextweek,
    r_relativetext,
    r_relative,
    r_dayspecial,
    r_daytext,
    r_time,
    r_exif,
    r_isoweek,
    r_datenoyear_time,
    r_datetextual,
    r_datenoday,
    r_datenodayrev,
    r_datenoyear,
    r_datenoyearrev,
    r_datefull,
    r_iso8601date4,
    r_gnudateshort,
    r_gnudateshorter,
    r_dateslash,
    r_american,
    r_americanshort,
    r_pointeddate,
    r_pgtextshort,
    r_pgtextreverse,
    r_datenocolon,
    r_pgydotd,
    r_monthtext,
    r_year4,
    r_tzcorrection,
    r_tz,
    r_separator,
];

// ---- the scanner --------------------------------------------------------------

/// php widens a year literal shorter than four digits: `23` is 2023 and `70`
/// is 1970, but `0069` stays year 69.
fn widen_year(y: i64, digits: usize) -> i64 {
    if digits >= 4 || !(0..100).contains(&y) {
        y
    } else if y < 70 {
        y + 2000
    } else {
        y + 1900
    }
}

/// Drop the time the way php's `TIMELIB_UNHAVE_TIME` does: the fields go to
/// zero *and* stop counting as specified, so nothing is filled in from the
/// base time later.
fn unhave_time(p: &mut Parsed) {
    p.h = Some(0);
    p.i = Some(0);
    p.s = Some(0);
    p.us = Some(0);
    p.have_time = 0;
}

/// Apply one token. Returns `false` when the parse must stop — php treats a
/// second date or a second time as a fatal error, but a second timezone only
/// as a warning.
fn apply(p: &mut Parsed, pos: usize, t: Tok) -> bool {
    match t {
        Tok::Nop => {}
        Tok::Stamp(secs, us) => {
            // php does not store `@N` as an instant: it resets the fields to
            // the epoch and files the seconds as a *relative* amount, which
            // is why `@0 +1 hour` is 3600 and why `@0 2009-02-13` keeps the
            // later date instead of reporting a double date.
            p.y = Some(1970);
            p.m = Some(1);
            p.d = Some(1);
            p.h = Some(0);
            p.i = Some(0);
            p.s = Some(0);
            p.us = Some(0);
            p.have_date = false;
            p.have_time = 0;
            p.have_relative = true;
            p.rel.s += secs;
            p.rel.us += us;
            if p.have_zone {
                p.warnings.push((pos, "Double timezone specification"));
            } else {
                p.have_zone = true;
                p.zone = Some(Tz::Offset(0));
            }
        }
        Tok::Date { y, m, d } => {
            if p.have_date {
                p.errors.push((pos, "Double date specification"));
                return false;
            }
            p.have_date = true;
            if let Some((v, n)) = y {
                p.y = Some(widen_year(v, n));
            }
            if let Some(v) = m {
                p.m = Some(v);
            }
            if let Some(v) = d {
                p.d = Some(v);
            }
        }
        Tok::Year(y) => p.y = Some(y),
        Tok::Time { h, i, s, us } => {
            if p.have_time != 0 {
                p.errors.push((pos, "Double time specification"));
                return false;
            }
            p.have_time = 1;
            p.h = Some(h);
            p.i = Some(i);
            p.s = Some(s);
            p.us = us;
        }
        Tok::GnuNoColon { h, i } => match p.have_time {
            0 => {
                p.have_time = 1;
                p.h = Some(h);
                p.i = Some(i);
                p.s = Some(0);
            }
            1 => {
                p.have_time = 2;
                p.y = Some(h * 100 + i);
            }
            _ => {
                p.errors.push((pos, "Double time specification"));
                return false;
            }
        },
        Tok::Offset(secs) => {
            if p.have_zone {
                p.warnings.push((pos, "Double timezone specification"));
            } else {
                p.have_zone = true;
                p.zone = Some(Tz::Offset(secs));
            }
        }
        Tok::Name(name) => {
            let text = String::from_utf8_lossy(&name).into_owned();
            let zone = lookup_zone(&text);
            if p.have_zone {
                // php only warns here, even when the second name is unknown.
                p.warnings.push((pos, "Double timezone specification"));
            } else {
                // A name the database does not know still counts as a
                // timezone having been given: `date_parse("xyz")` reports
                // `is_localtime` true with `zone_type` 0.
                p.have_zone = true;
                match zone {
                    Some(z) => p.zone = Some(z),
                    None => p
                        .errors
                        .push((pos, "The timezone could not be found in the database")),
                }
            }
        }
        Tok::Rel {
            unit,
            amount,
            behavior,
            keep_time,
        } => {
            p.have_relative = true;
            match unit {
                Unit::Us => p.rel.us += amount,
                Unit::Sec => p.rel.s += amount,
                Unit::Min => p.rel.i += amount,
                Unit::Hour => p.rel.h += amount,
                Unit::Day => p.rel.d += amount,
                Unit::Week => p.rel.d += amount * 7,
                Unit::Fortnight => p.rel.d += amount * 14,
                Unit::Month => p.rel.m += amount,
                Unit::Year => p.rel.y += amount,
                Unit::BusinessDay => {
                    p.rel.special_weekday = Some(p.rel.special_weekday.unwrap_or(0) + amount);
                    if !keep_time {
                        unhave_time(p);
                    }
                }
                Unit::Weekday(w) => {
                    // `next monday` is this week's Monday, `third monday` two
                    // weeks after that: the ordinal counts from one, not zero.
                    p.rel.d += if amount > 0 { amount - 1 } else { amount } * 7;
                    p.rel.weekday = Some(w);
                    if p.rel.weekday_behavior != 2 {
                        p.rel.weekday_behavior = behavior;
                    }
                    if !keep_time {
                        unhave_time(p);
                    }
                }
            }
        }
        Tok::NthWeekdayOf { n, weekday } => {
            p.have_relative = true;
            p.rel.nth_weekday = Some((n, weekday));
            unhave_time(p);
        }
        Tok::FirstLastDayOf(which) => {
            p.have_relative = true;
            p.rel.first_last_day_of = which;
        }
        Tok::Ago => {
            let r = &mut p.rel;
            r.y = -r.y;
            r.m = -r.m;
            r.d = -r.d;
            r.h = -r.h;
            r.i = -r.i;
            r.s = -r.s;
            // php leaves the microsecond amount alone here, so
            // `1 msec ago` is a *forward* millisecond.

            // A weekday walk flips direction; Sunday has no negative zero, so
            // php writes it as -7.
            if let Some(w) = r.weekday {
                r.weekday = Some(if w == 0 { -7 } else { -w });
            }
            if let Some(a) = r.special_weekday {
                r.special_weekday = Some(-a);
            }
        }
        Tok::ResetTime(hour) => {
            // `noon` drops whatever time was there and puts its own in, so
            // `3pm noon` is midday while `noon 3pm` is a double time.
            unhave_time(p);
            if let Some(h) = hour {
                p.have_time = 1;
                p.h = Some(h);
            }
        }
        Tok::RelDays(n) => {
            p.have_relative = true;
            p.rel.d += n;
            unhave_time(p);
        }
        Tok::Week(n) => {
            p.have_relative = true;
            p.rel.d += n * 7;
            if p.rel.weekday.is_none() {
                p.rel.weekday = Some(1);
            }
            p.rel.weekday_behavior = 2;
        }
    }
    true
}

/// Resolve a timezone word the way php's parser does: the literal `UTC` is
/// an identifier, any other spelling is looked up as an abbreviation first
/// and only then as an identifier.
fn lookup_zone(name: &str) -> Option<Tz> {
    if name == "UTC" {
        return Some(Tz::Id("UTC".to_string()));
    }
    if let Some((offset, dst)) = tz::lookup_abbr(name) {
        return Some(Tz::Abbr {
            name: name.to_ascii_uppercase(),
            offset,
            dst,
        });
    }
    if tz::is_known_id(name) {
        return Some(Tz::Id(name.to_string()));
    }
    None
}

/// Scan one string. Nothing here fails outright: an unmatched position is
/// recorded and skipped, because `date_parse()` reports every error it met
/// and `strtotime()` only asks whether the list is empty.
pub(crate) fn parse(input: &[u8]) -> Parsed {
    let mut p = Parsed::default();
    if input.is_empty() {
        p.errors.push((0, "Empty string"));
        return p;
    }
    let mut pos = 0usize;
    while pos < input.len() {
        let rest = &input[pos..];
        let mut best: Option<(usize, Vec<Tok>)> = None;
        for rule in RULES {
            let mut c = Cur::new(rest);
            if let Some(toks) = rule(&mut c) {
                if c.p > 0 && best.as_ref().is_none_or(|(len, _)| c.p > *len) {
                    best = Some((c.p, toks));
                }
            }
        }
        match best {
            Some((len, toks)) => {
                for t in toks {
                    if !apply(&mut p, pos, t) {
                        return p;
                    }
                }
                pos += len;
            }
            None => {
                p.errors.push((pos, "Unexpected character"));
                pos += 1;
            }
        }
    }
    p
}

// ---- resolution ---------------------------------------------------------------

/// The broken-down civil time the resolution pipeline works on. Fields are
/// allowed to leave their ranges between steps — `last day of` sets the day
/// to zero and the month to the next one — and [`normalize`] puts them back.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Civil {
    pub(crate) y: i64,
    pub(crate) mo: i64,
    pub(crate) d: i64,
    pub(crate) h: i64,
    pub(crate) mi: i64,
    pub(crate) s: i64,
    pub(crate) us: i64,
}

/// Carry every field back into range. The month is carried before the day, so
/// that `2021-01-31 +1 month` becomes 2021-02-31 and only then 2021-03-03 —
/// php's answer, and the reason `+1 month` is not "the same day next month".
pub(crate) fn normalize(c: &mut Civil) {
    let carry = c.us.div_euclid(1_000_000);
    c.us = c.us.rem_euclid(1_000_000);
    c.s += carry;
    let carry = c.s.div_euclid(60);
    c.s = c.s.rem_euclid(60);
    c.mi += carry;
    let carry = c.mi.div_euclid(60);
    c.mi = c.mi.rem_euclid(60);
    c.h += carry;
    let carry = c.h.div_euclid(24);
    c.h = c.h.rem_euclid(24);
    c.d += carry;
    let (y, mo) = civil::normalize_month(c.y, c.mo);
    let days = civil::days_from_civil(y, mo as i64, c.d);
    let (y, mo, d) = civil::civil_from_days(days);
    c.y = y;
    c.mo = mo as i64;
    c.d = d as i64;
}

/// php's weekday walk. `weekday_behavior` is 0 for `next monday`, 1 for a
/// bare `monday` and 2 for the `this week` family; a negative weekday comes
/// from `ago` and walks backwards.
fn adjust_weekday(c: &mut Civil, rel: &Rel) {
    let Some(w) = rel.weekday else { return };
    let dow = civil::weekday(civil::days_from_civil(c.y, c.mo, c.d)) as i64;
    if rel.weekday_behavior == 2 {
        let mut w = w;
        // `this week` counts from Monday, so a Sunday has to be pulled back
        // into the week that just ended, and a wanted Sunday pushed to its
        // end.
        if dow == 0 && w != 0 {
            w -= 7;
        }
        if w == 0 && dow != 0 {
            w = 7;
        }
        c.d += w - dow;
        return;
    }
    let mut difference = w - dow;
    if (rel.d < 0 && difference < 0) || (rel.d >= 0 && difference <= -rel.weekday_behavior) {
        difference += 7;
    }
    if w >= 0 {
        c.d += difference;
    } else {
        c.d -= 7 - (w.abs() - dow);
    }
}

/// `<ordinal> <weekday> of <month>`: the n-th weekday of the month the date
/// currently sits in. `last` counts back from the last day of that month and
/// `this` starts at the first day of the *next* one, which is why
/// `this monday of january 2021` is 1 February.
fn adjust_nth_weekday(c: &mut Civil, n: i64, w: i64) {
    if n >= 1 {
        c.d = 1;
    } else {
        // The first day of the next month, then one day back for `last`.
        c.d = 1;
        c.mo += 1;
    }
    normalize(c);
    let dow = civil::weekday(civil::days_from_civil(c.y, c.mo, c.d)) as i64;
    if n >= 1 {
        c.d += (w - dow).rem_euclid(7) + (n - 1) * 7;
    } else if n == 0 {
        c.d += (w - dow).rem_euclid(7);
    } else {
        // `c` is the first of the next month; step back onto the last day of
        // the wanted one and then back to the wanted weekday.
        c.d -= 1;
        normalize(c);
        let dow = civil::weekday(civil::days_from_civil(c.y, c.mo, c.d)) as i64;
        c.d -= (dow - w).rem_euclid(7);
    }
    normalize(c);
}

/// php's business-day walk: `+1 weekday` from a Friday is the Monday, and a
/// multiple of five weekdays keeps the weekday it started on.
fn adjust_business_days(c: &mut Civil, count: i64) {
    let dow = civil::weekday(civil::days_from_civil(c.y, c.mo, c.d)) as i64;
    c.d += (count / 5) * 7;
    let rem = count % 5;
    if count > 0 {
        if rem == 0 {
            // Landing on a weekend with nothing left to do means stepping
            // back to the Friday.
            if dow == 0 {
                c.d -= 2;
            } else if dow == 6 {
                c.d -= 1;
            }
        } else if dow == 6 {
            c.d += 1;
        } else if dow + rem > 5 {
            c.d += 2;
        }
    } else if rem == 0 {
        if dow == 6 {
            c.d += 2;
        } else if dow == 0 {
            c.d += 1;
        }
    } else if dow == 0 {
        c.d -= 1;
    } else if dow + rem < 1 {
        c.d -= 2;
    }
    c.d += rem;
}

/// Fill the fields the string left open from `base_ts` read in `default_tz`,
/// then run php's adjustment pipeline. The order is observable: a relative
/// *month* lands before `first monday of` picks its month, a relative *day*
/// lands after it, and `first day of` lands after both.
pub(crate) fn fill_and_adjust(p: &Parsed, base_ts: i64, default_tz: &Tz) -> Civil {
    let now = super::format::render(default_tz, base_ts, 0);
    let mut c = Civil {
        y: p.y.unwrap_or(now.year),
        mo: p.m.unwrap_or(now.month as i64),
        d: p.d.unwrap_or(now.day as i64),
        h: 0,
        mi: 0,
        s: 0,
        us: 0,
    };
    if !(p.have_date && p.have_time == 0) {
        c.h = p.h.unwrap_or(now.hour as i64);
        c.mi = p.i.unwrap_or(now.minute as i64);
        c.s = p.s.unwrap_or(now.second as i64);
        c.us = p.us.unwrap_or(0);
    }
    normalize(&mut c);

    let rel = &p.rel;
    match rel.nth_weekday {
        // `first monday of next month` picks its weekday inside the month the
        // relative *month* already moved to, but a relative *year* lands
        // afterwards — `first monday of next year` is the base month's first
        // Monday shifted a year on, and so need not be a Monday at all.
        Some((n, w)) => {
            c.mo += rel.m;
            normalize(&mut c);
            adjust_nth_weekday(&mut c, n, w);
            c.y += rel.y;
        }
        None => {
            // A plain weekday walk happens before every relative amount, so
            // `monday next month` is next Monday moved on by a month.
            adjust_weekday(&mut c, rel);
            normalize(&mut c);
            c.y += rel.y;
            c.mo += rel.m;
        }
    }

    c.d += rel.d;
    c.h += rel.h;
    c.mi += rel.i;
    c.s += rel.s;
    c.us += rel.us;

    // `first day of` lands on the *un-normalized* month, so
    // `2021-10-31 first day of next month` is 1 November and not 1 December:
    // the day is replaced before 31 November can spill over.
    match rel.first_last_day_of {
        1 => c.d = 1,
        2 => {
            c.d = 0;
            c.mo += 1;
        }
        _ => {}
    }
    normalize(&mut c);

    if let Some(count) = rel.special_weekday {
        adjust_business_days(&mut c, count);
        normalize(&mut c);
    }
    c
}

/// The civil time as "seconds since the epoch read as if it were UTC" — the
/// half-resolved value a timezone turns into an instant.
pub(crate) fn sse_of(c: &Civil) -> i64 {
    civil::days_from_civil(c.y, c.mo, c.d)
        .saturating_mul(86_400)
        .saturating_add(c.h * 3600 + c.mi * 60 + c.s)
}

/// Resolve a parse against a base timestamp: the instant, and the
/// microseconds `strtotime()` throws away but `DateTime` keeps.
pub(crate) fn resolve(p: &Parsed, base_ts: i64, default_tz: &Tz) -> (i64, u32) {
    let c = fill_and_adjust(p, base_ts, default_tz);
    let zone = p.zone.clone().unwrap_or_else(|| default_tz.clone());
    (zone.resolve(sse_of(&c), None), c.us as u32)
}

/// `strtotime()`: `None` when the scan met an error, which is the only thing
/// php checks before answering `false`.
pub(crate) fn strtotime(input: &[u8], base_ts: i64, default_tz: &Tz) -> Option<i64> {
    let p = parse(input);
    if !p.errors.is_empty() {
        return None;
    }
    Some(resolve(&p, base_ts, default_tz).0)
}
