<?php
// Engine faults are Throwable objects of the right class with php's
// messages; they are catchable by class, base class and interface.

function show($f) {
    try { $f(); echo "no fault\n"; }
    catch (Throwable $t) { echo get_class($t), ": ", $t->getMessage(), "\n"; }
}
show(function () { return 1 / 0; });
show(function () { return 1 % 0; });
show(function () { return [] + 1; });
show(function () { return "abc" * 2; });
show(function () { return 1 << -1; });
show(function () { $f = "nope"; $f(); });
show(function () { $o = new stdClass; $o->bar(); });
show(function () { $n = null; $n->bar(); });
show(function () { $c = "NoSuchClass"; new $c; });
show(function () { return NOPE; });
show(function () { return match (5) { 1 => "one" }; });
show(function () { return match ("str") { 1 => "one" }; });
show(function () { function two($a, $b) {} two(1); });
show(function () { function named($a) {} named(x: 1); });
show(function () { echo new stdClass; });
show(function () { return "x" . new stdClass; });
show(function () { new Throwable; });
show(function () { $a = [1]; return $a[[]]; });
show(function () { $s = "abc"; return $s["x"]; });
show(function () { intdiv(1, 0); });
show(function () { str_repeat("x", -1); });
show(function () { strlen(); });
class P { private $secret = 1; private function hidden() {} }
show(function () { return (new P)->secret; });
show(function () { (new P)->hidden(); });
show(function () { throw 42; });

// A fault inside a native callback propagates through the native.
try { array_map(fn($x) => $x % 0, [1]); } catch (DivisionByZeroError $e) { echo "via array_map: ", $e->getMessage(), "\n"; }

// catch (Exception) does not catch an Error; catch (Error) does.
try { try { 1 % 0; } catch (Exception $e) { echo "wrong\n"; } } catch (Error $e) { echo "Error branch: ", get_class($e), "\n"; }
// Class-hierarchy checks on the fault objects.
try { 1 % 0; } catch (ArithmeticError $e) { var_dump($e instanceof DivisionByZeroError, $e instanceof Error, $e instanceof Exception, $e->getCode(), $e->getLine()); }
echo "end\n";
