<?php
// One property site, many shapes: the engine caches the slot a `$o->name`
// site resolved, so the same site must keep answering right for a dynamic
// name (`$o->$n` names every property from one site), for instances of
// different classes, for a slot that `unset()` handed to magic, for a slot
// holding a reference, for a value the declared type coerces, and for a
// readonly/asymmetric slot.
class Hydrator {
    public static function hydrate(object $o, array $props): void { foreach ($props as $n => $v) { $o->$n = $v; } }
    public static function extract(object $o, array $names): array { $out = []; foreach ($names as $n) { $out[$n] = $o->$n; } return $out; }
}
class P { public $a = 1; public int $b = 2; public ?string $c = null; public float $f = 0.0; }
class Q extends P { public $a = 'q'; public array $d = []; }
$p = new P; $q = new Q;
Hydrator::hydrate($p, ['a' => 10, 'b' => 20, 'c' => 'x', 'f' => 1]);
Hydrator::hydrate($q, ['a' => 11, 'b' => 21, 'c' => 'y', 'd' => [1], 'f' => 2.5]);
var_dump(Hydrator::extract($p, ['a', 'b', 'c', 'f']), Hydrator::extract($q, ['a', 'b', 'c', 'd', 'f']));

function setB(P $o, $v) { $o->b = $v; return $o->b; }
function getA(P $o) { return $o->a; }
foreach ([new P, new Q, new P, new Q] as $o) { var_dump(setB($o, 5), setB($o, "7"), getA($o)); }
try { setB(new P, "nope"); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
function setF(P $o, $v) { $o->f = $v; return $o->f; }
var_dump(setF($p, 3), setF($p, 3.5), setF($p, "4"));

class M { public int $n = 1; public $u = 'u';
    function __get($k) { echo "__get($k)\n"; return 99; }
    function __set($k, $v) { echo "__set($k)\n"; }
    function w() { $this->n = 2; return $this->n; }
    function wu($v) { $this->u = $v; return $this->u; } }
$m = new M; var_dump($m->w()); unset($m->n); var_dump($m->w(), $m->w());
var_dump($m->wu('a')); unset($m->u); var_dump($m->wu('b'), $m->wu('c'));

class R { public $v = 1; public array $list = []; function set($x) { $this->v = $x; } function get() { return $this->v; } }
$r = new R; $ref = &$r->v; $r->set(2); $r->set(3); var_dump($ref, $r->get()); $ref = 4; var_dump($r->get());
$r2 = new R; $r2->set(5); var_dump($r2->get(), $r->get());

class RO { public function __construct(public readonly int $x, public private(set) int $y = 1) {} function bump() { $this->y++; return $this->y; } }
$ro = new RO(1); var_dump($ro->bump(), $ro->bump());
try { $ro->x = 2; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { $ro->y = 2; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$ro2 = new RO(3); try { $ro2->x = 4; } catch (Error $e) { echo $e->getMessage(), "\n"; }

class Base { private $p = 'base'; function readP() { return $this->p; } function writeP($v) { $this->p = $v; } }
class Child extends Base { private $p = 'child'; function readC() { return $this->p; } }
$c = new Child; $c->writeP('B'); var_dump($c->readP(), $c->readC()); $b = new Base; $b->writeP('B2'); var_dump($b->readP());
