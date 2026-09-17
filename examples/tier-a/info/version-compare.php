<?php
// Tier-A differential: version_compare — canonicalisation (`-`/`_`/`+` and
// other separators become `.`, digit/word boundaries split), the special
// forms dev < alpha/a < beta/b < RC/rc < # < pl/p, unknown words, longer
// versions, the operator argument (both spellings) and its ValueError.

$pairs = [
    ["1.0", "1.0.0"], ["1.0", "1.0.1"], ["5.2", "5.10"], ["1.0rc1", "1.0"], ["1.0RC1", "1.0rc1"],
    ["1.0-dev", "1.0"], ["1.0dev", "1.0alpha"], ["1.0a", "1.0b"], ["1.0beta", "1.0RC"], ["1.0RC", "1.0#"],
    ["1.0#", "1.0pl"], ["1.0pl", "1.0p"], ["1.0", "1.0.0-dev"], ["", ""], ["", "1"], ["1", ""],
    ["abc", "abd"], ["1.2.3", "1.2.3"], ["1..2", "1.2"], ["1.0.0", "1.0.0.0"], ["1_0", "1.0"], ["1-0", "1.0"],
    ["1+0", "1.0"], ["8.5.0", "8.4.99"], ["1.0.0-alpha", "1.0.0-beta"], ["v1.0", "1.0"], ["1.0 ", "1.0"],
    ["1.0.", "1.0"], ["1.0.", "1.0."], [".1", "0.1"], ["1.a", "1.b"], ["1.10a", "1.10b"], ["1.0.0-RC1", "1.0.0-RC2"],
    ["2.0", "10.0"], ["1.0.0-stable", "1.0.0"], ["1.0.0-snapshot", "1.0.0"], ["1.0zzz", "1.0"], ["#", "pl"],
    ["1.0.0-0", "1.0.0"], ["1.0.0.0.0.0", "1"], ["0", "0.0"], ["a", "1"], ["1", "a"], ["a", "a"], ["1.0-1", "1.0-1"],
    ["9007199254740993", "9007199254740992"], ["99999999999999999999", "1"], ["8.5.0", "8.5"], ["8.5", "8.5.0RC1"],
    ["7.4.33", "8.0.0"], ["2.1.0-beta.1", "2.1.0-beta.2"], ["1.0.0+build.1", "1.0.0"], ["5.3.0-dev", "5.3.0-alpha"],
];
foreach ($pairs as $pair) {
    $a = $pair[0];
    $b = $pair[1];
    echo "[", $a, "] vs [", $b, "]: ", version_compare($a, $b), " ";
    var_dump(version_compare($a, $b, "<"));
    var_dump(version_compare($a, $b, ">="));
    var_dump(version_compare($a, $b, "eq"));
}
$ops = ["<", "lt", "<=", "le", ">", "gt", ">=", "ge", "==", "eq", "!=", "<>", "ne"];
foreach ($ops as $op) {
    echo $op, ":";
    var_dump(version_compare("1.0", "1.1", $op));
}
var_dump(version_compare("1", "2", null));
var_dump(version_compare("1", "2", "bad"));
