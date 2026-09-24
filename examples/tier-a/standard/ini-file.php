<?php
// parse_ini_file: files, their errors (named by the path as given) and
// what only a file can carry (NUL bytes, a BOM, CRLF lines).
$f = 'rphp-ini-test.ini';

file_put_contents($f, "; config\n[db]\nhost = localhost\nport = 5432\n\n[cache]\nenabled = on\nttl = 60\n");
var_dump(parse_ini_file($f));
var_dump(parse_ini_file($f, true));
var_dump(parse_ini_file($f, true, INI_SCANNER_TYPED));

file_put_contents($f, "\xEF\xBB\xBFa = 1\r\nb = 2\rc = 3\n");
var_dump(parse_ini_file($f));

file_put_contents($f, "a = 1\x00b = 2\nc = 3\n");
var_dump(parse_ini_file($f));

file_put_contents($f, "ok = 1\nbad = (\n");
var_dump(parse_ini_file($f));
var_dump(parse_ini_file("./$f", false, 9));

unlink($f);
var_dump(parse_ini_file($f));

foreach (["", "a\0b"] as $name) {
    try {
        parse_ini_file($name);
    } catch (\ValueError $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
