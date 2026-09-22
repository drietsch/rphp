<?php
function my_notifier(int $code, int $severity, ?string $message, int $messageCode, int $transferred, int $max): void {}
$c = stream_context_create(['http' => ['method' => 'POST', 'header' => "X: 1\r\n"], 'ssl' => ['verify_peer' => false]], ['notification' => 'my_notifier']);
var_dump(get_resource_type($c), is_resource($c));
var_dump(stream_context_get_options($c));
var_dump(stream_context_set_option($c, 'http', 'timeout', 5));
var_dump(stream_context_set_option($c, ['ftp' => ['overwrite' => true]]));
var_dump(stream_context_get_options($c)['http']['timeout'], stream_context_get_options($c)['ftp']);
var_dump(stream_context_get_params($c));
$d = stream_context_get_default();
var_dump(get_resource_type($d), stream_context_get_options($d));
$e = stream_context_create();
var_dump(stream_context_get_options($e));
// a context passed to a file function is accepted and ignored for a local path
$tmp = tempnam(sys_get_temp_dir(), 'ctx');
file_put_contents($tmp, "x\n");
var_dump(file_get_contents($tmp, false, $c));
$h = fopen($tmp, 'r', false, $c);
var_dump(fgets($h));
fclose($h);
var_dump(stream_context_get_options(fopen($tmp, 'r')));
unlink($tmp);
