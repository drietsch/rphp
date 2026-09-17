<?php
// Tier-A differential: serialize() over scalars, floats (serialize_precision=-1
// layout: `d:1;`, `d:0.1;`, `d:1.0E+25;`, `d:INF;`), binary-safe strings,
// nested arrays with int/string keys, objects with visibility-mangled
// property names (`\0*\0prot`, `\0Class\0priv`) including inherited private
// slots, shared object handles as `r:N;`, and a serialize/unserialize round
// trip.

echo serialize(null), "\n";
echo serialize(true), serialize(false), "\n";
echo serialize(42), serialize(-1), serialize(constant('PHP_INT_MIN')), "\n";
echo serialize(1.0), serialize(0.1), serialize(-0.0), serialize(1e25), serialize(1e15), serialize(1.5), "\n";
echo serialize(fdiv(1, 0)), serialize(fdiv(-1, 0)), serialize(fdiv(0, 0)), serialize(123456789012345678.0), "\n";
echo serialize(""), serialize("abc"), serialize("a\0b"), serialize("日本"), "\n";
echo serialize([]), "\n";
echo serialize([1, 2, "k" => "v", 3 => [true, null]]), "\n";
echo serialize([-5 => 1.5, "10" => "ten", "x y" => ["nested" => []]]), "\n";

class Foo {
    public $p = 1;
    protected $q = [1];
    private $r = null;
    public $dyn;
}
class Bar extends Foo {
    private $s = "bar";
    public $z = 2;
}
// (A private property shadowing a parent's private of the same name is a
// second slot in php; the engine's layout merges them by name until the
// class model lands (E6), so the subclass uses a distinct name here.)
// (Every object stays referenced so handle numbers are the same in both
// engines: rphp allocates ids monotonically, php reuses freed ones.)
$f = new Foo;
$b1 = new Bar;
echo serialize($f), "\n";
echo serialize($b1), "\n";

// The same instance twice is a back-reference to its slot.
$b = new Bar;
$arr = [$b, $b, "k" => [$b], "n" => 1];
echo serialize($arr), "\n";

// Round trip: identity of the shared handle survives.
$u = unserialize(serialize($arr));
var_dump($u[0] === $u[1]);
var_dump($u[0] === $u["k"][0]);
var_dump($u[0] === $b);
var_dump($u);

// Scalars and arrays round-trip to identical values.
$vals = [1, 1.5, "s", true, null, [1 => [2 => 3]], -0.0, 1e25];
foreach ($vals as $v) {
    var_dump(unserialize(serialize($v)) === $v);
}
