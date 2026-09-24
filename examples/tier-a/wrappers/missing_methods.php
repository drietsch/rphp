<?php
// What php says when a wrapper class lacks the method an operation needs.
class OnlyOpen { public $context; function stream_open($p, $m, $o, &$op) { return true; } }
class Empty_ { public $context; }
class Refuses { public $context; function stream_open($p, $m, $o, &$op) { return false; } function dir_opendir($p, $o) { return false; } }
stream_wrapper_register('oo', 'OnlyOpen');
stream_wrapper_register('ee', 'Empty_');
stream_wrapper_register('rr', 'Refuses');

var_dump(fopen('ee://x', 'r'));
var_dump(file_get_contents('ee://x'));
var_dump(fopen('rr://x', 'r'));
var_dump(@fopen('rr://x', 'r'));
var_dump(file_exists('ee://x'), is_file('ee://x'));
var_dump(stat('ee://x'));
var_dump(filesize('ee://x'));
var_dump(unlink('ee://x'));
var_dump(rename('ee://x', 'ee://y'));
var_dump(rename('ee://x', 'oo://y'));
var_dump(mkdir('ee://x'));
var_dump(rmdir('ee://x'));
var_dump(opendir('ee://x'));
var_dump(opendir('rr://x'));
var_dump(touch('ee://x'));
var_dump(chmod('ee://x', 0644));
var_dump(chown('ee://x', 'root'));
var_dump(chgrp('ee://x', 0));

$f = fopen('oo://x', 'r+');
var_dump(get_resource_type($f));
var_dump(fread($f, 10));
var_dump(fgets($f));
var_dump(feof($f));
var_dump(fwrite($f, "abc"));
var_dump(ftell($f));
var_dump(fseek($f, 3));
var_dump(rewind($f));
var_dump(fflush($f));
var_dump(fstat($f));
var_dump(flock($f, LOCK_SH));
var_dump(ftruncate($f, 0));
var_dump(stream_set_blocking($f, false));
var_dump(stream_set_timeout($f, 5));
var_dump(stream_set_write_buffer($f, 0));
$r = [$f]; $w = null; $e = null;
try { var_dump(stream_select($r, $w, $e, 0)); } catch (\ValueError $ex) { echo $ex->getMessage(), "\n"; }
var_dump(fclose($f));
try { fclose($f); } catch (\TypeError $ex) { echo $ex->getMessage(), "\n"; }
