# 04 — Memory Management & Garbage Collection

**Status:** stable
**Source sections:** `base-idea.md` §10 (memory management), with §11 header details
**Reads with:** [02-value-model.md](02-value-model.md), [03-heap-types.md](03-heap-types.md), [07-jit.md](07-jit.md) (refcount elision), [09-runtime-sapi.md](09-runtime-sapi.md) (isolates), [decisions.md](decisions.md) (ADR-002, ADR-009, ADR-010, ADR-011)

`rphp-gc` is `#![no_std]` and takes the allocator as a **generic parameter**, so WASM and embedders supply their own. The design exploits PHP's request lifecycle: allocate freely, end, free everything at once.

---

## The GC header

Every heap object starts with:
```rust
#[repr(C)]
pub struct GcHeader {
    refcount: u32,
    kind:     HeapKind,   // Str | Array | Object | Closure | Ref | …
    color:    GcColor,    // Black | Gray | White | Purple  (Bacon–Rajan)
    flags:    u16,        // IMMORTAL, BUFFERED (in roots buffer), HAS_DESTRUCTOR, …
}
```
`HAS_DESTRUCTOR` is set at allocation from the sealed class table ([03-heap-types.md](03-heap-types.md)) and drives ADR-010. `IMMORTAL` drives ADR-009.

---

## 10.1 Per-request arena (bump + bulk reset)

Each isolate owns a **request-scoped arena**. Most request allocations bump a pointer; at request end the arena **resets in O(1)** — no per-object frees. This is the single biggest server-throughput lever and mirrors why Zend's request shutdown is cheap.

Long-lived allocations (compiled bytecode cache, interned symbols, the class table) live in a **separate process-lifetime arena** and are **immortal** (ADR-009).

---

## 10.4 + ADR-011 — Three-allocator routing policy

The baseline names three allocators but not the routing. Specified:

| Allocation kind | Allocator | Freed by | Notes |
|-----------------|-----------|----------|-------|
| High-frequency fixed-size: `Value`-adjacent cells, array entries, small strings, closures | **size-classed slab**, arena-backed | refcount → returned to its size-class free-list; **also** reclaimed wholesale at arena reset | both individually freeable *and* bulk-resettable |
| General request-scoped, variable-size, short-lived | **bump arena** | arena reset (bulk) | refcount-zero before reset just abandons the bytes; high-water stays low because slabs absorb the churny kinds |
| Large objects (big strings/arrays above a threshold) | **`mimalloc`** | refcount → `free` | not arena-bound; would bloat arena high-water |
| Process-lifetime shared (bytecode, interner, class table) | **`mimalloc`** (process arena) | never (immortal) | ADR-009 |

**Rule of thumb:** churn that benefits from individual reuse → slab; bulk-dies-with-request → arena; too big or too long-lived for the arena → mimalloc. All four paths funnel through one **per-isolate accounting hook** that enforces `memory_limit` / `max_memory_limit` ([10-testing.md](10-testing.md) §security) uniformly — a single counter incremented on alloc, decremented on free, checked against the ceiling before every grow.

**COW ↔ arena interaction.** A COW container that **escapes** a frame (returned, stored in a longer-lived structure) keeps its backing alive by **refcount** until the arena resets; it is not freed early just because the frame ended. Within a request that is fine — the arena bounds the worst case. Slab-backed entries of an escaped array are likewise retained by refcount. Nothing escapes a *request* except via the process-lifetime arena (immortal) or explicit cross-isolate message-passing ([09-runtime-sapi.md](09-runtime-sapi.md)), so arena reset is always safe.

---

## 10.2 + ADR-009 — Reference counting & immortality

Refcounting gives PHP its **deterministic destructor timing** — `__destruct` at refcount-zero, not at a future GC pause — which real code depends on. Most allocations are freed by refcount **before** the arena ever resets, keeping arena high-water low.

The copy/drop fast path (from [02-value-model.md](02-value-model.md)):
```
copy: if heap && !IMMORTAL: refcount_inc
drop: if heap && !IMMORTAL: if --refcount == 0 { destruct?; free }
```

### Immortality (ADR-009)
Process-lifetime shared data (interned strings, compiled bytecode, class table, literal constants) is **immortal**: `IMMORTAL` set, refcount a saturated sentinel **never read-modified-written**. Cross-isolate reads of immortal data touch **no shared cache line** — this is what makes "no shared mutable state" ([09-runtime-sapi.md](09-runtime-sapi.md)) actually true; without it the refcount word would be a contended, bouncing line that defeats isolate scaling. Copies/drops of immortal values are no-ops on the count.

### Destructor-timing guarantee (ADR-010)
The runtime guarantees `__destruct` runs at the point the last reference drops, for any object whose class has a destructor (`HAS_DESTRUCTOR`). The JIT may only elide/reorder refcount ops subject to the **legality rule** below.

---

## 10.3 + ADR-002 — Cycle collection (bounded Bacon–Rajan)

Refcounting leaks cycles. rPHP runs a **trial-deletion / synchronous-by-default but bounded** cycle collector (the Bacon–Rajan family, as Zend's `gc_collect_cycles` is).

- **Candidate roots:** objects whose refcount is decremented but stays nonzero are colored `Purple` and buffered (`BUFFERED` flag) in a **capped roots buffer**.
- **When:** scanning runs **only at `SafePoint`s** ([05-bytecode-isa.md](05-bytecode-isa.md)). The collector is the **only** component allowed to traverse object graphs, so the interpreter and JIT never race it.
- **Bounded/incremental (ADR-002):** scanning is **time-sliced** with a configurable work budget per slice and a roots-buffer cap, so worst-case pause is decoupled from graph size — required for the long-lived server SAPI under adversarial graphs.
- **The arena shortcut:** within a short request the **arena reset reclaims cycles in bulk** (the whole arena drops), so the cycle collector is effectively a **no-op fast path for CLI/per-request** lifetimes. It earns its keep only for long-running fibers/isolates and process-lifetime objects. The roots buffer that would have grown during a request is simply discarded at reset.

### Trial deletion (the algorithm, briefly)
For buffered roots: (1) **mark gray** — decrement internal refcounts along edges; (2) **scan** — objects still >0 are live, re-increment (mark black); 0 means only-internally-referenced (mark white); (3) **collect white** — free, running destructors. Destructors run at a safepoint, can resurrect, and are handled by re-checking refcounts post-destruct (matching Zend's care here).

---

## Safepoints — the cooperation contract

`SafePoint` (and loop back-edges, which carry an implicit safepoint) are the **only** places where the cycle collector and deopt may run. Between safepoints, the interpreter and compiled code own the heap exclusively.

- The optimizer keeps **precise stack maps** at safepoints so the collector finds roots in compiled frames ([07-jit.md](07-jit.md) §13.3).
- `max_execution_time` is enforced at safepoints via an interrupt flag set by a timer thread ([10-testing.md](10-testing.md) §security), so even hot JITed loops honor it (back-edges carry safepoints). The per-back-edge cost is a single relaxed load of the flag with a predicted-not-taken branch.

### ADR-010 — Refcount-elision legality rule (the precondition)
The Tier-2 optimizer may drop a balanced inc/dec pair **only when**:
- (a) the object's class **provably has no reachable `__destruct`** (`!HAS_DESTRUCTOR`, statically from the sealed class table), **or**
- (b) the destruction point is **provably unobservable** in the region: the object does not escape, and **no user code runs** between the elided `dec` and region exit.

Otherwise the `dec` is preserved at its semantically-required point. Conservative default: **preserve**. Hot numeric/array code touches no destructor-bearing objects, so it still elides freely.

---

## 10.4 — Allocator backing & placement

- **Global allocator:** `mimalloc` for the process arena and large objects.
- **Huge pages:** the request arena and the JIT code cache back onto **2 MiB / 1 GiB huge pages** to cut TLB misses on the hot bump path and on compiled-code execution.
- **NUMA:** isolate arenas are placed **NUMA-locally** to the worker thread that owns them ([09-runtime-sapi.md](09-runtime-sapi.md)).
- **Slab:** size-classed for cells, array entries, small strings, closures — kills allocator overhead and fragmentation on the churny kinds.

---

## Deviations from base-idea.md

- **Allocator routing made explicit (ADR-011).** §10 listed three allocators; this doc specifies which kinds go where, who frees what, and how `memory_limit` counts across all of them.
- **Immortality for shared data (ADR-009).** New invariant: process-lifetime shared data never mutates its refcount, so cross-isolate reads are contention-free. Closes a gap where §17's "no shared mutable state" was violated by the refcount word.
- **Cycle collection is bounded/incremental and arena-shortcutted (ADR-002).** §10.3 was synchronous-only; this bounds server pauses and makes per-request cycle work a no-op.
- **Refcount-elision legality rule (ADR-010).** §13.3's "elide balanced pairs" is given an explicit soundness precondition to preserve deterministic `__destruct` timing.

## Open questions

- Slab size-class boundaries and per-class free-list caps — tune against allocation histograms in M1.
- Whether incremental cycle collection needs a write barrier for long-lived isolates (snapshot-at-safepoint may suffice given the safepoint contract) — revisit in M4 under server load (links to §25.2).
