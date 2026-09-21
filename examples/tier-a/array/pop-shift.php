<?php
// array_pop/array_shift/end/reset/next/prev work on the array in place:
// the append index after a pop, renumbering after a shift, the internal
// pointer, references inside, tombstones from unset(), and an outside
// handle that must not see the change.
$a = [1, 2, 3]; var_dump(array_pop($a), $a); $a[] = 9; var_dump($a);
$b = [5 => 'x', 'k' => 'y', 7 => 'z']; var_dump(array_pop($b)); $b[] = 'n'; var_dump($b);
$c = ['a' => 1]; var_dump(array_pop($c), array_pop($c), $c); $c[] = 1; var_dump($c);
$d = [1, 2, 3]; unset($d[2]); var_dump(array_pop($d), $d); $d[] = 4; var_dump($d);
$e = [1, 2, 3]; var_dump(array_shift($e), $e); $e[] = 4; var_dump($e);
$f = [3 => 'a', 'k' => 'b', 9 => 'c']; var_dump(array_shift($f), $f); $f[] = 'd'; var_dump($f);
$g = []; var_dump(array_pop($g), array_shift($g));
$h = [1, 2, 3]; next($h); array_pop($h); var_dump(current($h)); end($h); array_shift($h); var_dump(current($h), key($h));
$r = [1, 2]; $ref = &$r[1]; var_dump(array_pop($r)); $ref = 5; var_dump($r);
$big = range(0, 9); unset($big[9], $big[8]); var_dump(array_pop($big), count($big)); $big[] = 'x'; var_dump(array_key_last($big));
$m = [1, 2, 3]; var_dump(end($m), key($m), prev($m), reset($m), next($m), next($m), next($m), current($m));
$n = ['x' => [1, 2]]; $p = $n['x']; array_pop($p); var_dump($n, $p);
$a = [1, 2]; var_dump(array_unshift($a, 'x', 'y'), $a); $a[] = 'z'; var_dump($a);
$b = [5 => 'a', 'k' => 'b', 9 => 'c']; var_dump(array_unshift($b, 0), $b); $b[] = 'e'; var_dump($b);
$c = []; var_dump(array_unshift($c), array_unshift($c, null), $c);
$d = [1, 2, 3]; end($d); array_unshift($d, 0); var_dump(current($d), key($d));
$r = [1]; $ref = &$r[0]; array_unshift($r, 9); $ref = 7; var_dump($r);
$n = ['x' => [1]]; $p = $n['x']; array_unshift($p, 0); var_dump($n, $p);
