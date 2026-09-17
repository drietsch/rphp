<?php
// Tier-A differential: the Mt19937 engine — seeded sequences of mt_rand()
// (`>> 1` of the 32-bit word), mt_rand($min, $max) with rejection-sampled
// 32- and 64-bit ranges, rand() as its alias, the MT_RAND_PHP legacy twist
// with RAND_RANGE_BADSCALING (and its deprecation), seed wrapping, and the
// three standard functions drawing from the same engine: shuffle,
// str_shuffle, array_rand. Only seeded output is printed.

mt_srand(42);
echo mt_rand(), " ", mt_rand(), " ", mt_rand(1, 100), "\n";
mt_srand(42);
echo rand(), " ", rand(), " ", rand(1, 100), "\n";
mt_srand(42);
echo mt_rand(1, 100), " ", mt_rand(1, 100), " ", mt_rand(-5, 5), " ", mt_rand(0, constant('PHP_INT_MAX')), " ", mt_rand(constant('PHP_INT_MIN'), constant('PHP_INT_MAX')), "\n";
mt_srand(42);
echo rand(1, 100), " ", rand(100, 1), " ", rand(-5, 5), "\n";
mt_srand(42);
echo mt_rand(1, 6), mt_rand(1, 6), mt_rand(1, 6), mt_rand(1, 6), "\n";
mt_srand(42);
echo mt_rand(0, 4294967295), "\n";
mt_srand(42);
echo mt_rand(0, 4294967296), "\n";
mt_srand(42);
echo mt_rand(0, 1), mt_rand(0, 1), mt_rand(0, 1), mt_rand(0, 1), "\n";
mt_srand(42);
echo mt_rand(0, 255), " ", mt_rand(0, 255), " ", mt_rand(0, 0), " ", mt_rand(5, 5), "\n";
mt_srand(0);
echo mt_rand(), "\n";
mt_srand(-1);
echo mt_rand(), "\n";
mt_srand(4294967296);
echo mt_rand(), "\n";
srand(42);
echo rand(), "\n";
echo mt_getrandmax(), " ", getrandmax(), "\n";

// A long run crosses the 624-word reload boundary.
mt_srand(7);
$sum = 0;
$i = 0;
while ($i < 2000) {
    $sum = $sum + mt_rand(0, 9);
    $i = $i + 1;
}
echo $sum, " ", mt_rand(), "\n";

// Legacy mode (deprecated since 8.3).
mt_srand(42, 1);
echo mt_rand(), " ", mt_rand(1, 100), " ", rand(1, 100), "\n";

// shuffle / str_shuffle / array_rand draw from the engine.
mt_srand(1);
$a = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
shuffle($a);
echo implode(",", $a), "\n";
mt_srand(1);
echo str_shuffle("abcdefghij"), "\n";
mt_srand(1);
var_dump(array_rand([1, 2, 3, 4, 5]));
var_dump(array_rand(["a" => 1, "b" => 2, "c" => 3, "d" => 4, "e" => 5], 2));
mt_srand(1);
var_dump(array_rand([1, 2, 3], 3));
mt_srand(3);
$m = ["x" => 1, "y" => 2, "z" => 3];
shuffle($m);
var_dump($m);
$e = [];
shuffle($e);
var_dump($e);
echo str_shuffle(""), "|", str_shuffle("a"), "\n";
mt_srand(9);
var_dump(array_rand(["p" => 1, "q" => 2, "r" => 3, "s" => 4], 3));

// Unseeded / CSPRNG output only checked for shape.
var_dump(strlen(random_bytes(7)));
$r = random_int(-3, 3);
var_dump($r >= -3 && $r <= 3);
var_dump(random_int(9, 9));
$x = mt_rand(10, 20);
var_dump($x >= 10 && $x <= 20);
var_dump(mt_rand(5, 1));
