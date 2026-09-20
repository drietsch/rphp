<?php
// A by-reference parameter binds to an element of a *nested* container —
// `$obj->list['k']`, `$a['x']['y']` — and a by-value one leaves it alone.
// Copying an array afterwards does not share the element: php drops a
// reference nobody else holds when it duplicates an array.
class D { public $l = ['e' => [3, 1, 2]]; }
function byval(array $a) { $a[] = 'x'; return count($a); }
function byref(array &$a) { $a[] = 'x'; return count($a); }
$d = new D;
var_dump(byval($d->l['e']), $d->l, byref($d->l['e']), $d->l);
$copy = $d->l; byref($d->l['e']); var_dump($copy['e'], $d->l['e']);
$m = [[1, 2], [3]]; var_dump(byval($m[0]), $m, byref($m[1]), $m);
$nested = ['a' => ['b' => ['c' => [1]]]];
byref($nested['a']['b']['c']); var_dump($nested);
var_dump(byref($d->l['auto']), $d->l);
$s = new stdClass; $s->arr = [];
byref($s->arr['k']); var_dump($s);
krsort($d->l['e']); sort($nested['a']['b']['c']); var_dump($d->l['e'], $nested['a']['b']['c']);
$g = [1]; byref($GLOBALS['g']); var_dump($g);
$arr2 = ['k' => 5]; var_dump(intdiv($arr2['k'], 2), max($arr2['k'], 1));
// A reference an array copy keeps: one somebody still holds.
$x = 1; $held = ['r' => &$x]; $dup = $held; $x = 2; var_dump($dup['r']);
// The finishing touch php gives an uncaught argument TypeError.
byref($d->l['e'][0]);
