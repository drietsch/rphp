<?php
// ReflectionFunctionAbstract::getStaticVariables(): a closure's `use`
// variables (by-reference ones as the shared cell), then the `static`
// variables — their current value once bound, else a constant-expression
// initializer's value (php 8.3 evaluates those on demand) and null for any
// other initializer until the body first runs.
function f($p) { static $s = 5; static $t; }
$a = 1; $b = [2];
$c = function() use ($a, &$b) { static $z = 9; return $a; };
var_dump((new ReflectionFunction("f"))->getStaticVariables());
var_dump((new ReflectionFunction($c))->getStaticVariables());
$c(); $z = 1;
var_dump((new ReflectionFunction($c))->getStaticVariables());
var_dump((new ReflectionFunction("strlen"))->getStaticVariables());
class K { const X = 4; function m() { static $q = self::X; return fn() => $this; } }
var_dump((new ReflectionMethod("K", "m"))->getStaticVariables(), (new ReflectionFunction((new K)->m()))->getStaticVariables());
function g() { static $u = PHP_VERSION_ID > 0 ? "x" : "y"; static $n = 0; $n++; return $n; }
var_dump((new ReflectionFunction("g"))->getStaticVariables());
g(); g();
var_dump((new ReflectionFunction("g"))->getStaticVariables());
function h() { static $w = strlen("abc"); static $v = 1 + 2; static $k = [1, 2][1]; return $w; }
var_dump((new ReflectionFunction("h"))->getStaticVariables());
h();
var_dump((new ReflectionFunction("h"))->getStaticVariables());
