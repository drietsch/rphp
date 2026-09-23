<?php
// Tier-A differential: BcMath\Number itself — construction from strings
// and ints, the virtual readonly `value`/`scale` properties, the dumps
// (var_dump, print_r, var_export, json_encode, (array), get_object_vars),
// string and bool casts, clone, serialize round trips, and the errors for
// writes, unsets, bad constructor arguments and bad serialized data.

use BcMath\Number;

$n = new Number("1.50");
var_dump($n);
print_r($n);
echo "\n";
var_export($n);
echo "\n", json_encode($n), "\n";
var_dump((array) $n, get_object_vars($n));
var_dump($n->value, $n->scale, (string) $n, "$n!", $n . "x", strlen($n));
var_dump(new Number(0), new Number(-42), new Number("-0.000"), new Number(".5"), new Number("0012.3400"), new Number(PHP_INT_MIN));
var_dump((bool) new Number("0.00"), (bool) new Number("0.01"), !new Number(0), new Number("-1") ? "yes" : "no");
var_dump(isset($n->value), isset($n->scale), isset($n->nope), empty($n->value), empty((new Number(0))->scale), property_exists($n, "value"));
var_dump($n instanceof Stringable, (new ReflectionClass(Number::class))->isFinal());

$c = clone $n;
var_dump($c, $c == $n, $c === $n, $c->value);

$s = serialize($n);
var_dump($s, unserialize($s), unserialize($s) == $n);
var_dump($n->__serialize());

$x = &$n->value;
var_dump($x);

$errors = [
    function () use ($n) { $n->value = "2"; },
    function () use ($n) { $n->scale = 5; },
    function () use ($n) { $n->scale++; },
    function () use ($n) { $n->dynamic = 1; },
    function () use ($n) { unset($n->value); },
    function () use ($n) { unset($n->scale); },
    function () use ($n) { unset($n->other); echo "unset other: ok\n"; },
    function () use ($n) { $n->__construct(5); },
    function () use ($n) { $n->__unserialize(["value" => "1"]); },
    fn() => $n->nope,
    fn() => new Number("abc"),
    fn() => new Number(" 1"),
    fn() => new Number("1e5"),
    fn() => new Number([]),
    fn() => (string) new Number(1.5),
    fn() => new Number(),
    fn() => unserialize('O:13:"BcMath\Number":1:{s:5:"value";s:3:"abc";}'),
    fn() => unserialize('O:13:"BcMath\Number":1:{s:5:"value";s:0:"";}'),
    fn() => unserialize('O:13:"BcMath\Number":1:{s:5:"value";i:5;}'),
    fn() => unserialize('O:13:"BcMath\Number":0:{}'),
    fn() => (int) new Number("5.5"),
    fn() => (float) new Number("5.5"),
];
foreach ($errors as $f) {
    try {
        var_dump($f());
    } catch (\Throwable $t) {
        echo get_class($t), ": ", $t->getMessage(), "\n";
    }
}
var_dump($n);
