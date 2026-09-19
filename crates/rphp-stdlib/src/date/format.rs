//! `date()`'s format characters. Every one of them is pinned against stock
//! php; the surprising ones are commented where they are produced.

use super::civil;
use super::tz::Tz;

/// A timestamp already broken down in some timezone: what every format
/// character reads from.
pub(crate) struct Rendered {
    pub(crate) ts: i64,
    pub(crate) usec: u32,
    pub(crate) year: i64,
    pub(crate) month: u32,
    pub(crate) day: u32,
    pub(crate) hour: u32,
    pub(crate) minute: u32,
    pub(crate) second: u32,
    pub(crate) offset: i32,
    pub(crate) abbr: String,
    pub(crate) dst: bool,
    pub(crate) tz_name: String,
}

pub(crate) const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

pub(crate) const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Break `ts` down in `tz`.
pub(crate) fn render(tz: &Tz, ts: i64, usec: u32) -> Rendered {
    let (offset, abbr, dst) = tz.info(ts);
    // php's timestamps run to the whole `i64` range, so adding the offset can
    // overflow at the very edge; saturating keeps the extreme value readable
    // instead of panicking.
    let local = ts.saturating_add(offset as i64);
    let (year, month, day) = civil::civil_from_days(local.div_euclid(86_400));
    let rem = local.rem_euclid(86_400);
    Rendered {
        ts,
        usec,
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        minute: ((rem / 60) % 60) as u32,
        second: (rem % 60) as u32,
        offset,
        abbr,
        dst,
        tz_name: tz.name(),
    }
}

/// php's `Y`: at least four digits, the sign written before the padding
/// (`-0010`).
fn year4(y: i64) -> String {
    if y < 0 {
        format!("-{:04}", y.unsigned_abs())
    } else {
        format!("{y:04}")
    }
}

/// php's `c` and `r` pad the year to four *columns* including the sign, so
/// year -10 prints as `-010` where `Y` prints `-0010`.
fn year_c(y: i64) -> String {
    format!("{y:04}")
}

/// php's `S`.
fn ordinal_suffix(day: u32) -> &'static str {
    match day {
        11..=13 => "th",
        _ => match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    }
}

/// php's `O` / `P`.
fn offset_hm(secs: i32, colon: bool) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    if colon {
        format!("{sign}{:02}:{:02}", a / 3600, (a / 60) % 60)
    } else {
        format!("{sign}{:02}{:02}", a / 3600, (a / 60) % 60)
    }
}

/// php's `B`: Swatch Internet Time, always reckoned in UTC+1 and independent
/// of the timezone the rest of the string is formatted in.
fn swatch_beat(ts: i64) -> String {
    let beats = (ts + 3600).rem_euclid(86_400) * 1000 / 86_400;
    format!("{beats:03}")
}

/// Format one instant with php's `date()` format string.
pub(crate) fn format(r: &Rendered, fmt: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(fmt.len() * 2);
    let wday = civil::weekday(civil::days_from_civil(r.year, r.month as i64, r.day as i64));
    let (iso_year, iso_week) = civil::iso_week(r.year, r.month, r.day);
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c == b'\\' {
            if i < fmt.len() {
                out.push(fmt[i]);
                i += 1;
            }
            continue;
        }
        let piece: String = match c {
            b'd' => format!("{:02}", r.day),
            b'D' => DAY_NAMES[wday as usize][..3].to_string(),
            b'j' => r.day.to_string(),
            b'l' => DAY_NAMES[wday as usize].to_string(),
            b'N' => (if wday == 0 { 7 } else { wday }).to_string(),
            b'S' => ordinal_suffix(r.day).to_string(),
            b'w' => wday.to_string(),
            b'z' => civil::day_of_year(r.year, r.month, r.day).to_string(),
            b'W' => format!("{iso_week:02}"),
            b'F' => MONTH_NAMES[(r.month - 1) as usize].to_string(),
            b'm' => format!("{:02}", r.month),
            b'M' => MONTH_NAMES[(r.month - 1) as usize][..3].to_string(),
            b'n' => r.month.to_string(),
            b't' => civil::days_in_month(r.year, r.month).to_string(),
            b'L' => u8::from(civil::is_leap(r.year)).to_string(),
            // `o` is *not* padded, unlike `Y`.
            b'o' => iso_year.to_string(),
            b'X' => {
                if r.year < 0 {
                    format!("-{:04}", r.year.unsigned_abs())
                } else {
                    format!("+{:04}", r.year)
                }
            }
            b'x' => {
                if r.year >= 10_000 {
                    format!("+{}", r.year)
                } else {
                    year4(r.year)
                }
            }
            b'Y' => year4(r.year),
            b'y' => {
                if r.year < 0 {
                    format!("-{:02}", r.year.unsigned_abs() % 100)
                } else {
                    format!("{:02}", r.year % 100)
                }
            }
            b'a' => (if r.hour < 12 { "am" } else { "pm" }).to_string(),
            b'A' => (if r.hour < 12 { "AM" } else { "PM" }).to_string(),
            b'B' => swatch_beat(r.ts),
            b'g' => hour12(r.hour).to_string(),
            b'G' => r.hour.to_string(),
            b'h' => format!("{:02}", hour12(r.hour)),
            b'H' => format!("{:02}", r.hour),
            b'i' => format!("{:02}", r.minute),
            b's' => format!("{:02}", r.second),
            b'u' => format!("{:06}", r.usec),
            b'v' => format!("{:03}", r.usec / 1000),
            b'e' => r.tz_name.clone(),
            b'I' => u8::from(r.dst).to_string(),
            b'O' => offset_hm(r.offset, false),
            b'P' => offset_hm(r.offset, true),
            // `p` is `P` except that a zero offset prints `Z` — unless the
            // zone's abbreviation is literally `GMT`, which php leaves as
            // `+00:00` (`Europe/London` in winter, `Etc/GMT`, `GMT`).
            // `gmdate()` is the exception to the exception: its zone is named
            // `UTC` but abbreviated `GMT`, and php prints `Z` for it.
            b'p' => {
                if r.offset == 0 && (r.abbr != "GMT" || r.tz_name == "UTC") {
                    "Z".to_string()
                } else {
                    offset_hm(r.offset, true)
                }
            }
            b'T' => r.abbr.clone(),
            b'Z' => r.offset.to_string(),
            b'c' => format!(
                "{}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
                year_c(r.year),
                r.month,
                r.day,
                r.hour,
                r.minute,
                r.second,
                offset_hm(r.offset, true)
            ),
            b'r' => format!(
                "{}, {:02} {} {} {:02}:{:02}:{:02} {}",
                &DAY_NAMES[wday as usize][..3],
                r.day,
                &MONTH_NAMES[(r.month - 1) as usize][..3],
                year_c(r.year),
                r.hour,
                r.minute,
                r.second,
                offset_hm(r.offset, false)
            ),
            b'U' => r.ts.to_string(),
            other => {
                out.push(other);
                continue;
            }
        };
        out.extend_from_slice(piece.as_bytes());
    }
    out
}

fn hour12(hour: u32) -> u32 {
    match hour % 12 {
        0 => 12,
        h => h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(ts: i64, f: &str) -> String {
        let r = render(&Tz::utc(), ts, 0);
        String::from_utf8(format(&r, f.as_bytes())).unwrap()
    }

    #[test]
    fn every_format_character_matches_php() {
        // php -r 'date_default_timezone_set("UTC"); echo date(…, 1234567890);'
        assert_eq!(
            fmt(1_234_567_890, "d D j l N S w z W F m M n t L o X x Y y"),
            "13 Fri 13 Friday 5 th 5 43 07 February 02 Feb 2 28 0 2009 +2009 2009 2009 09"
        );
        assert_eq!(
            fmt(1_234_567_890, "a A B g G h H i s u v e I O P p T Z"),
            "pm PM 021 11 23 11 23 31 30 000000 000 UTC 0 +0000 +00:00 Z UTC 0"
        );
        assert_eq!(fmt(1_234_567_890, "c"), "2009-02-13T23:31:30+00:00");
        assert_eq!(fmt(1_234_567_890, "r"), "Fri, 13 Feb 2009 23:31:30 +0000");
        assert_eq!(fmt(1_234_567_890, "U"), "1234567890");
        assert_eq!(fmt(0, "B"), "041");
        assert_eq!(fmt(1_000_000_000, "B"), "115");
    }

    #[test]
    fn extreme_years_pad_the_way_php_pads_them() {
        assert_eq!(
            fmt(-62_135_596_800, "Y y X x o c r"),
            "0001 01 +0001 0001 1 0001-01-01T00:00:00+00:00 Mon, 01 Jan 0001 00:00:00 +0000"
        );
        assert_eq!(
            fmt(-62_451_993_600, "Y y X x o"),
            "-0010 -10 -0010 -0010 -10"
        );
        assert_eq!(fmt(-62_451_993_600, "c"), "-010-12-23T00:00:00+00:00");
        assert_eq!(
            fmt(253_402_300_800, "Y y X x o L z W"),
            "10000 00 +10000 +10000 9999 1 0 52"
        );
        assert_eq!(
            fmt(-68_000_000_000, "Y y X x o z W"),
            "-0185 -85 -0185 -0185 -185 60 09"
        );
    }

    #[test]
    fn backslash_escapes_the_next_character() {
        assert_eq!(fmt(0, "\\Y Y"), "Y 1970");
        assert_eq!(fmt(0, "\\\\ q Q"), "\\ q Q");
        assert_eq!(fmt(0, "Y-m-d\\TH:i:s"), "1970-01-01T00:00:00");
    }

    #[test]
    fn a_dst_zone_reports_its_abbreviation_and_offset() {
        let berlin = Tz::Id("Europe/Berlin".to_string());
        let r = render(&berlin, 1_234_567_890, 0);
        assert_eq!(
            String::from_utf8(format(&r, b"c|e|T|I|O|P|p|Z|U")).unwrap(),
            "2009-02-14T00:31:30+01:00|Europe/Berlin|CET|0|+0100|+01:00|+01:00|3600|1234567890"
        );
        let r = render(&berlin, 1_616_893_200, 0);
        assert_eq!(
            String::from_utf8(format(&r, b"c|T|I|Z")).unwrap(),
            "2021-03-28T03:00:00+02:00|CEST|1|7200"
        );
    }
}
