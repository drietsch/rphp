<?php
// A seeded sweep over deflate_add(): random settings, data shapes, chunk
// sizes and flush modes, each stream's bytes hashed — the compressor has
// to be zlib's own for every one of them to agree.
mt_srand(2026);
function shape(int $kind, int $len): string {
    $s = '';
    switch ($kind) {
        case 0: for ($i = 0; $i < $len; $i++) $s .= chr(mt_rand(0, 255)); break;
        case 1: for ($i = 0; $i < $len; $i++) $s .= chr(mt_rand(97, 100)); break;
        case 2: while (strlen($s) < $len) $s .= str_repeat(chr(mt_rand(65, 70)), mt_rand(1, 300)); break;
        default:
            $words = ['alpha', 'beta', 'gamma', 'delta', "\n", ' ', 'epsilon', '0123456789'];
            while (strlen($s) < $len) $s .= $words[mt_rand(0, 7)];
    }
    return substr($s, 0, $len);
}
$flushes = [ZLIB_NO_FLUSH, ZLIB_NO_FLUSH, ZLIB_NO_FLUSH, ZLIB_PARTIAL_FLUSH, ZLIB_SYNC_FLUSH, ZLIB_FULL_FLUSH, ZLIB_BLOCK];
$all = '';
for ($case = 0; $case < 160; $case++) {
    $enc = [ZLIB_ENCODING_RAW, ZLIB_ENCODING_DEFLATE, ZLIB_ENCODING_GZIP][mt_rand(0, 2)];
    $opt = [
        'level' => mt_rand(-1, 9),
        'memory' => mt_rand(1, 9),
        'window' => mt_rand($enc == ZLIB_ENCODING_DEFLATE ? 8 : 9, 15),
        'strategy' => mt_rand(0, 4),
    ];
    $d = deflate_init($enc, $opt);
    $data = shape(mt_rand(0, 3), mt_rand(0, 3) ? mt_rand(0, 5000) : mt_rand(20000, 150000));
    $out = '';
    $pos = 0;
    while ($pos < strlen($data)) {
        $n = mt_rand(0, 3) ? mt_rand(1, 3000) : mt_rand(3000, 70000);
        $out .= deflate_add($d, substr($data, $pos, $n), $flushes[mt_rand(0, 6)]);
        $pos += $n;
    }
    $out .= deflate_add($d, '', ZLIB_FINISH);
    $all .= md5($out);
    if ($case % 20 == 0) echo $case, " ", json_encode($opt), " ", strlen($data), " ", md5($out), "\n";
}
echo md5($all), "\n";
