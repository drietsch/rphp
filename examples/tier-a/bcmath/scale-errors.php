<?php
// Tier-A differential: bcscale() and the bcmath.scale directive as the
// default scale, bccomp (operands cut to the scale first), and the errors
// every bc function shares: a malformed number, a scale out of range, a
// zero divisor, the wrong argument types.

var_dump(bcscale(), ini_get("bcmath.scale"));
var_dump(bcscale(3), bcscale(), ini_get("bcmath.scale"));
var_dump(bcadd("1", "2"), bcdiv("1", "3"), bcsqrt("2"), bcpow("2", "-1"), bccomp("1.0001", "1"));
ini_set("bcmath.scale", "5");
var_dump(bcscale(), bcmul("1.5", "1.5"), bcmod("10", "3"));
var_dump(bcscale(null), bcscale(0), bcadd("1.9", "1"));

foreach ([["1", "2"], ["2", "1"], ["1.0001", "1"], ["-1.0001", "-1"], ["-0", "0"], ["0.00", "-0.001"], ["1e0", "1"]] as [$a, $b]) {
    foreach ([0, 3, 10] as $s) {
        try {
            echo "cmp($a, $b, $s) = ", bccomp($a, $b, $s), "\n";
        } catch (\Throwable $t) {
            echo get_class($t), ": ", $t->getMessage(), "\n";
        }
    }
}

$errors = [
    fn() => bcadd(" 1", "1"),
    fn() => bcadd("1", "1 "),
    fn() => bcsub("1e5", "1"),
    fn() => bcmul("0x1A", "1"),
    fn() => bcdiv("1", "1_000"),
    fn() => bcmod("1.2.3", "1"),
    fn() => bcadd("--1", "1"),
    fn() => bcadd("1", "1", -1),
    fn() => bcadd("1", "x", -1),
    fn() => bcadd("1", "1", 2147483648),
    fn() => bcdivmod("x", "1"),
    fn() => bcdiv("1", "0"),
    fn() => bcdiv("1", "0.000"),
    fn() => bcmod("1", "0"),
    fn() => bcdivmod("1", "0"),
    fn() => bcscale(-1),
    fn() => bcscale(2147483648),
    fn() => bcadd([], "1"),
    fn() => bcadd("1", "1", "x"),
    fn() => bcadd("1"),
    fn() => bcadd(1, 2.5, 1),
    fn() => bccomp("1", "a"),
];
foreach ($errors as $f) {
    try {
        var_dump($f());
    } catch (\Throwable $t) {
        echo get_class($t), ": ", $t->getMessage(), "\n";
    }
}
