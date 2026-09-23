<?php
// gzwrite() through zlib's 8K buffers: small writes gather, large ones go
// straight through, seeks become zeros, flushes cut blocks. Level 0's
// stored blocks show every one of those boundaries.
mt_srand(5);
$chunks = [];
foreach ([1, 100, 8191, 8192, 8193, 3, 20000, 70000, 5, 16384, 40000] as $n) {
    $s = '';
    for ($i = 0; $i < $n; $i++) $s .= chr(mt_rand(97, 122));
    $chunks[] = $s;
}
foreach (["w0", "w1", "w", "w9", "wh", "wR"] as $mode) {
    foreach ([false, true] as $flushes) {
        $h = gzopen("zlib-gzw.gz", $mode);
        foreach ($chunks as $k => $c) {
            gzwrite($h, $c);
            if ($flushes && $k % 3 == 1) fflush($h);
            if ($k == 4) gzseek($h, 300, SEEK_CUR);
        }
        gzclose($h);
        $raw = file_get_contents("zlib-gzw.gz");
        echo $mode, $flushes ? " flushed " : " ", strlen($raw), " ", md5($raw), "\n";
    }
}
var_dump(strlen(gzdecode($raw)), gzdecode($raw) === implode('', array_slice($chunks, 0, 5)) . str_repeat("\0", 300) . implode('', array_slice($chunks, 5)));
unlink("zlib-gzw.gz");
