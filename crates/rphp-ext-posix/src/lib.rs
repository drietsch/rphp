//! `ext/posix` and the signal half of `ext/pcntl`.
#![forbid(unsafe_code)]

use rphp_runtime::Registry;

/// Register the extensions.
pub fn register(r: &mut Registry) {
    r.extension("posix");
    r.extension("pcntl");
}
