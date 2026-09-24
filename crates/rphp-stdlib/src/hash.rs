//! `hash` extension — message digests one-shot (`md5`, `sha1`, `hash`),
//! incremental (`hash_init`/`update`/`final` over a `HashContext`), keyed
//! (`hash_hmac`) and derived (`hash_pbkdf2`, `hash_hkdf`). Byte-oriented like
//! the rest of stdlib: the input is the `(string)` cast of each argument, the
//! result is a lowercase-hex string, or the raw digest bytes when the
//! `$binary` flag is set. Backed by the pure-Rust `md-5`, `sha1`, `sha2` and
//! `crc32fast` crates plus the four trivial checksums php ships (Adler-32,
//! the FNV pair, Jenkins one-at-a-time), so there is no `unsafe` and no
//! OpenSSL.
//!
//! **What a `HashContext` holds.** php shows one property, `algo`, so that is
//! a real declared slot; the running state lives in the instance's native
//! payload, which `payload_clone` copies so `hash_copy()` really forks a
//! digest. A context that has been finalized is spent: php's
//! `HashContext` cannot be reused, and using one raises.
//!
//! **Algorithms.** php lists 60; this is the 33 that a pure-Rust crate or a
//! few lines of arithmetic can answer exactly — the MD2/MD4/MD5, SHA-1,
//! SHA-2 and SHA-3 families, RIPEMD, Whirlpool, all four xxHash variants,
//! three CRC-32s, Adler-32, the four FNV-1 forms and joaat. The rest —
//! tiger, gost, snefru, haval and the murmur3 trio — are cataloged, not
//! faked: an unknown name is php's `ValueError`.
//!
//! **xxHash is little-endian on the way out.** php writes the 32- and
//! 64-bit variants big-endian but XXH3's 128-bit state high half first,
//! which is what makes `hash('xxh128', …)` agree with `XXH128` the C
//! function.
use crc32fast::Hasher as Crc32;
use md2::Md2;
use md4::Md4;
use md5::{Digest, Md5};
use ripemd::{Ripemd128, Ripemd160, Ripemd256, Ripemd320};
use sha1::Sha1;
use sha2::{Sha224, Sha256, Sha384, Sha512, Sha512_224, Sha512_256};
use sha3::{Sha3_224, Sha3_256, Sha3_384, Sha3_512};
use whirlpool::Whirlpool;
use xxhash_rust::xxh32::Xxh32;
use xxhash_rust::xxh3::Xxh3;
use xxhash_rust::xxh64::Xxh64;

use rphp_value::{Array, Object, Payload, Str, Value};

use rphp_runtime::{nf, nm, ClassFlags, Ctx, NativeFn, NativeResult, Registry, Unwind, Visibility};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("md5", 1, Some(2), md5),
    nf!("sha1", 1, Some(2), sha1),
    nf!("crc32", 1, Some(1), crc32),
    nf!("hash", 2, Some(4), hash),
    nf!("hash_algos", 0, Some(0), hash_algos),
    nf!("hash_hmac_algos", 0, Some(0), hash_hmac_algos),
    nf!("hash_init", 1, Some(3), hash_init),
    nf!("hash_update", 2, Some(2), hash_update),
    nf!("hash_final", 1, Some(2), hash_final),
    nf!("hash_copy", 1, Some(1), hash_copy),
    nf!("hash_equals", 2, Some(2), hash_equals),
    nf!("hash_hmac", 3, Some(4), hash_hmac),
    nf!("hash_file", 2, Some(3), hash_file),
    nf!("hash_hmac_file", 3, Some(4), hash_hmac_file),
    nf!("hash_pbkdf2", 4, Some(6), hash_pbkdf2),
    nf!("hash_hkdf", 2, Some(5), hash_hkdf),
    nf!("hash_update_file", 2, Some(3), hash_update_file),
    nf!("hash_update_stream", 2, Some(3), hash_update_stream),
    nf!("mhash", 2, Some(3), mhash),
    nf!("mhash_count", 0, Some(0), mhash_count),
    nf!("mhash_get_block_size", 1, Some(1), mhash_get_block_size),
    nf!("mhash_get_hash_name", 1, Some(1), mhash_get_hash_name),
    nf!("mhash_keygen_s2k", 4, Some(4), mhash_keygen_s2k),
];

/// The byte string an argument coerces to (the `(string)` cast), so any scalar
/// can be hashed the way PHP's weak typing allows.
fn bytes(v: &Value) -> Vec<u8> {
    v.to_php_bytes()
}

pub(crate) fn md5(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let binary = args.get(1).is_some_and(Value::to_bool);
    Ok(digest_value(
        &one_shot(Md5::new(), &bytes(&args[0])),
        binary,
    ))
}

pub(crate) fn sha1(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let binary = args.get(1).is_some_and(Value::to_bool);
    Ok(digest_value(
        &one_shot(Sha1::new(), &bytes(&args[0])),
        binary,
    ))
}

pub(crate) fn crc32(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // The standard reflected CRC-32 (IEEE 802.3), returned as a plain int. PHP
    // hands back the unsigned 32-bit value, which on a 64-bit build is just the
    // u32 widened (never negative).
    Ok(Value::Int(crc32_of(&bytes(&args[0])) as i64))
}

pub(crate) fn hash(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let data = bytes(&args[1]);
    let binary = args.get(2).is_some_and(Value::to_bool);
    let mut st = state_for(ctx, &algo, "hash")?;
    st.update(&data);
    Ok(digest_value(&st.finish(), binary))
}

pub(crate) fn hash_algos(_: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for name in SUPPORTED_ALGOS {
        out.push(Value::string(name.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `hash_hmac_algos(): array` — the subset a *keyed* hash accepts, which is
/// every real digest: a checksum has no block size to key with.
pub(crate) fn hash_hmac_algos(_: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for name in SUPPORTED_ALGOS {
        if State::new(name.as_bytes()).is_some_and(|s| s.block_size().is_some()) {
            out.push(Value::string(name.as_bytes()));
        }
    }
    Ok(Value::Array(out))
}

/// `hash_init(string $algo, int $flags = 0, string $key = "", array $options = []): HashContext`
pub(crate) fn hash_init(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let flags = args.get(1).map_or(0, |v| v.deref().to_int());
    let key = args
        .get(2)
        .map(|v| v.to_php_bytes().to_vec())
        .unwrap_or_default();
    let st = state_for(ctx, &algo, "hash_init")?;
    // `HASH_HMAC` keys the context: php runs the inner pass eagerly, so
    // everything fed in afterwards lands inside it.
    let ctx_state = if flags & HASH_HMAC != 0 {
        let Some(block) = st.block_size() else {
            return Err(Unwind::value_error(
                "hash_init(): Argument #1 ($algo) must be a cryptographic hashing algorithm if HMAC is requested",
            ));
        };
        if key.is_empty() {
            return Err(Unwind::value_error(
                "hash_init(): Argument #3 ($key) must not be empty when HMAC is requested",
            ));
        }
        let (inner, outer) = hmac_pads(&algo, &key, block);
        let mut st = State::new(&algo).expect("checked above");
        st.update(&inner);
        CtxState {
            algo: algo.to_ascii_lowercase(),
            state: Some(st),
            hmac: Some(outer),
        }
    } else {
        CtxState {
            algo: algo.to_ascii_lowercase(),
            state: Some(st),
            hmac: None,
        }
    };
    let cid = ctx
        .class_by_name(b"HashContext")
        .ok_or_else(|| Unwind::error("Class \"HashContext\" not found"))?;
    let o = ctx.instantiate(cid);
    o.set(b"algo", Value::string(&ctx_state.algo));
    o.set_payload(Payload::Native(Box::new(ctx_state)));
    Ok(Value::Object(o))
}

/// `hash_update(HashContext $context, string $data): bool`
pub(crate) fn hash_update(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let data = bytes(&args[1]);
    let o = context_arg(ctx, &args[0], "hash_update")?;
    with_context(&o, "hash_update", |c| {
        c.state.as_mut().expect("live").update(&data);
        Value::Bool(true)
    })
}

/// `hash_final(HashContext $context, bool $binary = false): string` — and the
/// context is spent afterwards, as php's is.
pub(crate) fn hash_final(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let binary = args.get(1).is_some_and(|v| v.deref().to_bool());
    let o = context_arg(ctx, &args[0], "hash_final")?;
    let algo = with_context(&o, "hash_final", |c| c.algo.clone())?;
    let (raw, outer) = with_context(&o, "hash_final", |c| {
        let raw = c.state.take().expect("live").finish();
        (raw, c.hmac.take())
    })?;
    let digest = match outer {
        // The outer pass of an HMAC context.
        Some(outer) => {
            let mut st = State::new(&algo).expect("context algorithm");
            st.update(&outer);
            st.update(&raw);
            st.finish()
        }
        None => raw,
    };
    Ok(digest_value(&digest, binary))
}

/// `hash_copy(HashContext $context): HashContext` — a fork of the running
/// state, which is what the payload clone hook is for.
pub(crate) fn hash_copy(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = context_arg(ctx, &args[0], "hash_copy")?;
    let copy = with_context(&o, "hash_copy", |c| c.clone())?;
    let cid = o.class_id();
    let new = ctx.instantiate(cid);
    new.set(b"algo", Value::string(&copy.algo));
    new.set_payload(Payload::Native(Box::new(copy)));
    Ok(Value::Object(new))
}

/// `hash_equals(string $known_string, string $user_string): bool` — the
/// comparison whose running time does not depend on where the strings differ.
pub(crate) fn hash_equals(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let a = bytes(&args[0]);
    let b = bytes(&args[1]);
    if a.len() != b.len() {
        return Ok(Value::Bool(false));
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    Ok(Value::Bool(diff == 0))
}

/// `hash_hmac(string $algo, string $data, string $key, bool $binary = false): string`
pub(crate) fn hash_hmac(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let data = bytes(&args[1]);
    let key = bytes(&args[2]);
    let binary = args.get(3).is_some_and(|v| v.deref().to_bool());
    let raw = hmac(ctx, &algo, &key, &data, "hash_hmac")?;
    Ok(digest_value(&raw, binary))
}

/// `hash_file(string $algo, string $filename, bool $binary = false): string|false`
pub(crate) fn hash_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let binary = args.get(2).is_some_and(|v| v.deref().to_bool());
    let Some(data) = read_file(ctx, &args[1], "hash_file")? else {
        return Ok(Value::Bool(false));
    };
    let mut st = state_for(ctx, &algo, "hash_file")?;
    st.update(&data);
    Ok(digest_value(&st.finish(), binary))
}

/// `hash_hmac_file(string $algo, string $filename, string $key, bool $binary = false): string|false`
pub(crate) fn hash_hmac_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let key = bytes(&args[2]);
    let binary = args.get(3).is_some_and(|v| v.deref().to_bool());
    let Some(data) = read_file(ctx, &args[1], "hash_hmac_file")? else {
        return Ok(Value::Bool(false));
    };
    let raw = hmac(ctx, &algo, &key, &data, "hash_hmac_file")?;
    Ok(digest_value(&raw, binary))
}

/// `hash_pbkdf2(string $algo, string $password, string $salt, int $iterations, int $length = 0, bool $binary = false): string`
pub(crate) fn hash_pbkdf2(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let password = bytes(&args[1]);
    let salt = bytes(&args[2]);
    let iterations = args[3].deref().to_int();
    let length = args.get(4).map_or(0, |v| v.deref().to_int());
    let binary = args.get(5).is_some_and(|v| v.deref().to_bool());
    if iterations <= 0 {
        return Err(Unwind::value_error(
            "hash_pbkdf2(): Argument #4 ($iterations) must be greater than 0",
        ));
    }
    if length < 0 {
        return Err(Unwind::value_error(
            "hash_pbkdf2(): Argument #5 ($length) must be greater than or equal to 0",
        ));
    }
    let hlen = hmac(ctx, &algo, &password, b"", "hash_pbkdf2")?.len();
    // php's `$length` counts *output* characters: hex ones unless `$binary`.
    let want = if length == 0 {
        hlen
    } else if binary {
        length as usize
    } else {
        ((length + 1) / 2) as usize
    };
    let mut out: Vec<u8> = Vec::with_capacity(want);
    let mut block = 1u32;
    while out.len() < want {
        let mut salted = salt.clone();
        salted.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac(ctx, &algo, &password, &salted, "hash_pbkdf2")?;
        let mut acc = u.clone();
        for _ in 1..iterations {
            u = hmac(ctx, &algo, &password, &u, "hash_pbkdf2")?;
            for (a, b) in acc.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        out.extend_from_slice(&acc);
        block += 1;
    }
    out.truncate(want);
    if binary || length == 0 {
        Ok(digest_value(&out, binary))
    } else {
        // An odd `$length` cuts the hex string, not the bytes.
        let mut hex = to_hex(&out);
        hex.truncate(length as usize);
        Ok(Value::Str(Str::from_vec(hex)))
    }
}

/// `hash_hkdf(string $algo, string $key, int $length = 0, string $info = "", string $salt = ""): string`
pub(crate) fn hash_hkdf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let algo = bytes(&args[0]);
    let key = bytes(&args[1]);
    let length = args.get(2).map_or(0, |v| v.deref().to_int());
    let info = args
        .get(3)
        .map(|v| v.to_php_bytes().to_vec())
        .unwrap_or_default();
    let salt = args
        .get(4)
        .map(|v| v.to_php_bytes().to_vec())
        .unwrap_or_default();
    if key.is_empty() {
        return Err(Unwind::value_error(
            "hash_hkdf(): Argument #2 ($key) must not be empty",
        ));
    }
    let hlen = hmac(ctx, &algo, b"", b"", "hash_hkdf")?.len();
    if length < 0 || length as usize > 255 * hlen {
        return Err(Unwind::value_error(format!(
            "hash_hkdf(): Argument #3 ($length) must be less than or equal to {}",
            255 * hlen
        )));
    }
    let want = if length == 0 { hlen } else { length as usize };
    // Extract, then expand (RFC 5869); an empty salt is `hlen` zero bytes.
    let salt = if salt.is_empty() {
        vec![0u8; hlen]
    } else {
        salt
    };
    let prk = hmac(ctx, &algo, &salt, &key, "hash_hkdf")?;
    let mut out: Vec<u8> = Vec::with_capacity(want);
    let mut prev: Vec<u8> = Vec::new();
    let mut counter = 1u8;
    while out.len() < want {
        let mut block = prev.clone();
        block.extend_from_slice(&info);
        block.push(counter);
        prev = hmac(ctx, &algo, &prk, &block, "hash_hkdf")?;
        out.extend_from_slice(&prev);
        counter += 1;
    }
    out.truncate(want);
    Ok(Value::Str(Str::from_vec(out)))
}

/// Register `HashContext`. php's dump shows one property, `algo`.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.0.class_by_name(b"HashContext").is_some() {
        return;
    }
    r.class("HashContext")
        .flags(ClassFlags::FINAL)
        .prop("algo", Visibility::Public, Value::Null)
        .method_vis(
            "__construct",
            Visibility::Private,
            nm!(0, Some(0), ctx_construct),
        )
        .payload_clone(ctx_payload_clone)
        .finish();
}

/// php's `HASH_*` flags.
pub(crate) fn register_constants(r: &mut Registry) {
    r.constant("HASH_HMAC", Value::Int(HASH_HMAC));
    for (id, name, _, _) in MHASH_ALGOS {
        r.deprecated_constant(
            &format!("MHASH_{name}"),
            Value::Int(*id),
            " since 8.5, as the mhash*() functions were deprecated",
        );
    }
}

/// `HASH_HMAC`.
const HASH_HMAC: i64 = 1;

/// php makes the constructor private: a context only comes from
/// `hash_init()`.
fn ctx_construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error(
        "Call to private HashContext::__construct() from global scope",
    ))
}

/// `clone $ctx` forks the running digest, like `hash_copy()`.
fn ctx_payload_clone(
    _: &mut rphp_runtime::Interp,
    src: &Object,
    dst: &Object,
) -> Result<(), Unwind> {
    if let Some(c) = src.with_payload::<CtxState, _>(|c| c.clone()) {
        dst.set_payload(Payload::Native(Box::new(c)));
    }
    Ok(())
}

/// The state behind a `HashContext`: the running digest, and the outer HMAC
/// pad when the context was keyed.
#[derive(Clone)]
struct CtxState {
    algo: Vec<u8>,
    /// `None` once `hash_final()` has spent it.
    state: Option<State>,
    hmac: Option<Vec<u8>>,
}

/// The argument as a live `HashContext`.
fn context_arg(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Object, Unwind> {
    match &*v.deref() {
        Value::Object(o) if ctx.class_name_of(o) == "HashContext" => Ok(o.clone()),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($context) must be of type HashContext, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// Run `f` on a context that has not been finalized.
fn with_context<R>(
    o: &Object,
    func: &str,
    f: impl FnOnce(&mut CtxState) -> R,
) -> Result<R, Unwind> {
    let spent = o.with_payload::<CtxState, _>(|c| c.state.is_none());
    match spent {
        Some(false) => Ok(o.with_payload::<CtxState, _>(f).expect("payload present")),
        // php: using a context after `hash_final()` is an error, and one that
        // never went through `hash_init()` has no state at all.
        _ => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($context) must be a valid, non-finalized HashContext"
        ))),
    }
}

/// The digest state for an algorithm name, or php's `ValueError`.
fn state_for(_: &mut Ctx, algo: &[u8], func: &str) -> Result<State, Unwind> {
    State::new(algo).ok_or_else(|| {
        Unwind::value_error(format!(
            "{func}(): Argument #1 ($algo) must be a valid hashing algorithm"
        ))
    })
}

/// One HMAC pass: `H((K ^ opad) || H((K ^ ipad) || m))`.
fn hmac(
    ctx: &mut Ctx,
    algo: &[u8],
    key: &[u8],
    data: &[u8],
    func: &str,
) -> Result<Vec<u8>, Unwind> {
    let st = state_for(ctx, algo, func)?;
    let Some(block) = st.block_size() else {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm"
        )));
    };
    let (inner, outer) = hmac_pads(algo, key, block);
    let mut h = State::new(algo).expect("checked above");
    h.update(&inner);
    h.update(data);
    let inner_digest = h.finish();
    let mut h = State::new(algo).expect("checked above");
    h.update(&outer);
    h.update(&inner_digest);
    Ok(h.finish())
}

/// The two padded keys HMAC needs. A key longer than the block is replaced by
/// its own digest first.
fn hmac_pads(algo: &[u8], key: &[u8], block: usize) -> (Vec<u8>, Vec<u8>) {
    let mut k = if key.len() > block {
        let mut h = State::new(algo).expect("caller checked the name");
        h.update(key);
        h.finish()
    } else {
        key.to_vec()
    };
    k.resize(block, 0);
    let inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    (inner, outer)
}

/// Read a file for `hash_file`/`hash_hmac_file`, warning as php does.
fn read_file(ctx: &mut Ctx, v: &Value, func: &str) -> Result<Option<Vec<u8>>, Unwind> {
    let path = crate::filestat::arg_path(ctx, v);
    match std::fs::read(&path) {
        Ok(b) => Ok(Some(b)),
        Err(e) => {
            let shown = String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
            ctx.warn(&format!(
                "{func}({shown}): Failed to open stream: {}",
                crate::filestat::io_text(&e)
            ))?;
            Ok(None)
        }
    }
}

// ---- incremental input from files and streams --------------------------------

/// `hash_update_file(HashContext $context, string $filename, ?resource $stream_context = null): bool`
pub(crate) fn hash_update_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = context_arg(ctx, &args[0], "hash_update_file")?;
    with_context(&o, "hash_update_file", |_| ())?;
    let Some(data) = read_file(ctx, &args[1], "hash_update_file")? else {
        return Ok(Value::Bool(false));
    };
    with_context(&o, "hash_update_file", |c| {
        c.state.as_mut().expect("live").update(&data);
        Value::Bool(true)
    })
}

/// `hash_update_stream(HashContext $context, resource $stream, int $length = -1): int`
/// — how many bytes it fed in; a negative length reads to the end.
pub(crate) fn hash_update_stream(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = context_arg(ctx, &args[0], "hash_update_stream")?;
    with_context(&o, "hash_update_stream", |_| ())?;
    let stream = args[1].deref().into_owned();
    if !matches!(stream, Value::Resource(_)) {
        return Err(Unwind::type_error(format!(
            "hash_update_stream(): Argument #2 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(&stream)
        )));
    }
    let length = args.get(2).map_or(-1, |v| v.deref().to_int());
    if length == 0 {
        return Ok(Value::Int(0));
    }
    let max = if length < 0 { Value::Null } else { Value::Int(length) };
    let data = ctx.call_function(b"stream_get_contents", &[stream, max])?;
    let data = match data {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    };
    with_context(&o, "hash_update_stream", |c| {
        c.state.as_mut().expect("live").update(&data);
        Value::Int(data.len() as i64)
    })
}

// ---- mhash (deprecated since 8.1) ----------------------------------------------

/// php's `mhash_to_hash` table: the `MHASH_*` number, the name
/// `mhash_get_hash_name()` answers, the `hash()` algorithm behind it and
/// its digest size (what `mhash_get_block_size()` really reports). The
/// holes (4, 6, 26) are ids libmhash had and php never mapped.
const MHASH_ALGOS: &[(i64, &str, &str, usize)] = &[
    (0, "CRC32", "crc32", 4),
    (1, "MD5", "md5", 16),
    (2, "SHA1", "sha1", 20),
    (3, "HAVAL256", "haval256,3", 32),
    (5, "RIPEMD160", "ripemd160", 20),
    (7, "TIGER", "tiger192,3", 24),
    (8, "GOST", "gost", 32),
    (9, "CRC32B", "crc32b", 4),
    (10, "HAVAL224", "haval224,3", 28),
    (11, "HAVAL192", "haval192,3", 24),
    (12, "HAVAL160", "haval160,3", 20),
    (13, "HAVAL128", "haval128,3", 16),
    (14, "TIGER128", "tiger128,3", 16),
    (15, "TIGER160", "tiger160,3", 20),
    (16, "MD4", "md4", 16),
    (17, "SHA256", "sha256", 32),
    (18, "ADLER32", "adler32", 4),
    (19, "SHA224", "sha224", 28),
    (20, "SHA512", "sha512", 64),
    (21, "SHA384", "sha384", 48),
    (22, "WHIRLPOOL", "whirlpool", 64),
    (23, "RIPEMD128", "ripemd128", 16),
    (24, "RIPEMD256", "ripemd256", 32),
    (25, "RIPEMD320", "ripemd320", 40),
    (27, "SNEFRU256", "snefru256", 32),
    (28, "MD2", "md2", 16),
    (29, "FNV132", "fnv132", 4),
    (30, "FNV1A32", "fnv1a32", 4),
    (31, "FNV164", "fnv164", 8),
    (32, "FNV1A64", "fnv1a64", 8),
    (33, "JOAAT", "joaat", 4),
    (34, "CRC32C", "crc32c", 4),
    (35, "MURMUR3A", "murmur3a", 4),
    (36, "MURMUR3C", "murmur3c", 16),
    (37, "MURMUR3F", "murmur3f", 16),
    (38, "XXH32", "xxh32", 4),
    (39, "XXH64", "xxh64", 8),
    (40, "XXH3", "xxh3", 8),
    (41, "XXH128", "xxh128", 16),
];

/// The table row for an `MHASH_*` number.
fn mhash_entry(id: i64) -> Option<&'static (i64, &'static str, &'static str, usize)> {
    MHASH_ALGOS.iter().find(|e| e.0 == id)
}

/// The digest state behind an `MHASH_*` number; `Ok(None)` for a number
/// php does not map. An algorithm php has and rphp does not (tiger, gost,
/// snefru, haval, murmur3) is an error rather than a made-up digest.
fn mhash_state(func: &str, id: i64) -> Result<Option<(&'static str, State)>, Unwind> {
    let Some(&(_, _, algo, _)) = mhash_entry(id) else {
        return Ok(None);
    };
    match State::new(algo.as_bytes()) {
        Some(st) => Ok(Some((algo, st))),
        None => Err(Unwind::error(format!(
            "{func}(): the {algo} algorithm is not available in rphp"
        ))),
    }
}

/// `mhash(int $algo, string $data, ?string $key = null): string|false` —
/// the raw digest, or an HMAC with a key.
pub(crate) fn mhash(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function mhash() is deprecated since 8.1")?;
    let id = args[0].deref().to_int();
    let data = bytes(&args[1]);
    let key = args.get(2).filter(|v| !matches!(&*v.deref(), Value::Null)).map(bytes);
    let Some((algo, mut st)) = mhash_state("mhash", id)? else {
        return Ok(Value::Bool(false));
    };
    let raw = match key {
        Some(key) => hmac(ctx, algo.as_bytes(), &key, &data, "mhash")?,
        None => {
            st.update(&data);
            st.finish()
        }
    };
    Ok(Value::Str(Str::from_vec(raw)))
}

/// `mhash_count(): int` — the highest `MHASH_*` number.
pub(crate) fn mhash_count(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function mhash_count() is deprecated since 8.1")?;
    Ok(Value::Int(MHASH_ALGOS.last().map_or(0, |e| e.0)))
}

/// `mhash_get_block_size(int $algo): int|false` — php answers the digest
/// size here, not the block size.
pub(crate) fn mhash_get_block_size(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function mhash_get_block_size() is deprecated since 8.1")?;
    Ok(match mhash_entry(args[0].deref().to_int()) {
        Some(e) => Value::Int(e.3 as i64),
        None => Value::Bool(false),
    })
}

/// `mhash_get_hash_name(int $algo): string|false`
pub(crate) fn mhash_get_hash_name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function mhash_get_hash_name() is deprecated since 8.1")?;
    Ok(match mhash_entry(args[0].deref().to_int()) {
        Some(e) => Value::string(e.1.as_bytes()),
        None => Value::Bool(false),
    })
}

/// `mhash_keygen_s2k(int $algo, string $password, string $salt, int $length): string|false`
/// — OpenPGP's salted S2K: the salt padded or cut to eight bytes, and one
/// digest per output block, each prefixed with one more NUL than the last.
pub(crate) fn mhash_keygen_s2k(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function mhash_keygen_s2k() is deprecated since 8.1")?;
    let id = args[0].deref().to_int();
    let password = bytes(&args[1]);
    let mut salt = bytes(&args[2]);
    let length = args[3].deref().to_int() as i32;
    if length <= 0 {
        return Err(Unwind::value_error(
            "mhash_keygen_s2k(): Argument #4 ($length) must be a greater than 0",
        ));
    }
    salt.resize(8, 0);
    let Some((algo, _)) = mhash_state("mhash_keygen_s2k", id)? else {
        return Ok(Value::Bool(false));
    };
    let length = length as usize;
    let mut key = Vec::with_capacity(length);
    let mut i = 0;
    while key.len() < length {
        let mut st = State::new(algo.as_bytes()).expect("checked above");
        st.update(&vec![0u8; i]);
        st.update(&salt);
        st.update(&password);
        key.extend_from_slice(&st.finish());
        i += 1;
    }
    key.truncate(length);
    Ok(Value::Str(Str::from_vec(key)))
}

// ---- helpers ----------------------------------------------------------------

/// The algorithm names `hash()` accepts, in php's own `hash_algos()` order
/// for the ones it has. A strict subset of stock php's 60 (see the header).
const SUPPORTED_ALGOS: &[&str] = &[
    "md2",
    "md4",
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512/224",
    "sha512/256",
    "sha512",
    "sha3-224",
    "sha3-256",
    "sha3-384",
    "sha3-512",
    "ripemd128",
    "ripemd160",
    "ripemd256",
    "ripemd320",
    "whirlpool",
    "adler32",
    "crc32",
    "crc32b",
    "crc32c",
    "fnv132",
    "fnv1a32",
    "fnv164",
    "fnv1a64",
    "joaat",
    "xxh32",
    "xxh64",
    "xxh3",
    "xxh128",
];

/// A running digest. One variant per algorithm rather than a boxed trait
/// object, because `hash_copy()` has to clone it.
#[derive(Clone)]
enum State {
    Md2(Md2),
    Md4(Md4),
    Md5(Md5),
    Sha1(Sha1),
    Sha224(Sha224),
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
    Sha512_224(Sha512_224),
    Sha512_256(Sha512_256),
    Sha3_224(Sha3_224),
    Sha3_256(Sha3_256),
    Sha3_384(Sha3_384),
    Sha3_512(Sha3_512),
    Ripemd128(Ripemd128),
    Ripemd160(Ripemd160),
    Ripemd256(Ripemd256),
    Ripemd320(Ripemd320),
    Whirlpool(Whirlpool),
    /// php's `crc32`: the non-reflected CRC-32/BZIP2 polynomial.
    Crc32(u32),
    /// php's `crc32b`: the reflected IEEE CRC-32 every other language calls
    /// "crc32".
    Crc32b(Crc32),
    Adler(u32, u32),
    Fnv132(u32),
    Fnv1a32(u32),
    Fnv164(u64),
    Fnv1a64(u64),
    Joaat(u32),
    /// php's `crc32c`: the Castagnoli polynomial, reflected.
    Crc32c(u32),
    Xxh32(Xxh32),
    Xxh64(Xxh64),
    /// XXH3, 64 bits out.
    Xxh3(Box<Xxh3>),
    /// The same state, 128 bits out.
    Xxh128(Box<Xxh3>),
}

impl State {
    /// The state for an algorithm name (case-insensitive, as php resolves
    /// it), or `None` when nothing here implements it.
    fn new(algo: &[u8]) -> Option<State> {
        Some(match algo.to_ascii_lowercase().as_slice() {
            b"md2" => State::Md2(Md2::new()),
            b"md4" => State::Md4(Md4::new()),
            b"md5" => State::Md5(Md5::new()),
            b"sha1" => State::Sha1(Sha1::new()),
            b"sha224" => State::Sha224(Sha224::new()),
            b"sha256" => State::Sha256(Sha256::new()),
            b"sha384" => State::Sha384(Sha384::new()),
            b"sha512" => State::Sha512(Sha512::new()),
            b"sha512/224" => State::Sha512_224(Sha512_224::new()),
            b"sha512/256" => State::Sha512_256(Sha512_256::new()),
            b"sha3-224" => State::Sha3_224(Sha3_224::new()),
            b"sha3-256" => State::Sha3_256(Sha3_256::new()),
            b"sha3-384" => State::Sha3_384(Sha3_384::new()),
            b"sha3-512" => State::Sha3_512(Sha3_512::new()),
            b"ripemd128" => State::Ripemd128(Ripemd128::new()),
            b"ripemd160" => State::Ripemd160(Ripemd160::new()),
            b"ripemd256" => State::Ripemd256(Ripemd256::new()),
            b"ripemd320" => State::Ripemd320(Ripemd320::new()),
            b"whirlpool" => State::Whirlpool(Whirlpool::new()),
            b"crc32" => State::Crc32(0xFFFF_FFFF),
            b"crc32b" => State::Crc32b(Crc32::new()),
            b"adler32" => State::Adler(1, 0),
            b"fnv132" => State::Fnv132(FNV32_OFFSET),
            b"fnv1a32" => State::Fnv1a32(FNV32_OFFSET),
            b"fnv164" => State::Fnv164(FNV64_OFFSET),
            b"fnv1a64" => State::Fnv1a64(FNV64_OFFSET),
            b"joaat" => State::Joaat(0),
            b"crc32c" => State::Crc32c(0),
            b"xxh32" => State::Xxh32(Xxh32::new(0)),
            b"xxh64" => State::Xxh64(Xxh64::new(0)),
            b"xxh3" => State::Xxh3(Box::new(Xxh3::new())),
            b"xxh128" => State::Xxh128(Box::new(Xxh3::new())),
            _ => return None,
        })
    }

    /// The HMAC block size, or `None` for a checksum php refuses to key.
    fn block_size(&self) -> Option<usize> {
        Some(match self {
            State::Md2(_) => 16,
            State::Md4(_)
            | State::Md5(_)
            | State::Sha1(_)
            | State::Sha224(_)
            | State::Sha256(_)
            | State::Ripemd128(_)
            | State::Ripemd160(_)
            | State::Ripemd256(_)
            | State::Ripemd320(_) => 64,
            State::Sha384(_) | State::Sha512(_) | State::Sha512_224(_) | State::Sha512_256(_) => {
                128
            }
            State::Sha3_224(_) => 144,
            State::Sha3_256(_) => 136,
            State::Sha3_384(_) => 104,
            State::Sha3_512(_) => 72,
            State::Whirlpool(_) => 64,
            _ => return None,
        })
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            State::Md2(h) => h.update(data),
            State::Md4(h) => h.update(data),
            State::Md5(h) => h.update(data),
            State::Sha1(h) => h.update(data),
            State::Sha224(h) => h.update(data),
            State::Sha256(h) => h.update(data),
            State::Sha384(h) => h.update(data),
            State::Sha512(h) => h.update(data),
            State::Sha512_224(h) => h.update(data),
            State::Sha512_256(h) => h.update(data),
            State::Sha3_224(h) => h.update(data),
            State::Sha3_256(h) => h.update(data),
            State::Sha3_384(h) => h.update(data),
            State::Sha3_512(h) => h.update(data),
            State::Ripemd128(h) => h.update(data),
            State::Ripemd160(h) => h.update(data),
            State::Ripemd256(h) => h.update(data),
            State::Ripemd320(h) => h.update(data),
            State::Whirlpool(h) => h.update(data),
            State::Crc32(c) => *c = crc32_bzip2_update(*c, data),
            State::Crc32c(c) => *c = crc32c::crc32c_append(*c, data),
            State::Xxh32(h) => h.update(data),
            State::Xxh64(h) => h.update(data),
            State::Xxh3(h) | State::Xxh128(h) => h.update(data),
            State::Crc32b(h) => h.update(data),
            State::Adler(a, b) => {
                for &byte in data {
                    *a = (*a + u32::from(byte)) % 65521;
                    *b = (*b + *a) % 65521;
                }
            }
            State::Fnv132(h) => {
                for &byte in data {
                    *h = h.wrapping_mul(FNV32_PRIME) ^ u32::from(byte);
                }
            }
            State::Fnv1a32(h) => {
                for &byte in data {
                    *h = (*h ^ u32::from(byte)).wrapping_mul(FNV32_PRIME);
                }
            }
            State::Fnv164(h) => {
                for &byte in data {
                    *h = h.wrapping_mul(FNV64_PRIME) ^ u64::from(byte);
                }
            }
            State::Fnv1a64(h) => {
                for &byte in data {
                    *h = (*h ^ u64::from(byte)).wrapping_mul(FNV64_PRIME);
                }
            }
            State::Joaat(h) => {
                for &byte in data {
                    *h = h.wrapping_add(u32::from(byte));
                    *h = h.wrapping_add(*h << 10);
                    *h ^= *h >> 6;
                }
            }
        }
    }

    /// The digest bytes, big-endian for the integer checksums — which is how
    /// php renders them.
    fn finish(self) -> Vec<u8> {
        match self {
            State::Md2(h) => h.finalize().to_vec(),
            State::Md4(h) => h.finalize().to_vec(),
            State::Md5(h) => h.finalize().to_vec(),
            State::Sha1(h) => h.finalize().to_vec(),
            State::Sha224(h) => h.finalize().to_vec(),
            State::Sha256(h) => h.finalize().to_vec(),
            State::Sha384(h) => h.finalize().to_vec(),
            State::Sha512(h) => h.finalize().to_vec(),
            State::Sha512_224(h) => h.finalize().to_vec(),
            State::Sha512_256(h) => h.finalize().to_vec(),
            State::Sha3_224(h) => h.finalize().to_vec(),
            State::Sha3_256(h) => h.finalize().to_vec(),
            State::Sha3_384(h) => h.finalize().to_vec(),
            State::Sha3_512(h) => h.finalize().to_vec(),
            State::Ripemd128(h) => h.finalize().to_vec(),
            State::Ripemd160(h) => h.finalize().to_vec(),
            State::Ripemd256(h) => h.finalize().to_vec(),
            State::Ripemd320(h) => h.finalize().to_vec(),
            State::Whirlpool(h) => h.finalize().to_vec(),
            // php writes this one's state out **little-endian**, which is why
            // `hash('crc32', …)` and `hash('crc32b', …)` of the same input
            // are byte-reversals of each other rather than equal.
            State::Crc32(c) => (!c).to_le_bytes().to_vec(),
            State::Crc32b(h) => h.finalize().to_be_bytes().to_vec(),
            State::Adler(a, b) => ((b << 16) | a).to_be_bytes().to_vec(),
            State::Fnv132(h) | State::Fnv1a32(h) => h.to_be_bytes().to_vec(),
            State::Fnv164(h) | State::Fnv1a64(h) => h.to_be_bytes().to_vec(),
            State::Joaat(h) => {
                let mut h = h;
                h = h.wrapping_add(h << 3);
                h ^= h >> 11;
                h = h.wrapping_add(h << 15);
                h.to_be_bytes().to_vec()
            }
            State::Crc32c(c) => c.to_be_bytes().to_vec(),
            State::Xxh32(h) => h.digest().to_be_bytes().to_vec(),
            State::Xxh64(h) => h.digest().to_be_bytes().to_vec(),
            State::Xxh3(h) => h.digest().to_be_bytes().to_vec(),
            State::Xxh128(h) => h.digest128().to_be_bytes().to_vec(),
        }
    }
}

/// FNV-1's 32-bit offset basis and prime.
const FNV32_OFFSET: u32 = 0x811c_9dc5;
const FNV32_PRIME: u32 = 0x0100_0193;
/// The 64-bit pair.
const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// php's `crc32` algorithm: the CRC-32/BZIP2 variant, most-significant bit
/// first with polynomial `0x04C11DB7` and no reflection — a different answer
/// from `crc32()` the *function*, which is `crc32b`.
fn crc32_bzip2_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

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
