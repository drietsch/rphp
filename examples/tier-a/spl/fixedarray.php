<?php
// SplFixedArray (spl_fixedarray.c): the one SPL container with no visible
// property at all. php synthesizes the numeric list a dump shows from the
// object's debug handler, which rphp's formatters do not consult yet, so
// everything here reads the elements through toArray() instead of dumping
// the instance.

$f = new SplFixedArray(3);
echo count($f), ' ', $f->getSize(), ' ', $f->count(), "\n";
print_r($f->toArray());

$f[0] = 'a';
$f[2] = 'c';
print_r($f->toArray());

// offsetExists is "in range AND not null", which is what makes isset() false
// for a slot nobody has filled.
var_dump($f->offsetExists(0), $f->offsetExists(1), $f->offsetExists(3));
var_dump(isset($f[0]), isset($f[1]), isset($f[9]));
echo $f[0], $f['2'], "\n";
var_dump($f[1]);

// unset() empties a slot; the size never changes.
unset($f[0]);
print_r($f->toArray());
echo count($f), "\n";

// The class is an IteratorAggregate in php 8, and hands out the engine's
// InternalIterator.
$f[0] = 'a';
echo get_class($f->getIterator()), "\n";
foreach ($f as $k => $v) {
    echo $k, '=', var_export($v, true), ' ';
}
echo "\n";
echo json_encode($f), "\n";
print_r($f->jsonSerialize());

// setSize grows with nulls and truncates, and answers true.
var_dump($f->setSize(5));
print_r($f->toArray());
$f->setSize(2);
print_r($f->toArray());

// fromArray keeps the keys by default (the size is the largest plus one) and
// packs the values when asked. It builds an SplFixedArray either way.
print_r(SplFixedArray::fromArray([1, 2, 3])->toArray());
print_r(SplFixedArray::fromArray([5 => 'a', 2 => 'b'])->toArray());
print_r(SplFixedArray::fromArray([5 => 'a', 2 => 'b'], false)->toArray());
print_r(SplFixedArray::fromArray([])->toArray());
echo get_class(SplFixedArray::fromArray([1])), "\n";

// clone copies the elements rather than aliasing them.
$src = SplFixedArray::fromArray(['x', 'y']);
$copy = clone $src;
$copy[0] = 'X';
print_r($src->toArray());
print_r($copy->toArray());

// A fractional offset is truncated, with php's precision deprecation.
$d = SplFixedArray::fromArray(['a', 'b']);
echo $d[1.5], "\n";

// The error paths.
$e = SplFixedArray::fromArray(['a', 'b']);
try {
    $v = $e[5];
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e[-1] = 1;
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->offsetUnset(7);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->offsetGet(null);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->offsetGet('nope');
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->offsetGet('01');
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->offsetSet(null, 1);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->offsetGet([]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    new SplFixedArray(-1);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    new SplFixedArray([]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    $e->setSize(-1);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    SplFixedArray::fromArray(['k' => 1]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    SplFixedArray::fromArray([-1 => 1]);
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}
try {
    SplFixedArray::fromArray('nope');
} catch (Throwable $t) {
    echo get_class($t), ': ', $t->getMessage(), "\n";
}

// A second __construct on a non-empty array is ignored; on an empty one it
// still sizes the array.
$twice = new SplFixedArray(2);
$twice->__construct(5);
echo count($twice), "\n";
$zero = new SplFixedArray(0);
$zero->__construct(4);
echo count($zero), "\n";

// The serialization pair: __serialize() is the bare element list, which is
// what serialize() then writes as the object's payload.
$s = SplFixedArray::fromArray(['p', 'q']);
print_r($s->__serialize());
echo serialize($s), "\n";
$back = unserialize(serialize($s));
print_r($back->toArray());
echo count($back), "\n";
$u = new SplFixedArray(0);
$u->__unserialize(['m', 'n', 'o']);
print_r($u->toArray());

// __wakeup survives only for the pre-8.3 payload format and says so.
$w = new SplFixedArray(1);
$w->__wakeup();
echo count($w), "\n";
