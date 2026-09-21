//! `ext/standard`'s password and `crypt(3)` surface — `password_hash`,
//! `password_verify`, `password_needs_rehash`, `password_get_info`,
//! `password_algos`, and `crypt` itself.
//!
//! # The two layers
//!
//! php stacks the password API on top of `crypt(3)`. `crypt()` is the byte
//! layer: a *setting* string selects a scheme, carries its salt and cost, and
//! the result is that same setting with a checksum appended. The password API
//! is a thin, opinionated wrapper that only ever *writes* three of those
//! schemes (`$2y$` bcrypt, `$argon2i$`, `$argon2id$`) but happily *reads* any
//! of them, because `password_verify` falls back to a plain
//! `crypt($password, $hash) === $hash` comparison for everything it does not
//! recognize. That fallback is why a `$1$` or `$6$` or DES hash verifies here
//! exactly as it does in php.
//!
//! # Which crate computes what
//!
//! The traditional unix schemes come from `pwhash`, whose implementations were
//! checked against stock php 8.5 vector by vector (bcrypt, extended DES and
//! traditional DES all reproduce php's output byte for byte):
//!
//! | setting | scheme | backend |
//! |---|---|---|
//! | `$2a$` `$2b$` `$2x$` `$2y$` | bcrypt (Blowfish) | `pwhash::bcrypt` |
//! | `$1$` | MD5-crypt | `pwhash::md5_crypt` |
//! | `$5$` / `$6$` | SHA-256 / SHA-512 crypt | `pwhash::sha256_crypt` / `sha512_crypt` |
//! | `_…` | extended (BSDi) DES | `pwhash::bsdi_crypt` |
//! | two salt bytes | traditional DES | `pwhash::unix_crypt` |
//!
//! Argon2 comes from RustCrypto's `argon2` through the PHC string format, which
//! is the same `$argon2id$v=19$m=…,t=…,p=…$salt$tag` layout libargon2 emits, so
//! php's `password_verify` accepts what is produced here and the reverse.
//!
//! `pwhash`'s per-scheme parsers are deliberately bypassed: this module parses
//! and validates the setting the way php does and hands the backend an already
//! decided `HashSetup`. That matters because the two disagree in places — php
//! *rejects* a `$5$`/`$6$` `rounds=` outside `1000..=999999999` with `*0` where
//! `pwhash` would silently clamp it, and php's `rounds=` follows `strtoul`, so
//! `rounds=abc$` is not a round count at all but a literal ten-byte salt.
//!
//! # php's failure convention
//!
//! A setting php cannot use does not raise: `crypt()` returns the two-byte
//! token `*0`, or `*1` when the setting itself began with `*0`, so that the
//! result can never equal the input and a naive `crypt($p, $h) === $h` check
//! cannot be tricked into passing. Everything below funnels through
//! [`crypt_failure`] for that.
//!
//! # Known divergences (ADR-004)
//!
//! * **`$2a$` and `$2x$` for certain 8-bit passwords.** php links
//!   crypt_blowfish, which keeps two deliberate deviations from the corrected
//!   algorithm: `$2x$` reproduces the original sign-extension bug, and `$2a$`
//!   carries an anti-collision safety measure that fires when the correct and
//!   the buggy key schedules coincide *and* a sign extension occurred. `pwhash`
//!   has a single, corrected implementation (the `$2b$`/`$2y$` one). Rather
//!   than return a wrong hash, [`bcrypt_setting`] computes crypt_blowfish's own
//!   `diff`/`sign` accumulators and returns the failure token in exactly the
//!   inputs where the two would disagree — a visible gap instead of a silent
//!   one. Every other password, including all 7-bit ones, is byte-exact.
//! * **Non-ASCII bytes in a `$1$`/`$5$`/`$6$` salt.** php is byte-oriented
//!   there; the `pwhash` entry points take `&str`. Such a setting yields `*0`.
//! * **The `$algo` float deprecation.** php's parameter coercion emits
//!   `Implicit conversion from float 2.5 to int loses precision` before this
//!   module ever runs; that belongs to the engine's argument binding, not here.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use pwhash::bcrypt::{BcryptSetup, BcryptVariant};
use pwhash::{bcrypt, bsdi_crypt, md5_crypt, sha256_crypt, sha512_crypt, unix_crypt, HashSetup};

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("crypt", 2, Some(2), crypt),
    nf!("password_hash", 2, Some(3), password_hash),
    nf!("password_verify", 2, Some(2), password_verify),
    nf!("password_needs_rehash", 2, Some(3), password_needs_rehash),
    nf!("password_get_info", 1, Some(1), password_get_info),
    nf!("password_algos", 0, Some(0), password_algos),
];

/// The `PASSWORD_*` algorithm names and tuning defaults, plus the `CRYPT_*`
/// capability flags `crypt()` advertises.
///
/// php 8.0 turned the three algorithm identifiers from ints into the strings
/// that appear in the hash itself, so `PASSWORD_BCRYPT` is `"2y"`, not `1`;
/// the tuning constants stayed ints. `PASSWORD_BCRYPT_DEFAULT_COST` is 12 as
/// of php 8.4 — it was 10 for the decade before that.
pub(crate) fn register_constants(r: &mut Registry) {
    r.constant("PASSWORD_DEFAULT", Value::string(BCRYPT_NAME));
    r.constant("PASSWORD_BCRYPT", Value::string(BCRYPT_NAME));
    r.constant("PASSWORD_ARGON2I", Value::string(ARGON2I_NAME));
    r.constant("PASSWORD_ARGON2ID", Value::string(ARGON2ID_NAME));
    r.constant("PASSWORD_BCRYPT_DEFAULT_COST", Value::Int(BCRYPT_DEFAULT_COST));
    r.constant("PASSWORD_ARGON2_DEFAULT_MEMORY_COST", Value::Int(ARGON2_DEFAULT_MEMORY_COST));
    r.constant("PASSWORD_ARGON2_DEFAULT_TIME_COST", Value::Int(ARGON2_DEFAULT_TIME_COST));
    r.constant("PASSWORD_ARGON2_DEFAULT_THREADS", Value::Int(ARGON2_DEFAULT_THREADS));
    // libargon2 rather than a distribution's own build; php reports "standard".
    r.constant("PASSWORD_ARGON2_PROVIDER", Value::string(b"standard"));

    // The `CRYPT_*` capability flags belong to this extension too, but
    // `string2.rs` already registers all seven with the values php reports
    // (`CRYPT_SALT_LENGTH` 123, every scheme flag 1), so they are not repeated
    // here.
}

const BCRYPT_NAME: &[u8] = b"2y";
const ARGON2I_NAME: &[u8] = b"argon2i";
const ARGON2ID_NAME: &[u8] = b"argon2id";
/// bcrypt is the one algorithm whose `algoName` differs from its `algo`.
const BCRYPT_DISPLAY_NAME: &[u8] = b"bcrypt";
/// What `password_get_info` reports for a hash it does not recognize.
const UNKNOWN_NAME: &[u8] = b"unknown";

const BCRYPT_DEFAULT_COST: i64 = 12;
const ARGON2_DEFAULT_MEMORY_COST: i64 = 65536;
const ARGON2_DEFAULT_TIME_COST: i64 = 4;
const ARGON2_DEFAULT_THREADS: i64 = 1;

/// The tag length libargon2 (and therefore php) writes, in bytes.
const ARGON2_TAG_LEN: usize = 32;
/// The salt length php draws for argon2, in bytes — 22 characters once
/// base64-encoded.
const ARGON2_SALT_LEN: usize = 16;

/// The alphabet the DES, MD5 and SHA-crypt schemes encode with.
const CRYPT64: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// bcrypt's alphabet, which orders the same 64 characters differently.
const BCRYPT64: &[u8] = b"./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

// ---- crypt() ----------------------------------------------------------------

/// `crypt(string $string, string $salt): string`.
pub(crate) fn crypt(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let password = args[0].to_php_bytes();
    let salt = args[1].to_php_bytes();
    Ok(Value::Str(Str::from_vec(crypt_bytes(&password, &salt))))
}

/// The whole scheme table, dispatched on the setting's prefix.
///
/// php hands both arguments to `crypt_r` as C strings, so a NUL byte ends them
/// — `crypt("a\0b", $s)` is `crypt("a", $s)`, and a salt of `$1$ab\0cd$` salts
/// with `ab`. Cutting both here is what reproduces that.
fn crypt_bytes(password: &[u8], salt: &[u8]) -> Vec<u8> {
    let password = nul_cut(password);
    let setting = nul_cut(salt);

    let computed = match setting {
        // The variant letter is its own byte, so `$2$10$…` (no letter) falls
        // through to the DES arm and fails there, as it does in php.
        [b'$', b'2', variant, b'$', rest @ ..] => bcrypt_setting(*variant, rest, password),
        [b'$', b'1', b'$', rest @ ..] => md5_setting(rest, password),
        [b'$', b'5', b'$', rest @ ..] => sha2_setting(rest, password, false),
        [b'$', b'6', b'$', rest @ ..] => sha2_setting(rest, password, true),
        [b'_', ..] => bsdi_setting(setting, password),
        _ => des_setting(setting, password),
    };
    computed.unwrap_or_else(|| crypt_failure(salt))
}

/// php's crypt(3) failure token. It is normally `*0`, but becomes `*1` when the
/// rejected setting itself started with `*0`, so the return value can never be
/// equal to the setting that produced it.
fn crypt_failure(salt: &[u8]) -> Vec<u8> {
    if salt.starts_with(b"*0") {
        b"*1".to_vec()
    } else {
        b"*0".to_vec()
    }
}

/// `$2a$` / `$2b$` / `$2x$` / `$2y$`: a two-digit cost, then 22 characters of
/// bcrypt-base64 salt.
///
/// The salt is decoded to 16 bytes and re-encoded on the way out, which is why
/// php answers a setting ending `…forsalt` with a hash ending `…fore`: the 22nd
/// character only carries two significant bits.
fn bcrypt_setting(variant: u8, rest: &[u8], password: &[u8]) -> Option<Vec<u8>> {
    if !matches!(variant, b'a' | b'b' | b'x' | b'y') {
        return None;
    }
    // The cost is exactly two decimal digits; php rejects both `$2y$4$` and
    // `$2y$004$`.
    let [hi, lo, b'$', tail @ ..] = rest else {
        return None;
    };
    if !hi.is_ascii_digit() || !lo.is_ascii_digit() {
        return None;
    }
    let cost = (*hi - b'0') as u32 * 10 + (*lo - b'0') as u32;
    if !(4..=31).contains(&cost) {
        return None;
    }
    if tail.len() < 22 {
        return None;
    }
    let salt = ascii_str(&tail[..22]).filter(|s| in_alphabet(s.as_bytes(), BCRYPT64))?;

    // See the module header: these are the inputs on which crypt_blowfish's
    // `$2a$` safety measure or `$2x$` bug would change the answer, and which
    // the corrected implementation behind this call therefore cannot produce.
    let (diff, sign) = bf_key_flags(password);
    match variant {
        b'a' if diff == 0 && sign != 0 => return None,
        b'x' if diff != 0 => return None,
        _ => {}
    }

    let setup = BcryptSetup {
        salt: Some(salt),
        cost: Some(cost),
        variant: Some(BcryptVariant::V2b),
    };
    let mut out = bcrypt::hash_with(setup, password).ok()?.into_bytes();
    // All four variants share one computation here, so only the tag differs.
    out[2] = variant;
    Some(out)
}

/// crypt_blowfish's `BF_set_key` bookkeeping, reproduced for its two flag words
/// alone.
///
/// The routine builds the 18-word Blowfish key twice — once reading each
/// password byte as unsigned (correct) and once as signed (the historical bug)
/// — and remembers whether the two ever differed (`diff`) and whether a sign
/// extension reached a word (`sign`). `$2x$` uses the buggy schedule, so it
/// differs from the corrected one exactly when `diff` is non-zero; `$2a$`
/// perturbs the first word when `diff` is zero but `sign` is not. The key is
/// the NUL-terminated password cycled to fill 72 bytes.
fn bf_key_flags(password: &[u8]) -> (u32, u32) {
    let (mut diff, mut sign) = (0u32, 0u32);
    let mut pos = 0usize;
    for _ in 0..18 {
        let (mut correct, mut buggy) = (0u32, 0u32);
        for j in 0..4 {
            let byte = if pos < password.len() { password[pos] } else { 0 };
            correct = (correct << 8) | byte as u32;
            buggy = (buggy << 8) | (byte as i8 as i32 as u32);
            if j != 0 {
                sign |= buggy & 0x8000_0000;
            }
            if byte == 0 {
                pos = 0;
            } else {
                pos += 1;
            }
        }
        diff |= correct ^ buggy;
    }
    (diff, sign)
}

/// `$1$`: MD5-crypt. The salt runs to the next `$` and is cut at 8 characters.
#[allow(deprecated)]
fn md5_setting(rest: &[u8], password: &[u8]) -> Option<Vec<u8>> {
    let salt = rest.split(|&c| c == b'$').next()?;
    let salt = ascii_str(&salt[..salt.len().min(8)])?;
    let setup = HashSetup {
        salt: Some(salt),
        rounds: None,
    };
    md5_crypt::hash_with(setup, password)
        .ok()
        .map(String::into_bytes)
}

/// `$5$` / `$6$`: SHA-256 and SHA-512 crypt, with the optional `rounds=N$`
/// prefix and a salt cut at 16 characters.
#[allow(deprecated)]
fn sha2_setting(rest: &[u8], password: &[u8], sha512: bool) -> Option<Vec<u8>> {
    let mut body = rest;
    let mut rounds = None;

    if let Some(after) = body.strip_prefix(b"rounds=") {
        let (value, tail) = strtoul(after);
        // php consumes the prefix only when the digits are followed by `$` —
        // which is why `rounds=abc$` and `rounds=12x$` are salts, not counts.
        if tail.first() == Some(&b'$') {
            // php rejects an out-of-range count outright instead of clamping.
            if !(1000..=999_999_999).contains(&value) {
                return None;
            }
            rounds = Some(value as u32);
            body = &tail[1..];
        }
    }

    let salt = body.split(|&c| c == b'$').next()?;
    let salt = ascii_str(&salt[..salt.len().min(16)])?;
    let setup = HashSetup {
        salt: Some(salt),
        rounds,
    };
    let hashed = if sha512 {
        sha512_crypt::hash_with(setup, password)
    } else {
        sha256_crypt::hash_with(setup, password)
    };
    hashed.ok().map(String::into_bytes)
}

/// `_…`: extended (BSDi) DES — four characters of iteration count, four of
/// salt. A zero count is rejected, which is what makes php answer `_....abcd`
/// with `*0`.
#[allow(deprecated)]
fn bsdi_setting(setting: &[u8], password: &[u8]) -> Option<Vec<u8>> {
    if setting.len() < 9 || !in_alphabet(&setting[1..9], CRYPT64) {
        return None;
    }
    let parsed = ascii_str(&setting[..9])?;
    bsdi_crypt::hash_with(parsed, password)
        .ok()
        .map(String::into_bytes)
}

/// Traditional DES: the first two characters of the setting are the salt.
#[allow(deprecated)]
fn des_setting(setting: &[u8], password: &[u8]) -> Option<Vec<u8>> {
    if setting.len() < 2 || !in_alphabet(&setting[..2], CRYPT64) {
        return None;
    }
    let salt = ascii_str(&setting[..2])?;
    unix_crypt::hash_with(salt, password)
        .ok()
        .map(String::into_bytes)
}

// ---- password_hash() --------------------------------------------------------

/// The three algorithms php will write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Algo {
    Bcrypt,
    Argon2i,
    Argon2id,
}

impl Algo {
    /// The `algo` value `password_get_info` reports, which is also the
    /// `PASSWORD_*` constant's value.
    fn name(self) -> &'static [u8] {
        match self {
            Algo::Bcrypt => BCRYPT_NAME,
            Algo::Argon2i => ARGON2I_NAME,
            Algo::Argon2id => ARGON2ID_NAME,
        }
    }

    /// The `algoName` value, which differs from `algo` only for bcrypt.
    fn display_name(self) -> &'static [u8] {
        match self {
            Algo::Bcrypt => BCRYPT_DISPLAY_NAME,
            Algo::Argon2i => ARGON2I_NAME,
            Algo::Argon2id => ARGON2ID_NAME,
        }
    }
}

/// `password_hash(string $password, string|int|null $algo, array $options = []): string`.
pub(crate) fn password_hash(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let password = args[0].to_php_bytes();
    // Argument binding runs before the body in php, so a non-array `$options`
    // is reported even when `$algo` is also wrong.
    let options = options_arg(args.get(2), "password_hash")?;
    let Some(algo) = resolve_algo(&args[1]) else {
        return Err(Unwind::value_error(
            "password_hash(): Argument #2 ($algo) must be a valid password hashing algorithm",
        ));
    };

    // A caller-supplied salt has been ignored since php 7.0 and dropped
    // outright in 8.0; php still warns rather than failing.
    if options
        .as_ref()
        .is_some_and(|o| o.contains_key(&ArrayKey::str(b"salt")))
    {
        ctx.warn(
            "password_hash(): The \"salt\" option has been ignored, \
             since providing a custom salt is no longer supported",
        )?;
    }

    let hashed = match algo {
        Algo::Bcrypt => bcrypt_hash(&password, options.as_ref())?,
        Algo::Argon2i => argon2_hash(Algorithm::Argon2i, &password, options.as_ref())?,
        Algo::Argon2id => argon2_hash(Algorithm::Argon2id, &password, options.as_ref())?,
    };
    Ok(Value::Str(Str::from_vec(hashed)))
}

/// Resolve `$algo`, which php accepts as the modern name string, as one of the
/// legacy ints still mapped for compatibility (`0` meaning the default), or as
/// `null` — also the default. Anything else is rejected by the caller.
fn resolve_algo(value: &Value) -> Option<Algo> {
    match &*value.deref() {
        Value::Null | Value::Uninit => Some(Algo::Bcrypt),
        // The names are matched exactly: php rejects "ARGON2I" and "bcrypt".
        Value::Str(s) => {
            let name = s.as_bytes();
            if name == BCRYPT_NAME {
                Some(Algo::Bcrypt)
            } else if name == ARGON2I_NAME {
                Some(Algo::Argon2i)
            } else if name == ARGON2ID_NAME {
                Some(Algo::Argon2id)
            } else {
                None
            }
        }
        // The pre-8.0 spellings: 0 is `PASSWORD_DEFAULT`, 1 bcrypt, 2/3 argon2.
        other => match other.to_int() {
            0 | 1 => Some(Algo::Bcrypt),
            2 => Some(Algo::Argon2i),
            3 => Some(Algo::Argon2id),
            _ => None,
        },
    }
}

/// Read the optional `$options` array, which php type-checks even when it is
/// only going to ignore the contents.
fn options_arg(value: Option<&Value>, func: &str) -> Result<Option<Array>, Unwind> {
    let Some(value) = value else {
        return Ok(None);
    };
    match &*value.deref() {
        Value::Array(a) => Ok(Some(a.clone())),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #3 ($options) must be of type array, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// An `$options` entry as an int, using php's ordinary scalar cast — `'4'` and
/// `4.0` are both a cost of 4, and `'abc'` is 0.
fn option_int(options: Option<&Array>, key: &[u8], default: i64) -> i64 {
    options
        .and_then(|o| o.get_deref(&ArrayKey::str(key)))
        .map_or(default, |v| v.to_int())
}

/// `password_hash`'s bcrypt branch: a fresh 16-byte salt at the requested cost,
/// always tagged `$2y$`.
fn bcrypt_hash(password: &[u8], options: Option<&Array>) -> Result<Vec<u8>, Unwind> {
    // php checks the password before the cost, so a NUL byte wins over a cost
    // that is also out of range.
    if password.contains(&0) {
        return Err(Unwind::value_error(
            "Bcrypt password must not contain null character",
        ));
    }
    let cost = option_int(options, b"cost", BCRYPT_DEFAULT_COST);
    if !(4..=31).contains(&cost) {
        return Err(Unwind::value_error(format!(
            "Invalid bcrypt cost parameter specified: {cost}"
        )));
    }
    // A password over 72 bytes is silently truncated, exactly as `crypt()`
    // truncates it — php 8.5 raises nothing for it.
    let setup = BcryptSetup {
        salt: None,
        cost: Some(cost as u32),
        variant: Some(BcryptVariant::V2y),
    };
    bcrypt::hash_with(setup, password)
        .map(String::into_bytes)
        .map_err(|_| Unwind::error("Unable to generate salt"))
}

/// `password_hash`'s argon2 branch. The four `ValueError`s and the order they
/// are checked in are libargon2's, as php surfaces them.
fn argon2_hash(
    algorithm: Algorithm,
    password: &[u8],
    options: Option<&Array>,
) -> Result<Vec<u8>, Unwind> {
    let memory_cost = option_int(options, b"memory_cost", ARGON2_DEFAULT_MEMORY_COST);
    let time_cost = option_int(options, b"time_cost", ARGON2_DEFAULT_TIME_COST);
    let threads = option_int(options, b"threads", ARGON2_DEFAULT_THREADS);

    if !(8..=u32::MAX as i64).contains(&memory_cost) {
        return Err(Unwind::value_error(
            "Memory cost is outside of allowed memory range",
        ));
    }
    if !(1..=u32::MAX as i64).contains(&time_cost) {
        return Err(Unwind::value_error(
            "Time cost is outside of allowed time range",
        ));
    }
    if !(1..=0xFF_FFFF).contains(&threads) {
        return Err(Unwind::value_error("Invalid number of threads"));
    }
    // Each lane needs two slices of four blocks, so the memory has to cover the
    // thread count — a separate message from the range check above.
    if memory_cost < threads * 8 {
        return Err(Unwind::value_error("Memory cost is too small"));
    }

    let params = Params::new(
        memory_cost as u32,
        time_cost as u32,
        threads as u32,
        Some(ARGON2_TAG_LEN),
    )
    .map_err(|_| Unwind::value_error("Memory cost is outside of allowed memory range"))?;

    let mut salt = [0u8; ARGON2_SALT_LEN];
    os_random(&mut salt)?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| Unwind::error("Unable to generate salt"))?;

    let hasher = Argon2::new(algorithm, Version::V0x13, params);
    let hashed = hasher
        .hash_password(password, &salt)
        .map_err(|_| Unwind::error("Unable to generate salt"))?;
    Ok(hashed.to_string().into_bytes())
}

// ---- password_verify() / needs_rehash() / get_info() / algos() ---------------

/// `password_verify(string $password, string $hash): bool`.
///
/// Only the two argon2 prefixes get a dedicated verifier. Everything else — a
/// real `$2y$` hash, but equally a `$2a$`, `$1$`, `$6$` or bare DES one — goes
/// through php's default path, which simply re-runs `crypt()` with the stored
/// hash as the setting and compares. Comparing in constant time is what stops
/// the check leaking how long a guessed prefix matched.
pub(crate) fn password_verify(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let password = args[0].to_php_bytes();
    let hash = args[1].to_php_bytes();

    let verified = match identify(&hash) {
        Some(Algo::Argon2i) | Some(Algo::Argon2id) => argon2_verify(&password, &hash),
        // `None` lands here too: bcrypt is php's fallback algorithm, and its
        // verifier is the generic crypt comparison.
        _ => {
            let computed = crypt_bytes(&password, &hash);
            constant_time_eq(&computed, &hash)
        }
    };
    Ok(Value::Bool(verified))
}

/// Re-derive an argon2 hash from its own recorded parameters and compare. The
/// stored string carries the variant, version, `m`/`t`/`p` and tag length, so
/// the verifier needs no configuration of its own.
fn argon2_verify(password: &[u8], hash: &[u8]) -> bool {
    let Some(text) = ascii_str(hash) else {
        return false;
    };
    let Ok(parsed) = PasswordHash::new(text) else {
        return false;
    };
    Argon2::default().verify_password(password, &parsed).is_ok()
}

/// `password_needs_rehash(string $hash, string|int|null $algo, array $options = []): bool`.
///
/// php never validates `$options` here, so a cost of 99 reports "yes, rehash"
/// instead of raising; and an algorithm it does not know reports `false`,
/// on the grounds that it cannot suggest rehashing to something unavailable.
pub(crate) fn password_needs_rehash(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let hash = args[0].to_php_bytes();
    let options = options_arg(args.get(2), "password_needs_rehash")?;

    let Some(wanted) = resolve_algo(&args[1]) else {
        return Ok(Value::Bool(false));
    };
    // Unlike `password_verify`, this identification does not fall back to
    // bcrypt: an unrecognized hash is a different algorithm, so it needs one.
    if identify(&hash) != Some(wanted) {
        return Ok(Value::Bool(true));
    }

    let stale = match wanted {
        Algo::Bcrypt => bcrypt_cost(&hash) != option_int(options.as_ref(), b"cost", BCRYPT_DEFAULT_COST),
        Algo::Argon2i | Algo::Argon2id => {
            let (memory_cost, time_cost, threads) = argon2_params(&hash);
            memory_cost != option_int(options.as_ref(), b"memory_cost", ARGON2_DEFAULT_MEMORY_COST)
                || time_cost != option_int(options.as_ref(), b"time_cost", ARGON2_DEFAULT_TIME_COST)
                || threads != option_int(options.as_ref(), b"threads", ARGON2_DEFAULT_THREADS)
        }
    };
    Ok(Value::Bool(stale))
}

/// `password_get_info(string $hash): array`.
///
/// The three keys are always present and always in this order; an unrecognized
/// hash reports a null `algo`, the name `"unknown"` and an empty options array
/// rather than failing.
pub(crate) fn password_get_info(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let hash = args[0].to_php_bytes();
    let mut info = Array::new();
    let mut options = Array::new();

    match identify(&hash) {
        Some(algo @ Algo::Bcrypt) => {
            info.set(ArrayKey::str(b"algo"), Value::string(algo.name()));
            info.set(ArrayKey::str(b"algoName"), Value::string(algo.display_name()));
            options.set(ArrayKey::str(b"cost"), Value::Int(bcrypt_cost(&hash)));
        }
        Some(algo) => {
            let (memory_cost, time_cost, threads) = argon2_params(&hash);
            info.set(ArrayKey::str(b"algo"), Value::string(algo.name()));
            info.set(ArrayKey::str(b"algoName"), Value::string(algo.display_name()));
            options.set(ArrayKey::str(b"memory_cost"), Value::Int(memory_cost));
            options.set(ArrayKey::str(b"time_cost"), Value::Int(time_cost));
            options.set(ArrayKey::str(b"threads"), Value::Int(threads));
        }
        None => {
            info.set(ArrayKey::str(b"algo"), Value::Null);
            info.set(ArrayKey::str(b"algoName"), Value::string(UNKNOWN_NAME));
        }
    }
    info.set(ArrayKey::str(b"options"), Value::Array(options));
    Ok(Value::Array(info))
}

/// `password_algos(): array` — the algorithms this build can write, in
/// registration order.
pub(crate) fn password_algos(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    out.push(Value::string(BCRYPT_NAME));
    out.push(Value::string(ARGON2I_NAME));
    out.push(Value::string(ARGON2ID_NAME));
    Ok(Value::Array(out))
}

/// Which of the three algorithms wrote this hash, by prefix.
///
/// The bcrypt test is deliberately narrow — length exactly 60 and a leading
/// `$2y` — so a `$2a$`, `$2b$` or `$2x$` hash reads as unknown even though
/// `crypt()` and `password_verify` handle it happily.
fn identify(hash: &[u8]) -> Option<Algo> {
    if hash.starts_with(b"$argon2id$") {
        Some(Algo::Argon2id)
    } else if hash.starts_with(b"$argon2i$") {
        Some(Algo::Argon2i)
    } else if hash.len() == 60 && hash.starts_with(b"$2y") {
        Some(Algo::Bcrypt)
    } else {
        None
    }
}

/// The cost recorded in a bcrypt hash. php reads it with `sscanf`, so the
/// digits are taken as far as they run and the default stands in when there are
/// none — `$2y$4x$…` is a cost of 4 and `$2y$xx$…` the default 12.
fn bcrypt_cost(hash: &[u8]) -> i64 {
    let after_tag = &hash[hash.len().min(4)..];
    match strtol(after_tag) {
        (value, _, true) => value,
        (_, _, false) => BCRYPT_DEFAULT_COST,
    }
}

/// The `m`/`t`/`p` recorded in an argon2 hash. php scans them with one
/// `sscanf` and keeps its defaults for whatever the pattern failed to reach,
/// which is why a bare `$argon2i$` reports 65536/4/1.
fn argon2_params(hash: &[u8]) -> (i64, i64, i64) {
    let mut memory_cost = ARGON2_DEFAULT_MEMORY_COST;
    let mut time_cost = ARGON2_DEFAULT_TIME_COST;
    let mut threads = ARGON2_DEFAULT_THREADS;

    // The scan is positional: `m` must be found before `t` counts, and so on.
    if let Some(rest) = find_after(hash, b"$m=") {
        let (value, rest, _) = strtol(rest);
        memory_cost = value;
        if let Some(rest) = rest.strip_prefix(b",t=") {
            let (value, rest, _) = strtol(rest);
            time_cost = value;
            if let Some(rest) = rest.strip_prefix(b",p=") {
                threads = strtol(rest).0;
            }
        }
    }
    (memory_cost, time_cost, threads)
}

// ---- shared helpers ---------------------------------------------------------

/// Everything up to the first NUL byte, the way a C string is read.
fn nul_cut(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&b| b == 0) {
        Some(i) => &bytes[..i],
        None => bytes,
    }
}

/// Borrow the bytes as `&str` when every one of them is ASCII, so the `pwhash`
/// entry points can be handed a slice without a lossy conversion.
fn ascii_str(bytes: &[u8]) -> Option<&str> {
    if bytes.is_ascii() {
        std::str::from_utf8(bytes).ok()
    } else {
        None
    }
}

/// Whether every byte is drawn from `alphabet`.
fn in_alphabet(bytes: &[u8], alphabet: &[u8]) -> bool {
    bytes.iter().all(|b| alphabet.contains(b))
}

/// The remainder after the first occurrence of `needle`.
fn find_after<'a>(bytes: &'a [u8], needle: &[u8]) -> Option<&'a [u8]> {
    if needle.is_empty() || bytes.len() < needle.len() {
        return None;
    }
    (0..=bytes.len() - needle.len())
        .find(|&i| &bytes[i..i + needle.len()] == needle)
        .map(|i| &bytes[i + needle.len()..])
}

/// C's `strtoul` as `crypt()`'s `rounds=` parser uses it: optional whitespace,
/// an optional sign, then decimal digits. Returns the value together with the
/// unconsumed tail, so the caller can test what stopped the scan.
///
/// A negative number wraps to something enormous in C, and no digits at all
/// yields zero without consuming anything — both land outside the accepted
/// round range, which is how php turns `rounds=-1$` and `rounds=$` into `*0`.
fn strtoul(bytes: &[u8]) -> (i64, &[u8]) {
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let negative = match bytes.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let digits_start = i;
    let mut value: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if i == digits_start {
        // No digits: the scan consumed nothing at all.
        return (0, bytes);
    }
    (if negative { i64::MAX } else { value }, &bytes[i..])
}

/// C's `strtol` as `sscanf("%ld")` applies it, for the cost and `m`/`t`/`p`
/// fields of a stored hash. Unlike [`strtoul`] a negative value stays negative,
/// which is how `password_get_info` reports a cost of `-1`. The third element
/// says whether any digit was read at all, since a failed conversion leaves
/// php's default in place rather than substituting zero.
fn strtol(bytes: &[u8]) -> (i64, &[u8], bool) {
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let negative = match bytes.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let digits_start = i;
    let mut value: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if i == digits_start {
        return (0, bytes, false);
    }
    (if negative { -value } else { value }, &bytes[i..], true)
}

/// Compare two byte strings without an early exit, so the time taken does not
/// reveal how far they agreed.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut delta = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        delta |= x ^ y;
    }
    delta == 0
}

/// Fill `buf` from the OS CSPRNG, the way `random_bytes` does.
fn os_random(buf: &mut [u8]) -> Result<(), Unwind> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .map_err(|_| Unwind::error("Unable to generate salt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed-salt vectors taken from stock php 8.5.
    #[test]
    fn crypt_matches_php() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "rasmuslerdorf",
                "$2y$10$usesomesillystringforsalt",
                "$2y$10$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG",
            ),
            (
                "rasmuslerdorf",
                "$2b$10$usesomesillystringforsalt",
                "$2b$10$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG",
            ),
            (
                "rasmuslerdorf",
                "$1$usesomes$",
                "$1$usesomes$dftn4s4hqGa1pDUjES2GI.",
            ),
            (
                "rasmuslerdorf",
                "$5$usesomesillystringforsalt$",
                "$5$usesomesillystri$KqJWpanXZHKq2BOB43TSaYhEWsQ1Lr5QNyPCDH/Tp.6",
            ),
            (
                "rasmuslerdorf",
                "$6$rounds=1000$usesomesillystri$",
                "$6$rounds=1000$usesomesillystri$HjCvpI.y.OobDcID9dgWXCgWs/IiFHrp08E8L.dk3EIh\
                 CkGCxRUinAAlLzum4MAjmlBXDY208uD6hQJyhKFKz/",
            ),
            ("rasmuslerdorf", "rl", "rl.3StKT.4T8M"),
            ("rasmuslerdorf", "_J9..rasm", "_J9..rasmBYk8r9AiWNc"),
            ("password", "_Gl/.K0Ay", "_Gl/.K0Ay.aosctsbJ1k"),
            ("test", "aZ", "aZGJuE6EXrjEE"),
        ];
        for (password, salt, expected) in cases {
            let got = crypt_bytes(password.as_bytes(), salt.as_bytes());
            assert_eq!(got, expected.as_bytes(), "crypt({password:?}, {salt:?})");
        }
    }

    /// php answers a setting it cannot use with `*0`, or `*1` when the setting
    /// began with `*0`.
    #[test]
    fn crypt_failure_tokens_match_php() {
        let failing = [
            "", "x", "$", "$2y$", "$2y$10$", "$2y$10$short", "$2y$03$usesomesillystringforsalt",
            "$2y$32$usesomesillystringforsalt", "$2z$10$usesomesillystringforsalt",
            "$2$10$usesomesillystringforsalt", "$9$abc$", "$6$rounds=999$usesomesillystri$",
            "$6$rounds=1000000000$x$", "$6$rounds=$usesomes$", "!!", "*", "*1", "_....abcd", "_",
        ];
        for salt in failing {
            assert_eq!(
                crypt_bytes(b"rasmuslerdorf", salt.as_bytes()),
                b"*0".to_vec(),
                "crypt(_, {salt:?})"
            );
        }
        assert_eq!(crypt_bytes(b"rasmuslerdorf", b"*0"), b"*1".to_vec());
    }

    /// `rounds=` follows `strtoul`, so a count is only a count when a `$`
    /// follows the digits; otherwise the whole thing is salt text.
    #[test]
    fn sha_crypt_rounds_parsing_matches_php() {
        // php's `rounds=` follows `strtoul`, so a prefix that is not digits
        // followed by `$` is not a count at all but part of the *salt*.
        // The two cases where that leaves a salt outside crypt's own
        // alphabet (`rounds=abc`, `rounds=12x`) answer `*0` here, because the
        // backend refuses a salt php passes through verbatim — see
        // COVERAGE.md. Everything that stays inside the alphabet matches.
        let cases: &[(&str, &str)] = &[
            ("$6$rounds=+5000$usesomes$", "$6$rounds=5000$usesomes$"),
            ("$6$rounds=0005000$usesomes$", "$6$rounds=5000$usesomes$"),
            ("$6$rounds=999999999$usesomes$", "$6$rounds=999999999$usesomes$"),
        ];
        for (salt, prefix) in cases {
            let got = crypt_bytes(b"rasmuslerdorf", salt.as_bytes());
            assert!(
                got.starts_with(prefix.as_bytes()),
                "crypt(_, {salt:?}) = {:?}, wanted prefix {prefix:?}",
                String::from_utf8_lossy(&got)
            );
        }
    }

    /// The password is read as a C string, so a NUL byte ends it.
    #[test]
    fn nul_truncates_password_and_salt() {
        for salt in ["$2y$04$usesomesillystringforsalt", "$1$usesomes$", "ab"] {
            assert_eq!(
                crypt_bytes(b"a\0b", salt.as_bytes()),
                crypt_bytes(b"a", salt.as_bytes()),
                "salt {salt:?}"
            );
        }
        assert_eq!(
            crypt_bytes(b"x", b"$1$ab\0cd$"),
            crypt_bytes(b"x", b"$1$ab$")
        );
    }

    /// The accumulators that decide whether `$2a$`/`$2x$` can be served, on the
    /// inputs php was measured with.
    #[test]
    fn bf_key_flags_predict_crypt_blowfish_variants() {
        // Pure ASCII: correct and buggy schedules agree, no sign extension, so
        // all four variants coincide.
        let (diff, sign) = bf_key_flags(b"abc");
        assert_eq!((diff == 0 && sign != 0, diff != 0), (false, false));
        // "café" in UTF-8 sign-extends, so `$2x$` diverges but `$2a$` does not.
        let (diff, sign) = bf_key_flags(b"caf\xc3\xa9");
        assert_eq!((diff == 0 && sign != 0, diff != 0), (false, true));
        // All-0xff: the two schedules coincide *and* a sign extension happened,
        // which is exactly when `$2a$`'s safety measure fires.
        let (diff, sign) = bf_key_flags(&[0xff; 100]);
        assert_eq!((diff == 0 && sign != 0, diff != 0), (true, false));
    }

    /// `$2b$` and `$2y$` are the same computation, so one can be derived from
    /// the other by swapping the tag.
    #[test]
    fn bcrypt_variants_share_one_computation() {
        let y = crypt_bytes(b"rasmuslerdorf", b"$2y$04$usesomesillystringforsalt");
        let b = crypt_bytes(b"rasmuslerdorf", b"$2b$04$usesomesillystringforsalt");
        assert_eq!(&y[4..], &b[4..]);
        assert_eq!(&y[..4], b"$2y$");
        assert_eq!(&b[..4], b"$2b$");
    }

    /// Only a 60-byte `$2y` string is a bcrypt hash to the password API.
    #[test]
    fn identify_matches_php() {
        let bcrypt60 = b"$2y$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG";
        assert_eq!(bcrypt60.len(), 60);
        assert_eq!(identify(bcrypt60), Some(Algo::Bcrypt));
        assert_eq!(identify(&bcrypt60[..59]), None);
        assert_eq!(
            identify(b"$2a$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG"),
            None
        );
        assert_eq!(identify(b"$1$usesomes$dftn4s4hqGa1pDUjES2GI."), None);
        assert_eq!(identify(b"$argon2i$"), Some(Algo::Argon2i));
        assert_eq!(identify(b"$argon2id$v=19$m=1,t=1,p=1$a$b"), Some(Algo::Argon2id));
    }

    /// `sscanf` semantics: digits as far as they run, defaults when there are
    /// none.
    #[test]
    fn stored_parameters_match_php() {
        let pad = "a".repeat(53);
        assert_eq!(bcrypt_cost(format!("$2y$04${pad}").as_bytes()), 4);
        assert_eq!(bcrypt_cost(format!("$2y$4x${pad}").as_bytes()), 4);
        assert_eq!(bcrypt_cost(format!("$2y$xx${pad}").as_bytes()), 12);
        assert_eq!(bcrypt_cost(format!("$2y$-1${pad}").as_bytes()), -1);

        assert_eq!(argon2_params(b"$argon2i$"), (65536, 4, 1));
        assert_eq!(
            argon2_params(b"$argon2i$v=19$m=8,t=1,p=1$abc$def"),
            (8, 1, 1)
        );
        assert_eq!(
            argon2_params(b"$argon2id$v=19$m=65536,t=4,p=1$"),
            (65536, 4, 1)
        );
    }

    /// A round trip through the argon2 writer and reader, for both variants.
    #[test]
    fn argon2_round_trips() {
        for algorithm in [Algorithm::Argon2i, Algorithm::Argon2id] {
            let mut options = Array::new();
            options.set(ArrayKey::str(b"memory_cost"), Value::Int(64));
            options.set(ArrayKey::str(b"time_cost"), Value::Int(1));
            options.set(ArrayKey::str(b"threads"), Value::Int(1));
            let hashed = argon2_hash(algorithm, b"rasmuslerdorf", Some(&options)).unwrap();
            assert!(argon2_verify(b"rasmuslerdorf", &hashed));
            assert!(!argon2_verify(b"wrong", &hashed));
            assert_eq!(argon2_params(&hashed), (64, 1, 1));
        }
    }

    /// The bcrypt writer produces something its own reader accepts.
    #[test]
    fn bcrypt_hash_round_trips() {
        let mut options = Array::new();
        options.set(ArrayKey::str(b"cost"), Value::Int(4));
        let hashed = bcrypt_hash(b"rasmuslerdorf", Some(&options)).unwrap();
        assert_eq!(hashed.len(), 60);
        assert!(hashed.starts_with(b"$2y$04$"));
        assert_eq!(identify(&hashed), Some(Algo::Bcrypt));
        assert_eq!(bcrypt_cost(&hashed), 4);
        assert!(constant_time_eq(
            &crypt_bytes(b"rasmuslerdorf", &hashed),
            &hashed
        ));
    }
}
