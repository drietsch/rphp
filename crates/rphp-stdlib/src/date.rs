//! php-src `ext/date`: the calendar, the timezone database and the string
//! parser behind `date()`, `mktime()` and `strtotime()`.
//!
//! The extension is split the way php splits it. [`civil`] is proleptic
//! Gregorian arithmetic in days since the epoch, unbounded because php's
//! calendar is (`date("Y", PHP_INT_MAX)` is the year 292 277 026 596).
//! [`tz`] is the timezone model — php's three `timezone_type` shapes, the
//! identifier and abbreviation tables php ships, and the rule that resolves a
//! wall clock across a DST transition. [`format`] is `date()`'s format
//! characters. [`parse`] is the longest-match scanner `strtotime()` and
//! `date_parse()` run. [`funcs`] is the PHP-visible surface.
//!
//! [`classes`] is the object surface — the `DateTime` family,
//! `DateTimeZone`, `DateInterval`, `DatePeriod`, their exception tree and
//! the procedural aliases php ships beside them.
//!
//! The clock functions php-src also keeps in this extension — `time`,
//! `microtime`, `gettimeofday`, `hrtime`, `sleep` — live in `uniqid.rs`
//! here.

use rphp_runtime::{NativeFn, Registry};

mod civil;
mod classes;
mod format;
mod funcs;
mod parse;
mod tz;

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = funcs::FUNCTIONS;

/// The procedural aliases of the date classes (`date_create`, `date_diff`,
/// `timezone_open`, …). They are a second slice because a `static` cannot
/// concatenate two others, so `lib.rs`'s table needs a row of its own:
/// `date::CLASS_FUNCTIONS` beside `date::FUNCTIONS`. The attribute goes
/// with that row.
pub(crate) static CLASS_FUNCTIONS: &[NativeFn] = classes::FUNCTIONS;

/// Constants this module provides.
pub(crate) fn register_constants(r: &mut Registry) {
    funcs::register_constants(r)
}

/// Classes this module provides: the `DateTime` family, `DateTimeZone`,
/// `DateInterval`, `DatePeriod` and `ext/date`'s exception tree.
pub(crate) fn register_classes(r: &mut Registry) {
    classes::register_classes(r)
}
