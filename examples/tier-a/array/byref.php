<?php
// By-reference builtins: the call writes back through the passed variable.

// --- sort family ---
$a = [3, 1, 2];
sort($a);
echo implode(",", $a) . "\n";            // 1,2,3
$b = [3, 1, 2];
rsort($b);
echo implode(",", $b) . "\n";            // 3,2,1
$c = ["banana" => 3, "apple" => 1, "cherry" => 2];
asort($c);
print_r($c);                             // apple,cherry,banana (by value, keys kept)
$d = ["banana" => 3, "apple" => 1, "cherry" => 2];
ksort($d);
print_r($d);                             // apple,banana,cherry (by key)
$e = [3, 1, 2];
arsort($e);
print_r($e);                             // 0=>3, 2=>2, 1=>1
$f = ["b" => 2, "a" => 1];
krsort($f);
print_r($f);                             // b, a

// --- push / pop / shift / unshift ---
$s = [1, 2];
$n = array_push($s, 3, 4);
echo $n . ":" . implode(",", $s) . "\n"; // 4:1,2,3,4
$p = array_pop($s);
echo $p . ":" . implode(",", $s) . "\n"; // 4:1,2,3
$sh = array_shift($s);
echo $sh . ":" . implode(",", $s) . "\n"; // 1:2,3
$cnt = array_unshift($s, 0);
echo $cnt . ":" . implode(",", $s) . "\n"; // 3:0,2,3

// --- splice ---
$g = [1, 2, 3, 4, 5];
$removed = array_splice($g, 1, 2, ["x", "y", "z"]);
echo implode(",", $g) . "\n";            // 1,x,y,z,4,5
echo implode(",", $removed) . "\n";      // 2,3

// --- preg_match with captures (by-ref $matches) ---
$m = [];
$r = preg_match("/(\d+)-(\d+)/", "ym 2026-06 end", $m);
echo $r . "\n";                          // 1
echo $m[0] . "|" . $m[1] . "|" . $m[2] . "\n"; // 2026-06|2026|06
$r2 = preg_match("/xyz/", "abc", $m2);
echo $r2 . "\n";                         // 0
var_dump($m2);                           // array(0) {}
