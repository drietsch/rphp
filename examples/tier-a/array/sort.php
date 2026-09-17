<?php
// array.c sorting: every SORT_* flag for sort/rsort/asort/arsort/ksort/krsort,
// natsort/natcasesort, usort family (incl. the deprecated bool return),
// array_multisort, array_unique flags. Mixed-type input exercises php's own
// sort algorithm order.

$a = ["10", 9, "9a", 1.5, true, null, "abc", "ABC", "", [1], "1e1", "02", 2];
foreach ([SORT_REGULAR, SORT_NUMERIC, SORT_STRING, SORT_STRING | SORT_FLAG_CASE, SORT_LOCALE_STRING, SORT_NATURAL, SORT_NATURAL | SORT_FLAG_CASE] as $f) {
    $b = $a;
    sort($b, $f);
    echo $f, ": ", json_encode($b), "\n";
    $b = $a;
    rsort($b, $f);
    echo "r", $f, ": ", json_encode($b), "\n";
}
$k = ["b" => 1, "a" => 2, "10" => 3, "9" => 4, "-1" => 5, "1.5" => 6, "B" => 7, "img12" => 8, "img10" => 9, "" => 10];
foreach ([SORT_REGULAR, SORT_NUMERIC, SORT_STRING, SORT_STRING | SORT_FLAG_CASE, SORT_NATURAL, SORT_NATURAL | SORT_FLAG_CASE] as $f) {
    $b = $k;
    ksort($b, $f);
    echo "k", $f, ": ", json_encode($b), "\n";
    $b = $k;
    krsort($b, $f);
    echo "kr", $f, ": ", json_encode($b), "\n";
}
$b = [3, "3", 3.0, "a", "b", "A", null, false, true, 0, "0", ""];
asort($b);
echo "asort: ", json_encode($b), "\n";
arsort($b);
echo "arsort: ", json_encode($b), "\n";
$b = ["x" => 3, "y" => 1, "z" => 2, 5 => 0];
rsort($b);
echo "rsort: ", json_encode($b), "\n";
$b = ["img12.png", "img10.png", "IMG2.png", "img1.png", "Img3.png"];
natsort($b);
echo "natsort: ", json_encode($b), "\n";
natcasesort($b);
echo "natcasesort: ", json_encode($b), "\n";
// A larger array takes the quicksort path (and stays stable for ties).
$big = [];
for ($i = 0; $i < 60; $i++) {
    $big[] = ($i * 7919) % 23;
}
sort($big);
echo json_encode($big), "\n";
$big = [];
for ($i = 0; $i < 40; $i++) {
    $big["k" . $i] = ($i * 31) % 7;
}
asort($big);
echo json_encode($big), "\n";
arsort($big);
echo json_encode(array_slice($big, 0, 12, true)), "\n";
$mixed = [];
for ($i = 0; $i < 30; $i++) {
    $mixed[] = ($i % 3 == 0) ? "x" . $i : (($i % 3 == 1) ? $i : $i / 2);
}
sort($mixed);
echo json_encode($mixed), "\n";

// --- user sorts ---
$u = [5, 3, 8, 1, 9, 2];
usort($u, fn($x, $y) => $x <=> $y);
echo json_encode($u), "\n";
usort($u, fn($x, $y) => $y - $x);
echo json_encode($u), "\n";
usort($u, fn($x, $y) => $x > $y);           // deprecated: bool return (once)
echo json_encode($u), "\n";
usort($u, fn($x, $y) => $x < $y);
echo json_encode($u), "\n";
usort($u, fn($x, $y) => 0.5);               // float return truncates to 0
echo json_encode($u), "\n";
$ua = ["b" => 2, "a" => 3, "c" => 1];
uasort($ua, fn($x, $y) => $x <=> $y);
echo json_encode($ua), "\n";
uksort($ua, "strcmp");
echo json_encode($ua), "\n";
uksort($ua, fn($x, $y) => strcmp($y, $x));
echo json_encode($ua), "\n";
$w = ["x10", "x9", "X1"];
usort($w, "strnatcasecmp");
echo json_encode($w), "\n";

// --- array_multisort ---
$a1 = [3, 1, 2];
$a2 = ["c", "a", "b"];
var_dump(array_multisort($a1, $a2), $a1, $a2);
$d1 = [10, 100, 100, 0];
$d2 = [1, 3, 2, 4];
array_multisort($d1, $d2);
echo json_encode($d1), json_encode($d2), "\n";
// (flags go through variables: the engine has no prefer-ref for literals yet)
$desc = SORT_DESC;
$asc = SORT_ASC;
$str = SORT_STRING;
$num = SORT_NUMERIC;
$e1 = ["10", 11, 100, 100, "a"];
$e2 = [1, 2, "2", 3, 1];
array_multisort($e1, $desc, $str, $e2, $num, $desc);
echo json_encode($e1), json_encode($e2), "\n";
$s = ["x" => 3, "y" => 1, 5 => 2];
array_multisort($s);
echo json_encode($s), "\n";
$m = [3, 1];
var_dump(array_multisort($m, $asc, $num), $m);
$n1 = ["b", "a", "b", "a"];
$n2 = [2, 2, 1, 1];
array_multisort($n1, $desc, $n2, $asc);
echo json_encode($n1), json_encode($n2), "\n";
$empty = [];
var_dump(array_multisort($empty));

// --- array_unique ---
$b = [1, "1", 1.0, true, "01", "a", "A", null, "", 0, false, "1e0", [1], [1]];
foreach ([SORT_STRING, SORT_REGULAR, SORT_NUMERIC, SORT_LOCALE_STRING] as $f) {
    echo "unique", $f, ": ", json_encode(array_unique($b, $f)), "\n";   // warnings: array to string
}
var_dump(array_unique([[1, 2], [2, 1], [1, 2]]));
var_dump(array_unique(["a" => "x", "b" => "X", "c" => "x"]), array_unique([3, "3", 4]), array_unique([]));
