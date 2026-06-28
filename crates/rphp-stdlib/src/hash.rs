//! `hash` extension — message digests (`md5`, `sha1`, `crc32`, the generic
//! `hash`/`hash_algos`). Byte-oriented like the rest of stdlib: the input is the
//! `(string)` cast of each argument, the result is a lowercase-hex string, or the
//! raw digest bytes when the `$binary` flag is set. Backed by the pure-Rust
//! `md-5`, `sha1`, `sha2`, and `crc32fast` crates (no `unsafe`, no OpenSSL).
use crc32fast::Hasher as Crc32;
use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::{Sha256, Sha384, Sha512};

use rphp_value::{Array, Str, Value};

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`). Keyed-hash and
/// incremental-state APIs (`hash_hmac`, `hash_init`/`hash_update`/`hash_final`)
/// wait on objects and resource handles; only the pure one-shot digests live
/// here.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("md5", 1, Some(2), md5),
    nf!("sha1", 1, Some(2), sha1),
    nf!("crc32", 1, Some(1), crc32),
    nf!("hash", 2, Some(3), hash),
    nf!("hash_algos", 0, Some(0), hash_algos),
];

/// The byte string an argument coerces to (the `(string)` cast), so any scalar
/// can be hashed the way PHP's weak typing allows.
fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

pub(crate) fn md5(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let binary = args.get(1).is_some_and(Value::to_bool);
    Ok(digest_value(&one_shot(Md5::new(), &bytes(&args[0])), binary))
}

pub(crate) fn sha1(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let binary = args.get(1).is_some_and(Value::to_bool);
    Ok(digest_value(&one_shot(Sha1::new(), &bytes(&args[0])), binary))
}

pub(crate) fn crc32(_: &mut Ctx, args: &[Value]) -> NativeResult {
    // The standard reflected CRC-32 (IEEE 802.3), returned as a plain int. PHP
    // hands back the unsigned 32-bit value, which on a 64-bit build is just the
    // u32 widened (never negative).
    Ok(Value::Int(crc32_of(&bytes(&args[0])) as i64))
}

pub(crate) fn hash(_: &mut Ctx, args: &[Value]) -> NativeResult {
    let data = bytes(&args[1]);
    let binary = args.get(2).is_some_and(Value::to_bool);
    // PHP resolves the algorithm name case-insensitively (`hash("MD5", …)` works
    // even though `hash_algos()` only lists the lowercase spelling).
    let raw = match bytes(&args[0]).to_ascii_lowercase().as_slice() {
        b"md5" => one_shot(Md5::new(), &data),
        b"sha1" => one_shot(Sha1::new(), &data),
        b"sha256" => one_shot(Sha256::new(), &data),
        b"sha384" => one_shot(Sha384::new(), &data),
        b"sha512" => one_shot(Sha512::new(), &data),
        // "crc32b" is the reflected CRC-32; its digest is the 4-byte big-endian
        // encoding of the integer `crc32()` returns. (PHP's "crc32" alias is a
        // different, non-reflected variant — see the module caveats.)
        b"crc32b" => crc32_of(&data).to_be_bytes().to_vec(),
        _ => {
            return Err(NativeError::new(
                "hash(): Argument #1 ($algo) must be a valid hashing algorithm",
            ))
        }
    };
    Ok(digest_value(&raw, binary))
}

pub(crate) fn hash_algos(_: &mut Ctx, _args: &[Value]) -> NativeResult {
    let mut out = Array::new();
    for name in SUPPORTED_ALGOS {
        out.push(Value::string(name.as_bytes()));
    }
    Ok(Value::Array(out))
}

// ---- helpers ----------------------------------------------------------------

/// The algorithm names `hash()` accepts, in registry order. A strict subset of
/// stock PHP's much larger list (see caveats).
const SUPPORTED_ALGOS: &[&str] = &["md5", "sha1", "sha256", "sha384", "sha512", "crc32b"];

/// Run a one-shot digest over `data`, returning the raw digest bytes. Generic
/// over every `RustCrypto` digest (they share the `Digest` trait).
fn one_shot<D: Digest>(mut h: D, data: &[u8]) -> Vec<u8> {
    h.update(data);
    h.finalize().to_vec()
}

/// The standard reflected CRC-32 of `data` as an unsigned 32-bit value.
fn crc32_of(data: &[u8]) -> u32 {
    let mut h = Crc32::new();
    h.update(data);
    h.finalize()
}

/// Wrap raw digest bytes as the PHP return value: lowercase hex by default, or
/// the bytes verbatim when `$binary` was requested.
fn digest_value(raw: &[u8], binary: bool) -> Value {
    let out = if binary { raw.to_vec() } else { to_hex(raw) };
    Value::Str(Str::from_vec(out))
}

/// Lowercase hex encoding of a byte slice (each byte → two ASCII nibbles),
/// matching PHP's digest formatting and zero-padding.
fn to_hex(raw: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(raw.len() * 2);
    for &b in raw {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 0x0f) as usize]);
    }
    out
}
