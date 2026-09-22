//! `SimpleDateFormat::format`: a CLDR date pattern rendered field by
//! field, from the calendar fields of an instant, the locale's names
//! (`data/datenames.rs`) and the zone names CLDR spells (`zone.rs`).

use super::civil::{Fields, WeekData};
use super::zone::{self, Style};
use super::ZoneRef;
use crate::data::datenames;

/// Where each block of names starts in a locale's row.
mod idx {
    pub const MONTH_ABBR: usize = 0;
    pub const MONTH_WIDE: usize = 12;
    pub const MONTH_NARROW: usize = 24;
    pub const MONTH_SA_ABBR: usize = 36;
    pub const MONTH_SA_WIDE: usize = 48;
    pub const MONTH_SA_NARROW: usize = 60;
    pub const DAY_ABBR: usize = 72;
    pub const DAY_WIDE: usize = 79;
    pub const DAY_NARROW: usize = 86;
    pub const DAY_SHORT: usize = 93;
    pub const DAY_SA_ABBR: usize = 100;
    pub const DAY_SA_WIDE: usize = 107;
    pub const DAY_SA_NARROW: usize = 114;
    pub const DAY_SA_SHORT: usize = 121;
    pub const ERA_ABBR: usize = 128;
    pub const ERA_WIDE: usize = 130;
    pub const ERA_NARROW: usize = 132;
    pub const AMPM_ABBR: usize = 134;
    pub const AMPM_WIDE: usize = 136;
    pub const AMPM_NARROW: usize = 138;
    pub const NOON_ABBR: usize = 140;
    pub const NOON_WIDE: usize = 142;
    pub const NOON_NARROW: usize = 144;
    pub const PERIOD_ABBR: usize = 146;
    pub const PERIOD_WIDE: usize = 194;
    pub const PERIOD_NARROW: usize = 242;
    pub const QUARTER_ABBR: usize = 290;
    pub const QUARTER_WIDE: usize = 294;
    pub const QUARTER_SA_ABBR: usize = 298;
    pub const QUARTER_SA_WIDE: usize = 302;
    pub const RELATIVE_DAY: usize = 306;
    pub const RELATIVE_GLUE: usize = 311;
}

/// One run of a pattern: a field letter repeated, or literal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Field(char, usize),
    Literal(String),
}

/// The pattern letters ICU's date format accepts.
fn is_field(c: char) -> bool {
    matches!(
        c,
        'G' | 'y' | 'Y' | 'u' | 'U' | 'r' | 'Q' | 'q' | 'M' | 'L' | 'w' | 'W' | 'd' | 'D' | 'F' | 'g' | 'E' | 'e' | 'c' | 'a' | 'b' | 'B' | 'h' | 'H' | 'K' | 'k' | 'm' | 's' | 'S' | 'A' | 'z' | 'Z' | 'O' | 'v' | 'V' | 'X' | 'x'
    )
}

/// Split a pattern into its runs; `None` when it uses a letter no date
/// pattern may carry (`j`, `J`, `C` and the like), which ICU refuses.
pub fn items(pattern: &str) -> Option<Vec<Item>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out: Vec<Item> = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                lit.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        lit.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                lit.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            if !is_field(c) {
                return None;
            }
            let mut n = 0;
            while i < chars.len() && chars[i] == c {
                n += 1;
                i += 1;
            }
            if !lit.is_empty() {
                out.push(Item::Literal(std::mem::take(&mut lit)));
            }
            out.push(Item::Field(c, n));
            continue;
        }
        lit.push(c);
        i += 1;
    }
    if !lit.is_empty() {
        out.push(Item::Literal(lit));
    }
    Some(out)
}

/// What a render needs besides the instant.
pub struct Ctx<'a> {
    pub names: &'a [&'static str; datenames::N],
    pub week: WeekData,
    pub locale: &'a icu::locale::Locale,
    pub zone: &'a ZoneRef,
    /// The instant, its offset and the DST part of it.
    pub ts: i64,
    pub offset: i32,
    pub dst: i32,
    /// The digits `0`..`9` of the locale's numbering system.
    pub digits: [char; 10],
}

impl Ctx<'_> {
    fn num(&self, v: i64, width: usize) -> String {
        let neg = v < 0;
        let text = v.unsigned_abs().to_string();
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        for _ in text.len()..width {
            out.push(self.digits[0]);
        }
        for c in text.chars() {
            out.push(self.digits[(c as u8 - b'0') as usize]);
        }
        out
    }

    fn name(&self, i: usize) -> &str {
        self.names.get(i).copied().unwrap_or("")
    }
}

/// `SimpleDateFormat`'s year field: `yy` is the last two digits.
fn year_text(ctx: &Ctx, year: i64, n: usize) -> String {
    if n == 2 {
        ctx.num(year.rem_euclid(100), 2)
    } else {
        ctx.num(year, n)
    }
}

/// Render one field.
fn field(ctx: &Ctx, f: &Fields, c: char, n: usize) -> String {
    match c {
        'G' => {
            let base = match n {
                4 => idx::ERA_WIDE,
                5 => idx::ERA_NARROW,
                _ => idx::ERA_ABBR,
            };
            ctx.name(base + usize::from(f.era == 0)).to_string()
        }
        'y' => year_text(ctx, f.year, n),
        'u' => ctx.num(f.ext_year, n),
        'U' => year_text(ctx, f.ext_year, n),
        'r' => ctx.num(f.ext_year, n),
        'Y' => {
            let (_, wyear) = ctx.week.week_of_year(f);
            year_text(ctx, wyear, n)
        }
        'Q' | 'q' => {
            let q = ((f.month - 1) / 3) as usize;
            let standalone = c == 'q';
            match n {
                3 => ctx.name(if standalone { idx::QUARTER_SA_ABBR } else { idx::QUARTER_ABBR } + q).to_string(),
                4 => ctx.name(if standalone { idx::QUARTER_SA_WIDE } else { idx::QUARTER_WIDE } + q).to_string(),
                5 => ctx.num(q as i64 + 1, 1),
                _ => ctx.num(q as i64 + 1, n),
            }
        }
        'M' | 'L' => {
            let m = (f.month - 1) as usize;
            let standalone = c == 'L';
            match n {
                3 => ctx.name(if standalone { idx::MONTH_SA_ABBR } else { idx::MONTH_ABBR } + m).to_string(),
                4 => ctx.name(if standalone { idx::MONTH_SA_WIDE } else { idx::MONTH_WIDE } + m).to_string(),
                5 => ctx.name(if standalone { idx::MONTH_SA_NARROW } else { idx::MONTH_NARROW } + m).to_string(),
                _ => ctx.num(i64::from(f.month), n),
            }
        }
        'w' => {
            let (w, _) = ctx.week.week_of_year(f);
            ctx.num(i64::from(w), n)
        }
        'W' => ctx.num(i64::from(ctx.week.week_of_month(f)), n),
        'd' => ctx.num(i64::from(f.day), n),
        'D' => ctx.num(i64::from(f.day_of_year), n),
        'F' => ctx.num(i64::from((f.day - 1) / 7 + 1), n),
        'g' => ctx.num(f.days + 2_440_588, n),
        'E' | 'c' | 'e' => {
            let d = f.weekday as usize;
            let standalone = c == 'c';
            if (c == 'e' || c == 'c') && n <= 2 {
                // the local day of the week, counted from the locale's first day
                let local = (f.weekday + 7 - ctx.week.first_day) % 7 + 1;
                return ctx.num(i64::from(local), if c == 'c' { 1 } else { n });
            }
            match n {
                4 => ctx.name(if standalone { idx::DAY_SA_WIDE } else { idx::DAY_WIDE } + d).to_string(),
                5 => ctx.name(if standalone { idx::DAY_SA_NARROW } else { idx::DAY_NARROW } + d).to_string(),
                6 => ctx.name(if standalone { idx::DAY_SA_SHORT } else { idx::DAY_SHORT } + d).to_string(),
                _ => ctx.name(if standalone { idx::DAY_SA_ABBR } else { idx::DAY_ABBR } + d).to_string(),
            }
        }
        'a' => {
            let base = match n {
                4 => idx::AMPM_WIDE,
                5 => idx::AMPM_NARROW,
                _ => idx::AMPM_ABBR,
            };
            ctx.name(base + usize::from(f.hour >= 12)).to_string()
        }
        'b' => {
            let exact = if f.minute == 0 && f.second == 0 && f.millis == 0 {
                match f.hour {
                    0 => Some(0),
                    12 => Some(1),
                    _ => None,
                }
            } else {
                None
            };
            match exact {
                Some(k) => {
                    let base = match n {
                        4 => idx::NOON_WIDE,
                        5 => idx::NOON_NARROW,
                        _ => idx::NOON_ABBR,
                    };
                    ctx.name(base + k).to_string()
                }
                None => field(ctx, f, 'a', n),
            }
        }
        'B' => {
            let base = match n {
                4 => idx::PERIOD_WIDE,
                5 => idx::PERIOD_NARROW,
                _ => idx::PERIOD_ABBR,
            };
            let half = usize::from(f.minute >= 30);
            ctx.name(base + f.hour as usize * 2 + half).to_string()
        }
        'h' => ctx.num(i64::from(if f.hour % 12 == 0 { 12 } else { f.hour % 12 }), n),
        'H' => ctx.num(i64::from(f.hour), n),
        'K' => ctx.num(i64::from(f.hour % 12), n),
        'k' => ctx.num(i64::from(if f.hour == 0 { 24 } else { f.hour }), n),
        'm' => ctx.num(i64::from(f.minute), n),
        's' => ctx.num(i64::from(f.second), n),
        'S' => {
            // the fraction of a second, truncated or zero-padded to `n`
            let mut text = format!("{:03}", f.millis);
            while text.len() < n {
                text.push('0');
            }
            text.truncate(n);
            text.chars().map(|c| ctx.digits[(c as u8 - b'0') as usize]).collect()
        }
        'A' => {
            let ms = (i64::from(f.hour) * 3600 + i64::from(f.minute) * 60 + i64::from(f.second)) * 1000 + i64::from(f.millis);
            ctx.num(ms, n)
        }
        'z' | 'v' | 'V' | 'O' | 'Z' | 'X' | 'x' => zone_field(ctx, c, n),
        _ => String::new(),
    }
}

/// The zone fields (`z v V O Z X x`).
fn zone_field(ctx: &Ctx, c: char, n: usize) -> String {
    let iana = ctx.zone.iana.as_str();
    let named = |style: Style| -> Option<String> {
        if iana.is_empty() {
            return None;
        }
        zone::name(ctx.locale, iana, ctx.offset, ctx.ts, style)
    };
    let offset_long = || named(Style::OffsetLong).unwrap_or_else(|| gmt_offset(ctx.offset, false));
    let offset_short = || named(Style::OffsetShort).unwrap_or_else(|| gmt_offset(ctx.offset, true));
    match c {
        'z' => match n {
            4 | 5 => named(Style::SpecificLong).unwrap_or_else(offset_long),
            _ => named(Style::SpecificShort).unwrap_or_else(offset_short),
        },
        'v' => match n {
            1 => named(Style::GenericShort).unwrap_or_else(offset_short),
            4 => named(Style::GenericLong).unwrap_or_else(offset_long),
            _ => String::new(),
        },
        'V' => match n {
            1 => ctx.zone.bcp47.clone(),
            2 => {
                if iana.is_empty() {
                    ctx.zone.id.clone()
                } else {
                    iana.to_string()
                }
            }
            3 => named(Style::ExemplarCity).unwrap_or_default(),
            4 => named(Style::Location).unwrap_or_else(offset_long),
            _ => String::new(),
        },
        'O' => match n {
            1 => offset_short(),
            4 => offset_long(),
            _ => String::new(),
        },
        'Z' => match n {
            1..=3 => zone::iso_offset(ctx.offset, true, false, 2, false),
            5 => zone::iso_offset(ctx.offset, false, true, 3, false),
            _ => offset_long(),
        },
        'X' => match n {
            1 => zone::iso_offset(ctx.offset, true, true, 2, true),
            2 => zone::iso_offset(ctx.offset, true, true, 2, false),
            3 => zone::iso_offset(ctx.offset, false, true, 2, false),
            4 => zone::iso_offset(ctx.offset, true, true, 3, false),
            5 => zone::iso_offset(ctx.offset, false, true, 3, false),
            _ => String::new(),
        },
        'x' => match n {
            1 => zone::iso_offset(ctx.offset, true, false, 2, true),
            2 => zone::iso_offset(ctx.offset, true, false, 2, false),
            3 => zone::iso_offset(ctx.offset, false, false, 2, false),
            4 => zone::iso_offset(ctx.offset, true, false, 3, false),
            5 => zone::iso_offset(ctx.offset, false, false, 3, false),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// The `GMT±h:mm` fallback when CLDR has no name (`root`'s spelling).
fn gmt_offset(secs: i32, short: bool) -> String {
    if secs == 0 {
        return "GMT".to_string();
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if short && m == 0 && s == 0 {
        format!("GMT{sign}{h}")
    } else if s == 0 {
        format!("GMT{sign}{h:02}:{m:02}")
    } else {
        format!("GMT{sign}{h:02}:{m:02}:{s:02}")
    }
}

/// Render a whole pattern.
pub fn render(ctx: &Ctx, f: &Fields, items: &[Item]) -> String {
    let mut out = String::new();
    for item in items {
        match item {
            Item::Literal(text) => out.push_str(text),
            Item::Field(c, n) => out.push_str(&field(ctx, f, *c, *n)),
        }
    }
    out
}
