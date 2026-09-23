<?php
// Read chains: the underlying stream is pulled a chunk (8192 bytes) at a
// time, the filter's output is what the read functions see, and the chain
// gets its closing call when the stream runs dry.

class counter extends php_user_filter {
    public static array $calls = [];
    public function filter($in, $out, &$consumed, $closing): int {
        $n = 0;
        while ($b = stream_bucket_make_writeable($in)) {
            $n += $b->datalen;
            $b->data = strtoupper($b->data);
            stream_bucket_append($out, $b);
        }
        self::$calls[] = $n . ($closing ? '/closing' : '');
        return PSFS_PASS_ON;
    }
}
stream_filter_register('counter', 'counter');

function calls(): string {
    $c = implode(' ', counter::$calls);
    counter::$calls = [];
    return $c;
}

$fn = sys_get_temp_dir() . '/read-chains.txt';
$lines = '';
for ($i = 0; $i < 2000; $i++) {
    $lines .= "line $i\n";
}
file_put_contents($fn, $lines);

echo "--- fgets over several chunks ---\n";
$h = fopen($fn, 'r');
stream_filter_append($h, 'counter');
$count = 0;
$last = '';
while (($line = fgets($h)) !== false) {
    $count++;
    $last = $line;
}
var_dump($count, $last, feof($h));
echo calls(), "\n";
fclose($h);

echo "--- fread sizes ---\n";
$h = fopen($fn, 'r');
stream_filter_append($h, 'counter');
var_dump(strlen(fread($h, 10)));
echo calls(), "\n";
var_dump(strlen(fread($h, 10000)));
echo calls(), "\n";
var_dump(ftell($h));
var_dump(strlen(stream_get_contents($h)));
echo calls(), "\n";
var_dump(feof($h), ftell($h));
fclose($h);

echo "--- stream_get_line and fgetc ---\n";
$m = fopen('php://memory', 'w+');
fwrite($m, "a,b;;c,d");
rewind($m);
stream_filter_append($m, 'string.toupper', STREAM_FILTER_READ);
var_dump(stream_get_line($m, 100, ';;'));
var_dump(fgetc($m));
var_dump(stream_get_line($m, 100, ','));
var_dump(stream_get_line($m, 100, ','));
var_dump(stream_get_line($m, 100, ','));
fclose($m);

echo "--- a seek starts the chain over at the new place ---\n";
$m = fopen('php://memory', 'w+');
fwrite($m, "abcdefghij");
rewind($m);
stream_filter_append($m, 'counter', STREAM_FILTER_READ);
var_dump(fread($m, 4));
var_dump(fseek($m, 6));
var_dump(fread($m, 100));
var_dump(ftell($m));
rewind($m);
var_dump(fread($m, 3));
echo calls(), "\n";
fclose($m);

echo "--- the rest of a partly read file ---\n";
$h = fopen($fn, 'r');
fgets($h);
stream_filter_append($h, 'string.rot13', STREAM_FILTER_READ);
var_dump(fgets($h));
fclose($h);

echo "--- stream_copy_to_stream through both chains ---\n";
$src = fopen('php://memory', 'w+');
fwrite($src, "copy me");
rewind($src);
stream_filter_append($src, 'string.toupper', STREAM_FILTER_READ);
$dst = fopen('php://memory', 'w+');
stream_filter_append($dst, 'string.rot13', STREAM_FILTER_WRITE);
var_dump(stream_copy_to_stream($src, $dst));
rewind($dst);
var_dump(stream_get_contents($dst));

echo "--- removing a read filter mid-stream ---\n";
$m = fopen('php://memory', 'w+');
fwrite($m, str_repeat('x', 10));
rewind($m);
$f = stream_filter_append($m, 'string.toupper', STREAM_FILTER_READ);
var_dump(fread($m, 4));
var_dump(stream_filter_remove($f));
var_dump(fread($m, 100));
fclose($m);

echo "--- a descriptor ---\n";
$p = proc_open(['printf', 'from a pipe'], [1 => ['pipe', 'w']], $pipes);
stream_filter_append($pipes[1], 'counter');
var_dump(stream_get_contents($pipes[1]));
echo calls(), "\n";
fclose($pipes[1]);
proc_close($p);

echo "--- output streams ---\n";
$o = fopen('php://output', 'w');
stream_filter_append($o, 'string.toupper');
fwrite($o, "to php://output\n");
fclose($o);
unlink($fn);
