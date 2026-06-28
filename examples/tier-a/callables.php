<?php
// Higher-order builtins: a callable (function-name string) is invoked by the
// builtin re-entering the engine. Exercises user functions and native callbacks.

function dbl($x) { return $x * 2; }
function is_even($n) { return $n % 2 == 0; }
function add($a, $b) { return $a + $b; }
function cmp_desc($a, $b) { return $b - $a; }
function cmp_asc($a, $b) { return $a - $b; }
function upper_match($m) { return strtoupper($m[0]); }

// array_map: user callback (keys preserved) and native callback
print_r(array_map('dbl', [1, 2, 3]));
print_r(array_map('strtoupper', ['x', 'y']));

// array_filter: with a predicate, and without (truthiness)
print_r(array_filter([1, 2, 3, 4, 5, 6], 'is_even'));
print_r(array_filter([0, 1, '', 'x', 2]));

// array_reduce
echo array_reduce([1, 2, 3, 4], 'add', 0) . "\n";   // 10

// usort / uasort / uksort with comparators (user + native strcmp)
$u = [3, 1, 4, 1, 5];
usort($u, 'cmp_desc');
echo implode(',', $u) . "\n";                        // 5,4,3,1,1
$ua = ['c' => 3, 'a' => 1, 'b' => 2];
uasort($ua, 'cmp_asc');
print_r($ua);                                        // a,b,c by value
$uk = ['banana' => 1, 'apple' => 2, 'cherry' => 3];
uksort($uk, 'strcmp');
print_r($uk);                                        // apple,banana,cherry by key

// call_user_func / call_user_func_array
echo call_user_func('add', 10, 20) . "\n";           // 30
echo call_user_func('strtoupper', 'hi') . "\n";      // HI
echo call_user_func_array('add', [7, 8]) . "\n";     // 15

// preg_replace_callback
echo preg_replace_callback('/[a-z]+/', 'upper_match', 'abc DEF ghi') . "\n"; // ABC DEF GHI
