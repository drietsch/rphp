<?php
// The SPL heaps (spl_heap.c). SplHeap keeps its mode word, its corruption
// latch and its elements in three private properties, so all three show up in
// a dump — and the element *array* is printed, which makes php's exact
// sift-up/sift-down order observable. Dumped first so the object handle is #1
// on both sides; every later object goes through print_r, which prints none.
$h = new SplMinHeap();
$h->insert(5);
$h->insert(1);
$h->insert(3);
var_dump($h);

// Equal elements come out in php's order, not in insertion order: the sift
// moves a parent down only while it compares *less* than the newcomer.
$d = new SplMinHeap();
foreach (['b1', 'a1', 'b2', 'a2', 'c1'] as $v) {
    $d->insert($v);
}
print_r($d);
$out = [];
while (!$d->isEmpty()) {
    $out[] = $d->extract();
}
print_r($out);

// SplMaxHeap differs from SplMinHeap by compare() alone. Iterating a heap
// consumes it.
$m = new SplMaxHeap();
foreach ([5, 3, 8, 1, 9, 2, 7] as $v) {
    $m->insert($v);
}
print_r($m);
echo $m->top(), ' ', $m->extract(), ' ', $m->count(), "\n";
print_r($m);
foreach ($m as $k => $v) {
    echo $k, '=>', $v, ' ';
}
echo "\n";
echo $m->count(), "\n";

// The iterator protocol: the key counts down, rewind() does nothing, next()
// throws the top away.
$i = new SplMinHeap();
foreach ([3, 1, 2] as $v) {
    $i->insert($v);
}
$i->rewind();
var_dump($i->valid(), $i->key(), $i->current());
$i->next();
var_dump($i->count(), $i->key(), $i->current());
$i->rewind();
var_dump($i->key(), $i->current());
$i->next();
$i->next();
var_dump($i->valid(), $i->key(), $i->current());
$i->next();
echo $i->count(), "\n";

// clone copies the elements.
$c = new SplMinHeap();
$c->insert(2);
$c->insert(1);
$c2 = clone $c;
$c2->insert(0);
var_dump($c->count(), $c2->count(), $c->top(), $c2->top());

// A user subclass overrides compare(); insertion and extraction call back
// into it through ordinary method dispatch.
class ByLength extends SplHeap
{
    protected function compare($value1, $value2): int
    {
        return strlen($value1) <=> strlen($value2);
    }
}

$b = new ByLength();
foreach (['ccc', 'a', 'bbbb', 'dd', 'ee'] as $v) {
    $b->insert($v);
}
print_r($b);
while (!$b->isEmpty()) {
    echo $b->extract(), ' ';
}
echo "\n";

// Throwing from compare() corrupts the heap: the element really is stored,
// count() really does grow, and insert/extract/top refuse until the latch is
// cleared. The state lives in a static so the instance keeps php's property
// shape.
class Fussy extends SplHeap
{
    public static bool $angry = false;

    protected function compare($value1, $value2): int
    {
        if (self::$angry) {
            throw new LogicException('no comparing today');
        }
        return $value1 <=> $value2;
    }
}

$fz = new Fussy();
foreach ([4, 7, 1] as $v) {
    $fz->insert($v);
}
print_r($fz);
Fussy::$angry = true;
try {
    $fz->insert(9);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
var_dump($fz->isCorrupted(), $fz->count());
print_r($fz);
try {
    $fz->insert(1);
} catch (Throwable $t) {
    echo 'insert: ', $t->getMessage(), "\n";
}
try {
    $fz->extract();
} catch (Throwable $t) {
    echo 'extract: ', $t->getMessage(), "\n";
}
try {
    $fz->top();
} catch (Throwable $t) {
    echo 'top: ', $t->getMessage(), "\n";
}
// count/isEmpty/current/key never check the latch.
var_dump($fz->count(), $fz->isEmpty(), $fz->current(), $fz->key());
Fussy::$angry = false;
var_dump($fz->recoverFromCorruption(), $fz->isCorrupted());
echo $fz->extract(), "\n";
print_r($fz);

// The empty-heap texts, and the abstract base.
$empty = new SplMinHeap();
try {
    $empty->top();
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $empty->extract();
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
var_dump($empty->key(), $empty->current(), $empty->valid(), $empty->isEmpty());
try {
    new SplHeap();
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}

// SplPriorityQueue is not a subclass of SplHeap: it declares its own three
// private slots, starts at EXTR_DATA, and stores data/priority pairs.
$q = new SplPriorityQueue();
print_r($q);
foreach ([['a', 1], ['b', 3], ['c', 2], ['d', 3]] as [$v, $p]) {
    $q->insert($v, $p);
}
print_r($q);
echo $q->getExtractFlags(), "\n";

$data = clone $q;
while (!$data->isEmpty()) {
    echo $data->extract(), ' ';
}
echo "\n";

$prio = clone $q;
echo $prio->setExtractFlags(SplPriorityQueue::EXTR_PRIORITY), "\n";
while (!$prio->isEmpty()) {
    echo $prio->extract(), ' ';
}
echo "\n";

$both = clone $q;
echo $both->setExtractFlags(SplPriorityQueue::EXTR_BOTH), "\n";
print_r($both->top());
foreach ($both as $k => $v) {
    echo $k, ':', $v['data'], '/', $v['priority'], ' ';
}
echo "\n";

// php masks the flag word down to the two meaningful bits and refuses an
// empty selection.
$flags = new SplPriorityQueue();
echo $flags->setExtractFlags(7), ' ', $flags->getExtractFlags(), "\n";
try {
    $flags->setExtractFlags(4);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $flags->setExtractFlags([]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    (new SplPriorityQueue())->extract();
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}

// A queue subclass sees the priorities in compare().
class LowFirst extends SplPriorityQueue
{
    public function compare($priority1, $priority2): int
    {
        return $priority2 <=> $priority1;
    }
}

$lf = new LowFirst();
foreach ([['a', 3], ['b', 1], ['c', 2]] as [$v, $p]) {
    $lf->insert($v, $p);
}
while (!$lf->isEmpty()) {
    echo $lf->extract(), ' ';
}
echo "\n";

// Serialization: a property bag plus a flags/heap_elements record. The
// elements are re-inserted on the way back, not copied.
$s = new SplMinHeap();
$s->insert(3);
$s->insert(1);
print_r($s->__serialize());
echo serialize($s), "\n";
print_r(unserialize(serialize($s)));

$r = new SplMinHeap();
$r->__unserialize([[], ['flags' => 0, 'heap_elements' => [9, 4, 6]]]);
print_r($r);
try {
    (new SplMinHeap())->__unserialize([[]]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    (new SplMinHeap())->__unserialize([[], ['flags' => 0]]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}

$pq = new SplPriorityQueue();
$pq->insert('z', 7);
print_r($pq->__serialize());
echo serialize($pq), "\n";
print_r(unserialize(serialize($pq)));
