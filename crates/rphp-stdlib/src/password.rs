//! Stub module reserved for the P4 stdlib wave — filled in by its workstream.

use rphp_runtime::{NativeFn, Registry};

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides.
pub(crate) fn register_constants(_r: &mut Registry) {}

/// Classes this module provides.
pub(crate) fn register_classes(_r: &mut Registry) {}
