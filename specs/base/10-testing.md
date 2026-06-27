# 10 — Testing, Observability & Security

**Status:** stable
**Source sections:** `base-idea.md` §19 (observability & tooling), §20 (testing, correctness, benchmarking), §21 (security & resource limits)
**Reads with:** [00-overview.md](00-overview.md), [04-memory-gc.md](04-memory-gc.md) (arena accounting, safepoints), [06-interpreter.md](06-interpreter.md) (Tier-0 reference), [07-jit.md](07-jit.md) (deopt, tiering), [08-stdlib-ext.md](08-stdlib-ext.md) (coverage metric, WASM sandbox), [09-runtime-sapi.md](09-runtime-sapi.md) (admin endpoint, capabilities), [decisions.md](decisions.md) (ADR-004, ADR-008, ADR-010)

Correctness in rPHP is **empirical, not asserted**: the engine is judged against running PHP, not against a prose spec. Three oracles do the judging — the php-src `.phpt` corpus, differential execution against stock PHP, and fuzzing — and one metric, `% .phpt passing`, is the north star. The decisive refinement over the baseline is **ADR-008**: the differential oracle is *fuzzy + allowlisted*, not byte-exact, because the corpus itself is fuzzy and exact-equality would mis-train the headline number. This doc also pins observability (the `--emit` introspection surface, JIT/deopt logs, probes, counters) and the enforced resource limits.

---

## Part A — Testing & correctness (§20)

### 20.1 The `.phpt` corpus — the north star

The php-src test suite is the **primary compatibility oracle**. `rphp-test` (driven by `xtask`) executes `.phpt` files and reports **pass percentage as a tracked, must-not-regress CI metric** — the single number that says "how much PHP does rPHP actually run." A milestone gate cannot be crossed if the global `.phpt` pass rate regresses ([00-overview.md](00-overview.md) §4); per-extension slices feed ADR-004 (§20.8).

A `.phpt` file is a flat section format. The runner parses the sections it must honor and skips the editorial ones:

| Section | Meaning | Runner action |
|---------|---------|---------------|
| `--TEST--` | one-line title | report label |
| `--DESCRIPTION--` / `--CREDITS--` | prose | ignored |
| `--SKIPIF--` | PHP snippet printing `skip <reason>` | run first; if it skips, the test is **skipped, not failed** |
| `--EXTENSIONS--` | required extensions | skip if a required `ext-*` is feature-gated out |
| `--INI--` | per-test ini directives | applied to the child config (e.g. `memory_limit`, `precision`) |
| `--ENV--` / `--GET--` / `--POST--` / `--STDIN--` | request/CLI inputs | wired into the SAPI invocation |
| `--FILE--` | the program body | **the code under test** |
| `--EXPECT--` | exact expected stdout | byte-equal compare |
| `--EXPECTF--` | expected output with `printf`-style wildcards | **fuzzy** compare (below) |
| `--EXPECTREGEX--` | expected output as a regex | regex compare |
| `--CLEAN--` | teardown snippet | run, output discarded |
| `--XFAIL--` | known-broken marker | expected-fail; an unexpected **pass** is also flagged |

`--EXPECTF--` / `--EXPECTREGEX--` are the load-bearing detail. The corpus does **not** assume byte-exact output: `%d` matches an integer run, `%s` a non-newline run, `%f` a float, `%a` any text incl. newlines, `%w` whitespace, `%i`/`%x` signed/hex integers, `%e` the platform directory separator, `%c` one char. The runner implements the full wildcard set so that a test author's intentional tolerance is honored rather than scored as a divergence. This is *why* the differential oracle below is fuzzy too — the corpus is the ground truth and the corpus is fuzzy.

The runner forks a real rPHP CLI process per test (isolation, accurate exit codes, `--INI--` fidelity), captures stdout/stderr/exit-status, and diffs against the expected section. Tests run in parallel across the worker pool; a content-addressed result cache skips unchanged `(test, engine-hash)` pairs so CI only re-runs what moved.

### 20.2 Differential testing — fuzzy + allowlisted (ADR-008)

Generated and real-world PHP snippets are run through **both** rPHP and a pinned stock PHP, and their **stdout, exit status, emitted warnings/notices, and thrown exception class+message** are compared. This catches divergence the static corpus never enumerated.

The comparison is **not byte-equality** (this is the key deviation, ADR-008). It is two-layered:

1. **Fuzzy match**, using the same `EXPECTF`/`EXPECTREGEX` wildcard machinery as §20.1 — applied either to a hand-written `%`-template or to a normalizing transform that collapses known-volatile spans (addresses, object ids, timing) before compare.
2. **Curated divergence allowlist** — output that is *legitimately* environment-dependent is whitelisted, and **each entry cites why it diverges**. A match against an allowlist entry is a pass; anything outside the allowlist that fails the fuzzy match is **a bug in rPHP**.

| Allowlist category | Example | Why it legitimately diverges |
|--------------------|---------|------------------------------|
| **Float formatting / precision** | `var_dump(0.1+0.2)`, `serialize(1/3)` | IEEE-754 rounding + `precision`/`serialize_precision` ini and dtoa shortest-round-trip differ between libc/implementations; not a semantic difference |
| **Locale-sensitive output** | `strftime`, `number_format`, `Collator`, `setlocale` | ICU version + locale data (native ICU vs `icu4x` subset, ADR-003) legitimately differ across builds |
| **Error / exception message wording** | `TypeError` argument text, deprecation phrasing | message *strings* are not a stability contract in PHP; the **class, code, and being-thrown** are compared, the prose is not |
| **Hash-order edge cases** | unseeded `array_rand`, set iteration after collisions, `spl_object_id` | order/identity depends on hash seed and allocation address, which are implementation details |
| **Platform-dependent values** | `__FILE__`, `getmypid`, `time()`, `tempnam`, `PHP_OS`, resource ids | paths, PIDs, timestamps, and handle numbers are environment facts, not program semantics |

Every allowlist entry is a row in a checked-in table with a justification and, where possible, a normalization rule rather than a blanket skip — the goal is to **shrink** the allowlist over time, not to hide failures in it. The rationale is in ADR-008: strict byte-equality would manufacture false failures (the corpus authors already declined to assume it) and would **mis-train the headline compatibility metric**, making it both noisy and dishonest. Fuzzy-plus-allowlist keeps the number *actionable*: a red diff is a real semantic divergence to fix.

### 20.3 Fuzzing

Two fuzzers, both on the CI nightly:

- **`cargo-fuzz` on the lexer and parser.** The contract is *total robustness on arbitrary bytes*: **no panics, no UB, no unbounded memory** on any `&[u8]` input. The fault-tolerant parser ([01-frontend.md](01-frontend.md)) must produce error nodes and terminate, never abort. This is the security floor for accepting untrusted source (WASM/edge embedding, §C).
- **Structure-aware differential fuzzer.** A grammar-aware generator emits *valid-ish* PHP programs (typed params, arrays, closures, arithmetic, control flow) and runs them through the §20.2 differential harness, diffing rPHP against stock PHP. This explores the semantic surface the hand-written corpus misses; any non-allowlisted divergence is minimized to a regression `.phpt` and added to the corpus.

### 20.4 Snapshot tests at every IR boundary

Insta-style snapshot tests pin the artifact at **every IR boundary** so a refactor's blast radius is visible in the diff, not discovered at runtime:

| Boundary | Snapshot | Producing crate / doc |
|----------|----------|------------------------|
| AST | pretty-printed typed AST | `rphp-ast` ([01-frontend.md](01-frontend.md)) |
| HIR | resolved/desugared HIR | `rphp-hir` ([01-frontend.md](01-frontend.md)) |
| Bytecode | disassembled register ISA + IC-slot table | `rphp-bytecode` ([05-bytecode-isa.md](05-bytecode-isa.md)) |

A change that perturbs lowering, desugaring, register allocation, or IC-slot assignment shows up as a reviewed snapshot delta. These are *structural* tests (does the compiler produce what we intend) complementing the *behavioral* `.phpt`/differential tests (does the program do what PHP does).

### 20.5 Miri over the `no_std` core

The `unsafe` in rPHP is enumerated and confined: NaN-free value access ([02-value-model.md](02-value-model.md)), GC/arena ([04-memory-gc.md](04-memory-gc.md)), and JIT codegen ([07-jit.md](07-jit.md)). **Miri** runs over the `#![no_std]` core (`rphp-value`, `rphp-gc`, `rphp-heap`, the interpreter core) to detect UB — invalid aliasing, out-of-bounds, uninitialized reads, provenance violations — that normal tests miss. Miri cannot execute the JIT's emitted machine code or FFI, so those layers are covered instead by ASan/UBSan builds and by the deopt-stress equivalence check (§20.7), which proves the compiled path matches the Miri-clean interpreter bit-for-bit.

### 20.6 Benchmarking & the regression gate (§20, §23)

`criterion` drives a microbenchmark suite plus macro workloads. **Every run reports the ratio against the Zend + tracing-JIT baseline** — the competitive bar from §0.3:

```
php -d opcache.enable_cli=1 -d opcache.jit_buffer_size=64M -d opcache.jit=tracing
```

The kernel set is the benchmarks-game suite (CPU-bound, the launch-claim territory) plus a framework-bootstrap macro-benchmark:

| Kernel | Stresses | Ties to §23 target |
|--------|----------|--------------------|
| mandelbrot | tight float loops, no allocation | numeric kernels — meet/beat after warmup |
| n-body | float arrays, method-free arithmetic | numeric kernels — the launch claim |
| fannkuch-redux | int arrays, permutation, swaps | tight array loops — packed arrays |
| spectral-norm | float arrays + nested loops, vectorizable | numeric kernels — auto-vectorized typed-packed |
| binary-trees | allocation churn, GC, object graphs | exercises arena + refcount + cycle GC |
| regex-redux | PCRE2 throughput, large strings | stdlib-bound — PCRE2 binding parity |
| framework-bootstrap (macro) | autoload, DI, routing, request lifecycle | framework throughput — parity *milestone*, not launch claim |

**CI fails on a throughput regression beyond noise.** The noise floor is established per-kernel from criterion's variance estimate; a run that regresses a kernel's ratio-vs-Zend past that band blocks the merge, the same way a `.phpt` regression does. This makes §23's targets *enforced*, not aspirational: the numeric/array kernels must hold "meet or beat after warmup," and the macro benchmark tracks the framework-parity milestone without gating the launch claim on it.

### 20.7 Deopt-stress mode — the tiering-correctness guarantee

Tiering is only correct if the optimized path and the interpreter fallback are **observably identical**. A **deopt-stress mode** proves this adversarially: it forces a **guard failure at every `SafePoint`**, so every speculative region deoptimizes back to the interpreter ([06-interpreter.md](06-interpreter.md)) at the first opportunity, on every run. The harness then asserts the program's full observable behavior — stdout, warnings, exceptions, refcount-timed `__destruct` order, final state — is **bit-identical** whether it ran optimized, baseline, or interpreted.

This is the load-bearing test for the whole JIT thesis ([07-jit.md](07-jit.md)): it validates that deopt metadata reconstructs interpreter state correctly at *every* bytecode index, that the call ABI is genuinely shared across tiers, and that refcount-elision (ADR-010) never changes destructor timing. Combined with running the entire `.phpt` corpus under `--deopt-stress`, `--tier0-only`, and `--tier2-aggressive`, it turns "the tiers agree" from a hope into a checked invariant.

### 20.8 Per-extension coverage metric (ADR-004)

Full php-src stdlib parity is a **committed goal** (ADR-004), so coverage is a first-class, **must-not-regress** CI metric, tracked **per extension** along two axes:

- **`% functions implemented`** — implemented vs. the extension's full php-src function/class surface.
- **`% .phpt passing`** — the extension's slice of the corpus (§20.1).

Both are reported per-extension and rolled up, with a burn-down owned by the parallel stdlib track ([08-stdlib-ext.md](08-stdlib-ext.md)). A PR may *raise* either number freely; a PR that *lowers* either is blocked. This is what converts "we'll get to the stdlib" into a measured, ratcheting commitment — the mitigation for the compatibility cliff that sank prior PHP reimplementations.

---

## Part B — Observability & tooling (§19)

### 19.1 Structured tracing

`tracing` spans instrument the engine end to end: **one span per compilation stage** (lex → parse → lower → compile → tier-1 → tier-2) and **one span per request** in the server SAPI, nested so a slow request attributes its time to lex/compile/JIT/execute/stdlib. Spans carry the `FileId`/`SymbolId`/region-id so a trace ties directly back to source and bytecode.

### 19.2 Pipeline introspection — `--emit`

`rphp --emit=<stage>` prints any pipeline artifact, the rustc `-Z`-style introspection that makes the engine debuggable end to end:

| `--emit=` | Artifact | Crate / doc |
|-----------|----------|-------------|
| `tokens` | lexer token stream (with trivia) | `rphp-lexer` ([01-frontend.md](01-frontend.md)) |
| `ast` | typed AST | `rphp-ast` ([01-frontend.md](01-frontend.md)) |
| `hir` | resolved/desugared HIR | `rphp-hir` ([01-frontend.md](01-frontend.md)) |
| `bytecode` | register ISA disassembly + IC slots + metadata | `rphp-bytecode` ([05-bytecode-isa.md](05-bytecode-isa.md)) |
| `cranelift` | Tier-2 Cranelift IR for a region | `rphp-jit-opt` ([07-jit.md](07-jit.md)) |
| `asm` | final native code (with PHP source mapping) | JIT backend ([07-jit.md](07-jit.md)) |

The same stages back the snapshot tests (§20.4), so `--emit` output *is* the snapshot format — what a developer inspects by hand is what CI pins.

### 19.3 JIT dump & deopt log

- **JIT dump** in `perf` (Linux `jit-*.dump`) and **VTune** formats, so optimized frames appear in standard profilers **with PHP-level symbols** (function name + source span), not as anonymous JITed blobs. Backed by the per-instruction spans threaded from the front end ([01-frontend.md](01-frontend.md) §3).
- **Deopt log** — *the single most important signal for tuning speculation.* Every deoptimization is traceable to **its failing guard, the bytecode site, the speculated-vs-observed type, and the region** that bailed ([07-jit.md](07-jit.md)). A site that deopts repeatedly is a misspeculation to fix (widen the type, drop the guard, or blacklist the speculation); the log is how that megamorphic/polymorphic reality surfaces.

### 19.4 Probes & counters

- **USDT/DTrace probes** fire on `call`, `compile`, `deopt`, and `gc`, so production behavior is observable with `dtrace`/`bpftrace`/`perf` at near-zero cost when disarmed.
- **Counters** exported over the **server SAPI admin endpoint** ([09-runtime-sapi.md](09-runtime-sapi.md)): per-function execution **tier**, **IC monomorphism rate** (the headline health number for speculation — falling monomorphism predicts deopt churn), **arena high-water mark** ([04-memory-gc.md](04-memory-gc.md)), and **GC cycles collected**. These let an operator see, live, whether a workload is staying on the fast path or thrashing the tiers.

---

## Part C — Security & resource limits (§21)

Limits are enforced at the two chokepoints the architecture already owns: the **single arena accounting hook** (memory) and **`SafePoint`s** (time). Nothing escapes them, including hot JITed code.

| Limit | Enforced at | Mechanism | On breach |
|-------|-------------|-----------|-----------|
| `memory_limit` | arena accounting hook ([04-memory-gc.md](04-memory-gc.md), ADR-011) | the single per-isolate counter, checked before every grow across all three allocators | **catchable** error (`\Error`), unwinds normally |
| `max_memory_limit` (8.5) | same hook | hard process ceiling above `memory_limit`; userland may *not* raise `memory_limit` past it | allocation refused past the ceiling; catchable error |
| `max_execution_time` | `SafePoint`s + loop back-edges ([04-memory-gc.md](04-memory-gc.md) §safepoints) | a timer thread sets an **interrupt flag**; safepoints do a relaxed load + predicted-not-taken branch | interrupt raised at the next safepoint |

The memory hook is the *same* counter that ADR-011 routes all allocation through (slab, arena, mimalloc), so the limit is enforced **uniformly** regardless of which allocator served the bytes. The 8.5 `max_memory_limit` is a true hard cap: code cannot `ini_set('memory_limit', ...)` its way past it.

The execution-time guarantee is the reason safepoints sit on **back-edges**: a hot loop fully resident in Tier-2 native code still hits a safepoint every iteration, so `max_execution_time` is honored even by code that never returns to the interpreter — closing the classic "infinite JITed loop ignores the timeout" hole. The per-back-edge cost is one relaxed flag load (ADR-010 / safepoint contract).

Further controls:

- **Capability-scoped SAPIs** ([09-runtime-sapi.md](09-runtime-sapi.md)). Filesystem, network, and process access are mediated by a capability set the embedder configures; the **WASM SAPI defaults to deny-all** plus explicit grants. A request cannot reach a resource its SAPI was not granted.
- **Sandboxed extensions via the WASM component model** ([08-stdlib-ext.md](08-stdlib-ext.md)). Untrusted/multi-tenant extension code runs as a WASM component, capability-confined to its declared WIT interface with no access to host memory — the modern answer to "a native extension can corrupt the engine."
- **Fatal-error backtraces** (new in 8.5) are produced from **frame metadata** ([05-bytecode-isa.md](05-bytecode-isa.md)) — the same per-instruction spans and frame layout the interpreter and JIT already maintain — so a fatal now yields an actionable stack, not just a one-line message.

---

## Deviations from base-idea.md

- **§20 "divergence is a bug by definition" → fuzzy + allowlisted oracle (ADR-008).** *The key deviation.* The differential oracle uses `EXPECTF`/`EXPECTREGEX`-style wildcard matching plus a **curated, documented divergence allowlist** (float formatting/precision, locale-sensitive output, error/exception message wording, hash-order edge cases, platform-dependent paths/PIDs/timestamps), each entry citing why it legitimately diverges. Byte-exact equality is rejected because the `.phpt` corpus itself is fuzzy, so exact-match would manufacture false failures and **mis-train the headline compatibility metric**. Outside the allowlist, divergence is still a bug.
- **Per-extension coverage is a first-class must-not-regress CI metric (ADR-004).** §20 tracked `.phpt` pass rate globally; this adds the per-extension `% functions` + `% .phpt` axes as a gating, ratcheting number ([08-stdlib-ext.md](08-stdlib-ext.md)).
- **Limits unified onto the existing chokepoints (ADR-010/ADR-011).** §21's `memory_limit`/`max_memory_limit` route through the *single* arena accounting hook, and `max_execution_time` through the *same* `SafePoint`s the GC and deopt already use — no separate enforcement machinery.

## Open questions

- **Divergence-allowlist normalization vs. skip.** How aggressively to replace blanket allowlist skips with precise normalization transforms (so a category still meaningfully tests *non-volatile* output) — tune as the differential corpus grows in M1–M3.
- **Float-formatting fidelity target.** Whether to match Zend's exact dtoa shortest-round-trip output (eliminating the float allowlist category) or accept the documented divergence — depends on the cost of bug-compatible dtoa; revisit when `serialize`/`var_export` parity is measured.
- **Noise-floor methodology for the macro benchmark.** The framework-bootstrap workload has higher variance than the kernels; the regression-gate band for it needs a dedicated statistical model so it gates honestly without flapping — settle in M4 under the server SAPI.
- **PMU-driven profiling in CI.** Whether hotness/coverage attribution may use the CPU PMU (PEBS/LBR, §12.3) in CI runners, where PMU access is often restricted — fall back to software counters where unavailable.
