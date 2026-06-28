<?php
// Calling a callable held in a variable: closures, callable strings, currying.

// A closure stored in a variable, called directly.
$double = fn($x) => $x * 2;
echo $double(21) . "\n";                       // 42

$greet = function ($name) { return "Hi, " . $name; };
echo $greet("PHP") . "\n";                      // Hi, PHP

// A callable string in a variable (builtin and user function).
$up = 'strtoupper';
echo $up("abc") . "\n";                         // ABC
function inc($n) { return $n + 1; }
$f = 'inc';
echo $f(41) . "\n";                             // 42

// Immediately-invoked closure.
echo (fn($x) => $x + 1)(9) . "\n";              // 10

// Currying via nested arrow functions, then chained calls.
$adder = fn($a) => fn($b) => $a + $b;
$add5 = $adder(5);
echo $add5(3) . "\n";                           // 8
echo $adder(10)(20) . "\n";                     // 30
