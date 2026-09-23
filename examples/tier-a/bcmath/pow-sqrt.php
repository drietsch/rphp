<?php
// Tier-A differential: bcpow (negative exponents, the scale rules, big
// powers), bcpowmod (negative bases and moduli, modulus ±1), bcsqrt
// (libbcmath's Newton iteration, numbers below one), and their errors.

foreach (["2", "-2", "1.5", "-0.1", "0.5", "10", "0", "123.456"] as $b) {
    foreach (["0", "1", "3", "-1", "-3", "10"] as $e) {
        try {
            echo "$b ** $e: ", bcpow($b, $e, 4), " ", bcpow($b, $e), " ", bcpow($b, $e, 12), "\n";
        } catch (\Throwable $t) {
            echo "$b ** $e: ", get_class($t), ": ", $t->getMessage(), "\n";
        }
    }
}
echo bcpow("2", "512"), "\n";
echo bcpow("1.0001", "300", 40), "\n";
echo bcpow("-3", "33"), "\n";
var_dump(bcpow("4", "2.0"), bcpow("-0.1", "3", 2));

foreach ([["4", "13", "497"], ["-4", "13", "497"], ["4", "13", "-497"], ["123456789", "987654321", "1000000007"], ["5", "0", "7"], ["0", "5", "7"], ["7", "3", "1"], ["7", "3", "-1"]] as [$b, $e, $m]) {
    echo "powmod($b, $e, $m): ", bcpowmod($b, $e, $m), " ", bcpowmod($b, $e, $m, 3), "\n";
}

foreach (["2", "0.5", "0.0001", "100", "1", "0", "12345678901234567890", "0.000000000001", "3.999999999999"] as $n) {
    echo "sqrt($n): ", bcsqrt($n), " ", bcsqrt($n, 5), " ", bcsqrt($n, 30), "\n";
}

$errors = [
    fn() => bcpow("2", "1.5"),
    fn() => bcpow("2", "99999999999999999999"),
    fn() => bcpow("0", "-2"),
    fn() => bcpow("x", "2"),
    fn() => bcpow("2", "x"),
    fn() => bcpowmod("4.5", "2", "3"),
    fn() => bcpowmod("4", "2.5", "3"),
    fn() => bcpowmod("4", "-2", "3"),
    fn() => bcpowmod("4", "2", "3.5"),
    fn() => bcpowmod("4", "2", "0"),
    fn() => bcpowmod("4", "2", "x"),
    fn() => bcsqrt("-4"),
    fn() => bcsqrt("abc"),
    fn() => bcsqrt("4", -1),
];
foreach ($errors as $f) {
    try {
        var_dump($f());
    } catch (\Throwable $t) {
        echo get_class($t), ": ", $t->getMessage(), "\n";
    }
}
