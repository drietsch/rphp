<?php
// The compress.zlib:// wrapper behind fopen and the whole-file functions.
$dir = getcwd();
$f = "$dir/zlib-wrap-a.gz";
var_dump(file_put_contents("compress.zlib://$f", "put data\nsecond line\n"));
echo bin2hex(file_get_contents($f)), "\n";
var_dump(file_get_contents("compress.zlib://$f"));
var_dump(file("compress.zlib://$f"));
var_dump(file("compress.zlib://$f", FILE_IGNORE_NEW_LINES));
var_dump(readfile("compress.zlib://$f"));

$h = fopen("compress.zlib://$f", "r");
var_dump(stream_get_meta_data($h));
var_dump(fgets($h), fread($h, 3), ftell($h), feof($h), stream_get_contents($h), feof($h));
fclose($h);

$h = fopen("compress.zlib://$dir/zlib-wrap-b.gz", "w");
var_dump(fwrite($h, "via fopen"), ftell($h));
var_dump(stream_get_meta_data($h)["wrapper_type"], stream_get_meta_data($h)["mode"]);
fclose($h);
echo bin2hex(file_get_contents("$dir/zlib-wrap-b.gz")), "\n";

file_put_contents("compress.zlib://$dir/zlib-wrap-c.gz", "a");
file_put_contents("compress.zlib://$dir/zlib-wrap-c.gz", "b", FILE_APPEND);
var_dump(bin2hex(file_get_contents("$dir/zlib-wrap-c.gz")), file_get_contents("compress.zlib://$dir/zlib-wrap-c.gz"));
var_dump(file_put_contents("compress.zlib://$dir/zlib-wrap-d.gz", ["a", "b"]));
var_dump(file_get_contents("compress.zlib://$dir/zlib-wrap-d.gz"));

// A plain file reads through unchanged.
file_put_contents("$dir/zlib-wrap-plain.txt", "plain\n");
var_dump(file_get_contents("compress.zlib://$dir/zlib-wrap-plain.txt"));

var_dump(file_get_contents("compress.zlib://$dir/nonexist.gz"));
var_dump(fopen("compress.zlib://$dir/nonexist.gz", "r"));
var_dump(file("compress.zlib://$dir/nonexist.gz"));
var_dump(file_put_contents("compress.zlib://$dir/nonexistdir/x.gz", "x"));
var_dump(fopen("compress.zlib://$f", "r+"));
var_dump(readfile("compress.zlib://$dir/nonexist.gz"));
var_dump(stream_is_local(gzopen($f, "r")));
foreach (glob("$dir/zlib-wrap-*") as $x) unlink($x);
