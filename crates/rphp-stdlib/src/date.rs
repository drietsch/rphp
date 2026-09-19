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
//! The clock functions php-src also keeps in this extension — `time`,
//! `microtime`, `gettimeofday`, `hrtime`, `sleep` — live in `uniqid.rs`
//! here, and the `DateTime` family arrives with the class wave.

use rphp_runtime::{NativeFn, Registry};

mod civil;
mod format;
mod funcs;
mod parse;
mod tz;

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = funcs::FUNCTIONS;

/// Constants this module provides.
pub(crate) fn register_constants(r: &mut Registry) {
    funcs::register_constants(r)
}

/// Classes this module provides: none yet — `DateTime`, `DateTimeImmutable`,
/// `DateTimeZone`, `DateInterval` and `DatePeriod` are the next wave.
pub(crate) fn register_classes(_r: &mut Registry) {}
