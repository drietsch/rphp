<?php
// A closure is an object: it has a handle (spl_object_id / spl_object_hash),
// keys SplObjectStorage and WeakMap, and dumps as `object(Closure)#N` with
// php's debug table (name/file/line, static captures, this, parameters —
// or `function` for a first-class callable over a named function).
class A {
    public $p = 1;
    function m() { $y = 2; return function ($a, int $b = 3, ...$rest) use ($y) { return $this; }; }
    static function s() { return static function () {}; }
}
function named(string $s, &$r, $opt = null) {}

$f = function ($x) { return $x; };
$g = fn() => 1;
var_dump(spl_object_id($f) === spl_object_id($f), spl_object_id($f) === spl_object_id($g));
var_dump(spl_object_hash($f) === spl_object_hash($f), spl_object_hash($f) === spl_object_hash($g));
var_dump(strlen(spl_object_hash($f)));
var_dump($f == $f, $f === $g, $f instanceof Closure, is_object($f), gettype($f));

$s = new SplObjectStorage;
$s[$f] = 'f';
$s[$g] = 'g';
$s[$f] = 'F';
var_dump(count($s), $s[$f], isset($s[$g]), $s->contains($g));
unset($s[$g]);
var_dump(count($s));
foreach ($s as $i => $k) { var_dump($i, $k === $f, $s[$k]); }

$w = new WeakMap;
$w[$f] = 'a';
$w[$g] = 'b';
var_dump($w[$f], count($w), isset($w[$g]));
unset($g);
var_dump(count($w));
try { $w[fn() => 2]; } catch (Error $e) { echo preg_replace('/#\d+/', '#N', $e->getMessage()), "\n"; }

var_dump($f);
print_r($f); echo "\n";
var_dump((new A)->m());
var_dump(A::s());
var_dump(Closure::fromCallable([new A, 'm']));
var_dump(strlen(...));
var_dump(named(...));
var_dump((new A)->m(...));
var_dump(fn(&$r, ?A $a = null) => 1);
var_export($f); echo "\n";
var_dump((array) $f, (bool) $f, json_encode($f), get_object_vars($f));

// A closure capturing itself by reference dumps with *RECURSION*.
$rec = function () use (&$rec) {};
var_dump($rec);
print_r($rec); echo "\n";

// The Closure class behind the value: `::class`, is_a, method_exists,
// Reflection, clone, and the property/string-conversion errors.
$f = function ($x = 1) { return $x; };
var_dump($f::class, is_a($f, 'Closure'), is_a($f, 'Closure', true), is_subclass_of($f, 'Closure'));
var_dump(method_exists($f, 'bindTo'), method_exists($f, 'nope'), property_exists($f, 'x'));
$g = clone $f;
var_dump($g === $f, $g(5));
try { echo (string) $f; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { $f->foo = 1; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump($f->foo); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(isset($f->foo));
var_dump($f == new stdClass, $f == $g, in_array($f, [$g, $f], true), array_search($f, [$g, $f], true));
var_dump($f::fromCallable('strlen')('abc'));
var_dump(is_iterable($f), is_scalar($f), is_countable($f));
try { var_dump(count($f)); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump((new ReflectionObject($f))->getName(), (new ReflectionClass($f))->getName());
var_dump(class_implements($f), get_parent_class($f));
try { serialize($f); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump($f <=> $f, gettype($f), settype($f, 'array'), count($f));
