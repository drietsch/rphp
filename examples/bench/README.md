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
