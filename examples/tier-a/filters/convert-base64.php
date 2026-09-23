<?php
// convert.base64-encode / -decode as stream filters: the answer must not
// depend on where the writes (or reads) cut the data, the encoder's line
// options, and php's forgiving decoder.

function through(string $filter, $params, array $pieces): string {
    $m = fopen('php://memory', 'w+');
    $f = $params === null
        ? stream_filter_append($m, $filter, STREAM_FILTER_WRITE)
        : stream_filter_append($m, $filter, STREAM_FILTER_WRITE, $params);
    if ($f === false) {
        return '(not attached)';
    }
    foreach ($pieces as $p) {
        $n = fwrite($m, $p);
        if ($n !== strlen($p)) {
            echo "  fwrite: ", var_export($n, true), "\n";
        }
    }
    // The closing flush writes the last quantum; read the buffer after it.
    fflush($m);
    stream_filter_remove($f);
    rewind($m);
    return stream_get_contents($m);
}

echo "--- encode, whole and in pieces ---\n";
$data = "The quick brown fox jumps over the lazy dog";
$whole = through('convert.base64-encode', null, [$data]);
var_dump($whole, $whole === base64_encode($data));
foreach ([1, 2, 4, 5, 7] as $n) {
    var_dump(through('convert.base64-encode', null, str_split($data, $n)) === $whole);
}
var_dump(through('convert.base64-encode', null, ['a', '', 'bc', 'defg', 'h']));
var_dump(through('convert.base64-encode', null, ["\x00\xff\x10"]));

echo "--- line-length and line-break-chars ---\n";
var_dump(through('convert.base64-encode', ['line-length' => 8, 'line-break-chars' => "\n"], [str_repeat('x', 20)]));
var_dump(through('convert.base64-encode', ['line-length' => 8], [str_repeat('x', 20)]));
var_dump(through('convert.base64-encode', ['line-length' => 7, 'line-break-chars' => '|'], ['xx', 'xxxxxxx', 'x', 'xxxxxxxxx']));
var_dump(through('convert.base64-encode', ['line-length' => 3, 'line-break-chars' => "\n"], ['abcdefgh']));
var_dump(through('convert.base64-encode', ['line-length' => '5', 'line-break-chars' => "\n"], ['abcdefgh']));
var_dump(through('convert.base64-encode', ['line-length' => -3, 'line-break-chars' => "\n"], ['abcdefgh']));
var_dump(through('convert.base64-encode', ['line-length' => 4, 'line-break-chars' => ''], ['abcdefgh']));
var_dump(through('convert.base64-encode', ['line-length' => 4, 'line-break-chars' => 7], ['abcdefgh']));
var_dump(through('convert.base64-encode', ['line-break-chars' => "\n"], [str_repeat('x', 60)]));
var_dump(through('convert.base64-encode', ['line-length' => 4, 'line-break-chars' => ['x']], ['abcdefgh']));
var_dump(through('convert.BASE64-encode', [], ['name case']));

echo "--- decode ---\n";
foreach ([
    ['YWJj', 'ZGVm', 'Zw', '=='],
    ["YW\nJj ZG\r\nVm"],
    ['YW*Jj'],
    ['YWJ'],
    ['YQ', '=', "\n", '='],
    ['Y'],
    ['YWJjZA'],
] as $pieces) {
    echo json_encode($pieces), " => ";
    var_dump(through('convert.base64-decode', null, $pieces));
}
$encoded = base64_encode(str_repeat("\x01\x80\xfe", 11));
foreach ([1, 3, 5] as $n) {
    var_dump(through('convert.base64-decode', null, str_split($encoded, $n)) === base64_decode($encoded));
}

echo "--- invalid input ---\n";
foreach (['YQ==YWJj', 'YWJj=', '=YWJj', 'Y===', 'YWJ=x'] as $bad) {
    echo "$bad => ";
    var_dump(through('convert.base64-decode', null, [$bad]));
}
echo "a good write, then a bad one:\n";
var_dump(through('convert.base64-decode', null, ['YWJj', '=', 'ZGVm']));

echo "--- parameters that are not an array ---\n";
$m = fopen('php://memory', 'w+');
var_dump(stream_filter_append($m, 'convert.base64-encode', STREAM_FILTER_WRITE, 'str'));
var_dump(stream_filter_append($m, 'convert.base64-encode', STREAM_FILTER_WRITE, null));
fclose($m);

echo "--- on the way out ---\n";
$m = fopen('php://temp', 'w+');
fwrite($m, str_repeat("0123456789", 1000));
rewind($m);
stream_filter_append($m, 'convert.base64-encode', STREAM_FILTER_READ);
$out = stream_get_contents($m);
var_dump(strlen($out), $out === base64_encode(str_repeat("0123456789", 1000)));
fclose($m);
