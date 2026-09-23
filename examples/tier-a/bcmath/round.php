<?php
// Tier-A differential: bcround in every RoundingMode (halves, ties to
// even/odd, negative precisions past the number's length, precisions past
// its scale that pad), bcfloor and bcceil, and the argument errors.

$modes = RoundingMode::cases();
$numbers = ["1.5", "2.5", "-1.5", "-2.5", "1.55", "1.45", "-1.451", "0.5", "-0.5", "1234.5678", "-1234.5678", "0.0001", "999.999", "5", "-5.0", "0"];
foreach ($numbers as $n) {
    foreach ([0, 1, 2, 6, -1, -2, -4, -6] as $p) {
        echo str_pad("$n @$p", 16);
        foreach ($modes as $m) {
            echo " ", bcround($n, $p, $m);
        }
        echo "\n";
    }
}
var_dump(bcround("1.955", 2), bcround("-1.955", 2), bcround("1.50", 1), bcround("12", 3));

foreach (["1.5", "-1.5", "1.0", "-1.000", "0.001", "-0.001", "0", "-0", "999.1", "-999.9", "123456789012345678901234567890.5"] as $n) {
    echo "$n: ", bcfloor($n), " ", bcceil($n), "\n";
}

$errors = [
    fn() => bcround("1.5", 0, 5),
    fn() => bcround("1.5", 0, null),
    fn() => bcround("x"),
    fn() => bcround("1.5", 2147483648),
    fn() => bcfloor("1e3"),
    fn() => bcceil(" 1"),
];
foreach ($errors as $f) {
    try {
        var_dump($f());
    } catch (\Throwable $t) {
        echo get_class($t), ": ", $t->getMessage(), "\n";
    }
}
