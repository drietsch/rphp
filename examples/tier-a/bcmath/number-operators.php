<?php
// Tier-A differential: operator overloading on BcMath\Number — + - * / %
// ** with a Number, an int or a numeric string on either side, compound
// assignment, ++/--, unary minus, pow(); comparisons (==, <, <=>, sorting,
// in_array, max) against Numbers, ints, strings, bools, null, arrays and
// other objects; and the TypeError/ValueError/DivisionByZeroError paths.

use BcMath\Number;

$n = new Number("1.50");
$m = new Number("-0.25");
foreach ([$m, 2, "0.125", "-3", true] as $o) {
    $label = $o instanceof Number ? "Number($o)" : var_export($o, true);
    echo "$label: ", $n + $o, " ", $n - $o, " ", $n * $o, " ", $n / $o, " ", $n % $o, " | ";
    echo $o + $n, " ", $o - $n, " ", $o * $n, " ", $o / $n, " ", $o % $n, "\n";
}
echo $n ** 2, " ", $n ** "3", " ", $n ** -1, " ", 2 ** new Number(10), " ", pow($n, 2), " ", pow(new Number("0"), 2), "\n";
echo -$n, " ", +$n, " ", -$m, "\n";

$x = new Number("10");
$x += 5;
$x -= "0.5";
$x *= new Number("2");
$x /= 4;
$x %= "3";
$x **= 2;
echo $x, " ", $x->scale, "\n";
$y = new Number("1.25");
$y++;
echo $y, " ";
$y--;
$y--;
echo $y, " ", $n, "\n";
var_dump($n + 1, 1.5 + $n);

// Comparisons.
$one = new Number("1.5");
foreach (["1.5", "1.50000", 1, 2, "abc", " 1.5", null, false, true, [], new stdClass, new Number("1.49"), $one, new Number("-0")] as $o) {
    $label = is_object($o) ? get_class($o) . ($o instanceof Number ? "($o)" : "") : var_export($o, true);
    echo str_pad($label, 26), " == ", var_export($one == $o, true), "  < ", var_export($one < $o, true), "  > ", var_export($one > $o, true),
        "  <=> ", $one <=> $o, "  rev <=> ", $o <=> $one, "  rev < ", var_export($o < $one, true), "\n";
}
$z = (new Number("-0.001"))->mul(1, 2);
var_dump($z->value, $z <=> 0, $z == 0, (new Number("-0.001"))->add(0, 2) <=> 0);

$list = [new Number("3"), 2, "2.5", new Number("-1"), new Number("2.25"), "10"];
sort($list);
echo implode(" ", array_map('strval', $list)), "\n";
usort($list, fn($p, $q) => $q <=> $p);
echo implode(" ", array_map('strval', $list)), "\n";
var_dump(in_array("2.50", [new Number("2.5")]), in_array(new Number("2.5"), ["2.50"]), in_array(new Number("2.5"), ["2.50"], true));
echo max(new Number("5"), new Number("5.1"), "4"), " ", min(new Number("5"), 6), "\n";
var_dump(new Number(0) == false, new Number("0.1") == true, new Number(0) == null, null < new Number(-5));

$errors = [
    fn() => $n + null,
    fn() => $n - [],
    fn() => $n * new stdClass,
    fn() => $n % null,
    fn() => $n / [],
    fn() => $n + 1e30,
    fn() => $n + INF,
    fn() => $n + "abc",
    fn() => "abc" - $n,
    fn() => $n + " 1",
    fn() => $n / 0,
    fn() => $n / "0.0",
    fn() => $n % 0,
    fn() => $n ** "0.5",
    fn() => $n ** "99999999999999999999",
    fn() => new Number(0) ** -1,
    fn() => $n & 1,
    fn() => $n | 1,
    fn() => $n << 1,
    fn() => ~$n,
    fn() => $n . "!",
    fn() => $n + 1.5,
    fn() => $n == 1.5,
    fn() => pow(new stdClass, 2),
    function () { $o = new stdClass; $o++; },
];
foreach ($errors as $f) {
    try {
        $r = $f();
        echo is_object($r) ? "$r" : var_export($r, true), "\n";
    } catch (\Throwable $t) {
        echo get_class($t), ": ", $t->getMessage(), "\n";
    }
}
