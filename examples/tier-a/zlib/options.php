<?php
// deflate_init()'s options: every memory level, strategy and window, at
// several levels — the parameters only php's own zlib reaches — and the
// validation errors.
mt_srand(11);
$data = '';
for ($i = 0; $i < 40000; $i++) {
    $data .= mt_rand(0, 4) ? chr(mt_rand(97, 102)) : str_repeat(chr(mt_rand(97, 99)), mt_rand(3, 40));
}
$data = substr($data, 0, 90000);
$strategies = [ZLIB_DEFAULT_STRATEGY, ZLIB_FILTERED, ZLIB_HUFFMAN_ONLY, ZLIB_RLE, ZLIB_FIXED];
foreach ([1, 4, 6, 9] as $level) {
    foreach ([1, 5, 8, 9] as $mem) {
        $row = [];
        foreach ($strategies as $st) {
            $d = deflate_init(ZLIB_ENCODING_DEFLATE, ['level' => $level, 'memory' => $mem, 'strategy' => $st]);
            $row[] = substr(md5(deflate_add($d, $data, ZLIB_FINISH)), 0, 12);
        }
        echo "L$level M$mem ", implode(' ', $row), "\n";
    }
}
foreach ([8, 9, 10, 12, 15] as $w) {
    $row = [];
    foreach ([ZLIB_ENCODING_RAW, ZLIB_ENCODING_DEFLATE, ZLIB_ENCODING_GZIP] as $enc) {
        $d = @deflate_init($enc, ['window' => $w]);
        $row[] = $d ? substr(md5(deflate_add($d, $data, ZLIB_FINISH)), 0, 12) : 'false';
    }
    echo "W$w ", implode(' ', $row), "\n";
}
// Level 0 over small, flushed pieces: stored blocks follow the buffers.
$d = deflate_init(ZLIB_ENCODING_RAW, ['level' => 0]);
$o = '';
foreach (str_split($data, 5000) as $k => $p) $o .= deflate_add($d, $p, $k % 3 ? ZLIB_NO_FLUSH : ZLIB_SYNC_FLUSH);
$o .= deflate_add($d, '', ZLIB_FINISH);
echo md5($o), " ", strlen($o), "\n";
$d = deflate_init(ZLIB_ENCODING_RAW, ['level' => 0]);
echo md5(deflate_add($d, $data . $data, ZLIB_FINISH)), "\n";

foreach ([['level' => 10], ['level' => -2], ['memory' => 0], ['memory' => 10], ['window' => 7], ['window' => 16], ['strategy' => 9], ['strategy' => -1]] as $opt) {
    try { deflate_init(ZLIB_ENCODING_RAW, $opt); echo "ok\n"; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
try { deflate_init(99); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { deflate_init(99, ['level' => 99]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_init(99); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_init(99, ['window' => 99]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_init(ZLIB_ENCODING_RAW, ['window' => 3]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(deflate_init(ZLIB_ENCODING_RAW, ['window' => 8]));
var_dump(get_class(deflate_init(ZLIB_ENCODING_RAW, ['level' => 'abc', 'bogus' => 1])));
var_dump(get_class(inflate_init(ZLIB_ENCODING_DEFLATE, ['level' => 88, 'memory' => 99, 'strategy' => 55])));
foreach ([["a\0b"], [""], 5, ["a", ""]] as $dict) {
    try { deflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => $dict]); echo "ok\n"; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    try { inflate_init(ZLIB_ENCODING_DEFLATE, ['dictionary' => $dict]); echo "ok\n"; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
try { deflate_init(15, 5); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$d = deflate_init(ZLIB_ENCODING_DEFLATE);
try { deflate_add($d, "x", 99); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$i = inflate_init(ZLIB_ENCODING_DEFLATE);
try { inflate_add($i, "x", 99); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
