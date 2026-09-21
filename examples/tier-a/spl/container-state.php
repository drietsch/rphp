<?php
// The containers' state through clone, serialize() and the Serializable
// forms, with a subclass's properties (mangled in the payload, as php
// writes them) riding along; SplQueue as a real deque.
class Q extends SplQueue { protected $tag = 't'; } $q = new Q; $q->enqueue('x'); $q->enqueue('y');
echo serialize($q), "\n"; $u = unserialize(serialize($q)); var_dump($u); var_dump($u->dequeue(), count($u));
$c = clone $q; $c->dequeue(); var_dump(count($q), count($c), $c->getIteratorMode());
class H extends SplMinHeap { public $pub = 1; protected $prot = 2; private $priv = 3; } $h = new H; $h->insert(2); $h->insert(1);
echo serialize($h), "\n"; var_dump(unserialize(serialize($h))); $hc = clone $h; $hc->extract(); var_dump(count($h), count($hc), $h->top());
$pq = new SplPriorityQueue; $pq->setExtractFlags(SplPriorityQueue::EXTR_BOTH); $pq->insert('a', 1); echo serialize($pq), "\n"; $pu = unserialize(serialize($pq)); var_dump($pu->getExtractFlags(), $pu->extract());
$o = new stdClass;
class S extends SplObjectStorage { public $name = 'n'; } $s = new S; $s[$o] = 'i'; $s->attach(new stdClass, [1]);
echo serialize($s), "\n"; $su = unserialize(serialize($s)); var_dump(count($su), $su->name); $su->rewind(); var_dump($su->getInfo());
$sc = clone $s; $sc->detach($o); var_dump(count($s), count($sc));
foreach ([[new stdClass], []] as $bad) { try { (new SplObjectStorage)->__unserialize([$bad, []]); } catch (UnexpectedValueException $e) { echo $e->getMessage(), "\n"; } }
try { (new SplObjectStorage)->__unserialize([1]); } catch (UnexpectedValueException $e) { echo $e->getMessage(), "\n"; }
class AI extends ArrayIterator { private $secret = 's'; public $open = 'o'; } $ai = new AI([1, 2], ArrayIterator::ARRAY_AS_PROPS);
echo serialize($ai), "\n"; $au = unserialize(serialize($ai)); var_dump($au->getFlags(), $au->open, $au->getArrayCopy());
$ao = new ArrayObject(['k' => [1]], 0, 'RecursiveArrayIterator'); $ac = clone $ao; $ac['k'][] = 2; $ac['n'] = 1;
var_dump($ao->getArrayCopy(), $ac->getArrayCopy(), $ac->getIteratorClass(), $ac->getFlags());
$d = new SplDoublyLinkedList; $d->push('a'); $d->push([1]); $d->unshift('z'); echo $d->serialize(), "\n"; $d2 = new SplDoublyLinkedList; $d2->unserialize($d->serialize()); var_dump(iterator_to_array($d2));
var_dump(unserialize('O:20:"SplDoublyLinkedList":3:{i:0;i:2;i:1;a:2:{i:0;i:1;i:1;i:2;}i:2;a:1:{s:1:"x";i:5;}}'));
$big = new SplQueue; for ($i = 0; $i < 50000; $i++) $big->enqueue($i); $sum = 0; while (!$big->isEmpty()) $sum += $big->dequeue(); var_dump($sum);
$st = new SplStack; $st->push(1); $st->push(2); $st->push(3); foreach ($st as $k => $v) echo "$k=$v "; echo "\n"; var_dump($st[0], $st->pop(), $st->top());
$st->setIteratorMode(SplDoublyLinkedList::IT_MODE_LIFO | SplDoublyLinkedList::IT_MODE_DELETE); foreach ($st as $v) echo $v; echo "\n", count($st), "\n";
$l = new SplDoublyLinkedList; $l->push('a'); $l->push('b'); $l->unshift('z'); $l->rewind(); $l->next(); $l->prev(); $l->prev(); var_dump($l->key(), $l->valid(), $l->current());
$l->add(1, 'ins'); $l->offsetUnset(0); var_dump(iterator_to_array($l)); $l->setIteratorMode(SplDoublyLinkedList::IT_MODE_DELETE); foreach ($l as $v) echo $v; var_dump(count($l), $l->isEmpty());
