<?php
// `fgetcsv()` and `fputcsv()`. A CSV record is not a line: a newline inside an
// enclosure belongs to the field, so the reader keeps taking lines until the
// enclosures balance. An empty line is one `null` field, and only a read that
// found nothing at all is `false`.
$dir = sys_get_temp_dir() . '/rphp-fs-csv';
@mkdir($dir);
$p = $dir . '/in.csv';
file_put_contents($p, "a,b,c\n\"x,1\",\"y\ny2\",z\n\n,,\n\"a\"\"b\",plain\n");

$h = fopen($p, 'rb');
while (($row = fgetcsv($h, 0, ',', '"', '\\')) !== false) {
    var_dump($row);
}
var_dump(feof($h), fgetcsv($h, 0, ',', '"', '\\'));
fclose($h);

// An enclosure the file never closes keeps the line ending.
file_put_contents($dir . '/open.csv', "\"abc\n");
$h = fopen($dir . '/open.csv', 'rb');
var_dump(fgetcsv($h, 0, ',', '"', '\\'));
var_dump(fgetcsv($h, 0, ',', '"', '\\'));
fclose($h);

// Leaving `$escape` out is deprecated, because its default is going away.
$h = fopen($p, 'rb');
var_dump(fgetcsv($h));
fclose($h);

// The separators are single characters, and the escape may also be empty.
$h = fopen($p, 'rb');
foreach ([[',,', '"', '\\'], [',', '""', '\\'], [',', '"', '\\\\']] as $bad) {
    try {
        fgetcsv($h, 0, $bad[0], $bad[1], $bad[2]);
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
fclose($h);

// A handle that cannot be read reports php's notice and answers `false`.
$wo = fopen($dir . '/w.csv', 'wb');
var_dump(@fgetcsv($wo, 0, ',', '"', '\\'), error_get_last()['message']);
fclose($wo);

// `fputcsv()` encloses a field holding the separator, the enclosure, the
// escape, a newline, a carriage return, a tab or a space — and nothing else.
$out = fopen($dir . '/out.csv', 'wb');
var_dump(fputcsv($out, ['a', 'b', 'c'], ',', '"', '\\'));
var_dump(fputcsv($out, ['x,1', 'y"q', "m\nn"], ',', '"', '\\'));
var_dump(fputcsv($out, [], ',', '"', '\\'));
var_dump(fputcsv($out, [1, 2.5, null, true, false], ',', '"', '\\'));
foreach ([['a b'], ["a\tb"], ["a\rb"], ['a\\b'], ['plain'], [''], ['a,b']] as $row) {
    fputcsv($out, $row, ',', '"', '\\');
}
// The escape suppresses the doubling of the enclosure that follows it.
var_dump(fputcsv($out, ['a\\"b'], ',', '"', '\\'));
var_dump(fputcsv($out, ['a\\"b'], ',', '"', ''));
// `$eol` is whatever the caller asks for.
var_dump(fputcsv($out, ['a', 'b'], ',', '"', '\\', "\r\n"));
fclose($out);
echo bin2hex(file_get_contents($dir . '/out.csv')), "\n";
echo file_get_contents($dir . '/out.csv');

// Round trip.
$back = fopen($dir . '/out.csv', 'rb');
var_dump(fgetcsv($back, 0, ',', '"', '\\'));
var_dump(fgetcsv($back, 0, ',', '"', '\\'));
fclose($back);

$out = fopen($dir . '/out2.csv', 'wb');
var_dump(fputcsv($out, ['a']));
try {
    fputcsv($out, ['a'], ',,', '"', '\\');
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
fclose($out);

$ro = fopen($p, 'rb');
var_dump(@fputcsv($ro, ['a'], ',', '"', '\\'), error_get_last()['message']);
fclose($ro);

foreach (['in.csv', 'open.csv', 'w.csv', 'out.csv', 'out2.csv'] as $f) {
    unlink($dir . '/' . $f);
}
var_dump(rmdir($dir));
