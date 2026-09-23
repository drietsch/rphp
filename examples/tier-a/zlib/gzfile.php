<?php
// gzopen and the gz handle functions: written bytes, reads, seeks, eof,
// gzfile/readgzfile, plain files read through, appended members, errors.
$f = "zlib-gzfile-a.gz";
$h = gzopen($f, "wb9");
var_dump($h, get_resource_type($h));
var_dump(gzwrite($h, "line one\nline two\n"), gzputs($h, "three", 3), gzwrite($h, ""), fwrite($h, "xyz\n"));
var_dump(gztell($h), ftell($h));
var_dump(gzclose($h));
echo bin2hex(file_get_contents($f)), "\n";

$h = gzopen($f, "r");
var_dump(stream_get_meta_data($h));
var_dump(gzgets($h), gzgetc($h), gzread($h, 4), gztell($h), gzeof($h));
var_dump(gzgets($h, 3));
var_dump(fread($h, 100), gzeof($h), feof($h), gzread($h, 10), gzgets($h), gzgetc($h));
var_dump(gzrewind($h), gztell($h), gzseek($h, 5), gzread($h, 3), gzseek($h, 2, SEEK_CUR), gzread($h, 3), gzseek($h, -1, SEEK_END));
var_dump(gzseek($h, 1000), gztell($h), gzread($h, 3), gzeof($h));
gzrewind($h);
var_dump(gzpassthru($h));
var_dump(gzclose($h));

// The idiomatic loop stops as soon as the last line is read.
$h = gzopen($f, "r");
$n = 0;
while (!gzeof($h)) { $line = gzgets($h); $n++; var_dump($line); }
var_dump($n);
gzclose($h);

var_dump(gzfile($f));
var_dump(readgzfile($f));

file_put_contents("zlib-gzfile-plain.txt", "not gzip\nat all\n");
var_dump(gzfile("zlib-gzfile-plain.txt"));
$h = gzopen("zlib-gzfile-plain.txt", "rb"); var_dump(gzread($h, 100)); gzclose($h);

var_dump(@gzopen("zlib-gzfile-nope.gz", "r"));
var_dump(gzopen("zlib-gzfile-nope.gz", "r"));
var_dump(gzfile("zlib-gzfile-nope.gz"));
var_dump(readgzfile("zlib-gzfile-nope.gz"));

// Seeking a write handle forward writes zeros; backwards fails.
$h = gzopen("zlib-gzfile-d.gz", "w");
var_dump(gzseek($h, 10), gzwrite($h, "x"), gztell($h), gzseek($h, 2), gzread($h, 1), gzgets($h), gzgetc($h), gzeof($h), gzpassthru($h));
var_dump(gzseek($h, 20), gzseek($h, 15), gztell($h), gzseek($h, 3, SEEK_CUR), gztell($h));
gzclose($h);
echo bin2hex(file_get_contents("zlib-gzfile-d.gz")), "\n";

// Level 1, then an appended member read back as one stream.
$h = gzopen("zlib-gzfile-e.gz", "w1"); gzwrite($h, str_repeat("abc", 1000)); gzclose($h);
echo md5(file_get_contents("zlib-gzfile-e.gz")), "\n";
$h = gzopen("zlib-gzfile-e.gz", "a"); gzwrite($h, "app"); gzclose($h);
var_dump(strlen(gzdecode(file_get_contents("zlib-gzfile-e.gz"))), gzfile("zlib-gzfile-e.gz")[0] === str_repeat("abc", 1000) . "app");

// fflush syncs; a second flush adds nothing; closing after a flush adds
// only the end of the member.
$h = gzopen("zlib-gzfile-f.gz", "w");
var_dump(fflush($h)); gzwrite($h, "abc"); fflush($h); gzwrite($h, "def"); fflush($h); fflush($h);
var_dump(stream_get_meta_data($h));
gzclose($h);
echo bin2hex(file_get_contents("zlib-gzfile-f.gz")), "\n";
$h = gzopen("zlib-gzfile-f.gz", "w"); gzclose($h);
echo bin2hex(file_get_contents("zlib-gzfile-f.gz")), "\n";
$h = gzopen("zlib-gzfile-f.gz", "r"); var_dump(gzwrite($h, "x"), fwrite($h, "y"), fflush($h), gzgets($h), gzeof($h)); gzclose($h);

// Modes: strategies, transparent writing, levels spelled oddly.
foreach (["w0", "w10", "wT", "wf", "wh", "wR", "wF", "ab", "w5f", "wb6h"] as $m) {
    @unlink("zlib-gzfile-m.gz");
    $h = gzopen("zlib-gzfile-m.gz", $m);
    var_dump(stream_get_meta_data($h)["mode"]);
    gzwrite($h, str_repeat("hello hello hello ", 20));
    gzclose($h);
    echo $m, " ", bin2hex(file_get_contents("zlib-gzfile-m.gz")), "\n";
}
foreach (["r+", "w+", "x", "", "rT", "c"] as $m) {
    var_dump(gzopen("zlib-gzfile-e.gz", $m));
}
@unlink("zlib-gzfile-x.gz");
var_dump(gzopen("zlib-gzfile-x.gz", "x"), file_exists("zlib-gzfile-x.gz"));
foreach (['gzread' => ["x", 1], 'gzclose' => ["x"], 'gzwrite' => [1, "a"], 'gztell' => [null], 'gzeof' => [[]], 'gzpassthru' => ["x"], 'gzseek' => ["x", 1], 'gzgets' => ["x"], 'gzgetc' => ["x"], 'gzrewind' => ["x"], 'gzputs' => ["x", "y"]] as $fn => $a) {
    try { $fn(...$a); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
$h = gzopen($f, "r");
try { gzgets($h, 0); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(gzgets($h, 1), gzgets($h, 2));
gzclose($h);
foreach (glob("zlib-gzfile-*") as $x) unlink($x);
