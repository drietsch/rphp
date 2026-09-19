<?php
// `disk_free_space()`, `disk_total_space()` and the `diskfreespace()` alias.
// The numbers move while the process runs, so only what is stable about them
// is printed: they are floats, they are positive, free never exceeds total,
// and the alias answers the same filesystem — but names *itself* when it
// complains.
$dir = sys_get_temp_dir() . '/rphp-fs-space';
@mkdir($dir);

$free = disk_free_space($dir);
$total = disk_total_space($dir);
var_dump(is_float($free), is_float($total));
var_dump($free > 0, $total > 0, $free <= $total);
var_dump(disk_total_space($dir) === disk_total_space(sys_get_temp_dir()));
var_dump(is_float(diskfreespace($dir)));

// A path that is not there is a warning and `false`, under the name that was
// called.
var_dump(@disk_free_space($dir . '/nope'), error_get_last()['message']);
var_dump(@disk_total_space($dir . '/nope'), error_get_last()['message']);
var_dump(@diskfreespace($dir . '/nope'), error_get_last()['message']);

// A file is as good as a directory: the call is about the filesystem.
$p = $dir . '/f.txt';
file_put_contents($p, 'x');
var_dump(disk_total_space($p) === $total);

var_dump(unlink($p), rmdir($dir));
