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
