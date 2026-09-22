//! Time zone names as CLDR spells them, through ICU4X's zone field sets:
//! the specific names (`EST`, `Eastern Standard Time`), the generic ones
//! (`ET`, `Eastern Time`), the location formats (`New York`, `New York
//! Time`) and the localized offsets (`GMT-5`, `GMT-05:00`) — the texts
//! behind a date pattern's `z v V O` fields and `IntlTimeZone::
//! getDisplayName()`.

use icu::datetime::fieldsets::zone;
use icu::datetime::NoCalendarFormatter;
use icu::time::zone::{UtcOffset, ZoneNameTimestamp};
use icu::time::{TimeZone, TimeZoneInfo};

/// Which name a caller wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    SpecificShort,
    SpecificLong,
    /// The daylight variant of the two specific names, which ICU's
    /// `getDisplayName(true, …)` asks for whether or not the zone is on
    /// daylight time right now.
    DaylightShort,
    DaylightLong,
    GenericShort,
    GenericLong,
    Location,
    ExemplarCity,
    OffsetShort,
    OffsetLong,
}

/// The zone as ICU4X wants it: the IANA id, the offset in force and the
/// instant the name is asked about.
fn zone_info(iana: &str, offset_secs: i32, ts: i64) -> TimeZoneInfo<icu::time::zone::models::AtTime> {
    let zone = if iana.is_empty() { TimeZone::UNKNOWN } else { TimeZone::from_iana_id(iana) };
    zone
        .with_offset(UtcOffset::try_from_seconds(offset_secs).ok())
        .with_zone_name_timestamp(ZoneNameTimestamp::from_epoch_seconds(ts))
}

/// A zone's name in a locale, or `None` when CLDR has none of that kind
/// (the caller then falls back the way ICU does).
pub fn name(locale: &icu::locale::Locale, iana: &str, offset_secs: i32, ts: i64, style: Style) -> Option<String> {
    // CLDR gives `Etc/UTC` no generic name, so ICU prints its offset
    let iana = if matches!(style, Style::GenericShort | Style::GenericLong) && iana == "Etc/UTC" { "" } else { iana };
    let prefs: icu::datetime::DateTimeFormatterPreferences = locale.into();
    let info = zone_info(iana, offset_secs, ts);
    let text = match style {
        Style::DaylightShort => NoCalendarFormatter::try_new(prefs, zone::SpecificShort).ok()?.format(&info).to_string(),
        Style::DaylightLong => NoCalendarFormatter::try_new(prefs, zone::SpecificLong).ok()?.format(&info).to_string(),
        Style::SpecificShort => NoCalendarFormatter::try_new(prefs, zone::SpecificShort).ok()?.format(&info).to_string(),
        Style::SpecificLong => NoCalendarFormatter::try_new(prefs, zone::SpecificLong).ok()?.format(&info).to_string(),
        Style::GenericShort => NoCalendarFormatter::try_new(prefs, zone::GenericShort).ok()?.format(&info).to_string(),
        Style::GenericLong => NoCalendarFormatter::try_new(prefs, zone::GenericLong).ok()?.format(&info).to_string(),
        Style::Location => NoCalendarFormatter::try_new(prefs, zone::Location).ok()?.format(&info).to_string(),
        Style::ExemplarCity => NoCalendarFormatter::try_new(prefs, zone::ExemplarCity).ok()?.format(&info).to_string(),
        Style::OffsetShort => NoCalendarFormatter::try_new(prefs, zone::LocalizedOffsetShort).ok()?.format(&info).to_string(),
        Style::OffsetLong => NoCalendarFormatter::try_new(prefs, zone::LocalizedOffsetLong).ok()?.format(&info).to_string(),
    };
    Some(text)
}

/// `Z`, `ZZZZZ`, `X`, `x`: the ISO 8601 spellings of an offset. `basic`
/// omits the colon, `z_for_zero` writes `Z` for UTC, `fields` is how many
/// of hours/minutes/seconds to write (2 = ±hhmm, 1 = ±hh, 3 = ±hhmmss).
pub fn iso_offset(offset_secs: i32, basic: bool, z_for_zero: bool, fields: u8, optional_minutes: bool) -> String {
    if z_for_zero && offset_secs == 0 {
        return "Z".to_string();
    }
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let a = offset_secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    let sep = if basic { "" } else { ":" };
    let mut out = format!("{sign}{h:02}");
    if fields >= 2 && !(optional_minutes && m == 0 && s == 0) {
        out.push_str(sep);
        out.push_str(&format!("{m:02}"));
        if fields >= 3 && s != 0 {
            out.push_str(sep);
            out.push_str(&format!("{s:02}"));
        }
    }
    out
}
