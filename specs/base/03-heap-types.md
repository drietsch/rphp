# 03 — Core Heap Types

**Status:** stable
**Source sections:** `base-idea.md` §11 (strings, arrays, objects/hidden classes, closures)
**Reads with:** [02-value-model.md](02-value-model.md), [04-memory-gc.md](04-memory-gc.md), [07-jit.md](07-jit.md) (typed-packed & shape guards), [decisions.md](decisions.md) (ADR-009, ADR-011)

`rphp-heap` is `#![no_std]`. All heap types begin with a `GcHeader` ([04-memory-gc.md](04-memory-gc.md)) and obey the COW/immortal protocol from [02-value-model.md](02-value-model.md).

---

## 11.1 Strings

```rust
#[repr(C)]
pub struct PhpStr {
    header: GcHeader,
    len:    u32,
    hash:   u32,          // cached, lazily computed; 0 = uncomputed (sentinel)
    flags:  StrFlags,     // INTERNED, VALID_UTF8_KNOWN, IS_SMALL, NUMERIC_KNOWN…
    data:   StrStorage,   // inline ≤ 22 bytes, else { ptr, cap }
}
```

- **Byte strings**, never assumed UTF-8 (matches [01-frontend.md](01-frontend.md)'s `&[u8]` stance).
- **Small-string optimization:** strings ≤ 22 bytes live inline in the header tail — no separate allocation. The 22 comes from the union footprint after `header/len/hash/flags`.
- **Cached hash**, computed lazily with **hardware AES-NI / VAES (or CRC32)** rather than a software hash, because string/key hashing is on the critical path of every array and property lookup ([04-memory-gc.md](04-memory-gc.md) routes this). `hash == 0` is the "uncomputed" sentinel; a real hash of 0 is stored as a fixed nonzero alias.
- **Interning** for compile-time-known strings (class/method names, literal keys). Interned strings are **immortal** (ADR-009): process-lifetime, refcount never touched, comparable by pointer.
- **COW** via `header.refcount`; mutation separates if shared (>1 and not immortal).
- **SIMD kernels** (simdutf-style, AVX-512/SVE2, AVX2 floor), shared with the lexer: length, compare, search/`strpos`, case folding, UTF-8 validation. Hot-loop concatenation can use a **rope/builder** fast path the optimizer selects when it proves the intermediate is unobserved ([07-jit.md](07-jit.md)).

---

## 11.2 Arrays — dual representation

The PHP array is list + dict + stack + ordered map in one type. Represented as a tagged enum that **auto-promotes**:

```rust
pub enum ArrayRepr {
    Packed(SlabVec<Value>),   // sequential int keys 0..n, no holes — a flat vector
    Hashed(OrderedMap),       // string keys, sparse/negative int keys, or holes
}
```

### Packed (the common case)
A flat vector. `$a[]` append and `$a[$i]` read are bounds-checked vector ops — **no hashing**. A plain list never pays for a hash table.

### Promotion (lazy)
Packed → Hashed on the **first** of: a string key, a negative/sparse int key, or a hole (unset in the middle). Promotion is one pass that rebuilds entries into the `OrderedMap` preserving order. There is no demotion (matches PHP; once hashed, stays hashed).

### Typed-packed (JIT-only refinement)
When the optimizer proves every element is `int` (or every element is `float`), a hot region operates on an unboxed `&mut [i64]` / `&mut [f64]` view ([07-jit.md](07-jit.md)), so numeric array loops touch **zero boxed values**. This is a *view* over the same packed storage, established under a guard; on guard failure it reverts to boxed packed.

### `OrderedMap`
Insertion-ordered, open-addressing, SwissTable-style:
- A contiguous **entry array** `[(key, value, next_collision)]` — position encodes insertion order, matching `foreach` order and PHP semantics.
- A **SwissTable control-byte index** probed with SIMD (16/32 lanes at a time); keys hashed with hardware AES/CRC ([04-memory-gc.md](04-memory-gc.md)).
- Tombstones for deletes; periodic compaction reclaims order-array gaps without changing observable order.

### The legacy internal pointer is a side table
`current`/`next`/`reset`/`key`/`end`/`prev` are **not** a field on every array. They are a **lazily-allocated side table** keyed by array identity, created only when one of those functions is actually called (rare in 8.5 code). The common array pays nothing — this is the §0.4 "legacy machinery leaves the hot path" lever made concrete.

### Bulk operations
`array_sum`, `array_map`, `array_filter`, `in_array`, `array_search`, comparisons over **typed-packed** storage are **SIMD kernels** over `[i64]`/`[f64]`. Above a size threshold they fan out across the isolate thread pool ([09-runtime-sapi.md](09-runtime-sapi.md)) — a pure bulk op has no shared mutable state to contend on. Below threshold, scalar/SIMD single-thread.

---

## 11.3 Objects & hidden classes — the central bet

Targeting 8.5, **dynamic properties are deprecated**, so a class declares a **sealed, compile-time-known property set by default**. Objects are therefore **structs, not dictionaries**.

```rust
#[repr(C)]
pub struct PhpObject {
    header: GcHeader,
    class:  ClassId,
    shape:  ShapeId,     // precomputed once at class definition (sealed classes)
    slots:  *mut Slot,   // flat, mixed boxed/unboxed property storage
}

pub struct Shape {
    class:   ClassId,
    layout:  Box<[SlotDesc]>,             // offset + storage kind per property
    by_name: PerfectHash<IdentId, u32>,   // property name → slot index, built once
    flags:   ShapeFlags,                  // SEALED, HAS_MAGIC, HAS_HOOKS, ALLOWS_DYNAMIC
    edges:   Option<SmallMap<IdentId, ShapeId>>, // only for #[AllowDynamicProperties]
}

pub enum SlotDesc {
    Boxed,                 // a 16-byte Value (mixed/object/array/nullable scalar)
    Int, Float, Bool,      // raw i64/f64/bool — declared scalar, stored UNBOXED
    // readonly / hook / visibility / type ride alongside in a SlotMeta
}
```

Two consequences of sealed-by-default:

- **Typed scalar properties are stored unboxed.** `public int $x` is a raw `i64` slot, not a 16-byte `Value`. Object storage is laid out like a C struct — contiguous typed/boxed slots at fixed offsets. An all-typed-scalar class is byte-for-byte a Rust struct.
- **No transition chains.** The property set is known at class definition, so the shape is built **once** and shared by every instance. `New` stamps the prebuilt shape and fills slots; it never walks an add-property edge graph. The `edges` map exists **only** for classes that opt into dynamic properties via `#[AllowDynamicProperties]`.

### Property access compilation
`PropGet`/`PropSet` compile to: **guard** the object's `ClassId` against the IC slot's cached class; on hit, **load/store `slots[offset]`** at a constant offset, typed per `SlotDesc` (raw `i64`/`f64` or a `Value`). For a monomorphic site on a `final` class the optimizer **drops even the guard** (ADR-006 closed-world proof). This is a constant-offset load, often of an unboxed scalar — versus Zend's string-keyed property hashtable probe.

### Modern-feature encoding
`readonly`, asymmetric visibility (extended to statics in 8.5), property hooks (8.4), and enums are encoded in `SlotMeta` + `ShapeFlags`. The `HAS_MAGIC`/`HAS_HOOKS` flags gate `__get`/`__set`/hook dispatch so the fast path **skips those checks entirely** for the overwhelming majority of classes that declare none. Internal `resource`-style handles are a **private object kind**, so the legacy `resource` value tag imposes nothing on the value model ([02-value-model.md](02-value-model.md)).

---

## 11.4 Closures & references

```rust
#[repr(C)]
pub struct PhpClosure {
    header:  GcHeader,
    code:    CodeRef,             // bytecode body / compiled entry
    this:    Option<*mut PhpObject>,   // bound $this
    scope:   ClassId,            // bound scope class (for visibility)
    upvals:  Box<[Value]>,       // captured per the compiler's capture descriptors
}
```

First-class callable syntax `f(...)` and the new const-expression closures lower to **this same representation** ([01-frontend.md](01-frontend.md) §6.2). Upvalue capture is by value (short closures) or by reference (a captured `Reference` cell) per the descriptor.

**References** (`PhpRef`) are defined in [02-value-model.md](02-value-model.md) §9.4 — a small heap cell holding a shared `Value` slot with its own refcount.

---

## Deviations from base-idea.md

- **None changed.** §11's decisions (SSO strings, dual-rep arrays, sealed-object hidden classes, lazy internal-pointer side table) are **affirmed and detailed**. Additions: interned strings are explicitly **immortal** (ADR-009); allocator routing of entries/cells/small-strings is specified in [04-memory-gc.md](04-memory-gc.md) (ADR-011); typed-packed is framed as a guarded JIT *view* with revert semantics ([07-jit.md](07-jit.md)).

## Open questions

- Exact SSO inline capacity (22 vs a power-of-two-friendly value) pending the final `GcHeader`/`StrStorage` layout — fix in M1 once `rphp-gc` header is frozen.
- `OrderedMap` compaction trigger policy (load factor / tombstone ratio) — tune against `.phpt` + bench in M1.
