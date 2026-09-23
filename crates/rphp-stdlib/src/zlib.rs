//! `ext/zlib`: the `gz*`/`zlib_*` functions, the incremental
//! `deflate_*`/`inflate_*` contexts, `gzopen` and `compress.zlib://`.

use rphp_runtime::{NativeFn, Registry};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

pub(crate) fn register_classes(_r: &mut Registry) {}

pub(crate) fn register_constants(_r: &mut Registry) {}
