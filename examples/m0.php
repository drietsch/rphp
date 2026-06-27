<?php
// M0 scalar-slice demo: variables, arithmetic precedence, control flow,
// user functions, and recursion.
//
// NOTE: the M0 slice is scalar-only — no strings yet — so `echo` here prints
// raw numbers with no separators (newlines need string support, which lands in
// the next batch alongside arrays/objects + the GC). Each value is labelled in
// a comment with its expected result.

function factorial($n) {
    if ($n <= 1) {
        return 1;
    }
    return $n * factorial($n - 1);
}

function fib($n) {
    if ($n < 2) {
        return $n;
    }
    return fib($n - 1) + fib($n - 2);
}

// Sum 1..100 with a while loop.
$sum = 0;
$i = 1;
while ($i <= 100) {
    $sum = $sum + $i;
    $i = $i + 1;
}

echo factorial(5);   // 120
echo fib(10);        // 55
echo $sum;           // 5050
echo 2 ** 3 ** 2;    // 512  (** is right-associative)
echo 1 + 2 * 3;      // 7    (precedence)
