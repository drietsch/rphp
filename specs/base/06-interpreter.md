# 06 — Tier 0: the Interpreter

**Status:** stable
**Source sections:** `base-idea.md` §12 (Tier 0), with §7.1/§7.4 (IC slots, profiling block) and §13 (the tiers it feeds)
**Reads with:** [05-bytecode-isa.md](05-bytecode-isa.md) (ISA, IC slots, frame/call ABI), [07-jit.md](07-jit.md) (deopt target, profile hand-off), [02-value-model.md](02-value-model.md) (`Value` reify), [04-memory-gc.md](04-memory-gc.md) (safepoints), [00-overview.md](00-overview.md) (toolchain/flags), [decisions.md](decisions.md) (ADR-001, ADR-013)

`rphp-runtime` is a register-bytecode interpreter and, more importantly, **the correctness reference** for the whole engine. Its `#![no_std]` core executes the [05-bytecode-isa.md](05-bytecode-isa.md) ISA directly, resolves and caches polymorphic operations in IC slots, and accumulates the type feedback the JITs consume. Every observable effect a JIT tier produces must match what this interpreter would have produced — because the interpreter is exactly where a deopt lands ([07-jit.md](07-jit.md)).

---

## 12.0 Role — reference semantics, deopt target, `no_std` core

The interpreter is the one component that implements *all* of PHP 8.5, without speculation and without a fast-path it cannot fall back from. Three roles follow:

1. **Correctness oracle.** The JIT tiers are optimizations *of* the interpreter; the `.phpt`/differential suite ([10-testing.md](10-testing.md)) is run against Tier 0 first, and a passing interpreter is the precondition for trusting any compiled output.
2. **Deopt landing pad.** Tier-2 guard failure transfers control back here, at a specific bytecode index, with abstract register state reified into `Value` cells (§12.4). The interpreter must be re-enterable mid-function for this to work.
3. **The `--no-default-features` engine.** With both JIT crates compiled out, `rphp-runtime` alone is a complete (if slower) PHP. It links nothing from `rphp-jit-baseline`/`rphp-jit-opt` ([00-overview.md](00-overview.md) §2.1, contract 2).

The core is `#![no_std]` and allocator-pluggable, so the same interpreter runs the CLI, a server isolate, and the WASM SAPI where it is the *only* execution tier above copy-and-patch.

---

## 12.1 Dispatch — `become`-threaded (ADR-001)

The **primary** dispatch path is **tail-threaded** on the pinned nightly toolchain ([00-overview.md](00-overview.md) §3.1, ADR-013). Each opcode is a handler that does its work, decodes the next instruction, and **tail-calls** the next handler via Rust's guaranteed-tail-call `become`. The hot VM state — instruction pointer, frame base, stack top, and a context pointer to the IC table / constant pool / profile block — is **threaded through the handler's arguments**, so the register allocator pins it to fixed registers across the whole handler chain. This is *context threading* (Berndl, Vitale, Zaleski & Brown, CGO 2005) expressed in safe-ish Rust, and on a tail-call-guaranteeing backend it lowers to the same machine code as a classic computed-`goto` threaded interpreter (Bell 1973; Ertl & Gregg 2003).

```rust
/// Hot VM state is *arguments*, not a struct field — kept in registers across handlers.
/// `Dispatch` is the per-opcode handler table; one entry per primary opcode.
type Handler = fn(pc: *const u8, fp: *mut Value, sp: *mut Value, cx: &mut VmCtx) -> Outcome;

#[inline(never)]                       // distinct i-cache region + stable regalloc per handler
fn op_add(pc: *const u8, fp: *mut Value, sp: *mut Value, cx: &mut VmCtx) -> Outcome {
    let (dst, a, b) = decode3(pc);                 // r[dst] = r[a] + r[b]
    let ic = cx.ic_slot(pc);                       // this instruction's mutable IC slot
    // fast int/float path under the IC guard; slow path resolves & records feedback…
    let next = unsafe { pc.add(LEN_ADD) };
    become DISPATCH[opcode(next) as usize](next, fp, sp, cx)  // guaranteed tail call
}
```

Handlers are `#[inline(never)]`. That is deliberate, not incidental: it keeps each handler a bounded, hot i-cache region; it stabilizes per-handler register allocation (no spill churn from a giant merged body); and it gives every dispatch its **own** indirect-branch site at the handler tail, which is the entire performance point below.

**Why the portable fallback alone is insufficient.** A `loop { match opcode { … } }` core is the natural stable-Rust shape, and rPHP keeps it — feature-gated as `dispatch-portable` ([00-overview.md](00-overview.md) §3.2) and used as a correctness cross-check. But LLVM reliably **merges the per-arm dispatch back into a single indirect branch** at the bottom of the loop, so every opcode returns to *one* shared branch site. A single site means the branch-target buffer cannot learn the per-opcode (and bytecode-position) correlations that real PHP exhibits, and the indirect-branch mispredict — tens of cycles — lands on every instruction boundary. Threaded dispatch spreads the indirect branch across one site *per handler tail*, restoring that history and recovering the prediction (Ertl & Gregg 2003 quantify exactly this). This is the §25.1 risk — guaranteed tail calls being nightly-only — which **ADR-001 resolves** by accepting the pinned nightly and making `become` the committed path. The fallback is held to a small constant factor of the threaded core; because hot code climbs to Tier-1 copy-and-patch quickly ([07-jit.md](07-jit.md)), the interpreter's dispatch style bounds, but does not dominate, steady-state performance.

The same threaded approach now powers the fastest portable interpreters in the wild (the `[[musttail]]` protobuf parser, Haberman 2021; Wasm/Luau-style dispatchers); rPHP applies it to a register ISA where each handler also does the IC read below.

**Breaking the chain.** Most handlers `become` their successor and never return, so the threaded chain is one long tail-call sequence with no growing call stack. The chain unwinds — an actual `return Outcome` instead of a `become` — only for **non-local control**: a normal `Ret` past the outermost interpreter frame (yield control to the host), an **exception** that must run table-driven unwinding (`Throw`/`EnterTry`/`Catch`, §7.3), or a **fiber suspension** ([09-runtime-sapi.md](09-runtime-sapi.md)) that parks the whole VM-state tuple to be resumed later. `Outcome` is that small return enum (`Continue` is never produced — it is encoded as the tail call itself). Exception dispatch consults the function's exception-region table to find the handler `pc`, reifies the in-flight throwable into the destination register, and re-enters the threaded loop at the catch target — so even unwinding lands back on the same dispatch mechanism rather than a separate interpreter.

---

## 12.2 Frames and the call ABI

A single **contiguous VM stack** holds all frames. A frame is a **register window** into that stack: the callee's locals/temporaries are a slice `fp[0..frame_size]`. The caller lays arguments directly into the callee's *incoming* register range, so an ordinary call **copies no arguments** through an intermediate buffer — it advances `fp` and `become`s the callee's entry handler. `Ret` restores the caller's `fp`/`pc` from the frame's saved header and tail-calls back.

The canonical layout (frame header fields, argument/return register ranges, spill discipline, exception-region binding) is defined once in [05-bytecode-isa.md](05-bytecode-isa.md) and is **not** re-specified here. The load-bearing property for this document: **the call ABI is shared verbatim by Tier 0, Tier 1, and Tier 2.** Interpreted and compiled frames interleave on the one stack, so OSR can enter the optimizer mid-loop at an `OsrEntry` safepoint, and deopt can replace a compiled frame with an interpreter frame **in place**, without unwinding (§12.4).

---

## 12.3 Inline caches and profiling (§7.1 / §7.4)

Every polymorphic opcode — arithmetic, comparison, property access, method call, array access — carries an **IC slot index** ([05-bytecode-isa.md](05-bytecode-isa.md)); the slot is mutable per-instruction state living in the function's profiling block (§7.4). First execution **resolves** the operation — which `Add` variant, which property offset on which shape, which method target — caches the result behind a guard, and records the operand types. Subsequent executions check a cheap guard and reuse the cached handler.

```rust
/// One IC slot: 1:1 with a polymorphic instruction. Guard + resolved target + feedback.
#[repr(C)]
pub struct IcSlot {
    guard:    GuardKey,       // what `target` is valid for (checked every execution)
    target:   CacheEntry,     // the resolved fast operation on a guard hit
    feedback: TypeFeedback,   // histogram accumulated for the JIT (read at tier-up)
}

#[repr(C)] pub union GuardKey {
    shape:     ShapeId,       // Prop*: cached object shape  (03-heap-types §11.3)
    type_pair: [TypeTag; 2],  // Add/Cmp: cached operand tag pair (02-value-model)
    callee:    FnId,          // CallMethod: cached resolved target
    bits:      u64,
}

#[repr(C)] pub union CacheEntry {
    prop_offset: u32,         // constant slot offset, loaded on a Prop* hit
    handler:     Handler,     // type-specialized op handler
    code:        CodeRef,     // resolved call entry
}

/// Compact, fixed-size type feedback — the profile the JIT specializes from.
pub struct TypeFeedback {
    seen:   TypeMask,         // bitset of operand tags ever observed here
    bucket: [ShapeId; 4],     // up to 4 shapes / call targets, LRU
    count:  [u16; 4],         // saturating per-bucket hit counts (an array shape too)
    flags:  FeedbackFlags,    // MEGAMORPHIC (overflowed 4), SAW_NULL, SAW_INT_OVERFLOW…
}
```

The guarded fast path is the same shape for every IC; property load is representative:

```rust
#[inline(always)]
fn prop_get(obj: &PhpObject, ic: &mut IcSlot) -> Value {
    if unsafe { ic.guard.shape } == obj.shape {          // monomorphic guard: one compare
        unsafe { obj.load_slot(ic.target.prop_offset) }  // constant-offset typed load
    } else {
        prop_get_miss(obj, ic)   // resolve offset on obj.shape, fill slot, bump feedback
    }
}
```

A **miss** re-resolves, rewrites `guard`/`target`, and folds the observed type/shape into `feedback` — promoting the site to polymorphic (it may hold several buckets) and finally to `MEGAMORPHIC` once it overflows, which tells the JIT not to speculate there. The feedback histogram is *the* substrate the optimizer reads: an `Add` that only ever `seen` two `Int`s becomes unboxed `i64` arithmetic under a tag guard; a `PropGet` with one `bucket` shape becomes a guarded offset load; an array site that stayed packed-int becomes a typed `[i64]` view ([07-jit.md](07-jit.md)).

**Edge profiling.** `rphp-profile` also counts **loop back-edges** (`LoopHeader`/`ProfileEdge`, §7.3) and **branch directions** at conditional jumps, so the optimizer knows trip counts and which arm is hot for region formation. When a function or loop crosses its hotness threshold, the assembled profile — IC histograms plus edge weights — is handed to the JIT, so compilation **starts already knowing the likely types** rather than re-discovering them.

Back-edges carry an implicit **safepoint**: the back-edge handler does a single relaxed load of the per-isolate interrupt flag with a predicted-not-taken branch, which is where `max_execution_time` preemption, the cycle collector, and (when the software-counter path is active) the tier-up check are admitted ([04-memory-gc.md](04-memory-gc.md), [10-testing.md](10-testing.md) §security). The cost is one load per loop iteration on the cold-predicted path; it is the only tax the dispatch loop pays for cooperative scheduling, and it is exactly the spot a deopt or OSR may also fire (§12.5).

---

## 12.4 PMU-driven hotness (an optimization over counters)

Software back-edge/call counters are the portable default. On supporting hardware, hotness detection can instead run off the CPU **performance-monitoring unit**: PEBS-sampled retired-instruction events attribute time to exact bytecode/compiled addresses, and the **LBR** (last-branch record) reconstructs recent call/branch history for free. This gives **near-zero-overhead tiering decisions** and accurate hot-path attribution **without instrumenting the bytecode** — no counter increments on the back-edge at all. It is strictly an optimization over software counters, **selected at runtime** by capability probe (and unavailable on much WASM/virtualized hardware, where the counter path stands in). The two paths feed the *same* threshold logic and the *same* profile hand-off; only the source of "this is hot" differs.

---

## 12.5 The deopt-entry contract (§13.2.3)

The interpreter is the **bit-exact** fallback target for deoptimization ([07-jit.md](07-jit.md)). When a Tier-2 guard fails, control transfers here at the bytecode index recorded in the region's **deopt metadata map**, and the optimizer's abstract machine state — unboxed SSA registers, sunk allocations, elided refcounts — is **reified back into `Value` cells** in the frame's register window ([02-value-model.md](02-value-model.md) §9.3 makes the materialization a branchless two-word store per cell). This is the Hölzle–Chambers–Ungar dynamic-deoptimization model (PLDI 1992). Requirements the interpreter therefore guarantees:

- **Re-enterability at any deopt point.** Execution can begin at any bytecode index named by a deopt map or `OsrEntry`, not only at function entry — the dispatch loop is index-addressed, frame state is fully described by the register window plus saved header.
- **Deopt and the cycle collector run only at safepoints.** `SafePoint` and loop back-edges are the *only* places abstract state is guaranteed consistent and the GC may traverse ([04-memory-gc.md](04-memory-gc.md)); between them the executing tier owns the heap exclusively.
- **The observable-equivalence invariant.** For any input, interpreter and any optimized path produce identical observable behavior — value, warnings/exceptions, and destructor *timing* ([04-memory-gc.md](04-memory-gc.md) ADR-010). This is not asserted, it is *tested*: the **deopt-stress mode** ([10-testing.md](10-testing.md)) forces guard failure at every safepoint and diffs the result against an interpreter-only run, proving the fallback is bit-identical to the speculated path.

---

## Deviations from base-idea.md

- **§12.1 dispatch is committed to `become` on pinned nightly (ADR-001 / ADR-013).** The baseline presented tail-threaded dispatch with a portable fallback but flagged guaranteed tail calls as a §25.1 *risk*. That risk is now resolved: `become` is the **primary** path on a pinned nightly toolchain, and `loop { match }` is an explicitly **feature-gated** (`dispatch-portable`) correctness cross-check held to a small constant factor — with the LLVM branch-merging rationale spelled out as the reason the fallback cannot stand alone.
- Otherwise §12 is **affirmed and detailed**: the frame/register-window ABI (deferred canonically to [05-bytecode-isa.md](05-bytecode-isa.md)), the IC-slot + type-feedback layout, edge profiling, PMU hotness, and the deopt-entry contract are elaborations, not changes.

## Open questions

- Hotness thresholds and counter widths (back-edge/call) — tune in M1/M2 against the benchmark suite; co-tune with the software↔PMU crossover so both paths trip at the same effective heat.
- IC megamorphic cutoff (the 4-bucket `TypeFeedback`) — is 4 the right shape/target fan-out before giving up on speculation? Measure against framework-class polymorphism in M3.
- PMU availability and attribution fidelity on virtualized/cloud hosts (PEBS/LBR may be disabled or coarsely sampled) — validate the counter-fallback parity under M4 server load.
- Whether selectively inlining the tiniest handlers (`Mov`, `LoadConst`) beats strict `#[inline(never)]` on net i-cache pressure — decide empirically in M1.
