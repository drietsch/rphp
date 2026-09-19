<?php
// php://memory is a read/write buffer with a cursor.
$h = fopen('php://memory', 'r+');
var_dump(fwrite($h, "abc\ndef\n"));
rewind($h);
var_dump(fgets($h));
var_dump(ftell($h), feof($h));
var_dump(stream_get_contents($h));
var_dump(feof($h));
rewind($h);
var_dump(fread($h, 2), fgetc($h));
fseek($h, 0, SEEK_END);
var_dump(ftell($h));
fseek($h, 1, SEEK_SET);
var_dump(ftell($h));
var_dump(fclose($h));

// A real file round-trips through the same path; `a` appends.
$dir = sys_get_temp_dir() . '/rphp-streams-test';
@mkdir($dir);
$p = $dir . '/s.txt';
$f = fopen($p, 'w');
var_dump(fwrite($f, 'hello'));
fclose($f);
var_dump(file_get_contents($p), filesize($p));
$f = fopen($p, 'a');
fwrite($f, ' more');
fclose($f);
var_dump(file_get_contents($p));
$f = fopen($p, 'r');
var_dump(fgets($f), feof($f));
fclose($f);
// The mode is enforced on a real file: php reads and writes through the fd in
// 8192-byte chunks, so the wrong mode fails there and is reported as a notice
// — naming 8192 bytes whatever length was asked for.
$f = fopen($p, 'r');
var_dump(fwrite($f, 'x'), fwrite($f, ''));
fclose($f);
$f = fopen($p, 'w');
var_dump(fread($f, 5), fgets($f), fgetc($f), stream_get_contents($f), feof($f));
fclose($f);

// A memory stream has no fd behind it: it refuses the write without a
// diagnostic, and reading an empty buffer is simply the empty string.
$m = fopen('php://memory', 'r');
var_dump(fwrite($m, 'abc'), fread($m, 4));
fclose($m);

// `feof()` reports php's end-of-file *flag*, not "the cursor is at the end":
// a read that came back short sets it, an exact read does not, and a seek
// clears it again.
file_put_contents($p, "one\ntwo\n");
$f = fopen($p, 'r');
var_dump(feof($f), fgets($f), feof($f), fgets($f), feof($f), fgets($f), feof($f));
fseek($f, 0);
var_dump(feof($f), fread($f, 4), feof($f), fread($f, 99), feof($f));
rewind($f);
var_dump(feof($f), stream_get_contents($f, 3), feof($f), stream_get_contents($f), feof($f));
fclose($f);

var_dump(unlink($p), rmdir($dir));
