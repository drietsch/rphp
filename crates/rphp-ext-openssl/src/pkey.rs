//! Public keys: what `openssl_pkey_get_public()` accepts, what the
//! `OpenSSLAsymmetricKey` handle carries, and the parse every verifying
//! function shares.
//!
//! php takes a key as a PEM string, a `file://` path to one, or an already
//! parsed handle. What it does *not* take here is a private key or a
//! certificate: this wave verifies signatures, so a public key is the whole
//! vocabulary — `openssl_x509_*` arrives with the certificate half.

use const_oid::ObjectIdentifier;
use rphp_value::{Payload, Value};
use spki::{der::Decode, SubjectPublicKeyInfoRef};

use crate::errors;

/// The algorithm OIDs a SubjectPublicKeyInfo can name here.
const RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const EC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const P256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const P384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
const P521: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.35");

/// A parsed public key: the DER of its SubjectPublicKeyInfo, which
/// algorithm it names, and — for the curves — the SEC1 point the info's bit
/// string carries. The DER is kept whole for RSA, which reads its modulus
/// back out of SPKI; the point is kept because the three curve crates agree
/// on `from_sec1_bytes()` and disagree on everything above it.
#[derive(Clone)]
pub struct PublicKey {
    pub der: Vec<u8>,
    pub sec1: Vec<u8>,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Rsa,
    EcP256,
    EcP384,
    EcP521,
}

/// Parse a public key from PEM or bare DER. `None` when the bytes are not a
/// key this build recognises — and then the DECODER error is queued, as
/// OpenSSL's own decoder does before php reports the failure.
pub fn parse(bytes: &[u8]) -> Option<PublicKey> {
    let der = match pem_rfc7468::decode_vec(bytes) {
        Ok((_label, der)) => der,
        Err(_) => bytes.to_vec(),
    };
    let key = SubjectPublicKeyInfoRef::from_der(&der).ok().and_then(|spki| {
        let kind = match spki.algorithm.oid {
            RSA => Some(Kind::Rsa),
            EC => match spki.algorithm.parameters_oid().ok()? {
                P256 => Some(Kind::EcP256),
                P384 => Some(Kind::EcP384),
                P521 => Some(Kind::EcP521),
                _ => None,
            },
            _ => None,
        }?;
        let sec1 = spki.subject_public_key.as_bytes().unwrap_or_default().to_vec();
        Some(PublicKey { der: der.clone(), sec1, kind })
    });
    if key.is_none() {
        errors::push(errors::DECODER_UNSUPPORTED);
    }
    key
}

/// A `file://` argument names a file whose contents are the key; anything
/// else is the key itself. php reads the file with its own stream layer, so
/// a missing file is simply a key that does not parse.
pub fn from_value(value: &Value) -> Option<PublicKey> {
    if let Value::Object(o) = value {
        return o.with_payload::<PublicKey, _>(|k| k.clone());
    }
    let bytes = value.to_php_bytes();
    match bytes.strip_prefix(b"file://") {
        Some(path) => {
            let path = String::from_utf8_lossy(path).into_owned();
            match std::fs::read(path) {
                Ok(contents) => parse(&contents),
                Err(_) => {
                    errors::push(errors::DECODER_UNSUPPORTED);
                    None
                }
            }
        }
        None => parse(&bytes),
    }
}

/// Wrap a parsed key in the `OpenSSLAsymmetricKey` handle php returns.
pub fn handle(ctx: &mut rphp_runtime::Ctx, key: PublicKey) -> Result<Value, rphp_runtime::Unwind> {
    let cid = ctx.lookup_class_or_error(b"OpenSSLAsymmetricKey")?;
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(key)));
    Ok(Value::Object(obj))
}
