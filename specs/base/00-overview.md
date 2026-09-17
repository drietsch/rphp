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
| 01 | [01-frontend.md](01-frontend.md) | source, diagnostics, tokenizer, parser/AST (mago adapter), HIR | stable (roadmap amendments pending, ADR-025/026/029) |
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
tokens ─ parse ─▶ typed AST (rphp-ast)        01-frontend  (mago-syntax adapter, ADR-007/ADR-025)
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
| Lex | `&[u8]` | tokens (inside `mago-syntax`; `rphp-tokenizer` serves `ext/tokenizer` only, ADR-029) | no |
| Parse | tokens | typed AST (CST deferred, ADR-007) | yes (AST) |
| Lower | AST | HIR | yes |
| Compile | HIR | bytecode + metadata | **yes — on-disk code cache** |
| Tier 0 | bytecode | effects + profile | n/a |
| Tier 1 | bytecode + profile | native stub | per-process |
| Tier 2 | region + feedback | optimized native + deopt map | per-process |

The compiled-bytecode artifact is the persistent unit, **content-addressed** by `hash(source bytes, compiler version, feature flags)` and stored in an on-disk code cache (the modern OPcache equivalent) so cold processes skip lex/parse/compile.

---

## 2. Crate topology (§2)

Single Cargo workspace, monorepo style. **Dependencies point strictly downward**; this is an enforced invariant, not a guideline (CI checks the dependency graph). Per **ADR-014** the order through the middle of the graph is

```
rphp-sapi-cli / -server / -fcgi ─▶ rphp-embed ─▶ rphp-stdlib (bundle) ─▶ crates/ext/rphp-ext-* ─▶ rphp-runtime ─▶ rphp-bytecode ─▶ rphp-value
                                        └──────▶ rphp-compiler ─▶ rphp-hir ─▶ rphp-parser (mago adapter) / rphp-ast ───┘
```

i.e. the **stdlib sits above the runtime** (natives receive `Ctx(&mut Interp)`; the `Host` trait is gone), the **compiler has no stdlib dependency** (bytecode carries names, resolved at runtime — ADR-020/030), and **SAPIs depend on `rphp-embed` only**. Crates marked *(later)* below are specified but not on the Symfony-ladder critical path.

```
rphp/
├── Cargo.toml                  # workspace
├── rust-toolchain.toml         # PINNED NIGHTLY (ADR-013; bump policy tracks mago's MSRV, ADR-028)
├── xtask/                      # codegen (arginfo/consts/ini from the manifest, opcodes, stencils), coverage, phpt/ladder/corpus/missing drivers (ADR-030/031/034)
├── manifest/php-8.5.0/         # committed oracle dump: functions/classes/constants/ini JSON (ADR-030)
├── tools/
│   ├── rphp/                   # the binary (depends on sapi-cli); tests/differential.rs (ADR-034)
│   ├── manifest/dump.php       # produces manifest/ with the installed php (ADR-030)
│   ├── php-src/                # sparse php-8.5.0 checkout script for the .phpt corpus (ADR-034)
│   └── fixtures/               # composer setup for the ladder rungs (ADR-034)
├── fixtures/ladder/L<n>-<name>/ # Symfony ladder rungs L0–L9: composer.lock + rung.toml (ADR-034)
├── crates/
│   ├── rphp-span/              # byte spans, source ids — no deps
│   ├── rphp-source/            # source files, line maps, virtual FS
│   ├── rphp-diagnostics/       # error model, codes, renderer
│   ├── rphp-intern/            # string interner (global + per-isolate)
│   ├── rphp-lexer/             # TO BE REMOVED by F3 — replaced by the mago adapter + rphp-tokenizer (ADR-025/029)
│   ├── rphp-tokenizer/         # owned Zend-exact scanner: token_get_all / PhpToken; not used for parsing (ADR-029)
│   ├── rphp-ast/               # OWNED typed-AST contract, v2 (ADR-007, ADR-026)
│   ├── rphp-parser/            # mago-syntax adapter + superset conformance pass → rphp-ast (ADR-025)
│   ├── rphp-hir/               # resolve + desugar; newtype over rphp-ast, absorbs rphp-resolve (ADR-026)
│   ├── rphp-bytecode/          # register ISA, encoding, CompiledUnit + declaration metadata (ADR-016/020/027)
│   ├── rphp-compiler/          # HIR → bytecode; no stdlib dependency (ADR-014)
│   ├── rphp-value/             # Value cell, tags, conversions; interim safe-Rc heap incl. Ref/Uninit/Resource, arrays, objects (ADR-005, ADR-015)
│   ├── rphp-gc/      [no_std]  # arena, slab, refcount, cycle collector — DEFERRED (ADR-015/024)
│   ├── rphp-heap/    [no_std]  # string, array, object, closure — DEFERRED; lives in rphp-value meanwhile (ADR-015)
│   ├── rphp-shape/             # hidden classes / inline-cache infra (later)
│   ├── rphp-runtime/           # ENGINE API: Interp, frames, call ABI, class model, diagnostics, core classes; forbid(unsafe) (ADR-014/016–024)
│   ├── rphp-ext-api/           # extension descriptor types: FnSig/NativeFn/Extension/TypeMask/FnFlags — no engine dep (ADR-030/031)
│   ├── ext/rphp-ext-<name>/    # one crate per PHP extension, php-src file naming, generated/ + catalog.toml (ADR-031)
│   ├── rphp-stdlib/            # the BUNDLE: extensions() behind ext-<name> features; COVERAGE.md (ADR-031)
│   ├── rphp-streams/           # Stream/Wrapper/filter layer: file://, php://, data://, glob://, sockets (ADR-012/033)
│   ├── rphp-embed/             # Engine/Interp/Request/OutputSink/Response — the only crate SAPIs see (ADR-014/033)
│   ├── rphp-sapi-cli/          # php-compatible CLI, streaming output (ADR-033)
│   ├── rphp-sapi-server/       # rphp -S: hand-rolled HTTP/1.1, thread-per-connection (ADR-033)
│   ├── rphp-sapi-fcgi/         # FastCGI, same request-binding layer (ADR-033)
│   ├── rphp-test/              # phpt runner + differential harness library (ADR-034)
│   ├── rphp-fibers/            # (provisional name) feature-gated; the only crate allowed `unsafe` stack switching (ADR-018)
│   ├── rphp-jit-baseline/      # Tier 1 copy-and-patch (later)
│   ├── rphp-jit-opt/           # Tier 2 Cranelift + speculation + deopt (later)
│   ├── rphp-profile/           # counters, type feedback, edge weights (later)
│   ├── rphp-analyze/           # whole-program type inference (opt-in first, O-4) (later)
│   ├── rphp-ext-abi/           # C ABI + WASM component host (later)
│   ├── rphp-ffi/               # C ABI surface (librphp) (later)
│   ├── rphp-sapi-wasm/         # (later)
│   └── rphp-bench/             # criterion + benchmarks-game suite (later)
```

### 2.1 Modularity contracts (enforced invariants)

1. Crates at/below `rphp-value` are `#![no_std]` and **allocator-pluggable** (allocator is a generic parameter). They assume no filesystem, clock, or threads. *(Interim, ADR-015: `rphp-value` is a safe-`Rc` heap and may use `alloc`; the `no_std`/allocator-pluggable contract resumes when `rphp-heap`/`rphp-gc` land.)*
2. `rphp-runtime` exposes execution via a `Vm` trait; Tier 1/Tier 2 register as `Tier` implementations. The interpreter has **no compile-time dependency** on either JIT crate. `--no-default-features` ⇒ a pure interpreter.
3. The stdlib is a registry of `NativeFn` descriptors (types in `rphp-ext-api`, one crate per extension under `crates/ext/`, bundled by `rphp-stdlib` — ADR-031). Removing an extension is removing a feature flag, never editing the engine (see `08-stdlib-ext.md`). The stdlib depends on the runtime, never the reverse (ADR-014).
4. SAPIs depend on `rphp-embed` **only** — they cannot reach runtime internals. If a SAPI needs something, the public embedding API grows; the abstraction does not leak. Enforced by the crate graph since ADR-014 (`Engine`/`Interp`/`Request`/`OutputSink`/`Response`, ADR-033).

---

## 3. Build, toolchain, features, targets (§22)

### 3.1 Toolchain — pinned nightly (ADR-013, deviates from §22)

The workspace pins a **specific nightly** via `rust-toolchain.toml` to enable `become` (guaranteed tail calls) for the threaded interpreter (ADR-001) and other nightly features the `no_std` core needs. "MSRV" = the pinned nightly version, tested in CI and bumped deliberately. A `stable`-buildable subset remains via the `dispatch-portable` feature (the `loop { match }` core), at a measured perf cost. **Today the portable core is the default build** and `become` sits behind `tailcall-dispatch`; the pin is no older than `mago-syntax`'s MSRV and moves only by the ADR-028 procedure (`become` build + stable portable build + corpus job).

### 3.2 Feature flags

| Flag | Effect |
|------|--------|
| (default) | interpreter + `jit-baseline` + core stdlib + `std` |
| `jit-baseline` | Tier-1 copy-and-patch |
| `jit-opt` | Tier-2 (pulls Cranelift) |
| `dispatch-portable` | `loop { match }` dispatch (stable-toolchain path) — **currently the default build** (ADR-028) |
| `tailcall-dispatch` | `become`-threaded Tier-0 dispatch (ADR-001); needs the pinned nightly |
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
2. **The parser is `mago-syntax`** — chosen as the M0 bootstrap by ADR-007 and made the pinned, conformance-checked **production** parser by ADR-025; an owned parser is only the documented fallback, the lossless CST stays deferred.

Every milestone gate: `.phpt` pass rate must not regress, and benchmark ratios versus `php -d opcache.jit=tracing` must move the right direction (see `10-testing.md`).

> **Active milestone track (roadmap 2026-09-17): the Symfony ladder L0–L9** (ADR-034) — L0 php-src `.phpt` baselines, L1 Composer autoload, L2 string/var-dumper, L3 console, L4 byte-identical dumped container, L5 http-foundation, L6 FrameworkBundle kernel, L7 `rphp -S` over HTTP, L8 `symfony/demo`, L9 webapp pack / Pimcore-class. Each rung is a differential fixture under `fixtures/ladder/` run by `cargo xtask ladder`; the M-series above remains the engine's long-run shape, but rungs are what gate commits until L7 is green (ADR-014…035).

---

## Deviations from base-idea.md

- **§22 "stable Rust" → pinned nightly** (ADR-013), to enable `become`.
- **§2 `rphp-parser` is a `mago-syntax` adapter in M0** (ADR-007); `rphp-ast` owns the AST contract; lossless CST deferred.
- **§24 stdlib is a parallel committed track** (ADR-004), not demand-best-effort.
- **§2 dependency direction inverted** (ADR-014): stdlib above runtime, compiler without stdlib dependency, SAPIs on `rphp-embed` only; extension crates under `crates/ext/` (ADR-031); `rphp-lexer` removed in favour of the mago adapter + `rphp-tokenizer` (ADR-025/029); `rphp-resolve` folded into `rphp-hir` (ADR-026).
- **§22 toolchain pin tracks mago's MSRV; the portable core is the default build** (ADR-028).
- **§24 milestones: the Symfony ladder L0–L9 is the active gate** (ADR-034) ahead of the M-series.

## Open questions

- O-1 (own vs reuse lexer) — **closed** by ADR-025/ADR-029 (no owned lexer for parsing; `rphp-tokenizer` for `ext/tokenizer` only).
- Dependency-graph enforcement mechanism (cargo-deny vs a custom `xtask` check) — `xtask` detail, pick during M0.
