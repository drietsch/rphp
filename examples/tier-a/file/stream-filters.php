<?php
// Stream filters: the chain a stream's bytes pass through on the way in or
// out. `dechunk` is the one Symfony's NativeHttpClient attaches, and the
// one whose state has to survive a write landing mid-chunk.

echo "--- dechunk on write ---\n";
$b = fopen('php://temp', 'w+');
$f = stream_filter_append($b, 'dechunk', STREAM_FILTER_WRITE);
var_dump(get_resource_type($f));
// fwrite answers the bytes it was *given*, not the bytes stored.
var_dump(fwrite($b, "5\r\nHello\r\n6\r\n world\r\n0\r\n\r\n"));
rewind($b);
var_dump(stream_get_contents($b));
fclose($b);

echo "--- and across write boundaries ---\n";
// Three bytes at a time cuts every size line and every chunk in half.
$b = fopen('php://temp', 'w+');
stream_filter_append($b, 'dechunk', STREAM_FILTER_WRITE);
foreach (str_split("5\r\nHello\r\n6\r\n world\r\n0\r\n\r\n", 3) as $piece) {
    fwrite($b, $piece);
}
rewind($b);
var_dump(stream_get_contents($b));
fclose($b);

echo "--- chunk extensions and trailers ---\n";
$b = fopen('php://temp', 'w+');
stream_filter_append($b, 'dechunk', STREAM_FILTER_WRITE);
fwrite($b, "5;name=value\r\nHello\r\n0\r\nX-Checksum: abc\r\n\r\n");
rewind($b);
var_dump(stream_get_contents($b));
fclose($b);

echo "--- the string filters, on the way in ---\n";
foreach (['string.toupper', 'string.tolower', 'string.rot13'] as $name) {
    $b = fopen('php://temp', 'w+');
    stream_filter_append($b, $name, STREAM_FILTER_WRITE);
    fwrite($b, 'Hello World');
    rewind($b);
    printf("%-15s %s\n", $name, stream_get_contents($b));
    fclose($b);
}

echo "--- and on the way out ---\n";
$b = fopen('php://temp', 'w+');
fwrite($b, 'abc');
rewind($b);
stream_filter_append($b, 'string.rot13', STREAM_FILTER_READ);
var_dump(stream_get_contents($b));
fclose($b);

echo "--- prepend puts one first ---\n";
// rot13 then toupper is not the same as toupper then rot13 for the case,
// but both orders are applied in the order the chain holds them.
$b = fopen('php://temp', 'w+');
stream_filter_append($b, 'string.toupper', STREAM_FILTER_WRITE);
stream_filter_prepend($b, 'string.rot13', STREAM_FILTER_WRITE);
fwrite($b, 'abc');
rewind($b);
var_dump(stream_get_contents($b));
fclose($b);

echo "--- removing one stops it ---\n";
$b = fopen('php://temp', 'w+');
$f = stream_filter_append($b, 'string.toupper', STREAM_FILTER_WRITE);
fwrite($b, 'aa');
var_dump(stream_filter_remove($f));
fwrite($b, 'bb');
rewind($b);
var_dump(stream_get_contents($b));
fclose($b);

echo "--- a filter nobody registered ---\n";
$b = fopen('php://temp', 'w+');
var_dump(stream_filter_append($b, 'no.such.filter', STREAM_FILTER_WRITE));
fclose($b);
