<?php
// `stream_get_meta_data()` names the implementation behind a handle, and
// `unread_bytes` is what is left of php's 8192-byte read buffer — so it is
// zero on a fresh handle and after a seek.
$dir = sys_get_temp_dir() . '/rphp-meta-test';
@mkdir($dir);
$p = $dir . '/m.txt';
file_put_contents($p, str_repeat('x', 20));

$f = fopen($p, 'r');
$m = stream_get_meta_data($f);
var_dump(array_keys($m));
var_dump($m['timed_out'], $m['blocked'], $m['eof'], $m['wrapper_type'], $m['stream_type'], $m['mode'], $m['unread_bytes'], $m['seekable'], $m['uri'] === $p);
var_dump(get_resource_type($f), stream_set_blocking($f, true), stream_isatty($f));
fread($f, 4);
var_dump(stream_get_meta_data($f)['unread_bytes']);
fseek($f, 0);
var_dump(stream_get_meta_data($f)['unread_bytes']);
fgetc($f);
var_dump(stream_get_meta_data($f)['unread_bytes']);
fclose($f);

$g = fopen($p, 'rb+');
var_dump(stream_get_meta_data($g)['mode'], stream_get_meta_data($g)['seekable']);
fclose($g);

// A memory stream is a buffer with no fd: binary mode, no read buffer.
$mem = fopen('php://memory', 'w+');
fwrite($mem, 'hello');
$mm = stream_get_meta_data($mem);
var_dump($mm['wrapper_type'], $mm['stream_type'], $mm['mode'], $mm['unread_bytes'], $mm['seekable'], $mm['uri']);
$tmp = fopen('php://temp', 'w+');
var_dump(stream_get_meta_data($tmp)['stream_type']);

// The output handles are not seekable.
$out = fopen('php://stdout', 'w');
$om = stream_get_meta_data($out);
var_dump($om['wrapper_type'], $om['stream_type'], $om['seekable'], $om['uri']);

// Copying moves the cursor of the source and returns the byte count.
rewind($mem);
$dst = fopen('php://memory', 'w+');
var_dump(stream_copy_to_stream($mem, $dst), ftell($mem));
rewind($dst);
var_dump(stream_get_contents($dst));
rewind($mem);
$part = fopen('php://memory', 'w+');
var_dump(stream_copy_to_stream($mem, $part, 3), ftell($mem));
fclose($mem);
fclose($dst);
fclose($part);
fclose($tmp);

var_dump(unlink($p), rmdir($dir));
