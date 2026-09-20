<?php
function loop() { $t = microtime(true); $s = 0; for ($i = 0; $i < 3000000; $i++) { $s += $i * 2 % 7; } echo "arith loop 3M in fn: ", round((microtime(true)-$t)*1000), "ms\n"; return $s; }
loop();
$t = microtime(true); $s = 0; for ($i = 0; $i < 3000000; $i++) { $s += $i * 2 % 7; } echo "arith loop 3M global: ", round((microtime(true)-$t)*1000), "ms\n";
function calls() { $t = microtime(true); $s = 0; for ($i = 0; $i < 1000000; $i++) { $s = strlen("hello"); } echo "1M strlen in fn: ", round((microtime(true)-$t)*1000), "ms\n"; }
calls();
function add($a, $b) { return $a + $b; }
function ucalls() { $t = microtime(true); $s = 0; for ($i = 0; $i < 1000000; $i++) { $s = add($s, $i); } echo "1M user calls in fn: ", round((microtime(true)-$t)*1000), "ms\n"; }
ucalls();
