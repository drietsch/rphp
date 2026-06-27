# rPHP Decision Log (ADRs)

**Status:** living document
**Supersedes:** the open questions in `base-idea.md` §25, and the specific baseline decisions noted below
**Scope:** every cross-cutting decision that the per-subsystem specs depend on

This is the single index of record. Each architecture-decision record (ADR) states *Context → Decision → Rationale → Status → Affected docs*. Where an ADR changes a decision in `base-idea.md`, the affected sub-spec repeats it in its own `Deviations from base-idea.md` section, but this file is authoritative on the wording and status. `base-idea.md` itself is frozen as the historical v0.1 baseline and is **not** edited.

## How to read

- **Resolved (§25):** the five baseline open questions, now closed.
- **Deviation:** a baseline decision this project changes, with rationale.
- **Affirmed:** a baseline decision re-confirmed after scrutiny (listed compactly; no change).
- **Open:** anything still genuinely undecided, with an owner cue.

## Index

| ADR | Title | Kind | Status |
|-----|-------|------|--------|
| 001 | Tier-0 dispatch is threaded via `become` | Resolved §25.1 | Accepted |
| 002 | Cycle collection is bounded/incremental; arena reaps request cycles | Resolved §25.2 | Accepted |
| 003 | ICU native, `icu4x` subset on WASM | Resolved §25.3 | Accepted |
| 004 | Full stdlib parity is a committed goal | Resolved §25.4 / Deviation (§0.2) | Accepted |
| 005 | 16-byte cell committed; NaN-boxing stays a gated experiment | Resolved §25.5 | Accepted |
| 006 | AOT regions keep a deopt path | Deviation (§14) | Accepted |
| 007 | Lossless CST deferred; bootstrap on `mago-syntax` | Deviation (§5) | Accepted |
| 008 | Compatibility oracle is fuzzy + allowlisted, not exact | Deviation (§20) | Accepted |
| 009 | Shared-immutable data is refcount-immortal | Deviation (§17/§10) | Accepted |
| 010 | Refcount-elision legality rule | Deviation (§13.3) | Accepted |
| 011 | Three-allocator routing policy is specified | Deviation (§10) | Accepted |
| 012 | Streams/wrappers/filters, PDO+drivers, sessions are first-class | Deviation (§15/§17) | Accepted |
| 013 | Pinned nightly toolchain | Deviation (§22) | Accepted |

---

## ADR-001 — Tier-0 dispatch is threaded via `become`

**Context.** Baseline §12.1 wants tail-threaded dispatch (each opcode handler tail-calls the next) with a `loop { match }` fallback. §25.1 flags that guaranteed tail calls (`become`) are nightly-only in Rust, and that the portable `loop { match }` fallback is not reliably compiled to a threaded interpreter by LLVM (the dispatch tends to be merged back into one indirect branch, defeating per-opcode branch prediction).

**Decision.** The primary Tier-0 dispatch is **tail-threaded using `become`** on a pinned nightly toolchain (see ADR-013). The `loop { match }` core is retained as a **feature-gated fallback** (`--no-default-features` or `dispatch-portable`) for stable/other toolchains and as a correctness cross-check, held to a small constant factor of the threaded core. The interpreter is the correctness reference, not the headline speed path; hot code is expected to climb to Tier-1 copy-and-patch quickly regardless of dispatch style.

**Rationale.** The user accepts a nightly toolchain to get guaranteed tail calls, removing the central §25.1 risk. Threaded dispatch is the difference between a competitive and a mediocre interpreter, and copy-and-patch (Tier 1) further reduces how much interpreter speed matters for hot code.

**Status.** Accepted. **Affected:** `06-interpreter.md`, `00-overview.md`, `05-bytecode-isa.md` (decode shape).

## ADR-002 — Cycle collection is bounded/incremental; the arena reaps request cycles

**Context.** Refcounting leaks cycles; baseline §10.3 runs a synchronous Bacon–Rajan trial-deletion collector at safepoints. §25.2 worries about pause behavior under adversarial graphs in the long-lived server SAPI.

**Decision.** Bacon–Rajan is **bounded and incremental from day one**: a capped candidate-roots buffer plus time-sliced scanning at `SafePoint`s, with a configurable work budget per slice. Within a short-lived request the **per-request arena reaps cycles in bulk at reset** (the whole arena is dropped), so the cycle collector is effectively unnecessary for CLI and per-request lifetimes and is a no-op fast path there. It earns its keep only for long-running fibers/isolates and process-lifetime objects, where incremental collection bounds pauses.

**Rationale.** Decouples worst-case pause from graph size for the server, and exploits the arena lifecycle (the project's biggest memory lever) to skip cycle work entirely on the common path.

**Status.** Accepted. **Affected:** `04-memory-gc.md`, `09-runtime-sapi.md`.

## ADR-003 — ICU natively, `icu4x` subset on WASM

**Context.** `intl`/`mbstring` need Unicode/locale data. Full ICU is large; §25.3 flags its footprint on the WASM/browser target.

**Decision.** Native builds bind **system ICU** (the proven, complete implementation). The WASM/browser target uses a pure-Rust, `no_std`-friendly **`icu4x`** subset behind a feature flag, with a slim, configurable locale-data bundle. The `intl`/`mbstring` surface is defined against a backend trait so the two implementations are interchangeable and differential-tested for the locales we ship.

**Rationale.** Keeps full fidelity where size is free (native) and a deployable footprint where it is not (WASM), without forking the API.

**Status.** Accepted. **Affected:** `08-stdlib-ext.md`, `09-runtime-sapi.md`.

## ADR-004 — Full stdlib parity is a committed goal

**Context.** Baseline non-goal §0.2(#2) says "not 100% stdlib coverage on day one," with demand-driven coverage. §25.4 names the stdlib long tail as the dominant schedule risk and the thing that sank prior PHP reimplementations (HippyVM, Quercus, Tagua). **The user has mandated full stdlib coverage as a committed goal.**

**Decision.** **Full php-src stdlib parity is a committed end-state goal**, superseding non-goal §0.2(#2). Sequencing is still tiered by dependency and benchmark/framework demand (see `08-stdlib-ext.md`), but completeness is the target, not best-effort. Coverage is a first-class CI metric: per-extension `% functions implemented` and `% .phpt passing`, must-not-regress. The stdlib is run as a dedicated, parallelizable track alongside the engine milestones, not squeezed into them.

**Rationale.** The compatibility cliff is the real product risk; treating completeness as a goal (with a measured burn-down) rather than an aspiration is what makes framework compatibility reachable.

**Status.** Accepted. **Affected:** `08-stdlib-ext.md`, `10-testing.md`, `00-overview.md` (roadmap note).

## ADR-005 — 16-byte cell committed; NaN-boxing stays a gated experiment

**Context.** §9 chooses a 16-byte tagged cell over NaN-boxing because PHP integers are full `i64`. §25.5 keeps a NaN-boxed variant as a possible future experiment.

**Decision.** The 16-byte cell is the committed representation. A NaN-boxed / 32-bit-smallint variant remains a **feature-flagged experiment behind the sealed `rphp-value` API**, to be evaluated only if profiling shows the cell's memory bandwidth dominating a real workload. The `Value` public contract is designed so this swap touches no other crate.

**Rationale.** Affirms a well-argued baseline decision while keeping the escape hatch cheap because the representation is encapsulated.

**Status.** Accepted. **Affected:** `02-value-model.md`.

---

## ADR-006 — AOT regions keep a deopt path

**Context.** Baseline §14 describes AOT as compiling fully-typed, `final`, non-dynamic modules "straight through Cranelift with no interpreter warmup and **no deopt metadata**, because there is nothing to speculate."

**Decision.** "AOT" is reframed as **warm-start native with minimized guards**, *not* "no deopt metadata." A region may run guard-free only where a **closed world is provable** (sealed `final` types, no autoload edges reachable, no reachable `eval`/dynamic class mutation, all callees resolved and themselves closed). Anywhere that proof does not hold, guards and a deopt path remain. A deopt path is always *available*; it is merely *empty* where soundly proven unreachable.

**Rationale.** PHP is open-world: autoloading, conditional class definition, runtime class mutation, and `eval` can invalidate a "fully known" assumption after compile time. A miscompile with no fallback violates correctness-first (baseline principle #5, "no UB in safe paths" and "correctness is measured"). Keeping the deopt path costs little (metadata is small, unused guards fold away) and removes a class of unsound optimizations.

**Status.** Accepted. **Affected:** `07-jit.md`.

## ADR-007 — Lossless CST deferred; bootstrap on `mago-syntax`

**Context.** Baseline §5 specifies a dual model: a lossless rowan-style CST plus a typed AST. §5's own note offers `mago-syntax` as a drop-in front end, decision deferred to the first milestone.

**Decision.** `rphp-ast` **owns the typed-AST contract**. For M0, the front end is **`mago-syntax` behind a thin adapter** that produces `rphp-ast` nodes. The **lossless CST is deferred (non-v1)**: it is tooling value (formatter, refactor, exact round-trip) and never sits on the runtime hot path. It can be added later behind the same adapter boundary without disturbing downstream crates (HIR and below consume `rphp-ast` only).

**Rationale.** Fastest path to a running interpreter (M1) by not rebuilding a proven PHP 8.5 parser; preserves the option to own the parser later; avoids spending scarce M0 budget on a CST a runtime does not need.

**Status.** Accepted. **Affected:** `01-frontend.md`, `00-overview.md` (crate `rphp-parser` becomes an adapter in M0).

## ADR-008 — Compatibility oracle is fuzzy + allowlisted, not exact

**Context.** Baseline §20 says "divergence is a bug in rPHP by definition" for differential testing against stock PHP.

**Decision.** The oracle uses **fuzzy matching** in the style the `.phpt` corpus already uses (`EXPECTF`/`EXPECTREGEX` with `%d`, `%s`, `%f` wildcards), plus a **curated, documented divergence allowlist** for output that is legitimately environment-dependent: float formatting/precision, locale-sensitive output, error/exception message wording, hash-order edge cases, and platform-dependent values (paths, PIDs, timestamps). Each allowlist entry cites why it diverges. Outside the allowlist, divergence is a bug.

**Rationale.** Strict byte-equality would generate false failures (the corpus itself does not assume it) and would mis-train the headline compatibility metric. Fuzzy-plus-allowlist keeps the metric honest and actionable.

**Status.** Accepted. **Affected:** `10-testing.md`.

## ADR-009 — Shared-immutable data is refcount-immortal

**Context.** Baseline §17 promises share-nothing isolates with "no shared mutable state." But §10.2/§11 store interned strings, compiled bytecode, the class table, and literal constants in a process-lifetime arena shared by all isolates, and those carry a `GcHeader { refcount }`.

**Decision.** Process-lifetime shared data is **refcount-immortal**: its refcount is a saturated sentinel value that the runtime **never increments or decrements**. Copies and drops of an immortal value are no-ops on the count. The "immortal" state is a `GcHeader` flag checked on the (already necessary) refcount fast path.

**Rationale.** If isolates touched a shared refcount, "no shared mutable state" would be false — the count word itself becomes a contended, bouncing cache line, defeating isolate scaling under load. Immortality makes cross-isolate reads truly read-only. This mirrors how CRuby/CPython handle immortal/frozen objects.

**Status.** Accepted. **Affected:** `04-memory-gc.md`, `09-runtime-sapi.md`, `03-heap-types.md` (interned strings).

## ADR-010 — Refcount-elision legality rule

**Context.** Baseline §13.3 lets the optimizer "elide refcount pairs it can prove balanced within a region." PHP guarantees `__destruct` runs at the moment a refcount reaches zero; eliding or reordering refcount operations can change *when* (or whether) a destructor fires within a region.

**Decision.** The JIT may drop a balanced inc/dec pair **only when** either (a) the object's class **provably has no reachable `__destruct`** (statically, via the sealed class table), or (b) the destruction point is **provably unobservable** within the region (the object does not escape and no user code runs between the elided dec and the region exit). Otherwise the dec is preserved at its semantically-required point. The rule is stated as an explicit precondition the optimizer pass must check, with the conservative default being *preserve*.

**Rationale.** Deterministic destructor timing is observable PHP semantics that real code depends on; an unsound elision is a correctness bug, not just a perf detail. Most hot numeric/array code touches no objects with destructors, so the common case still elides freely.

**Status.** Accepted. **Affected:** `07-jit.md`, `04-memory-gc.md` (destructor-timing guarantee).

## ADR-011 — Three-allocator routing policy is specified

**Context.** Baseline §10 names three allocators — per-request bump arena (§10.1), size-classed slab (§10.4), and `mimalloc` for the process arena and large objects — but does not specify which allocations go where, who frees what, or how they interact with COW lifetimes and `memory_limit`.

**Decision.** Routing is specified explicitly in `04-memory-gc.md`. Summary: short-lived, request-scoped allocations bump the **request arena**; high-frequency fixed-size kinds (cells, array entries, small strings) come from **size-classed slabs** backed by the arena so they can be both individually refcount-freed *and* bulk-reset; **large** objects and **process-lifetime** data use **`mimalloc`**. All three count against a single per-isolate `memory_limit` accounting hook. COW-extended lifetimes that escape a frame keep their backing alive via refcount until the arena resets.

**Rationale.** The baseline lists ingredients without a recipe; an explicit ownership/routing model is required to implement memory correctly and to enforce limits uniformly.

**Status.** Accepted. **Affected:** `04-memory-gc.md`, `03-heap-types.md`.

## ADR-012 — Streams/wrappers/filters, PDO+drivers, sessions are first-class

**Context.** The baseline scopes the M4 milestone as "real request throughput" but never specifies PHP streams (`php://`, custom wrappers, stream filters), the PDO/driver database layer, or sessions — all of which any framework request depends on.

**Decision.** These are treated as **first-class subsystems** with explicit design in `08-stdlib-ext.md` (streams/wrappers/filters API and the native filter set; PDO core + at least one driver — PostgreSQL and MySQL prioritized; session handler interface and default backends) and runtime hooks in `09-runtime-sapi.md` (async stream I/O via the reactor; per-request session lifecycle within the isolate/arena model).

**Rationale.** "Framework throughput parity" is unreachable without them; leaving them implicit hides a large, schedule-dominating chunk of work.

**Status.** Accepted. **Affected:** `08-stdlib-ext.md`, `09-runtime-sapi.md`, `10-testing.md`.

## ADR-013 — Pinned nightly toolchain

**Context.** Baseline §22 says the workspace "builds with stable Rust; an MSRV is pinned and tested." ADR-001 requires `become` (guaranteed tail calls), which is nightly-only.

**Decision.** The workspace pins a **specific nightly toolchain** (via `rust-toolchain.toml`) to enable `become` and any other nightly features the core needs (e.g. `no_std` niceties, target features). "MSRV" becomes "pinned nightly version," tested in CI and bumped deliberately. The `loop { match }` dispatch fallback (ADR-001) plus a `stable`-buildable feature subset keep a stable build path available for users who cannot take nightly, at a measured performance cost.

**Rationale.** The user accepts nightly to unlock the threaded interpreter. Pinning makes "nightly" reproducible rather than a moving target, preserving build determinism.

**Status.** Accepted. **Affected:** `00-overview.md`, `06-interpreter.md`.

---

## Affirmed baseline decisions (re-confirmed, unchanged)

These were scrutinized and kept as the baseline states them; the sub-specs deepen rather than change them.

- **16-byte tagged cell** over NaN-boxing (§9) — see ADR-005.
- **Register ISA** over a stack machine (§7).
- **Cranelift** over LLVM for the optimizing tier; **copy-and-patch** for Tier 1 (§13).
- **Region-based** JIT over pure trace/method, per HHVM PLDI'18 (§13.2).
- **Sealed-objects-as-structs** with hidden classes and unboxed typed scalar slots (§11.3) — the central performance bet.
- **Per-request arena** with bulk reset (§10.1).
- **Share-nothing isolates** (§17) — strengthened by ADR-009.
- **`.phpt` pass-rate** as the north-star compatibility metric (§20) — refined by ADR-008.

## Open decisions (still genuinely undecided)

| # | Question | Recommendation (not yet binding) | Owner cue |
|---|----------|----------------------------------|-----------|
| O-1 | Own the lexer (byte-level, §4) or reuse `mago-syntax`'s lexer in M0? | Reuse mago's in M0; build the owned byte-level lexer when SIMD lexing (§4) becomes a measured win. | resolved-in `01-frontend.md` after a lexing micro-benchmark |
| O-2 | Default DB driver beyond PG/MySQL (e.g. SQLite for tests)? | Add SQLite early purely for hermetic test fixtures. | `08-stdlib-ext.md` |
| O-3 | Zend C ABI shim — ever, or never? | Keep as a research track (baseline §16); revisit after M5. | post-M5 |
| O-4 | Whole-program analysis (`rphp-analyze`) default-on vs opt-in given its cost/soundness limits? | Opt-in at first; promote to default-on per-extension as soundness is proven. | `07-jit.md` |

*Resolve open items here and cross-link the doc that implements them.*
