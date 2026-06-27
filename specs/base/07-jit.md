# 07 — JIT: Tiers, Speculation, Deopt, AOT

**Status:** stable
**Source sections:** `base-idea.md` §13 (JIT), §14 (type-driven specialization & AOT)
**Reads with:** [02-value-model.md](02-value-model.md) (unboxing boundary), [04-memory-gc.md](04-memory-gc.md) (refcount/safepoints), [05-bytecode-isa.md](05-bytecode-isa.md), [06-interpreter.md](06-interpreter.md) (deopt target), [decisions.md](decisions.md) (ADR-006, ADR-010)

Two tiers above the interpreter, sized to a small-team budget while reaching native speed. The interpreter ([06-interpreter.md](06-interpreter.md)) is the correctness reference; every tier must match its observable behavior exactly, and deopt falls back to it.

---

## 13.1 Tier 1 — copy-and-patch baseline

The baseline JIT uses **copy-and-patch compilation** (Xu & Kjolstad, 2021; the technique in CPython's experimental JIT).

- **Build time:** a stencil library is generated — one pre-compiled machine-code template per bytecode opcode, with **holes** for operands and continuation addresses. `xtask` drives stencil generation ([00-overview.md](00-overview.md)); the one-op→one-stencil property is guaranteed by the encoding in [05-bytecode-isa.md](05-bytecode-isa.md).
- **Runtime:** compiling a function is **memcpy-ing stencils and patching the holes** — no IR, no register allocator, microseconds per function. The result is straight-line native code that removes interpreter dispatch and **inlines the inline-cache fast paths** ([06-interpreter.md](06-interpreter.md) §12.3).

Tier 1 is **whole-method, non-speculative, and correct for any types**. It exists to get hot code off the interpreter immediately at near-zero compile latency. It is the **workhorse tier** and the one most code ever reaches; the call ABI it emits is the shared frame ABI from [05-bytecode-isa.md](05-bytecode-isa.md) so its frames interleave with interpreted and Tier-2 frames.

---

## 13.2 Tier 2 — optimizing JIT (Cranelift, region-based)

Very hot loops/regions are recompiled by `rphp-jit-opt`, a profile-guided **region-based** optimizer lowered through **Cranelift**.

**Region-based**, not pure method- or trace-based: HHVM's published result (PLDI 2018) is that for PHP-shaped code, profile-guided region selection beats both. A region is a hot, type-stable subgraph stitched from profiled edges, possibly spanning inlined callees.

### Pipeline
1. **Region formation** from edge profiles ([06-interpreter.md](06-interpreter.md)) and the call-target feedback in IC slots; hot **monomorphic** callees are inlined into the region.
2. **Type specialization** from IC type histograms:
   - a site that saw only `int` → unboxed `i64` arithmetic under a type guard;
   - a property site that saw one shape → a shape-guarded constant-offset load ([03-heap-types.md](03-heap-types.md));
   - an array always packed-int → an unboxed `[i64]` view ([03-heap-types.md](03-heap-types.md) typed-packed).
   Values are unboxed into typed SSA per the [02-value-model.md](02-value-model.md) §9.3 contract; a `Value` is materialized only at region boundaries and side exits.
3. **Guards & side exits.** Every speculation is protected by a guard. Guard failure is a **deopt**: control transfers to the interpreter at the corresponding bytecode index, with the abstract (unboxed SSA) state **reified back into `Value` cells** using a **deopt metadata map** emitted with the region (the Hölzle/Ungar dynamic-deoptimization model). The deopt map records, per side-exit, the live bytecode-level state → machine-location mapping.
4. **Classic optimizations on unboxed IR:** allocation sinking (don't build a `Value`/box that never escapes), store-to-load forwarding, guard hoisting and deduplication out of loops, **redundant refcount elimination subject to ADR-010** (below), and bounds-check elimination on proven-packed arrays.
5. **Lowering to Cranelift IR → native.** Cranelift is the Rust-native backend (it powers wasmtime): acceptable peak codegen and, crucially, **fast compile times** for online compilation. LLVM is rejected here — better peak code, wrong compile latency. Codegen targets the reference ISA unconditionally (AVX-512 / SVE2, AVX2 floor; [00-overview.md](00-overview.md) §3.3), and typed-array loops are auto-vectorized.
6. **OSR.** A loop that goes hot mid-execution is entered at its `OsrEntry` safepoint ([05-bytecode-isa.md](05-bytecode-isa.md)) **without unwinding**, by translating the live interpreter/baseline frame into the optimized frame layout (the shared call ABI makes this an in-place tier swap).

---

## 13.3 Refcount & GC interaction — ADR-010

Compiled code participates in refcounting and cooperates with the cycle collector **at safepoints only** ([04-memory-gc.md](04-memory-gc.md)). Two obligations:

- **Precise stack maps** at every `SafePoint` so the cycle collector finds roots in compiled frames. Between safepoints, compiled code owns the heap exclusively.
- **Refcount-elision legality (ADR-010).** The optimizer may drop a balanced inc/dec pair **only when**:
  - (a) the object's class **provably has no reachable `__destruct`** (`!HAS_DESTRUCTOR` from the sealed class table), **or**
  - (b) the destruction point is **provably unobservable** in the region — the object does not escape and **no user code runs** between the elided `dec` and region exit.

  Otherwise the `dec` is preserved at its semantically-required point. Conservative default: **preserve**. This protects PHP's deterministic `__destruct`-at-refcount-zero guarantee ([04-memory-gc.md](04-memory-gc.md)). Hot numeric/array code touches no destructor-bearing objects, so it still elides freely; the rule only bites on object-graph code that observes destruction timing.

---

## 13.4 Endgame note

The LuaJIT-class ceiling — a hand-written assembler with linear-scan allocation and a bespoke trace optimizer for minimal compile latency — is explicitly **out of scope for v1**. Cranelift gets most of the way at a fraction of the engineering cost. The **stencil and IR boundaries are designed so a custom backend could replace Cranelift later** without touching region formation or speculation.

---

## 14. Type-driven specialization & AOT

Targeting typed 8.5 code makes the type system a **primary optimization input**, not a hint.

### 14.1 Declared types are facts
Declared parameter, return, and property types are assumed by the optimizer. Where the type is **sealed** — a `final` class, a scalar, a sealed shape ([03-heap-types.md](03-heap-types.md)) — the assumption needs **no guard**, because nothing can invalidate it. This is why, on well-typed 8.5 code, many sites are statically monomorphic and the guard is *elided rather than emitted-and-hoisted*.

### 14.2 `rphp-analyze` — whole-program inference
`rphp-analyze` is a whole-program type-inference pass (in the spirit of Psalm / the Mago analyzer) that annotates bytecode with inferred types, collapsing the set of sites Tier 2 must speculate on. **Gating (O-4):** the baseline made it *default-on*; this project ships it **opt-in first** (`analyze` feature) and promotes it toward default-on **per-module as its soundness is proven**, because real-world PHP inference is not fully sound and an over-trusted inference that feeds guard elision is a miscompile risk. Where analysis is **uncertain**, the optimizer falls back to **profile-guided speculation with guards** — never to an unguarded assumption.

### 14.3 AOT — warm-start native that **keeps a deopt path** (ADR-006)

> **Deviation from §14.** The baseline compiles fully-typed modules "straight through Cranelift with **no deopt metadata**, because there is nothing to speculate." That is **unsound** under PHP's open world. We reframe AOT as **warm-start native with minimized guards**, *not* "no deopt."

- A region may run **guard-free** only where a **closed world is provable**: sealed `final` types, **no reachable autoload edges** ([01-frontend.md](01-frontend.md) §6.1 records these), no reachable `eval` or runtime class mutation, and all callees resolved and themselves closed.
- Where that proof holds, the guards fold away and the deopt map for that region is **empty** — so the cost is zero, but the path *exists*.
- Where it does not hold (the common reality: autoloading, conditional class definition, `eval`, dynamic dispatch into unknown targets), **guards and a deopt path remain**. A miscompile with no fallback would violate correctness-first (baseline principle #5); a deopt path that is provably never taken costs nothing.
- **AOT is gradual and per-region**, not whole-program-or-nothing (the KPHP idea, made sound): the typed, closed core is AOT-native; dynamic edges fall back to the interpreter/Tier-1/Tier-2.

A module compiled this way skips interpreter warmup for its closed regions (`aot` feature, [00-overview.md](00-overview.md)).

### 14.4 Dynamic features are local slow-path triggers
`eval`, variable variables, dynamic property creation on `#[AllowDynamicProperties]` classes, and runtime class mutation are explicit slow-path triggers. Code that uses them **deoptimizes locally**; code that does not **pays nothing** for their existence. On 8.5 the common case is the typed, non-reflective one, so this is a tax you pay only when you reach for it.

---

## Deviations from base-idea.md

- **AOT keeps a deopt path (ADR-006).** §14's "AOT = no deopt metadata" is replaced by "warm-start native with minimized guards; guard-free only where closed-world is provable; a deopt path always exists (empty where proven unreachable)." Rationale: PHP is open-world (autoload, conditional class def, `eval`); an unguarded miscompile with no fallback is a correctness failure. Cost of keeping the path where proven-empty is zero.
- **Refcount-elision legality rule (ADR-010).** §13.3's "elide balanced pairs" is given an explicit soundness precondition (no reachable `__destruct`, or provably-unobservable destruction) to preserve deterministic destructor timing.
- **`rphp-analyze` opt-in first (O-4).** §14 made whole-program inference default-on; this project makes it opt-in and promotes per-module as soundness is proven, with profile-guided guarded speculation as the fallback for uncertain sites. (Tracked as open item O-4 in [decisions.md](decisions.md).)

## Open questions

- **O-4** — promotion policy for `rphp-analyze` from opt-in toward default-on (per-extension confidence thresholds).
- Deopt-map encoding density vs reconstruction speed — choose after the first Tier-2 bring-up (M3); must stay small enough that "keep the path" remains near-free for proven-empty regions.
- Inlining budget / region-size heuristics — tune against the benchmark suite ([10-testing.md](10-testing.md)) in M3.
