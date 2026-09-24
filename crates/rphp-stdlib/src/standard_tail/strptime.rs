//! `strptime()` (deprecated since 8.1): php hands the string to the C
//! library's `strptime(3)`, so this is the macOS one — FreeBSD's parser
//! with Apple's older `%Z`, `%z`, `%n` and range checks — in the C locale,
//! with its post-pass that derives the day of the year, month, day and
//! weekday from what was parsed, and the `timegm()` + `localtime()`
//! round trip a `%Z` of `GMT`, a `%z` or a `%s` asks for.
//!
//! **Divergence:** the local time zone of that round trip, and the zone
//! names `%Z` accepts besides `GMT`, are UTC's: rphp does not consult the
//! C library's `TZ`.

/// The fields of a `struct tm` php reports.
#[derive(Clone, Copy, Default)]
pub(super) struct Tm {
    pub sec: i64,
    pub min: i64,
    pub hour: i64,
    pub mday: i64,
    pub mon: i64,
    pub year: i64,
    pub wday: i64,
    pub yday: i64,
}

const FLAG_YEAR: u32 = 1 << 1;
const FLAG_MONTH: u32 = 1 << 2;
const FLAG_YDAY: u32 = 1 << 3;
const FLAG_MDAY: u32 = 1 << 4;
const FLAG_WDAY: u32 = 1 << 5;

const WEEKDAY: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const WDAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTH: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];
const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const START_OF_MONTH: [[i64; 13]; 2] = [
    [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365],
    [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335, 366],
];

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn leap_idx(tm_year: i64) -> usize {
    usize::from(is_leap(tm_year + 1900))
}

/// The weekday of January 1st of `year` (FreeBSD's `first_wday_of`).
fn first_wday_of(year: i64) -> i64 {
    (2 * (3 - (year / 100) % 4) + (year % 100) + ((year % 100) / 4) + if is_leap(year) { 6 } else { 0 } + 1)
        .rem_euclid(7)
}

fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Up to `len` digits at `*b` (the caller checked the first is one).
fn digits(s: &[u8], b: &mut usize, mut len: u32) -> i64 {
    let mut i = 0i64;
    while len > 0 && at(s, *b).is_ascii_digit() {
        i = i * 10 + i64::from(at(s, *b) - b'0');
        *b += 1;
        len -= 1;
    }
    i
}

fn prefix_ci(s: &[u8], b: usize, word: &str) -> bool {
    let w = word.as_bytes();
    s.len() >= b + w.len() && s[b..b + w.len()].eq_ignore_ascii_case(w)
}

struct State {
    flags: u32,
    century: i64,
    year: i64,
    day_offset: i64,
    week_offset: i64,
    gmt: bool,
}

/// One level of `_strptime()`: the offset in `s` where parsing stopped,
/// or `None` for a mismatch.
fn parse(s: &[u8], mut b: usize, fmt: &[u8], tm: &mut Tm, st: &mut State) -> Option<usize> {
    let mut p = 0usize;
    while p < fmt.len() {
        let c = fmt[p];
        p += 1;
        if c != b'%' {
            if is_space(c) {
                while at(s, b) != 0 && is_space(at(s, b)) {
                    b += 1;
                }
            } else {
                if c != at(s, b) {
                    return None;
                }
                b += 1;
            }
            continue;
        }
        let mut alt = false;
        let mut c = at(fmt, p);
        p += 1;
        while matches!(c, b'E' | b'O') && !alt {
            alt = true;
            c = at(fmt, p);
            p += 1;
        }
        let sub = |fmt: &[u8], b: usize, tm: &mut Tm, st: &mut State| parse(s, b, fmt, tm, st);
        match c {
            0 | b'%' => {
                if at(s, b) != b'%' {
                    return None;
                }
                b += 1;
            }
            b'+' => {
                b = sub(b"%a %b %e %H:%M:%S %Z %Y", b, tm, st)?;
                st.flags |= FLAG_WDAY | FLAG_MONTH | FLAG_MDAY | FLAG_YEAR;
            }
            b'C' => {
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                st.century = digits(s, &mut b, 2);
                st.flags |= FLAG_YEAR;
            }
            b'c' => {
                b = sub(b"%a %b %e %H:%M:%S %Y", b, tm, st)?;
                st.flags |= FLAG_WDAY | FLAG_MONTH | FLAG_MDAY | FLAG_YEAR;
            }
            b'D' | b'x' => {
                b = sub(b"%m/%d/%y", b, tm, st)?;
                st.flags |= FLAG_MONTH | FLAG_MDAY | FLAG_YEAR;
            }
            b'F' => {
                b = sub(b"%Y-%m-%d", b, tm, st)?;
                st.flags |= FLAG_MONTH | FLAG_MDAY | FLAG_YEAR;
            }
            b'R' => b = sub(b"%H:%M", b, tm, st)?,
            b'r' => b = sub(b"%I:%M:%S %p", b, tm, st)?,
            b'T' | b'X' => b = sub(b"%H:%M:%S", b, tm, st)?,
            b'j' => {
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, 3);
                if !(1..=366).contains(&i) {
                    return None;
                }
                tm.yday = i - 1;
                st.flags |= FLAG_YDAY;
            }
            b'M' | b'S' => {
                if at(s, b) == 0 || is_space(at(s, b)) {
                    continue;
                }
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, 2);
                if c == b'M' {
                    if i > 59 {
                        return None;
                    }
                    tm.min = i;
                } else {
                    if i > 60 {
                        return None;
                    }
                    tm.sec = i;
                }
            }
            b'H' | b'I' | b'k' | b'l' => {
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, 2);
                if matches!(c, b'H' | b'k') {
                    if i > 23 {
                        return None;
                    }
                } else if i > 12 {
                    return None;
                }
                tm.hour = i;
            }
            b'p' => {
                if prefix_ci(s, b, "AM") {
                    if tm.hour > 12 {
                        return None;
                    }
                    if tm.hour == 12 {
                        tm.hour = 0;
                    }
                    b += 2;
                } else if prefix_ci(s, b, "PM") {
                    if tm.hour > 12 {
                        return None;
                    }
                    if tm.hour != 12 {
                        tm.hour += 12;
                    }
                    b += 2;
                } else {
                    return None;
                }
            }
            b'A' | b'a' => {
                let mut hit = None;
                for i in 0..7 {
                    if prefix_ci(s, b, WEEKDAY[i]) {
                        hit = Some((i, WEEKDAY[i].len()));
                        break;
                    }
                    if prefix_ci(s, b, WDAY[i]) {
                        hit = Some((i, WDAY[i].len()));
                        break;
                    }
                }
                let (i, len) = hit?;
                b += len;
                tm.wday = i as i64;
                st.flags |= FLAG_WDAY;
            }
            b'U' | b'W' => {
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, 2);
                if i > 53 {
                    return None;
                }
                st.day_offset = if c == b'U' { 0 } else { 1 };
                st.week_offset = i;
            }
            b'u' | b'w' => {
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = i64::from(at(s, b) - b'0');
                b += 1;
                if (c == b'u' && !(1..=7).contains(&i)) || (c == b'w' && i > 6) {
                    return None;
                }
                tm.wday = i % 7;
                st.flags |= FLAG_WDAY;
            }
            b'd' | b'e' => {
                let mut len = 2;
                if is_space(at(s, b)) && at(s, b + 1).is_ascii_digit() && !at(s, b + 2).is_ascii_digit() {
                    len = 1;
                    b += 1;
                }
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, len);
                if i > 31 {
                    return None;
                }
                tm.mday = i;
                st.flags |= FLAG_MDAY;
            }
            b'B' | b'b' | b'h' => {
                let full = (0..12).find(|&i| prefix_ci(s, b, MONTH[i])).map(|i| (i, MONTH[i].len()));
                let (i, len) = full.or_else(|| (0..12).find(|&i| prefix_ci(s, b, MON[i])).map(|i| (i, 3)))?;
                tm.mon = i as i64;
                b += len;
                st.flags |= FLAG_MONTH;
            }
            b'm' => {
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, 2);
                if !(1..=12).contains(&i) {
                    return None;
                }
                tm.mon = i - 1;
                st.flags |= FLAG_MONTH;
            }
            b's' => {
                // strtol(): optional space and sign, then digits.
                let mut e = b;
                while is_space(at(s, e)) {
                    e += 1;
                }
                let neg = at(s, e) == b'-';
                if matches!(at(s, e), b'+' | b'-') {
                    e += 1;
                }
                let start = e;
                let mut n: i64 = 0;
                while at(s, e).is_ascii_digit() {
                    n = n.checked_mul(10)?.checked_add(i64::from(at(s, e) - b'0'))?;
                    e += 1;
                }
                if e > start {
                    b = e;
                } else {
                    n = 0;
                }
                *tm = gmtime(if neg { -n } else { n });
                st.gmt = true;
                st.flags |= FLAG_YDAY | FLAG_WDAY | FLAG_MONTH | FLAG_MDAY | FLAG_YEAR;
            }
            b'Y' | b'y' => {
                if at(s, b) == 0 || is_space(at(s, b)) {
                    continue;
                }
                if !at(s, b).is_ascii_digit() {
                    return None;
                }
                let i = digits(s, &mut b, if c == b'Y' { 4 } else { 2 });
                if c == b'Y' {
                    st.century = i / 100;
                }
                st.year = i % 100;
                st.flags |= FLAG_YEAR;
            }
            b'Z' => {
                let n = s[b.min(s.len())..].iter().take_while(|c| c.is_ascii_uppercase()).count();
                let zone = &s[b..b + n];
                if zone == b"GMT" {
                    st.gmt = true;
                } else if zone != b"UTC" {
                    // The C library's `tzname` pair (both "UTC" here).
                    return None;
                }
                b += n;
            }
            b'z' => {
                let sign = match at(s, b) {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                b += 1;
                let mut i = 0;
                for _ in 0..4 {
                    if !at(s, b).is_ascii_digit() {
                        return None;
                    }
                    i = i * 10 + i64::from(at(s, b) - b'0');
                    b += 1;
                }
                tm.hour -= sign * (i / 100);
                tm.min -= sign * (i % 100);
                st.gmt = true;
            }
            b'n' | b't' => {
                if !is_space(at(s, b)) {
                    return None;
                }
                while is_space(at(s, b)) {
                    b += 1;
                }
            }
            _ => return None,
        }
    }
    Some(b)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `gmtime_r()` of `t`.
fn gmtime(t: i64) -> Tm {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Tm {
        sec: secs % 60,
        min: (secs / 60) % 60,
        hour: secs / 3600,
        mday: d,
        mon: m - 1,
        year: y - 1900,
        wday: (days + 4).rem_euclid(7),
        yday: days - days_from_civil(y, 1, 1),
    }
}

/// `timegm()`: the normalised broken-down time as seconds, or -1 before
/// 1900 (where macOS's `timegm` gives up).
fn timegm(tm: &Tm) -> i64 {
    let mon = tm.mon.rem_euclid(12);
    let year = tm.year + 1900 + tm.mon.div_euclid(12);
    let days = days_from_civil(year, mon + 1, 1) + tm.mday - 1;
    let t = days * 86400 + tm.hour * 3600 + tm.min * 60 + tm.sec;
    if t < days_from_civil(1900, 1, 1) * 86400 {
        -1
    } else {
        t
    }
}

/// `strptime(ts, format)` into a zeroed `tm`: the fields and the unparsed
/// rest, or `None` where the C library returns `NULL`.
pub(super) fn strptime(ts: &[u8], format: &[u8]) -> Option<(Tm, Vec<u8>)> {
    // Both are C strings.
    let cut = |b: &[u8]| b.iter().position(|&c| c == 0).map_or(b.to_vec(), |n| b[..n].to_vec());
    let (s, fmt) = (cut(ts), cut(format));
    let mut tm = Tm::default();
    let mut st = State { flags: 0, century: -1, year: -1, day_offset: -1, week_offset: 0, gmt: false };
    let end = parse(&s, 0, &fmt, &mut tm, &mut st)?;
    if st.century != -1 || st.year != -1 {
        let mut year = if st.year == -1 { 0 } else { st.year };
        if st.century == -1 {
            if year < 69 {
                year += 100;
            }
        } else {
            year += st.century * 100 - 1900;
        }
        tm.year = year;
    }
    let mut flags = st.flags;
    if flags & FLAG_YDAY == 0 && flags & FLAG_YEAR != 0 {
        if flags & (FLAG_MONTH | FLAG_MDAY) == FLAG_MONTH | FLAG_MDAY {
            tm.yday = START_OF_MONTH[leap_idx(tm.year)][tm.mon as usize] + (tm.mday - 1);
            flags |= FLAG_YDAY;
        } else if st.day_offset != -1 {
            let fwo = first_wday_of(tm.year + 1900);
            let wday = if flags & FLAG_WDAY != 0 { tm.wday } else { st.day_offset };
            let yday = (7 - fwo + st.day_offset) % 7 + (st.week_offset - 1) * 7 + (wday - st.day_offset + 7) % 7;
            if yday < 0 {
                return None;
            }
            tm.yday = yday;
            flags |= FLAG_YDAY;
        }
    }
    if flags & (FLAG_YEAR | FLAG_YDAY) == FLAG_YEAR | FLAG_YDAY {
        if flags & FLAG_MONTH == 0 {
            let mut i = 0;
            while i < 13 && tm.yday >= START_OF_MONTH[leap_idx(tm.year)][i] {
                i += 1;
            }
            if i > 12 {
                i = 1;
                tm.yday -= START_OF_MONTH[leap_idx(tm.year)][12];
                tm.year += 1;
            }
            tm.mon = i as i64 - 1;
            flags |= FLAG_MONTH;
        }
        if flags & FLAG_MDAY == 0 {
            tm.mday = tm.yday - START_OF_MONTH[leap_idx(tm.year)][tm.mon as usize] + 1;
            flags |= FLAG_MDAY;
        }
        if flags & FLAG_WDAY == 0 {
            tm.wday = (first_wday_of(tm.year + 1900) + tm.yday).rem_euclid(7);
        }
    }
    if st.gmt {
        tm = gmtime(timegm(&tm));
    }
    Some((tm, s[end.min(s.len())..].to_vec()))
}
