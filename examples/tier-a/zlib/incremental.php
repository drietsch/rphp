<?php
// deflate_init/deflate_add and inflate_init/inflate_add: every flush mode,
// the reset after ZLIB_FINISH, status and read length, dictionaries.
$d = deflate_init(ZLIB_ENCODING_DEFLATE);
var_dump($d);
$out = '';
foreach ([ZLIB_NO_FLUSH, ZLIB_PARTIAL_FLUSH, ZLIB_SYNC_FLUSH, ZLIB_FULL_FLUSH, ZLIB_BLOCK, ZLIB_FINISH] as $f) {
    $o = deflate_add($d, "chunk $f data data data ", $f);
    echo $f, ": ", bin2hex($o), "\n";
    $out .= $o;
}
echo bin2hex(deflate_add($d, "again", ZLIB_FINISH)), "\n";
echo bin2hex(deflate_add($d, "again")), "\n";
var_dump(bin2hex(deflate_add($d, "")), bin2hex(deflate_add($d, "", ZLIB_SYNC_FLUSH)));
var_dump(bin2hex(deflate_add($d, "", ZLIB_FINISH)), bin2hex(deflate_add($d, "", ZLIB_FINISH)));

$i = inflate_init(ZLIB_ENCODING_DEFLATE);
var_dump($i, inflate_get_status($i), inflate_get_read_len($i));
var_dump(inflate_add($i, substr($out, 0, 7)));
var_dump(inflate_get_status($i), inflate_get_read_len($i));
var_dump(inflate_add($i, substr($out, 7)));
var_dump(inflate_get_status($i), inflate_get_read_len($i));
var_dump(inflate_add($i, "more"));
var_dump(inflate_get_status($i), inflate_get_read_len($i));

$c = gzcompress(str_repeat("hello world ", 50));
$i = inflate_init(ZLIB_ENCODING_DEFLATE);
var_dump(inflate_add($i, substr($c, 0, 10), ZLIB_FINISH));
var_dump(inflate_get_status($i), inflate_get_read_len($i));
var_dump(inflate_add($i, "", ZLIB_FINISH), inflate_get_status($i));
var_dump(inflate_add($i, ""), inflate_get_status($i));
var_dump(strlen(inflate_add($i, substr($c, 10), ZLIB_SYNC_FLUSH)));
var_dump(inflate_get_status($i), inflate_get_read_len($i));

$i = inflate_init(ZLIB_ENCODING_GZIP);
var_dump(inflate_add($i, gzencode("abc")), inflate_get_status($i));
$i = inflate_init(ZLIB_ENCODING_GZIP);
var_dump(inflate_add($i, "abc"), inflate_get_status($i));
$i = inflate_init(ZLIB_ENCODING_RAW);
var_dump(inflate_add($i, gzdeflate("abc")), inflate_get_status($i));
var_dump(inflate_add($i, gzdeflate("xyz")), inflate_get_status($i), inflate_get_read_len($i));

// Output larger than one 8K chunk, and a stream fed byte by byte.
$big = str_repeat("0123456789", 5000);
$i = inflate_init(ZLIB_ENCODING_DEFLATE);
var_dump(strlen(inflate_add($i, gzcompress($big))), inflate_get_status($i));
$i = inflate_init(ZLIB_ENCODING_RAW);
$z = gzdeflate("byte by byte by byte");
$got = '';
foreach (str_split($z) as $ch) $got .= inflate_add($i, $ch, ZLIB_NO_FLUSH);
var_dump($got, inflate_get_status($i));

// A streamed deflate matches the one-shot bytes when the settings agree.
$d = deflate_init(ZLIB_ENCODING_GZIP, ['memory' => 9]);
$parts = '';
foreach (str_split($big, 777) as $p) $parts .= deflate_add($d, $p, ZLIB_NO_FLUSH);
$parts .= deflate_add($d, '', ZLIB_FINISH);
var_dump($parts === gzencode($big));

// Dictionaries.
$d = deflate_init(ZLIB_ENCODING_RAW, ['dictionary' => 'hello world']);
$c = deflate_add($d, "hello world hello", ZLIB_FINISH);
echo bin2hex($c), "\n";
$i = inflate_init(ZLIB_ENCODING_RAW, ['dictionary' => 'hello world']);
var_dump(inflate_add($i, $c));
$d = deflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => ['hello', 'world']]);
$c = deflate_add($d, "hello world hello", ZLIB_FINISH);
echo bin2hex($c), "\n";
$i = inflate_init(ZLIB_ENCODING_DEFLATE);
var_dump(inflate_add($i, $c));
$i = inflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => "hello\0world"]);
var_dump(inflate_add($i, $c));
$i = inflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => ['hello', 'world']]);
var_dump(inflate_add($i, $c), inflate_get_status($i));
$d = deflate_init(ZLIB_ENCODING_GZIP, ['dictionary' => 'hello']);
echo bin2hex(deflate_add($d, "hello hello", ZLIB_FINISH)), "\n";
$d = deflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => '']);
echo bin2hex(deflate_add($d, "hello", ZLIB_FINISH)), "\n";

// Windows.
$d = deflate_init(ZLIB_ENCODING_GZIP, ['level' => 9, 'window' => 9]);
echo bin2hex(deflate_add($d, str_repeat("abcdefgh", 100))), "\n";
$d = deflate_init(ZLIB_ENCODING_DEFLATE, ['window' => 8]);
echo bin2hex(deflate_add($d, str_repeat("abcdefgh", 100))), "\n";
$i = inflate_init(ZLIB_ENCODING_DEFLATE, ['window' => 9]);
var_dump(inflate_add($i, gzcompress(str_repeat("abcdefghij", 200))));
