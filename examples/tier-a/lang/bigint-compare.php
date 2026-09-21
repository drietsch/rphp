<?php
// Tier-A differential: integers past 2^53 compare exactly — `<=>`, `<`,
// `==`, sort() and SplPriorityQueue keep their order where a double would
// round two neighbours together; an int beside a float compares as doubles.
$a = 9223372036854775806; $b = 9223372036854775803;
var_dump($a <=> $b, $a > $b, $a == $b, $b < $a, $a - 1 <=> $a, PHP_INT_MAX <=> PHP_INT_MAX - 1, PHP_INT_MIN <=> PHP_INT_MIN + 1);
var_dump(9007199254740993 <=> 9007199254740992, 9007199254740993 == 9007199254740992, 9007199254740993 == 9007199254740992.0, 9007199254740993 <=> 9007199254740992.0);
$xs = [9223372036854775806, 9223372036854775803, 9223372036854775807, 9223372036854775800];
sort($xs); var_dump($xs);
$q = new SplPriorityQueue;
foreach ([[0, PHP_INT_MAX - 3], [0, PHP_INT_MAX - 1], [0, PHP_INT_MAX - 2], [1, PHP_INT_MAX - 5]] as $i => $p) { $q->insert("v$i", $p); }
while (!$q->isEmpty()) { echo $q->extract(), " "; } echo "\n";
var_dump(max(9223372036854775806, 9223372036854775803), min([9223372036854775806, 9223372036854775803]));
var_dump("9223372036854775806" <=> "9223372036854775803", "9223372036854775806" == "9223372036854775803");
