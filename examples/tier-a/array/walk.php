<?php
// array.c walking and the internal pointer: array_walk (by-reference
// callback, extra argument), array_walk_recursive, current/key/next/prev/
// reset/end/pos, plus the by-ref mutators on fresh arrays.

$a = ["a" => 1, "b" => 2];
var_dump(array_walk($a, function ($v, $k, $extra) {
    echo "$k=$v/$extra ";
}, "X"));
$n = [1, [2, 3, [4]], "k" => 5];
var_dump(array_walk_recursive($n, function ($v, $k) {
    echo "$k=$v ";
}));
$w = [1, 2, 3];
array_walk($w, function (&$v, $k) {
    $v = $v * 10;
});
var_dump($w);
$w2 = [1, [2, 3], "x" => ["y" => 4]];
array_walk_recursive($w2, function (&$v) {
    $v = $v + 1;
});
var_dump($w2);
$w3 = ["p" => "a", "q" => "b"];
array_walk($w3, function (&$v, $k, $suffix) {
    $v = $v . $k . $suffix;
}, "!");
var_dump($w3);
$e = [];
var_dump(array_walk($e, fn($v) => print("never")));

// --- internal pointer ---
$p = ["x" => 1, "y" => 2, "z" => 3];
var_dump(current($p), key($p), next($p), key($p), next($p), next($p), key($p), current($p), reset($p), end($p), key($p), prev($p), prev($p), prev($p), current($p), pos($p));
$q = [];
var_dump(current($q), key($q), next($q), reset($q), end($q));
$r = [5 => "a", "b"];
var_dump(end($r), key($r), reset($r), key($r));
// The pointer survives a copy and is per array value.
$s = [1, 2, 3];
next($s);
$t = $s;
next($t);
var_dump(current($s), current($t));
foreach ($s as $v) {
}
var_dump(current($s));
