//! Proleptic Gregorian calendar arithmetic, in `i64` days since the Unix
//! epoch. php's calendar is unbounded — `date("Y", 253402300800)` is `10000`
//! and `date("Y", -68000000000)` is `-0185` — so the conversion is done here
//! rather than through a library type with a year range.

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`). `month` is 1..=12 but any value is accepted so that
/// `mktime(0, 0, 0, 13, 1, 2000)` can normalize by itself.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date `days` days after 1970-01-01.
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// php's `L`.
pub(crate) fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// php's `t`.
pub(crate) fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// php's `w`: 0 = Sunday … 6 = Saturday. Day 0 (1970-01-01) was a Thursday.
pub(crate) fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

/// php's `z`: the zero-based day of the year.
pub(crate) fn day_of_year(year: i64, month: u32, day: u32) -> u32 {
    (days_from_civil(year, month as i64, day as i64) - days_from_civil(year, 1, 1)) as u32
}

/// php's `o` and `W`: the ISO-8601 week-numbering year and week.
pub(crate) fn iso_week(year: i64, month: u32, day: u32) -> (i64, u32) {
    let days = days_from_civil(year, month as i64, day as i64);
    // ISO weekday, 1 = Monday … 7 = Sunday.
    let dow = ((days + 3).rem_euclid(7) + 1) as i64;
    // The Thursday of this week decides the week-numbering year.
    let thursday = days - (dow - 4);
    let (iso_year, _, _) = civil_from_days(thursday);
    let jan1 = days_from_civil(iso_year, 1, 1);
    let week = (thursday - jan1) / 7 + 1;
    (iso_year, week as u32)
}

/// Normalize `(year, month)` so that `month` lands in 1..=12, carrying into
/// the year — php's `mktime(0, 0, 0, 13, 1, 2000)`.
pub(crate) fn normalize_month(year: i64, month: i64) -> (i64, u32) {
    let m0 = month - 1;
    let y = year + m0.div_euclid(12);
    (y, (m0.rem_euclid(12) + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_civil_calendar() {
        for days in [-800_000i64, -719_468, -1, 0, 1, 11_017, 18_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m as i64, d as i64), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(weekday(0), 4);
    }

    #[test]
    fn iso_weeks_match_the_edge_cases() {
        // 1970-01-01 was a Thursday, so it is week 1 of 1970.
        assert_eq!(iso_week(1970, 1, 1), (1970, 1));
        // 1969-12-31 was a Wednesday, still week 1 of 1970.
        assert_eq!(iso_week(1969, 12, 31), (1970, 1));
        // 2009-02-13 is week 7 of 2009.
        assert_eq!(iso_week(2009, 2, 13), (2009, 7));
        // 10000-01-01 is a Saturday, week 52 of 9999.
        assert_eq!(iso_week(10000, 1, 1), (9999, 52));
    }
}
