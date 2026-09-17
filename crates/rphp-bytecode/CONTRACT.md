# rphp-bytecode v2 contract

The bytecode crate is the contract between the compiler (producer), the
runtime (consumer) and the stdlib/reflection surface (reader of metadata).
This file summarizes the *behavioural* contract the v2 types in `src/` assume;
the per-item details live in the doc comments (`cargo doc -p rphp-bytecode`).
Plan reference: `Track E` (E3 calls, E5 exceptions, E6 objects, E7 units,
E8 generators, E10 reflection, E11 performance).

Status: **frozen; the E3 slice is live.** The compiler emits the v2 call
sequence, prologue, reference / fetch-for-write, isset / unset, late-bound
name, control-flow and operator ops, and the runtime executes them over an
explicit frame stack (plan E3). Section 8 lists what is still not lowered
(E5 exceptions, E6 object model, E7 units, E8 generators) and the additive
changes E3 made to the frozen set.

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

## 8. What the current compiler and runtime do **not** implement, and the E3 additions

### 8.1 Additive changes made by E3 (plan E3, ADR-016/019)

* **New ops** — needed by the lowering and missing from the freeze:
  * `AssignRefElem{arr, key: Option<Reg>, src}` — `$a[k] = &$x` / `$a[] = &$x`
    (`RefElem` binds the *variable* to the element; this binds the *element*
    to the variable's cell).
  * `AssignRefProp{obj, name, src}` — `$o->p = &$x`.
  * `FetchDynVar{dst, name, global}` / `BindDynVar{reg, name, global}` —
    symbol-table access by a runtime name (`$$x`, `${expr}`, `$GLOBALS[$k]`
    as a write container); §4 mentioned `$$x` without an op for it.
  * `BindStaticOrJmp{reg, idx, target}` — `static $x = <expr>` with a
    non-constant initializer: php 8.3 evaluates the expression in the
    function's own scope (it may read locals and recurse), which a
    zero-argument thunk cannot do; the initializer is compiled inline
    between two `BindStaticOrJmp`s (see the op's docs).
  * `ListGet{dst, base, key}` — the element read of a destructuring
    pattern (`[$a, $b] = $x`), whose non-array diagnostics differ from
    `ArrayGet` (`Cannot use int as array`, silent null).
* **`Module` gained `hoist_funcs`, `hoist_classes` and `file`** (the
  `CompiledUnit` fields the runtime needs today; `Module::new_hoisted` keeps
  the M0 shape), and **`Class` gained `line`** (for `Cannot redeclare class
  X (previously declared in file:line)`).
* **`MakeClosure{dst, proto}`** is reused with `proto` = the closure
  function's unit-local `FuncId`; the closure's `Function::captures`
  (`CaptureDesc{src, dst, by_ref}`) drive the capture, `capture_regs` /
  `closures` / `ClosureProto` are no longer produced. The runtime's interim
  closure value (`Value::Closure`, until E6 makes closures objects) stores
  `[captures…, bound $this | null, scope class id | null]`.
* **`FetchElemW` / `FetchPropW` semantics refined**: the handle is not a
  `Ref` to the slot (a permanent reference cell would change `$b = $a` copy
  semantics, since refcount-1 references are not collapsed on array copy).
  Instead the element is **moved out** of its container (or, when the
  element already is a reference, its cell is shared) into `dst`, the nested
  lvalue op mutates it in place, and the compiler **writes it back** with
  `ArraySet` / `ArrayPush` / `AssignProp` in reverse order. The intermediate
  state is unobservable because every key and the assigned value are
  evaluated before the fetch chain starts.
* Symbol-table frames: any op that rebinds a named register to another cell
  (`AssignRef`, `RefElem`, `RefProp`, `BindGlobal`, `BindStatic`,
  `IterNext` by reference) also updates the frame's table entry for that
  name, so `compact()` / `get_defined_vars()` see the new binding.
* `Function.lines` is populated by the compiler; `{main}` ends with
  `return 1` (the value an `include` of the unit evaluates to).

### 8.2 Not lowered / executed yet

* **E5**: `try`/`catch`/`finally` (`ExRegion`, `FinallyEnd`), `Throw` is
  executed as `Unwind::Throw(object)` but nothing catches it; engine faults
  stay `Unwind::Pending` (no Throwable objects); VM warnings for undefined
  variables / the `Only variables should be passed by reference` notice.
* **E6**: `ClassDecl` (interfaces, traits, enums, class constants, static
  properties and methods, abstract/readonly/hooks/promotion, magic methods,
  `__invoke`, `Closure` as a class, `ArrayAccess`/`Iterator` protocols,
  `MakeCallableClosure`, `FetchClassConst` except `::class`,
  `FetchStaticProp`/`AssignStaticProp`/`RefStaticProp`/`IssetStaticProp`
  (executed as `Access to undeclared static property`), `Clone` with
  `__clone`/`with`). Anonymous classes.
* **E7**: `Eval`, namespaces (`InitFCall.ns_fallback`, `FetchConst.ns_fallback`
  are always `None`), `use` imports, `FetchClass.autoload`, `__halt_compiler`.
  `Include` is executed for files (no `include_path` search beyond the ini
  value, the including file's directory and the cwd).
* **E8**: `Yield`, `YieldFrom`, `GenReturn`, `FnFlags::GENERATOR`.
* `Const::ArgNames`, `Const::Type` have no producer; `TypeDecl` metadata
  (`ParamDef.ty`, `ret_ty`) is not emitted (types are ignored).
* `RetRef` / `FnFlags::RETURNS_REF`: `function &f()` compiles with the flag
  set but returns by value.
* The M0 ops (`Call`, `CallNative`, `CallDynamic`, `New`, `PropGet`,
  `PropSet`, `MethodCall`, `StaticCall`, `InstanceOf`, `ForeachNext`) and
  `Function.capture_regs`/`closures` are dead: the compiler never emits them
  and the runtime returns an internal error for them. They are removed
  together with `Class`/`PropDef`/`Method`/`Module` when E6 switches to
  `ClassDecl`/`CompiledUnit`.
* Deliberately left out of the ops (lowerable from existing ones or too rare
  for the freeze): a dynamic-name static-property fetch beyond `NameRef`,
  `NewDyn` (covered by `InitNew{ClassRef::reg}`), `InstanceOfDyn` (covered by
  `InstanceOfRef{ClassRef::reg}`), a fused `CallFn` (E11 peephole, later),
  `AssignOpStaticProp` (lower via `RefStaticProp` + `AssignOp`).
