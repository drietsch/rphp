<?php
// Strings slice demo: string literals, escapes, concatenation, interpolation,
// and PHP-8 numeric-string semantics. Unlike the scalar-only m0.php, echo now
// produces real text (with newlines), so each line is labelled inline.

// --- literals & escapes ---
echo "single and double quotes\n";   // double quotes process \n
echo 'literal backslash-n: \n', "\n"; // single quotes keep \n literal

// --- concatenation ---
$greeting = "Hello" . ", " . "world!";
echo $greeting . "\n";               // Hello, world!

// --- interpolation ---
$name = "PHP";
$ver = 8;
echo "Running $name {$ver}.5\n";      // Running PHP 8.5

// --- PHP 8 concat precedence: `.` is looser than `+` ---
echo "sum = " . 1 + 2 . "\n";         // sum = 3   ("sum = " . (1+2))

// --- numeric strings juggle in arithmetic, compare per PHP 8 ---
echo "10 apples" + 5, "\n";           // 15  (leading-numeric coercion)

// The PHP 8 change: a number vs a non-numeric string compares as strings, so
// `0 == "foo"` is false. (Ternary/`?:` is not implemented yet, so use if/else.)
if (0 == "foo") {
    echo "0 == \"foo\"\n";
} else {
    echo "0 != \"foo\" (PHP 8)\n";    // this branch runs
}
