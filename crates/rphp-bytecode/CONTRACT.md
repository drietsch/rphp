# rphp-bytecode v2 contract

The bytecode crate is the contract between the compiler (producer), the
runtime (consumer) and the stdlib/reflection surface (reader of metadata).
This file summarizes the *behavioural* contract the v2 types in `src/` assume;
the per-item details live in the doc comments (`cargo doc -p rphp-bytecode`).
Plan reference: `Track E` (E3 calls, E5 exceptions, E6 objects, E7 units,
E8 generators, E10 reflection, E11 performance).

Status: **frozen but not yet lowered.** Every v2 item is additive next to the
M0 set; the current compiler never emits a v2 op and the tier-0 interpreter
returns a runtime error for any of them. Section 8 lists exactly what is not
implemented.

## 1. Frames and registers

* A frame belongs to one `Function`; it owns `num_regs` registers on one
  contiguous, process-wide register stack. Registers hold `Value`s; a register
  may hold a `Value::Ref` (a shared cell) or `Value::Uninit`.
* Reads dereference (`Ref` → cell contents). Only the ops documented as
  writing *through* a reference do so: `AssignThroughRef`, `AssignOp*`,
  `IncDec`, `RecvInit`, `SendVar` (by-ref), `BindStatic`/`BindGlobal`
  (rebinding). `Move`, `LoadConst`, arithmetic etc. *replace* the register.
* `$this`, the lexical scope class and the late-static-bound class are frame
  slots, not registers (`LoadThis` reads `$this`). This is what lets the
  argument window start at register 0 for methods too.
* Frame kinds (runtime `Frame.kind`): `Normal`, `ReentryBoundary` (pushed by
  natives that call back into the VM via `run_until(depth)`; unwinding stops
  at it and returns `Err(Unwind)` to the native), `Generator` (a parked
  frame owned by a `Generator` object), `Include` (runs a unit's `{main}`
  sharing the includer's symbol table, `$this` and scope), `Native`.
* `Function.lines[pc]` gives the source line for every diagnostic;
  `Function.ic_count` sizes the frame's inline-cache slots (`ic` operands).

## 2. Calling convention: `Init*` → `Send*` × n → `DoCall`

1. `InitFCall` / `InitDynCall` / `InitMethodCall` / `InitStaticCall` /
   `InitNew` resolves the target **now** (undefined function/method/class
   errors happen here, before arguments are evaluated — PHP order) and pushes
   a *pending-call record* on the frame: `{target, this, scope, static_class,
   argc = 0, named: [], ic}`. Records nest: a call in an argument position
   pushes its own record on top and pops it at its `DoCall`.
2. `SendVal{pos,src}` / `SendVar{pos,var}` / `SendRefElem` / `SendRefProp`
   stage argument `pos` (0-based, ascending) into the **zero-copy window**:
   the next free registers at the top of the register stack, which become
   the callee's registers `0 .. argc`. By-reference decisions use the resolved
   target's `ParamDef.by_ref` (so `SendVar` on a by-ref parameter shares a
   `Ref`, on a by-value parameter copies). `SendUnpack` spreads (string keys →
   named). `SendNamed` follows all positional sends and is placed by the
   runtime using the callee's `params`.
3. `DoCall{dst}` checks arity (`ArgumentCountError`) and coerces/checks
   parameter types under the **caller's** `STRICT_TYPES`, pushes the callee
   frame over the window, runs it, pops it and stores the result in `dst`.
   The callee's prologue then runs `RecvInit` (defaults for missing
   parameters, `InitRef::Const` or a `Thunk` run in the callee's class scope),
   `RecvVariadic`, `BindStatic`/`BindGlobal`, and `BindSymtab` for
   `NEEDS_SYMTAB` functions.
4. `MakeCallableClosure{dst}` consumes a record with no sends and yields a
   `Closure` (first-class callable syntax).
5. `Ret{src}` / `RetRef{var}` return; return-type coercion uses the
   **callee's** `STRICT_TYPES`. `GenReturn` finishes a generator.

Natives take `(&mut Ctx, &mut [Value])` over the same window; by-ref
positions are dereferenced before the call and written back afterwards.

## 3. Late binding and names

Functions, classes, constants and catch types resolve at runtime through the
interpreter's tables (plan D5). Every name operand is a `Const::Name` — the
original spelling, an ASCII-lowercased twin and an FNV-1a hash of the twin —
so the hot path never lowercases or rehashes. `InitFCall`/`FetchConst` carry
the namespace-fallback name. `ClassRef` packs `Named(pool idx)` / `self` /
`parent` / `static` / `Reg` into 32 bits; `NameRef` packs a member name that
is either a pool constant or a register (`$o->$p`, `$o->$m()`, `A::{$x}`).
Method names in the pool are `Const::Name`; property/static-property/class
constant names are `Const::Str` (case-sensitive).

Inline caches (`ic: u16`) are monomorphic slots stamped with the table
generation; declaring a function/class bumps the generation.

## 4. `NEEDS_SYMTAB` frames

Set on every unit `{main}` and on any function that uses `include`/`require`,
`eval`, `compact`, `extract`, `get_defined_vars`, `$$x` or `$GLOBALS` writes.
`BindSymtab` is the first op: the frame gets a name → `Ref` table and every
`Function.var_names` register is bound to its cell (so later `extract()` or an
included file writing `$x` is visible in the register). An `Include` frame
reuses the includer's table; the entry script's `{main}` binds to the globals
table; `global $x` (`BindGlobal`) binds to the globals table from anywhere.
`FetchGlobals` yields a read-only copy; `AssignGlobal` writes through.

## 5. Exceptions: `ExRegion` unwinding

Per function, `ex_regions` is ordered **innermost-first**. For a fault at
`pc` the runtime takes the first region that applies:

* `try_lo <= pc < try_hi` → try the `catches` in order; each type is a
  `Const::Name` matched by `instanceof` **without autoload**; on a hit store
  the object in `dst` and continue at `handler`.
* else, if the region has a `finally` and `try_lo <= pc < finally.entry`
  (try body *and* catch handlers) → `state = Throw`, `payload = exception`,
  continue at `finally.entry`.
* no region → pop the frame (releasing iterators, the symbol table, and
  restoring `@` silence depth), continue unwinding in the caller; a
  `ReentryBoundary` returns `Err(Unwind::Throw)` to the native; an empty
  stack is an uncaught exception (rendered like php-cli, exit 255).

Every `finally` body ends in `FinallyEnd{state, payload, targets}` with
`FinallyState` codes `None` (fall through) / `Throw` (rethrow from this pc —
enclosing regions apply normally) / `Return` (return `payload`) / `Jump`
(continue at `targets[payload]`, a `Const::JumpTable`). The runtime never
chains finally bodies: crossing several is lowered by the compiler with
`Jump` + trampoline stubs that move the payload into the outer region's
registers; only the outermost crossing uses `Return`. A `return` inside a
finally body overrides a pending exception (PHP semantics). An exception
thrown inside a finally body while `state == Throw` gets the pending one as
`previous`. Destroying a suspended generator runs the protecting finally
bodies innermost-first with `state = None`.

Engine faults are throws (`TypeError`, `Error`, `DivisionByZeroError`,
`ArgumentCountError`, `UnhandledMatchError` via `MatchError`, …); warnings go
through the diagnostics channel honouring `Silence` brackets.

## 6. `InitRef` thunks and metadata

`InitRef::Const(k)` is a literal in the owner's pool (`Function.consts` for
parameters/statics/attributes, `ClassDecl.pool` for class members).
`InitRef::Thunk(f)` is a zero-argument function in the same unit returning
the value; it runs lazily in the owner's class scope: per evaluation for
parameter defaults and static-variable inits, once (cached by the runtime,
with cycle detection) for class constants, property defaults and enum cases.
Reflection reads `ParamDef`/`PropDecl`/`ConstDecl`/`AttrDef` without
evaluating anything until asked (`getDefaultValue`, `newInstance`).

`TypeDecl` is owned and engine-independent; its `Display` is PHP's canonical
spelling (`?int`, `string|int`, `A|B|null`, `(A&B)|null`).

## 7. Units

`CompiledUnit{file, funcs, classes, main, hoist_funcs, hoist_classes,
strict_types, halt_offset}` is one file or `eval` string. `load_unit` declares
`hoist_funcs` and `hoist_classes` first, then runs `{main}` as an `Include`
frame; conditional/nested declarations happen at their `DeclareFunction` /
`DeclareClass` / `DeclareConst` ops in statement order. Cross-unit references
never use ids — always names.

## 8. What the current compiler and runtime do **not** implement

Everything v2 is "not lowered yet":

* Ops after `Op::Echo` (the whole `Init*`/`Send*`/`DoCall` sequence,
  prologue ops, references/fetch-for-write, isset/empty/unset, late-bound
  names, `Switch`/`MatchError`/`Silence`/`Include`/`Eval`/`Declare*`/`Exit`/
  `Throw`/`FinallyEnd`, generators, `Clone`, `Iter*`, `InstanceOfRef`,
  compound assignment, `IncDec`, `Cast`, bitwise/shift ops, `ConcatN`,
  `LoadThis`, `FetchProp`/`AssignProp`). The tier-0 interpreter's wildcard
  arm returns `internal error: opcode not implemented` for them.
* `Function`'s v2 fields (`params`, `ret_ty`, `flags`, `ex_regions`,
  `captures`, `statics`, `var_names`, `lines`, `ic_count`, `doc`, `attrs`,
  `decl_line`, `end_line`, `in_class`) are emitted empty by the compiler.
* `ClassDecl`, `CompiledUnit` and `Const::{Name, JumpTable, ArgNames, Type,
  Bool, Null}` have no producer or consumer yet; `Module`/`Class` remain the
  live path until E3/E6 remove `Call`/`CallNative`/`CallDynamic`/
  `MethodCall`/`StaticCall`/`New`/`PropGet`/`PropSet`/`InstanceOf`/
  `MakeClosure`/`ForeachNext`, `Function.capture_regs`/`closures`, `Class`,
  `PropDef`, `Method`, `Module`.
* Deliberately left out of the ops (lowerable from existing ones or too rare
  for the freeze): a dynamic-name static-property fetch beyond `NameRef`,
  `NewDyn` (covered by `InitNew{ClassRef::reg}`), `InstanceOfDyn` (covered by
  `InstanceOfRef{ClassRef::reg}`), a fused `CallFn` (E11 peephole, later),
  `AssignOpStaticProp` (lower via `RefStaticProp` + `AssignOp`).
