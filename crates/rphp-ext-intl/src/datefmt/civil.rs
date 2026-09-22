//! The calendar arithmetic behind ICU's Gregorian calendar: proleptic
//! Gregorian dates from 1582-10-15 on, the Julian calendar before (ICU's
//! default cutover), the fields a date pattern prints, and the CLDR week
//! numbering a locale's week data implies.

/// The first Gregorian day, 1582-10-15, in days since 1970-01-01.
pub const CUTOVER_DAYS: i64 = -141_427;

/// The broken-down fields of an instant in some zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fields {
    /// Days since the epoch of the local date.
    pub days: i64,
    /// The extended (astronomical) year: 0 is 1 BC.
    pub ext_year: i64,
    /// 1 = AD, 0 = BC.
    pub era: u8,
    /// The year within the era (1 BC is 1).
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
    /// 0 = Sunday … 6 = Saturday.
    pub weekday: u32,
    /// 1-based.
    pub day_of_year: u32,
    /// Whether the date is before the Gregorian cutover (Julian rules).
    pub julian: bool,
}

/// Proleptic Gregorian civil date from days since the epoch.
pub fn gregorian_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since the epoch of a proleptic Gregorian date.
pub fn days_from_gregorian(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Julian civil date from days since the epoch.
pub fn julian_from_days(z: i64) -> (i64, u32, u32) {
    // via the Julian day number (1970-01-01 is JDN 2440588)
    let jdn = z + 2_440_588;
    let c = jdn + 32_082;
    let d = (4 * c + 3).div_euclid(1461);
    let e = c - (1461 * d).div_euclid(4);
    let m = (5 * e + 2).div_euclid(153);
    let day = (e - (153 * m + 2).div_euclid(5) + 1) as u32;
    let month = (m + 3 - 12 * m.div_euclid(10)) as u32;
    let year = d - 4800 + m.div_euclid(10);
    (year, month, day)
}

/// Days since the epoch of a Julian date.
pub fn days_from_julian(y: i64, m: u32, d: u32) -> i64 {
    let a = i64::from((14 - m) / 12);
    let yy = y + 4800 - a;
    let mm = i64::from(m) + 12 * a - 3;
    let jdn = i64::from(d) + (153 * mm + 2).div_euclid(5) + 365 * yy + yy.div_euclid(4) - 32_083;
    jdn - 2_440_588
}

/// ICU's hybrid calendar: Julian before the cutover, Gregorian from it.
pub fn civil_from_days(z: i64) -> (i64, u32, u32, bool) {
    if z < CUTOVER_DAYS {
        let (y, m, d) = julian_from_days(z);
        (y, m, d, true)
    } else {
        let (y, m, d) = gregorian_from_days(z);
        (y, m, d, false)
    }
}

/// Days since the epoch of a hybrid-calendar date: Gregorian when the
/// result lies on or after the cutover, Julian otherwise (a date in the
/// ten-day gap lands in the Gregorian side, as ICU resolves it).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let g = days_from_gregorian(y, m, d);
    if g >= CUTOVER_DAYS {
        g
    } else {
        days_from_julian(y, m, d)
    }
}

pub fn is_leap(ext_year: i64, julian: bool) -> bool {
    if julian {
        ext_year.rem_euclid(4) == 0
    } else {
        ext_year.rem_euclid(4) == 0 && (ext_year.rem_euclid(100) != 0 || ext_year.rem_euclid(400) == 0)
    }
}

pub fn days_in_month(ext_year: i64, month: u32, julian: bool) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(ext_year, julian) {
                29
            } else {
                28
            }
        }
    }
}

/// The fields of an instant seen at a UTC offset.
pub fn fields_at(ts: i64, usec: u32, offset: i32) -> Fields {
    let local = ts + i64::from(offset);
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (ext_year, month, day, julian) = civil_from_days(days);
    let (era, year) = if ext_year > 0 { (1, ext_year) } else { (0, 1 - ext_year) };
    let year_start = if julian { days_from_julian(ext_year, 1, 1) } else { days_from_gregorian(ext_year, 1, 1) };
    Fields {
        days,
        ext_year,
        era,
        year,
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: ((secs % 3600) / 60) as u32,
        second: (secs % 60) as u32,
        millis: usec / 1000,
        weekday: (days + 4).rem_euclid(7) as u32,
        day_of_year: (days - year_start + 1) as u32,
        julian,
    }
}

/// The week data of a locale: the first day of the week (0 = Sunday) and
/// the minimal days in the first week.
#[derive(Clone, Copy, Debug)]
pub struct WeekData {
    pub first_day: u32,
    pub min_days: u32,
}

/// CLDR's regions whose weeks need four days to count as the first.
const MIN_DAYS_4: &[&str] = &[
    "AD", "AN", "AT", "AX", "BE", "BG", "CH", "CZ", "DE", "DK", "EE", "ES", "FI", "FJ", "FO", "FR", "GB", "GF", "GG", "GI", "GP", "GR", "HU", "IE", "IM", "IS", "IT", "JE", "LI", "LT", "LU", "MC", "MQ", "NL", "NO", "PL", "RE", "RU", "SE", "SJ", "SK", "SM", "VA",
];

impl WeekData {
    /// The week data of a locale, its region inferred where absent.
    pub fn for_locale(bcp47: &str) -> WeekData {
        let mut first_day = 1u32;
        let mut region = String::new();
        if let Ok(loc) = bcp47.parse::<icu::locale::Locale>() {
            let mut l = loc.clone();
            icu::locale::LocaleExpander::new_extended().maximize(&mut l.id);
            if let Some(r) = l.id.region {
                region = r.to_string();
            }
            let prefs = icu::calendar::week::WeekPreferences::from(&loc);
            if let Ok(info) = icu::calendar::week::WeekInformation::try_new(prefs) {
                first_day = match info.first_weekday {
                    icu::calendar::types::Weekday::Sunday => 0,
                    icu::calendar::types::Weekday::Monday => 1,
                    icu::calendar::types::Weekday::Tuesday => 2,
                    icu::calendar::types::Weekday::Wednesday => 3,
                    icu::calendar::types::Weekday::Thursday => 4,
                    icu::calendar::types::Weekday::Friday => 5,
                    icu::calendar::types::Weekday::Saturday => 6,
                };
            }
        }
        let min_days = if MIN_DAYS_4.contains(&region.as_str()) { 4 } else { 1 };
        WeekData { first_day, min_days }
    }

    /// ICU's `weekNumber`: the week a day of a period falls in, counting
    /// from the period's first day.
    fn week_number(&self, desired_day: u32, weekday: u32) -> u32 {
        let period_start = (weekday as i64 - self.first_day as i64 - desired_day as i64 + 1).rem_euclid(7) as u32;
        let mut week = (desired_day + period_start - 1) / 7;
        if 7 - period_start >= self.min_days {
            week += 1;
        }
        week
    }

    /// The week of the year and the week year of a date
    /// (`Calendar::computeWeekFields`).
    pub fn week_of_year(&self, f: &Fields) -> (u32, i64) {
        let year_len = |y: i64| if is_leap(y, f.julian) { 366 } else { 365 };
        let rel_dow = (f.weekday + 7 - self.first_day) % 7;
        let rel_dow_jan1 = (f.weekday as i64 - f.day_of_year as i64 + 7001 - self.first_day as i64).rem_euclid(7) as u32;
        let mut woy = (f.day_of_year - 1 + rel_dow_jan1) / 7;
        if 7 - rel_dow_jan1 >= self.min_days {
            woy += 1;
        }
        if woy == 0 {
            let prev_doy = f.day_of_year + year_len(f.ext_year - 1);
            return (self.week_number(prev_doy, f.weekday), f.ext_year - 1);
        }
        let last_doy = year_len(f.ext_year);
        if f.day_of_year >= last_doy - 5 {
            let last_rel_dow = (rel_dow + last_doy - f.day_of_year) % 7;
            if 6 - last_rel_dow >= self.min_days && f.day_of_year + 7 - rel_dow > last_doy {
                return (1, f.ext_year + 1);
            }
        }
        (woy, f.ext_year)
    }

    /// The week of the month.
    pub fn week_of_month(&self, f: &Fields) -> u32 {
        self.week_number(f.day, f.weekday)
    }
}
