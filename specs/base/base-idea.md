# rPHP Technical Specification

**Project:** rPHP — a Rust implementation of the PHP 8.5 language and runtime
**Document status:** Draft v0.1 (architecture baseline)
**Date:** June 2026
**Target language level:** PHP 8.5 (released 2025-11-20)
**Primary goal:** the fastest correct PHP runtime, built on modern compiler and runtime techniques, with a strictly modular architecture that lets every layer be replaced, embedded, or compiled out.

---

## 0. Scope and design principles

rPHP is a clean-room PHP engine written in Rust. It is not a transpiler and not a binding layer over `php-src`. It owns the whole pipeline from source bytes to native machine code, plus the value model, memory manager, standard library, and host integration surface.

### 0.1 Design principles

1. **Decided, not optional.** This spec commits to one representation per layer. Alternatives that were considered and rejected are documented inline so the reasoning survives, but the build target is singular. Configurability lives behind Cargo features only where it earns its keep (platform, JIT tier, stdlib surface).
2. **Speed is a layout problem first, an algorithm problem second.** Most PHP slowness is boxing traffic, dynamic dispatch, and hashtable-for-everything. The architecture attacks those three at the data-structure level before any JIT exists.
3. **Tiered execution.** Code starts interpreted and climbs to optimized native only where it is hot. No ahead-of-time compile wall, no cold-start cliff.
4. **Modularity by crate boundary.** Each subsystem is a crate with a typed API and no reach-through. The interpreter must not know how the parser works; the JIT must not know how strings are allocated beyond the published `Value` contract.
5. **Correctness is measured, not asserted.** The php-src `.phpt` corpus and differential testing against stock PHP are the oracles. "Percent of `.phpt` passing" is a first-class CI metric from day one.
6. **`no_std`-friendly core.** The value model, bytecode, interpreter, and GC compile without `std` so the engine can target WASM and bare embedding. `std` is a feature that the CLI and server SAPIs turn on.
7. **No undefined behavior in safe paths.** `unsafe` is allowed, audited, confined to documented modules (NaN-free value access, GC, JIT codegen, FFI), and covered by Miri where feasible.

### 0.2 Non-goals (v1)

- Bug-for-bug compatibility with the Zend C extension ABI. rPHP defines its own extension model. A Zend ABI shim is a research track, not a v1 deliverable.
- 100% stdlib coverage on day one. The library is built demand-first, prioritized by what real benchmark and framework code touches.
- Matching Zend on I/O-bound web requests at launch. The launch claim is numeric and CPU-bound throughput. Framework-class throughput is a later milestone.

### 0.3 The competitive bar

HHVM dropped PHP support at 3.30; HHVM 4.0 (Feb 2019) was the first version that ran only Hack. The fastest runtime for *the PHP language* today is therefore stock PHP with OPcache and the tracing JIT. That is the baseline every rPHP benchmark is measured against: `php -d opcache.enable_cli=1 -d opcache.jit_buffer_size=64M -d opcache.jit=tracing`.

### 0.4 Target constraints

Two constraints define the design envelope and are assumed everywhere below.

**No legacy. PHP 8.5+ only.** rPHP implements the 8.5 language and nothing earlier. This is not a compatibility shortcut, it is an architectural lever, because the most expensive assumptions in a PHP engine come from pre-8.x behavior that 8.5 code no longer exhibits.

What dropping legacy buys, and why each one matters:

- **Sealed objects.** Dynamic properties are deprecated since 8.2 (`#[AllowDynamicProperties]` to opt back in), so a class has a fixed, compile-time-known property set by default. Objects become structs, not dictionaries (section 11.3). This is the single largest performance unlock in the spec and it is impossible on legacy PHP.
- **Typed-by-default code.** 8.5 code declares parameter, return, and property types pervasively, which turns the optional optimizer into a near-static one (sections 11.3, 14).
- **Legacy machinery leaves the hot path.** The array internal pointer (`current`/`next`/`reset`), the `resource` value kind, removed constructs (`each`, `create_function`, PHP4 constructors), and the Zend C extension ABI are not implemented in the core. The internal pointer becomes a lazily-allocated side table that exists only if those functions are actually called, so it never taxes a normal array.

What does *not* change, because it is current 8.5 semantics and not legacy: copy-on-write value-type arrays, `&` references, loose comparison and type juggling, and deterministic refcount-timed `__destruct`. These stay and still shape the value model and GC. The engine optimizes past them on hot paths rather than removing them.

**Modern hardware is the floor, not a tier.** rPHP assumes current server and workstation silicon and compiles for it directly instead of writing portable scalar code as the primary path.

- **Reference targets:** x86-64 with AVX-512 (including VBMI2, VAES, GFNI) on Zen 4/5 and Sapphire Rapids-class server parts; aarch64 with SVE2 and NEON on Apple silicon and Graviton 4-class parts. AVX2 is the only hard floor for any x86 build; below that is unsupported.
- **The data plane is SIMD-first.** String scanning, comparison, case folding, UTF-8 validation, lexing, and JSON parsing use wide vectors (simdutf/simdjson techniques). Bulk array operations over typed storage vectorize and auto-parallelize above a size threshold.
- **Hashing is hardware-accelerated.** Array-key and string hashing use AES-NI / VAES or hardware CRC32, not a software hash, because key hashing is on the critical path of every array access.
- **Memory is configured for the workload.** The request arena and JIT code cache back onto 2 MiB / 1 GiB huge pages to cut TLB pressure; isolates are NUMA-aware.
- **Profiling is hardware-driven.** Tiering decisions can use the CPU PMU (PEBS/LBR) for near-zero-overhead hotness detection rather than software counters.
- **JIT codegen targets the best ISA unconditionally.** The optimizing tier emits AVX-512 / SVE2 and auto-vectorizes typed-array loops, which is only safe given the hardware assumption.

---

## 1. Architecture overview

### 1.1 Execution pipeline

```
 source bytes
     │
     ▼
 ┌─────────┐   tokens    ┌─────────┐   CST/AST   ┌──────────────┐
 │  lexer  │ ──────────▶ │ parser  │ ──────────▶ │ lower → HIR  │
 └─────────┘             └─────────┘             └──────┬───────┘
                                                        │ name resolution,
                                                        │ desugaring, const-fold
                                                        ▼
                                                 ┌──────────────┐
                                                 │   compiler   │
                                                 │  HIR → BC    │
                                                 └──────┬───────┘
                                                        │ register bytecode (+ IC slots)
                                                        ▼
   ┌──────────────────────── runtime ────────────────────────────┐
   │                                                              │
   │   Tier 0: bytecode interpreter ──profile──▶ Tier 1: copy-    │
   │        ▲    │                                and-patch JIT   │
   │   deopt│    │ OSR / hot               │                      │
   │        │    ▼                         ▼ very hot loop/region │
   │   ┌─────────────┐               ┌──────────────────────┐     │
   │   │ value model │◀──────────────│ Tier 2: optimizing   │     │
   │   │ gc / arena  │   guards/     │ JIT (Cranelift) +    │     │
   │   │ data types  │   deopt md    │ speculative spec.    │     │
   │   └─────────────┘               └──────────────────────┘     │
   └──────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
                    SAPI: cli │ server │ fastcgi │ embed │ wasm
```

### 1.2 Stage artifacts

| Stage | Input | Output | Cacheable |
|------|-------|--------|-----------|
| Lex | `&[u8]` source | token stream | no |
| Parse | tokens | lossless CST + typed AST | yes (CST) |
| Lower | AST | HIR (resolved, desugared) | yes |
| Compile | HIR | register bytecode + metadata | yes (on-disk opcache) |
| Tier 0 | bytecode | side effects + profile | n/a |
| Tier 1 | bytecode + profile | native stub (copy-and-patch) | per-process |
| Tier 2 | hot region + type feedback | optimized native + deopt map | per-process |

The compiled-bytecode artifact is the persistent unit. It is content-addressed by a hash of (source bytes, compiler version, feature flags) and stored in an on-disk code cache, the modern equivalent of OPcache, so cold processes skip lex/parse/compile entirely.

---

## 2. Workspace and crate topology

rPHP is a single Cargo workspace, monorepo style (the rust-analyzer / oxc / ruff model). Crates are layered; dependencies point strictly downward.

```
rphp/
├── Cargo.toml                  # workspace
├── xtask/                      # build orchestration, codegen, phpt runner driver
├── crates/
│   ├── rphp-span/              # byte spans, source ids, no deps
│   ├── rphp-source/            # source files, line maps, virtual FS
│   ├── rphp-diagnostics/       # error model, rendering, codes
│   ├── rphp-intern/            # global + per-isolate string interner
│   ├── rphp-lexer/             # byte-level lexer
│   ├── rphp-ast/               # CST + typed AST node defs
│   ├── rphp-parser/            # recursive-descent, fault-tolerant
│   ├── rphp-hir/               # high-level IR, resolved + desugared
│   ├── rphp-resolve/           # name resolution, scoping, autoload hooks
│   ├── rphp-bytecode/          # register ISA, encoding, metadata
│   ├── rphp-compiler/          # HIR → bytecode
│   ├── rphp-value/             # Value cell, tags, conversions  [no_std]
│   ├── rphp-gc/                # arena, refcount, cycle collector [no_std]
│   ├── rphp-heap/              # string, array, object, closure  [no_std]
│   ├── rphp-shape/             # hidden classes / inline cache infra
│   ├── rphp-runtime/           # interpreter, frames, call ABI    [no_std core]
│   ├── rphp-jit-baseline/      # tier 1 copy-and-patch
│   ├── rphp-jit-opt/           # tier 2 Cranelift + speculation + deopt
│   ├── rphp-profile/           # counters, type feedback, edge weights
│   ├── rphp-analyze/           # optional whole-program type inference
│   ├── rphp-stdlib/            # standard library, feature-gated by ext
│   ├── rphp-ext-abi/           # extension traits + C ABI + WASM component host
│   ├── rphp-embed/             # public Rust embedding API
│   ├── rphp-ffi/               # C ABI surface (librphp)
│   ├── rphp-sapi-cli/          # command line
│   ├── rphp-sapi-server/       # persistent worker HTTP server
│   ├── rphp-sapi-fcgi/         # FastCGI
│   ├── rphp-sapi-wasm/         # wasm32 entry
│   ├── rphp-test/              # phpt runner, differential harness
│   └── rphp-bench/             # criterion + benchmarks-game suite
└── tools/
    └── rphp/                   # the binary (depends on sapi-cli)
```

### 2.1 Modularity contracts

- Crates below `rphp-value` are `#![no_std]` and allocator-pluggable. They never assume a filesystem, clock, or threads.
- `rphp-runtime` exposes execution through a `Vm` trait. Tier 1 and Tier 2 are registered as `Tier` implementations; the interpreter has no compile-time dependency on either JIT crate. A build with `--no-default-features` is a pure interpreter.
- The stdlib is a registry of `NativeFunction` descriptors. Removing an extension is removing a feature flag, not editing the engine.
- SAPIs depend on `rphp-embed` only. They cannot reach into runtime internals. This keeps the embedding API honest: if a SAPI needs something, the public API grows, the abstraction does not leak.

---

## 3. Source and diagnostics

`rphp-span` defines `Span { file: FileId, lo: u32, hi: u32 }` (byte offsets, 4 GiB per-file ceiling, which is fine). Every IR node from CST to bytecode carries a span for backtraces, error reporting, and JIT-dump source mapping.

`rphp-diagnostics` is a structured error model with stable codes (`RPHP_E0001`...), severity, primary and secondary labels, and a renderer modeled on `ariadne`/rustc output. Parser errors are recoverable: the parser never aborts on the first error, it produces error nodes and keeps going, which is required for editor tooling and for fault-tolerant batch compilation.

---

## 4. Lexer

`rphp-lexer` is a hand-written byte-level scanner. It operates on `&[u8]`, not `&str`, because PHP source and PHP strings are byte sequences, not guaranteed UTF-8.

Design notes:

- Single forward pass, no backtracking, branch-predictable hot loop over a 256-entry classification table.
- Handles the awkward PHP corners directly: `<?php`/`?>` island toggling between literal HTML and code, heredoc and nowdoc with indentation stripping, string interpolation states, numeric literal separators, attributes `#[...]`, and the 8.5 pipe token `|>`.
- Emits a flat token stream with trivia (whitespace, comments) attached as side-channel data so the CST can be lossless without slowing the parser.
- Produces interned identifiers eagerly: every identifier token carries an `IdentId` from `rphp-intern`, so downstream stages compare symbols by integer, never by string.

Lexing is fully streamable for the WASM and large-file cases.

---

## 5. Parser and AST

`rphp-parser` is a hand-written recursive-descent parser with Pratt-style expression precedence. Hand-written rather than generated because PHP's grammar has context sensitivity (the cast vs parenthesized expression ambiguity, the heredoc body, magic constants) that a generator fights.

It builds two layers:

- A **lossless CST** (concrete syntax tree) retaining every token and trivium. This backs formatters, refactoring, and exact source round-tripping.
- A **typed AST** view over the CST, the structured tree the rest of the compiler consumes.

This dual model is the modern editor-grade approach (rust-analyzer's rowan, oxc). For a runtime it costs little and buys lossless tooling for free.

> **Front-end reuse option.** Mago's `mago-syntax` crate is a maintained, PHP 8.5-aware Rust lexer/parser/AST. rPHP's parser API is shaped to allow `mago-syntax` as a drop-in front end behind a feature flag, so the project can bootstrap on a proven parser and replace it later if the lossless-CST requirements diverge. The decision is deferred to first milestone; the AST contract is owned by `rphp-ast` either way via a thin adapter.

---

## 6. HIR: name resolution and desugaring

`rphp-hir` is where PHP's surface syntax collapses into a small, regular core. Lowering AST to HIR does:

- **Name resolution** (`rphp-resolve`): resolves class, function, constant, and namespace references to fully qualified `SymbolId`s, wires up `use` imports, records autoload points where a symbol is unresolved at compile time and must be loaded at runtime.
- **Desugaring**: rewrites the convenient-but-redundant into the canonical. Short closures (`fn() =>`) become closures. The pipe operator `$x |> f(...)` becomes a straight call chain. `clone($o, [...])` (8.5 clone-with) becomes a clone followed by typed-property writes that respect hooks and visibility. `match` becomes a strict-compare decision tree. Null-safe `?->` becomes guarded access. Interpolated strings become concatenation or an optimized join.
- **Constant folding and const-expression evaluation**: attribute arguments, const initializers, enum cases, and the now-permitted static closures and first-class callables in constant expressions are evaluated to a normal form here.

HIR is intentionally boring: a few expression kinds, explicit control flow, explicit temporaries. The bytecode compiler that follows is therefore small and the optimizer has a regular target.

---

## 7. Bytecode: a register ISA

rPHP compiles to a **register-based** virtual ISA, not a stack machine. Register VMs (Lua 5, Dalvik) execute fewer instructions per operation and far fewer dispatches, and they lower to a JIT's SSA form almost directly. Zend is stack-ish with a fixed temporary model; rPHP goes fully register.

### 7.1 Model

- Each function gets a virtual register file of unbounded width during compilation; a linear-scan pass packs it to a concrete frame size.
- Instructions are three-address: `dst, src1, src2`.
- Operands are register indices, small immediates, or constant-pool indices.
- Every polymorphic instruction (arithmetic, property access, method call, array access, comparison) carries an **inline cache slot index**. The IC slot is mutable per-instruction state living beside the bytecode, holding the last-seen type/shape and a resolved handler. This is the substrate for both fast interpretation and type feedback.

### 7.2 Encoding

Variable-length, byte-aligned, little-endian. One-byte primary opcode, optional extension byte for the long-tail ops, then operands. A wide prefix promotes 8-bit operand fields to 32-bit for large functions. Decoding is a single load plus table dispatch. The format is designed so the interpreter's decode is branch-light and so the copy-and-patch JIT can map one bytecode op to one machine-code stencil.

### 7.3 Representative opcode groups

| Group | Examples | Notes |
|-------|----------|-------|
| Move/const | `Mov`, `LoadConst`, `LoadNull`, `LoadTrue`, `LoadInt8` | immediates inline |
| Arithmetic | `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Pow`, `Concat` | IC slot, fast int/float paths |
| Compare | `CmpEq`, `CmpIdentical`, `CmpLt`, `Spaceship` | PHP 8 comparison semantics baked in |
| Branch | `Jmp`, `JmpIfTrue`, `JmpIfFalse`, `JmpTable` | edge-profiled |
| Array | `NewArray`, `ArrGet`, `ArrSet`, `ArrAppend`, `ArrUnset` | packed fast path + IC |
| Property | `PropGet`, `PropSet`, `PropInit` | shape-guarded inline cache |
| Call | `CallFn`, `CallMethod`, `CallStatic`, `CallDynamic`, `Ret` | monomorphic IC, arg adapters |
| Object | `New`, `Clone`, `InstanceOf`, `InitProps` | hidden-class transitions |
| Closure | `MakeClosure`, `BindThis` | upvalue capture descriptors |
| Iter | `IterInit`, `IterNext`, `IterValue` | foreach over packed/hashed/Traversable |
| Type | `TypeCheck`, `Coerce`, `CastBool/Int/Float/String` | declared-type enforcement |
| Exc | `Throw`, `EnterTry`, `LeaveTry`, `Catch` | table-driven unwinding |
| Tier | `ProfileEdge`, `LoopHeader`, `SafePoint`, `OsrEntry` | tiering and GC safepoints |

`LoopHeader` and `OsrEntry` mark on-stack-replacement entry points so the optimizing tier can take over a running loop without restarting the function. `SafePoint` marks where the cycle collector and deopt are allowed to run.

### 7.4 Metadata

Alongside the instruction array, a function's compiled body carries: the constant pool, the IC slot table, exception-handler regions (try ranges, catch types, finally targets), the declared signature with types, upvalue descriptors, source spans per instruction, and a profiling block reserved for counters and type feedback.

---

## 8. Compiler

`rphp-compiler` lowers HIR to bytecode. It is deliberately thin: HIR already removed the surface complexity. The compiler does register allocation (linear scan over SSA-ish HIR temporaries), constant-pool construction, IC slot assignment, exception-region layout, and a small peephole pass (dead move elimination, constant propagation of already-folded values, jump threading).

Heavy optimization is *not* done here. It happens in the optimizing JIT where runtime type feedback is available, which is where it actually pays off for a dynamic language. Compiling cheaply and optimizing late is the whole tiering thesis.

---

## 9. Value representation

### 9.1 The decision

The canonical in-memory value is a **16-byte tagged cell**, not a NaN-boxed word.

```rust
#[repr(C)]
pub struct Value {
    data: ValueData,   // 8 bytes
    tag:  ValueTag,    // 1 byte + 7 padding (kept for alignment + future flags)
}

#[repr(C)]
union ValueData {
    int:   i64,
    float: f64,
    ptr:   *mut GcHeader,     // string | array | object | closure | reference
    bits:  u64,
}

#[repr(u8)]
pub enum ValueTag {
    Null, False, True,
    Int, Float,
    Str, Array, Object, Closure, Reference,
    // room for: Indirect, Uninit (typed-prop), FuncRef
}
```

### 9.2 Why not NaN-boxing

NaN-boxing packs a value into one 64-bit word and is the reason LuaJIT flies. It works for Lua because Lua numbers are doubles. PHP integers are full 64-bit, and a NaN payload is ~51 bits. Pure NaN-boxing therefore forces either a 32-bit small-int fast path with heap-boxed promotion for large ints, or lossy integers. PHP code uses 64-bit integers constantly (hashes, ids, bitmasks, timestamps), so the boxing churn at the boundary would land on a hot path, not an edge case.

The 16-byte tagged cell is what Zend itself uses for `zval`, and for good reason: full `i64` and `f64` inline, no allocation for scalars, two-word locality, trivial tag dispatch. rPHP keeps that proven storage shape and wins elsewhere (arena GC, packed arrays, hidden classes, late optimization), rather than paying an integer tax to save 8 bytes per slot.

### 9.3 Where unboxing happens

Inside JIT traces and optimized regions, values are fully unboxed into typed SSA registers: an `i64` lives in a machine register, an `f64` in an XMM register, a string pointer in a GPR. A hot numeric loop carries native integers and never materializes a `Value` until it crosses a trace boundary or hits a side exit. This captures the entire LuaJIT-style win (no boxing in the loop) without making the storage representation lossy. The 16-byte cell is the boundary format; the register file inside compiled code is unboxed.

### 9.4 References and COW

`Reference` is a distinct tag wrapping a shared, mutable `Value` slot, implementing PHP's `&$x` aliasing. Copy-on-write is a property of the heap containers (string, array), tracked by refcount, not of the `Value` cell. A `Value` copy is a tag+8-byte memcpy plus, for heap tags, a refcount increment.

---

## 10. Memory management

PHP's lifecycle is the lever. A request begins, allocates freely, and ends; almost everything dies at once. rPHP exploits this with a three-part design.

### 10.1 Per-request arena (bump + bulk reset)

Each isolate owns a request-scoped arena allocator. Most allocations bump a pointer; at request end the arena resets in O(1) without per-object frees. This is the single biggest server-throughput win and mirrors why Zend's request shutdown is cheap. Long-lived allocations (the compiled bytecode cache, interned symbols, the class table) live in a separate process-lifetime arena.

### 10.2 Reference counting for in-request reuse

Heap objects carry a `GcHeader { refcount: u32, type: HeapKind, color: GcColor, flags }`. Refcounting gives PHP its deterministic destructor timing, which real code depends on (`__destruct` at refcount zero, not at some future GC pause). Most allocations are freed by refcount before the arena ever resets, keeping arena high-water marks low.

### 10.3 Cycle collection (Bacon-Rajan)

Refcounting leaks cycles. rPHP runs a concurrent **trial-deletion / synchronous cycle collector** (the Bacon-Rajan algorithm, the same family Zend's `gc_collect_cycles` is based on). Candidate roots are objects whose refcount is decremented but stays nonzero; they are buffered and scanned at safepoints. The collector is the only component allowed to traverse object graphs, and it does so only at `SafePoint` instructions, so the interpreter and JIT never race it.

### 10.4 Allocator and layout

- Global allocator: `mimalloc` for the process arena and large objects.
- The request arena and the JIT code cache are backed by 2 MiB / 1 GiB huge pages to cut TLB misses on the hot bump path and on compiled-code execution; isolate arenas are placed NUMA-locally to the worker thread that owns them.
- Size-classed slab allocator for the high-frequency object kinds (cells, array entries, small strings) to kill allocator overhead and fragmentation.
- Small-string optimization: strings up to 22 bytes live inline in the header's tail, no separate allocation.
- `rphp-gc` is `no_std` and takes the allocator as a generic parameter, so WASM and embedders can supply their own.

---

## 11. Core heap types

### 11.1 Strings

```rust
#[repr(C)]
pub struct PhpStr {
    header: GcHeader,
    len:    u32,
    hash:   u32,         // cached, lazily computed, 0 = uncomputed
    flags:  StrFlags,    // interned, valid-utf8-known, small
    data:   StrStorage,  // inline<=22 bytes, else ptr+cap
}
```

Byte strings, never assumed UTF-8. Cached hash so array keys and symbol comparisons are integer-fast; the hash is computed with hardware AES-NI / VAES (or CRC32 instructions) rather than a software hash, since string and key hashing sits on the critical path of every array and property lookup. Interning table for compile-time-known strings (class names, method names, literal keys). COW on mutation. Length, comparison, search, case folding, and UTF-8 validation are SIMD kernels (simdutf-style, AVX-512/SVE2 with an AVX2 floor). Concatenation in hot loops can use a rope/builder fast path chosen by the optimizer when it proves the intermediate is not observed.

### 11.2 Arrays: dual representation

The PHP array is a list, a dict, a stack, and an ordered map in one type. rPHP represents it as a tagged enum that auto-promotes:

```rust
pub enum ArrayRepr {
    Packed(Vec<Value>),         // sequential int keys 0..n, no holes
    Hashed(OrderedMap),         // string keys, sparse int keys, or holes
}
```

- **Packed** is a flat vector. `$a[]` append and `$a[$i]` access are bounds-checked vector ops, no hashing. The common case (a list) is a `Vec`.
- **Promotion** to `Hashed` happens lazily on the first string key, negative or sparse int key, or hole.
- **Typed packed** is a JIT-only refinement: when the optimizer proves every element is `int` or every element is `float`, a hot region operates on an unboxed `&mut [i64]` / `&mut [f64]` view, so numeric array loops touch zero boxed values.

`OrderedMap` is an insertion-ordered open-addressing hash table: a contiguous entry array (key, value, next-collision) plus a SwissTable-style control-byte index probed with SIMD, and keys hashed with hardware AES/CRC instructions. Insertion order is preserved by entry-array position, matching PHP semantics and `foreach` order. The legacy internal pointer for `current`/`next`/`reset`/`key`/`end`/`prev` is not a field on every array; it is a lazily-allocated side table created only when one of those functions is actually called, so the common array pays nothing for it. COW via the header refcount.

Bulk standard-library operations over typed-packed arrays (`array_sum`, `array_map`, `array_filter`, `in_array`, `array_search`, comparisons) are SIMD kernels over the unboxed `[i64]`/`[f64]` storage, and above a size threshold they fan out across cores using the isolate thread pool, since a pure bulk operation has no shared mutable state to contend on.

### 11.3 Objects and hidden classes

Targeting 8.5 changes objects fundamentally. Dynamic properties are deprecated, so a class declares a sealed, compile-time-known property set by default. rPHP therefore does not store properties in a per-instance hashtable and does not even need a transition chain in the common case: the full layout is computed once when the class is defined.

```rust
#[repr(C)]
pub struct PhpObject {
    header: GcHeader,
    class:  ClassId,
    shape:  ShapeId,        // precomputed at class definition for sealed classes
    slots:  *mut Slot,      // flat, mixed boxed/unboxed property storage
}

pub struct Shape {
    class:     ClassId,
    layout:    Box<[SlotDesc]>,            // offset + storage kind per property
    by_name:   PerfectHash<IdentId, u32>,  // property name → slot, built once
    flags:     ShapeFlags,                 // sealed, has-magic, has-hooks, allows-dynamic
    edges:     Option<SmallMap<IdentId, ShapeId>>, // only for #[AllowDynamicProperties]
}

pub enum SlotDesc {
    Boxed,            // Value cell (mixed/object/array/nullable scalar)
    Int,              // raw i64, declared `int`
    Float,            // raw f64, declared `float`
    Bool,             // raw bool, declared `bool`
    // declared `readonly`, hook, and visibility info ride alongside
}
```

Two consequences fall out of sealed-by-default classes:

- **Typed scalar properties are stored unboxed.** A `public int $x` occupies a raw `i64` slot, not a 16-byte `Value`. The object's storage is laid out like a C struct: a contiguous block of typed and boxed slots at fixed offsets. An all-typed-scalar class is byte-for-byte a Rust struct.
- **No transition chains.** Because the property set is known at class definition, the shape is built once and shared by every instance. Object construction stamps the prebuilt shape and fills slots; it does not walk an add-property edge graph. The edge map exists only for classes that explicitly opt into dynamic properties.

Property access compiles to: guard the object's `ClassId` against the IC slot's cached class, on hit load `slots[offset]` at a constant offset, with the load typed (raw `i64`/`f64` or a `Value`) per the slot descriptor. For a monomorphic site on a `final` class the optimizer drops even the guard. This is a constant-offset load, frequently of an unboxed scalar, versus Zend's string-keyed property hashtable probe.

`readonly`, asymmetric visibility (extended to statics in 8.5), property hooks (8.4), and enums are encoded in the slot descriptors and shape flags. The `has-magic` and `has-hooks` flags gate `__get`/`__set`/hook dispatch so the fast path skips those checks entirely for the overwhelming majority of classes that declare none. Internal `resource`-style handles are modeled as a private object kind rather than a separate value tag, so the legacy resource type imposes nothing on the value model.

### 11.4 Closures and references

Closures are objects with a code pointer, a bound `$this` (optional), a bound scope class for visibility, and a captured-upvalue vector built from the compiler's capture descriptors. First-class callable syntax `f(...)` and the new const-expression closures lower to the same representation. References are a small heap cell holding a shared `Value` slot with its own refcount.

---

## 12. Tier 0: the interpreter

`rphp-runtime` is a register-bytecode interpreter and the correctness reference. Everything the JITs do must match the interpreter's observable behavior exactly; the interpreter is what deopt falls back to.

### 12.1 Dispatch

Rust lacks computed `goto`, so the interpreter uses **tail-threaded dispatch**: each opcode is a function that performs its work and tail-calls the handler for the next opcode, threading the VM state through arguments. On toolchains with guaranteed tail calls this compiles to the same machine code as a computed-goto threaded interpreter; a `loop { match }` core is kept as a portable fallback selected by feature flag. Handlers are `#[inline(never)]` to keep the instruction cache hot and register allocation stable per handler.

### 12.2 Frames and call ABI

A single contiguous VM stack holds frames. A frame is a register window into that stack: arguments are laid down by the caller in the callee's incoming register range, so calls do not copy arguments through an intermediate buffer. The call ABI is shared verbatim by Tier 1 and Tier 2 so compiled and interpreted frames interleave on one stack and OSR/deopt can swap a frame's execution tier in place.

### 12.3 Inline caches and profiling

Every polymorphic opcode reads and writes its IC slot. First execution resolves the operation (which `Add` variant, which property offset, which method target) and caches it. Subsequent executions check a cheap guard and reuse the cached handler. The same IC slots accumulate **type feedback**: observed operand types, call targets, and array shapes, recorded as a small histogram. `rphp-profile` also counts loop back-edges and branch directions. When a function or loop crosses its hotness threshold, the profile is handed to the JIT, which is why compilation starts already knowing the likely types. On supporting hardware, hotness detection can run off the CPU performance-monitoring unit (PEBS sampling plus LBR call/branch history) instead of software counters, giving near-zero-overhead tiering decisions and accurate hot-path attribution without instrumenting the bytecode.

---

## 13. JIT

Two tiers above the interpreter, chosen to match a small-team budget while reaching for native speed.

### 13.1 Tier 1: copy-and-patch baseline

The baseline JIT uses **copy-and-patch compilation** (Xu and Kjolstad, 2021; the technique now in CPython's experimental JIT). At build time, a stencil library is generated: one pre-compiled machine-code template per bytecode opcode, with holes for operands and continuation addresses. At runtime, compiling a function is memcpy-ing stencils and patching the holes. This is close to interpreter compile speed (microseconds per function, no IR, no register allocator) while producing straight-line native code that removes dispatch overhead and inlines the inline-cache fast paths.

Tier 1 is whole-method, non-speculative, and always correct for any types. It exists to get hot code off the interpreter immediately with near-zero compile latency. It is the workhorse tier and the one most code ever reaches.

### 13.2 Tier 2: optimizing JIT

Very hot loops and regions are recompiled by `rphp-jit-opt`, a profile-guided **region-based** optimizer lowered through **Cranelift**.

Region-based rather than pure method-based or pure trace-based: HHVM's published result (PLDI 2018) is that for PHP-shaped code, profile-guided region selection beats both. A region is a hot, type-stable subgraph stitched from profiled edges, possibly spanning inlined callees.

The pipeline:

1. **Region formation** from edge profiles and the call-target feedback in IC slots; hot monomorphic callees are inlined.
2. **Type specialization** from IC type histograms. If a site saw only `int`, emit unboxed `i64` arithmetic guarded by a type check. If a property site saw one shape, emit a shape-guarded offset load. If an array was always packed-int, emit `[i64]` access.
3. **Guards and side exits.** Every speculation is protected by a guard. Guard failure is a **deopt**: control transfers to the interpreter at the corresponding bytecode index with the abstract state reified back into `Value` cells, using a deopt metadata map emitted with the compiled region. This is the Hölzle/Ungar dynamic-deoptimization model.
4. **Classic optimizations on unboxed IR:** allocation sinking (don't build a `Value` that never escapes), store-to-load forwarding, guard hoisting and deduplication out of loops, redundant refcount elimination, bounds-check elimination on proven-packed arrays.
5. **Lowering to Cranelift IR**, then to native. Cranelift is the Rust-native backend (it powers wasmtime), has acceptable peak codegen, and crucially has fast compile times, which matters for a JIT. LLVM is rejected here: better peak code, but compile latency wrong for online compilation. Codegen targets the reference ISA unconditionally (AVX-512 / SVE2, AVX2 floor), and typed-array loops are auto-vectorized. Because whole-program type inference (section 14) runs by default on typed 8.5 code, many sites need no speculation at all: a declared, `final`, typed shape is a static fact, so the guard is elided rather than emitted-and-hoisted.
6. **OSR.** A loop hot mid-execution is entered at its `OsrEntry` safepoint without unwinding, by translating the live interpreter/baseline frame into the optimized frame layout.

### 13.3 Refcount and GC interaction

Compiled code participates in refcounting and cooperates with the cycle collector at safepoints only. The optimizer elides refcount pairs it can prove balanced within a region and keeps precise stack maps so the collector can find roots in compiled frames.

### 13.4 Endgame note

The LuaJIT-class ceiling (a hand-written assembler with linear-scan allocation and a bespoke trace optimizer for minimal compile latency and maximal control) is explicitly out of scope for v1. Cranelift gets most of the way at a fraction of the engineering cost. The stencil and IR boundaries are designed so a custom backend could replace Cranelift later without touching region formation or speculation.

---

## 14. Type-driven specialization and AOT

Targeting typed 8.5 code makes the type system central rather than supplementary. rPHP treats declared and inferred types as primary optimization inputs, not hints.

- Declared parameter, return, and property types are facts the optimizer assumes. Where the type is sealed (a `final` class, a scalar, a sealed shape from section 11.3) the assumption needs no guard.
- `rphp-analyze` is a **default-on** whole-program type inference pass (in the spirit of Psalm and the Mago analyzer), not an optional one. It runs at compile/deploy time and annotates bytecode with inferred types, collapsing the set of sites the JIT must speculate on. On well-typed code most arithmetic, property, and call sites become statically monomorphic.
- **AOT is a first-class tier, not a future note.** A module whose types are fully known (typed, `final`, no dynamic dispatch into unknown targets) is compiled ahead of time straight through Cranelift with no interpreter warmup and no deopt metadata, because there is nothing to speculate. Mixed code stays tiered: the typed core is AOT-native, dynamic edges fall back to the interpreter/JIT. This is the KPHP idea, but gradual and per-region rather than whole-program-or-nothing.
- Dynamic and reflective features (`eval`, variable variables, dynamic property creation on opted-in classes, runtime class mutation) are explicit slow-path triggers. Code that uses them deoptimizes locally; code that does not pays nothing for their existence. On 8.5 the common case is the typed, non-reflective one, so this is a tax you only pay when you reach for it.

---

## 15. Standard library

`rphp-stdlib` is a registry of native functions and classes, organized by PHP extension namespace, each behind a Cargo feature.

```rust
pub struct NativeFn {
    name:    SymbolId,
    arity:   Arity,
    arginfo: &'static [ParamInfo],   // types, by-ref, variadic, defaults
    flags:   FnFlags,                // deterministic, no-side-effect, etc.
    handler: NativeHandler,
}
```

Strategy:

- **Pure Rust** for the core surface: array, string, math, ctype, date/time, JSON, hashing, filter, the 8.5 `URI` extension, `array_first`/`array_last`, the standard SPL data structures.
- **System library bindings** where reimplementation is unwise: PCRE2 via the `pcre2` crate (the pure-Rust `regex` crate is rejected, no backreferences or lookbehind, so it is not PHP-compatible), ICU for `intl`/`mbstring` collation and `IntlListFormatter`, OpenSSL, libxml/DOM, zlib, curl.
- **arginfo is generated** from a declarative table by `xtask`, the modern analog of Zend's stub system, so signatures, reflection data, and the optimizer's effect flags stay in sync.
- Functions carry purity and effect flags so the optimizer can constant-fold deterministic calls and hoist side-effect-free ones.

Coverage is demand-driven and tracked against the `.phpt` corpus. The `(void)` cast and the 8.5 `#[\NoDiscard]` attribute are honored by the compiler's diagnostic pass.

---

## 16. Extension model

rPHP does not reproduce Zend's C ABI. It offers three layers, most-safe first.

1. **Safe Rust extensions.** An extension is a crate implementing an `Extension` trait, registering functions and classes through a typed builder. No raw pointers, no manual refcounting in the common case; the API hands out safe `Value` handles. This is the blessed path.
2. **Stable C ABI (`librphp`).** `rphp-ffi` exposes a C header for FFI from any language, mirroring the embedding API. This is how non-Rust extensions and host languages bind.
3. **WASM component extensions.** The forward-looking path: extensions compiled to WASM components described by WIT, loaded into a sandbox by `rphp-ext-abi`'s component host. Untrusted extensions run capability-confined with no access to host memory beyond the declared interface. This is genuinely modern, directly useful for multi-tenant and edge deployment, and ties into the same WASM toolchain the runtime itself targets.

A Zend C ABI compatibility shim is acknowledged as the thing that would unlock the existing PECL ecosystem, and is filed as a hard, optional research track rather than a v1 promise.

---

## 17. Concurrency and execution model

PHP is share-nothing per request. rPHP makes that an explicit isolate model.

- **Isolates.** An `Isolate` owns its arena, interner view, class table, and VM stacks. Isolates share only immutable, process-lifetime data (compiled bytecode, interned global symbols, the JIT code cache). N isolates run on a thread pool with no shared mutable state and therefore no global lock. This is the V8/Workers model applied to PHP and is what makes the server SAPI scale.
- **Fibers.** PHP 8.1 fibers are first-class: stackful coroutines scheduled cooperatively within an isolate, used to build async without coloring functions.
- **Async I/O.** The server and FCGI SAPIs drive a non-blocking reactor. On Linux the reactor uses `io_uring`; elsewhere `epoll`/`kqueue`/IOCP via a portable layer. Blocking stdlib calls have async variants that yield the current fiber instead of the OS thread.
- **No userland shared memory races.** Cross-isolate communication is message-passing or explicit shared immutable caches, never shared mutable `Value`s.

---

## 18. SAPIs

| SAPI | Crate | Model |
|------|-------|-------|
| CLI | `rphp-sapi-cli` | one isolate, run script, exit. The dev and benchmark entry point. |
| Server | `rphp-sapi-server` | persistent multi-isolate HTTP/1.1 + HTTP/2 worker server, warm JIT, arena-per-request. The throughput story (FrankenPHP/RoadRunner-class, in-process). |
| FastCGI | `rphp-sapi-fcgi` | drop-in behind nginx/Apache. |
| Embed | `rphp-embed` | the public Rust API: create isolate, define host functions, eval, exchange values. |
| C embed | `rphp-ffi` | the same surface over a C ABI (`librphp`). |
| WASM | `rphp-sapi-wasm` | `wasm32-unknown-unknown` / WASI build of the whole engine for browser and edge. Interpreter + copy-and-patch only (no Cranelift host JIT inside WASM); the engine runs PHP inside a WASM sandbox. |

The WASM SAPI is the bridge to in-browser and edge execution, and to running rPHP inside a larger WASM host.

---

## 19. Observability and tooling

- **Structured tracing** via the `tracing` crate throughout, with spans per compilation stage and per request.
- **Pipeline dumps**: `rphp --emit=tokens|ast|hir|bytecode|cranelift|asm` prints any stage's artifact, the rustc `-Z`-style introspection that makes the engine debuggable.
- **JIT dump** in the `perf`/`VTune` formats so optimized frames show up in standard profilers with PHP-level symbols.
- **Deopt log**: every deoptimization is traceable to its guard and bytecode site, the single most important signal for tuning speculation.
- **USDT/DTrace probes** on call, compile, deopt, GC.
- **Counters**: per-function tier, IC monomorphism rate, arena high-water, GC cycles collected, exposed over an admin endpoint in the server SAPI.

---

## 20. Testing, correctness, benchmarking

Correctness is empirical:

- **`.phpt` corpus.** The php-src test suite is the primary oracle. `rphp-test` runs `.phpt` files and reports pass percentage as a tracked, must-not-regress CI metric. This is the north star for compatibility.
- **Differential testing.** Generated and real PHP snippets run through both rPHP and stock PHP; outputs, exceptions, and warnings are compared. Divergence is a bug in rPHP by definition.
- **Fuzzing.** `cargo-fuzz` on the lexer and parser (no panics, no UB on arbitrary bytes) and a structure-aware fuzzer that diffs rPHP against PHP on generated programs.
- **Snapshot tests** at every IR boundary (AST, HIR, bytecode) so refactors show their blast radius.
- **Miri** over the `no_std` core for UB detection where the JIT and GC `unsafe` lives.
- **Benchmarking.** `criterion` plus the benchmarks-game kernels (mandelbrot, n-body, fannkuch, spectral-norm, binary-trees, regex-redux) and a framework-bootstrap macro-benchmark. Every run reports the ratio against the Zend+tracing-JIT baseline. CI fails on a throughput regression beyond noise.

Tiering correctness gets special attention: a "deopt stress" mode forces guard failure at every safepoint to prove the interpreter fallback is bit-identical to the optimized path.

---

## 21. Security and resource limits

- **Memory ceiling.** `memory_limit` and the 8.5 `max_memory_limit` hard cap are enforced by the arena allocator, which refuses past the ceiling and raises a catchable error.
- **Execution timeout.** `max_execution_time` is enforced at `SafePoint`s via an interruption flag, so even hot JITed loops honor it (safepoints are emitted on back-edges).
- **Capability-scoped SAPIs.** Filesystem, network, and process access are mediated by a capability set the embedder configures; the WASM SAPI defaults to deny-all plus explicit grants.
- **Sandboxed extensions** via the WASM component model (section 16) for untrusted or multi-tenant extension code.
- **Fatal-error backtraces** (new in 8.5) are produced from frame metadata.

---

## 22. Build, features, platforms

- Workspace builds with stable Rust; an MSRV is pinned and tested. `unsafe` modules are enumerated and Miri-gated.
- **Feature flags**: `jit-baseline`, `jit-opt` (pulls Cranelift), each stdlib extension, `std`, `server`, `wasm`, `analyze`, `aot`. `--no-default-features` yields a pure `no_std` interpreter core.
- **Targets**: x86-64 with an AVX2 floor and an AVX-512 (VBMI2/VAES/GFNI) optimized path on Zen 4/5 and Sapphire Rapids-class parts; aarch64 with NEON plus an SVE2 path on Apple silicon and Graviton 4-class parts. Linux, macOS, Windows, full JIT. `wasm32` runs interpreter plus copy-and-patch only. Pre-AVX2 x86 is unsupported by design; there is no scalar-only fallback build.
- `xtask` drives codegen (opcode tables, stencil generation, arginfo), the `.phpt` runner, and release packaging.

---

## 23. Performance targets

| Workload | Target vs Zend 8.5 + tracing JIT |
|----------|-----------------------------------|
| Numeric kernels (benchmarks-game) | meet or beat after warmup; the launch claim lives here |
| Tight array/object loops | meet or beat via packed arrays + hidden classes |
| Method-call-heavy OO | within parity after Tier 2, on monomorphic-IC strength |
| Cold-start CLI scripts | competitive via code cache + copy-and-patch (no warmup cliff) |
| Framework request throughput | parity is the milestone, not the launch claim; arena + isolates are the levers |

Honest ceiling: beating Zend on CPU-bound numeric and array code is achievable and is the headline. Beating it on full framework requests is a research-grade effort because there the bottleneck is the stdlib and memory behavior, and that is staged accordingly.

---

## 24. Roadmap

- **M0 — Front end.** Lexer, parser, AST, HIR, name resolution. Round-trips and parses the `.phpt` corpus source. Reuse `mago-syntax` to bootstrap if it accelerates M0.
- **M1 — Interpreter.** Value cell, arena+refcount+cycle GC, strings, dual-rep arrays, objects with shapes, register bytecode, Tier 0 interpreter. Goal: a rising `.phpt` pass rate and correct numeric kernels. Benchmark harness live against Zend from day one.
- **M2 — Baseline JIT.** Copy-and-patch Tier 1. Goal: numeric kernels cross over Zend after warmup.
- **M3 — Optimizing JIT.** Region formation, type specialization, guards, deopt, OSR, Cranelift lowering, typed packed arrays, inline caches with feedback. Goal: the headline numeric/array wins.
- **M4 — Server.** Isolates, fibers, async reactor (`io_uring`), persistent worker SAPI. Goal: real request throughput numbers.
- **M5 — Reach.** WASM SAPI, WASM component extensions, optional static analysis and AOT, broader stdlib, Zend ABI shim research.

Each milestone is gated on the `.phpt` pass rate not regressing and the benchmark ratios moving the right direction.

---

## 25. Open questions and risks

1. **Tail-call dispatch portability.** Guaranteed tail calls are not yet stable across all target toolchains; the `loop { match }` fallback must stay within a small constant factor or M1 perf suffers.
2. **Cycle collector pause behavior** under adversarial object graphs in the server SAPI; may need incremental collection sooner than planned.
3. **ICU footprint** on the WASM target. `intl`/`mbstring` may need a slimmed locale build or a pure-Rust subset for browser deployment.
4. **Stdlib long tail.** The compatibility cliff that sank prior alternative runtimes (HippyVM, Tagua, Quercus) is the standard library, not the engine. Demand-driven coverage plus differential testing is the mitigation, but it is the dominant schedule risk.
5. **NaN-boxing revisit.** If profiling shows the 16-byte cell's memory bandwidth dominating a real workload, a NaN-boxed variant with 32-bit smallints is a feature-flagged experiment, not a rewrite, because the `Value` API is sealed behind `rphp-value`.

---

## Appendix A — Value bit layout

16-byte cell, 8-byte aligned. Byte 0..8 is the payload union (`i64` / `f64` / pointer). Byte 8 is the tag; bytes 9..16 are reserved for per-value flags (e.g. interned-key marker on `Str`, packed marker on `Array`) and alignment. Heap tags' payload is a `*mut GcHeader`; the header's first word is `{ refcount: u32, kind: u8, color: u8, flags: u16 }`.

## Appendix B — Glossary

- **Shape / hidden class.** Shared descriptor of an object's property layout enabling offset-based access and monomorphic inline caches.
- **Inline cache (IC).** Per-instruction mutable slot caching the last resolved operation and its guard.
- **Copy-and-patch.** Baseline compilation by memcpy of pre-built machine-code stencils with operand holes patched at runtime.
- **Region-based JIT.** Optimizer that compiles profile-selected, type-stable subgraphs spanning inlined calls, rather than whole methods or single traces.
- **Deopt.** Transfer from optimized native code back to the interpreter on guard failure, reconstructing interpreter state from a metadata map.
- **OSR.** On-stack replacement: switching a running frame's execution tier without unwinding.
- **Isolate.** A share-nothing execution context owning its own arena and mutable state.

---

*End of rPHP Technical Specification v0.1.*