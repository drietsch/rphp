<?php
// References to ArrayAccess elements: an ArrayObject/ArrayIterator hands
// out its own storage element, a by-reference offsetGet() its cell, any
// other offsetGet() a copy with php's "Indirect modification" notice.
class C implements ArrayAccess { public $d = ["k" => 1]; function offsetExists($o): bool { return true; } function offsetGet($o): mixed { return $this->d[$o]; } function offsetSet($o, $v): void { $this->d[$o] = $v; } function offsetUnset($o): void {} }
$c = new C; $r = &$c["k"]; $r = 5; var_dump($c->d, $r);
class R implements ArrayAccess { public $d = ["k" => [1]]; function offsetExists($o): bool { return true; } function &offsetGet($o): mixed { return $this->d[$o]; } function offsetSet($o, $v): void { $this->d[$o] = $v; } function offsetUnset($o): void {} }
$x = new R; $r = &$x["k"]; $r[] = 2; var_dump($x->d); $x["k"][] = 3; var_dump($x->d);
$ao = new ArrayObject(["a" => [1]]); $q = &$ao["a"]; $q[] = 2; var_dump($ao["a"]); $ao["a"][] = 3; var_dump($ao["a"]); $q = 'replaced'; var_dump($ao["a"]);
$z = &$ao["new"]; $z = 'made'; var_dump($ao["new"], count($ao));
$it = new ArrayIterator([1, 2]); $e = &$it[0]; $e = 'x'; var_dump($it[0]);
function byref(&$v) { $v = 'byref'; } byref($ao["a"]); var_dump($ao["a"]);
