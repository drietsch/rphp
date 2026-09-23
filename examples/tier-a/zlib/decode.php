<?php
// gzuncompress / gzinflate / gzdecode / zlib_decode: round trips,
// $max_length, the data errors, and zlib_decode's format detection.
$s = str_repeat("hello world ", 50);
$c = gzcompress($s); $d = gzdeflate($s); $g = gzencode($s);
var_dump(gzuncompress($c) === $s, gzinflate($d) === $s, gzdecode($g) === $s);
var_dump(zlib_decode($c) === $s, zlib_decode($d) === $s, zlib_decode($g) === $s);
var_dump(gzuncompress($c, 10));
var_dump(gzinflate($d, 10));
var_dump(gzdecode($g, 10));
var_dump(zlib_decode($c, 10));
var_dump(strlen(gzuncompress($c, 600)), strlen(gzuncompress($c, 599) ?: ''));
var_dump(strlen(gzuncompress($c, 0)), strlen(gzinflate($d, 5000)));
foreach (['gzuncompress', 'gzinflate', 'gzdecode', 'zlib_decode'] as $f) {
    try { $f($c, -1); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    var_dump($f("garbage"));
    var_dump($f(""));
}
var_dump(gzuncompress(substr($c, 0, 10)));
var_dump(gzinflate(substr($d, 0, 5)));
var_dump(gzdecode(substr($g, 0, 15)));
var_dump(gzuncompress($c . "trailing") === $s);
var_dump(gzinflate($d . "trailing") === $s);
var_dump(gzdecode($g . "trailing") === $s);
var_dump(gzuncompress($d));
var_dump(gzinflate($c));
var_dump(gzuncompress($g));
var_dump(gzdecode($c));
var_dump(gzinflate($g));
var_dump(zlib_decode(gzdeflate("abc")), zlib_decode(gzencode("abc") . "junk"), gzinflate(gzdeflate("abc") . "\x01"));
var_dump(gzdecode("\x1f\x8b"), zlib_decode("\x1f"));
// Two gzip members: gzdecode() stops after the first.
var_dump(gzdecode(gzencode("one") . gzencode("two")));
// A stream that needs a dictionary.
$dd = deflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => 'hello']);
$withdict = deflate_add($dd, "hello hello", ZLIB_FINISH);
var_dump(gzuncompress($withdict));
// Highly compressible data takes many of php's rounds.
$big = str_repeat("a", 3000000);
var_dump(strlen(gzuncompress(gzcompress($big))), strlen(gzinflate(gzdeflate($big, 9))));
mt_srand(3);
$r = '';
for ($i = 0; $i < 50000; $i++) $r .= chr(mt_rand(0, 255));
var_dump(gzdecode(gzencode($r)) === $r, zlib_decode(gzcompress($r, 0)) === $r);
