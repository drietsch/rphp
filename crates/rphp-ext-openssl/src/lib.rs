//! `ext/openssl` in Rust (ADR-038), wave one: the **verifying** half —
//! `openssl_verify()`, the public-key handle it takes, the digests its
//! `$algorithm` names, and the error queue `openssl_error_string()` hands
//! out. RustCrypto underneath, so the extension links no C library and
//! `#![forbid(unsafe_code)]` holds across the crate.
//!
//! What php decides, this crate decides the same way — measured against
//! php 8.5.10 over OpenSSL 3.6: the three ints `openssl_verify()` answers
//! (`1` verified, `0` not verified, `-1` could not be attempted) and the
//! fourth answer `false` with its warning when the key cannot be read; the
//! DECODER error a refused key queues; MD4 declared and unavailable, as
//! OpenSSL 3's legacy provider leaves it. The signing half, the cipher
//! family and the certificate functions are not here: `register_functions`
//! only registers a signature this crate implements, so `function_exists()`
//! answers honestly for the rest.
#![deny(unsafe_code)]
// OpenSSL is reached through rust-openssl's safe API; the few calls it does
// not wrap go through `openssl-sys` in `sys/`, the one module allowed
// `unsafe` (each call with its own SAFETY note), as ext-posix does.

mod cipher;
mod digest;
mod errors;
mod generated;
mod keys;
mod pkey;
mod shape;
mod sys;
mod verify;
mod x509;

use rphp_runtime::{Ctx, NativeResult, Registry};
use rphp_value::Value;

use crate::digest::Requested;
use crate::shape::{register_functions, FnImpl};

/// Register the extension.
pub fn register(r: &mut Registry) {
    if r.interp().class_by_name(b"OpenSSLAsymmetricKey").is_some() {
        return;
    }
    r.extension("openssl");
    for ini in generated::ini::INI {
        r.interp().ini.register(ini.name, ini.default.unwrap_or(""));
    }
    for c in generated::consts::CONSTANTS {
        let v = shape::const_value(&c.value);
        if c.deprecated {
            r.deprecated_constant(c.name, v, "");
        } else {
            r.constant(c.name, v);
        }
    }
    for class in generated::classes::CLASSES {
        shape::register_handle(r, class);
    }
    for table in TABLES {
        register_functions(r, generated::arginfo::FUNCTIONS, table);
    }
}

/// Every module's functions: the verifying half here, and one table per
/// module of the later waves.
static TABLES: &[&[FnImpl]] = &[FUNCTIONS, cipher::FUNCTIONS, keys::FUNCTIONS, x509::FUNCTIONS];

static FUNCTIONS: &[FnImpl] = &[
    ("openssl_verify", openssl_verify),
    ("openssl_pkey_get_public", openssl_pkey_get_public),
    ("openssl_get_publickey", openssl_pkey_get_public),
    ("openssl_free_key", openssl_free_key),
    ("openssl_error_string", openssl_error_string),
];

/// `openssl_verify(string $data, string $signature, $public_key, string|int $algorithm = OPENSSL_ALGO_SHA1, int $padding = 0): int|false`
fn openssl_verify(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let data = args.first().map(Value::to_php_bytes).unwrap_or_default();
    let signature = args.get(1).map(Value::to_php_bytes).unwrap_or_default();
    let algorithm = args.get(3).cloned().unwrap_or(Value::Int(1));

    let digest = match digest::resolve(&algorithm) {
        Requested::Known(d) => d,
        Requested::Unavailable => {
            // The key is still read first — php reaches the provider only
            // after it has a key — so a bad key still wins the report.
            if pkey::from_value(args.get(2).unwrap_or(&Value::Null)).is_none() {
                return key_refused(ctx);
            }
            digest::queue_unavailable();
            return Ok(Value::Int(-1));
        }
        Requested::Unknown => {
            let who = ctx.active_function_name();
            ctx.warn(&format!("{who}(): Unknown digest algorithm"))?;
            return Ok(Value::Bool(false));
        }
    };

    let Some(key) = pkey::from_value(args.get(2).unwrap_or(&Value::Null)) else {
        return key_refused(ctx);
    };
    Ok(Value::Int(verify::verify(&key, digest, &data, &signature).as_int()))
}

/// php's report for a `$public_key` it could not read: a warning naming the
/// argument's job, and `false`.
fn key_refused(ctx: &mut Ctx) -> NativeResult {
    let who = ctx.active_function_name();
    ctx.warn(&format!("{who}(): Supplied key param cannot be coerced into a public key"))?;
    Ok(Value::Bool(false))
}

/// `openssl_pkey_get_public($public_key): OpenSSLAsymmetricKey|false`
fn openssl_pkey_get_public(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match pkey::from_value(args.first().unwrap_or(&Value::Null)) {
        // Unlike `openssl_verify()`, this one does not warn: it queues the
        // decoder's error and answers false.
        None => Ok(Value::Bool(false)),
        Some(key) => pkey::handle(ctx, key),
    }
}

/// `openssl_free_key($key): void` — deprecated in php 8.0 and a no-op ever
/// since keys became objects; the arginfo's deprecation is what warns.
fn openssl_free_key(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// `openssl_error_string(): string|false`
fn openssl_error_string(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(match errors::take() {
        Some(e) => Value::string(e.as_bytes()),
        None => Value::Bool(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handler bound to a name php does not declare would never be called,
    /// and nothing else would say so: `register_functions` matches by name
    /// and silently skips what it cannot find.
    #[test]
    fn every_handler_names_a_function_php_declares() {
        for (name, _) in TABLES.iter().flat_map(|t| t.iter()) {
            assert!(
                generated::arginfo::FUNCTIONS.iter().any(|s| s.name.eq_ignore_ascii_case(name)),
                "{name} is not in ext/openssl's arginfo"
            );
        }
    }

    /// No function is answered for by two modules.
    #[test]
    fn every_function_has_one_handler() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in TABLES.iter().flat_map(|t| t.iter()) {
            assert!(seen.insert(name.to_ascii_lowercase()), "{name} is bound twice");
        }
        assert_eq!(generated::arginfo::FUNCTIONS.len(), 64);
    }

    /// The P-256 key and signature of `examples/tier-a/openssl/verify.php`,
    /// so the crate's own test fails before the differential does.
    #[test]
    fn verifies_a_p256_signature() {
        const PEM: &str = "-----BEGIN PUBLIC KEY-----\n\
            MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE2UC03zwwGtiR0Z7rE031ohLbExRn\n\
            /rTbkpFIilIBuQZjz1+S6AvJk+hR1W2eeV5XsRtHszXqBK/A6WMd5oHOxw==\n\
            -----END PUBLIC KEY-----\n";
        let key = pkey::parse(PEM.as_bytes()).expect("the pem parses");
        assert_eq!(key.kind, pkey::Kind::EcP256);
    }
}
