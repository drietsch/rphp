<?php
// Parameter and return type checks in coercive mode: exact matches,
// scalar juggling in php's preference order, nullable / union types,
// class types, TypeError texts with the call site.

function i(int $x) { return $x; }
function f(float $x) { return $x; }
function s(string $x) { return $x; }
function b(bool $x) { return $x; }
function a(array $x) { return count($x); }
function n(?int $x) { return $x; }
function u(int|string $x) { return $x; }
function fs(float|string $x) { return $x; }
function ib(int|bool $x) { return $x; }
function m(mixed $x) { return $x; }
function it(iterable $x) { return 1; }
function cb(callable $x) { return 1; }
class Base {} class Child extends Base {}
function c(Base $x) { return get_class($x); }
function r(): int { return "5"; }
function rs(): string { return 12; }
function rn(): ?int { return null; }
function rv(): void { return; }
function rfail(): int { return "x"; }
function rnone(): int { }

var_dump(i("12"), i(1.0), i(true), i(" 7 "), i("1e3"));
var_dump(f(1), f("1.5"), f(true), f("7"));
var_dump(s(5), s(1.5), s(true), s(false));
var_dump(b(1), b("0"), b(0.0), b("abc"));
var_dump(n(null), n("3"), m(null), it([1]), cb('strlen'), c(new Child));
var_dump(u("5"), u("5.5"), u(true), u(2.0), fs(1), fs("7"), ib("5"), ib("abc"));
var_dump(r(), rs(), rn(), rv());

function try_call($f, ...$args) {
    try { var_dump($f(...$args)); }
    catch (TypeError $e) { echo str_replace(__FILE__, "FILE", $e->getMessage()), "\n"; }
}
try_call('i', "abc");
try_call('i', "12abc");
try_call('i', null);
try_call('i', []);
try_call('i', new Child);
try_call('i', 1.5);
try_call('i', 9.9e18);
try_call('s', []);
try_call('s', null);
try_call('a', 1);
try_call('a', true);
try_call('n', "x");
try_call('u', []);
try_call('c', new Base);
try_call('c', new stdClass);
try_call('c', "Base");
try_call('it', 1);
try_call('cb', 'nope');
try_call('rfail');
try_call('rnone');
try_call('ib', 1.5);

// Called directly from user code the message names the call site.
try { i("z"); } catch (TypeError $e) { echo str_replace(__FILE__, "FILE", $e->getMessage()), "\n"; }
$cl = function (int $q): string { return $q; };
var_dump($cl(3));
class T { function m(int $i): int { return $i; } }
try { (new T)->m("z"); } catch (TypeError $e) { echo str_replace(__FILE__, "FILE", $e->getMessage()), "\n"; }
class Str { function __toString(): string { return "stringable"; } }
var_dump(s(new Str));
echo "end\n";
