//! The embedding API every SAPI sits on (spec 09, ADR-014/033): wires the
//! front end, the engine (`rphp-runtime`) and the extension bundle
//! (`rphp-stdlib`) into an `Engine` (per process) that produces `Interp`s (per
//! request/script). Stub — filled in by workstream E1/SAPI-1.
#![forbid(unsafe_code)]
