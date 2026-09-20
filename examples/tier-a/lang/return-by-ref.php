<?php
// `function &f()`: a returned place hands the caller its cell (`$x = &f()`
// binds, a by-value use derefs); a non-place earns php's notice and a
// fresh cell. Covers elements, properties, static properties, statics.
function &f(array &$a) { return $a[0]; }
$arr = [1]; $r = &f($arr); $r = 5; var_dump($arr);
class C { public $d = ['k' => 1]; function &g($k) { return $this->d[$k]; } }
$c = new C; $x = &$c->g('k'); $x = 7; var_dump($c->d);
$y = &$c->g('new'); $y = 3; var_dump($c->d);
$z = $c->g('k'); $z = 0; var_dump($c->d['k']);
class Reg { private array $items = []; public function &get(string $k) { if (!isset($this->items[$k])) { $this->items[$k] = null; } return $this->items[$k]; } public function all() { return $this->items; } }
$reg = new Reg;
$slot = &$reg->get('a'); $slot = [1]; $slot[] = 2; var_dump($reg->all());
$copy = $reg->get('a'); $copy[] = 3; var_dump($reg->all()['a']);
function &counter() { static $n = 0; $n++; return $n; }
$c = &counter(); $c += 10; var_dump(counter());
function &nonplace() { return 1 + 1; }
$v = &nonplace(); var_dump($v);
function &viaProp(object $o) { return $o->p; }
$o = new stdClass; $o->p = 'x'; $p = &viaProp($o); $p = 'y'; var_dump($o->p);
class S { public static $s = [1]; static function &sp() { return self::$s; } }
$sp = &S::sp(); $sp[] = 2; var_dump(S::$s);
var_dump(array_map(fn($x) => $x, [counter()]));
$arr = ['k' => 1];
function &el(array &$a, $k) { return $a[$k]; }
foreach ([el($arr, 'k')] as $x) { var_dump($x); }
$e = &el($arr, 'k'); $e = 99; var_dump($arr);
echo strlen(nonplace()), "\n";
