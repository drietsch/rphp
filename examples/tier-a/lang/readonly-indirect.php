<?php
// A readonly property refuses every *indirect* modification — a fetch for
// writing, a reference, `unset` of an element, a by-reference argument —
// with php's own message, in every scope and even before initialization,
// except when it holds an object (a handle). A nested place sent as an
// argument (`f($this->list[$k])`) is fetched for writing only when the
// parameter turns out to be by-reference (`FETCH_DIM_FUNC_ARG`), so a
// by-value call reads a readonly array element without complaint.
class D { public function __construct(public $a, public $b = null) {} }
function f($x) { return $x; }
function g(&$x) { $x = 'changed'; }
class M {
    public function __construct(private readonly array $ttl, private readonly array $nested = ['k' => ['z' => 9]]) {}
    private string $name = 'k';
    function wrap() { return new D($this->ttl[$this->name], $this->nested['k']['z']); }
    function call() { return f($this->ttl[$this->name]); }
    function callm() { return $this->m($this->ttl[$this->name]); }
    function m($v) { return $v; }
    function bad() { try { g($this->ttl[$this->name]); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function bad2() { try { g($this->ttl); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function missing() { return f($this->ttl['nope'] ?? 'dflt'); }
    function missing2() { return f($this->ttl['nope']); }
}
$m = new M(['k' => 5]);
var_dump($m->wrap(), $m->call(), $m->callm(), $m->missing(), $m->missing2());
$m->bad(); $m->bad2();

class M2 {
    public function __construct(public readonly array $ttl = ['k' => 5], public readonly int $n = 1) {}
    function t1() { try { g($this->ttl); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } var_dump($this->ttl); }
    function t2() { try { g($this->ttl['k']); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } var_dump($this->ttl); }
    function t3() { try { $r = &$this->ttl; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t4() { try { $r = &$this->ttl['k']; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t5() { try { $this->ttl['k'] = 2; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t6() { try { $this->n++; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t7() { try { foreach ($this->ttl as &$v) {} } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t8() { try { sort($this->ttl); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t9() { try { $this->ttl[] = 1; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t10() { try { unset($this->ttl['k']); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t11() { try { array_push($this->ttl, 1); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
    function t12() { try { preg_match('/a/', 'a', $this->ttl); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
}
$m = new M2;
foreach (['t1','t2','t3','t4','t5','t6','t7','t8','t9','t10','t11','t12'] as $t) { echo "$t: "; $m->$t(); }
$o = new M2; try { g($o->ttl); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { g($o->ttl['k']); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
class O { public $x = 1; }
class M3 {
    public readonly array $arr;
    public readonly O $o;
    public readonly int $n;
    function __construct() {
        try { $this->arr[] = 1; var_dump($this->arr); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        $this->o = new O;
        $this->o->x = 2;              // allowed: the object is a handle
        $this->o->x++;
        var_dump($this->o->x);
        try { $r = &$this->o; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        try { $this->n++; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        try { $this->n += 1; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    }
}
$m = new M3;
try { $m->o->x = 5; var_dump($m->o->x); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { $m->arr['q'] = 1; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$c = clone $m;
var_dump($c->o === $m->o);

// A write fetch, a reference or a by-reference send of `$this->p` from an
// ancestor's method reaches the ancestor's own private slot when the
// subclass re-declares the name.
class P2 { private array $res = []; private $n = 0; function add($x) { $this->res['k'][] = $x; $this->n++; $r = &$this->res; $r['ref'] = 1; sort($this->res); return $this->res; } function pn() { return $this->n; } }
class C2 extends P2 { private array $res = ['own']; private $n = 100; function own() { return [$this->res, $this->n]; } }
$c2 = new C2; var_dump($c2->add(1), $c2->add(2), $c2->own(), $c2->pn());
