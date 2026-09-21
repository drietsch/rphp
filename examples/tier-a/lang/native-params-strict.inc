<?php
declare(strict_types=1);
// Under the caller's strict_types only an int widens to a float.
t(fn() => strlen(12)); t(fn() => str_repeat("a", "3")); t(fn() => round(3)); t(fn() => strlen(null)); t(fn() => abs("1")); t(fn() => str_pad("a", 3.0));
t(fn() => substr("abc", 1.0)); t(fn() => sqrt(4)); t(fn() => str_contains("abc", 1)); t(fn() => array_fill(0, "2", 1)); t(fn() => array_slice([1, 2, 3], 1, 1, "1"));
t(fn() => strlen(true)); t(fn() => implode(",", null)); t(fn() => substr("abc", 1, null));
