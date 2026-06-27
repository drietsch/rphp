# 00 — Architecture Overview & Doc Map

**Status:** stable
**Source sections:** `base-idea.md` §1 (pipeline), §2 (crate topology), §22 (build/features/platforms)
**Reads with:** [decisions.md](decisions.md) (authoritative on cross-cutting choices)

This is the entry point to the rPHP design specs. It maps the documents, restates the execution pipeline and crate topology as enforceable contracts, and pins the build/toolchain/target envelope. Per-subsystem depth lives in the numbered docs; cross-cutting choices live in [decisions.md](decisions.md).

---

## Document map & reading order

| # | Doc | Covers | Status |
|---|-----|--------|--------|
| — | [base-idea.md](base-idea.md) | v0.1 baseline (frozen, historical) | frozen |
| — | [decisions.md](decisions.md) | ADR log; resolves §25 + all deviations | living |
| 00 | this doc | pipeline, crates, build/targets, doc map | stable |
| 01 | [01-frontend.md](01-frontend.md) | source, diagnostics, lexer, parser/AST, HIR | stable |
| 02 | [02-value-model.md](02-value-model.md) | `Value` cell, tags, conversions, COW, unboxing boundary | stable |
| 03 | [03-heap-types.md](03-heap-types.md) | strings, arrays, objects/shapes, closures | stable |
| 04 | [04-memory-gc.md](04-memory-gc.md) | arena, slab, refcount, cycle GC, allocator routing | stable |
| 05 | [05-bytecode-isa.md](05-bytecode-isa.md) | register ISA, encoding, metadata, frame/call ABI | stable |
| 06 | [06-interpreter.md](06-interpreter.md) | Tier-0 dispatch, inline caches, profiling | stable |
| 07 | [07-jit.md](07-jit.md) | Tier-1 copy-and-patch, Tier-2 Cranelift, deopt, AOT | stable |
| 08 | [08-stdlib-ext.md](08-stdlib-ext.md) | stdlib (full-coverage plan), extension model | stable |
| 09 | [09-runtime-sapi.md](09-runtime-sapi.md) | isolates, fibers, async reactor, SAPIs | stable |
| 10 | [10-testing.md](10-testing.md) | `.phpt`, differential oracle, fuzzing, bench, observability, security | stable |

**Recommended reading order for a new contributor:** 00 → 02 → 03 → 04 (the value/memory core) → 05 → 06 → 07 (execution) → 01 (front end) → 08 → 09 → 10. Skim [decisions.md](decisions.md) first if you only want the *what* and *why* of contested choices.

---

## 1. Execution pipeline (§1)

```
source bytes
   │ lex            01-frontend
   ▼
tokens ─ parse ─▶ typed AST (rphp-ast)        01-frontend  (mago-syntax adapter, ADR-007)
   │ lower
   ▼
HIR (resolved, desugared, const-folded)        01-frontend
   │ compile
   ▼
register bytecode + IC slots + metadata        05-bytecode-isa
   │
   ▼
┌──────────────── runtime ────────────────┐
│ Tier 0 interpreter ──profile──▶ Tier 1   │   06-interpreter, 07-jit
│   ▲ deopt │ OSR/hot       copy-and-patch  │
│   │       ▼                    │          │
│ value/gc/heap  ◀── guards ── Tier 2 opt   │   02/03/04, 07-jit
│ (02,03,04)        deopt md   (Cranelift)  │
└──────────────────────────────────────────┘
   │
   ▼  SAPI: cli │ server │ fcgi │ embed │ wasm    09-runtime-sapi
```

**Stage/artifact contract** (what is cacheable is the persistence boundary):

| Stage | In | Out | Cacheable |
|-------|----|-----|-----------|
| Lex | `&[u8]` | tokens | no |
| Parse | tokens | typed AST (CST deferred, ADR-007) | yes (AST) |
| Lower | AST | HIR | yes |
| Compile | HIR | bytecode + metadata | **yes — on-disk code cache** |
| Tier 0 | bytecode | effects + profile | n/a |
| Tier 1 | bytecode + profile | native stub | per-process |
| Tier 2 | region + feedback | optimized native + deopt map | per-process |

The compiled-bytecode artifact is the persistent unit, **content-addressed** by `hash(source bytes, compiler version, feature flags)` and stored in an on-disk code cache (the modern OPcache equivalent) so cold processes skip lex/parse/compile.

---

## 2. Crate topology (§2)

Single Cargo workspace, monorepo style. **Dependencies point strictly downward**; this is an enforced invariant, not a guideline (CI checks the dependency graph).

```
rphp/
├── Cargo.toml                  # workspace
├── rust-toolchain.toml         # PINNED NIGHTLY (ADR-013)
├── xtask/                      # codegen (opcodes, stencils, arginfo), phpt runner driver
├── crates/
│   ├── rphp-span/              # byte spans, source ids — no deps
│   ├── rphp-source/            # source files, line maps, virtual FS
│   ├── rphp-diagnostics/       # error model, codes, renderer
│   ├── rphp-intern/            # string interner (global + per-isolate)
│   ├── rphp-lexer/             # byte-level lexer (or mago adapter in M0, O-1)
│   ├── rphp-ast/               # OWNED typed-AST contract (ADR-007)
│   ├── rphp-parser/            # mago-syntax adapter → rphp-ast (M0); own parser later
│   ├── rphp-hir/               # resolved + desugared IR
│   ├── rphp-resolve/           # name resolution, scoping, autoload points
│   ├── rphp-bytecode/          # register ISA, encoding, metadata
│   ├── rphp-compiler/          # HIR → bytecode
│   ├── rphp-value/   [no_std]  # Value cell, tags, conversions (sealed API, ADR-005)
│   ├── rphp-gc/      [no_std]  # arena, slab, refcount, cycle collector
│   ├── rphp-heap/    [no_std]  # string, array, object, closure
│   ├── rphp-shape/             # hidden classes / inline-cache infra
│   ├── rphp-runtime/ [no_std core] # interpreter, frames, call ABI
│   ├── rphp-jit-baseline/      # Tier 1 copy-and-patch
│   ├── rphp-jit-opt/           # Tier 2 Cranelift + speculation + deopt
│   ├── rphp-profile/           # counters, type feedback, edge weights
│   ├── rphp-analyze/           # whole-program type inference (opt-in first, O-4)
│   ├── rphp-stdlib/            # standard library, feature-gated by ext
│   ├── rphp-ext-abi/           # extension traits + C ABI + WASM component host
│   ├── rphp-embed/             # public Rust embedding API
│   ├── rphp-ffi/               # C ABI surface (librphp)
│   ├── rphp-sapi-cli/  -server/  -fcgi/  -wasm/
│   ├── rphp-test/              # phpt runner, differential harness
│   └── rphp-bench/             # criterion + benchmarks-game suite
└── tools/rphp/                 # the binary (depends on sapi-cli)
```

### 2.1 Modularity contracts (enforced invariants)

1. Crates at/below `rphp-value` are `#![no_std]` and **allocator-pluggable** (allocator is a generic parameter). They assume no filesystem, clock, or threads.
2. `rphp-runtime` exposes execution via a `Vm` trait; Tier 1/Tier 2 register as `Tier` implementations. The interpreter has **no compile-time dependency** on either JIT crate. `--no-default-features` ⇒ a pure interpreter.
3. The stdlib is a registry of `NativeFn` descriptors. Removing an extension is removing a feature flag, never editing the engine (see `08-stdlib-ext.md`).
4. SAPIs depend on `rphp-embed` **only** — they cannot reach runtime internals. If a SAPI needs something, the public embedding API grows; the abstraction does not leak.

---

## 3. Build, toolchain, features, targets (§22)

### 3.1 Toolchain — pinned nightly (ADR-013, deviates from §22)

The workspace pins a **specific nightly** via `rust-toolchain.toml` to enable `become` (guaranteed tail calls) for the threaded interpreter (ADR-001) and other nightly features the `no_std` core needs. "MSRV" = the pinned nightly version, tested in CI and bumped deliberately. A `stable`-buildable subset remains via the `dispatch-portable` feature (the `loop { match }` core), at a measured perf cost.

### 3.2 Feature flags

| Flag | Effect |
|------|--------|
| (default) | interpreter + `jit-baseline` + core stdlib + `std` |
| `jit-baseline` | Tier-1 copy-and-patch |
| `jit-opt` | Tier-2 (pulls Cranelift) |
| `dispatch-portable` | `loop { match }` dispatch (stable-toolchain path) |
| `analyze` | whole-program type inference (`rphp-analyze`) |
| `aot` | ahead-of-time native for provably-closed regions (ADR-006) |
| `std` | filesystem/clock/threads; CLI & server turn it on |
| `server` | isolates + async reactor + worker SAPI |
| `wasm` | wasm32 build: interpreter + copy-and-patch only |
| `ext-<name>` | one flag per stdlib extension (json, pcre, intl, …) |
| `--no-default-features` | pure `no_std` interpreter core |

### 3.3 Targets (§22)

- **x86-64:** AVX2 is the hard floor; an AVX-512 (VBMI2/VAES/GFNI) optimized path targets Zen 4/5 and Sapphire Rapids-class parts. Pre-AVX2 x86 is **unsupported by design** — no scalar-only fallback build.
- **aarch64:** NEON baseline plus an SVE2 path on Apple silicon and Graviton 4-class parts.
- **OS:** Linux, macOS, Windows with full JIT.
- **wasm32:** `wasm32-unknown-unknown` / WASI; interpreter + copy-and-patch only (no host Cranelift inside WASM); `icu4x` locale subset (ADR-003).

`xtask` drives codegen (opcode tables, copy-and-patch stencils, arginfo), the `.phpt` runner, and release packaging.

---

## 4. Milestones & the stdlib track (§24 + ADR-004)

The engine milestones are unchanged from baseline §24 (M0 front end → M1 interpreter → M2 baseline JIT → M3 optimizing JIT → M4 server → M5 reach). **Two adjustments:**

1. **The stdlib is a parallel, first-class track** (ADR-004), not folded into M1–M4. It has its own burn-down (per-extension `% functions` and `% .phpt`), staffed and scheduled independently, because full parity is a committed goal and the dominant schedule risk.
2. **M0 bootstraps on `mago-syntax`** (ADR-007); the owned parser/CST is a later, optional replacement.

Every milestone gate: `.phpt` pass rate must not regress, and benchmark ratios versus `php -d opcache.jit=tracing` must move the right direction (see `10-testing.md`).

---

## Deviations from base-idea.md

- **§22 "stable Rust" → pinned nightly** (ADR-013), to enable `become`.
- **§2 `rphp-parser` is a `mago-syntax` adapter in M0** (ADR-007); `rphp-ast` owns the AST contract; lossless CST deferred.
- **§24 stdlib is a parallel committed track** (ADR-004), not demand-best-effort.

## Open questions

- O-1 (own vs reuse lexer in M0) — see [decisions.md](decisions.md); resolved in `01-frontend.md`.
- Dependency-graph enforcement mechanism (cargo-deny vs a custom `xtask` check) — `xtask` detail, pick during M0.
