<?php
// hebrev() reorders ISO-8859-8 Hebrew for visual display; print as hex.
$cases = [
    ["abc", 0], ["", 0], ["\xe0\xe1\xe2", 0], ["\xe0\xe1\xe2 abc \xe3\xe4", 0], ["\xe0\xe1\xe2 abc \xe3\xe4", 3],
    ["\xe0(\xe1)\xe2 [x] {y} <z> a/b c\\d", 0], ["line one\n\xe0\xe1 two\r\nthree \xe2", 0],
    ["\xe0\xe1 \xe2\xe3 \xe4\xe5 \xe6\xe7 \xe8\xe9", 4], ["\xe0\xe1 \xe2\xe3 \xe4\xe5 \xe6\xe7 \xe8\xe9", 1],
    ["hello, world. \xe0\xe1!", 5], ["  \xe0  ", 0], ["\n\n\xe0\n", 2], ["abc def ghi", -1], ["x", 1],
    ["12-34 \xf0\xf1/\xf2", 0], ["\xe0\t\xe1", 0],
];
foreach ($cases as [$s, $m]) {
    echo bin2hex($s), " / $m => ", bin2hex(hebrev($s, $m)), "\n";
}
var_dump(hebrev("plain text"));
try { hebrev([]); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
