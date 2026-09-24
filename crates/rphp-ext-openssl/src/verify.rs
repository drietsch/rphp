//! `openssl_verify()`: does this signature belong to this data under this
//! public key?
//!
//! php's three answers are all int: `1` verified, `0` not verified, `-1`
//! the verification could not be attempted — a signature that is not a
//! signature, a digest the provider will not supply. A key that cannot be
//! read at all is the fourth answer, `false`, and it warns first.
//!
//! RSA is PKCS#1 v1.5 over the DigestInfo of the hash; ECDSA hashes and
//! then verifies the prehash, which is where the hash is truncated to the
//! field size — so a P-256 key answers for SHA-512 as OpenSSL does, by
//! taking the leftmost 256 bits.

use rsa::signature::hazmat::PrehashVerifier;
use rsa::{pkcs1v15::Pkcs1v15Sign, pkcs8::DecodePublicKey, RsaPublicKey};

use crate::digest::Digest;
use crate::pkey::{Kind, PublicKey};

/// The outcome of a verification attempt, in php's vocabulary.
pub enum Verdict {
    Verified,
    NotVerified,
    Failed,
}

impl Verdict {
    pub fn as_int(&self) -> i64 {
        match self {
            Verdict::Verified => 1,
            Verdict::NotVerified => 0,
            Verdict::Failed => -1,
        }
    }
}

pub fn verify(key: &PublicKey, digest: Digest, data: &[u8], signature: &[u8]) -> Verdict {
    let hash = crate::digest::hash(digest, data);
    match key.kind {
        Kind::Rsa => rsa_verify(key, digest, &hash, signature),
        Kind::EcP256 => ec_p256(key, &hash, signature),
        Kind::EcP384 => ec_p384(key, &hash, signature),
        Kind::EcP521 => ec_p521(key, &hash, signature),
    }
}

fn rsa_verify(key: &PublicKey, digest: Digest, hash: &[u8], signature: &[u8]) -> Verdict {
    let Ok(public) = RsaPublicKey::from_public_key_der(&key.der) else {
        return Verdict::Failed;
    };
    // The DigestInfo prefix is the digest's OID, so the scheme is typed by
    // the digest even though the hash is already computed.
    let scheme = match digest {
        Digest::Md5 => Pkcs1v15Sign::new::<md5::Md5>(),
        Digest::Ripemd160 => Pkcs1v15Sign::new::<ripemd::Ripemd160>(),
        Digest::Sha1 => Pkcs1v15Sign::new::<sha1::Sha1>(),
        Digest::Sha224 => Pkcs1v15Sign::new::<sha2::Sha224>(),
        Digest::Sha256 => Pkcs1v15Sign::new::<sha2::Sha256>(),
        Digest::Sha384 => Pkcs1v15Sign::new::<sha2::Sha384>(),
        Digest::Sha512 => Pkcs1v15Sign::new::<sha2::Sha512>(),
        Digest::Sha3_224 => Pkcs1v15Sign::new::<sha3::Sha3_224>(),
        Digest::Sha3_256 => Pkcs1v15Sign::new::<sha3::Sha3_256>(),
        Digest::Sha3_384 => Pkcs1v15Sign::new::<sha3::Sha3_384>(),
        Digest::Sha3_512 => Pkcs1v15Sign::new::<sha3::Sha3_512>(),
    };
    match public.verify(scheme, hash, signature) {
        Ok(()) => Verdict::Verified,
        Err(_) => Verdict::NotVerified,
    }
}

/// One curve's verification. Written per curve rather than generically:
/// the ECDSA type parameters carry half a dozen bounds each, and three
/// named curves are shorter than the where-clause that would unify them.
macro_rules! ec_verify {
    ($curve:ident, $key:expr, $hash:expr, $signature:expr) => {{
        use $curve::ecdsa::{Signature, VerifyingKey};
        // From the SEC1 point rather than from SPKI: the three curve crates
        // agree on this constructor and not on `DecodePublicKey`.
        let Ok(verifying) = VerifyingKey::from_sec1_bytes(&$key.sec1) else {
            return Verdict::Failed;
        };
        // OpenSSL takes the DER ECDSA-Sig-Value; a signature that does not
        // parse is php's -1, not a failed verification.
        let Ok(sig) = Signature::from_der($signature) else {
            return Verdict::Failed;
        };
        match verifying.verify_prehash($hash, &sig) {
            Ok(()) => Verdict::Verified,
            Err(_) => Verdict::NotVerified,
        }
    }};
}

fn ec_p256(key: &PublicKey, hash: &[u8], signature: &[u8]) -> Verdict {
    ec_verify!(p256, key, hash, signature)
}

fn ec_p384(key: &PublicKey, hash: &[u8], signature: &[u8]) -> Verdict {
    ec_verify!(p384, key, hash, signature)
}

fn ec_p521(key: &PublicKey, hash: &[u8], signature: &[u8]) -> Verdict {
    ec_verify!(p521, key, hash, signature)
}
