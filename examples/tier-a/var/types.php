<?php
// Tier-A differential: type.c — get_debug_type names, settype casts through a
// by-reference variable, is_iterable / is_countable / is_callable, intval with
// explicit and auto-detected bases, and var_dump's float layout
// (serialize_precision=-1: shortest round trip, `1.0E+25` past 17 digits).

class Foo {
    public $v = 1;
    public function m() { return 1; }
}
$foo = new Foo;
$vals = [1, 1.5, "s", true, null, [], $foo];
foreach ($vals as $v) {
    echo get_debug_type($v), "|", gettype($v), "\n";
}

var_dump(is_iterable([1]), is_iterable($foo), is_iterable("x"));
var_dump(is_countable([]), is_countable($foo), is_countable(1));

// settype writes the cast back into the variable.
$v = "12abc"; var_dump(settype($v, "integer"), $v);
$v = "1.5x"; var_dump(settype($v, "float"), $v);
$v = 1.9; var_dump(settype($v, "int"), $v);
$v = 0; var_dump(settype($v, "bool"), $v);
$v = 1.5; var_dump(settype($v, "string"), $v);
$v = "x"; var_dump(settype($v, "array"), $v);
$v = null; var_dump(settype($v, "array"), $v);
$v = 5; var_dump(settype($v, "null"), $v);
$v = 5; var_dump(settype($v, "boolean"), $v);
$v = 5; var_dump(settype($v, "double"), $v);
$v = 5; var_dump(settype($v, "INTEGER"), $v);
$v = "abc"; var_dump(settype($v, "integer"), $v);

// is_callable: strings naming functions, [$obj, 'method'] pairs, syntax-only.
function userfn() { return 1; }
var_dump(is_callable("strlen"), is_callable("userfn"), is_callable("nope"), is_callable("nope", true));
var_dump(is_callable([$foo, "m"]), is_callable([$foo, "zz"]), is_callable(["Foo", "m"]), is_callable([1, 2]), is_callable(1));
var_dump(is_callable("strlen", false, $name), $name);
var_dump(is_callable([$foo, "m"], false, $name2), $name2);

// intval with bases.
var_dump(intval("  12 "), intval("1e3"), intval("0b101", 0), intval("0o17", 0), intval("0o17", 8), intval("017", 8));
var_dump(intval("0x", 16), intval("-0x1f", 16), intval("z", 36), intval("12", 37), intval("12", 1), intval("  0x1A", 16));
var_dump(intval("1e3", 0), intval("0x1A", 0), intval("+0b11", 0), intval("0X1a", 16), intval("0B11", 2), intval("012", 10), intval("12abc", 0));
var_dump(intval("9999999999999999999999", 0), intval("ffffffffffffffffffff", 16), intval(null), intval([]), intval([0]), intval(true), intval("1.9999"), intval("0x1A", 10));
var_dump(floatval("1.5abc"), floatval("abc"), floatval(" .5"), floatval("-.5e-2"), boolval("0.0"), boolval([]), boolval("0"));
var_dump(strval(1.0), strval(1e25), strval(true), strval(null), strval(0.1 + 0.2), strval(-0.0));

// var_dump float layout.
var_dump(1.0, -0.0, 0.1 + 0.2, 1e15, 1e17, 1e25, 1e-7, 0.0001, 123456789012345678.0, 1.5e300, 2.0 ** 63);
var_dump(fdiv(1, 0), fdiv(-1, 0), fdiv(0, 0));
var_dump(1, "two", [3.0]);

// Objects count as iterable when Traversable, countable when Countable.
var_dump(is_iterable(new ArrayIterator([1])), is_iterable(new stdClass), is_iterable((function () { yield 1; })()), is_countable(new ArrayObject([])), is_countable(new stdClass));

// settype() is the cast: an array becomes a stdClass, a scalar its `scalar`
// property, and `resource` is refused.
$arr = [1, 'b' => 2]; settype($arr, 'object'); var_dump(get_class($arr), get_object_vars($arr)); $num = 5; settype($num, 'object'); var_dump(get_object_vars($num));
$lead = '12abc'; settype($lead, 'int'); var_dump($lead); try { settype($lead, 'resource'); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
