//! `ext/random`'s object half: `Random\Randomizer`, the engines
//! (`Mt19937`, `PcgOneseq128XslRr64`, `Xoshiro256StarStar`, `Secure`), the
//! `Random\Engine` / `Random\CryptoSafeEngine` interfaces and the error
//! classes. (`Random\IntervalBoundary` is an enum, which the native registry
//! cannot declare: it lives in the embed crate's php-written prelude.)
//!
//! **Engines.** Each seeded engine keeps its state in the object's native
//! payload — the `Mt19937` one is `random.rs`'s own twister (a separate
//! instance: php's `mt_rand()` state and an engine object never share), the
//! PCG one a `u128`, Xoshiro four `u64`s — so `clone` forks it and a
//! `Randomizer` drawing from an engine advances the very object a script
//! holds. `generate()` hands back php's `php_random_result`: the value
//! little-endian in 4 (Mt19937) or 8 bytes. The state array of
//! `__serialize()` / `__debugInfo()` is php's `bin2hex_le` of every word
//! (plus Mt19937's index and mode), and `__unserialize()` validates it the
//! way php does, failing with "Invalid serialization data for … object".
//!
//! **A user engine** is any class implementing `Random\Engine`: its
//! `generate()` string is read as a little-endian integer of at most eight
//! bytes (longer strings are cut, an empty one is `BrokenRandomEngineError`).
//!
//! **The Randomizer** reproduces `php_random_range32` / `range64` exactly —
//! the results of several short draws are stitched together low byte first
//! until a full word is filled, rejection sampling gives up after 50
//! attempts — so seeded sequences match php for every engine, user ones
//! with odd output widths included. `getFloat()` is php's γ-section
//! (Goualard 2022), `getInt()` on a `MT_RAND_PHP` engine keeps php's legacy
//! bad scaling, `shuffleArray` / `shuffleBytes` / `pickArrayKeys` are the
//! same walks `shuffle()` / `str_shuffle()` / `array_rand()` run.
//!
//! No per-request state: everything lives on the objects.

use rphp_runtime::{nm, ClassFlags, Ctx, Interp, NativeFn, NativeProps, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Payload, Str, Value};

use crate::random::{csprng_fill, Mt, MT_RAND_MAX, MT_RAND_MT19937, MT_RAND_PHP, N};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

const ENGINE: &str = "Random\\Engine";
const MT19937: &str = "Random\\Engine\\Mt19937";
const PCG: &str = "Random\\Engine\\PcgOneseq128XslRr64";
const XOSHIRO: &str = "Random\\Engine\\Xoshiro256StarStar";
const SECURE: &str = "Random\\Engine\\Secure";
const RANDOMIZER: &str = "Random\\Randomizer";
const BROKEN: &str = "Random\\BrokenRandomEngineError";
const BOUNDARY: &str = "Random\\IntervalBoundary";

/// `PHP_RANDOM_RANGE_ATTEMPTS`.
const ATTEMPTS: u32 = 50;

pub(crate) fn register_classes(r: &mut Registry) {
    r.class("Random\\RandomError").extends("Error").finish();
    r.class(BROKEN).extends("Random\\RandomError").finish();
    r.class("Random\\RandomException").extends("Exception").finish();
    r.interface(ENGINE).abstract_method("generate", nm!(0, Some(0), engine_generate)).finish();
    r.interface("Random\\CryptoSafeEngine").implements(&[ENGINE]).finish();
    r.class(MT19937)
        .flags(ClassFlags::FINAL)
        .implements(&[ENGINE])
        .method("__construct", nm!(0, Some(2), mt_construct))
        .method("generate", nm!(0, Some(0), engine_generate))
        .method("__serialize", nm!(0, Some(0), engine_serialize))
        .method("__unserialize", nm!(1, Some(1), engine_unserialize))
        .method("__debugInfo", nm!(0, Some(0), engine_debug_info))
        .native_props(STRICT_PROPS)
        .payload_clone(state_clone)
        .finish();
    r.class(PCG)
        .flags(ClassFlags::FINAL)
        .implements(&[ENGINE])
        .method("__construct", nm!(0, Some(1), pcg_construct))
        .method("generate", nm!(0, Some(0), engine_generate))
        .method("jump", nm!(1, Some(1), pcg_jump))
        .method("__serialize", nm!(0, Some(0), engine_serialize))
        .method("__unserialize", nm!(1, Some(1), engine_unserialize))
        .method("__debugInfo", nm!(0, Some(0), engine_debug_info))
        .native_props(STRICT_PROPS)
        .payload_clone(state_clone)
        .finish();
    r.class(XOSHIRO)
        .flags(ClassFlags::FINAL)
        .implements(&[ENGINE])
        .method("__construct", nm!(0, Some(1), xoshiro_construct))
        .method("generate", nm!(0, Some(0), engine_generate))
        .method("jump", nm!(0, Some(0), xoshiro_jump))
        .method("jumpLong", nm!(0, Some(0), xoshiro_jump_long))
        .method("__serialize", nm!(0, Some(0), engine_serialize))
        .method("__unserialize", nm!(1, Some(1), engine_unserialize))
        .method("__debugInfo", nm!(0, Some(0), engine_debug_info))
        .native_props(STRICT_PROPS)
        .payload_clone(state_clone)
        .finish();
    r.class(SECURE)
        .flags(ClassFlags::FINAL)
        .implements(&["Random\\CryptoSafeEngine"])
        .method("generate", nm!(0, Some(0), engine_generate))
        .native_props(STRICT_PROPS)
        .uncloneable()
        .finish();
    r.class(RANDOMIZER)
        .flags(ClassFlags::FINAL)
        .readonly_prop("engine", ENGINE)
        .method("__construct", nm!(0, Some(1), randomizer_construct))
        .method("nextInt", nm!(0, Some(0), next_int))
        .method("nextFloat", nm!(0, Some(0), next_float))
        .method("getFloat", nm!(2, Some(3), get_float))
        .method("getInt", nm!(2, Some(2), get_int))
        .method("getBytes", nm!(1, Some(1), get_bytes))
        .method("getBytesFromString", nm!(2, Some(2), get_bytes_from_string))
        .method("shuffleArray", nm!(1, Some(1), shuffle_array))
        .method("shuffleBytes", nm!(1, Some(1), shuffle_bytes))
        .method("pickArrayKeys", nm!(2, Some(2), pick_array_keys))
        .method("__serialize", nm!(0, Some(0), randomizer_serialize))
        .method("__unserialize", nm!(1, Some(1), randomizer_unserialize))
        .native_props(STRICT_PROPS)
        .uncloneable()
        .finish();
}

// ---- no dynamic properties ----------------------------------------------------

/// php declares every class here `@strict-properties`: a dynamic property
/// is an `Error`, not the 8.2 deprecation.
const STRICT_PROPS: NativeProps = NativeProps {
    names: &[],
    get: no_prop_get,
    set: no_prop_set,
    isset: None,
    unset: Some(no_prop_unset),
    list: None,
    debug: None,
    cast: None,
};

fn no_prop_get(_: &mut Interp, _: &Object, _: &[u8]) -> Option<Result<Value, Unwind>> {
    None
}

fn no_prop_set(it: &mut Interp, o: &Object, name: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    Some(Err(Unwind::error(format!(
        "Cannot create dynamic property {}::${}",
        it.class_name_of(o),
        String::from_utf8_lossy(name)
    ))))
}

fn no_prop_unset(_: &mut Interp, _: &Object, _: &[u8]) -> Option<Result<(), Unwind>> {
    Some(Ok(()))
}

// ---- engine state ---------------------------------------------------------------

/// Which engine an object is: the four native classes are final, so the
/// class name decides; anything else implementing `Random\Engine` is a
/// user engine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mt,
    Pcg,
    Xoshiro,
    Secure,
    User,
}

fn kind_of(o: &Object) -> Kind {
    let layout = o.layout();
    let name = layout.class_name();
    if name == MT19937.as_bytes() {
        Kind::Mt
    } else if name == PCG.as_bytes() {
        Kind::Pcg
    } else if name == XOSHIRO.as_bytes() {
        Kind::Xoshiro
    } else if name == SECURE.as_bytes() {
        Kind::Secure
    } else {
        Kind::User
    }
}

/// A seeded engine's state (the object's payload).
#[derive(Clone)]
enum State {
    Mt(Box<Mt>),
    Pcg(u128),
    Xoshiro([u64; 4]),
}

const PCG_MULT: u128 = (2_549_297_995_355_413_924u128 << 64) | 4_865_540_595_714_422_341;
const PCG_INC: u128 = (6_364_136_223_846_793_005u128 << 64) | 1_442_695_040_888_963_407;

fn pcg_step(s: u128) -> u128 {
    s.wrapping_mul(PCG_MULT).wrapping_add(PCG_INC)
}

/// `php_random_pcgoneseq128xslrr64_seed128`.
fn pcg_seed(seed: u128) -> u128 {
    pcg_step(pcg_step(0).wrapping_add(seed))
}

/// `php_random_pcgoneseq128xslrr64_advance`: jump `advance` steps ahead.
fn pcg_advance(state: u128, mut advance: u64) -> u128 {
    let (mut cur_mult, mut cur_plus) = (PCG_MULT, PCG_INC);
    let (mut acc_mult, mut acc_plus) = (1u128, 0u128);
    while advance > 0 {
        if advance & 1 != 0 {
            acc_mult = acc_mult.wrapping_mul(cur_mult);
            acc_plus = acc_plus.wrapping_mul(cur_mult).wrapping_add(cur_plus);
        }
        cur_plus = cur_mult.wrapping_add(1).wrapping_mul(cur_plus);
        cur_mult = cur_mult.wrapping_mul(cur_mult);
        advance /= 2;
    }
    acc_mult.wrapping_mul(state).wrapping_add(acc_plus)
}

/// splitmix64, Xoshiro's integer seeder.
fn splitmix64(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut r = *seed;
    r = (r ^ (r >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    r = (r ^ (r >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    r ^ (r >> 31)
}

fn xoshiro_seed64(mut seed: u64) -> [u64; 4] {
    [splitmix64(&mut seed), splitmix64(&mut seed), splitmix64(&mut seed), splitmix64(&mut seed)]
}

/// One xoshiro256** step.
fn xoshiro_next(s: &mut [u64; 4]) -> u64 {
    let r = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
    let t = s[1] << 17;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = s[3].rotate_left(45);
    r
}

const XOSHIRO_JUMP: [u64; 4] = [0x180e_c6d3_3cfd_0aba, 0xd5a6_1266_f0c9_392c, 0xa958_2618_e03f_c9aa, 0x39ab_dc45_29b1_661c];
const XOSHIRO_JUMP_LONG: [u64; 4] = [0x76e1_5d3e_fefd_cbbf, 0xc500_4e44_1c52_2fb3, 0x7771_0069_854e_e241, 0x3910_9bb0_2acb_e635];

fn xoshiro_jump_by(s: &mut [u64; 4], jmp: &[u64; 4]) {
    let mut acc = [0u64; 4];
    for word in jmp {
        for b in 0..64 {
            if word & (1u64 << b) != 0 {
                for (a, x) in acc.iter_mut().zip(s.iter()) {
                    *a ^= x;
                }
            }
            xoshiro_next(s);
        }
    }
    *s = acc;
}

impl State {
    /// A fresh object's zeroed state (php `ecalloc`s it), before any seed.
    fn zeroed(kind: Kind) -> Option<State> {
        Some(match kind {
            Kind::Mt => State::Mt(Box::new(Mt::zeroed(MT_RAND_MT19937))),
            Kind::Pcg => State::Pcg(0),
            Kind::Xoshiro => State::Xoshiro([0; 4]),
            Kind::Secure | Kind::User => return None,
        })
    }

    /// `algo->generate`: the value and its width in bytes.
    fn generate(&mut self) -> (u64, usize) {
        match self {
            State::Mt(mt) => (u64::from(mt.next_u32()), 4),
            State::Pcg(s) => {
                *s = pcg_step(*s);
                let (hi, lo) = ((*s >> 64) as u64, *s as u64);
                ((hi ^ lo).rotate_right((hi >> 58) as u32), 8)
            }
            State::Xoshiro(s) => (xoshiro_next(s), 8),
        }
    }

    /// `algo->serialize`: php's state array.
    fn serialize(&self) -> Array {
        let mut out = Array::new();
        match self {
            State::Mt(mt) => {
                for w in &mt.state {
                    out.push(hex_le(&w.to_le_bytes()));
                }
                out.push(Value::Int(mt.next as i64));
                out.push(Value::Int(mt.mode));
            }
            State::Pcg(s) => {
                out.push(hex_le(&((*s >> 64) as u64).to_le_bytes()));
                out.push(hex_le(&(*s as u64).to_le_bytes()));
            }
            State::Xoshiro(s) => {
                for w in s {
                    out.push(hex_le(&w.to_le_bytes()));
                }
            }
        }
        out
    }

    /// `algo->unserialize`: the state a serialized array describes, or
    /// `None` when php would reject it.
    fn unserialize(kind: Kind, a: &Array) -> Option<State> {
        let word = |i: usize, bytes: usize| -> Option<u64> {
            let v = a.get(&ArrayKey::Int(i as i64))?;
            let Value::Str(s) = v else { return None };
            let b = unhex(s.as_bytes(), bytes)?;
            let mut w = [0u8; 8];
            w[..bytes].copy_from_slice(&b);
            Some(u64::from_le_bytes(w))
        };
        let int = |i: usize| -> Option<i64> {
            match a.get(&ArrayKey::Int(i as i64))? {
                Value::Int(n) => Some(*n),
                _ => None,
            }
        };
        match kind {
            Kind::Mt => {
                if a.len() != N + 2 {
                    return None;
                }
                let mut mt = Mt::zeroed(MT_RAND_MT19937);
                for (i, w) in mt.state.iter_mut().enumerate() {
                    *w = word(i, 4)? as u32;
                }
                // php stores the index in a `uint32_t`.
                let count = int(N)? as u32 as usize;
                if count > N {
                    return None;
                }
                mt.next = count;
                let mode = int(N + 1)?;
                if mode != MT_RAND_MT19937 && mode != MT_RAND_PHP {
                    return None;
                }
                mt.mode = mode;
                Some(State::Mt(Box::new(mt)))
            }
            Kind::Pcg => {
                if a.len() != 2 {
                    return None;
                }
                let (hi, lo) = (word(0, 8)?, word(1, 8)?);
                Some(State::Pcg((u128::from(hi) << 64) | u128::from(lo)))
            }
            Kind::Xoshiro => {
                if a.len() != 4 {
                    return None;
                }
                let s = [word(0, 8)?, word(1, 8)?, word(2, 8)?, word(3, 8)?];
                if s == [0; 4] {
                    return None;
                }
                Some(State::Xoshiro(s))
            }
            Kind::Secure | Kind::User => None,
        }
    }
}

/// `php_random_bin2hex_le`: the bytes as lowercase hex, in memory order.
fn hex_le(bytes: &[u8]) -> Value {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 15) as usize]);
    }
    Value::Str(Str::from_vec(out))
}

/// `php_random_hex2bin_le` over a string that must be exactly `2 * n` hex
/// digits (either case).
fn unhex(s: &[u8], n: usize) -> Option<Vec<u8>> {
    if s.len() != 2 * n {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    s.chunks(2).map(|p| Some((digit(p[0])? << 4) | digit(p[1])?)).collect()
}

/// Run `f` on a native engine's state, creating the zeroed one an object
/// made without its constructor (`unserialize`) starts from.
fn with_state<R>(o: &Object, f: impl FnOnce(&mut State) -> R) -> Option<R> {
    if o.with_payload::<State, _>(|_| ()).is_none() {
        let st = State::zeroed(kind_of(o))?;
        o.set_payload(Payload::Native(Box::new(st)));
    }
    o.with_payload(f)
}

fn state_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(st) = src.with_payload::<State, _>(|s| s.clone()) {
        dst.set_payload(Payload::Native(Box::new(st)));
    }
    Ok(())
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method cannot be called statically"))
}

fn broken(msg: &str) -> Unwind {
    Unwind::exception(BROKEN, msg)
}

fn attempts_exhausted() -> Unwind {
    broken(&format!("Failed to generate an acceptable random number in {ATTEMPTS} attempts"))
}

fn secure_bytes(buf: &mut [u8]) -> Result<(), Unwind> {
    csprng_fill(buf).map_err(|e| Unwind::exception("Random\\RandomException", format!("Cannot open source device: {e}")))
}

/// One `algo->generate` call on any engine object: the native engines in
/// place, a user engine through its `generate()` method.
fn generate(ctx: &mut Ctx, o: &Object) -> Result<(u64, usize), Unwind> {
    match kind_of(o) {
        Kind::Secure => {
            let mut b = [0u8; 8];
            secure_bytes(&mut b)?;
            Ok((u64::from_le_bytes(b), 8))
        }
        Kind::User => {
            let v = ctx.call_method(o, b"generate", &[])?;
            let bytes = v.deref().to_php_bytes();
            if bytes.is_empty() {
                return Err(broken("A random engine must return a non-empty string"));
            }
            let size = bytes.len().min(8);
            let r = bytes[..size].iter().enumerate().fold(0u64, |r, (i, b)| r | (u64::from(*b) << (8 * i)));
            Ok((r, size))
        }
        // A seeded engine always has (or gets) its state.
        _ => with_state(o, State::generate).ok_or_else(|| broken("A random engine must return a non-empty string")),
    }
}

/// The 32-bit word `php_random_range32` assembles from as many draws as it
/// takes, low bytes first.
fn draw32(ctx: &mut Ctx, o: &Object) -> Result<u32, Unwind> {
    let (mut result, mut total) = (0u32, 0usize);
    while total < 4 {
        let (r, size) = generate(ctx, o)?;
        result |= (r as u32) << (total * 8);
        total += size;
    }
    Ok(result)
}

/// The 64-bit word of `php_random_range64` / `nextFloat()`.
fn draw64(ctx: &mut Ctx, o: &Object) -> Result<u64, Unwind> {
    let (mut result, mut total) = (0u64, 0usize);
    while total < 8 {
        let (r, size) = generate(ctx, o)?;
        result |= r << (total * 8);
        total += size;
    }
    Ok(result)
}

/// `php_random_range32`: uniform in `0..=umax`.
fn range32(ctx: &mut Ctx, o: &Object, umax: u32) -> Result<u32, Unwind> {
    let mut result = draw32(ctx, o)?;
    if umax == u32::MAX {
        return Ok(result);
    }
    let umax = umax + 1;
    if umax & (umax - 1) == 0 {
        return Ok(result & (umax - 1));
    }
    let limit = u32::MAX - (u32::MAX % umax) - 1;
    let mut count = 0;
    while result > limit {
        count += 1;
        if count > ATTEMPTS {
            return Err(attempts_exhausted());
        }
        result = draw32(ctx, o)?;
    }
    Ok(result % umax)
}

/// `php_random_range64`: uniform in `0..=umax`.
fn range64(ctx: &mut Ctx, o: &Object, umax: u64) -> Result<u64, Unwind> {
    let mut result = draw64(ctx, o)?;
    if umax == u64::MAX {
        return Ok(result);
    }
    let umax = umax + 1;
    if umax & (umax - 1) == 0 {
        return Ok(result & (umax - 1));
    }
    let limit = u64::MAX - (u64::MAX % umax) - 1;
    let mut count = 0;
    while result > limit {
        count += 1;
        if count > ATTEMPTS {
            return Err(attempts_exhausted());
        }
        result = draw64(ctx, o)?;
    }
    Ok(result % umax)
}

/// `php_random_range`: uniform in `min..=max`.
fn range(ctx: &mut Ctx, o: &Object, min: i64, max: i64) -> Result<i64, Unwind> {
    let umax = (max as u64).wrapping_sub(min as u64);
    let r = if umax > u64::from(u32::MAX) {
        range64(ctx, o, umax)?
    } else {
        u64::from(range32(ctx, o, umax as u32)?)
    };
    Ok(r.wrapping_add(min as u64) as i64)
}

// ---- argument coercion ------------------------------------------------------------

fn type_error(who: &str, pos: u32, name: &str, ty: &str, v: &Value) -> Unwind {
    Unwind::type_error(format!(
        "{who}(): Argument #{pos} (${name}) must be of type {ty}, {} given",
        rphp_runtime::value_name(v)
    ))
}

fn value_error(who: &str, pos: u32, name: &str, msg: &str) -> Unwind {
    Unwind::value_error(format!("{who}(): Argument #{pos} (${name}) {msg}"))
}

/// A float that becomes an int parameter: php's precision deprecation for
/// a fractional one, `None` (the `TypeError`) outside the int range.
fn float_to_int(ctx: &mut Ctx, f: f64) -> Result<Option<i64>, Unwind> {
    // `i64::MIN as f64` is exact; `-(i64::MIN as f64)` is 2^63.
    if !f.is_finite() || f < i64::MIN as f64 || f >= -(i64::MIN as f64) {
        return Ok(None);
    }
    if f.fract() != 0.0 {
        ctx.deprecated(&format!(
            "Implicit conversion from float {} to int loses precision",
            Value::Float(f).to_php_string()
        ))?;
    }
    Ok(Some(f as i64))
}

/// php's weak `int` parameter coercion; `None` for a `null` the type
/// allows (`nullable`).
fn int_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value, nullable: bool) -> Result<Option<i64>, Unwind> {
    let ty = if nullable { "?int" } else { "int" };
    let v = v.deref();
    match &*v {
        Value::Int(i) => Ok(Some(*i)),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        Value::Float(f) => float_to_int(ctx, *f)?.map(Some).ok_or_else(|| type_error(who, pos, name, ty, &v)),
        Value::Null | Value::Uninit if nullable => Ok(None),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!("{who}(): Passing null to parameter #{pos} (${name}) of type int is deprecated"))?;
            Ok(Some(0))
        }
        s @ Value::Str(_) if s.is_numeric() => {
            let f = s.to_float();
            if f.fract() == 0.0 && f.abs() < 9.2e18 {
                Ok(Some(s.to_int()))
            } else {
                float_to_int(ctx, f)?.map(Some).ok_or_else(|| type_error(who, pos, name, ty, &v))
            }
        }
        other => Err(type_error(who, pos, name, ty, other)),
    }
}

fn int_req(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<i64, Unwind> {
    Ok(int_arg(ctx, who, pos, name, v, false)?.unwrap_or(0))
}

/// php's weak `float` parameter coercion.
fn float_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<f64, Unwind> {
    let v = v.deref();
    match &*v {
        Value::Float(f) => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!("{who}(): Passing null to parameter #{pos} (${name}) of type float is deprecated"))?;
            Ok(0.0)
        }
        s @ Value::Str(_) if s.is_numeric() => Ok(s.to_float()),
        other => Err(type_error(who, pos, name, "float", other)),
    }
}

/// php's weak `string` parameter coercion.
fn str_arg(ctx: &mut Ctx, who: &str, pos: u32, name: &str, v: &Value) -> Result<Vec<u8>, Unwind> {
    let v = v.deref();
    match &*v {
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => Ok(v.to_php_bytes()),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!("{who}(): Passing null to parameter #{pos} (${name}) of type string is deprecated"))?;
            Ok(Vec::new())
        }
        Value::Object(_) => {
            let owned = v.clone().into_owned();
            match ctx.to_string(&owned) {
                Ok(s) => Ok(s.as_bytes().to_vec()),
                Err(_) => Err(type_error(who, pos, name, "string", &owned)),
            }
        }
        other => Err(type_error(who, pos, name, "string", other)),
    }
}

fn array_arg(who: &str, pos: u32, name: &str, v: &Value) -> Result<Array, Unwind> {
    match &*v.deref() {
        Value::Array(a) => Ok(a.clone()),
        other => Err(type_error(who, pos, name, "array", other)),
    }
}

/// A `string|int|null` seed (`Z_PARAM_STR_OR_LONG_OR_NULL`).
enum Seed {
    Random,
    Int(i64),
    Bytes(Vec<u8>),
}

fn seed_arg(ctx: &mut Ctx, who: &str, v: Option<&Value>) -> Result<Seed, Unwind> {
    let Some(v) = v else { return Ok(Seed::Random) };
    let v = v.deref();
    let ty = "string|int|null";
    match &*v {
        Value::Null | Value::Uninit => Ok(Seed::Random),
        Value::Int(i) => Ok(Seed::Int(*i)),
        Value::Bool(b) => Ok(Seed::Int(i64::from(*b))),
        Value::Float(f) => match float_to_int(ctx, *f)? {
            Some(i) => Ok(Seed::Int(i)),
            None => Ok(Seed::Bytes(v.to_php_bytes())),
        },
        Value::Str(s) => Ok(Seed::Bytes(s.as_bytes().to_vec())),
        Value::Object(_) => {
            let owned = v.clone().into_owned();
            match ctx.to_string(&owned) {
                Ok(s) => Ok(Seed::Bytes(s.as_bytes().to_vec())),
                Err(_) => Err(type_error(who, 1, "seed", ty, &owned)),
            }
        }
        other => Err(type_error(who, 1, "seed", ty, other)),
    }
}

// ---- engine methods --------------------------------------------------------------

/// `Random\Engine\Mt19937::__construct(?int $seed = null, int $mode = MT_RAND_MT19937)`
fn mt_construct(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let who = "Random\\Engine\\Mt19937::__construct";
    let seed = match args.first() {
        Some(v) => int_arg(ctx, who, 1, "seed", v, true)?,
        None => None,
    };
    let mode = match args.get(1) {
        Some(v) => int_req(ctx, who, 2, "mode", v)?,
        None => MT_RAND_MT19937,
    };
    if mode == MT_RAND_PHP {
        ctx.deprecated("The MT_RAND_PHP variant of Mt19937 is deprecated")?;
    } else if mode != MT_RAND_MT19937 {
        return Err(value_error(who, 2, "mode", "must be either MT_RAND_MT19937 or MT_RAND_PHP"));
    }
    let seed = match seed {
        Some(s) => s as u32,
        None => {
            let mut b = [0u8; 4];
            secure_bytes(&mut b)?;
            u32::from_le_bytes(b)
        }
    };
    let mut mt = Mt::zeroed(mode);
    mt.seed(seed);
    o.set_payload(Payload::Native(Box::new(State::Mt(Box::new(mt)))));
    Ok(Value::Null)
}

/// `Random\Engine\PcgOneseq128XslRr64::__construct(string|int|null $seed = null)`
fn pcg_construct(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let who = "Random\\Engine\\PcgOneseq128XslRr64::__construct";
    let seed = match seed_arg(ctx, who, args.first())? {
        Seed::Random => {
            let mut b = [0u8; 16];
            secure_bytes(&mut b)?;
            u128::from_le_bytes(b)
        }
        Seed::Int(i) => u128::from(i as u64),
        Seed::Bytes(b) => {
            if b.len() != 16 {
                return Err(value_error(who, 1, "seed", "must be a 16 byte (128 bit) string"));
            }
            let hi = u64::from_le_bytes(b[..8].try_into().expect("8 bytes"));
            let lo = u64::from_le_bytes(b[8..].try_into().expect("8 bytes"));
            (u128::from(hi) << 64) | u128::from(lo)
        }
    };
    o.set_payload(Payload::Native(Box::new(State::Pcg(pcg_seed(seed)))));
    Ok(Value::Null)
}

/// `Random\Engine\PcgOneseq128XslRr64::jump(int $advance): void`
fn pcg_jump(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let who = "Random\\Engine\\PcgOneseq128XslRr64::jump";
    let advance = int_req(ctx, who, 1, "advance", &args[0])?;
    if advance < 0 {
        return Err(value_error(who, 1, "advance", "must be greater than or equal to 0"));
    }
    with_state(o, |s| {
        if let State::Pcg(p) = s {
            *p = pcg_advance(*p, advance as u64);
        }
    });
    Ok(Value::Null)
}

/// `Random\Engine\Xoshiro256StarStar::__construct(string|int|null $seed = null)`
fn xoshiro_construct(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let who = "Random\\Engine\\Xoshiro256StarStar::__construct";
    let words = |b: &[u8]| -> [u64; 4] {
        let w = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
        [w(0), w(1), w(2), w(3)]
    };
    let state = match seed_arg(ctx, who, args.first())? {
        Seed::Random => loop {
            let mut b = [0u8; 32];
            secure_bytes(&mut b)?;
            let s = words(&b);
            if s != [0; 4] {
                break s;
            }
        },
        Seed::Int(i) => xoshiro_seed64(i as u64),
        Seed::Bytes(b) => {
            if b.len() != 32 {
                return Err(value_error(who, 1, "seed", "must be a 32 byte (256 bit) string"));
            }
            let s = words(&b);
            if s == [0; 4] {
                return Err(value_error(who, 1, "seed", "must not consist entirely of NUL bytes"));
            }
            s
        }
    };
    o.set_payload(Payload::Native(Box::new(State::Xoshiro(state))));
    Ok(Value::Null)
}

fn xoshiro_jump_with(this_: Option<&Object>, jmp: &[u64; 4]) -> NativeResult {
    let o = this(this_)?;
    with_state(o, |s| {
        if let State::Xoshiro(x) = s {
            xoshiro_jump_by(x, jmp);
        }
    });
    Ok(Value::Null)
}

/// `Random\Engine\Xoshiro256StarStar::jump(): void` — 2^128 steps.
fn xoshiro_jump(_: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    xoshiro_jump_with(this_, &XOSHIRO_JUMP)
}

/// `Random\Engine\Xoshiro256StarStar::jumpLong(): void` — 2^192 steps.
fn xoshiro_jump_long(_: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    xoshiro_jump_with(this_, &XOSHIRO_JUMP_LONG)
}

/// `generate(): string` of every native engine: the value little-endian,
/// as wide as the engine's output.
fn engine_generate(ctx: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let (r, size) = generate(ctx, o)?;
    Ok(Value::Str(Str::from_vec(r.to_le_bytes()[..size].to_vec())))
}

/// `__serialize(): array` — `[properties, state]`.
fn engine_serialize(_: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let state = with_state(o, |s| s.serialize()).unwrap_or_default();
    let mut out = Array::new();
    out.push(Value::Array(Array::new()));
    out.push(Value::Array(state));
    Ok(Value::Array(out))
}

/// `__unserialize(array $data): void`
fn engine_unserialize(_: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let class = kind_name(kind_of(o));
    let data = array_arg(&format!("{class}::__unserialize"), 1, "data", &args[0])?;
    let invalid = || Unwind::exception("Exception", format!("Invalid serialization data for {class} object"));
    if data.len() != 2 {
        return Err(invalid());
    }
    // No property can be restored: the classes admit no dynamic ones.
    match data.get(&ArrayKey::Int(0)) {
        Some(Value::Array(m)) if m.is_empty() => {}
        _ => return Err(invalid()),
    }
    let Some(Value::Array(st)) = data.get(&ArrayKey::Int(1)) else { return Err(invalid()) };
    let state = State::unserialize(kind_of(o), st).ok_or_else(invalid)?;
    o.set_payload(Payload::Native(Box::new(state)));
    Ok(Value::Null)
}

/// `__debugInfo(): array` — the properties and `__states`.
fn engine_debug_info(_: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let state = with_state(o, |s| s.serialize()).unwrap_or_default();
    let mut out = Array::new();
    out.set(ArrayKey::str(b"__states"), Value::Array(state));
    Ok(Value::Array(out))
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Mt => MT19937,
        Kind::Pcg => PCG,
        Kind::Xoshiro => XOSHIRO,
        Kind::Secure => SECURE,
        Kind::User => ENGINE,
    }
}

// ---- Randomizer --------------------------------------------------------------------

/// The Randomizer's engine (its readonly `engine` property).
fn engine_of(this_: Option<&Object>) -> Result<Object, Unwind> {
    match this(this_)?.get(b"engine").map(|v| v.deref().into_owned()) {
        Some(Value::Object(e)) => Ok(e),
        _ => Err(Unwind::error(
            "Typed property Random\\Randomizer::$engine must not be accessed before initialization",
        )),
    }
}

fn is_engine(ctx: &Ctx, o: &Object) -> bool {
    ctx.class_by_name(ENGINE.as_bytes()).is_some_and(|e| ctx.instanceof_class(o.class_id(), e))
}

/// `Random\Randomizer::__construct(?Random\Engine $engine = null)` — the
/// default engine is a fresh `Secure`.
fn randomizer_construct(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let engine = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(Value::Object(e)) if is_engine(ctx, &e) => Some(e),
        Some(other) => {
            return Err(type_error("Random\\Randomizer::__construct", 1, "engine", "?Random\\Engine", &other));
        }
    };
    let engine = match engine {
        Some(e) => e,
        None => {
            let cid = ctx
                .class_by_name(SECURE.as_bytes())
                .ok_or_else(|| Unwind::error("Class \"Random\\Engine\\Secure\" not found"))?;
            ctx.new_object(cid)?
        }
    };
    if o.get(b"engine").is_some_and(|v| !matches!(*v.deref(), Value::Uninit)) {
        return Err(Unwind::error("Cannot modify readonly property Random\\Randomizer::$engine"));
    }
    o.set(b"engine", Value::Object(engine));
    Ok(Value::Null)
}

/// `nextInt(): int` — one draw, shifted right one bit.
fn next_int(ctx: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let e = engine_of(this_)?;
    let (r, _) = generate(ctx, &e)?;
    Ok(Value::Int((r >> 1) as i64))
}

/// `nextFloat(): float` — the top 53 bits of a 64-bit draw, in `[0, 1)`.
fn next_float(ctx: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let e = engine_of(this_)?;
    let r = draw64(ctx, &e)?;
    Ok(Value::Float((r >> 11) as f64 * (1.0 / (1u64 << 53) as f64)))
}

/// `getInt(int $min, int $max): int`
fn get_int(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = "Random\\Randomizer::getInt";
    let min = int_req(ctx, who, 1, "min", &args[0])?;
    let max = int_req(ctx, who, 2, "max", &args[1])?;
    if max < min {
        return Err(value_error(who, 2, "max", "must be greater than or equal to argument #1 ($min)"));
    }
    let e = engine_of(this_)?;
    // A `MT_RAND_PHP` engine keeps `mt_rand()`'s legacy scaling.
    let legacy = kind_of(&e) == Kind::Mt
        && with_state(&e, |s| matches!(s, State::Mt(m) if m.mode != MT_RAND_MT19937)).unwrap_or(false);
    if legacy {
        let r = (generate(ctx, &e)?.0 >> 1) as f64;
        let offset = ((max as f64 - min as f64 + 1.0) * (r / (MT_RAND_MAX as f64 + 1.0))) as u64;
        return Ok(Value::Int(offset.wrapping_add(min as u64) as i64));
    }
    Ok(Value::Int(range(ctx, &e, min, max)?))
}

/// `getBytes(int $length): string` — draws concatenated little-endian.
fn get_bytes(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = "Random\\Randomizer::getBytes";
    let len = int_req(ctx, who, 1, "length", &args[0])?;
    if len < 1 {
        return Err(value_error(who, 1, "length", "must be greater than 0"));
    }
    let e = engine_of(this_)?;
    let len = len as usize;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let (r, size) = generate(ctx, &e)?;
        let take = size.min(len - out.len());
        out.extend_from_slice(&r.to_le_bytes()[..take]);
    }
    Ok(Value::Str(Str::from_vec(out)))
}

/// `getBytesFromString(string $string, int $length): string` — a masked
/// byte-at-a-time walk for alphabets up to 256 bytes, a range draw per
/// byte above that.
fn get_bytes_from_string(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = "Random\\Randomizer::getBytesFromString";
    let source = str_arg(ctx, who, 1, "string", &args[0])?;
    let len = int_req(ctx, who, 2, "length", &args[1])?;
    if source.is_empty() {
        return Err(value_error(who, 1, "string", "must not be empty"));
    }
    if len < 1 {
        return Err(value_error(who, 2, "length", "must be greater than 0"));
    }
    let e = engine_of(this_)?;
    let len = len as usize;
    let max_offset = (source.len() - 1) as u64;
    let mut out = Vec::with_capacity(len);
    if max_offset > 0xff {
        while out.len() < len {
            let off = range(ctx, &e, 0, max_offset as i64)?;
            out.push(source[off as usize]);
        }
    } else {
        let mut mask = max_offset;
        mask |= mask >> 1;
        mask |= mask >> 2;
        mask |= mask >> 4;
        let mut failures = 0;
        'outer: while out.len() < len {
            let (r, size) = generate(ctx, &e)?;
            for j in 0..size {
                let off = (r >> (j * 8)) & mask;
                if off > max_offset {
                    failures += 1;
                    if failures > ATTEMPTS {
                        return Err(attempts_exhausted());
                    }
                    continue;
                }
                failures = 0;
                out.push(source[off as usize]);
                if out.len() >= len {
                    break 'outer;
                }
            }
        }
    }
    Ok(Value::Str(Str::from_vec(out)))
}

/// The Fisher–Yates walk of `php_array_data_shuffle` /
/// `php_binary_string_shuffle`.
fn shuffle_in_place<T>(ctx: &mut Ctx, e: &Object, items: &mut [T]) -> Result<(), Unwind> {
    let n = items.len();
    if n <= 1 {
        return Ok(());
    }
    let mut n_left = n - 1;
    while n_left > 0 {
        let j = range(ctx, e, 0, n_left as i64)? as usize;
        if j != n_left {
            items.swap(n_left, j);
        }
        n_left -= 1;
    }
    Ok(())
}

/// `shuffleArray(array $array): array` — the values, shuffled and
/// re-indexed.
fn shuffle_array(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let a = array_arg("Random\\Randomizer::shuffleArray", 1, "array", &args[0])?;
    let e = engine_of(this_)?;
    let mut items: Vec<Value> = a.values().cloned().collect();
    shuffle_in_place(ctx, &e, &mut items)?;
    let mut out = Array::new();
    for v in items {
        match v {
            Value::Ref(r) => out.push_ref(r),
            v => out.push(v),
        }
    }
    Ok(Value::Array(out))
}

/// `shuffleBytes(string $bytes): string`
fn shuffle_bytes(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let mut bytes = str_arg(ctx, "Random\\Randomizer::shuffleBytes", 1, "bytes", &args[0])?;
    let e = engine_of(this_)?;
    shuffle_in_place(ctx, &e, &mut bytes)?;
    Ok(Value::Str(Str::from_vec(bytes)))
}

/// `pickArrayKeys(array $array, int $num): array` — `php_array_pick_keys`:
/// one draw for a single key, else the bitset walk (over the complement
/// when more than half are wanted), keys in array order.
fn pick_array_keys(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = "Random\\Randomizer::pickArrayKeys";
    let a = array_arg(who, 1, "array", &args[0])?;
    let num = int_req(ctx, who, 2, "num", &args[1])?;
    let e = engine_of(this_)?;
    let avail = a.len();
    if avail == 0 {
        return Err(value_error(who, 1, "array", "must not be empty"));
    }
    let keys: Vec<&ArrayKey> = a.keys().collect();
    let mut out = Array::new();
    if num == 1 {
        let i = range(ctx, &e, 0, avail as i64 - 1)? as usize;
        out.push(keys[i].to_value());
        return Ok(Value::Array(out));
    }
    if num <= 0 || num as usize > avail {
        return Err(value_error(
            who,
            2,
            "num",
            "must be between 1 and the number of elements in argument #1 ($array)",
        ));
    }
    let mut want = num as usize;
    let negative = want > avail >> 1;
    if negative {
        want = avail - want;
    }
    let mut bitset = vec![false; avail];
    let mut failures = 0;
    while want > 0 {
        let r = range(ctx, &e, 0, avail as i64 - 1)? as usize;
        if bitset[r] {
            failures += 1;
            if failures > ATTEMPTS {
                return Err(attempts_exhausted());
            }
        } else {
            bitset[r] = true;
            want -= 1;
            failures = 0;
        }
    }
    for (i, k) in keys.iter().enumerate() {
        if bitset[i] ^ negative {
            out.push(k.to_value());
        }
    }
    Ok(Value::Array(out))
}

/// `getFloat(float $min, float $max, IntervalBoundary $boundary = ClosedOpen): float`
fn get_float(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let who = "Random\\Randomizer::getFloat";
    let min = float_arg(ctx, who, 1, "min", &args[0])?;
    let max = float_arg(ctx, who, 2, "max", &args[1])?;
    let boundary = match args.get(2).map(|v| v.deref().into_owned()) {
        None => b"ClosedOpen".to_vec(),
        Some(Value::Object(b)) if b.layout().class_name() == BOUNDARY.as_bytes() => {
            b.get(b"name").map(|n| n.to_php_bytes()).unwrap_or_default()
        }
        Some(other) => return Err(type_error(who, 3, "boundary", BOUNDARY, &other)),
    };
    if !min.is_finite() {
        return Err(value_error(who, 1, "min", "must be finite"));
    }
    if !max.is_finite() {
        return Err(value_error(who, 2, "max", "must be finite"));
    }
    let e = engine_of(this_)?;
    let greater = || value_error(who, 2, "max", "must be greater than argument #1 ($min)");
    let r = match &boundary[..] {
        b"ClosedClosed" => {
            if max < min {
                return Err(value_error(who, 2, "max", "must be greater than or equal to argument #1 ($min)"));
            }
            gamma_closed_closed(ctx, &e, min, max)?
        }
        b"OpenClosed" => {
            if max <= min {
                return Err(greater());
            }
            gamma_open_closed(ctx, &e, min, max)?
        }
        b"OpenOpen" => {
            if max <= min {
                return Err(greater());
            }
            let r = gamma_open_open(ctx, &e, min, max)?;
            if r.is_nan() {
                return Err(Unwind::value_error(
                    "The given interval is empty, there are no floats between argument #1 ($min) and argument #2 ($max)",
                ));
            }
            r
        }
        _ => {
            if max <= min {
                return Err(greater());
            }
            gamma_closed_open(ctx, &e, min, max)?
        }
    };
    Ok(Value::Float(r))
}

// ---- the γ-section ---------------------------------------------------------------
//
// php's `gammasection.c`: "Drawing Random Floating-Point Numbers from an
// Interval", Frédéric Goualard, ACM TOMACS 32:3, 2022.

/// `nextafter(x, -DBL_MAX)`.
fn toward_min(x: f64) -> f64 {
    if x == -f64::MAX { x } else { x.next_down() }
}

/// `nextafter(x, DBL_MAX)`.
fn toward_max(x: f64) -> f64 {
    if x == f64::MAX { x } else { x.next_up() }
}

fn gamma_max(x: f64, y: f64) -> f64 {
    if x.abs() > y.abs() { toward_max(x) - x } else { y - toward_min(y) }
}

fn split(v: u64) -> (f64, f64) {
    ((v >> 2) as f64, (v & 3) as f64)
}

fn ceilint(a: f64, b: f64, g: f64) -> u64 {
    let s = b / g - a / g;
    let e = if a.abs() <= b.abs() { -a / g - (s - b / g) } else { b / g - (s + a / g) };
    let si = s.ceil();
    if s != si { si as u64 } else { si as u64 + u64::from(e > 0.0) }
}

fn from_max(max: f64, k: u64, g: f64) -> f64 {
    let (hi, lo) = split(k);
    4.0 * (max / 4.0 - hi * g) - lo * g
}

fn from_min(min: f64, k: u64, g: f64) -> f64 {
    let (hi, lo) = split(k);
    4.0 * (min / 4.0 + hi * g) + lo * g
}

fn gamma_closed_open(ctx: &mut Ctx, e: &Object, min: f64, max: f64) -> Result<f64, Unwind> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max <= min || hi < 1 {
        return Ok(f64::NAN);
    }
    let k = 1 + range64(ctx, e, hi - 1)?;
    Ok(if min.abs() <= max.abs() {
        if k == hi { min } else { from_max(max, k, g) }
    } else {
        from_min(min, k - 1, g)
    })
}

fn gamma_closed_closed(ctx: &mut Ctx, e: &Object, min: f64, max: f64) -> Result<f64, Unwind> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max < min {
        return Ok(f64::NAN);
    }
    let k = range64(ctx, e, hi)?;
    Ok(if min.abs() <= max.abs() {
        if k == hi { min } else { from_max(max, k, g) }
    } else if k == hi {
        max
    } else {
        from_min(min, k, g)
    })
}

fn gamma_open_closed(ctx: &mut Ctx, e: &Object, min: f64, max: f64) -> Result<f64, Unwind> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max <= min || hi < 1 {
        return Ok(f64::NAN);
    }
    let k = range64(ctx, e, hi - 1)?;
    Ok(if min.abs() <= max.abs() {
        from_max(max, k, g)
    } else if k == hi - 1 {
        max
    } else {
        from_min(min, k + 1, g)
    })
}

fn gamma_open_open(ctx: &mut Ctx, e: &Object, min: f64, max: f64) -> Result<f64, Unwind> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max <= min || hi < 2 {
        return Ok(f64::NAN);
    }
    let k = 1 + range64(ctx, e, hi - 2)?;
    Ok(if min.abs() <= max.abs() { from_max(max, k, g) } else { from_min(min, k, g) })
}

// ---- Randomizer serialization -------------------------------------------------------

/// `__serialize(): array` — `[properties]`.
fn randomizer_serialize(_: &mut Ctx, this_: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let mut props = Array::new();
    if let Some(v) = o.get(b"engine") {
        if !matches!(*v.deref(), Value::Uninit) {
            props.set(ArrayKey::str(b"engine"), v.deref().into_owned());
        }
    }
    let mut out = Array::new();
    out.push(Value::Array(props));
    Ok(Value::Array(out))
}

/// `__unserialize(array $data): void`
fn randomizer_unserialize(ctx: &mut Ctx, this_: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(this_)?;
    let data = array_arg("Random\\Randomizer::__unserialize", 1, "data", &args[0])?;
    let invalid = || Unwind::exception("Exception", "Invalid serialization data for Random\\Randomizer object");
    if data.len() != 1 {
        return Err(invalid());
    }
    let Some(Value::Array(members)) = data.get(&ArrayKey::Int(0)) else { return Err(invalid()) };
    let mut engine = None;
    for (k, v) in members.iter() {
        match (k, &*v.deref()) {
            (ArrayKey::Str(s), Value::Object(e)) if s.as_bytes() == b"engine" && is_engine(ctx, e) => {
                engine = Some(e.clone());
            }
            _ => return Err(invalid()),
        }
    }
    let engine = engine.ok_or_else(invalid)?;
    // A constructed Randomizer's readonly engine cannot be replaced (GH-19765).
    if o.get(b"engine").is_some_and(|v| !matches!(*v.deref(), Value::Uninit)) {
        return Err(invalid());
    }
    o.set(b"engine", Value::Object(engine));
    Ok(Value::Null)
}
