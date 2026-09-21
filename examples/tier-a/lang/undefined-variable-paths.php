<?php
// php's "Undefined variable" warning on every conditional path: what a
// branch assigns is only conditionally assigned after it, what was
// assigned before a branch stays assigned inside and after it, and only
// unset() takes a value away. Each read below is judged by what really ran.
function ifelse($c) { if ($c) { $x = 1; } else { echo $x ?? 'u', "\n"; echo $x; } echo $x; echo "\n"; }
ifelse(false); ifelse(true);
function elseif_chain($c) { if ($c === 1) { $x = 1; } elseif ($x = $c) { echo $x; } else { echo $x; } echo "\n"; }
elseif_chain(1); elseif_chain(2); elseif_chain(0);
function loops($n) { $before = 'b'; for ($i = 0; $i < $n; $i++) { echo $before, $i, $later ?? '-'; $later = $i; } echo $i, $later, "\n"; while ($n-- > 0) { echo $w; $w = $n; } do { echo $d; $d = 1; } while (false); foreach ([1] as $v) { echo $before; } echo $v, "\n"; }
loops(2); loops(0);
function switches($c) { switch ($c) { case 1: $s = 'one'; case 2: echo $s; break; default: echo $s; } echo "\n"; }
switches(1); switches(2); switches(3);
function tries($t) { try { if ($t) { throw new Exception('e'); } $x = 1; } catch (Exception $e) { echo $x, get_class($e); } finally { echo $x; } echo $x, "\n"; }
tries(false); tries(true);
function shortcircuit($c) { $c && $a = 1; echo $a; $c || $b = 1; echo $b; $n = $c ? $t = 1 : $e = 1; echo $t, $e; $q = $c ?? $z = 1; echo $z; $m = match ($c) { true => $mt = 1, default => $md = 1 }; echo $mt, $md; echo "\n"; }
shortcircuit(true); shortcircuit(false);
function coalesce_assign($o) { $u ??= $k = 1; echo $k; $o?->m($p = 1); echo $p; echo "\n"; }
coalesce_assign(null);
function unsets() { $x = 1; unset($x); echo $x; if (true) { $y = 1; } unset($y); echo $y; $z = 1; for ($i = 0; $i < 2; $i++) { echo $z; unset($z); } echo "\n"; }
unsets();
function refs() { $r = &$u; echo $r; static $s; echo $s; global $g; echo $g; [$l, [$m]] = [1, [2]]; echo $l, $m; foreach ([[1, 2]] as [$a, $b]) { echo $a, $b; } echo "\n"; }
refs();
function params($p, ...$rest) { if ($p) { echo $p; } while ($p-- > 0) { echo $p, count($rest); } echo "\n"; }
params(2, 'x');
function nested_closure() { $outer = 1; $f = function () use ($outer) { if ($outer) { echo $outer; } echo $inner; $inner = 1; }; $f(); $g = fn() => $outer + $undefined; echo $g(), "\n"; }
nested_closure();
function generator() { $a = 1; while ($a < 3) { yield $a; echo $b; $b = $a++; } }
foreach (generator() as $v) echo $v; echo "\n";
function gotos() { $x = 1; goto L; $y = 2; L: echo $y; echo $x, "\n"; }
gotos();
function dynamic($name) { $x = 1; $$name = 2; unset($$name); echo $x; echo "\n"; }
dynamic('x');
function compound() { $a .= 'x'; $b += 1; $c[] = 1; $d['k'] = 1; $e++; echo $a, $b, count($c), count($d), $e, "\n"; }
compound();
function results() { $r = &$t; $t = 5; $r = $r + 1; echo $t; $s = 'a'; $q = &$s; $q = $q . 'b'; echo $s; $c = [1]; $d = &$c; $d = $d[0]; echo $c; $e = 1; $f = &$e; $f = -$f; echo $e; $g = 2; $h = &$g; $h = $h < 3; var_dump($g); $o = new stdClass; $o->p = 7; $v = &$o; $v = $v->p; echo $o, "\n"; }
results();
function statics() { static $n = 0; $n = $n + 1; echo $n; static $m = 'x'; $m = $m . 'y'; echo $m, "\n"; }
statics(); statics();
function byref(&$p) { $p = $p + 10; $p = $p * 2; } $z = 1; byref($z); echo $z, "\n";
$gl = 3; function globals() { global $gl; $gl = $gl + 1; $gl = "v$gl"; } globals(); echo $gl, "\n";
function closures() { $n = 1; $f = function () use (&$n) { $n = $n + 1; $n = $n . 'x'; }; $f(); echo $n, "\n"; }
closures();
