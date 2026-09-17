//! Stub module reserved for a stdlib wave — filled in by its workstream.

use rphp_runtime::{NativeFn, Registry};

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides.
pub(crate) fn register_constants(_r: &mut Registry) {}
