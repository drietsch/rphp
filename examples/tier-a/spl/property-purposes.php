<?php
// php's property-table purposes on the SPL containers: the state lives out
// of the property table, so (array), var_export(), json_encode(),
// get_object_vars() and Reflection see none of it, dumps see it through
// __debugInfo() (the standard properties of a subclass first), and
// ArrayObject/SplFixedArray hand (array)/var_export/json their contents.
$o = new stdClass;
$q = new SplQueue; $q->enqueue(1); $q->enqueue("a");
var_dump($q); print_r($q); var_export($q); echo "\n", json_encode($q), "\n";
var_dump((array)$q, get_object_vars($q), count((new ReflectionObject($q))->getProperties()));
class Q extends SplQueue { public $x = 1; protected $p = 2; private $s = 3; }
$sub = new Q; $sub->push(2); var_dump($sub); var_dump((array)$sub); var_export($sub); echo "\n", json_encode($sub), "\n";
$h = new SplMinHeap; $h->insert(3); $h->insert(1); var_dump($h, (array)$h); var_export($h); echo "\n";
class H extends SplMaxHeap { public $pub = 'p'; } $hh = new H; $hh->insert(1); print_r($hh);
$pq = new SplPriorityQueue; $pq->insert("x", 2); var_dump($pq); echo json_encode($pq), "\n";
$s = new SplObjectStorage; $s[$o] = "inf"; var_dump($s, (array)$s, get_object_vars($s)); var_export($s); echo "\n", json_encode($s), "\n";
class S extends SplObjectStorage { public $name = 'n'; } $ss = new S; $ss[$o] = 'i'; print_r($ss);
$a = new ArrayObject([1, 2]); $a->dyn = 3;
var_dump($a, (array)$a, get_object_vars($a)); var_export($a); echo "\n", json_encode($a), "\n";
$std = new ArrayObject(['k' => 'v'], ArrayObject::STD_PROP_LIST); $std->d = 1; var_dump((array)$std); echo json_encode($std), "\n";
class AO extends ArrayObject { public $z = 'z'; } $ao = new AO(['a' => 1]); var_dump($ao); var_dump((array)$ao);
$i = new ArrayIterator(['k' => 1]); var_dump($i, (array)$i); var_export($i); echo "\n", json_encode($i), "\n";
class AI extends ArrayIterator { private $secret = 's'; } print_r(new AI([1]));
$f = new SplFixedArray(2); $f[0] = 7; var_dump($f, (array)$f, get_object_vars($f)); var_export($f); echo "\n", json_encode($f), "\n";
class F extends SplFixedArray { public $x = 1; } $fx = new F(1); $fx[0] = 'a'; var_dump($fx, (array)$fx); var_export($fx); echo "\n";
$m = new MultipleIterator; $m->attachIterator(new ArrayIterator([1])); var_dump($m, (array)$m);
$w = new WeakMap; $w[$o] = 1; var_dump($w, (array)$w); print_r($w); echo json_encode($w), "\n";
$r = WeakReference::create($o); var_dump($r, (array)$r); print_r($r);
foreach ([new ArrayObject, new ArrayIterator, new SplObjectStorage, new SplDoublyLinkedList, new SplMinHeap, new SplPriorityQueue, new SplFixedArray, new WeakMap, new MultipleIterator] as $c) echo get_class($c), ': ', var_export(method_exists($c, '__debugInfo'), true), "\n";
var_dump(ArrayObject::STD_PROP_LIST, ArrayObject::ARRAY_AS_PROPS, ArrayIterator::ARRAY_AS_PROPS);
