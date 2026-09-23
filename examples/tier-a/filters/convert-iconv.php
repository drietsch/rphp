<?php
// convert.iconv.<from>.<to> (or <from>/<to>): a charset conversion that
// keeps a character cut across two writes whole, and php's warnings.

function conv(string $filter, array $pieces): void {
    $m = fopen('php://memory', 'w+');
    if (!stream_filter_append($m, $filter, STREAM_FILTER_WRITE)) {
        fclose($m);
        return;
    }
    foreach ($pieces as $p) {
        $n = fwrite($m, $p);
        if ($n !== strlen($p)) {
            echo "  fwrite: ", var_export($n, true), "\n";
        }
    }
    rewind($m);
    echo "  ", bin2hex(stream_get_contents($m)), "\n";
    fclose($m);
}

echo "--- the two spellings ---\n";
conv('convert.iconv.utf-8.iso-8859-1', ["h\xc3\xa9llo"]);
conv('convert.iconv.ISO-8859-1/UTF-8', ["h\xe9llo"]);
conv('convert.iconv.UTF-8/UTF-16LE', ["ab\xe2\x82\xac"]);
conv('convert.iconv.utf-16le.utf-8', ["a\x00", "b", "\x00"]);

echo "--- a character cut across writes ---\n";
conv('convert.iconv.utf-8.iso-8859-1', ["h\xc3", "\xa9llo"]);
conv('convert.iconv.utf-8.utf-16be', ["\xe2", "\x82", "\xac!"]);
$text = "Grüße aus Köln — 東京";
$want = iconv('UTF-8', 'UTF-16LE', $text);
$ok = true;
for ($n = 1; $n <= 5; $n++) {
    $m = fopen('php://memory', 'w+');
    stream_filter_append($m, 'convert.iconv.UTF-8/UTF-16LE', STREAM_FILTER_WRITE);
    foreach (str_split($text, $n) as $p) {
        fwrite($m, $p);
    }
    rewind($m);
    $ok = $ok && stream_get_contents($m) === $want;
    fclose($m);
}
var_dump($ok);

echo "--- what cannot be converted ---\n";
conv('convert.iconv.utf-8.ascii', ["h\xc3\xa9llo"]);
conv('convert.iconv.utf-8.iso-8859-1', ["ab", "\xffcd", "ef"]);
echo "a character still waiting at close:\n";
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'convert.iconv.utf-8.iso-8859-1', STREAM_FILTER_WRITE);
fwrite($m, "h\xc3");
fclose($m);

echo "--- charsets and names php does not take ---\n";
conv('convert.iconv.utf-8.nope', ['x']);
conv('convert.iconv.utf-8', ['x']);
conv('convert.iconv.', ['x']);
conv('CONVERT.ICONV.UTF-8.ISO-8859-1', ['x']);

echo "--- on the way out ---\n";
$m = fopen('php://memory', 'w+');
fwrite($m, "caf\xe9 cr\xe8me");
rewind($m);
stream_filter_append($m, 'convert.iconv.ISO-8859-1.UTF-8', STREAM_FILTER_READ);
var_dump(stream_get_contents($m));
fclose($m);
