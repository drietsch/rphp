# Micro-benchmarks

Timing probes, run by hand against stock php (they are not part of the
differential corpus, which compares output, not time):

    php -n examples/bench/array-writes.php
    target/release/rphp examples/bench/array-writes.php

`array-writes.php` and `array-passing.php` catch accidental copy-on-write
(an array copied on every append or every call is quadratic — the 2026-09-20
finding that made every `$a[] = $v` O(n)); `interp-baseline.php` is the
interpreter's per-op cost next to php's VM.

Measured 2026-09-20 (release build, M-series Mac; php 8.5 without opcache):
`$a[] = $i` ×20000 4 ms (php 0), a 20000-element array passed and returned
×20000 6 ms (php 0), typed `array $a` the same; the baseline is 15–65×
php's (arithmetic loop 26×, user calls 17×, method calls 50×, native calls
65×, `new` 13×) — the interpreter's dispatch, register reads that clone,
and the native call path are the next performance wave. symfony/demo's
`/en/blog/` under `rphp -S`: 2.2 s → 0.48 s warm over the day (php 75 ms);
the compiled-unit cache took it from 0.84 s (every request had parsed and
compiled ~1500 files, then read them again) to 0.48 s.

`function-vs-global.php`: top-level code keeps its variables in the symbol
table (reference cells), so the same loop runs ~3× slower at file scope
than in a function — write benchmarks (and hot code) inside functions.
Numeric fast paths (int/float operands read straight from the registers)
took the in-function arithmetic loop to 12× php; the native call path is
~130 ns per call (php 2 ns): the 272-byte `Frame` moved three times, the
argument copies for traces, the output flush check.

Measured 2026-09-21 after the second wave (release build; in-function
numbers from a copy of `interp-baseline.php` wrapped in a function): user
calls 195 → 103 ms per million (php 14), method calls 517 → 165 (php 11),
a getter 388 → 162, closure calls 208 → 112, native calls 276 → 188 per
two million (php 7), `new` 51 → 41 per 300k. symfony/demo's `/en/blog/`
0.48 s → 0.32 s (php 0.064). What did it:

- **A property inline cache** (`IcSlot::PropRead`/`PropWrite` in
  `rphp-runtime/src/unit.rs`): a `$o->name` site with a constant name
  remembers the slot it resolved for the object's class from its scope,
  and a hit reads or writes the slot without the name lookup, the
  visibility check or the type coercion (a write caches only an
  untyped/scalar-typed, non-readonly, unhooked slot and stores a value of
  exactly that type). A site with a dynamic name (`$o->$n`) is never
  cached — Symfony's hydrator closure writes every property from one.
- **The call path allocated per call**: two `String`s for a "called in"
  site nobody read unless a `TypeError` was raised, the callee's name in
  the pending call (only a `__call` trampoline needed it — its descriptor
  carries it), and a 272-byte `Frame` whose rarely used parts (variadic
  leftovers, symbol table, iterators, generator/include bookkeeping, a
  closure's statics) now live behind `Frame::extra`, boxed on first use.
- **Temporaries holding containers**: a statement's temporaries lived on
  in their registers until reused, so `isset($this->loaded[$c])` left a
  handle on the array that made the `$this->loaded[$c] = true` right
  after it copy the whole array — Symfony's DebugClassLoader did that
  for every class it loaded (quadratic). The compiler now emits
  `Op::FreeTemps` at a statement's end (and before an `if`/`while`/`for`
  condition's branch) over the temporaries that received a value which
  may be a container; scalar arithmetic temporaries are left alone.
  `FetchElemW` and `WriteBackElem` held a clone of the container across
  the write for the same reason (the `ArrayAccess` check), which made
  every `$this->x[$k][] = $v` copy `$this->x`.
- **Compiled functions are shared** with the per-thread unit cache
  (`Module.funcs: Vec<Rc<Function>>`, `FuncRt.f: Rc<Function>`): a
  request's link no longer clones every function's code.

How the copies were found: a temporary trace in `array_set`/
`fetch_elem_w` printing `owners()` of the container (via
`Array::owners()`) with the calling site whenever a write to an array of
64+ elements had more than one owner — run against the demo page, then
reproduced in a five-line script. Any `rd()` clone of a container held
across a write, and any register left holding one, shows up there.

Third wave, the same day: method calls 165 → 118 ms per million, the demo
page 0.32 → 0.29 s:

- **A method inline cache** (`IcSlot::Method`): a constant-name `$o->m()`
  site remembers the method its dispatch resolved for the object's class
  from its scope (visible, not abstract, not a `__call` trampoline), so a
  hit pushes the call without the name lookup — which allocated the name
  and hashed it every call. `examples/tier-a/lang/method-sites.php` runs
  one site over subclasses, a shadowed private method, `__call`, a trait
  method, a static method through an instance and a closure receiver.
- **hashbrown/foldhash** behind arrays, object layouts, the class,
  function and native indexes: std's SipHash was 4 % of the demo page by
  itself.
- **`fopen(…, 'a')` writes straight to the file**: the stream design read
  a file whole on open and wrote it back on close, so Monolog's append to
  a growing `dev.log` cost the whole log per request (and two workers
  would have lost each other's lines).
- **The include path** keeps a realpath cache (120 s, php's
  `realpath_cache_ttl`) and trusts a cached unit for 2 s without a `stat`
  (opcache's `revalidate_freq`).

What the demo page's profile shows now (a worker's time): the plain
interpretation of Symfony's code — callbacks, Twig's generator-rendered
templates — with `open(2)` of the log and profiler files (4–6 ms each on
this machine, php pays the same), class linking per request (~12 %: php
without opcache re-links too), and `serialize()` of the profiler data.

Fourth wave — **algorithmic** traps, found by timing loops of a few hundred
thousand operations against php (`/tmp`-style scripts; the ratios that
were 1000× and more were not constant factors):

- `$s .= $x` copied the whole string every time (21 s for 300k appends of
  a short line; php 12 ms): `Str` now keeps a `Vec` and appends in place
  when the handle is unique (`Str::push_bytes`), which `AssignOp`,
  `$o->prop .= …` (declared or dynamic property) and string offset writes
  (`$s[$i] = 'x'`) use. 53 ms now.
- Every by-reference native call copied its array: `array_pop($a)` in a
  loop was O(n²) (195 s for 100k pops of a 200k array; php 1 ms). The
  call boundary now **takes** a by-reference array out of its cell for
  the call (the handler holds the only handle) and puts it back after;
  a native with a callback (`usort`, `array_walk`) keeps copy semantics
  so its callback sees the variable as php's does. `array_pop`,
  `array_shift`, `array_unshift`, `end`/`reset`/`next`/`prev` work on
  the array in place (`Array::pop/shift/unshift`, the pointer moves)
  instead of rebuilding it: 30 ms, 4 s (php's own shift is O(n): 2.8 s),
  330 ms, 57 ms.
- `unset($a[$k])` held a clone of the container across the unset, so a
  `foreach ($a as $k => $v) unset($a[$k])` loop copied the array per
  element (996 s for 200k; php 4 ms): 56 ms now. `in_array`/
  `array_search`/`array_keys` compared through the engine's operand
  conversion for every element (35×): only an object beside a scalar
  needs it now. `array_slice` snapshotted the whole array per call.

The ratios that remain are constant factors (2–10×): the native call
boundary, `serialize`, `htmlspecialchars`, generators (Twig renders
through them: 6× php per yield).

Fifth wave (still 2026-09-21): the SPL containers rebuilt their backing
array per operation — `SplObjectStorage` (77 s for 20k attaches; php
1.5 ms), `ArrayObject` (22 s for 100k writes), `SplStack`/`SplQueue`/the
heaps (4 s for 20k), `WeakMap` (0.9 s) — and work on it in place now,
`SplObjectStorage` and `WeakMap` indexed by object handle (30 ms, 40 ms,
16 ms / 65 ms, 24 ms). `str_contains`/`strpos` searched byte by byte
(`memchr::memmem` now: 1.45 s → 64 ms for 500k searches of a 1 KB
string). Static properties and class constants got inline caches
(`IcSlot::StaticProp`, `IcSlot::ClassConst`; a named class also resolves
from its prelowercased name without allocating): 54 → 29 ms and
55 → 30 ms per 500k (php 1.6 / 1.3).

`SplQueue` is a deque since the property-purposes batch (200k
enqueue/dequeue: 105 ms, php 9 ms).

Sixth wave, the interpreter's per-op cost (`interp-loop.php`,
2026-09-21). A counted loop with one addition ran 10 ops per iteration
where php runs 4, at 40 ns (php 2.8): every read of a local was preceded
by a `CheckVar` (the compiler forgot what was assigned at every branch),
every literal went through a `LoadConst` into a temporary, the result of
every operator was moved into its variable by a second op, and every
register write called the drop glue. Now: the compiler carries definite
assignment across branches when nothing in the body can `unset()` a
local (`FnCompiler::structured_assign`; each alternative of an `if`,
`switch`, `try` comes back to the set assigned before it, and so does the
end), a literal operand names the pool directly
(`rphp_bytecode::CONST_OPERAND`, for the operators, comparisons and
`SendVal`), an operator computes straight into the variable it is
assigned to (`store_var` folds the store into the producer when nothing
jumps between them; `Function::var_count` tells the runtime which
registers may hold a reference cell to store through), a comparison
feeding a branch is one `JmpUnless`, and a scalar's overwrite skips the
drop (`Value::overwrite`). The loop is php's 4 ops now, at 17 ns
(6×, from 14×); at the top level, where every variable is a symbol-table
cell, 26 ns (from 108: the fast paths look through the cell).

The native call boundary: `abs()` cost 90 ns per call against php's 6.5
(`strlen`/`count` are dedicated opcodes in php) — a frame push with a
copy of the arguments, the pooled argument vector, the object-parameter
check, the named-argument binding, the fault-site capture, the flush.
The builtins that dominate call counts (`Interp::LIGHT_NATIVES`,
`FnFlags::LIGHT`: `strlen`, `count`, `abs`, `max`, the `is_*`, the string
and array functions of their arguments alone) now take a **frameless
path** in `DoCall`: the handler runs over the argument window and
nothing else; a diagnostic it would emit, an exception, or any user code
it would run (`__toString`, a callback) aborts it with `Unwind::Retry`
before anything observable happens, and the call re-runs on the full
path with a frame — so traces, error handlers and side effects are
exactly the full path's. An object argument goes to the full path
outright (it may need fitting to the parameter). 59 ns per call now
(the copy kept for the retry costs ~10 of them).

User calls (88 → 70 ns, php 11): a call or return no longer leaves
`run_frame` and re-enters it through `run_until` (the frame switch is a
`continue 'frames` that reloads the locals), the per-function facts a
call needs (`required`, `has_typed_params`) are computed at link, and the
return value is moved rather than cloned into the caller's register.

Array keys: `ArrayKey::Str` holds the `Str` itself (a refcount bump per
`$a[$k]`, never a copy of the bytes) and hashes through the string's
cached hash, so a key looked up twice is hashed once — string-keyed reads
6M: 1134 → 854 ms (php 177), a `foreach` copy of 300 string keys × 2000:
87 → 35 ms (php 9.5). Class/function/method lookups lowercase their query
on the stack (`with_lowercase`) instead of allocating. The per-request
load of a unit no longer rebuilds each function's constant and
register-name tables (`Function::derive_tables`, computed once at compile
time and shared).

**The demo page, measured properly.** `rphp -S` serving symfony/demo's
`/en/blog/` costs **192 ms of CPU per request against php's 26.5 ms**
(`ps -o utime,stime` over 20 requests; wall clock is dominated on this
machine by `open()` — the profiler's `index.csv` and Monolog's log take
5–20 ms to open right after being written, for php too). The `profile`
cargo feature (`cargo build --release -p rphp --features profile`) prints
at request end the functions that executed the most ops and the natives
that took the most inclusive time: 5.0 M ops per request, 46 % of them in
Symfony's dev-mode `DebugClassLoader` (`checkAnnotations`/`parsePhpDoc`
over 608 classes, as in php); the natives' inclusive time is spread thin
(`class_exists` 20 ms over 1435 calls, autoloads included; `serialize`
4 ms; Reflection and pcre under 3 ms each). The remaining 7× is the
interpreter's per-op cost on real code — property fetches, method calls,
string building — at ~20 ns per op, and the allocator behind it
(`malloc`/`free` were ~20 % of the samples). The `rphp` binary's global
allocator is **mimalloc** now: 192 → 161 ms CPU per demo request,
string-key reads with an interpolated key 854 → 603 ms.

**Prod mode (2026-09-21 evening).** The same page with `APP_ENV=prod`
(`/tmp/demo-*-prod`, `.env.local` with the secret): php 12.5 ms CPU per
request (16 ms wall), rphp 53 ms (52 ms wall) — 4.2×. The `profile`
feature's exclusive per-op-kind table (each op's own time, its nested ops
subtracted) on that request: 1.08 M ops; `DoCall` 18.7 ms (the natives'
own bodies), `IterNext` 18.7 ms at 1.2 µs a step (Twig renders through
nested `yield from` generators: every step resumes a chain of parked
frames through `run_until`), `DeclareClass` 5.7 ms at 9 µs a class,
`InitNew` 4.4 ms (a `new` whose class autoloads pays the unit's link,
`load_unit`, ~7 µs a file), the property ops at 40–110 ns, the plain
ops at ~5 ns. Next in that order: generators resumed inside the dispatch
loop (`continue 'frames` instead of a nested `run_until`), and the
per-request loading — ~900 units linked and ~600 classes declared per
request, php's opcache keeps both.

**Correction, the same evening.** The 1.2 µs generator step was the
profiler's: the ops that switch frames (a user call, its return) were
never recorded, and the bookkeeping between two ops landed in the
exclusive time of the innermost op with a nested run — `IterNext` for
Twig's generators, over every op they render. With the switches
recorded and one clock reading per op boundary, the same request reads
`IterNext` 1.8 ms at 115 ns a step, `DoCall` 21 ms over 64k calls
(natives' bodies and the user-call switch), `Include` 5.3 ms at 5.2 µs
a file (`load_unit`), `DeclareClass` 3 ms at 4.8 µs a class, `Ret`
108 ns. A plain generator step measured by wall clock: 1M `yield`s
104 ms against php's 18 (a `for` loop of the same length: 16 vs 3);
`yield from` chains resume the leaf directly now (php's delegation tree
instead of a `run_until` per level per step), depth 4: 31 ms/200k
(php 5). The order stands: the per-request loading, then the natives'
bodies — `array_sum` over 50 elements is 1.1 µs (php 80 ns) because
the array helpers clone every entry into a `Vec` first.
