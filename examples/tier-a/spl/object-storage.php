<?php
// SplObjectStorage, WeakMap and ArrayObject indexed by object handle /
// mutated in place: attach, replace, detach, iteration, getInfo/setInfo,
// clone, serialize, addAll/removeAll, dead WeakMap keys, ArrayObject writes.
class O { function __construct(public $n) {} }
$s = new SplObjectStorage; $os = []; for ($i = 0; $i < 10; $i++) { $os[] = new O($i); }
foreach ($os as $i => $o) { $s[$o] = $i * 10; }
var_dump(count($s), $s[$os[3]], isset($s[$os[9]]), $s->contains($os[5]));
$s[$os[3]] = 'three'; var_dump($s[$os[3]], count($s));
unset($s[$os[0]]); $s->detach($os[5]); var_dump(count($s), isset($s[$os[0]]));
foreach ($s as $i => $o) { echo $i, ':', $o->n, '=', $s->getInfo(), ' '; } echo "\n";
$s->rewind(); $s->setInfo('first'); var_dump($s[$s->current()]);
$t = clone $s; $t[$os[0]] = 'back'; var_dump(count($s), count($t), $t[$os[0]]);
$u = unserialize(serialize($s)); var_dump(count($u)); foreach ($u as $o) { echo $o->n, ' '; } echo "\n";
$s->removeAll($t); var_dump(count($s)); $s->addAll($t); var_dump(count($s));
try { $s[new O(99)]; } catch (UnexpectedValueException $e) { echo $e->getMessage(), "\n"; }
$w = new WeakMap; foreach ($os as $i => $o) { $w[$o] = $i; } var_dump(count($w), $w[$os[4]]);
unset($os[4], $o); gc_collect_cycles(); var_dump(count($w)); $w[$os[1]] = 'one'; var_dump($w[$os[1]]); unset($w[$os[2]]); var_dump(isset($w[$os[2]]), count($w));
foreach ($w as $k => $v) { echo $k->n, '=', $v, ' '; } echo "\n";
try { $w[new O(1)]; } catch (Error $e) { echo preg_replace('/#\d+/', '#N', $e->getMessage()), "\n"; }
$ao = new ArrayObject([]); for ($i = 0; $i < 5; $i++) { $ao[] = $i; $ao["k$i"] = $i; } unset($ao[2]); var_dump(count($ao), $ao->getArrayCopy());
$b = $ao->getArrayCopy(); $ao[] = 'x'; var_dump(count($b), count($ao));
