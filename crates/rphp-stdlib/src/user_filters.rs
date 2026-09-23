//! User stream filters: `stream_filter_register()`, `php_user_filter`
//! and the `stream_bucket_*` functions.

use rphp_runtime::{NativeFn, Registry};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

pub(crate) fn register_classes(_r: &mut Registry) {}

pub(crate) fn register_constants(_r: &mut Registry) {}
