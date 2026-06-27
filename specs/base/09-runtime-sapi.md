# 09 — Concurrency, Isolates & SAPIs

**Status:** stable
**Source sections:** `base-idea.md` §17, §18
**Reads with:** [00-overview.md](00-overview.md), [02-value-model.md](02-value-model.md), [04-memory-gc.md](04-memory-gc.md), [07-jit.md](07-jit.md), [08-stdlib-ext.md](08-stdlib-ext.md), [10-testing.md](10-testing.md), [decisions.md](decisions.md) (ADR-002, ADR-003, ADR-009)

PHP is share-nothing per request; rPHP makes that an explicit **isolate** model — the V8/Cloudflare-Workers design applied to PHP. N isolates run on a thread pool with **no shared mutable state and no global lock**, sharing only process-lifetime immutable data that is **refcount-immortal** (ADR-009) so cross-isolate reads touch no contended cache line. Within an isolate, **PHP 8.1 fibers** provide cooperative concurrency and an `io_uring`-backed reactor turns blocking I/O into fiber yields. The whole engine is reachable through one public surface, `rphp-embed`, on top of which every SAPI is built.

---

## 17.1 — Isolates

An `Isolate` is the unit of execution and the unit of memory ownership. It owns its **arena** ([04-memory-gc.md](04-memory-gc.md)), its **interner view**, its **class table**, and its **VM stacks**. Everything an isolate mutates lives behind this struct; nothing outside it is writable by two isolates at once.

```rust
pub struct Isolate {
    arena:      RequestArena,    // bump + slab; resets O(1) at request end (04 §10.1)
    interner:   InternerView,    // per-isolate handles into the immortal global interner
    classes:    ClassTable,      // request-local view; sealed entries point at immortal defs
    stacks:     VmStacks,        // value stack + call frames, one live set per running fiber
    sched:      FiberScheduler,  // run queue + reactor handle (17.2 / 17.3)
    meter:      MemoryMeter,     // the single memory_limit counter (04 §10.4)
    caps:       CapabilitySet,   // filesystem / network / process grants (§capability model)
    shared:     &'static Process // immortal: bytecode, global symbols, JIT code cache (ADR-009)
}
```

Isolates share **only** immutable, process-lifetime data, reached through `&'static Process`:

- **compiled bytecode** (the content-addressed code-cache artifact, [00-overview.md](00-overview.md) §1),
- **interned global symbols** (class/function/constant names, [02-value-model.md](02-value-model.md)),
- the **JIT code cache** (Tier-1/Tier-2 native, [07-jit.md](07-jit.md)).

### Why immortality is load-bearing (ADR-009)

Each of those objects carries a `GcHeader { refcount }`. If an isolate copied or dropped one, it would read-modify-write that count — and a count word shared by every worker is a **bouncing cache line**: each `inc`/`dec` invalidates the other cores' copies, serializing scaling under exactly the load the server SAPI is built for. ADR-009 makes shared data **refcount-immortal**: the count is a saturated sentinel, `IMMORTAL` is set in the header, and the copy/drop fast path skips the count entirely (`if heap && !IMMORTAL`). Cross-isolate reads are then **truly read-only** — no shared writable line exists — which is what makes "no shared mutable state" a fact rather than an aspiration. This mirrors immortal/frozen objects in CRuby and CPython.

### The thread pool

N isolates are scheduled across an OS thread pool sized to the core count. There is **no global lock** because there is no shared mutable structure to guard: name resolution reads immortal symbols, code lookup reads the immortal cache, and all writes land in the per-isolate arena. Isolate arenas are placed **NUMA-locally** to their owning worker ([04-memory-gc.md](04-memory-gc.md) §10.4). This is the property the server SAPI monetizes: throughput scales with cores instead of contending on a runtime lock.

---

## 17.4 — Cross-isolate communication

Two isolates never share a mutable `Value` ([02-value-model.md](02-value-model.md)). Communication is one of:

- **Message-passing.** A value crossing an isolate boundary is **deep-copied** (or moved through an owned, serialized envelope) into the receiver's arena; the sender retains nothing aliased. Channels carry these envelopes between isolates.
- **Explicit shared immutable caches.** Read-only data structures published into the process arena, made **immortal** on publication, after which any isolate reads them with zero coordination (same mechanism as §17.1).

There is no third option — no shared heap, no cross-isolate references into a live arena — so a request can never observe another request's mutation, and arena reset is always sound ([04-memory-gc.md](04-memory-gc.md) §10.1 COW↔arena).

---

## 17.2 — Fibers

PHP 8.1 **fibers** are first-class: **stackful coroutines** scheduled **cooperatively within a single isolate**. They are the substrate async frameworks build on without *coloring* functions — any function can suspend, so there is no `async`/`await` bifurcation of the call graph.

```rust
struct FiberScheduler {
    ready:   VecDeque<FiberId>,   // run queue of resumable fibers (FIFO)
    blocked: SlotMap<FiberId, WaitState>, // parked on a reactor completion
    stacks:  FiberStackPool,      // guard-paged segments, pooled and reused
    current: Option<FiberId>,
}
```

- **Run queue / scheduling.** One worker thread runs one isolate at a time; the scheduler pops the next ready fiber, resumes it until it suspends or returns, then loops. Scheduling is non-preemptive — a fiber runs until a **yield point**.
- **Yield points.** Explicit `Fiber::suspend`, and implicitly every **async I/O call** (§17.3): the call parks the fiber in `blocked` and returns control to the scheduler. `SafePoint`s ([04-memory-gc.md](04-memory-gc.md)) are emitted at suspension boundaries so the cycle collector and `max_execution_time` interrupt observe a consistent stack.
- **Stack management.** Each fiber owns a separate, **guard-paged** stack segment drawn from a pool and returned on completion; stacks are *not* bump-allocated in the request arena, because they outlive individual frames and must survive across suspensions. Fiber-local *heap* allocation still goes to the isolate arena.

### Fibers, the arena, and the bounded collector (ADR-002)

Within a single request, all fibers share that request's arena, and the **arena reset at request end reaps everything in bulk** — including any reference cycles created by the fibers — so the cycle collector is a no-op on the common path. **Long-lived fibers** (a persistent worker loop, an in-process async runtime that spans many requests without an arena reset) are exactly where the **bounded, incremental Bacon–Rajan collector** earns its keep: time-sliced scanning at safepoints with a capped roots buffer keeps the worst-case pause decoupled from graph size while the long fiber runs ([04-memory-gc.md](04-memory-gc.md) §10.3, ADR-002).

---

## 17.3 — Async I/O: the reactor

The **server** and **FastCGI** SAPIs drive a **non-blocking reactor** per worker thread.

| Platform | Backend |
|----------|---------|
| Linux | **`io_uring`** (submission/completion queues; batched, syscall-light) |
| macOS / BSD | `kqueue` |
| other Unix | `epoll` |
| Windows | IOCP |

A thin portable layer presents one completion-driven interface over all four; `io_uring` is the headline path because it amortizes syscalls and pairs naturally with the fiber model.

**The yield contract.** Blocking stdlib calls have **async variants that yield the current fiber instead of the OS thread.** When PHP code reads a socket stream or runs a PDO query, the native function submits the operation to the reactor, parks the fiber, and the worker immediately runs the next ready fiber — the OS thread never blocks. On completion the reactor moves the fiber back to the run queue with its result. This is wired through the stream layer and PDO drivers in [08-stdlib-ext.md](08-stdlib-ext.md) (ADR-012): the same `fread`/`fwrite`/query surface, backed by reactor I/O, so userland code is unaware it suspended. Synchronous variants remain for the CLI and for code that wants a hard block.

---

## 18 — SAPIs

A SAPI is a thin front end over [`rphp-embed`](00-overview.md). **Every SAPI depends on `rphp-embed` only** — modularity contract #4 ([00-overview.md](00-overview.md) §2.1): a SAPI may not reach runtime internals, and if it needs something the public embedding API grows rather than leaking the abstraction.

| SAPI | Crate | Model |
|------|-------|-------|
| **CLI** | `rphp-sapi-cli` | One isolate, run script, exit. The dev and benchmark entry point. |
| **Server** | `rphp-sapi-server` | Persistent **multi-isolate** HTTP/1.1 + HTTP/2 worker; warm JIT, **arena-per-request**. The FrankenPHP/RoadRunner-class throughput story, **in-process**. |
| **FastCGI** | `rphp-sapi-fcgi` | Drop-in behind nginx/Apache; same persistent isolate pool + reactor as the server. |
| **Embed** | `rphp-embed` | The **public Rust API**: create an isolate, define host functions, eval, exchange values. The base every other SAPI sits on. |
| **C embed** | `rphp-ffi` (`librphp`) | The same surface exposed over a **C ABI** for non-Rust embedders. |
| **WASM** | `rphp-sapi-wasm` | `wasm32-unknown-unknown` / WASI build of the **whole engine**: interpreter **+ copy-and-patch only**, **no host Cranelift inside WASM** ([07-jit.md](07-jit.md)); `icu4x` locale subset (ADR-003). The bridge to browser and edge execution. |

The WASM build runs PHP inside a WASM sandbox: Tier-2 Cranelift is unavailable (no host code generation), so the engine relies on the interpreter and the portable copy-and-patch Tier-1 stencils, and ships the pure-Rust `icu4x` subset instead of system ICU for `intl`/`mbstring` (ADR-003, [08-stdlib-ext.md](08-stdlib-ext.md)).

### The `rphp-embed` spine

Every row above reduces to a handful of operations on `rphp-embed`; the SAPIs differ only in their transport (argv, an HTTP listener, a FastCGI socket, a C ABI shim, a WASM import table) and their default `CapabilitySet`.

```rust
let mut iso = Isolate::builder()
    .caps(CapabilitySet::deny_all().grant_fs_read("/srv/app"))
    .memory_limit(128 << 20)
    .build()?;                              // create isolate

iso.define("host_now", |_args| Value::int(now_ms())); // host function
let ret: Value = iso.eval(source_handle)?;            // run; exchange values
```

Values cross the boundary by the same rules as §17.4 — owned or deep-copied, never aliased into a live arena — so a host embedding many isolates gets the share-nothing guarantee for free. The `rphp-ffi` C surface (`librphp`) is a 1:1 projection of these calls over a C ABI, with `Value` handles opaque to the caller.

---

## Capability model

Filesystem, network, and process access are mediated by a **capability set the embedder configures** on each isolate. Native code does not call the OS directly; it asks the `CapabilitySet`, which is consulted on the (already necessary) syscall boundary.

```rust
pub struct CapabilitySet {
    fs:      FsCaps,    // allowed roots + read/write masks; default per-SAPI
    net:     NetCaps,   // allowed hosts/ports, outbound vs. inbound
    process: ProcCaps,  // exec / signals / fork — off by default off-CLI
    env:     EnvCaps,   // which environment variables are readable
}
```

- **CLI / server / FCGI** default to host-equivalent grants (configurable to tighten for multi-tenant hosting).
- **WASM** defaults to **deny-all plus explicit grants**: nothing is reachable until the host hands out a capability, matching the WASI sandbox posture.

This capability set is the **substrate** for the resource-limit and sandboxing story — `memory_limit`/`max_memory_limit`, `max_execution_time`, and extension sandboxing — detailed in [10-testing.md](10-testing.md) §security. This doc establishes the mechanism; the policy and enforcement live there, referenced rather than duplicated.

---

## Arena-per-request lifecycle (server)

The server SAPI is where the isolate, arena, and JIT designs compound into throughput. A request flows:

1. **Acquire** a warm isolate from the pool — its JIT is already hot, its `&'static Process` already wired to the immortal caches; there is no per-request warmup.
2. **Bind** the request; allocations **bump the arena** (cells and entries from slabs, large objects to `mimalloc`), all counted by the single `MemoryMeter` ([04-memory-gc.md](04-memory-gc.md) §10.4).
3. **Run** the handler — possibly spawning fibers and issuing reactor I/O (§17.2/§17.3) — to completion.
4. **Reset** the arena in **O(1)** at request end: no per-object frees, the whole region drops. Per-request cycles die with it, so the **cycle collector is a no-op on the common request path** (ADR-002).
5. **Return** the isolate to the pool, warm, for the next request.

Because nothing escapes a request except via the process-lifetime immortal arena or explicit cross-isolate message-passing (§17.4), step 4 is always sound. This is the same bump-and-reset lever that makes Zend's request shutdown cheap, kept warm across requests and replicated across isolates with no shared lock.

---

## Deviations from base-idea.md

- **Refcount-immortality made the basis of share-nothing (ADR-009).** §17 claimed "no shared mutable state" while §10/§11 stored shared data carrying a mutable refcount. This doc grounds the claim on immortality: the shared count is never written, so no contended line exists.
- **Cycle collection scoped to long-lived fibers/isolates (ADR-002).** The per-request arena reset reaps request cycles in bulk; the bounded incremental collector is specified to run only where an arena does not reset frequently.
- **Async stream/PDO I/O wired to the reactor (ADR-012).** §17's "async variants" are tied concretely to the stream and PDO layers of [08-stdlib-ext.md](08-stdlib-ext.md), with the fiber-yield contract spelled out.
- **WASM JIT envelope restated (ADR-003).** The WASM SAPI is pinned to interpreter + copy-and-patch with the `icu4x` subset; no host Cranelift inside the sandbox.

## Open questions

- **Isolate-to-core ratio and work-stealing.** Whether the thread pool should pin one isolate per worker or allow work-stealing of ready fibers across workers (and the cache cost of doing so) — revisit under server load in M4.
- **Cross-isolate message envelope format.** Deep-copy vs. a compact serialized wire form for large messages, and whether immortal-publish should be the default for big read-only payloads — tune against real multi-isolate workloads.
- **Fiber stack sizing.** Default segment size, growth policy, and pool high-water caps under adversarial fiber counts — needs the bounded-collector interaction measured (links to [04-memory-gc.md](04-memory-gc.md) open questions).
- **`io_uring` feature floor.** Minimum kernel/feature set assumed (registered buffers, multishot) before falling back to `epoll` on older Linux — pin during M4.
