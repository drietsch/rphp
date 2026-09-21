<?php
// __debugInfo(): var_dump()/print_r() show its table (mangled keys print
// as their visibility, integer keys as indexes), var_export()/(array)/
// json_encode()/get_object_vars() ignore it; null is deprecated and empty.
class A { public $a = 1; private $p = 2; function __debugInfo() { return ['x' => $this->a, "\0A\0priv" => 5, "\0*\0prot" => 6, 3 => 'i', 'obj' => $this]; } }
$a = new A; var_dump($a); print_r($a); var_export($a); echo "\n"; var_dump((array)$a, get_object_vars($a)); echo json_encode($a), "\n";
class B { function __debugInfo() { return null; } } var_dump(new B); print_r(new B); echo "\n";
class C extends A { public $c = 3; } var_dump(new C);
class D extends A { function __debugInfo() { return ['d' => 1] + parent::__debugInfo(); } } print_r(new D);
class N { public array $items = []; function __debugInfo() { return $this->items; } } $n = new N; $n->items = [1, [2, 3]]; var_dump($n); print_r($n);
class R { public $self; function __debugInfo() { return ['self' => $this]; } } $r = new R; var_dump($r); print_r($r);
class T { public int $typed; public $u; function __debugInfo() { return ['typed' => 'not set']; } } var_dump(new T);
$nested = ['k' => new A]; var_dump($nested); print_r($nested);
