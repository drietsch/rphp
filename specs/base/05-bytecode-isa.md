# 05 — Bytecode: a Register ISA

**Status:** stable
**Source sections:** `base-idea.md` §7 (register ISA), §8 (compiler), §12.2 (frames & call ABI)
**Reads with:** [01-frontend.md](01-frontend.md) (HIR, the compile input), [02-value-model.md](02-value-model.md) (operand values, compare semantics), [03-heap-types.md](03-heap-types.md) (array/property/shape fast paths), [04-memory-gc.md](04-memory-gc.md) (safepoints, immortal shared bodies), [06-interpreter.md](06-interpreter.md) (decode & dispatch, IC slots), [07-jit.md](07-jit.md) (stencils, OSR/deopt), [decisions.md](decisions.md) (ADR-001, ADR-009, ADR-013)

`rphp-bytecode` defines the compile target: a **register-based** virtual ISA, not a stack machine. Register VMs (Lua 5, Dalvik) execute fewer instructions and far fewer dispatches per operation than stack machines, and they lower to a JIT's SSA form almost directly. The encoding is shaped so the Tier-0 decode is a single load plus table dispatch (feeding the `become`-threaded interpreter, ADR-001) and so the Tier-1 copy-and-patch JIT maps **one bytecode op to one machine-code stencil**. `rphp-compiler` ([§8](#8-compiler--rphp-compiler)) is deliberately thin because the front end already collapsed surface complexity into HIR ([01-frontend.md](01-frontend.md)).

---

## 7.1 Register VM model

- **Three-address.** Instructions name `dst, src1, src2` directly; an `Add` is one instruction, not push-push-add-pop.
- **Unbounded virtual register file during compilation.** Each function is compiled against an arbitrarily wide virtual register file (one register per SSA-ish HIR temporary), then a **linear-scan** pass packs it to a concrete `frame_size` ([§8](#8-compiler--rphp-compiler)). Registers are not typed in the bytecode — a register holds a `Value` ([02-value-model.md](02-value-model.md)); typing is recovered later from IC feedback in [07-jit.md](07-jit.md).
- **Operands are one of three kinds:** a **register index** (into the frame window), a **small immediate** (inline in the stream), or a **constant-pool index** (into the function's constant table, [§7.4](#74-function-metadata-block)).
- **Inline-cache slot index.** Every *polymorphic* op (arithmetic, comparison, array access, property access, call, `new`, `instanceof`, iteration) carries a 16-bit **IC slot index**. The IC slot is mutable per-instruction state — last-seen type/shape and a resolved handler — and is the substrate for both fast interpretation and type feedback. It is consumed by [06-interpreter.md](06-interpreter.md) (guard + reuse) and [07-jit.md](07-jit.md) (specialization).

> **IC slots are not inline in the code stream.** A compiled function body is **immortal and shared across isolates** (ADR-009, [04-memory-gc.md](04-memory-gc.md)): the `code` bytes are immutable. Mutable IC state therefore lives in a **per-isolate side table** ([§7.4](#74-function-metadata-block)) indexed by the slot index baked into the instruction. The instruction carries the *index*; the *state* is per-activation-context. This is the single most important encoding consequence of the immortal-bytecode invariant.

---

## 7.2 Encoding

Variable-length, byte-aligned, little-endian. The format is chosen so that decode is `op = code[pc]` followed by a table dispatch, with operands read at fixed offsets relative to `pc`.

```
[ WIDE? ] [ primary:u8 ] [ EXT:u8? ] [ operands… ]
            │                │
            │                └─ present only when primary == OP_EXT (long-tail ops)
            └─ 1-byte opcode → 256-entry dispatch/stencil table
```

- **Primary opcode (1 byte).** Indexes a 256-entry table. One slot, `OP_EXT`, is an **escape** to a second 256-entry table for the long-tail ops (rare/cold opcodes), so the common opcodes stay one-byte while the catalogue is not capped at 256.
- **Operands follow** in a fixed per-opcode layout. Default operand widths: register `r` = **u8**, constant index `k` = **u16**, IC slot `ic` = **u16**, inline immediate `i8`/`i16`, branch target `tgt` = **i16** (signed, relative to the next instruction).
- **`WIDE` prefix.** A function with more than 256 live registers or 64 K constants promotes operand fields to 32-bit. `WIDE` is itself an opcode whose handler re-dispatches the following primary through a **parallel wide-variant table**: register and constant fields become **u32**, branch targets **i32**. This keeps one-op→one-stencil intact (each table entry is still exactly one stencil) and costs one extra dispatch only on the rare wide instruction. IC slot indices stay u16 (a function with >65 K polymorphic sites is implausible; `WIDE` widens them too if ever needed — a documented ceiling).
- **Decode is branch-light.** No operand is bit-packed across a byte boundary; every field is a byte-aligned load. A handler reads its operands at constant offsets, advances `pc` by its fixed length, then `become`s the next handler via `DISPATCH[code[pc]]` (ADR-001). On the `dispatch-portable` build the same layout drives a `loop { match }` core (ADR-013).
- **One op → one stencil (Tier 1).** Because each opcode has a single fixed operand layout and a single continuation shape, the copy-and-patch JIT ([07-jit.md](07-jit.md)) holds one pre-compiled stencil per (primary, wide?) entry, with holes for operand values and the next-op continuation address (Xu & Kjolstad, 2021). Compiling a function is memcpy-ing stencils and patching holes.

**Example encodings** (non-wide):

| Instruction | Bytes | Layout |
|-------------|-------|--------|
| `Mov d, s` | 3 | `op d s` |
| `LoadInt8 d, i` | 3 | `op d i8` |
| `LoadConst d, k` | 4 | `op d k:u16` |
| `Add d, a, b, ic` | 6 | `op d a b ic:u16` |
| `JmpIfFalse s, tgt` | 4 | `op s tgt:i16` |
| `PropGet d, o, name, ic` | 7 | `op d o name:u16(k) ic:u16` |

Per-opcode lengths are derived mechanically from the operand layout and generated by `xtask` alongside the dispatch and stencil tables, never hand-maintained.

---

## 7.3 Opcode catalogue

Representative groups (the full table is generated; this is the shape). **IC** marks groups whose ops carry an IC slot.

| Group | Example opcodes | Operand format | IC |
|-------|-----------------|----------------|:--:|
| Move/const | `Mov d,s` · `LoadConst d,k` · `LoadNull/True/False d` · `LoadInt8 d,i8` · `LoadInt d,k` | reg dst; immediates inline; large literals via const pool | — |
| Arithmetic | `Add` `Sub` `Mul` `Div` `Mod` `Pow` `Concat` `BitAnd` `BitOr` `BitXor` `Shl` `Shr` (`d,a,b,ic`) | three-address `d,a,b` + `ic`; fast int/float path, slow path via [02](02-value-model.md) kernels | ✓ |
| Compare | `CmpEq` `CmpIdentical` `CmpLt` `CmpLe` `Spaceship` (`d,a,b,ic`) | `d,a,b` + `ic`; **PHP 8 loose-compare semantics** baked in ([02-value-model.md](02-value-model.md)) | ✓ |
| Branch | `Jmp tgt` · `JmpIfTrue s,tgt` · `JmpIfFalse s,tgt` · `JmpTable s,k` | signed relative `tgt`; `JmpTable` indexes a jump table in the const pool; edges profiled (see `ProfileEdge`) | — |
| Array | `NewArray d,i` · `ArrGet d,arr,key,ic` · `ArrSet arr,key,val,ic` · `ArrAppend arr,val` · `ArrUnset arr,key` | packed fast path + IC on key shape ([03-heap-types.md](03-heap-types.md)) | ✓ |
| Property | `PropGet d,o,name(k),ic` · `PropSet o,name(k),val,ic` · `PropInit o,slot,val` | shape-guarded IC: guard `ClassId`, load/store `slots[offset]` ([03-heap-types.md](03-heap-types.md)) | ✓ |
| Call | `CallFn d,callee,base,argc` · `CallMethod d,o,name(k),base,argc,ic` · `CallStatic` · `CallDynamic` · `Ret s` | `base` = first reg of callee window; monomorphic call-target IC; arg adapters | ✓ |
| Object | `New d,class(k),ic` · `Clone d,s` · `InstanceOf d,o,class(k),ic` · `InitProps o` | hidden-class stamp (no transition chain for sealed classes, [03-heap-types.md](03-heap-types.md)) | ✓ |
| Closure | `MakeClosure d,proto(k)` · `BindThis c,o` | upvalue capture from the function's capture descriptors ([§7.4](#74-function-metadata-block)) | — |
| Iter | `IterInit it,src,ic` · `IterNext it,tgt` · `IterValue d,it` · `IterKey k,it` | `foreach` over packed / hashed / `Traversable`; IC on iterable shape | ✓ |
| Type | `TypeCheck s,type(k)` · `Coerce d,s,type(k)` · `CastBool/Int/Float/String d,s` | declared-type enforcement; casts via [02](02-value-model.md) conversion kernels | ◐ |
| Exc | `Throw s` · `EnterTry region` · `LeaveTry region` · `Catch d,class(k)` | references the exception-region table ([§7.4](#74-function-metadata-block)); unwinding is table-driven, not opcode-walked | — |
| Tier | `ProfileEdge` · `LoopHeader` · `SafePoint` · `OsrEntry` | tiering & GC safepoints (see [§7.3.1](#731-tier--safepoint-ops)) | — |

`◐` Type: object/interface checks (`instanceof`-style `TypeCheck`) carry an IC; scalar coercions do not.

The **Compare** group encodes 8.5 semantics directly so interpreter and both JIT tiers stay bit-identical: `CmpEq` is loose `==` (the PHP 8 number-vs-non-numeric-string rule, `0 == "foo"` is **false**), `CmpIdentical` is `===` (tag + value, pointer identity for objects), `Spaceship` is `<=>` returning `-1|0|1`. `match` lowers to `CmpIdentical` decision trees in HIR ([01-frontend.md](01-frontend.md) §6.2), **never** to ordering ops.

### 7.3.1 Tier & safepoint ops

These carry no `dst`; they are observation and cooperation points the interpreter executes cheaply and the JIT consumes or lowers away.

| Op | Meaning |
|----|---------|
| `LoopHeader` | Marks a loop entry. Carries a back-edge counter index in the profiling block; the interpreter bumps it, and crossing the hotness threshold triggers tier-up. Also a candidate `OsrEntry`. |
| `OsrEntry` | An on-stack-replacement entry point: a `pc` at which the optimizing tier may **take over a running loop without restarting the function**, by translating the live interpreter/baseline frame into the optimized frame layout ([07-jit.md](07-jit.md)). Emitted at loop headers and other re-entry-safe points. |
| `SafePoint` | A point where the **cycle collector and deopt may run** ([04-memory-gc.md](04-memory-gc.md), [07-jit.md](07-jit.md)). Between safepoints the executor owns the heap exclusively. Loop back-edges carry an **implicit** safepoint, so `max_execution_time` and bounded cycle collection are honored even in hot JITed loops. The per-back-edge cost is one relaxed load of the interrupt flag, predicted not-taken. |
| `ProfileEdge` | Records branch direction / edge weight into the profiling block, feeding region formation ([07-jit.md](07-jit.md)). In Tier 1/2 it is lowered to a counter increment or elided once the region is formed. |

`SafePoint` and `OsrEntry` placement is a contract with [04-memory-gc.md](04-memory-gc.md) (precise stack maps exist at every safepoint) and [07-jit.md](07-jit.md) (deopt targets a bytecode `pc` that is reachable as interpreter state).

---

## 7.4 Function metadata block

A compiled function is split into an **immutable, shared, immortal** part (the persistent on-disk-cacheable unit, [00-overview.md](00-overview.md) §1) and a **per-isolate mutable** side part allocated on first activation:

```rust
#[repr(C)]
pub struct FunctionBody {                 // immutable, immortal (ADR-009), content-addressed
    code:        Box<[u8]>,               // the instruction stream (§7.2)
    consts:      Box<[Value]>,            // constant pool: literals, names, jump tables
    ex_regions:  Box<[ExRegion]>,         // exception-handler regions (try/catch/finally)
    signature:   Signature,              // declared param/return types, by-ref, variadic, defaults
    upvals:      Box<[UpvalDesc]>,        // closure capture descriptors (by-value | by-ref slot)
    spans:       SpanTable,              // per-instruction source spans, delta-compressed
    frame_size:  u16,                    // packed register count (from linear scan, §8)
    ic_count:    u32,                    // size of the per-isolate IC table
    profile_meta: ProfileLayout,         // counter/histogram offsets reserved per site
}

pub struct ExRegion {
    try_lo:    u32, try_hi: u32,          // [lo,hi) bytecode range covered
    catches:   Box<[(ClassId, u32)]>,    // (catch type, handler pc), in source order
    finally:   Option<u32>,              // finally entry pc
}

// Allocated per isolate on first activation, indexed by the immutable indices above:
pub struct FunctionState {
    ics:     Box<[IcSlot]>,              // ic_count slots — mutable inline caches (§7.1)
    profile: ProfileBlock,               // back-edge & branch counters, type-feedback histograms
}
```

- **Constant pool.** Holds literals too large for inline immediates, interned name strings (class/method/property names, immortal — [03-heap-types.md](03-heap-types.md)), and `JmpTable` targets.
- **IC slot table.** `ic_count` slots; an instruction's `ic` field indexes here. Lives in the mutable `FunctionState`, never in `code`.
- **Exception regions.** Unwinding is **table-driven**: `Throw` searches `ex_regions` for the innermost region whose `[try_lo, try_hi)` contains the throwing `pc` and whose catch type matches, with `finally` targets chained — no per-frame handler stack walked in the common (no-throw) path, which pays nothing.
- **Signature.** Declared types are facts the optimizer assumes ([07-jit.md](07-jit.md)); `CallFn`/`CallMethod` arg adapters bind/coerce against it.
- **Upvalue descriptors.** Drive `MakeClosure` capture (by value or by a shared `Reference` cell, [03-heap-types.md](03-heap-types.md) §11.4).
- **Spans & profiling.** Per-instruction spans ([01-frontend.md](01-frontend.md)) back backtraces, deopt diagnostics, and JIT-dump source maps; the profiling block is the type-feedback substrate read by [06-interpreter.md](06-interpreter.md) and consumed by [07-jit.md](07-jit.md).

---

## 8. Compiler — `rphp-compiler`

`rphp-compiler` lowers HIR to bytecode. It is **deliberately thin**: HIR already removed the surface complexity ([01-frontend.md](01-frontend.md) §6 — desugaring, name resolution, const-folding), so the compiler is a layout-and-allocation pass, not an optimizer. It does:

1. **Linear-scan register allocation** over SSA-ish HIR temporaries — compute live ranges, assign each to a physical register, packing the unbounded virtual file ([§7.1](#71-register-vm-model)) down to `frame_size`. Linear scan is chosen over graph-coloring for the same reason the whole tier is thin: cheap, fast, good-enough; the JIT re-allocates anyway in unboxed SSA.
2. **Constant-pool construction** — dedupe literals and interned names; emit `JmpTable`s for `match`/`switch`.
3. **IC-slot assignment** — allocate one slot per polymorphic site, set `ic_count`.
4. **Exception-region layout** — build `ex_regions` from HIR try/catch/finally; place `EnterTry`/`LeaveTry`/`Catch`.
5. **A small peephole pass** — **dead-move elimination** (collapse `Mov` chains a register allocator leaves behind), **constant propagation of already-folded values** (forward `LoadConst` into consumers where trivial; heavy folding already happened in HIR §6.3), and **jump threading** (collapse `Jmp`→`Jmp` and branch-to-branch).

**Heavy optimization is *not* here.** It happens in [07-jit.md](07-jit.md), where **runtime type feedback** exists — which is the only place speculation pays off for a dynamic language. This is the **"compile cheaply, optimize late"** tiering thesis ([00-overview.md](00-overview.md) §1): get correct bytecode out fast and let hot code climb. The compiler must be cheap enough that cold-start CLI scripts feel no compile wall, with the on-disk code cache skipping even that on warm runs.

---

## 12.2 Frames & call ABI

A single **contiguous VM stack** holds frames. A **frame is a register window** into that stack — `frame_size` consecutive `Value` slots beginning at a `base` index. There is no per-frame heap allocation for registers; calls grow and shrink the one stack.

```rust
#[repr(C)]
pub struct CallFrame {
    func:       *const FunctionBody,   // shared immutable body (§7.4)
    base:       u32,                   // index of register 0 in the VM stack
    return_pc:  u32,                   // resume point in the caller's code
    return_dst: u16,                   // caller register receiving Ret's value
    tier:       Tier,                  // 0 interp | 1 baseline | 2 optimizing
    ic_base:    u32,                   // into the isolate FunctionState (§7.4)
}
```

- **Zero-copy argument passing.** The caller materializes the callee and its arguments directly into the registers `base, base+1, … base+argc` of its *own* window; the `Call` op then sets the **new frame's `base` to that same slot**, so the callee's parameters are already in place. Arguments are **not** copied through an intermediate buffer — the caller's tail registers *become* the callee's incoming registers (the Lua 5 / Dalvik calling convention). `Ret s` writes `s` back to the caller's `return_dst`.
- **One ABI, all three tiers.** The call ABI — window layout, `base`/argc convention, return slot — is **shared verbatim by Tier 0, Tier 1, and Tier 2**. Consequences:
  - Interpreted and compiled frames **interleave on one stack**; a Tier-2 region can call an un-inlined Tier-0 callee and vice versa with no thunk.
  - **OSR and deopt swap a frame's tier *in place*** ([06-interpreter.md](06-interpreter.md), [07-jit.md](07-jit.md)): the `base`, register contents, and `func` are unchanged; only `tier` and the executor (handler loop vs native entry) flip, with the OSR/deopt map translating `pc` ↔ native IP and reboxing/unboxing live registers across the [02-value-model.md](02-value-model.md) boundary.
- **Safepoints & stack maps.** At every `SafePoint` the layout is GC-walkable: the collector finds roots in interpreted frames by `frame_size` and in compiled frames by the precise stack map ([04-memory-gc.md](04-memory-gc.md), [07-jit.md](07-jit.md) §13.3). A contiguous stack of typed windows is what makes both root-finding and zero-copy calls cheap.

---

## Deviations from base-idea.md

- **None changed.** §7 (register ISA), §8 (thin compiler), and §12.2 (windowed frames, zero-copy call ABI) are **affirmed and detailed**. The register-ISA-over-stack-machine choice is a re-confirmed baseline decision ([decisions.md](decisions.md), Affirmed list).
- **Affirmed/added — decode shape tied to ADR-001.** The single-load + table-dispatch decode and the one-op→one-stencil property are made explicit as a *requirement* serving both the `become`-threaded interpreter (ADR-001) and the copy-and-patch JIT, with the `WIDE` prefix re-dispatched through a parallel stencil table so the property survives operand widening.
- **Added — IC slots are a per-isolate side table, not inline (ADR-009).** New, load-bearing consequence of the immortal-shared-bytecode invariant: the immutable `code` carries only the IC *index*; mutable IC state and profiling counters live in a per-isolate `FunctionState`. The baseline said IC state lives "beside the bytecode"; this pins *where*, so the shared body stays immutable across isolates.

## Open questions

- Final default operand widths (u8 reg / u16 const-and-ic) vs the `WIDE` promotion threshold — validate against register and constant-pool histograms from real `.phpt`/framework bodies in M1.
- Whether `OP_EXT`'s long-tail table should itself be `WIDE`-aware or whether long-tail ops are simply never wide (they are cold) — decide once the opcode count past 256 is known.
- Span-table compression scheme (delta vs interval) and whether spans live in the immortal body or a lazily-loaded sidecar to shrink the resident code-cache footprint — tune in M1 against cache size.
- IC slot record layout (one cache line per polymorphic site vs packed) and its interaction with the profiling block — co-design with [06-interpreter.md](06-interpreter.md) once the interpreter's guard sequence is fixed.
