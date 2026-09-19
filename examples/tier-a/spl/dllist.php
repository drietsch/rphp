<?php
// SplDoublyLinkedList keeps its elements and its mode word in two private
// properties, which is why both show up in a dump.
$l = new SplDoublyLinkedList();
$l->push('a');
$l->push('b');
$l->unshift('z');
var_dump($l->count(), $l->top(), $l->bottom(), $l->isEmpty());
var_dump($l);
$l->add(1, 'q');
var_dump(iterator_to_array($l));
var_dump($l->offsetExists(3), $l->offsetGet(2), isset($l[1]), $l[0]);
$l[1] = 'Q';
unset($l[0]);
var_dump(iterator_to_array($l));
var_dump($l->pop(), $l->shift(), $l->count());

// Iteration walks front to back, and the mode word says so.
$l2 = new SplDoublyLinkedList();
$l2->push(1);
$l2->push(2);
$l2->push(3);
foreach ($l2 as $k => $v) {
    echo $k, '=', $v, "\n";
}
var_dump($l2->getIteratorMode());
$l2->setIteratorMode(SplDoublyLinkedList::IT_MODE_LIFO);
foreach ($l2 as $k => $v) {
    echo $k, '=', $v, "\n";
}
var_dump($l2->getIteratorMode(), SplDoublyLinkedList::IT_MODE_FIFO, SplDoublyLinkedList::IT_MODE_KEEP);

// IT_MODE_DELETE consumes the list as it goes.
$l3 = new SplDoublyLinkedList();
$l3->push('x');
$l3->push('y');
$l3->setIteratorMode(SplDoublyLinkedList::IT_MODE_FIFO | SplDoublyLinkedList::IT_MODE_DELETE);
foreach ($l3 as $v) {
    echo 'del ', $v, "\n";
}
var_dump($l3->count());

// A stack numbers its offsets from the top and freezes its direction.
$s = new SplStack();
$s->push('one');
$s->push('two');
var_dump($s, $s[0], $s->top());
foreach ($s as $k => $v) {
    echo 'stack ', $k, '=', $v, "\n";
}
try {
    $s->setIteratorMode(SplDoublyLinkedList::IT_MODE_FIFO);
} catch (RuntimeException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

$q = new SplQueue();
$q->enqueue('first');
$q->enqueue('second');
var_dump($q, $q->dequeue(), $q->count());

// Errors on an empty structure, and the out-of-range index.
$e = new SplDoublyLinkedList();
try {
    $e->pop();
} catch (RuntimeException $ex) {
    echo get_class($ex), ': ', $ex->getMessage(), "\n";
}
try {
    $e->top();
} catch (RuntimeException $ex) {
    echo get_class($ex), ': ', $ex->getMessage(), "\n";
}
try {
    $l2->offsetGet(99);
} catch (OutOfRangeException $ex) {
    echo get_class($ex), ': ', $ex->getMessage(), "\n";
}

// clone copies the elements; the original is untouched.
$c = clone $l2;
$c->push(4);
var_dump($l2->count(), $c->count());

// The serialization shapes.
var_dump(serialize($l2));
$back = unserialize(serialize($l2));
var_dump($back->count(), $back->top(), $back instanceof SplDoublyLinkedList);
var_dump($l2->__serialize());
