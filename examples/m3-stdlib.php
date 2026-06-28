<?php
// Standard-library slice demo: the first batch of builtins resolved through the
// rphp-stdlib registry — output/debug, type inspection, strings, arrays, math.
// (The language slice has no ternary yet, so checks use if/else.)

// --- type inspection & casts ---
echo gettype(42) . " " . gettype(3.5) . " " . gettype("x") . " " . gettype([1]) . "\n";
if (is_numeric("12.5")) { echo "num "; } else { echo "not "; }
if (is_numeric("12abc")) { echo "num\n"; } else { echo "not\n"; }
echo intval("0x1A", 16) . " " . intval("42px") . " " . strval(3.5) . "\n";

// --- strings ---
echo strtoupper("hello") . " " . ucfirst("php rocks") . "\n";
echo str_repeat("=", 10) . "\n";
echo substr("abcdef", 1, 3) . " " . substr("abcdef", -2) . "\n";
echo strpos("hello world", "world") . "\n";
echo str_replace("cat", "dog", "the cat sat") . "\n";
echo trim("  spaced  ") . "|" . "\n";
echo implode(", ", ["a", "b", "c"]) . "\n";
$parts = explode("-", "2026-06-28");
echo $parts[0] . "/" . $parts[1] . "/" . $parts[2] . "\n";
if (str_contains("needle in haystack", "in")) { echo "yes\n"; } else { echo "no\n"; }
echo ord("A") . " " . chr(66) . "\n";

// --- arrays ---
$xs = [3, 1, 4, 1, 5, 9, 2, 6];
echo count($xs) . " " . array_sum($xs) . "\n";
if (in_array(4, $xs)) { echo "has4 "; } else { echo "no4 "; }
if (in_array(7, $xs)) { echo "has7\n"; } else { echo "no7\n"; }
$merged = array_merge([1, 2], ["x" => 3], [4]);
echo count($merged) . " " . $merged["x"] . "\n";
$rev = array_reverse([1, 2, 3]);
echo $rev[0] . $rev[1] . $rev[2] . "\n";
echo implode(",", range(1, 5)) . "\n";
echo implode("", range("a", "e")) . "\n";

// --- math ---
echo abs(-7) . " " . max(3, 9, 2) . " " . min([4, 1, 8]) . "\n";
echo floor(3.7) . " " . ceil(3.2) . " " . round(3.14159, 2) . "\n";
echo sqrt(144) . " " . intdiv(17, 5) . "\n";

// --- var_dump / print_r ---
var_dump(42, "hi", true, null, 3.5);
var_dump(["a" => 1, "b" => [2, 3]]);
echo print_r(["name" => "rphp", "nums" => [1, 2]], true);
