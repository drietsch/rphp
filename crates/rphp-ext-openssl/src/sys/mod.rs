//! The raw OpenSSL calls rust-openssl's safe API does not wrap. The only
//! module of the crate allowed `unsafe`: every call states why it is sound,
//! and each area keeps its calls in a file of its own (`sys/<area>.rs`).
#![allow(unsafe_code)]
