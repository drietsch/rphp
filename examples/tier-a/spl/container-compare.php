<?php
// == and <=> on the SPL containers: ArrayObject/ArrayIterator compare their
// storage before their properties, SplObjectStorage its entries (identity
// and info; the verdict is final), the rest their properties alone.
$o = new stdClass; $p = new stdClass;
$a = new SplObjectStorage; $a[$o] = 1; $b = new SplObjectStorage; $b[$o] = 1;
var_dump($a == $b); $b[$o] = 2; var_dump($a == $b, $a < $b); $c = new SplObjectStorage; $c[$p] = 1; var_dump($a == $c, $a < $c); $c[$o] = 0; var_dump($a < $c, $a <=> $c);
$x = new ArrayObject([1]); $y = new ArrayObject([2]);
var_dump($x == $y, $x < $y, $x <=> $y, new ArrayObject([1]) == new ArrayObject([1]), new ArrayObject(['a' => 1]) == new ArrayIterator(['a' => 1]));
class AO extends ArrayObject { public $z; } $m = new AO([1]); $n = new AO([1]); $n->z = 1; var_dump($m == $n, $m < $n, $m <=> $n);
$f = new SplFixedArray(1); $g = new SplFixedArray(2); var_dump($f == $g);
$h = new SplMinHeap; $h->insert(1); var_dump($h == new SplMinHeap);
$q = new SplQueue; $q->push(1); var_dump($q == new SplQueue, in_array(new SplQueue, [$q]));
$w = new WeakMap; $w[$o] = 1; var_dump($w == new WeakMap);
var_dump(in_array(new ArrayObject([2]), [$x, $y]), array_search(new ArrayObject([2]), [$x, $y]));
