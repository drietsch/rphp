//! `pcre` extension — `preg_*` over PCRE2 (the engine php-src links, so pattern
//! semantics match). (Stub: filled by the Tier-A burn-down. `preg_match`'s
//! `$matches` out-param and `preg_match_all` wait on by-reference parameters.)
use crate::NativeFn;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[];
