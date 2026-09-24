//! The digests `openssl_verify()`'s `$algorithm` names, as php's
//! `OPENSSL_ALGO_*` constants and as OpenSSL's digest names.
//!
//! The seven `OPENSSL_ALGO_*` digests, plus the SHA-3 family, which has no
//! php constant and is reachable only by name (`"sha3-256"`). A name
//! OpenSSL knows and this crate does not — BLAKE2, SM3, the SHAKEs — is
//! reported as unknown, which is the one place this function is narrower
//! than php's (ADR-038).
//!
//! MD4 is declared by php and refused by OpenSSL 3, which moved it to the
//! legacy provider: `openssl_verify()` answers `-1` and queues two errors
//! rather than warning. This crate answers the same way, because "the
//! digest exists but this build will not use it" is exactly the situation.

use crate::errors;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Digest {
    Md5,
    Ripemd160,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
}

/// php's `OPENSSL_ALGO_*` values (`ext/openssl/php_openssl.h`).
const ALGO_SHA1: i64 = 1;
const ALGO_MD5: i64 = 2;
const ALGO_MD4: i64 = 3;
const ALGO_SHA224: i64 = 6;
const ALGO_SHA256: i64 = 7;
const ALGO_SHA384: i64 = 8;
const ALGO_SHA512: i64 = 9;
const ALGO_RMD160: i64 = 10;

/// What resolving an `$algorithm` argument can produce.
pub enum Requested {
    /// A digest this build computes.
    Known(Digest),
    /// A digest php declares that OpenSSL 3 will not provide (MD4): the
    /// call fails with `-1`, having queued the provider's refusal.
    Unavailable,
    /// Not a digest at all: php warns and answers `false`.
    Unknown,
}

/// Resolve `$algorithm`, which php takes as one of its constants or as a
/// digest name (`"sha256"`, `"SHA256"`, `"RSA-SHA256"` — OpenSSL's aliases).
pub fn resolve(value: &rphp_value::Value) -> Requested {
    if let rphp_value::Value::Int(id) = value {
        return match *id {
            ALGO_MD5 => Requested::Known(Digest::Md5),
            ALGO_RMD160 => Requested::Known(Digest::Ripemd160),
            ALGO_SHA1 => Requested::Known(Digest::Sha1),
            ALGO_SHA224 => Requested::Known(Digest::Sha224),
            ALGO_SHA256 => Requested::Known(Digest::Sha256),
            ALGO_SHA384 => Requested::Known(Digest::Sha384),
            ALGO_SHA512 => Requested::Known(Digest::Sha512),
            ALGO_MD4 => Requested::Unavailable,
            _ => Requested::Unknown,
        };
    }
    let name = value.to_php_bytes();
    let name = String::from_utf8_lossy(&name).to_ascii_lowercase();
    let name = name.strip_prefix("rsa-").unwrap_or(&name).replace('-', "");
    match name.as_str() {
        "md5" => Requested::Known(Digest::Md5),
        "ripemd160" | "rmd160" | "ripemd" => Requested::Known(Digest::Ripemd160),
        "sha1" => Requested::Known(Digest::Sha1),
        "sha224" => Requested::Known(Digest::Sha224),
        "sha256" => Requested::Known(Digest::Sha256),
        "sha384" => Requested::Known(Digest::Sha384),
        "sha512" => Requested::Known(Digest::Sha512),
        "sha3224" => Requested::Known(Digest::Sha3_224),
        "sha3256" => Requested::Known(Digest::Sha3_256),
        "sha3384" => Requested::Known(Digest::Sha3_384),
        "sha3512" => Requested::Known(Digest::Sha3_512),
        "md4" => Requested::Unavailable,
        _ => Requested::Unknown,
    }
}

/// The two errors OpenSSL 3 queues when a digest is declared but its
/// provider is not loaded.
pub fn queue_unavailable() {
    errors::push("error:0308010C:digital envelope routines::unsupported");
    errors::push("error:1C80007A:Provider routines::invalid digest");
}

/// The message digest of `data`.
pub fn hash(digest: Digest, data: &[u8]) -> Vec<u8> {
    use ::sha2::Digest as _;
    match digest {
        Digest::Md5 => md5::Md5::digest(data).to_vec(),
        Digest::Ripemd160 => ripemd::Ripemd160::digest(data).to_vec(),
        Digest::Sha1 => sha1::Sha1::digest(data).to_vec(),
        Digest::Sha224 => sha2::Sha224::digest(data).to_vec(),
        Digest::Sha256 => sha2::Sha256::digest(data).to_vec(),
        Digest::Sha384 => sha2::Sha384::digest(data).to_vec(),
        Digest::Sha512 => sha2::Sha512::digest(data).to_vec(),
        Digest::Sha3_224 => sha3::Sha3_224::digest(data).to_vec(),
        Digest::Sha3_256 => sha3::Sha3_256::digest(data).to_vec(),
        Digest::Sha3_384 => sha3::Sha3_384::digest(data).to_vec(),
        Digest::Sha3_512 => sha3::Sha3_512::digest(data).to_vec(),
    }
}
