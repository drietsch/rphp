<?php
// array.c set operations: diff/intersect by value, key, assoc and their
// u*/ *u* callback variants; the (string) comparison rule; key preservation.

var_dump(array_diff([1, 2, 3, "3", 4.0, null, ""], [3, 4, null]), array_diff([1], []), array_diff([1]), array_diff(["a" => "x", "b" => "y"], ["y"], ["z"]));
var_dump(array_intersect(["a" => 1, 2, "x" => "2", 3], [2, "3"], ["2", 3, 5]), array_intersect([1, 2], []), array_intersect([1, "1", 1.0, true], [1]));
var_dump(array_diff_key(["a" => 1, "b" => 2, 1 => 3, "1" => 4], ["a" => 9], [1 => 0]), array_diff_assoc(["a" => "green", "b" => "brown", "c" => "blue", "red"], ["a" => "green", "yellow", "red"]), array_diff_assoc([1, "1", 1.0], ["1"]), array_diff_assoc(["a" => [1]], ["a" => [1]]));   // warnings: array to string
var_dump(array_intersect_key(["a" => 1, "b" => 2, "c" => 3], ["a" => 0, "c" => 0], ["c" => 5, "a" => 1]), array_intersect_assoc(["a" => "green", "b" => "brown", "c" => "blue", "red"], ["a" => "green", "b" => "yellow", "blue", "red"]), array_intersect_key([1]));
var_dump(array_diff([[1]], [1]));     // warning: array to string
function cmp_len($a, $b)
{
    return strlen($a) <=> strlen($b);
}
var_dump(array_intersect_ukey(["a" => 1, "b" => 2, 3], ["A" => 1, 5 => 1], "strcasecmp"), array_diff_ukey(["a" => 1, "bb" => 2, "ccc" => 3], ["x" => 0, "yy" => 0], "cmp_len"), array_diff_ukey([1], "strcmp"));
var_dump(array_udiff([1, 5, "5", "a"], [5], fn($a, $b) => $a <=> $b), array_udiff_assoc(["a" => 1, "b" => 5, 5], ["a" => 1, "B" => 5, 5], fn($a, $b) => $a <=> $b), array_udiff_uassoc(["a" => 1, "b" => 5, 5], ["A" => "1", "B" => 5, 5], fn($a, $b) => $a <=> $b, "strcasecmp"));
var_dump(array_uintersect([1, 5, "5", "a"], ["5", "A"], "strcasecmp"), array_uintersect_assoc(["a" => 1, "b" => 5, 5], ["a" => "1", "B" => 5, 5], fn($a, $b) => $a <=> $b), array_uintersect_uassoc(["a" => 1, "b" => 5, 5], ["A" => "1", "B" => 5, 5], fn($a, $b) => $a <=> $b, "strcasecmp"));
var_dump(array_diff_uassoc(["a" => 1, "b" => 5, 5], ["A" => "1", "B" => 5, 5], "strcasecmp"), array_intersect_uassoc(["a" => 1, "b" => 5, 5], ["A" => "1", "B" => 6, 5], "strcasecmp"));
// Callbacks are only ever asked about candidate pairs; results keep the
// first array's keys and order.
$calls = 0;
$r = array_udiff(["x" => 3, "y" => 1, "z" => 2], [1, 2], function ($a, $b) use (&$calls) {
    $calls++;
    return $a - $b;
});
var_dump($r, $calls > 0);
var_dump(array_udiff([1, 2, 3], [2], fn($a, $b) => $a > $b));   // deprecated: bool return
