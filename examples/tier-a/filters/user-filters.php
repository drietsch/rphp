<?php
// User stream filters: a php_user_filter subclass registered by name, its
// lifecycle (onCreate, filter on every write and read, onClose), what it is
// told ($closing, $consumed, $params, $this->stream) and what fwrite answers.

class tracer extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        $seen = [];
        while ($bucket = stream_bucket_make_writeable($in)) {
            $seen[] = $bucket->data;
            $consumed += $bucket->datalen;
            $bucket->data = strtoupper($bucket->data);
            stream_bucket_append($out, $bucket);
        }
        printf("  filter(%s) %s consumed=%s closing=%s stream=%s\n",
            $this->filtername, json_encode($seen), var_export($consumed, true),
            var_export($closing, true), get_debug_type($this->stream));
        if ($closing) {
            stream_bucket_append($out, stream_bucket_new($this->stream, "<end>"));
        }
        return PSFS_PASS_ON;
    }
    public function onCreate(): bool {
        echo "  onCreate(", $this->filtername, ") params=", json_encode($this->params), "\n";
        return true;
    }
    public function onClose(): void {
        echo "  onClose(", $this->filtername, ")\n";
    }
}

echo "--- register ---\n";
var_dump(stream_filter_register('trace.*', 'tracer'));
var_dump(stream_filter_register('trace.*', 'tracer'));
var_dump(stream_filter_register('string.rot13', 'tracer'));
var_dump(stream_filter_register('convert.*', 'tracer'));
$all = stream_get_filters();
var_dump(end($all), in_array('dechunk', $all), in_array('convert.iconv.*', $all));

echo "--- writes, a flush, then close ---\n";
$fn = sys_get_temp_dir() . '/user-filters.txt';
$h = fopen($fn, 'w');
$f = stream_filter_append($h, 'trace.write', STREAM_FILTER_WRITE, ['level' => 3]);
var_dump(get_resource_type($f));
var_dump(fwrite($h, "hello "));
var_dump(fwrite($h, "world"));
var_dump(fwrite($h, ""));
var_dump(fflush($h));
var_dump(fclose($h));
var_dump(get_resource_type($f));
var_dump(file_get_contents($fn));

echo "--- reads, to the end ---\n";
file_put_contents($fn, "line one\nline two\n");
$h = fopen($fn, 'r');
stream_filter_append($h, 'trace.read');
var_dump(fgets($h));
var_dump(feof($h));
var_dump(fgets($h));
var_dump(fgets($h));
var_dump(feof($h));
var_dump(fgets($h));
fclose($h);

echo "--- fread asks for what it needs ---\n";
file_put_contents($fn, "abcdef");
$h = fopen($fn, 'r');
stream_filter_append($h, 'trace.fread');
var_dump(fread($h, 3));
var_dump(fread($h, 100));
var_dump(fread($h, 100));
var_dump(feof($h));
fclose($h);

echo "--- both directions on a memory stream ---\n";
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'trace.both');
fwrite($m, "xy");
rewind($m);
var_dump(stream_get_contents($m));
fclose($m);

echo "--- removing flushes the filter first ---\n";
$m = fopen('php://memory', 'w+');
$f = stream_filter_append($m, 'trace.rm', STREAM_FILTER_WRITE);
fwrite($m, "abc");
var_dump(stream_filter_remove($f));
fwrite($m, "def");
rewind($m);
var_dump(stream_get_contents($m));
fclose($m);

echo "--- a filter that leaves \$consumed alone ---\n";
class passthru extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        while ($b = stream_bucket_make_writeable($in)) {
            stream_bucket_append($out, $b);
        }
        return PSFS_PASS_ON;
    }
}
stream_filter_register('passthru', 'passthru');
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'passthru', STREAM_FILTER_WRITE);
var_dump(fwrite($m, "12345"));
var_dump(fputs($m, "678"));
rewind($m);
var_dump(stream_get_contents($m));
fclose($m);

echo "--- a native filter ahead of it answers for the write ---\n";
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'string.toupper', STREAM_FILTER_WRITE);
stream_filter_append($m, 'passthru', STREAM_FILTER_WRITE);
var_dump(fwrite($m, "abc"));
rewind($m);
var_dump(stream_get_contents($m));
fclose($m);

echo "--- prepend runs first ---\n";
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'trace.second', STREAM_FILTER_WRITE);
stream_filter_prepend($m, 'string.rot13', STREAM_FILTER_WRITE);
fwrite($m, "abc");
rewind($m);
var_dump(stream_get_contents($m));
fclose($m);
unlink($fn);
