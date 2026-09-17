<?php
function f(int $a = 1, string ...$rest): ?int { return 1; }
function &g(&$x, array|null $y = [], A&B $z = null) {}
f(1, ...$args, ); f(a: 1, b: 2); f(...["a" => 1]); f(1)(2);
$c = static function () use (&$a, $b): int { return $a; };
$d = static fn&($x) => $x;
$e = #[A] fn(int ...$xs): int => 1;
$f = #[B] function (): never { throw new E; };
$g = strlen(...); $h = $o->m(...); $i = A::m(...); $j = $x(...); $k = $o->{"m"}(...);
$l = function () { yield 1; yield 2 => 3; yield from [4]; $y = yield; };
$m = fn() => fn() => 1;
