//! Test harness library.
//!
//! * [`phpt`] — the php-src `.phpt` runner (sections, EXPECTF wildcards,
//!   SKIPIF/INI/ENV/STDIN…), driven by `cargo xtask phpt`.
//! * [`differential`] — the fuzzy differential oracle (ADR-008): byte-exact
//!   stdout by default, normalized stderr, exact exit codes, and a closed-set
//!   divergence allowlist. Used by `tools/rphp/tests/differential.rs` and the
//!   ladder runner.
//!
//! The two modules are owned by different workstreams; keep them independent.
#![forbid(unsafe_code)]

pub mod differential;
pub mod phpt;
