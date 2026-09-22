<?php
// A file written through a handle is visible to the stat functions at once:
// php's stat cache is invalidated by the write, even when an earlier call
// cached "does not exist". Symfony's Filesystem::copy does exactly this
// (is_file() before the copy, is_file() after it).
$dir = sys_get_temp_dir() . '/rphp-stat-' . getmypid();
@mkdir($dir);
$path = $dir . '/written.txt';
@unlink($path);
var_dump(is_file($path), file_exists($path));
$h = fopen($path, 'w');
fwrite($h, "one\n");
fflush($h);
var_dump(is_file($path), filesize($path));
fwrite($h, "two\n");
fclose($h);
var_dump(filesize($path), file_get_contents($path));
// unlink invalidates it the other way round
unlink($path);
var_dump(is_file($path), file_exists($path));
// and an append handle's writes are seen too
$h = fopen($path, 'a');
fwrite($h, "three\n");
fclose($h);
var_dump(file_get_contents($path));
unlink($path);
rmdir($dir);
