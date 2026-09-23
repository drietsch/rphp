<?php
// php://filter/read=<list>/write=<list>/resource=<url>: a filter chain in
// a url, for every function that takes one.

$fn = sys_get_temp_dir() . '/php-filter-wrapper.txt';
file_put_contents($fn, "Hello World\n");

echo "--- reading ---\n";
var_dump(file_get_contents("php://filter/read=convert.base64-encode/resource=$fn"));
var_dump(file_get_contents("php://filter/convert.base64-encode|string.rot13/resource=$fn"));
var_dump(file_get_contents("php://filter/read=string.toupper/read=string.rot13/resource=$fn"));
var_dump(file_get_contents("php://filter/read=string.toupper/write=string.rot13/resource=$fn"));
var_dump(file_get_contents("php://filter/read=convert.iconv.utf-8%2Futf-16le/resource=$fn") === iconv('UTF-8', 'UTF-16LE', "Hello World\n"));
var_dump(file_get_contents("PHP://Filter/read=string.toupper/resource=$fn"));
var_dump(file("php://filter/read=string.toupper/resource=$fn", FILE_IGNORE_NEW_LINES));
var_dump(readfile("php://filter/read=string.rot13/resource=$fn"));

echo "--- through fopen ---\n";
$h = fopen("php://filter/read=string.toupper/resource=$fn", 'r');
var_dump(fgets($h));
$meta = stream_get_meta_data($h);
var_dump($meta['wrapper_type'], $meta['stream_type'], $meta['mode']);
var_dump(str_replace($fn, '<file>', $meta['uri']));
fclose($h);
$h = fopen('php://filter/write=string.toupper/resource=php://memory', 'w+');
fwrite($h, "into memory");
rewind($h);
var_dump(stream_get_contents($h));
fclose($h);

echo "--- writing ---\n";
var_dump(file_put_contents("php://filter/write=string.rot13/resource=$fn", "abc"));
var_dump(file_get_contents($fn));
var_dump(file_put_contents("php://filter/write=convert.base64-encode/resource=$fn", "abcd"));
var_dump(file_get_contents($fn));
var_dump(file_put_contents("php://filter/write=string.toupper/resource=$fn", "more", FILE_APPEND));
var_dump(file_put_contents("php://filter/write=string.toupper/resource=$fn", ["x", "y"], FILE_APPEND));
var_dump(file_get_contents($fn));

echo "--- a user filter in the url ---\n";
class wrap_brackets extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        while ($b = stream_bucket_make_writeable($in)) {
            $consumed += $b->datalen;
            $b->data = "[" . $b->data . "]";
            stream_bucket_append($out, $b);
        }
        return PSFS_PASS_ON;
    }
}
stream_filter_register('brackets.*', 'wrap_brackets');
var_dump(file_get_contents("php://filter/read=brackets.any/resource=$fn"));
var_dump(file_put_contents("php://filter/write=brackets.any/resource=$fn", "abc"));
var_dump(file_get_contents($fn));

echo "--- a filter that swallows the count ---\n";
class no_count extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        while ($b = stream_bucket_make_writeable($in)) {
            stream_bucket_append($out, $b);
        }
        return PSFS_PASS_ON;
    }
}
stream_filter_register('no_count', 'no_count');
var_dump(file_put_contents("php://filter/write=no_count/resource=$fn", "abc"));
var_dump(file_put_contents("php://filter/write=no_count/resource=$fn", ["abc", "def"]));
var_dump(file_get_contents($fn));

echo "--- what goes wrong ---\n";
var_dump(file_get_contents("php://filter/read=no.such/resource=$fn"));
var_dump(file_get_contents("php://filter/read=string.toupper/resource=/no/such/file"));
var_dump(file_get_contents("php://filter//resource=$fn"));
var_dump(file_get_contents("php://filter/read=/resource=$fn"));
try {
    file_get_contents("php://filter/read=string.toupper");
} catch (\Error $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
unlink($fn);
