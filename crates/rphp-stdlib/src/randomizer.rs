//! `ext/random`'s object half: `Random\Randomizer`, the engines
//! (`Mt19937`, `PcgOneseq128XslRr64`, `Xoshiro256StarStar`, `Secure`) and
//! `Random\IntervalBoundary`.

use rphp_runtime::{NativeFn, Registry};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

pub(crate) fn register_classes(_r: &mut Registry) {}
