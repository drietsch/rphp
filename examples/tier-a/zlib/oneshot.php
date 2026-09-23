<?php
// gzcompress / gzdeflate / gzencode / zlib_encode: byte-exact output at
// every level and encoding. The large inputs are big enough for memLevel 9
// (what php's one-shot functions use) to change the bytes.
mt_srand(7);
$text = str_repeat("hello world ", 50) . "abc";
$noisy = '';
for ($i = 0; $i < 120000; $i++) $noisy .= chr(mt_rand(32, 90));
$mixed = '';
for ($i = 0; $i < 30000; $i++) $mixed .= mt_rand(0, 3) ? "word" . mt_rand(0, 99) . " " : chr(mt_rand(0, 255));

foreach ([-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9] as $l) {
    echo $l, " ", bin2hex(gzcompress($text, $l)), "\n";
    echo "  ", md5(gzdeflate($text, $l)), " ", md5(gzencode($text, $l)), "\n";
    echo "  ", md5(gzcompress($noisy, $l)), " ", md5(gzdeflate($mixed, $l)), " ", md5(gzencode($mixed, $l)), "\n";
}
echo bin2hex(gzencode("abc")), "\n";
echo bin2hex(gzencode("abc", 9, FORCE_DEFLATE)), "\n";
echo bin2hex(gzcompress("abc", 9, ZLIB_ENCODING_GZIP)), "\n";
echo bin2hex(gzdeflate("abc", 9, ZLIB_ENCODING_RAW)), "\n";
echo bin2hex(zlib_encode("abc", ZLIB_ENCODING_RAW)), "\n";
echo bin2hex(zlib_encode("abc", ZLIB_ENCODING_GZIP, 1)), "\n";
echo bin2hex(zlib_encode("x", ZLIB_ENCODING_DEFLATE, -1)), "\n";
echo bin2hex(gzcompress("")), " ", bin2hex(gzdeflate("")), " ", bin2hex(gzencode("")), "\n";
echo md5(gzcompress(str_repeat("\0", 300000), 9)), " ", md5(gzdeflate(str_repeat("ab", 100000), 1)), "\n";
// level 0 stores; the stored blocks are cut at 65535 bytes.
echo md5(gzdeflate($noisy, 0)), " ", strlen(gzdeflate($noisy, 0)), "\n";

foreach (['gzcompress', 'gzdeflate', 'gzencode'] as $f) {
    foreach ([[10], [-2], [1, 99], [1, 0]] as $a) {
        try { $f("x", ...$a); echo "$f ok\n"; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    }
}
try { zlib_encode("x", 99); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { zlib_encode("x", 15, 10); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { gzcompress([]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(gzcompress(123) === gzcompress("123"));
