<?php
// zlib.deflate / zlib.inflate as stream filters: the bytes they produce,
// their parameters, and the round trip through a file.
$f = fopen('php://memory', 'w+');
stream_filter_append($f, 'zlib.deflate', STREAM_FILTER_WRITE, 6);
fwrite($f, 'hello world hello world hello world');
fclose($f);

$data = str_repeat("The quick brown fox jumps over the lazy dog. ", 400);
foreach ([null, -1, 1, 9, ['level' => 3, 'window' => 15], ['level' => 9, 'window' => 31, 'memory' => 4]] as $p) {
    $m = fopen('php://temp', 'w+');
    $filter = $p === null
        ? stream_filter_append($m, 'zlib.deflate', STREAM_FILTER_WRITE)
        : stream_filter_append($m, 'zlib.deflate', STREAM_FILTER_WRITE, $p);
    foreach (str_split($data, 777) as $chunk) {
        fwrite($m, $chunk);
    }
    stream_filter_remove($filter);
    rewind($m);
    $z = stream_get_contents($m);
    echo json_encode($p), ': ', strlen($z), ' ', md5($z), "\n";
}

// Flushing mid-stream is a sync flush.
$m = fopen('php://temp', 'w+');
$filter = stream_filter_append($m, 'zlib.deflate', STREAM_FILTER_WRITE);
fwrite($m, 'part one ');
fflush($m);
echo bin2hex(stream_get_contents($m, -1, 0)), "\n";
fwrite($m, 'part two');
stream_filter_remove($filter);
echo bin2hex(stream_get_contents($m, -1, 0)), "\n";

// Round trip through a file, inflating on read.
$path = sys_get_temp_dir() . '/zlib-filter-' . getmypid() . '.bin';
$w = fopen($path, 'w');
stream_filter_append($w, 'zlib.deflate', STREAM_FILTER_WRITE, ['window' => 31]);
fwrite($w, $data);
fclose($w);
var_dump(gzdecode(file_get_contents($path)) === $data);
$r = fopen($path, 'r');
stream_filter_append($r, 'zlib.inflate', STREAM_FILTER_READ, ['window' => 47]);
var_dump(stream_get_contents($r) === $data);
fclose($r);
var_dump(file_get_contents("php://filter/read=zlib.inflate/resource=$path") === false);
var_dump(file_get_contents("php://filter/read=zlib.deflate|zlib.inflate/resource=$path") === file_get_contents($path));
unlink($path);

// Parameters php refuses or ignores.
$f = fopen('php://memory', 'w+');
var_dump(stream_filter_append($f, 'zlib.inflate', STREAM_FILTER_WRITE, ['window' => 3]));
var_dump(is_resource(stream_filter_append($f, 'zlib.deflate', STREAM_FILTER_WRITE, ['level' => 42, 'memory' => 0])));
var_dump(is_resource(stream_filter_append($f, 'zlib.deflate', STREAM_FILTER_WRITE, null)));
var_dump(stream_filter_append($f, 'zlib.nope'));
var_dump(in_array('zlib.*', stream_get_filters(), true));

// Garbage through inflate is zlib's data error, as a notice.
$g = fopen('php://memory', 'w+');
stream_filter_append($g, 'zlib.inflate', STREAM_FILTER_WRITE);
var_dump(fwrite($g, 'garbage data here'));
fclose($g);
