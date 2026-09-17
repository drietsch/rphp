<?php
// Closures and arrow functions as first-class values, invoked through the
// higher-order builtins and call_user_func (direct $f() comes next).

// Arrow function with auto-capture by value.
$factor = 3;
$triple = fn($x) => $x * $factor;
echo call_user_func($triple, 5) . "\n";                 // 15
print_r(array_map(fn($x) => $x * $factor, [1, 2, 3]));  // 3, 6, 9

// Closure with an explicit `use` list.
$base = 10;
$adder = function ($x) use ($base) { return $x + $base; };
echo call_user_func($adder, 5) . "\n";                  // 15

// Capturing several variables.
$a = 1;
$b = 2;
$sum = function () use ($a, $b) { return $a + $b; };
echo call_user_func($sum) . "\n";                       // 3

// Closures driving higher-order builtins.
print_r(array_filter([1, 2, 3, 4, 5, 6], fn($n) => $n % 2 == 0));
$nums = [3, 1, 2];
usort($nums, fn($x, $y) => $x - $y);
echo implode(',', $nums) . "\n";                        // 1,2,3
echo array_reduce([1, 2, 3, 4], fn($c, $v) => $c + $v, 0) . "\n"; // 10

// Capture is by value: a later reassignment does not change the snapshot.
$n = 5;
$snap = fn() => $n;
$n = 99;
echo call_user_func($snap) . "\n";                      // 5
