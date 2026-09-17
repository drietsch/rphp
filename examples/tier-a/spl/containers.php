<?php
// Tier-A differential: the builtin container classes — ArrayObject and
// ArrayIterator (spl_array.c), WeakReference and WeakMap (zend_weakrefs.c)
// and the shape of the Closure class (zend_closures.c).

// ---- ArrayObject -------------------------------------------------------
// php keeps the backing array in a *private* `storage` property, so this is
// what var_dump/print_r show. Dumped first so the object handle is #1 on
// both sides.
var_dump(new ArrayObject([1, 2]));
print_r(new ArrayObject([1, 2]));
echo "\n";

$ao = new ArrayObject([3, 1, 2]);
echo count($ao), ' ', $ao->count(), "\n";
echo $ao[0], $ao[1], $ao[2], "\n";
var_dump($ao->offsetExists(1), $ao->offsetExists(9));
$ao[] = 9;
$ao['k'] = 'v';
$ao->append('t');
print_r($ao->getArrayCopy());
unset($ao['k']);
var_dump($ao->offsetExists('k'));
echo count($ao), "\n";
var_dump($ao->getFlags());
$ao->setFlags(2);
var_dump($ao->getFlags());
echo $ao->getIteratorClass(), "\n";

$sorted = new ArrayObject([3, 1, 2]);
$sorted->asort();
print_r($sorted->getArrayCopy());
$sorted->ksort();
print_r($sorted->getArrayCopy());
$sorted->uasort(fn ($a, $b) => $b <=> $a);
print_r($sorted->getArrayCopy());

$old = $sorted->exchangeArray(['x' => 1]);
print_r($old);
print_r($sorted->getArrayCopy());

// ---- ArrayIterator -----------------------------------------------------
$ai = new ArrayIterator(['a' => 1, 'b' => 2, 'c' => 3]);
echo count($ai), "\n";
$ai->rewind();
while ($ai->valid()) {
    echo $ai->key(), '=', $ai->current(), ' ';
    $ai->next();
}
echo "\n";
var_dump($ai->valid(), $ai->key(), $ai->current());
$ai->seek(1);
echo $ai->key(), '=', $ai->current(), "\n";
try {
    $ai->seek(99);
} catch (OutOfBoundsException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
foreach (new ArrayIterator([10, 20]) as $k => $v) {
    echo $k, ':', $v, ' ';
}
echo "\n";

// ArrayObject hands out an ArrayIterator.
$it = (new ArrayObject(['p' => 1]))->getIterator();
echo get_class($it), ' ', (int) ($it instanceof Iterator), (int) ($it instanceof Traversable), "\n";

// The interfaces both containers carry.
foreach (['ArrayObject', 'ArrayIterator'] as $c) {
    $impl = array_values(class_implements($c));
    sort($impl);
    echo $c, ': ', implode(',', $impl), "\n";
}

// ---- SplObjectStorage --------------------------------------------------
$sos = new SplObjectStorage;
$o1 = new stdClass;
$o2 = new stdClass;
$sos[$o1] = 'one';
$sos->offsetSet($o2, 'two');
$sos[$o1] = 'uno';
echo $sos->count(), ' ', count($sos), "\n";
echo $sos[$o1], ' ', $sos[$o2], "\n";
var_dump($sos->offsetExists($o1), isset($sos[$o2]));
$sos->rewind();
while ($sos->valid()) {
    echo $sos->key(), ':', $sos->getInfo(), ' ';
    $sos->next();
}
echo "\n";
foreach ($sos as $i => $obj) {
    echo $i, get_class($obj), ' ';
}
echo "\n";
$other = new SplObjectStorage;
$other[$o2] = 'dup';
echo $sos->removeAll($other), ' ', $sos->count(), "\n";
$sos->offsetUnset($o1);
echo $sos->count(), "\n";
try {
    $sos[1] = 'x';
} catch (TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    $sos->offsetGet(new stdClass);
} catch (UnexpectedValueException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
// The 8.5 aliases still work, with a deprecation.
$sos->attach($o1, 'back');
echo $sos->count(), "\n";

// ---- WeakReference -----------------------------------------------------
$target = new stdClass;
$target->v = 1;
$ref = WeakReference::create($target);
var_dump($ref instanceof WeakReference);
var_dump($ref->get() === $target);
unset($target);
var_dump($ref->get());
try {
    new WeakReference();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// ---- WeakMap -----------------------------------------------------------
$map = new WeakMap;
var_dump($map instanceof ArrayAccess, $map instanceof Countable, $map instanceof IteratorAggregate);
echo count($map), "\n";
$k1 = new stdClass;
$k2 = new stdClass;
$map[$k1] = 'one';
$map[$k2] = 'two';
$map[$k1] = 'uno';
echo count($map), ' ', $map[$k1], ' ', $map[$k2], "\n";
var_dump(isset($map[$k1]));
unset($map[$k2]);
echo count($map), "\n";
try {
    $map[1] = 'x';
} catch (TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
unset($k1);
echo count($map), "\n";

// ---- Closure -----------------------------------------------------------
$fn = function () { return 1; };
var_dump($fn instanceof Closure);
echo get_class($fn), "\n";
var_dump(method_exists('Closure', 'bindTo'), method_exists('Closure', 'fromCallable'));
try {
    new Closure();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// The two static methods reach the class table (the instance ones are
// dispatched by the engine, because a closure is not an object).
$up = Closure::fromCallable('strtoupper');
var_dump($up instanceof Closure);
echo $up('hi'), "\n";
$plain = function () { return 'p'; };
$rebound = Closure::bind($plain, null);
var_dump($rebound instanceof Closure);
echo $rebound(), "\n";
try {
    Closure::bind('nope', null);
} catch (TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    Closure::fromCallable('no_such_function');
} catch (TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
