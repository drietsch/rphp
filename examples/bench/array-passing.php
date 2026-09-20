<?php
function f1($a) { return 1; }
function f2($a) { return $a; }
function f3($a) { $b = $a; return 1; }
function f4($a) { return count($a); }
function f5(array $a) { return 1; }
$g = range(1, 20000);
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { f1($g); } echo "f1(g) ignore: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { f2($g); } echo "f2(g) return a, discard: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { $x = f2($g); } echo "x = f2(g): ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { $g = f2($g); } echo "g = f2(g): ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { f3($g); } echo "f3 local copy: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { f4($g); } echo "f4 count: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { f5($g); } echo "f5 typed: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { $y = $g; } echo "y = g: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { $y = $g; $y[0] = 1; } echo "y = g; y[0]=1 (legit copy): ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 20000; $i++) { $z = array_values($g); } echo "array_values(g): ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 2000; $i++) { $z = array_map(fn($v) => $v, $g); } echo "array_map x2000: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 2000; $i++) { $z = array_keys($g); } echo "array_keys x2000: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 2000; $i++) { $z = implode(',', $g); } echo "implode x2000: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 2000; $i++) { $z = $g === $g; } echo "=== x2000: ", round((microtime(true)-$t)*1000), "ms\n";
