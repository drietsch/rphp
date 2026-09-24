<?php
// Syntax errors: php's bison messages, the line, and the false return.
$cases = [
    "a = \"unterminated",
    "a = b = c",
    "yes = 1",
    "null = 1",
    "[sec",
    "a[ = 1",
    "a[x] 1",
    "a[x]",
    "=1",
    "a = (1",
    "a = 1)",
    "a = |1",
    "a = !",
    "a = \${x",
    "a = \${}",
    "a = 'x",
    "ok = 1\n\nb = (\n",
    "a = \"one\ntwo\nthree\" (",
    "[s]\n[t]\nx = )",
];
foreach ($cases as $c) {
    echo json_encode($c), "\n";
    var_dump(parse_ini_string($c));
}
var_dump(parse_ini_string("a[] = x\na = (", true, INI_SCANNER_TYPED));

// An unknown scanner mode.
var_dump(parse_ini_string("a=1", false, 3));
var_dump(parse_ini_string("a=1", false, -1));

// Argument types.
try {
    parse_ini_string([]);
} catch (\TypeError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
try {
    parse_ini_string("a=1", false, "x");
} catch (\TypeError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
