//! `DateTime::createFromFormat()`'s and `date_parse_from_format()`'s
//! scanner (php-src `timelib_parse_from_format`).
//!
//! This is a *second* parser, and deliberately so: `strtotime()` guesses,
//! this one is told. The loop is timelib's, rule for rule:
//!
//! * it runs while **both** the format and the input have characters left;
//!   format characters left over when the input ran out are
//!   `Not enough data available to satisfy format` (only `!`, `|` and `+`
//!   may trail), input left over is `Trailing data`;
//! * a numeric field first checks that a digit is next (`Unexpected data
//!   found.` otherwise, without stopping) and then *skips* whatever is not a
//!   digit before reading — so `Y` reads `2020` out of `x2020`, with that
//!   one error;
//! * a failed field records its message at the input position where the
//!   field began and consumes nothing more; a literal format character that
//!   does not match still consumes one input character;
//! * diagnostics are keyed by input position, so a later one at the same
//!   position replaces an earlier one while the counts keep both.
//!
//! What the format never mentions is left unset: `createFromFormat()` fills
//! it from the wall clock (unless `!` or `|` reset it to the epoch) and
//! `date_parse_from_format()` reports it as `false`. Naming any clock field
//! pins the other clock fields to zero, as timelib's clean-up does. The
//! microseconds start pinned at zero (php fills the *clock* from `now` but
//! never the fraction; [`Scan::us_given`] tells the two apart), and `U` is
//! written the way the scanner writes `@<ts>` — epoch fields plus a relative
//! second count — so a timestamp and a trailing `.u` compose.

use super::super::civil;
use super::super::format::{DAY_NAMES, MONTH_NAMES};
use super::super::parse::Parsed;
use super::super::tz::{self, Tz};

/// What one scan produced.
pub(crate) struct Scan {
    /// The fields, as the resolution pipeline wants them.
    pub(crate) parsed: Parsed,
    /// `(position, message)`, in the order they were met.
    pub(crate) errors: Vec<(usize, String)>,
    /// See [`Scan::errors`].
    pub(crate) warnings: Vec<(usize, String)>,
    /// Whether the format named the fraction (`u`, `v`), reset it (`!`,
    /// `|`) or named another clock field — `date_parse_from_format()`
    /// reports it as unset otherwise.
    pub(crate) us_given: bool,
    /// Whether a `U` was read (its seconds sit in `parsed.rel.s`).
    pub(crate) from_unix: bool,
}

/// The characters `#` accepts, and the ones that are separators when they
/// appear in the format themselves.
const SEPARATORS: &[u8] = b";:/.,-()";

/// timelib's `timelib_get_nr_ex`: skip to the next digit (`None` at the end
/// of the input), then read at most `max` digits. Answers the number and
/// how many digits it had.
fn nr(input: &[u8], pos: &mut usize, max: usize) -> Option<(i64, usize)> {
    while *pos < input.len() && !input[*pos].is_ascii_digit() {
        *pos += 1;
    }
    if *pos >= input.len() {
        return None;
    }
    let start = *pos;
    while *pos < input.len() && *pos - start < max && input[*pos].is_ascii_digit() {
        *pos += 1;
    }
    let n = std::str::from_utf8(&input[start..*pos]).ok()?.parse().ok()?;
    Some((n, *pos - start))
}

/// timelib's `timelib_get_signed_nr`: like [`nr`], but a `+`/`-` met while
/// skipping sets the sign.
fn signed_nr(input: &[u8], pos: &mut usize, max: usize) -> Option<i64> {
    let mut neg = false;
    while *pos < input.len() && !input[*pos].is_ascii_digit() {
        match input[*pos] {
            b'-' => neg = true,
            b'+' => neg = false,
            _ => {}
        }
        *pos += 1;
    }
    let (n, _) = nr(input, pos, max)?;
    Some(if neg { -n } else { n })
}

/// Record a diagnostic.
fn fail(list: &mut Vec<(usize, String)>, at: usize, msg: &str) {
    list.push((at, msg.to_string()));
}

/// The multiplier timelib's relative-unit table gives a word (a weekday's
/// number for the day names, which is all `D`/`l` mean to use it for).
fn relunit(word: &[u8]) -> Option<i64> {
    let w = String::from_utf8_lossy(word).to_ascii_lowercase();
    for (i, day) in DAY_NAMES.iter().enumerate() {
        let day = day.to_ascii_lowercase();
        if w == day || w == day[..3] {
            return Some(i as i64);
        }
    }
    Some(match w.as_str() {
        "ms" | "msec" | "msecs" | "millisecond" | "milliseconds" => 1000,
        "µs" | "usec" | "usecs" | "µsec" | "µsecs" | "microsecond" | "microseconds" => 1,
        "sec" | "secs" | "second" | "seconds" | "min" | "mins" | "minute" | "minutes" | "hour"
        | "hours" | "day" | "days" | "month" | "months" | "year" | "years" => 1,
        "week" | "weeks" => 7,
        "fortnight" | "forthnight" | "fortnights" | "forthnights" => 14,
        _ => return None,
    })
}

/// timelib's month table: the full names, the three-letter forms, `sept`
/// and the roman numerals.
fn month_number(word: &[u8]) -> Option<i64> {
    let w = String::from_utf8_lossy(word).to_ascii_lowercase();
    for (i, name) in MONTH_NAMES.iter().enumerate() {
        let name = name.to_ascii_lowercase();
        if w == name || w == name[..3] {
            return Some(i as i64 + 1);
        }
    }
    const ROMAN: [&str; 12] = ["i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii"];
    if let Some(i) = ROMAN.iter().position(|r| *r == w) {
        return Some(i as i64 + 1);
    }
    (w == "sept").then_some(9)
}

/// timelib's `timelib_meridian_with_check`: what to add to the hour, or
/// `None` when no meridian follows.
fn meridian(input: &[u8], pos: &mut usize, h: i64) -> Option<i64> {
    while *pos < input.len() && !matches!(input[*pos], b'A' | b'a' | b'P' | b'p') {
        *pos += 1;
    }
    let first = *input.get(*pos)?;
    let add = if first.eq_ignore_ascii_case(&b'a') {
        if h == 12 { -12 } else { 0 }
    } else if h != 12 {
        12
    } else {
        0
    };
    *pos += 1;
    if input.get(*pos) == Some(&b'.') {
        *pos += 1;
    }
    if !input.get(*pos).is_some_and(|c| c.eq_ignore_ascii_case(&b'm')) {
        return None;
    }
    *pos += 1;
    if input.get(*pos) == Some(&b'.') {
        *pos += 1;
    }
    Some(add)
}

/// Scan `input` against `fmt`.
pub(crate) fn scan(fmt: &[u8], input: &[u8]) -> Scan {
    let mut p = Parsed {
        us: Some(0),
        have_time: 1,
        ..Parsed::default()
    };
    let mut errors: Vec<(usize, String)> = Vec::new();
    let mut warnings: Vec<(usize, String)> = Vec::new();
    let mut zone: Option<Tz> = None;
    let mut pos = 0usize;
    let mut f = 0usize;
    let mut allow_trailing = false;
    let mut us_given = false;
    let mut from_unix = false;

    // `TIMELIB_CHECK_NUMBER` / `TIMELIB_CHECK_SIGNED_NUMBER`.
    let check = |errors: &mut Vec<(usize, String)>, pos: usize, signed: bool| {
        let c = input[pos];
        if !(c.is_ascii_digit() || (signed && (c == b'-' || c == b'+'))) {
            fail(errors, pos, "Unexpected data found.");
        }
    };

    while f < fmt.len() && pos < input.len() {
        let c = fmt[f];
        let begin = pos;
        match c {
            b'D' | b'l' => {
                let mut end = pos;
                while end < input.len() && !b" ,\t;:/.-()".contains(&input[end]) {
                    end += 1;
                }
                match relunit(&input[pos..end]) {
                    Some(w) => {
                        pos = end;
                        p.have_relative = true;
                        p.rel.weekday = Some(w);
                        p.rel.weekday_behavior = 1;
                    }
                    None => {
                        pos = end;
                        fail(&mut errors, begin, "A textual day could not be found");
                    }
                }
            }
            b'd' | b'j' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, _)) => {
                        p.d = Some(n);
                        p.have_date = true;
                    }
                    None => fail(&mut errors, begin, "A two digit day could not be found"),
                }
            }
            // `S` names an ordinal suffix php parses and throws away.
            b'S' => {
                if !input[pos].is_ascii_whitespace()
                    && input.len() >= pos + 2
                    && ["nd", "rd", "st", "th"]
                        .iter()
                        .any(|s| input[pos..pos + 2].eq_ignore_ascii_case(s.as_bytes()))
                {
                    pos += 2;
                }
            }
            // `z` is the day of the year, counted from 1 January — only
            // once a year is known.
            b'z' => {
                check(&mut errors, pos, false);
                if p.y.is_none() {
                    fail(
                        &mut errors,
                        begin,
                        "A 'day of year' can only come after a year has been found",
                    );
                }
                match nr(input, &mut pos, 3) {
                    Some((n, _)) => {
                        if let Some(y) = p.y {
                            let (yy, m, d) = civil::civil_from_days(civil::days_from_civil(y, 1, 1) + n);
                            p.y = Some(yy);
                            p.m = Some(m as i64);
                            p.d = Some(d as i64);
                            p.have_date = true;
                        }
                    }
                    None => fail(
                        &mut errors,
                        begin,
                        "A three digit day-of-year could not be found",
                    ),
                }
            }
            b'm' | b'n' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, _)) => {
                        p.m = Some(n);
                        p.have_date = true;
                    }
                    None => fail(&mut errors, begin, "A two digit month could not be found"),
                }
            }
            b'M' | b'F' => {
                let mut end = pos;
                while end < input.len() && input[end].is_ascii_alphabetic() {
                    end += 1;
                }
                let m = month_number(&input[pos..end]);
                pos = end;
                match m {
                    Some(m) => {
                        p.m = Some(m);
                        p.have_date = true;
                    }
                    None => fail(&mut errors, begin, "A textual month could not be found"),
                }
            }
            b'y' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, len)) => {
                        // php's two-digit window: 00-69 is 2000-2069.
                        p.y = Some(if len >= 4 {
                            n
                        } else if n < 70 {
                            2000 + n
                        } else {
                            1900 + n
                        });
                        p.have_date = true;
                    }
                    None => fail(&mut errors, begin, "A two digit year could not be found"),
                }
            }
            b'Y' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 4) {
                    Some((n, _)) => {
                        p.y = Some(n);
                        p.have_date = true;
                    }
                    None => fail(&mut errors, begin, "A four digit year could not be found"),
                }
            }
            b'X' | b'x' => {
                check(&mut errors, pos, true);
                match signed_nr(input, &mut pos, 19) {
                    Some(n) => {
                        p.y = Some(n);
                        p.have_date = true;
                    }
                    None => fail(&mut errors, begin, "A four digit year could not be found"),
                }
            }
            b'g' | b'h' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, _)) => {
                        p.h = Some(n);
                        if n > 12 {
                            fail(&mut errors, begin, "Hour cannot be higher than 12");
                        }
                    }
                    None => fail(&mut errors, begin, "A two digit hour could not be found"),
                }
            }
            b'G' | b'H' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, _)) => p.h = Some(n),
                    None => fail(&mut errors, begin, "A two digit hour could not be found"),
                }
            }
            b'a' | b'A' => match p.h {
                // The meridian is still consumed.
                None => {
                    meridian(input, &mut pos, 0);
                    fail(
                        &mut errors,
                        begin,
                        "Meridian can only come after an hour has been found",
                    );
                }
                Some(h) => match meridian(input, &mut pos, h) {
                    Some(add) => p.h = Some(h + add),
                    None => fail(&mut errors, begin, "A meridian could not be found"),
                },
            },
            b'i' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, 2)) => p.i = Some(n),
                    _ => fail(&mut errors, begin, "A two digit minute could not be found"),
                }
            }
            b's' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 2) {
                    Some((n, 2)) => p.s = Some(n),
                    _ => fail(&mut errors, begin, "A two digit second could not be found"),
                }
            }
            b'v' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 3) {
                    Some((n, len)) => {
                        p.us = Some(n * 10i64.pow(3 - len as u32) * 1000);
                        us_given = true;
                    }
                    None => fail(
                        &mut errors,
                        begin,
                        "A three digit millisecond could not be found",
                    ),
                }
            }
            b'u' => {
                check(&mut errors, pos, false);
                match nr(input, &mut pos, 6) {
                    Some((n, len)) => {
                        // A short fraction is left-aligned: `.5` is 500 000 µs.
                        p.us = Some(n * 10i64.pow(6 - len as u32));
                        us_given = true;
                    }
                    None => fail(
                        &mut errors,
                        begin,
                        "A six digit microsecond could not be found",
                    ),
                }
            }
            // A space in the format eats any run of blanks, none included.
            b' ' => {
                while pos < input.len() && (input[pos] == b' ' || input[pos] == b'\t') {
                    pos += 1;
                }
            }
            b'U' => {
                check(&mut errors, pos, true);
                match signed_nr(input, &mut pos, 24) {
                    Some(n) => {
                        // The same shape the scanner gives `@<ts>`: epoch
                        // fields plus a relative second count, so `U.u`
                        // composes and the zone becomes `+00:00`.
                        p.y = Some(1970);
                        p.m = Some(1);
                        p.d = Some(1);
                        p.h = Some(0);
                        p.i = Some(0);
                        p.s = Some(0);
                        p.have_relative = true;
                        p.rel.s += n;
                        from_unix = true;
                        zone = Some(Tz::Offset(0));
                    }
                    None => fail(&mut errors, begin, "Found unexpected data"),
                }
            }
            // `#` accepts any one of php's separator characters.
            b'#' => {
                if SEPARATORS.contains(&input[pos]) {
                    pos += 1;
                } else {
                    fail(
                        &mut errors,
                        begin,
                        "The separation symbol ([;:/.,-]) could not be found",
                    );
                }
            }
            b';' | b':' | b'/' | b'.' | b',' | b'-' | b'(' | b')' => {
                if input[pos] == c {
                    pos += 1;
                } else {
                    fail(&mut errors, begin, "The separation symbol could not be found");
                }
            }
            // `!` resets every field to the epoch; `|` resets only the ones
            // still unset.
            b'!' => {
                reset_all(&mut p);
                us_given = true;
            }
            b'|' => {
                reset_unset(&mut p);
                us_given = true;
            }
            // `+` downgrades trailing data to a warning.
            b'+' => allow_trailing = true,
            // `\` escapes the next format character into a literal.
            b'\\' => match fmt.get(f + 1) {
                None => fail(&mut errors, begin, "Escaped character expected"),
                Some(&lit) => {
                    f += 1;
                    if input[pos] == lit {
                        pos += 1;
                    } else {
                        fail(&mut errors, begin, "The escaped character could not be found");
                    }
                }
            },
            // `*` eats at least one character, then up to a separator or a
            // digit.
            b'*' => {
                pos += 1;
                while pos < input.len() && !b" \t.,:;/-0123456789".contains(&input[pos]) {
                    pos += 1;
                }
            }
            // `?` eats one character, whatever it is.
            b'?' => pos += 1,
            b'e' | b'T' | b'P' | b'p' | b'O' => {
                let (tz, end) = read_zone(input, pos);
                pos = end;
                p.have_zone = true;
                match tz {
                    Some(tz) => zone = Some(tz),
                    None => {
                        zone = None;
                        fail(
                            &mut errors,
                            begin,
                            "The timezone could not be found in the database",
                        );
                    }
                }
            }
            // Anything else has to be in the input verbatim, and consumes
            // one character either way.
            other => {
                if input[pos] != other {
                    fail(&mut errors, begin, "The format separator does not match");
                }
                pos += 1;
            }
        }
        f += 1;
    }

    if pos < input.len() {
        if allow_trailing {
            fail(&mut warnings, pos, "Trailing data");
        } else {
            fail(&mut errors, pos, "Trailing data");
        }
    }
    // Only the reset specifiers (and `+`) may be left over in the format.
    while f < fmt.len() {
        match fmt[f] {
            b'!' => {
                reset_all(&mut p);
                us_given = true;
            }
            b'|' => {
                reset_unset(&mut p);
                us_given = true;
            }
            b'+' => {}
            _ => {
                fail(&mut errors, pos, "Not enough data available to satisfy format");
                break;
            }
        }
        f += 1;
    }
    // timelib's clean-up: one clock field named pins the others to zero.
    if p.h.is_some() || p.i.is_some() || p.s.is_some() || us_given {
        p.h = p.h.or(Some(0));
        p.i = p.i.or(Some(0));
        p.s = p.s.or(Some(0));
        us_given = true;
        let (h, i, s) = (p.h.unwrap_or(0), p.i.unwrap_or(0), p.s.unwrap_or(0));
        if !(0..=23).contains(&h) || !(0..=59).contains(&i) || !(0..=59).contains(&s) {
            fail(&mut warnings, pos, "The parsed time was invalid");
        }
    }
    if let (Some(y), Some(m), Some(d)) = (p.y, p.m, p.d) {
        if !(1..=12).contains(&m) || d < 1 || d > civil::days_in_month(y, m as u32) as i64 {
            fail(&mut warnings, pos, "The parsed date was invalid");
        }
    }
    if let Some(tz) = zone {
        p.have_zone = true;
        p.zone = Some(tz);
    }
    Scan {
        parsed: p,
        errors,
        warnings,
        us_given,
        from_unix,
    }
}

/// `!`: every field back to the Unix epoch.
fn reset_all(p: &mut Parsed) {
    p.y = Some(1970);
    p.m = Some(1);
    p.d = Some(1);
    p.h = Some(0);
    p.i = Some(0);
    p.s = Some(0);
    p.us = Some(0);
}

/// `|`: the fields still unset back to the Unix epoch.
fn reset_unset(p: &mut Parsed) {
    p.y = p.y.or(Some(1970));
    p.m = p.m.or(Some(1));
    p.d = p.d.or(Some(1));
    p.h = p.h.or(Some(0));
    p.i = p.i.or(Some(0));
    p.s = p.s.or(Some(0));
}

/// timelib's `timelib_parse_zone`: an offset after a sign (`GMT+…` too),
/// else a word of `[A-Za-z0-9/_+-]` looked up as an abbreviation and then
/// as an identifier. The word is consumed whether or not it resolves, as
/// are leading blanks and `(` and trailing `)`.
fn read_zone(input: &[u8], mut pos: usize) -> (Option<Tz>, usize) {
    while pos < input.len() && matches!(input[pos], b' ' | b'\t' | b'(') {
        pos += 1;
    }
    let rest = &input[pos..];
    if rest.len() >= 4 && &rest[..3] == b"GMT" && matches!(rest[3], b'+' | b'-') {
        pos += 3;
    }
    let tz = if matches!(input.get(pos), Some(b'+' | b'-')) {
        let start = pos;
        pos += 1;
        while pos < input.len() && (input[pos].is_ascii_digit() || input[pos] == b':') {
            pos += 1;
        }
        std::str::from_utf8(&input[start..pos])
            .ok()
            .and_then(tz::parse_offset)
            .map(Tz::Offset)
    } else {
        let start = pos;
        while pos < input.len()
            && (input[pos].is_ascii_alphanumeric() || matches!(input[pos], b'/' | b'_' | b'-' | b'+'))
        {
            pos += 1;
        }
        let word = String::from_utf8_lossy(&input[start..pos]).into_owned();
        match tz::lookup_abbr(&word) {
            Some(_) if word == "UTC" => Some(Tz::Id(word)),
            Some((offset, dst)) => Some(Tz::Abbr {
                name: word.to_ascii_uppercase(),
                offset,
                dst,
            }),
            None if !word.is_empty() && tz::is_known_id(&word) => Some(Tz::Id(word)),
            None => None,
        }
    };
    while input.get(pos) == Some(&b')') {
        pos += 1;
    }
    (tz, pos)
}
