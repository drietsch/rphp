<?php
// By-reference natives mutate in place; the array stays visible to other
// handles, callbacks, and traces; a cell shared between two positions is
// read by both.
$a = [3, 1, 2]; sort($a); var_dump($a);
$b = $a; array_push($b, 4); var_dump($a, $b);
$c = [1, 2]; array_push($c, $c); var_dump($c);
$d = [3, 1]; $e = &$d; sort($e); var_dump($d);
$arr = [5, 4, 3];
usort($arr, function ($x, $y) use (&$arr) { return $x <=> $y; });
var_dump($arr);
$z = [2, 1]; usort($z, function ($x, $y) { global $z; var_dump($z); return $x <=> $y; }); var_dump($z);
function f(array &$r) { array_shift($r); return count($r); }
$g = [1, 2, 3]; var_dump(f($g), $g);
try { $q = [1]; array_walk($q, function (&$v) { throw new Exception('x'); }); } catch (Exception $e) { var_dump($q, $e->getTrace()[0]['function'], $e->getTrace()[1]['args'][0] ?? 'none'); }
