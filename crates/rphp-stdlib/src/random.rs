//! php-src `ext/random` (the procedural API) plus the three `ext/standard`
//! functions that draw from the same engine: `rand`, `mt_rand`, `mt_srand`,
//! `srand`, `mt_getrandmax`, `getrandmax`, `random_int`, `random_bytes`,
//! `lcg_value`, `shuffle`, `str_shuffle`, `array_rand`.
//!
//! The Mersenne Twister is php's exact `Mt19937` engine — `php_mt_initialize`
//! seeding, `php_mt_reload` twist (the standard variant, and the legacy
//! `MT_RAND_PHP` one that twists with the wrong bit), `>> 1` on the 32-bit
//! output for the bare call, rejection-sampled ranges (`rand_range32` /
//! `rand_range64`), and `RAND_RANGE_BADSCALING` in legacy mode — so a seeded
//! sequence is byte-identical to stock php (`mt_srand(42); mt_rand()` is
//! `804318771`). `shuffle`/`str_shuffle`/`array_rand` are the same
//! Fisher–Yates / bitset walks php runs over it.
//!
//! An unseeded engine seeds itself from the OS (`/dev/urandom`, falling back
//! to hashing the clock and pid); `random_int` / `random_bytes` read the
//! CSPRNG directly. The engine state is a thread-local — one interpreter
//! runs per thread, so it is per-request in practice; a request-scoped slot
//! on `Interp` replaces it when `ExtState` grows one.

use std::cell::RefCell;
use std::io::Read;

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("mt_srand", 0, Some(2), mt_srand),
    nf!("srand", 0, Some(2), mt_srand),
    nf!("mt_rand", 0, Some(2), mt_rand),
    nf!("rand", 0, Some(2), rand),
    nf!("mt_getrandmax", 0, Some(0), mt_getrandmax),
    nf!("getrandmax", 0, Some(0), mt_getrandmax),
    nf!("random_int", 2, Some(2), random_int),
    nf!("random_bytes", 1, Some(1), random_bytes),
    nf!("lcg_value", 0, Some(0), lcg_value),
    nf_ref!("shuffle", 1, Some(1), 0b1, shuffle),
    nf!("str_shuffle", 1, Some(1), str_shuffle),
    nf!("array_rand", 1, Some(2), array_rand),
];

/// `MT_RAND_MT19937` / `MT_RAND_PHP`.
pub(crate) fn register_constants(r: &mut Registry) {
    r.constant("MT_RAND_MT19937", Value::Int(MT_RAND_MT19937));
    r.constant("MT_RAND_PHP", Value::Int(MT_RAND_PHP));
}

const MT_RAND_MT19937: i64 = 0;
const MT_RAND_PHP: i64 = 1;
/// `mt_getrandmax()`: the 31-bit output range.
const MT_RAND_MAX: i64 = 0x7FFF_FFFF;

const N: usize = 624;
const M: usize = 397;

/// The Mt19937 engine state, `php_random_status_state_mt19937`.
struct Mt {
    state: [u32; N],
    /// Index of the next word to output (`N` = reload first).
    next: usize,
    mode: i64,
    seeded: bool,
}

thread_local! {
    static ENGINE: RefCell<Mt> = const { RefCell::new(Mt { state: [0; N], next: N, mode: MT_RAND_MT19937, seeded: false }) };
}

impl Mt {
    /// `php_mt_initialize` + `php_mt_reload`.
    fn seed(&mut self, seed: u32) {
        self.state[0] = seed;
        for i in 1..N {
            let prev = self.state[i - 1];
            self.state[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.reload();
        self.seeded = true;
    }

    /// `php_mt_reload`: regenerate all `N` words.
    fn reload(&mut self) {
        let legacy = self.mode == MT_RAND_PHP;
        let twist = |m: u32, u: u32, v: u32| -> u32 {
            let mix = (u & 0x8000_0000) | (v & 0x7FFF_FFFF);
            // The legacy variant tests the low bit of `u` instead of `v`.
            let low = if legacy { u & 1 } else { v & 1 };
            m ^ (mix >> 1) ^ (0u32.wrapping_sub(low) & 0x9908_B0DF)
        };
        let s = &mut self.state;
        for i in 0..(N - M) {
            s[i] = twist(s[i + M], s[i], s[i + 1]);
        }
        for i in (N - M)..(N - 1) {
            s[i] = twist(s[i + M - N], s[i], s[i + 1]);
        }
        s[N - 1] = twist(s[M - 1], s[N - 1], s[0]);
        self.next = 0;
    }

    /// `php_mt_rand`: the next tempered 32-bit word.
    fn next_u32(&mut self) -> u32 {
        if !self.seeded {
            self.seed(os_seed());
        }
        if self.next >= N {
            self.reload();
        }
        let mut s1 = self.state[self.next];
        self.next += 1;
        s1 ^= s1 >> 11;
        s1 ^= (s1 << 7) & 0x9D2C_5680;
        s1 ^= (s1 << 15) & 0xEFC6_0000;
        s1 ^ (s1 >> 18)
    }

    /// `rand_range32`: uniform in `0..=umax` by rejection.
    fn range32(&mut self, umax: u32) -> u32 {
        let mut result = self.next_u32();
        if umax == u32::MAX {
            return result;
        }
        let umax = umax + 1;
        if umax & (umax - 1) == 0 {
            return result & (umax - 1);
        }
        let limit = u32::MAX - (u32::MAX % umax) - 1;
        while result > limit {
            result = self.next_u32();
        }
        result % umax
    }

    /// Two words assembled low-first (`php_random_range64` fills the 64-bit
    /// result from the least significant byte up).
    fn next_u64(&mut self) -> u64 {
        let lo = self.next_u32() as u64;
        let hi = self.next_u32() as u64;
        (hi << 32) | lo
    }

    /// `rand_range64`: uniform in `0..=umax` from two words.
    fn range64(&mut self, umax: u64) -> u64 {
        let mut result = self.next_u64();
        if umax == u64::MAX {
            return result;
        }
        let umax = umax + 1;
        if umax & (umax - 1) == 0 {
            return result & (umax - 1);
        }
        let limit = u64::MAX - (u64::MAX % umax) - 1;
        while result > limit {
            result = self.next_u64();
        }
        result % umax
    }

    /// `php_mt_rand_range`: uniform in `min..=max`.
    fn range(&mut self, min: i64, max: i64) -> i64 {
        let umax = (max as u64).wrapping_sub(min as u64);
        let r = if umax > u32::MAX as u64 {
            self.range64(umax)
        } else {
            self.range32(umax as u32) as u64
        };
        (r as i64).wrapping_add(min)
    }

    /// `php_mt_rand_common`: the range in the current mode — rejection
    /// sampling, or the legacy `RAND_RANGE_BADSCALING` in `MT_RAND_PHP`.
    fn range_common(&mut self, min: i64, max: i64) -> i64 {
        if self.mode == MT_RAND_MT19937 {
            return self.range(min, max);
        }
        let n = (self.next_u32() >> 1) as f64;
        min + ((max as f64 - min as f64 + 1.0) * (n / (MT_RAND_MAX as f64 + 1.0))) as i64
    }
}

/// A 32-bit seed from the OS CSPRNG, or from the clock and pid when
/// `/dev/urandom` is unavailable.
fn os_seed() -> u32 {
    let mut buf = [0u8; 4];
    if csprng_fill(&mut buf).is_ok() {
        return u32::from_le_bytes(buf);
    }
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u128(std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos()));
    h.finish() as u32
}

/// Fill `buf` from the OS CSPRNG (`/dev/urandom`).
fn csprng_fill(buf: &mut [u8]) -> std::io::Result<()> {
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

/// A uniform CSPRNG integer in `min..=max` (`php_random_range` over the
/// secure engine).
fn csprng_range(min: i64, max: i64) -> Result<i64, Unwind> {
    let umax = (max as u64).wrapping_sub(min as u64);
    let mut word = [0u8; 8];
    let mut next = || -> Result<u64, Unwind> {
        csprng_fill(&mut word).map_err(|e| {
            Unwind::exception("Random\\RandomException", format!("Cannot open source device: {e}"))
        })?;
        Ok(u64::from_le_bytes(word))
    };
    let mut result = next()?;
    if umax == u64::MAX {
        return Ok((result as i64).wrapping_add(min));
    }
    let span = umax + 1;
    if span & (span - 1) == 0 {
        return Ok(((result & (span - 1)) as i64).wrapping_add(min));
    }
    let limit = u64::MAX - (u64::MAX % span) - 1;
    while result > limit {
        result = next()?;
    }
    Ok(((result % span) as i64).wrapping_add(min))
}

fn with_engine<R>(f: impl FnOnce(&mut Mt) -> R) -> R {
    ENGINE.with(|e| f(&mut e.borrow_mut()))
}

/// `mt_srand(int $seed = 0, int $mode = MT_RAND_MT19937): void` (and its
/// alias `srand`). Without a seed the engine reseeds from the OS. The
/// `MT_RAND_PHP` mode carries php 8.3's two deprecations.
pub(crate) fn mt_srand(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let seed = match args.first() {
        Some(v) if !matches!(*v.deref(), Value::Null) => v.to_int() as u32,
        _ => os_seed(),
    };
    let mode = args.get(1).map_or(MT_RAND_MT19937, Value::to_int);
    if mode == MT_RAND_PHP {
        ctx.deprecated("The MT_RAND_PHP variant of Mt19937 is deprecated")?;
    }
    with_engine(|e| {
        e.mode = if mode == MT_RAND_PHP { MT_RAND_PHP } else { MT_RAND_MT19937 };
        e.seed(seed);
    });
    Ok(Value::Null)
}

fn range_args(func: &str, args: &[Value]) -> Result<Option<(i64, i64)>, Unwind> {
    if args.is_empty() {
        return Ok(None);
    }
    if args.len() == 1 {
        return Err(Unwind::argument_count_error(format!("{func}() expects exactly 2 arguments, 1 given")));
    }
    Ok(Some((args[0].to_int(), args[1].to_int())))
}

/// `mt_rand(): int` / `mt_rand(int $min, int $max): int`
pub(crate) fn mt_rand(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some((min, max)) = range_args("mt_rand", args)? else {
        return Ok(Value::Int((with_engine(Mt::next_u32) >> 1) as i64));
    };
    if max < min {
        return Err(Unwind::value_error(
            "mt_rand(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)",
        ));
    }
    Ok(Value::Int(with_engine(|e| e.range_common(min, max))))
}

/// `rand(): int` / `rand(int $min, int $max): int` — the Mt19937 engine;
/// a reversed range is swapped rather than rejected.
pub(crate) fn rand(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some((min, max)) = range_args("rand", args)? else {
        return Ok(Value::Int((with_engine(Mt::next_u32) >> 1) as i64));
    };
    let (min, max) = if max < min { (max, min) } else { (min, max) };
    Ok(Value::Int(with_engine(|e| e.range_common(min, max))))
}

/// `mt_getrandmax(): int` / `getrandmax(): int`
pub(crate) fn mt_getrandmax(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(MT_RAND_MAX))
}

/// `random_int(int $min, int $max): int` — CSPRNG.
pub(crate) fn random_int(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (min, max) = (args[0].to_int(), args[1].to_int());
    if min > max {
        return Err(Unwind::value_error(
            "random_int(): Argument #1 ($min) must be less than or equal to argument #2 ($max)",
        ));
    }
    Ok(Value::Int(csprng_range(min, max)?))
}

/// `random_bytes(int $length): string` — CSPRNG.
pub(crate) fn random_bytes(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let len = args[0].to_int();
    if len < 1 {
        return Err(Unwind::value_error("random_bytes(): Argument #1 ($length) must be greater than 0"));
    }
    let mut buf = vec![0u8; len as usize];
    csprng_fill(&mut buf).map_err(|e| {
        Unwind::exception("Random\\RandomException", format!("Cannot open source device: {e}"))
    })?;
    Ok(Value::Str(Str::from_vec(buf)))
}

/// `lcg_value(): float` — deprecated since 8.4; a uniform float in `[0, 1)`
/// from the CSPRNG (php's combined LCG is seeded from the clock and pid, so
/// its sequence is never reproducible either).
pub(crate) fn lcg_value(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function lcg_value() is deprecated since 8.4, use \\Random\\Randomizer::getFloat() instead")?;
    let r = csprng_range(0, (1i64 << 53) - 1)?;
    Ok(Value::Float(r as f64 / (1u64 << 53) as f64))
}

/// `shuffle(array &$array): true` — `php_array_data_shuffle`: Fisher–Yates
/// from the tail with `range(0, n_left)` draws; the result is re-indexed.
pub(crate) fn shuffle(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Array(a) = args[0].deref().into_owned() else {
        return Err(Unwind::type_error(format!(
            "shuffle(): Argument #1 ($array) must be of type array, {} given",
            args[0].type_name()
        )));
    };
    let mut items: Vec<Value> = a.iter().map(|(_, v)| v.clone()).collect();
    let n = items.len();
    if n > 1 {
        with_engine(|e| {
            let mut n_left = n - 1;
            while n_left > 0 {
                let j = e.range(0, n_left as i64) as usize;
                if j != n_left {
                    items.swap(n_left, j);
                }
                n_left -= 1;
            }
        });
    }
    let mut out = Array::new();
    for v in items {
        match v {
            Value::Ref(r) => out.push_ref(r),
            v => out.push(v),
        }
    }
    Value::assign(&mut args[0], Value::Array(out));
    Ok(Value::Bool(true))
}

/// `str_shuffle(string $string): string` — `php_string_shuffle`, the same
/// walk over bytes.
pub(crate) fn str_shuffle(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut bytes = args[0].to_php_bytes();
    let n = bytes.len();
    if n > 1 {
        with_engine(|e| {
            let mut n_left = n - 1;
            while n_left > 0 {
                let j = e.range(0, n_left as i64) as usize;
                if j != n_left {
                    bytes.swap(n_left, j);
                }
                n_left -= 1;
            }
        });
    }
    Ok(Value::Str(Str::from_vec(bytes)))
}

/// `array_rand(array $array, int $num = 1): int|string|array` — one random
/// key, or `$num` keys in array order chosen through php's bitset walk
/// (drawing the complement when more than half are requested).
pub(crate) fn array_rand(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Array(a) = args[0].deref().into_owned() else {
        return Err(Unwind::type_error(format!(
            "array_rand(): Argument #1 ($array) must be of type array, {} given",
            args[0].type_name()
        )));
    };
    let num_req = args.get(1).map_or(1, Value::to_int);
    let num_avail = a.len();
    if num_avail == 0 {
        return Err(Unwind::value_error("array_rand(): Argument #1 ($array) must not be empty"));
    }
    let keys: Vec<&ArrayKey> = a.keys().collect();
    if num_req == 1 {
        let i = with_engine(|e| e.range(0, num_avail as i64 - 1)) as usize;
        return Ok(keys[i].to_value());
    }
    if num_req <= 0 || num_req as usize > num_avail {
        return Err(Unwind::value_error(
            "array_rand(): Argument #2 ($num) must be between 1 and the number of elements in argument #1 ($array)",
        ));
    }
    let mut num_req = num_req as usize;
    let negative = num_req > num_avail >> 1;
    if negative {
        num_req = num_avail - num_req;
    }
    let mut bitset = vec![false; num_avail];
    with_engine(|e| {
        let mut i = num_req;
        while i > 0 {
            let r = e.range(0, num_avail as i64 - 1) as usize;
            if !bitset[r] {
                bitset[r] = true;
                i -= 1;
            }
        }
    });
    let mut out = Array::new();
    for (i, k) in keys.iter().enumerate() {
        if bitset[i] ^ negative {
            out.push(k.to_value());
        }
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use rphp_value::{ArrayKey, Value};

    use crate::tests::{arr, call_named, interp};

    fn seq(seed: i64, calls: &[&[Value]]) -> Vec<i64> {
        let mut it = interp();
        it.call_function(b"mt_srand", &[Value::Int(seed)]).unwrap();
        calls
            .iter()
            .map(|args| it.call_function(b"mt_rand", args).unwrap().to_int())
            .collect()
    }

    #[test]
    fn mt19937_matches_php_seeded_sequences() {
        assert_eq!(seq(42, &[&[], &[], &[Value::Int(1), Value::Int(100)]]), vec![804318771, 1710563033, 77]);
        assert_eq!(
            seq(42, &[&[Value::Int(1), Value::Int(100)], &[Value::Int(1), Value::Int(100)], &[Value::Int(-5), Value::Int(5)]]),
            vec![43, 68, 4]
        );
        assert_eq!(seq(42, &[&[Value::Int(0), Value::Int(4294967295)]]), vec![1608637542]);
        assert_eq!(seq(42, &[&[Value::Int(0), Value::Int(4294967296)]]), vec![2482478772]);
        assert_eq!(seq(0, &[&[]]), vec![1178568022]);
        assert_eq!(seq(-1, &[&[]]), vec![209663185]);
        assert_eq!(
            seq(
                42,
                &[
                    &[Value::Int(1), Value::Int(100)],
                    &[Value::Int(1), Value::Int(100)],
                    &[Value::Int(-5), Value::Int(5)],
                    &[Value::Int(0), Value::Int(i64::MAX)],
                    &[Value::Int(i64::MIN), Value::Int(i64::MAX)],
                ]
            ),
            vec![43, 68, 4, 4279532807823660302, 1819927850260223047]
        );
    }

    #[test]
    fn legacy_mode_uses_the_bad_scaling() {
        let mut it = interp();
        it.call_function(b"mt_srand", &[Value::Int(42), Value::Int(1)]).unwrap();
        let _ = it.take_test_output();
        assert_eq!(it.call_function(b"mt_rand", &[]).unwrap(), Value::Int(1354439493));
        assert_eq!(it.call_function(b"mt_rand", &[Value::Int(1), Value::Int(100)]).unwrap(), Value::Int(80));
        assert_eq!(it.call_function(b"rand", &[Value::Int(1), Value::Int(100)]).unwrap(), Value::Int(96));
    }

    #[test]
    fn shuffles_and_array_rand_follow_php() {
        let mut it = interp();
        it.call_function(b"mt_srand", &[Value::Int(1)]).unwrap();
        let mut args = [arr(&(1..=10).map(Value::Int).collect::<Vec<_>>())];
        it.call_native(it.native_by_name(b"shuffle").unwrap(), &mut args).unwrap();
        let got: Vec<i64> = match &args[0] {
            Value::Array(a) => a.values().map(Value::to_int).collect(),
            _ => panic!(),
        };
        assert_eq!(got, vec![1, 9, 3, 8, 7, 2, 4, 5, 10, 6]);
        it.call_function(b"mt_srand", &[Value::Int(1)]).unwrap();
        assert_eq!(it.call_function(b"str_shuffle", &[Value::string(b"abcdefghij")]).unwrap(), Value::string(b"aichgbdejf"));
        it.call_function(b"mt_srand", &[Value::Int(1)]).unwrap();
        assert_eq!(it.call_function(b"array_rand", &[arr(&[Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4), Value::Int(5)])]).unwrap(), Value::Int(0));
        let mut m = rphp_value::Array::new();
        for k in ["a", "b", "c", "d", "e"] {
            m.set(ArrayKey::str(k.as_bytes()), Value::Int(1));
        }
        assert_eq!(
            it.call_function(b"array_rand", &[Value::Array(m), Value::Int(2)]).unwrap(),
            arr(&[Value::string(b"d"), Value::string(b"e")])
        );
        let r = call_named(b"random_int", &[Value::Int(3), Value::Int(3)]);
        assert_eq!(r, Value::Int(3));
        assert_eq!(call_named(b"random_bytes", &[Value::Int(7)]).to_php_bytes().len(), 7);
    }
}
