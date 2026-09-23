<?php
// Tier-A differential: bcadd/bcsub/bcmul/bcdiv/bcmod/bcdivmod — results
// truncated (never rounded) to the scale, zero-padded to it, the sign of a
// zero that the scale hides dropped, operand formats (".5", "5.", "-0",
// leading zeros, an empty string), and numbers far past 64 bits.

$pairs = [
    ["1", "2"], ["1.5", "2.25"], ["-1.5", "2.25"], ["0.1", "0.2"], ["-0.001", "0"],
    [".5", "5."], ["-0", "+0.000"], ["000123.4500", "-0000.4500"], ["", "7"],
    ["99999999999999999999.999999999", "0.000000001"],
    ["-123456789012345678901234567890.123456789", "987654321098765432109876543210.987654321"],
];
foreach ($pairs as [$a, $b]) {
    foreach ([0, 2, 5, 12] as $s) {
        echo "$a,$b @$s: ", bcadd($a, $b, $s), " | ", bcsub($a, $b, $s), " | ", bcmul($a, $b, $s);
        if (bccomp($b, "0", 40) !== 0) {
            echo " | ", bcdiv($a, $b, $s), " | ", bcmod($a, $b, $s), " | ", implode(" ", bcdivmod($a, $b, $s));
        }
        echo "\n";
    }
}

// Truncation, not rounding.
var_dump(bcdiv("2", "3", 5), bcdiv("-2", "3", 5), bcmul("1.25", "1.25", 1), bcmul("-1.25", "1.25", 3));
var_dump(bcdiv("1", "8", 2), bcdiv("-1", "8", 2), bcdiv("12.3", "0.01", 0), bcdiv("123", "1000", 2));
var_dump(bcdiv("1", "1", 3), bcdiv("-7.5", "-1", 1), bcdiv("0.0012", "0.1", 2), bcdiv("5", "7", 0));
var_dump(bcmod("7", "3"), bcmod("-7", "3"), bcmod("7", "-3"), bcmod("7.5", "2", 1), bcmod("-7.5", "2", 2), bcmod("10", "0.3", 3));
var_dump(bcdivmod("7", "3"), bcdivmod("-7.25", "2", 2), bcdivmod("1", "3", 5));

// Big operands.
$big = str_repeat("9", 120);
echo bcmul($big, $big), "\n";
echo bcdiv($big, "7", 30), "\n";
echo bcadd($big, "1"), "\n";
echo bcsub("1", $big), "\n";
echo bcmod($big, "12345678901234567890"), "\n";
echo bcdiv(bcmul($big, "123456789"), "123456789"), "\n";

// The default scale is 0 here.
var_dump(bcadd("1.9", "0.05"), bcdiv("1", "3"));
// A NUL ends the number as C reads it.
var_dump(bcadd("1\0garbage", "1"));
