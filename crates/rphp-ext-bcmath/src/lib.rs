//! `ext/bcmath`: arbitrary-precision decimal arithmetic — the `bc*`
//! functions and php 8.4's `BcMath\Number`.
#![forbid(unsafe_code)]

use rphp_runtime::Registry;

/// Register the extension.
pub fn register(r: &mut Registry) {
    r.extension("bcmath");
}
