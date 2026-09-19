//! `DateTime::createFromFormat()`'s scanner (php-src
//! `timelib_parse_from_format`).
//!
//! This is a *second* parser, and deliberately so: `strtotime()` guesses,
//! this one is told. Each format character consumes exactly what it names
//! and nothing else, an unrecognized one has to match the input literally,
//! and what the format never mentions is filled from the wall clock —
//! unless `!` or `|` resets it to the epoch, which is the whole reason
//! those two exist.
//!
//! The result is expressed as a [`Parsed`], so the same civil resolution
//! pipeline finishes the job. Two details make that work: the microseconds
//! are always pinned (php fills the *clock* from `now` but never the
//! fraction), and `U` is written the way the scanner writes `@<ts>` —
//! epoch fields plus a relative second count — so a timestamp and a
//! trailing `.u` compose.
//!
//! **Known divergence.** php reports a diagnostic at the offset of the
//! format character that failed; the offsets here are the input offsets the
//! scanner stopped at, so `getLastErrors()`'s *keys* can differ from php's
//! even when the messages and the counts agree.

use super::super::format::{DAY_NAMES, MONTH_NAMES};
use super::super::parse::Parsed;
use super::super::tz::{self, Tz};

/// What one scan produced.
pub(crate) struct Scan {
    /// The fields, as the resolution pipeline wants them.
    pub(crate) parsed: Parsed,
    /// The zone the format named, if any.
    pub(crate) zone: Option<Tz>,
    /// `(position, message)`, in the order they were met.
    pub(crate) errors: Vec<(usize, String)>,
    /// See [`Scan::errors`].
    pub(crate) warnings: Vec<(usize, String)>,
}

/// The characters php treats as separators: what `*` skips to and what `#`
/// accepts.
const SEPARATORS: &[u8] = b";:/.,-()";

/// Consume between `min` and `max` decimal digits.
fn digits(input: &[u8], pos: usize, min: usize, max: usize) -> Option<(i64, usize)> {
    let mut end = pos;
    while end < input.len() && end - pos < max && input[end].is_ascii_digit() {
        end += 1;
    }
    if end - pos < min {
        return None;
    }
    let n = std::str::from_utf8(&input[pos..end]).ok()?.parse().ok()?;
    Some((n, end))
}

/// Record a diagnostic.
fn fail(errors: &mut Vec<(usize, String)>, at: usize, msg: &str) {
    errors.push((at, msg.to_string()));
}

/// Case-insensitively match the longest of `names`, answering its index and
/// the number of bytes eaten. `prefix` allows the three-letter forms.
fn word(input: &[u8], pos: usize, names: &[&str], prefix: Option<usize>) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for (i, name) in names.iter().enumerate() {
        for len in [name.len(), prefix.unwrap_or(name.len())] {
            if len > name.len() || pos + len > input.len() {
                continue;
            }
            if input[pos..pos + len].eq_ignore_ascii_case(name[..len].as_bytes())
                && best.is_none_or(|(_, b)| len > b)
            {
                best = Some((i, len));
            }
        }
    }
    best
}

/// Scan `input` against `fmt`.
pub(crate) fn scan(fmt: &[u8], input: &[u8]) -> Scan {
    let mut p = Parsed {
        // php fills an unnamed clock field from `now` but never the
        // fraction, so the microseconds start pinned at zero.
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
    let mut pm: Option<bool> = None;
    let mut twelve = false;

    while f < fmt.len() {
        let c = fmt[f];
        f += 1;
        match c {
            // `\` escapes the next format character into a literal.
            b'\\' => {
                let Some(&lit) = fmt.get(f) else { break };
                f += 1;
                if input.get(pos) == Some(&lit) {
                    pos += 1;
                } else {
                    fail(&mut errors, pos, "The separation symbol could not be found");
                }
            }
            // `!` resets every field to the epoch; `|` resets only the ones
            // still unset.
            b'!' => {
                p.y = Some(1970);
                p.m = Some(1);
                p.d = Some(1);
                p.h = Some(0);
                p.i = Some(0);
                p.s = Some(0);
                p.us = Some(0);
            }
            b'|' => {
                p.y = p.y.or(Some(1970));
                p.m = p.m.or(Some(1));
                p.d = p.d.or(Some(1));
                p.h = p.h.or(Some(0));
                p.i = p.i.or(Some(0));
                p.s = p.s.or(Some(0));
            }
            // `+` downgrades trailing data to a warning.
            b'+' => allow_trailing = true,
            // `?` eats one character, whatever it is.
            b'?' => {
                if pos < input.len() {
                    pos += 1;
                } else {
                    fail(
                        &mut errors,
                        pos,
                        "Not enough data available to satisfy format",
                    );
                }
            }
            // `*` eats everything up to the next separator.
            b'*' => {
                while pos < input.len() && !SEPARATORS.contains(&input[pos]) {
                    pos += 1;
                }
            }
            // `#` accepts any one of php's separator characters.
            b'#' => match input.get(pos) {
                Some(ch) if SEPARATORS.contains(ch) => pos += 1,
                _ => fail(&mut errors, pos, "The separation symbol could not be found"),
            },
            b'd' | b'j' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    p.d = Some(n);
                    p.have_date = true;
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit day could not be found"),
            },
            b'm' | b'n' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    p.m = Some(n);
                    p.have_date = true;
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit month could not be found"),
            },
            b'Y' => match digits(input, pos, 1, 4) {
                Some((n, end)) => {
                    p.y = Some(n);
                    p.have_date = true;
                    pos = end;
                }
                None => fail(&mut errors, pos, "A four digit year could not be found"),
            },
            b'y' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    // php's two-digit window: 00-69 is 2000-2069.
                    p.y = Some(if n < 70 { 2000 + n } else { 1900 + n });
                    p.have_date = true;
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit year could not be found"),
            },
            b'X' | b'x' => {
                let neg = input.get(pos) == Some(&b'-');
                if neg || input.get(pos) == Some(&b'+') {
                    pos += 1;
                }
                match digits(input, pos, 1, 19) {
                    Some((n, end)) => {
                        p.y = Some(if neg { -n } else { n });
                        p.have_date = true;
                        pos = end;
                    }
                    None => fail(&mut errors, pos, "A four digit year could not be found"),
                }
            }
            b'G' | b'H' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    p.h = Some(n);
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit hour could not be found"),
            },
            b'g' | b'h' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    p.h = Some(n);
                    twelve = true;
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit hour could not be found"),
            },
            b'i' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    p.i = Some(n);
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit minute could not be found"),
            },
            b's' => match digits(input, pos, 1, 2) {
                Some((n, end)) => {
                    p.s = Some(n);
                    pos = end;
                }
                None => fail(&mut errors, pos, "A two digit second could not be found"),
            },
            b'v' => match digits(input, pos, 1, 3) {
                Some((n, end)) => {
                    p.us = Some(n * 1000);
                    pos = end;
                }
                None => fail(
                    &mut errors,
                    pos,
                    "A three digit millisecond could not be found",
                ),
            },
            b'u' => match digits(input, pos, 1, 6) {
                Some((n, end)) => {
                    // A short fraction is left-aligned: `.5` is 500 000 µs.
                    let scale = 10i64.pow(6 - (end - pos) as u32);
                    p.us = Some(n * scale);
                    pos = end;
                }
                None => fail(
                    &mut errors,
                    pos,
                    "A six digit microsecond could not be found",
                ),
            },
            b'a' | b'A' => match word(input, pos, &["am", "pm"], None) {
                Some((i, len)) => {
                    pm = Some(i == 1);
                    pos += len;
                }
                None => fail(&mut errors, pos, "A meridian could not be found"),
            },
            b'D' | b'l' => match word(input, pos, &DAY_NAMES[..], Some(3)) {
                Some((_, len)) => pos += len,
                None => fail(&mut errors, pos, "A textual day could not be found"),
            },
            b'F' | b'M' => match word(input, pos, &MONTH_NAMES[..], Some(3)) {
                Some((i, len)) => {
                    p.m = Some(i as i64 + 1);
                    p.have_date = true;
                    pos += len;
                }
                None => fail(&mut errors, pos, "A textual month could not be found"),
            },
            // `S` names an ordinal suffix php parses and throws away.
            b'S' => {
                if word(input, pos, &["st", "nd", "rd", "th"], None).is_some() {
                    pos += 2;
                }
            }
            // `z` is the day of the year, counted from 1 January.
            b'z' => match digits(input, pos, 1, 3) {
                Some((n, end)) => {
                    p.m = Some(1);
                    p.d = Some(n + 1);
                    p.have_date = true;
                    pos = end;
                }
                None => fail(
                    &mut errors,
                    pos,
                    "A three digit day-of-year could not be found",
                ),
            },
            // `N` and `w` are consumed and discarded: the weekday cannot
            // contradict a date php already has.
            b'N' | b'w' => match digits(input, pos, 1, 1) {
                Some((_, end)) => pos = end,
                None => fail(
                    &mut errors,
                    pos,
                    "A single digit day of week could not be found",
                ),
            },
            b'U' => {
                let neg = input.get(pos) == Some(&b'-');
                if neg {
                    pos += 1;
                }
                match digits(input, pos, 1, 19) {
                    Some((n, end)) => {
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
                        p.rel.s += if neg { -n } else { n };
                        zone = Some(Tz::Offset(0));
                        pos = end;
                    }
                    None => fail(
                        &mut errors,
                        pos,
                        "A valid unix timestamp could not be found",
                    ),
                }
            }
            b'e' | b'T' | b'P' | b'p' | b'O' => match read_zone(input, pos) {
                Some((tz, end)) => {
                    zone = Some(tz);
                    pos = end;
                }
                None => fail(
                    &mut errors,
                    pos,
                    "The timezone could not be found in the database",
                ),
            },
            // Anything else — a separator, a space, an unsupported letter —
            // has to be in the input verbatim.
            other => {
                if input.get(pos) == Some(&other) {
                    pos += 1;
                } else {
                    fail(&mut errors, pos, "The format separator does not match");
                }
            }
        }
    }

    if pos < input.len() {
        if allow_trailing {
            warnings.push((pos, "Trailing data".to_string()));
        } else {
            errors.push((pos, "Trailing data".to_string()));
        }
    }
    if twelve {
        if let (Some(h), Some(afternoon)) = (p.h, pm) {
            p.h = Some(h % 12 + if afternoon { 12 } else { 0 });
        }
    }
    if let Some(tz) = zone.clone() {
        p.have_zone = true;
        p.zone = Some(tz);
    }
    Scan {
        parsed: p,
        zone,
        errors,
        warnings,
    }
}

/// A timezone written as an offset, an abbreviation or an identifier —
/// everything `e`, `T`, `P`, `p` and `O` accept.
fn read_zone(input: &[u8], pos: usize) -> Option<(Tz, usize)> {
    if input.get(pos) == Some(&b'Z') {
        return Some((Tz::Offset(0), pos + 1));
    }
    // An identifier or an abbreviation runs while the bytes can belong to
    // one; an offset starts with a sign.
    let mut end = pos;
    while end < input.len()
        && (input[end].is_ascii_alphanumeric()
            || matches!(input[end], b'/' | b'_' | b'+' | b'-' | b':'))
    {
        end += 1;
    }
    // The longest prefix php's database knows wins, so a trailing literal
    // in the format still has something to match.
    while end > pos {
        let Ok(text) = std::str::from_utf8(&input[pos..end]) else {
            end -= 1;
            continue;
        };
        if let Some(offset) = tz::parse_offset(text) {
            return Some((Tz::Offset(offset), end));
        }
        if text.eq_ignore_ascii_case("utc") {
            return Some((Tz::Id(text.to_string()), end));
        }
        if let Some((offset, dst)) = tz::lookup_abbr(text) {
            return Some((
                Tz::Abbr {
                    name: text.to_string(),
                    offset,
                    dst,
                },
                end,
            ));
        }
        if tz::is_known_id(text) {
            return Some((Tz::Id(text.to_string()), end));
        }
        end -= 1;
    }
    None
}
