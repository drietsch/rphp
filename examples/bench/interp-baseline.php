<?php
$t = microtime(true); $s = 0; for ($i = 0; $i < 3000000; $i++) { $s += $i * 2 % 7; } echo "arith loop 3M: ", round((microtime(true)-$t)*1000), "ms\n";
function add($a, $b) { return $a + $b; }
$t = microtime(true); $s = 0; for ($i = 0; $i < 1000000; $i++) { $s = add($s, $i); } echo "1M calls: ", round((microtime(true)-$t)*1000), "ms\n";
class P { private int $n = 0; function inc(): void { $this->n++; } function get(): int { return $this->n; } }
$t = microtime(true); $p = new P; for ($i = 0; $i < 1000000; $i++) { $p->inc(); } echo "1M method calls: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $s = ''; for ($i = 0; $i < 1000000; $i++) { $s = "x" . $i . "y"; } echo "1M concat: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $a = ['k' => 1]; for ($i = 0; $i < 1000000; $i++) { $x = $a['k'] + 1; $a['k'] = $x; } echo "1M assoc rw: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 300000; $i++) { $o = new P; } echo "300k new: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $f = fn($x) => $x + 1; for ($i = 0; $i < 1000000; $i++) { $s = $f($i); } echo "1M closure calls: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 1000000; $i++) { $s = strlen("hello") + abs(-3); } echo "2M native calls: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 300000; $i++) { $s = sprintf("%s-%d", "a", $i); } echo "300k sprintf: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); for ($i = 0; $i < 300000; $i++) { $s = str_replace("a", "b", "banana"); } echo "300k str_replace: ", round((microtime(true)-$t)*1000), "ms\n";
