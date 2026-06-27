# 02 — Value Model

**Status:** stable
**Source sections:** `base-idea.md` §9 (value representation), §11.4 (references & COW), Appendix A (bit layout)
**Reads with:** [03-heap-types.md](03-heap-types.md), [04-memory-gc.md](04-memory-gc.md), [07-jit.md](07-jit.md) (unboxing), [decisions.md](decisions.md) (ADR-005, ADR-009)

`rphp-value` is `#![no_std]`, allocator-pluggable, and **the sealed boundary** behind which the storage representation could change (ADR-005). Nothing outside this crate may assume the cell's bit layout; everything goes through the `Value` API.

---

## 9.1 The cell — 16-byte tagged value

```rust
#[repr(C)]
pub struct Value {
    data: ValueData,   // 8 bytes
    tag:  ValueTag,    // 1 byte
    flags: ValueFlags, // 1 byte  (see Appendix A)
    _pad: [u8; 6],     // reserved; keeps 8-byte alignment, room for future state
}

#[repr(C)]
union ValueData {
    int:   i64,
    float: f64,
    ptr:   *mut GcHeader,   // str | array | object | closure | reference
    bits:  u64,
}

#[repr(u8)]
pub enum ValueTag {
    Null, False, True,        // unit tags; no payload
    Int, Float,               // inline scalar payload
    Str, Array, Object, Closure, Reference,   // heap payload = *mut GcHeader
    // reserved, not yet emitted:
    Indirect,   // pointer to another Value slot (property/array indirection)
    Uninit,     // typed-property "declared but unset" (distinct from Null)
    FuncRef,    // resolved callable without an allocated closure
}
```

`False`/`True` are separate tags rather than a payload bit so boolean dispatch is a pure tag compare. `Uninit` distinguishes a typed property that has never been written (accessing it is an `Error`) from one explicitly set to `null` — a real 8.x semantic distinction that must not collapse to `Null`.

**Why 16 bytes, not NaN-boxing (§9.2):** PHP integers are full `i64`; a NaN payload is ~51 bits, so NaN-boxing forces either 32-bit small-ints with heap-boxed promotion or lossy integers. PHP uses 64-bit ints constantly (ids, hashes, bitmasks, timestamps), so the boxing churn lands on hot paths. The 16-byte cell stores full `i64`/`f64` inline, needs no allocation for scalars, and has two-word locality — the same shape Zend's `zval` uses, for the same reasons. A NaN-boxed variant stays a gated experiment behind this API (ADR-005).

---

## 9.3 The unboxing boundary contract (the JIT relies on this)

The 16-byte cell is the **boundary/storage format**, not the working format inside compiled code.

- **Inside Tier-2 regions and Tier-1 fast paths**, values are unboxed into typed SSA/machine registers: an `i64` in a GPR, an `f64` in an XMM/vector register, a heap pointer in a GPR. A hot numeric loop carries native scalars and **never materializes a `Value`** until it crosses a region boundary or takes a side exit.
- A `Value` is (re)materialized only at: region entry/exit, a deopt/side-exit (the abstract state is reified back into cells per the deopt map — see [07-jit.md](07-jit.md)), a call into un-inlined code, or a store to memory that escapes.

**Contract guarantees the value crate makes to the JIT:**
1. The mapping `tag → payload interpretation` is fixed and ABI-stable within a compiled artifact (the tag enum discriminants are pinned).
2. Reconstructing a `Value` from `(tag, raw payload)` is a pure, branchless store of the two words.
3. Scalar tags (`Null/False/True/Int/Float`) carry **no heap reference**, so unboxing/reboxing them is refcount-free.
4. Heap tags' payload is always a valid `*mut GcHeader` whose first word is `{ refcount: u32, kind: u8, color: u8, flags: u16 }` ([04-memory-gc.md](04-memory-gc.md), Appendix A).

This contract is what lets [07-jit.md](07-jit.md) capture the LuaJIT-style "no boxing in the loop" win without making storage lossy.

---

## 9.4 References & copy-on-write

- **`Reference`** is a distinct tag wrapping a shared, mutable `Value` slot — PHP's `&$x` aliasing. A reference is a small heap cell (`PhpRef { header, slot: Value }`) with its own refcount; multiple `Value`s tagged `Reference` can point at one `PhpRef`.
- **COW is a property of the heap *containers* (string, array), not of the `Value` cell.** It is tracked by the container's `GcHeader.refcount`, not by anything in the cell.
- A **`Value` copy** is: copy `data`+`tag`+`flags` (a 16-byte memcpy) and, **for heap tags only**, increment the target's refcount — unless the target is **immortal** (ADR-009), in which case the increment is skipped. A `Value` drop symmetrically decrements (skipped if immortal), freeing at zero.

```
copy(v):  out = v;  if heap_tag(v.tag) && !immortal(v): refcount_inc(v.ptr)
drop(v):  if heap_tag(v.tag) && !immortal(v): if refcount_dec(v.ptr) == 0 { free(v.ptr) }
```

COW write barrier on a container (`array`/`string`): if `refcount > 1` (and not immortal), **separate** (clone the backing, decrement the old), then mutate the private copy. Refcount `1` mutates in place. See [03-heap-types.md](03-heap-types.md) for per-container separation.

---

## Conversions: type juggling (8.5 semantics)

Conversions are a `rphp-value` kernel set, encoding **current 8.5 semantics** (not legacy 7.x). They are exhaustively `.phpt`/differential-tested ([10-testing.md](10-testing.md)).

### Loose equality `==` (PHP 8 rules)
- `number == number`: numeric compare.
- `number == numeric-string`: numeric compare.
- `number == non-numeric-string`: **the number is cast to string and compared as strings** (the PHP 8 change; `0 == "foo"` is **false**).
- `string == string`: if both are numeric strings, numeric compare; else byte compare.
- `bool`/`null` operands: compare as booleans.
- `array == array`: same count, same key/value pairs loosely equal (order-independent).
- `object == object`: same class and equal properties (loosely).

### Identity `===`
Same tag *and* same value; for `Object`/`Closure`/`Reference`, the **same instance** (pointer identity). No juggling. `array === array` requires same key/value pairs in the **same order**, identically.

### Ordering `<` `<=` `>` `>=` and `<=>` (spaceship)
A total-ish comparator per PHP 8 rules (numeric where both numeric, else string/array/object rules). `<=>` returns `-1|0|1`. Used by `sort` family and `match` is **not** ordering (`match` uses `===`).

### Scalar casts
`CastBool/Int/Float/String` kernels implement the standard truthiness and numeric-string rules:
- to bool: `0`, `0.0`, `""`, `"0"`, `[]`, `null` are false; else true.
- to int/float: numeric-string parsing with the 8.x rules — a **non-well-formed** leading-numeric string (`"10 apples"`) yields the leading number plus an `E_WARNING`; a fully non-numeric string in an **arithmetic** context raises `TypeError`.
- to string: `__toString` for objects (else `Error`), canonical float formatting (locale-independent, `precision`/`serialize_precision` honored — a documented divergence axis, [10-testing.md](10-testing.md) ADR-008).

These kernels are the single source of truth; the interpreter, both JIT tiers, and const-folding ([01-frontend.md](01-frontend.md) §6.3) all call them so folded and runtime results are bit-identical.

---

## Appendix A — bit layout (expanded)

16-byte cell, 8-byte aligned:
- **bytes 0..8** — payload union (`i64`/`f64`/`*mut GcHeader`).
- **byte 8** — `tag`.
- **byte 9** — `flags`: per-value markers usable without chasing the pointer — e.g. `INTERNED_KEY` on `Str` (skip hashing for array keys), `PACKED` hint on `Array`, `NUMERIC_STR_KNOWN` on `Str`. Flags are advisory/correct-by-construction; clearing them is always safe.
- **bytes 10..16** — reserved (alignment + future tags/flags).

Heap header (first word at `*mut GcHeader`): `{ refcount: u32, kind: u8, color: GcColor, flags: u16 }` — defined in [04-memory-gc.md](04-memory-gc.md). The `IMMORTAL` state (ADR-009) lives in the header `flags`, checked on the copy/drop fast path above.

---

## Deviations from base-idea.md

- **None changed.** §9's 16-byte-cell decision is **affirmed** (ADR-005); the NaN-boxed variant remains a gated experiment behind this sealed API. Additions over the baseline: the explicit **unboxing-boundary contract** (for [07-jit.md](07-jit.md)), the **immortal-skip** on copy/drop (ADR-009, defined in [04-memory-gc.md](04-memory-gc.md)), and the spelled-out **conversion kernels** as the single source of truth.

## Open questions

- Whether `FuncRef`/`Indirect`/`Uninit` ship in M1 or are introduced when their consumers land (typed-prop enforcement, array/property indirection). Tags are reserved now so the discriminant numbering is stable for the JIT ABI.
