<?php
// The interpreter's per-op cost on the tightest loops: a counted loop
// with an addition, in a function and at the top level (where variables
// are symbol-table cells), and the native call boundary (strlen is a
// dedicated opcode in php; abs() goes through its call path).
function loop() { $t = microtime(true); for ($i = 0; $i < 20000000; $i++) { $x = $i + 1; } printf("loop 20M in function %.1fms\n", (microtime(true)-$t)*1000); }
loop();
$t = microtime(true); for ($i = 0; $i < 20000000; $i++) { $x = $i + 1; } printf("loop 20M global %.1fms\n", (microtime(true)-$t)*1000);
function natives() {
    $t = microtime(true); $s = 0; for ($i = 0; $i < 2000000; $i++) { $s += strlen("abc"); } printf("strlen 2M %.1fms\n", (microtime(true)-$t)*1000);
    $t = microtime(true); $s = 0; for ($i = 0; $i < 2000000; $i++) { $s += abs(-3); } printf("abs 2M %.1fms\n", (microtime(true)-$t)*1000);
    $t = microtime(true); $a = [1,2,3]; for ($i = 0; $i < 2000000; $i++) { $s += count($a); } printf("count 2M %.1fms\n", (microtime(true)-$t)*1000);
    $t = microtime(true); $o = new ArrayObject([1]); for ($i = 0; $i < 2000000; $i++) { $x = $o->count(); } printf("ArrayObject::count 2M %.1fms\n", (microtime(true)-$t)*1000);
}
natives();
function f($a) { return $a; }
function calls() { $t = microtime(true); for ($i = 0; $i < 2000000; $i++) { $x = f($i); } printf("user call 2M %.1fms\n", (microtime(true)-$t)*1000); }
calls();
