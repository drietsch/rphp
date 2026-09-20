<?php
// php 8.4 lazy objects: ghosts and proxies from ReflectionClass, what
// triggers their initializer and what does not, how they dump, and the
// per-property escape hatches on ReflectionProperty.

class Svc {
    public int $a = 1;
    public string $b;
    public $untyped = 'u';
    protected int|string $mixed = 2;
    private array $c = [];
    public function __construct(int $a = 7) { echo "construct($a)\n"; $this->a = $a; $this->b = 'set'; $this->c = [1]; }
    public function get(): int { return $this->a; }
    public function noProps(): string { return 'ok'; }
    public function id(): int { return spl_object_id($this); }
}

$r = new ReflectionClass(Svc::class);

echo "--- ghost ---\n";
$g = $r->newLazyGhost(function (Svc $o) { echo "init ghost\n"; $o->__construct(5); });
var_dump($r->isUninitializedLazyObject($g), get_class($g), $g instanceof Svc);
var_dump($g);
print_r($g); echo "\n";
var_dump($g->noProps(), $r->isUninitializedLazyObject($g));   // a method call does not initialize
var_dump($g->a, $r->isUninitializedLazyObject($g));           // a property read does
var_dump($g);
var_dump($g->get(), $g->b);

echo "--- proxy ---\n";
$p = $r->newLazyProxy(function (Svc $o) { echo "init proxy\n"; return new Svc(9); });
var_dump($r->isUninitializedLazyObject($p), get_class($p));
var_dump($p);
var_dump($p->noProps(), $r->isUninitializedLazyObject($p));
var_dump($p->a, $r->isUninitializedLazyObject($p));
var_dump($p);
print_r($p); echo "\n";
$real = $r->initializeLazyObject($p);
var_dump($real !== $p, $real->a, get_class($real));
$p->a = 42; var_dump($real->a, $p->get(), $p->id() === spl_object_id($p));
var_dump((array) $p, get_object_vars($p), json_encode($p), serialize($p));
var_export($p); echo "\n";

echo "--- initializer bookkeeping ---\n";
$g2 = $r->newLazyGhost(fn(Svc $o) => $o->__construct(3));
var_dump($r->getLazyInitializer($g2) instanceof Closure, $r->getLazyInitializer(new Svc) === null);
var_dump($r->initializeLazyObject($g2) === $g2, $r->isUninitializedLazyObject($g2), $g2->a, $r->getLazyInitializer($g2));
$g3 = $r->newLazyGhost(fn(Svc $o) => $o->__construct(3));
var_dump($r->markLazyObjectAsInitialized($g3) === $g3, $r->isUninitializedLazyObject($g3), $g3->a);
var_dump($g3);

echo "--- skipping properties ---\n";
$g4 = $r->newLazyGhost(fn(Svc $o) => $o->__construct(3));
$pa = new ReflectionProperty(Svc::class, 'a');
$pb = new ReflectionProperty(Svc::class, 'b');
$pc = new ReflectionProperty(Svc::class, 'c');
var_dump($pa->isLazy($g4), $pc->isLazy($g4), $pa->isLazy(new Svc));
$pa->skipLazyInitialization($g4);
var_dump($g4->a, $r->isUninitializedLazyObject($g4), $pa->isLazy($g4));
$pb->setRawValueWithoutLazyInitialization($g4, 'raw');
var_dump($g4->b, $r->isUninitializedLazyObject($g4), isset($g4->a), isset($g4->b));
var_dump($g4);
var_dump($g4->c ?? 'private-from-outside', $r->isUninitializedLazyObject($g4));
$g5 = $r->newLazyGhost(fn(Svc $o) => $o->__construct(3));
foreach (['a', 'b', 'untyped', 'mixed', 'c'] as $n) {
    (new ReflectionProperty(Svc::class, $n))->skipLazyInitialization($g5);
}
var_dump($r->isUninitializedLazyObject($g5), $r->getLazyInitializer($g5));   // every slot skipped: initialized
var_dump($g5);

echo "--- what initializes ---\n";
$fresh = fn() => $r->newLazyGhost(fn(Svc $o) => $o->__construct(3));
$x = $fresh(); foreach ($x as $k => $v) { echo "iter $k\n"; break; }
$x = $fresh(); var_dump(count((array) $x), $r->isUninitializedLazyObject($x));
$x = $fresh(); var_dump(json_encode($x), $r->isUninitializedLazyObject($x));
$x = $fresh(); var_dump(serialize($x), $r->isUninitializedLazyObject($x));
$x = $fresh(); var_dump(get_object_vars($x), $r->isUninitializedLazyObject($x));
$x = $fresh(); var_export($x); echo "\n"; var_dump($r->isUninitializedLazyObject($x));
$x = $fresh(); $c = clone $x; var_dump($r->isUninitializedLazyObject($x), $r->isUninitializedLazyObject($c));
$x = $fresh(); var_dump($x == new Svc(3), $r->isUninitializedLazyObject($x));
$x = $fresh(); var_dump($x === $x, $x != $x, $r->isUninitializedLazyObject($x));
$x = $fresh(); unset($x->a); var_dump($r->isUninitializedLazyObject($x));
$x = $fresh(); var_dump(empty($x->a), $r->isUninitializedLazyObject($x));
$x = $fresh(); $x->dyn = 1; var_dump($r->isUninitializedLazyObject($x));
$x = $fresh(); var_dump(property_exists($x, 'a'), method_exists($x, 'get'), $x instanceof Svc, strlen(spl_object_hash($x)), $r->isUninitializedLazyObject($x));
$x = $r->newLazyGhost(fn(Svc $o) => $o->__construct(3), ReflectionClass::SKIP_INITIALIZATION_ON_SERIALIZE);
var_dump(serialize($x), $r->isUninitializedLazyObject($x));

echo "--- reset ---\n";
$plain = new Svc; $plain->a = 11;
$r->resetAsLazyGhost($plain, fn(Svc $o) => $o->__construct(8));
var_dump($r->isUninitializedLazyObject($plain));
var_dump($plain);
var_dump($plain->a, $plain->untyped);
$plain2 = new Svc(2);
$r->resetAsLazyProxy($plain2, fn(Svc $o) => new Svc(4));
var_dump($r->isUninitializedLazyObject($plain2), $plain2->a);

echo "--- destructors ---\n";
class D {
    public int $a = 1;
    public function __construct() { echo "ctor\n"; }
    public function __destruct() { echo "dtor\n"; }
}
$rd = new ReflectionClass(D::class);
$d = $rd->newLazyGhost(fn(D $o) => $o->__construct()); unset($d); echo "released uninitialized ghost\n";
$d = $rd->newLazyGhost(fn(D $o) => $o->__construct()); $d->a; unset($d); echo "released initialized ghost\n";
$d = $rd->newLazyProxy(fn(D $o) => new D); $d->a; unset($d); echo "released initialized proxy\n";
$d = new D; $rd->resetAsLazyGhost($d, fn(D $o) => $o->__construct()); echo "reset ran the destructor\n"; unset($d); echo "released\n";
$d = new D; $rd->resetAsLazyGhost($d, fn(D $o) => $o->__construct(), ReflectionClass::SKIP_DESTRUCTOR); echo "reset skipped the destructor\n"; unset($d); echo "released\n";

echo "--- errors ---\n";
$tries = [
    fn() => $r->newLazyGhost(fn(Svc $o) => 5)->a,
    fn() => $r->newLazyProxy(fn(Svc $o) => new stdClass)->a,
    fn() => $r->newLazyProxy(fn(Svc $o) => 'str')->a,
    fn() => $r->newLazyGhost(fn(Svc $o) => throw new RuntimeException('boom'))->a,
    fn() => $r->newLazyGhost('nope'),
    fn() => $r->newLazyGhost(fn() => null, 3),
    fn() => $r->newLazyGhost(fn() => null, ReflectionClass::SKIP_DESTRUCTOR),
    fn() => $r->resetAsLazyGhost(new stdClass, fn() => null),
    fn() => $r->isUninitializedLazyObject(1),
    fn() => $r->resetAsLazyGhost($r->newLazyGhost(fn() => null), fn() => null),
    fn() => (new ReflectionClass('ArrayObject'))->newLazyGhost(fn() => null),
    fn() => (new ReflectionClass(RuntimeException::class))->newLazyGhost(fn() => null),
    fn() => (new ReflectionClass('Stringable'))->newLazyGhost(fn() => null),
];
foreach ($tries as $t) {
    try { $t(); echo "no error\n"; } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
}
class MyEx extends Exception {}
try { (new ReflectionClass(MyEx::class))->newLazyGhost(fn() => null); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
$std = (new ReflectionClass('stdClass'))->newLazyGhost(function ($o) { $o->x = 1; });
var_dump($std->x, $std);
// An initializer that throws leaves the object lazy, its slots empty again.
$g6 = $r->newLazyGhost(function (Svc $o) { $o->a = 99; throw new RuntimeException('boom'); });
try { $g6->a; } catch (RuntimeException) {}
var_dump($r->isUninitializedLazyObject($g6));
var_dump($g6);
var_dump(ReflectionClass::SKIP_INITIALIZATION_ON_SERIALIZE, ReflectionClass::SKIP_DESTRUCTOR);
