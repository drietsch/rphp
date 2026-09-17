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
- **Roadmap:** a decision that fills a spec hole or sequences the Symfony ladder (roadmap 2026-09-17). Proposed until the workstream that implements it lands, then Accepted.

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
| 014 | Dependency inversion: `rphp-runtime` is the engine API; `Host` removed; `Ctx = &mut Interp`; SAPIs depend on `rphp-embed` only | Deviation (§2) | Proposed |
| 015 | Interim heap is safe `Rc`: `Value::{Ref, Uninit, Resource}`, closures are objects, slot-layout objects, thin `Str`; `rphp-heap`/`rphp-gc` deferred | Deviation (§10/§11) | Proposed |
| 016 | Call ABI = `InitFCall*/Send*/DoCall` with pending-call records; fused `CallFn` later | Deviation (§12.2) | Proposed |
| 017 | Destructors run from an op-boundary queue | Roadmap | Proposed |
| 018 | Re-entrant interpreter (`run_until`); generators are parked frames; Fibers deferred to an `unsafe`-isolating crate | Deviation (§17.2) | Proposed |
| 019 | `NEEDS_SYMTAB` frames; `$GLOBALS` snapshot with write-through ops | Roadmap | Proposed |
| 020 | Units, include/eval, `CompileHook`, autoload protocol, late-bound names with generation-stamped ICs | Roadmap | Proposed |
| 021 | Native class model: builder, `Payload::Native`, native inheritance, object handler table | Roadmap | Proposed |
| 022 | Diagnostics channel; exceptions remain the only non-local control channel | Roadmap | Proposed |
| 023 | Monotonic object ids, per-`Interp` resource ids; allowlisted | Roadmap | Proposed |
| 024 | Interim synchronous weak-registry cycle collector; `memory_get_usage` estimate | Deviation (§10.3) | Proposed |
| 025 | `mago-syntax` is the production parser, pinned + conformance pass; supersedes ADR-007's bootstrap reading; closes O-1 | Deviation (§5) | Proposed |
| 026 | HIR shares the AST vocabulary (newtype + canonical subset) | Deviation (§6) | Proposed |
| 027 | Declaration metadata is a bytecode contract (Reflection round-trips it) | Roadmap | Proposed |
| 028 | Nightly pin tracks mago's MSRV; bump procedure | Deviation (§22) | Proposed |
| 029 | Two-way `php -l` parity gate; owned `rphp-tokenizer` is `ext/tokenizer` | Deviation (§4) | Proposed |
| 030 | Arginfo/constants/ini generated from a committed PHP 8.5 manifest; names in bytecode; runtime arity/type errors | Deviation (§15) | Proposed |
| 031 | Extension crate layout: `crates/ext/rphp-ext-<name>`, `rphp-stdlib` bundle, `catalog.toml` | Roadmap | Proposed |
| 032 | Backend choices: mbstring/iconv pure Rust (narrows ADR-003), libxml2/libxslt, intl polyfilled, curl deferred, date on `jiff` + timelib port | Deviation (§15/§25.3) | Proposed |
| 033 | SAPI order CLI → `rphp -S` → FastCGI; thread-per-connection; `Request`/`OutputSink`/`Response` contract | Deviation (§17/§18) | Proposed |
| 034 | Differential oracle: pinned `php -n` ini, byte-exact stdout, closed divergence categories, ladder fixtures, ratcheting `.phpt` baselines | Deviation (§20) | Proposed |
| 035 | PDO deferred until L7 is green; SQLite first; pure-Rust drivers by default | Roadmap | Proposed |

---

## ADR-001 — Tier-0 dispatch is threaded via `become`

**Context.** Baseline §12.1 wants tail-threaded dispatch (each opcode handler tail-calls the next) with a `loop { match }` fallback. §25.1 flags that guaranteed tail calls (`become`) are nightly-only in Rust, and that the portable `loop { match }` fallback is not reliably compiled to a threaded interpreter by LLVM (the dispatch tends to be merged back into one indirect branch, defeating per-opcode branch prediction).

**Decision.** The primary Tier-0 dispatch is **tail-threaded using `become`** on a pinned nightly toolchain (see ADR-013). The `loop { match }` core is retained as a **feature-gated fallback** (`--no-default-features` or `dispatch-portable`) for stable/other toolchains and as a correctness cross-check, held to a small constant factor of the threaded core. The interpreter is the correctness reference, not the headline speed path; hot code is expected to climb to Tier-1 copy-and-patch quickly regardless of dispatch style.

**Rationale.** The user accepts a nightly toolchain to get guaranteed tail calls, removing the central §25.1 risk. Threaded dispatch is the difference between a competitive and a mediocre interpreter, and copy-and-patch (Tier 1) further reduces how much interpreter speed matters for hot code.

**Status.** Accepted. **Affected:** `06-interpreter.md`, `00-overview.md`, `05-bytecode-isa.md` (decode shape).
*Narrowed by ADR-028:* the portable `loop { match }` core is the **default build today**; `become` dispatch remains the intended Tier-0 headline path behind the `tailcall-dispatch` feature.

## ADR-002 — Cycle collection is bounded/incremental; the arena reaps request cycles

**Context.** Refcounting leaks cycles; baseline §10.3 runs a synchronous Bacon–Rajan trial-deletion collector at safepoints. §25.2 worries about pause behavior under adversarial graphs in the long-lived server SAPI.

**Decision.** Bacon–Rajan is **bounded and incremental from day one**: a capped candidate-roots buffer plus time-sliced scanning at `SafePoint`s, with a configurable work budget per slice. Within a short-lived request the **per-request arena reaps cycles in bulk at reset** (the whole arena is dropped), so the cycle collector is effectively unnecessary for CLI and per-request lifetimes and is a no-op fast path there. It earns its keep only for long-running fibers/isolates and process-lifetime objects, where incremental collection bounds pauses.

**Rationale.** Decouples worst-case pause from graph size for the server, and exploits the arena lifecycle (the project's biggest memory lever) to skip cycle work entirely on the common path.

**Status.** Accepted. **Affected:** `04-memory-gc.md`, `09-runtime-sapi.md`.
*Interim implementation:* ADR-024 (synchronous weak-registry collector) until the arena/`GcHeader` heap lands; this ADR stays the end state.

## ADR-003 — ICU natively, `icu4x` subset on WASM

**Context.** `intl`/`mbstring` need Unicode/locale data. Full ICU is large; §25.3 flags its footprint on the WASM/browser target.

**Decision.** Native builds bind **system ICU** (the proven, complete implementation). The WASM/browser target uses a pure-Rust, `no_std`-friendly **`icu4x`** subset behind a feature flag, with a slim, configurable locale-data bundle. The `intl`/`mbstring` surface is defined against a backend trait so the two implementations are interchangeable and differential-tested for the locales we ship.

**Rationale.** Keeps full fidelity where size is free (native) and a deployable footprint where it is not (WASM), without forking the API.

**Status.** Accepted. **Affected:** `08-stdlib-ext.md`, `09-runtime-sapi.md`.
*Narrowed by ADR-032:* ICU natively / `icu4x` on WASM applies to **`intl` only**; `mbstring` and `iconv` are pure Rust.

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
*Superseded in part by ADR-025:* `mago-syntax` is the **production** parser (pinned exactly, conformance-checked), not an M0 bootstrap; the `rphp-ast` ownership boundary and the CST deferral stand.

## ADR-008 — Compatibility oracle is fuzzy + allowlisted, not exact

**Context.** Baseline §20 says "divergence is a bug in rPHP by definition" for differential testing against stock PHP.

**Decision.** The oracle uses **fuzzy matching** in the style the `.phpt` corpus already uses (`EXPECTF`/`EXPECTREGEX` with `%d`, `%s`, `%f` wildcards), plus a **curated, documented divergence allowlist** for output that is legitimately environment-dependent: float formatting/precision, locale-sensitive output, error/exception message wording, hash-order edge cases, and platform-dependent values (paths, PIDs, timestamps). Each allowlist entry cites why it diverges. Outside the allowlist, divergence is a bug.

**Rationale.** Strict byte-equality would generate false failures (the corpus itself does not assume it) and would mis-train the headline compatibility metric. Fuzzy-plus-allowlist keeps the metric honest and actionable.

**Status.** Accepted. **Affected:** `10-testing.md`.
*Narrowed by ADR-034:* stdout is compared **byte-exact by default**; the divergence categories are a **closed set**; error wording is no longer a blanket category.

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
*Narrowed by ADR-033 and ADR-035:* reactor-driven I/O is deferred behind CLI → `rphp -S` → FastCGI; PDO is deferred until L7 is green with SQLite as the first driver. First-class status is unchanged.

## ADR-013 — Pinned nightly toolchain

**Context.** Baseline §22 says the workspace "builds with stable Rust; an MSRV is pinned and tested." ADR-001 requires `become` (guaranteed tail calls), which is nightly-only.

**Decision.** The workspace pins a **specific nightly toolchain** (via `rust-toolchain.toml`) to enable `become` and any other nightly features the core needs (e.g. `no_std` niceties, target features). "MSRV" becomes "pinned nightly version," tested in CI and bumped deliberately. The `loop { match }` dispatch fallback (ADR-001) plus a `stable`-buildable feature subset keep a stable build path available for users who cannot take nightly, at a measured performance cost.

**Rationale.** The user accepts nightly to unlock the threaded interpreter. Pinning makes "nightly" reproducible rather than a moving target, preserving build determinism.

**Status.** Accepted. **Affected:** `00-overview.md`, `06-interpreter.md`.
*Narrowed by ADR-028:* the pin tracks `mago-syntax`'s MSRV and follows a fixed bump procedure.

---

## Roadmap ADRs (Symfony ladder, 2026-09-17)

ADR-014 … ADR-035 were appended as one batch from the approved Symfony-8 roadmap. They are **Proposed** and become Accepted individually as the workstream that implements each one lands (Tracks E/F/S/T of the roadmap). Where one narrows or supersedes an earlier ADR, both records say so.

## ADR-014 — Dependency inversion: `rphp-runtime` is the engine API; `rphp-stdlib` and extension crates depend on it; `Host` removed; `Ctx = &mut Interp`; SAPIs depend on `rphp-embed` only

**Context.** Today `rphp-runtime` depends on `rphp-stdlib`, and the stdlib reaches back into the engine through a `Host` trait (`crates/rphp-stdlib/src/lib.rs`: `Host`, `BufHost`, `NativeError`, `NativeResult = Result<Value, NativeError>`). The compiler resolves native names at compile time (`rphp_stdlib::resolve(name)`) and bakes a `NativeId` into the bytecode. A native therefore cannot call back into the VM, throw a `Throwable` object, emit a warning through `error_reporting`/`set_error_handler`, or touch output buffers, ini, resources or the class table — all of which every rung of the Symfony ladder needs (`array_map` with a closure that throws, `spl_autoload_register`, `ob_start` callbacks, `set_error_handler` turning warnings into `ErrorException`). `rphp-sapi-cli` currently depends on every front-end crate directly, so 00-overview.md §2.1 contract #4 is not enforced by the crate graph.

**Decision.** The dependency direction inverts. `rphp-runtime` becomes the engine crate **below** the stdlib and owns `Interp` (units, function/class/constant tables, VM stack + frames, globals symtab, output stack, ini table, `error_reporting` + handler stacks + shutdown functions + `last_error`, per-extension state, resource table, included-files set, autoloader stack, superglobals, compile hook, object-id allocator, cwd/argv/script path/sapi kind). Natives receive `Ctx<'a>(&mut Interp)` (`Deref` to `Interp`) and return `NativeResult = Result<Value, Unwind>`; by-ref positions are dereferenced before the call and written back through the `Ref` afterwards so handler bodies stay untouched. The `Host` trait, `BufHost` and `NativeError` are **deleted**; a `Registry` builder (`function/functions/constant/class(...)`) is the only way to add natives. `rphp-stdlib` and every extension crate (ADR-031) depend on `rphp-runtime` + `rphp-ext-api`. The compiler stops depending on the stdlib: bytecode carries names, resolved at runtime (ADR-020, ADR-030). A new `rphp-embed` crate wires front end + runtime + stdlib into `Engine`/`Interp` (`run_file`, `eval`, `shutdown`; installs the compile hook, registers engine constants, seeds `$_SERVER/$argv/$argc`, runs shutdown functions, drains destructors, flushes output, renders uncaught exceptions, returns the exit code). `rphp-sapi-cli` and later SAPIs depend on `rphp-embed` **only**. Target graph: `rphp-sapi-* → rphp-embed → rphp-stdlib → crates/ext/* → rphp-runtime → rphp-bytecode → rphp-value`, with `rphp-embed → rphp-compiler → rphp-hir → rphp-parser/rphp-ast` alongside.

**Rationale.** A stdlib that cannot re-enter the interpreter cannot implement PHP: callbacks, autoload, output-buffer handlers and error handlers are all native→PHP calls. Putting the engine below the stdlib is the arrangement php-src itself has (`Zend/` below `ext/`) and is what makes the stdlib a parallel track with no engine edits per extension.

**Status.** Proposed (roadmap 2026-09-17). Turns 00-overview.md §2.1 contract #4 into a build-graph invariant. **Affected:** `00-overview.md` (§2 crate topology and dependency direction, §2.1), `08-stdlib-ext.md` (§15.1 `Ctx`/`NativeResult`, §16 `Extension`/`Registry`), `09-runtime-sapi.md` (§18 `rphp-embed` spine), `06-interpreter.md`.

## ADR-015 — Interim heap is safe `Rc`: `Value::{Ref, Uninit, Resource}`, closures are objects, slot-layout objects with a dynamic-props side map, thin `Str`; `rphp-heap`/`rphp-gc`/`GcHeader` deferred

**Context.** `02-value-model.md`, `03-heap-types.md` and `04-memory-gc.md` specify a `no_std` `rphp-heap`/`rphp-gc` with `GcHeader`, arena/slab routing (ADR-011) and hidden-class shapes. None of it exists: the current `rphp-value` is an `Rc`-based enum with a `Value::Closure(Closure)` variant, objects as name-keyed property maps, arrays without `unset` or element references, and no PHP references at all. Symfony needs references (`foreach (… as &$v)`, `&$param`, `use (&$x)`, `$GLOBALS` write-through), typed-property `Uninit`, `unset($a[$k])`, closures that are real `Closure` objects (`bind`/`bindTo`/`call`/`fromCallable`), resources for streams, and stable object ids.

**Decision.** The heap stays **safe `Rc`** through the Symfony ladder. `Value` gains `Ref(PhpRef)` (`Rc<RefCell<Value>>` with `deref()/make_ref()/assign()` and the invariant *a `Ref` never contains a `Ref`*; every kernel derefs first), `Uninit` (typed props, hooks) and `Resource(Rc<ResourceCell{id, kind, payload: RefCell<Option<Box<dyn Any>>>}>)` (closed = `None`). The `Closure` variant is **removed**: closures are objects of class `Closure` with `Payload::Closure` (func, unit, captures incl. `Ref` cells, bound `this`, `scope`, `called_scope`, `is_static`, statics). Objects become `ObjectData{class, id, slots: Vec<Value>, dyn_props: Option<Map>, payload: Payload::{None, Closure, Native(Box<dyn Any>)}, flags}` — slot layout parent-first with private shadowing, dynamic properties in a side map. Arrays gain tombstoned `unset` + compaction, `get_ref()` (the element becomes a `Ref` in place), an internal pointer and `is_packed_list()`; a packed representation is deferred behind the same API. `Str` becomes thin so `Value` stays 16 bytes (ADR-005, enforced by a size assert). `rphp-heap`, `rphp-gc` and `GcHeader` are **deferred**, not abandoned: the `Value` public API stays closed so the later swap touches no other crate — ADR-005's escape hatch applied in the other direction.

**Rationale.** Correctness-first: an `Rc` heap with the right *semantics* (refs, uninit, resources, slot objects) unblocks every ladder rung now; an arena/shape heap with the wrong semantics unblocks nothing. The one new hazard is `RefCell` double borrows under re-entrancy — rule: copy out, drop the borrow, then call the VM; no helper ever yields a guard across a VM call.

**Status.** Proposed (roadmap 2026-09-17). Defers ADR-011's allocator routing and the shape material in 03 to a later milestone; ADR-005 is affirmed. **Affected:** `02-value-model.md`, `03-heap-types.md`, `04-memory-gc.md` (interim section), `08-stdlib-ext.md` (resources).

## ADR-016 — Call ABI = `InitFCall*/Send*/DoCall` with per-frame pending-call records; zero-copy window; fused `CallFn` is a later peephole

**Context.** `05-bytecode-isa.md` §12.2 specifies a zero-copy register window with a single `CallFn d,callee,base,argc` op; the current interpreter has that op and recurses on the Rust stack per call. PHP argument passing needs more than "materialize then jump": by-reference positions must yield `Ref` cells *before* the callee runs, spread/named/default/variadic arguments resolve against the callee's `ParamDef`s, evaluation order is observable (`undefined_fn(print("never"))` must fault before its arguments are evaluated), and first-class callable syntax `f(...)` needs a call *set up but not executed*.

**Decision.** Calls are a Zend-style sequence. `InitFCall{name, ns_fallback, ic}` / `InitDynCall` / `InitMethodCall` / `InitStaticCall{ClassRef::{Named, Self_, Parent, Static}}` / `InitNew` resolve the target (autoloading where PHP does, ADR-020) and push a **pending-call record** on the current frame; `SendVal/SendVar/SendRefElem/SendRefProp/SendUnpack/SendNamed` place arguments into the contiguous register stack (zero-copy window preserved); `DoCall` binds parameters (`RecvInit{param, init}` with defaults as a const or a 0-arg thunk, `RecvVariadic`), applies `strict_types` coercion (caller's flag for parameters, callee's for returns) and activates the frame; `Ret/RetRef` return. The pending record is what `f(...)` captures into a `Closure`. `Frame{func: Rc<FuncRt>, base, pc, argc, extra_args, this, scope, static_class, ret, kind: Normal|ReentryBoundary|Generator|Include|Native, symtab, pending, silence_base, strict}` lives on the explicit frame stack of ADR-018. A fused `CallFn` remains a **later peephole** over the sequence; 05 §7.2's one-op→one-stencil property holds because each of `Init/Send/DoCall` is itself one op.

**Rationale.** The split makes by-ref, named, spread, defaults, arity errors, evaluation order and FCC fall out of one mechanism instead of five special cases; it is the ABI php-src has had since 7.0 and the one every `Zend/tests` `.phpt` assumes.

**Status.** Proposed (roadmap 2026-09-17). Narrows 05 §7.3 (Call group) and §12.2 (`CallFrame`). **Affected:** `05-bytecode-isa.md`, `06-interpreter.md`.

## ADR-017 — Destructors run from an op-boundary queue (resurrect-and-drain)

**Context.** PHP runs `__destruct` at the moment a refcount reaches zero; ADR-010 makes that timing a contract. With the heap as safe `Rc` (ADR-015) the only hook at refcount-zero is `Drop`, and safe Rust cannot call the VM from `Drop`: no `Interp` is in scope and the drop may occur inside a `RefCell` borrow.

**Decision.** `ObjectData::drop` pushes objects whose class has `__destruct` onto a **thread-local destructor queue**, resurrecting them by moving the data into a fresh handle. The interpreter **drains the queue at op boundaries and on every native return**; `Engine::shutdown` drains it after shutdown functions, then destructs still-live objects in php's order (globals in declaration order, then the rest). Drains are FIFO, so `$a = null; $b = null;` observes `aDb`-style output exactly as php-cli.

**Rationale.** "At the next op boundary" is indistinguishable from php-src for every program that does not inspect the stack between the drop and the next opcode — the same tolerance ADR-010 grants the JIT — and costs one length check per op.

**Status.** Proposed (roadmap 2026-09-17). **Affected:** `04-memory-gc.md` (destructor-timing guarantee), `06-interpreter.md`.

## ADR-018 — Re-entrant interpreter (`run_until(depth)`) for native→PHP calls; generators are heap-parked frames; Fibers deferred to a feature-gated crate that isolates `unsafe` stack switching

**Context.** Natives call back into PHP constantly (`array_map`, `usort`, autoloaders, `ob_start` handlers, `__toString`, `set_error_handler`). The current runtime recurses on the Rust stack per PHP call and no native can run PHP, and a 50k-deep PHP recursion overflows the host stack. Generators (`yield` in 97 Symfony files) need frames that outlive their activation. `09-runtime-sapi.md` §17.2 specifies stackful Fibers with guard-paged stacks — machinery Symfony's console and http-kernel paths never touch.

**Decision.** One explicit frame stack; `Interp::run_until(stop_depth)` is the **single dispatch loop**. A native that needs PHP calls `call_value()`/`call_method()`, which push a `ReentryBoundary` frame and run to it; an exception popping a boundary returns `Err(Unwind::Throw)` to the native, which propagates it with `?`, so exceptions cross native frames. A depth guard bounds native re-entrancy so the Rust stack cannot overflow. **Generators**: a `FnFlags::GENERATOR` function builds its frame, copies the register window into `GeneratorState{frame, regs, status, current_key/val, sent, return_val, delegate, auto_key}` and returns a `Generator` object; resume pushes the parked frame and `run_until(depth)`; `Op::Yield{dst, key, val}`, `Op::YieldFrom{dst, src}` and `Op::GenReturn` park it again; `yield from` delegates to generators, arrays and iterators; destroying a suspended generator runs its pending `finally` blocks. **Fibers** are deferred to a separate feature-gated crate that is the *only* place `unsafe` stack switching is allowed; `rphp-runtime` stays `#![forbid(unsafe_code)]`.

**Rationale.** A re-entrant loop with boundary frames gives native↔PHP calls with no trampoline and no Rust recursion per PHP call; parking a register window is the cheapest generator on top of it. Fibers need real stack switching, which is exactly the `unsafe` the safe engine must not host.

**Status.** Proposed (roadmap 2026-09-17). Narrows 09 §17.2 (Fibers leave the core crate). **Affected:** `05-bytecode-isa.md` (generator ops, `Frame.kind`), `06-interpreter.md`, `09-runtime-sapi.md` (§17.2).

## ADR-019 — Symbol-table fallback frames (`NEEDS_SYMTAB`) bind registers to named `Ref` cells; `$GLOBALS` is a read-only snapshot with write-through ops

**Context.** Registers are anonymous (05 §7.1), but PHP exposes locals by name: `compact`/`extract`/`get_defined_vars`/`$$x`, `include` sharing the includer's scope, `global $x`, `eval`, `$GLOBALS`. No spec says how a register machine serves them; the engine has none of them.

**Decision.** The compiler marks a function `NEEDS_SYMTAB` when it is a file `{main}` or uses `include/require/eval/compact/extract/get_defined_vars/$$x`; such frames **bind their registers to named `Ref` cells at entry** (`BindSymtab`), so name-based and register access alias one cell. Included files share the includer's symtab (an `Include` frame, ADR-020); `global $x` (`BindGlobal`) binds a register to the globals-table cell; `static $x` (`BindStatic`) to the function's per-closure/per-class static cell. `$GLOBALS` follows 8.1 semantics: a read yields a **read-only snapshot array**, `$GLOBALS['x'] = …`/`unset($GLOBALS['x'])` compile to **write-through ops** on the globals table, and `$GLOBALS = …` or by-ref use is a compile error with PHP's text. The superglobals (`$_SERVER`, `$_GET`, …) are seeded by the SAPI (ADR-033) and bound in every frame without a `global` declaration.

**Rationale.** Paying the symtab only in frames that can observe it keeps ordinary functions register-only (the 05 design), while Composer's autoloader, Symfony's `PhpFileLoader` and every templating engine — all `include`-based — get exact PHP scoping.

**Status.** Proposed (roadmap 2026-09-17). **Affected:** `02-value-model.md`, `05-bytecode-isa.md` (`BindSymtab/BindGlobal/BindStatic`), `06-interpreter.md`.

## ADR-020 — Units, include/require/eval, runtime `CompileHook`, autoload protocol, late-bound names with generation-stamped inline caches

**Context.** `Module` is exactly one file with whole-program compile-time resolution: functions, classes and natives are numbered at compile time, so a second file, `eval`, `spl_autoload_register`, conditional declarations (`if (!function_exists('f')) { function f() {} }` — every polyfill) and `class_alias` are impossible. `01-frontend.md` §6.1 records autoload points in HIR but nothing specifies the runtime protocol, and nothing specifies `include` at all.

**Decision.** The compile artifact is `CompiledUnit{file, funcs, classes, main, hoist_funcs, hoist_classes, strict_types}`. `Interp::load_unit(unit, scope)` **hoists** unconditional top-level functions and early-bindable top-level classes (no parent/interfaces/traits — PHP's rule), declares the rest in statement order via `DeclareFunction/DeclareClass`, and runs `{main}` as an `Include` frame sharing the includer's symtab, `$this` and scope (Composer's `Closure::bind(static function ($file) { include $file; }, null, null)` relies on the null scope); the include's value is its `return` or `1`. `Op::Include` resolves absolute → `include_path` → the current file's dir → cwd, keeps a canonical-path set for `_once`, makes `require` failure fatal and `include` failure a warning + `false`, and throws `ParseError`; `Op::Eval` names its unit `file(line) : eval()'d code`. The runtime owns a **`CompileHook`** (installed by `rphp-embed`) so `include`/`eval` compile on demand, and the process-level `Engine` caches compiled units by `(realpath, mtime, size)` — the opcache role, with `opcache_*` functions deliberately absent. **Autoload**: `lookup_class(name, autoload)` with a recursion guard over an autoloader stack (`spl_autoload_register(cb, throw, prepend)`), triggered by `new`, static access, `extends`/`implements`/`use`, `class_exists(.., true)`; **not** by `instanceof`, `catch` types or type checks (catch types match by name without autoload). **Late binding**: functions, classes, constants and catch types resolve at runtime through `Interp` tables; every call/new/const/static/prop site carries a **generation-stamped inline cache** invalidated when the owning table's generation changes. Unqualified function/const names in a namespace resolve by PHP's two-step (`ns\f`, then `f`), cached per site. `declare(strict_types=1)` is a per-unit flag carried on the frame.

**Rationale.** PHP is late-bound and file-granular; a whole-program compile-time model cannot run `vendor/autoload.php`, the first rung of the ladder. Generation-stamped ICs keep the resolved path as fast as the compile-time numbering it replaces.

**Status.** Proposed (roadmap 2026-09-17). Refines 01 §6.1 (`Autoload(SymbolId)` becomes the runtime protocol) and 06's inline caches. **Affected:** `01-frontend.md` (§6.1, eval semantics), `05-bytecode-isa.md` (`CompiledUnit`, `Include/Eval/DeclareFunction/DeclareClass`), `06-interpreter.md` (ICs), `09-runtime-sapi.md` (compile hook, unit cache).

## ADR-021 — Native class model: builder, `Payload::Native(Box<dyn Any>)`, inheritance from native classes via inherited slot layout + `native_init`, `payload_clone`/`trace` hooks, object handler table

**Context.** There are no native classes today — not `stdClass`, not `Exception`. Symfony L2 alone needs 82 internal classes (Reflection, SPL, DOM, DateTime, `Closure`, `WeakMap`, the `Throwable` tree), user classes must extend them (`class MyException extends \RuntimeException`, `class Foo extends ArrayObject`), and SPL/DOM/PDO objects must iterate, `count()`, index, compare and `var_dump` exactly as php.

**Decision.** One `ClassDef{kind, parent, interfaces (flattened), flags, props: Vec<PropInfo{slot, vis, set_vis, decl, ty, readonly, default, hooks}>, prop_index, private_index, static_props, consts (lazy thunks with cycle detection), methods: HashMap<lower name, Rc<MethodDef{User|Native}>>, magic flags, native_init, payload_clone, enum_cases, attrs, doc, unit, line}` serves user and native classes alike. Native classes are declared through a **builder** (`r.class("Exception").implements([..]).prop("message", Protected, ..).method("getMessage", ..).native_init(..).finish()`); native state lives in `Payload::Native(Box<dyn Any>)`. **User classes may extend native classes**: the slot layout is inherited, `native_init` runs at `new`, `payload_clone` on `clone`, and a `trace` hook exposes payload-held objects to the cycle collector (ADR-024). A native class may install an **object handler table** — `get_iterator`, `count`, `dim_read/write/has/unset`, `debug_info`, `clone`, `serialize/unserialize`, `compare`, `cast`, `do_operation` — consulted by `IterInit`, `count()`, `ArrGet/Set/Isset/Unset`, `var_dump`, `==`, casts and arithmetic ahead of the userland protocols (`ArrayAccess`, `Countable`, `IteratorAggregate`, `__toString`).

**Rationale.** This is Zend's `zend_object_handlers` with a typed builder in front; without a handler table SPL/DOM/PDO objects cannot behave natively in `foreach`/`count`/`$o[$k]`, and without native inheritance no user exception can exist.

**Status.** Proposed (roadmap 2026-09-17). Extends 08 §16.1's `r.class::<Widget>(...)` builder with inheritance and handlers. **Affected:** `03-heap-types.md` (objects), `08-stdlib-ext.md` (§16 builder; new object-handler-table section), `06-interpreter.md`.

## ADR-022 — Diagnostics channel: `error_reporting`/handler stack/`@`/`display_errors`→stdout on CLI; warnings may become throws via user handlers; exceptions remain the only non-local control channel

**Context.** The engine has no warning channel: undefined variables, keys, offsets, array-to-string and non-numeric operands are silent or fatal, and there is no `set_error_handler`, `@`, `error_get_last`, `trigger_error` or `display_errors`. Symfony's `ErrorHandler` **registers a handler that throws** `ErrorException` on warnings, its `Debug` component reads `error_get_last()` at shutdown, and `.phpt` expectations are full of `\nWarning: msg in %s on line %d\n`. The Affirmed list already says native errors are thrown `Throwable` objects.

**Decision.** Every runtime path returns `Result<_, Unwind>` with `Unwind::{Throw(Object), Pending(kind, msg), Exit(code)}`; **engine faults construct Throwables** through one `interp.throw(kind, msg)` (`DivisionByZeroError`, `TypeError`, `ArgumentCountError`, `Error`, `UnhandledMatchError`, `ValueError`, `ArithmeticError`) with PHP-exact messages. Non-fatal diagnostics go through `emit_error(level, msg, file, line)`: it honours `error_reporting`, the `@` silence depth (`Op::Silence`), the user handler stack (`set_error_handler` — the handler **may throw**, turning a warning into `Unwind::Throw` at the emitting op), records `last_error`, and on the CLI renders `\nWarning: msg in file on line N\n` to **stdout** (`display_errors=1`, as `php -n` does). Uncaught exceptions render byte-identically to php-cli (`PHP Fatal error:  Uncaught …`, `Stack trace:`, `thrown in … on line N`) and exit 255; `set_exception_handler` is honoured. Exceptions remain the **only** non-local control channel: no native returns a separate error enum.

**Rationale.** Frameworks program *against* the warning channel (handlers, `@`, `error_get_last`), and byte-exact warning text is what the differential oracle (ADR-034) compares by default. One `Unwind` type keeps table-driven unwinding (05 §7.4) the single mechanism.

**Status.** Proposed (roadmap 2026-09-17). Makes the affirmed "native errors are thrown `Throwable` objects" concrete. **Affected:** `06-interpreter.md`, `08-stdlib-ext.md` (§15.1 `NativeResult`), `09-runtime-sapi.md` (CLI error display), `10-testing.md`.

## ADR-023 — Object ids are monotonic and resource ids are per-`Interp`; both allowlisted under ADR-008

**Context.** `spl_object_id`, `var_dump`'s `#N`, `SplObjectStorage`, `WeakMap` and resource handles (`Resource id #N`) are observable. php-src reuses freed object handles from a free list, so ids depend on allocation history; ADR-008 / 10 §20.2 already list resource ids and `spl_object_id` as volatile without saying what rPHP does.

**Decision.** Object ids come from a **monotonic per-`Interp` counter** (never reused within a request); resource ids from a separate per-`Interp` counter starting where php-cli's do (`STDIN/STDOUT/STDERR` = 1, 2, 3). Both are deterministic for a given program but are **not** promised to match php-src's free-list reuse; the oracle normalizes them under the closed categories `object-id` and `resource-id` (ADR-034). Identity semantics (`===`, `SplObjectStorage`, `WeakMap`) are exact.

**Rationale.** Monotonic ids never alias a live object and key the interim collector's weak registry (ADR-024) trivially; matching Zend's handle reuse would buy only bug-compatibility of a number the corpus already treats as volatile.

**Status.** Proposed (roadmap 2026-09-17). **Affected:** `03-heap-types.md`, `10-testing.md` (§20.2 allowlist rows).

## ADR-024 — Interim synchronous weak-registry cycle collector (threshold + `gc_collect_cycles`); `memory_get_usage` is a high-water estimate

**Context.** ADR-002 specifies a bounded, incremental Bacon–Rajan collector over `GcHeader`s plus arena reset. Neither the header nor the arena exists (ADR-015); `Rc` leaks every cycle, and Symfony's container, event dispatcher and Doctrine graphs are cyclic by construction. `gc_collect_cycles()` is called by Symfony's `Kernel` and by PHPUnit; `memory_get_usage()` must return something plausible for `memory_limit`-guarded code.

**Decision.** Until the arena/`GcHeader` heap lands, `rphp-runtime` runs a **synchronous trial-deletion collector over a weak object registry** filled by `Interp::alloc_object`: count internal edges (slots, dynamic props, closure captures/`this`, generator registers, `WeakMap` values, native-payload `trace` hooks, recursing through arrays and `Ref`s); an object is a root if `strong_count − internal − 1 > 0`; mark from roots; run `__destruct` on the garbage through the ADR-017 queue; then break cycles by taking the objects' contents. Triggered at op-boundary safepoints past an allocation threshold and by `gc_collect_cycles()`, which returns the collected count as php does. `memory_get_usage()`/`memory_get_peak_usage()` return a **high-water estimate** tracked at allocation sites, allowlisted under ADR-034's `platform-value` category. The collector is unsafe-free; `rphp-gc` and ADR-002's incremental design remain the end state.

**Rationale.** Bounded RSS on cyclic graphs is a correctness requirement for a long console command or the dev server; a synchronous collector over a weak registry is the smallest exact (no false frees) implementation in safe Rust. ADR-002's pause bounds are a server-throughput concern deferred with that milestone.

**Status.** Proposed (roadmap 2026-09-17). Interim to ADR-002, which stays Accepted as the end state. **Affected:** `04-memory-gc.md` (interim section), `10-testing.md` (allowlist).

## ADR-025 — `mago-syntax` is the production parser, pinned exactly, with a superset conformance pass, a `NodeKind` coverage test, a bump policy behind the corpus job, and documented fallback triggers/cost; supersedes the "bootstrap" reading of ADR-007; closes O-1

**Context.** ADR-007 chose `mago-syntax` "for M0" as a bootstrap, and the code never followed it: the current `rphp-lexer`/`rphp-parser` are an owned subset that rejects typed parameters and most of PHP 8.4. Symfony's vendor tree uses the full grammar (namespaces, attributes, `readonly`, `match`, enums; property hooks may appear in apps). `mago-syntax` 1.49.0 (25k LOC, MIT/Apache-2.0, `rust-version 1.97`, arena AST via `LocalArena`, 227 node kinds, statement-level error recovery, trivia and docblocks retained) already parses all of PHP 8.4/8.5. It over-accepts a few things php 8.5.0 rejects (partial function application `f(?, 1)`, `&&=`, `(real)`/`(unset)` casts, `;` as `switch` separator, nullsafe writes, positional-after-named, `new` in FCC), and its lexer is not Zend-token-exact.

**Decision.** `mago-syntax` is the **production** parser, not a bootstrap: **pinned exactly** (`mago-syntax = "=1.49.0"` with `mago-allocator`/`mago-database` at the same version) and bumped only by the ADR-028 procedure. `rphp-parser` is the adapter: `LocalArena` per file → `parse_file_content` → AST v2 (intern names, decode strings/heredoc incl. `DocumentIndentation`, int overflow → float, attach docblocks, spans 1:1, mago's error kinds → `RPHP_E0001..E0006`), then a **conformance pass** rejecting mago's superset with php's own messages (`RPHP_E0011`), plus an `IdentSpans` side table for `TOKEN_PARSE`. A **`NodeKind` coverage test** asserts every mago node kind is either mapped or explicitly rejected as `RPHP_E0010`, so a mago release cannot add syntax silently. **Fallback triggers** (documented, not expected): mago drops PHP 8.5 conformance, stops building on the pinned toolchain, changes licence, or its error recovery diverges from what `rphp -l`/the corpus job need. **Fallback cost:** an own recursive-descent parser producing the same AST v2 (~8–12k LOC) with no downstream change, because HIR and below consume `rphp-ast` only — ADR-007's ownership boundary is unchanged. **O-1 is closed**: there is no owned lexer for *parsing*; the owned scanner exists only for `ext/tokenizer` (ADR-029). `rphp-lexer` is deleted in F3.

**Rationale.** A complete, maintained PHP 8.5 parser with error recovery is 25k LOC the project should not rewrite before it can run `bin/console`; the conformance pass and the coverage test are the two mechanisms that turn a dependency into a contract.

**Status.** Proposed (roadmap 2026-09-17). Supersedes ADR-007's "bootstrap/M0" wording (the `rphp-ast` ownership boundary and CST deferral are kept). Closes O-1. **Affected:** `01-frontend.md` (§4 lexer → tokenizer, §5, O-1), `00-overview.md` (`rphp-lexer` removal), `10-testing.md` (corpus job).

## ADR-026 — HIR shares the AST vocabulary (newtype + canonical-subset invariant; `Let/Temp/Seq` only additions)

**Context.** `01-frontend.md` §6 describes HIR as "a small, regular core" with its own node set, a separate `rphp-resolve` crate and a desugaring catalogue. A second tree means a second printer, visitor, snapshot format and a second set of 118-site compiler matches — for a front end that must grow from a subset to all of PHP 8.5 in weeks, that duplication is the schedule risk.

**Decision.** `rphp-hir` **reuses the AST types**: HIR is a newtype over `rphp-ast` with a **canonical-subset invariant** enforced by `validate` (after lowering there is no `elseif`, alt syntax, `for`, `match`, `??=`, `op=`, `?:`, `?->` chain, destructuring, interpolation, arrow fn, FCC, pipe, `@`, legacy cast, hook-as-syntax or promotion), and `Let/Temp/Seq` are the **only** HIR-only nodes. Resolution (per-statement namespace, per-kind import tables, group use, `self/static/parent`, `::class` folding on names only, magic constants incl. 8.4 closure naming and `__PROPERTY__`, hoisting classification, `goto` and `strict_types` validation) lives in `rphp-hir::resolve`; the separate `rphp-resolve` crate is folded in. Desugaring follows one shared **stabilize** rule (bind lvalue sub-expressions to temps so `$a[f()] ??= 1` evaluates `f()` once, in PHP order). `switch`, `clone`, `isset/empty`, `print/exit`, `yield`, `include/eval`, `global/static` stay as nodes. One printer (`--emit=ast|hir`), one visitor, one compiler input.

**Rationale.** The value of HIR is the *invariant*, not a distinct type system; a newtype gives the invariant and keeps one vocabulary for every tool.

**Status.** Proposed (roadmap 2026-09-17). Narrows 01 §6/§6.2 (desugaring catalogue v2 to be written). **Affected:** `01-frontend.md` (§6, §6.2), `00-overview.md` (`rphp-resolve` folded into `rphp-hir`), `10-testing.md` (§20.4 snapshots).

## ADR-027 — Declaration metadata (attributes, doc comments, unevaluated constant-expression defaults, types) is a bytecode contract because Reflection round-trips it

**Context.** 05 §7.4 carries `signature` and spans but nothing about attributes, doc comments, promoted/hooked properties, enum cases or *unevaluated* defaults. Symfony's DI autowiring, routing, serializer, validator and var-exporter run on `ReflectionClass`/`ReflectionParameter`/`ReflectionAttribute`/`getDocComment()`/`isDefaultValueConstant()`; L4's byte-identical dumped container depends on all of it.

**Decision.** The compiled unit carries declaration metadata as a **contract**, not a debug aid: `FuncMeta{params: [ParamMeta{name, ty, default: ConstExpr, by_ref, variadic, promoted, attrs}], ret, doc, attrs, flags, lines}`, `ClassMeta{consts, props (type/default/hooks/vis/set_vis/readonly/static), enum backing + cases, attrs, doc, unit, line}`, `AttrMeta{name_fqn, args: (name?, ConstExpr), target, line}`. Defaults and constant initializers are stored as a `ConstExpr` normal form that **keeps unevaluated constant references** and is evaluated lazily with cycle detection — `isDefaultValueConstant`/`getDefaultValueConstantName` need the name, `newInstance` needs the value. `TypeDecl` maps 1:1 onto `ReflectionNamedType/UnionType/IntersectionType`. Line tables (`Function.lines` parallel to `code`) belong to the same contract so every message says "in file on line N".

**Rationale.** Reflection is not optional in the Symfony ecosystem; metadata absent from the bytecode cannot be reconstructed later, and the on-disk code cache (00 §1) would drop it.

**Status.** Proposed (roadmap 2026-09-17). Extends 05 §7.4. **Affected:** `05-bytecode-isa.md` (§7.4), `01-frontend.md` (§6.3 const-expr evaluator), `08-stdlib-ext.md` (Reflection).

## ADR-028 — Nightly pin tracks mago's MSRV; bump procedure = `become` build + stable portable build + corpus job

**Context.** ADR-013 pins a nightly for `become`. With `mago-syntax` a hard dependency (ADR-025, `rust-version 1.97`) the pin has two masters, and the default build today is the **portable core** (`rphp-runtime` is a `loop { match }`; `become` is a later feature), so the workspace also compiles on stable.

**Decision.** `rust-toolchain.toml` pins a nightly **no older than mago's MSRV** (currently `nightly-2026-09-01`, rustc ≥ 1.97). A bump is one PR that (1) bumps the channel, (2) proves the `become` path builds behind `tailcall-dispatch`, (3) proves the portable core builds on the matching stable, (4) passes the corpus job (ADR-029), and (5) bumps mago in lockstep if it moved. mago bumps are **quarterly at most** and rehearsed once (F8) before being relied on.

**Rationale.** A dependency's MSRV and a compiler feature are both reasons to pin; one procedure keeps "nightly" reproducible (ADR-013's intent) while the front end depends on a third-party crate.

**Status.** Proposed (roadmap 2026-09-17). Narrows ADR-013 (pin policy) and ADR-001 (the portable core is the default build today; `become` remains the intended Tier-0 headline path behind a feature). **Affected:** `00-overview.md` (§3.1), `06-interpreter.md`.

## ADR-029 — Two-way `php -l` parity is a must-not-regress gate; the owned `rphp-tokenizer` is the `ext/tokenizer` implementation

**Context.** The front end must accept exactly what php 8.5.0 accepts. mago's lexer is not Zend-token-exact, and `token_get_all`/`PhpToken` are used by Symfony's `AttributeFileLoader::findClass`, `PhpDumper::stripComments`, Twig and every static-analysis tool the ladder will run. The local vendor corpus is 22k files and `php -l` costs ~1.2 ms/file, so whole-corpus parity is cheap.

**Decision.** (1) A **corpus job** (`RPHP_CORPUS_DIR`, parallel, `php -l` verdicts cached by sha256) asserts two-way parity — every `php -l`-clean file parses with 0 diagnostics and every file of a negative corpus is rejected — plus a parse-only sweep over php-src `Zend/tests` and `tests/lang`; it is a **must-not-regress CI gate**. (2) `rphp-tokenizer` is an **owned, direct implementation of the Zend scanner states** (`Initial, Scripting, LookingForProperty, DoubleQuotes, Heredoc, Nowdoc, Backquote, VarOffset, LookingForVarname, EndHeredoc, HaltCompiler`): `tokenize(src, short_open_tag) -> Vec<RawToken{id, lo, hi, line}>`, lossless, token ids 260–411 generated from the installed php and locked by a test, the Zend quirk list as differential tests, `TOKEN_PARSE` reclassifying `IdentSpans` from the adapter (ADR-025), `PhpToken::tokenize()` returning `static`. It has no dependency on mago and is **not** used for parsing. Gate: 100 % token-stream identity with `php` over the corpus in both modes; lossless under fuzz.

**Rationale.** `token_get_all` is a byte-exact userland contract only a Zend-shaped scanner satisfies; the parser needs the full grammar, not token exactness. Two tools, two jobs.

**Status.** Proposed (roadmap 2026-09-17). Closes O-1 together with ADR-025. **Affected:** `01-frontend.md` (§4 rewritten as the tokenizer section), `08-stdlib-ext.md` (`ext/tokenizer`), `10-testing.md` (corpus job, `--emit=tokens`).

## ADR-030 — Arginfo, constants and ini defaults are generated from a committed manifest dumped from the installed PHP 8.5; `NativeId` is process-stable and bytecode carries names; arity/type errors are runtime `ArgumentCountError`/`TypeError`

**Context.** 08 §15.2 generates arginfo from a hand-written declarative table. The installed oracle has 1956 internal functions, 305 classes / 4136 methods, 2854 constants and 288 ini directives; hand-typing them is drift the coverage metric (ADR-004) cannot see. Today the compiler resolves natives at compile time (`rphp_stdlib::resolve`) into a `NativeId` baked into bytecode and checks arity there — neither PHP's behaviour (`strlen()` with 0 args is a *runtime* `ArgumentCountError`) nor compatible with late binding (ADR-020).

**Decision.** `tools/manifest/dump.php` (run with the installed `php`) dumps `manifest/php-8.5.0/{functions,classes,constants,ini}.json`, which is **committed**. `cargo xtask gen` emits `crates/ext/rphp-ext-<x>/src/generated/{arginfo,consts,ini}.rs` — `FnSig{name, params: [ParamInfo{name, ty: TypeMask, by_ref, variadic, nullable, default}], required, ret, deprecated}`, class/method skeletons, `ConstDef`, `IniDef` — and CI runs `gen --check`. Value constants are generated verbatim; runtime constants (`PHP_BINARY`, `PHP_OS`, `DIRECTORY_SEPARATOR`, `STDIN/STDOUT/STDERR`, `PHP_SAPI`, …) are computed at startup, and a test diffs `get_defined_constants(true)` against `php -n` per category. **Function identity:** bytecode carries **names**; the runtime resolves user table → native registry at first execution and caches a **process-stable `NativeId`** (assigned at link time, never persisted) in the call-site IC. Arity and parameter-type checks happen at `DoCall` with PHP-exact messages (`strlen() expects exactly 1 argument, 0 given`; `strlen(): Argument #1 ($string) must be of type string, array given`); named arguments and defaults for natives come from the generated `ParamInfo`. The purity flags of 08 §15.1 stay hand-annotated (they are not in the manifest) and conservative by default.

**Rationale.** The oracle *is* the spec; generating from it makes signatures, reflection, constants and ini defaults unable to drift and makes `cargo xtask coverage` exact. Names in bytecode are what late binding and the on-disk cache both need.

**Status.** Proposed (roadmap 2026-09-17). Narrows 08 §15.2 (the source of truth is the manifest, not a hand-written table). **Affected:** `08-stdlib-ext.md` (§15.1–15.2), `05-bytecode-isa.md` (`Const::Name`), `10-testing.md` (§20.8, `gen --check`).

## ADR-031 — Extension crate layout: `crates/ext/rphp-ext-<name>` per extension mirroring php-src file names, `rphp-stdlib` as the feature-gated bundle, per-crate `catalog.toml` for deferred functions

**Context.** `rphp-stdlib` is one crate of module files (`ctype/math/string/array/json/hash/pcre`) that the runtime depends on. The parity burn-down (ADR-004) runs one agent per extension against `ext/<name>/tests/`; a single crate is a merge bottleneck and hides which extension owns a file, and the "deferred, blocked by X" policy the coverage report needs is not machine-readable.

**Decision.** One crate per PHP extension at `crates/ext/rphp-ext-<name>`, with source files **mirroring php-src's `ext/<name>/<file>.c` naming** (`rphp-ext-standard/{string,array,math,var,file,dir,head,…}.rs`), each depending on `rphp-ext-api` (descriptor types only, no engine dependency: `FnSig`, `NativeFn{sig, flags, handler}`, `ConstDef`, `IniDef`, `ExtInfo`, the `Extension` trait with `info/functions/classes/constants/ini/module_init/request_init/request_shutdown`, `TypeMask`, `FnFlags`, the `nf!(sig::STRLEN, strlen)` macro) and on `rphp-runtime` for `Ctx`. `crates/rphp-stdlib` becomes the **bundle** crate: `extensions()` returns the enabled set behind `ext-<name>` cargo features. Core functions and classes (`Closure`, `Generator`, `stdClass`, the `Throwable` tree, `WeakReference`/`WeakMap`, engine constants) live in `rphp-runtime`. Each extension crate carries a **`catalog.toml`** listing every manifest symbol it does not implement with a reason and blocker (`deferred = "blocked by libxml2 binding"`); `cargo xtask coverage` consumes it, rewrites `crates/rphp-stdlib/COVERAGE.md`, keeps `coverage.lock.json`, and a symbol absent from both registry and catalog fails CI.

**Rationale.** Disjoint crates are what makes the stdlib a parallel multi-agent track in practice; php-src file naming lets an agent map straight to the tests it must pass; the catalog turns "never stub" (ADR-004) into a checked policy.

**Status.** Proposed (roadmap 2026-09-17). Refines 08 §16.1. **Affected:** `00-overview.md` (crate table), `08-stdlib-ext.md` (§16, coverage), `10-testing.md` (§20.8).

## ADR-032 — Backend choices: mbstring/iconv pure Rust (ADR-003 narrowed to `intl`); dom/xml family binds libxml2/libxslt (pure-Rust backend deferred to WASM); `intl` behind Symfony polyfills until Tier C; curl deferred behind `NativeHttpClient`; date on `jiff` + own timelib port

**Context.** ADR-003 binds system ICU for `intl`/`mbstring`; 08 §15.3 routes XML, curl and others to system bindings and says nothing about `date`. Measured needs: framework-bundle hard-requires `ext-ctype`, `ext-iconv`, `ext-xml`; Translation's XLIFF loader and the Config XML loaders do **XSD schema validation and XPath 1.0**, which no pure-Rust crate offers; php-src's `mbstring` does not use ICU (own codecs and case tables); Symfony ships `symfony/polyfill-intl-*`, and `NativeHttpClient` works without curl.

**Decision.** (1) **`mbstring` and `iconv` are pure Rust** (`encoding_rs` + own UTF-16/32/UCS-2/UTF-7/HTML-ENTITIES/BASE64/QP codecs, case mapping incl. FOLD/SIMPLE/TITLE, `unicode-width`, a port of php's detect-encoding scoring, `//TRANSLIT` via `any_ascii`); **ADR-003 is narrowed to `intl`**. (2) **`dom`/`libxml`/`xml`/`simplexml`/`xmlreader`/`xmlwriter`/`xsl` bind system libxml2 + libxslt** behind an `XmlBackend` trait, with a node-proxy identity map per document; a pure-Rust backend is deferred to the WASM target. (3) **`intl`** stays cataloged ("polyfilled by symfony/polyfill-intl-*") until Tier C, then `icu4x` on all targets. (4) **`curl`** is deferred behind Symfony's `NativeHttpClient` (stream sockets + `rustls`). (5) **`date`** is `jiff` + `jiff-tzdb` with an **own rule-by-rule port of timelib's parser** for `strtotime`/`DateTime::__construct`/`modify`/`createFromFormat`, fuzz-diffed against `php -r`; the `tzdb-version` category (ADR-034) covers tzdata drift. (6) **`pcre`** moves to `pcre2-sys` behind an own ~500-LOC safe wrapper (the `pcre2` crate lacks modifiers, limits, offsets and unmatched-as-null).

**Rationale.** Each choice follows one test: bind where a clean-room rewrite is a correctness liability against an oracle we cannot match (XSD/XPath, PCRE); write Rust where php-src itself is self-contained (mbstring, date parsing) or the framework already polyfills the gap (intl, curl).

**Status.** Proposed (roadmap 2026-09-17). Narrows ADR-003 (ICU/`icu4x` only for `intl`). **Affected:** `08-stdlib-ext.md` (§15.3, §15.4 tiers), `09-runtime-sapi.md` (WASM backend note).

## ADR-033 — SAPI order CLI → built-in server → FastCGI; hand-rolled HTTP/1.1, thread-per-connection with one `Rc`-based `Interp` per request; `rphp-embed` `Request`/`OutputSink`/`Response` contract; request binding, headers, output buffering and superglobal seeding specified in 09

**Context.** 09 §17/§18 specify a multi-isolate server with an `io_uring` reactor, fibers, HTTP/2 and a warm JIT. None of it exists; the current CLI buffers all output into a `Vec<u8>` and writes it at exit, so a Symfony `ProgressBar` or `bin/console` under a pty cannot work, and there is no request context at all. The `Interp` is `Rc`-based and `!Send` (ADR-015). Nothing specifies output buffering, headers, cookies, `$_SERVER` seeding or `php://input`.

**Decision.** **Order: CLI parity → built-in `rphp -S` dev server → FastCGI**; the reactor/isolate-pool server of 09 §17.3 is deferred behind them. `rphp-embed` exposes the contract every SAPI uses: `Engine` (per process: registry bundle, compiled-unit cache, ini defaults, immortal data; `Send + Sync`), `Interp` (per request/script, `!Send`), `Request{server vars, raw query, body, cookies, headers, files, method, script_filename, sapi}`, the `OutputSink` trait (`write/flush/send_headers/headers_sent`), `Response{status, headers, exit_code}`, and `run_file/run_code/set_ini/define/shutdown` with php's shutdown order (shutdown functions → destructors → ob flush → session write → stream close). **CLI** (`rphp-sapi-cli`): php's argument grammar (`[options] [-f] <file> [--] [args…]`, `-r`, repeatable `-d`, `-n`, `-l`, `-m`, `-i`, `-v`, `--ri/--rf/--rc`, `-S/-t` delegating to the server, `--emit=` kept), `$argv/$argc`, `$_SERVER` exactly as stock CLI, `$_ENV` per `variables_order`, `PHP_BINARY`/`PHP_SAPI`, `STDIN/STDOUT/STDERR` resources, **streaming output** straight to fd 1 with `implicit_flush=1` (`php://stdout` bypasses `ob_*`, `php://output` goes through it), uncaught → exit 255. **Server** (`rphp-sapi-server`): a hand-rolled minimal HTTP/1.1 over `std::net::TcpListener`, **thread-per-connection with a bounded pool**, keep-alive, chunked responses, **no hyper/tokio** — one `Interp` per request lives on one thread for the request's life, sharing the `Engine`; request binding produces `$_SERVER/$_GET/$_POST/$_FILES/$_COOKIE/$_REQUEST` and a re-readable `php://input` exactly as `php -S` (own RFC 1867 multipart parser, `max_input_vars`, `request_parse_body()`, `getallheaders()`), with `php -S` router semantics, static files with PHP's mime table, the 404 page and access-log line format. The headers API (`header` incl. replace/`Location`→302/status lines, `header_remove`, `headers_list`, `headers_sent(&$f,&$l)`, `http_response_code`, `setcookie/setrawcookie` incl. the options array, `header_register_callback`) flushes on the first output byte with `ob_*` delaying exactly as php. **FastCGI** (`rphp-sapi-fcgi`) reuses the binding layer and implements `fastcgi_finish_request`. Output buffering, headers and superglobal seeding get their own section in 09.

**Rationale.** The ladder's L7 gate is "status/headers/body identical to `php -S`", which needs a request contract and streaming output, not a reactor. Thread-per-connection is the only model compatible with an `Rc` interpreter, and it is what `php -S` and php-fpm effectively are.

**Status.** Proposed (roadmap 2026-09-17). Narrows ADR-012's runtime hooks (reactor-driven I/O deferred; streams/sessions stay first-class) and 09 §17.3/§18 (server model). **Affected:** `09-runtime-sapi.md` (§18; new output-buffering/headers/superglobals section), `00-overview.md` (crate table), `08-stdlib-ext.md` (`head.rs`, `php://input`).

## ADR-034 — Differential oracle implementation: pinned `php -n -d …` ini, stdout byte-exact by default, stderr normalized, exit codes exact, closed set of divergence categories, ladder fixtures as conformance milestones, `.phpt` baselines that only ratchet up

**Context.** ADR-008 makes the oracle fuzzy + allowlisted, and 10 §20.2 lists five open-ended categories including "error/exception message wording". The current harness compares `examples/tier-a/*.php` stdout byte-for-byte in-process, and that has been the stronger, more useful signal: PHP's warning text, `var_dump` shapes and uncaught-exception rendering are exactly what frameworks and `.phpt` files depend on, and treating message wording as free would hide real bugs.

**Decision.** The harness (`crates/rphp-test`, driven from `tools/rphp/tests/differential.rs`) runs both binaries as **processes** with a **pinned oracle ini**: `php -n -d display_errors=1 -d log_errors=0 -d error_reporting=E_ALL -d html_errors=0 -d date.timezone=UTC -d precision=14 -d serialize_precision=-1`. **stdout is compared byte-exact by default**, stderr normalized, exit codes exact; a snippet may opt into a `<name>.expectf` template. The allowlist `examples/tier-a/divergences.toml` has a **closed set of categories** — `float-format, locale, error-wording, hash-order, platform-value, resource-id, object-id, tzdb-version, pid, timing, tempnam-path` — every entry names one and cites why; adding a category is an amendment to this ADR. `error-wording` covers only messages php-src itself changes between patch releases, never engine-fault text. Snippets live at `examples/tier-a/<ext>/<topic>.php`, and every wave adds snippets that exercise warnings and exceptions on purpose. **Ladder fixtures** `fixtures/ladder/L<n>-<name>/` (committed `composer.json`/`composer.lock`/`rung.toml`; `vendor/` installed by stock php via `tools/fixtures/setup.sh`) are the conformance **milestones** L0–L9, run by `cargo xtask ladder [--rung L<n>]`, which reports the first divergence with context. **`.phpt` baselines** per extension (`tests/phpt/enabled/<ext>.toml` over a sparse php-src checkout at `php-8.5.0`) may only **ratchet up** (`cargo xtask phpt --gate`); the runner implements run-tests.php's sections and `EXPECTF` wildcard set, one real `rphp` process per test in a temp cwd with the `php -n`-equivalent ini.

**Rationale.** The corpus is fuzzy where its authors chose to be (`EXPECTF`) and exact everywhere else; byte-exact by default with an explicit, finite escape hatch keeps ADR-008's honesty and removes its blind spot. Ladder rungs are the only milestones that measure "runs Symfony".

**Status.** Proposed (roadmap 2026-09-17). Narrows ADR-008 (byte-exact default; closed categories; error wording is no longer a blanket category). **Affected:** `10-testing.md` (§20.1, §20.2, §20.8), `00-overview.md` (§4 milestones).

## ADR-035 — PDO deferred until L7 is green; the driver approach is decided then (default pure Rust: `rusqlite`, `mysql`, `postgres`)

**Context.** ADR-012 makes PDO first-class and 08 §15.5 prioritizes PostgreSQL/MySQL with "SQLite early for tests" (O-2) and reactor-driven async execution. Symfony's skeleton through L7 (HTTP) has no database; the first rung that needs one is L8 (`symfony/demo` on sqlite). Building PDO before the engine can serve a request would optimize the wrong risk.

**Decision.** PDO work (`rphp-ext-pdo` core over a `PdoDriver` trait with an emulated-prepare rewriter, plus `SQLite3`) **starts only after L7 is green**. The driver approach is decided at that point; the **default** is pure Rust — `pdo_sqlite` via `rusqlite` (bundled, incl. `sqliteCreateFunction`), `pdo_mysql` via the sync `mysql` crate, `pdo_pgsql` via `postgres`. **SQLite is the first driver** because L8 runs on it and it is hermetic in CI — this answers O-2. The reactor-driven async execution of 08 §15.5 is deferred with the reactor (ADR-033).

**Rationale.** Sequencing by the ladder puts PDO exactly where the first app needs it and lets the driver decision be made against a working request path rather than a spec.

**Status.** Proposed (roadmap 2026-09-17). Narrows ADR-012 (sequencing and driver order; first-class status unchanged). Closes O-2 as "SQLite first". **Affected:** `08-stdlib-ext.md` (§15.5 PDO, O-2).

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
| O-1 | Own the lexer (byte-level, §4) or reuse `mago-syntax`'s lexer in M0? | **Closed by ADR-025/ADR-029:** no owned lexer for parsing (mago's lexer feeds the adapter); the owned `rphp-tokenizer` exists only for `ext/tokenizer`. The §4 SIMD lexer is dropped. | closed |
| O-2 | Default DB driver beyond PG/MySQL (e.g. SQLite for tests)? | **SQLite first when PDO lands, per ADR-035** (L8 runs on it; hermetic in CI); MySQL/PostgreSQL follow, pure-Rust drivers by default. | closed (ADR-035) |
| O-3 | Zend C ABI shim — ever, or never? | Keep as a research track (baseline §16); revisit after M5. | post-M5 |
| O-4 | Whole-program analysis (`rphp-analyze`) default-on vs opt-in given its cost/soundness limits? | Opt-in at first; promote to default-on per-extension as soundness is proven. | `07-jit.md` |

*Resolve open items here and cross-link the doc that implements them.*

## Spec sections to write (holes filled by the roadmap)

These are subjects the numbered docs do not cover today; each ADR above decides the design, and the section is written when the implementing workstream lands (the ADR then flips to Accepted).

| Section | Doc(s) | Decided by |
|---------|--------|------------|
| Output buffering (`ob_*` levels, sinks, `php://output` vs `php://stdout`), headers/cookies API, superglobal seeding per SAPI | `09-runtime-sapi.md` | ADR-033, ADR-022 |
| `eval` semantics (unit naming `file(line) : eval()'d code`, scope sharing, `ParseError`) | `01-frontend.md`, `05-bytecode-isa.md` | ADR-020, ADR-019 |
| `$GLOBALS` and superglobals (read-only snapshot, write-through ops, symtab binding) | `02-value-model.md`, `06-interpreter.md` | ADR-019 |
| Generator ops `Yield`/`YieldFrom`/`GenReturn`, `GeneratorState`, parked frames | `05-bytecode-isa.md` | ADR-018 |
| Tokenizer (`rphp-tokenizer`: Zend scanner states, token ids, quirk list, `TOKEN_PARSE`, `PhpToken`) | `01-frontend.md` (§4 replaced), `08-stdlib-ext.md` | ADR-029 |
| Desugaring catalogue v2 (stabilize rule, the full lowering table, what stays a node) | `01-frontend.md` (§6.2 replaced) | ADR-026 |
| Object handler table for native classes (`get_iterator`, `count`, `dim_*`, `debug_info`, `clone`, `serialize`, `compare`, `cast`, `do_operation`) | `08-stdlib-ext.md` | ADR-021 |
| Streams crate API (`rphp-streams`: `Stream`/`PhpStream`/`Wrapper`/`DirStream`, built-in wrappers, contexts, bucket-brigade filters, stat/realpath caches) | `08-stdlib-ext.md` (§15.5 rewritten) | ADR-012, ADR-033 |

