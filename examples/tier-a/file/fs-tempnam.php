<?php
// `tempnam()`: an empty file nobody else holds. The name is random, so only
// its shape is printed — the prefix, the 19 characters php appends, the
// owner-only permissions and the fact that the file is really there and
// really empty.
$dir = sys_get_temp_dir() . '/rphp-fs-tempnam';
@mkdir($dir);

$a = tempnam($dir, 'pre');
var_dump(is_string($a));
var_dump(dirname($a) === realpath($dir));
var_dump(str_starts_with(basename($a), 'pre'), strlen(basename($a)));
var_dump(file_exists($a), filesize($a), is_file($a));
var_dump(substr(sprintf('%o', fileperms($a)), -4));

// Two calls never collide.
$b = tempnam($dir, 'pre');
var_dump($a !== $b, strlen(basename($b)));

// The prefix is optional, and it is run through `basename` and capped at 64
// bytes, so it can never place the file anywhere else.
$c = tempnam($dir, '');
var_dump(strlen(basename($c)));
$d = tempnam($dir, 'a/b');
var_dump(dirname($d) === realpath($dir), basename($d)[0], strlen(basename($d)));
$e = tempnam($dir, str_repeat('x', 80));
var_dump(dirname($e) === realpath($dir), strlen(basename($e)));

// The suffix is alphanumeric.
var_dump(ctype_alnum(substr(basename($a), 3)));

foreach ([$a, $b, $c, $d, $e] as $f) {
    unlink($f);
}

// A directory that cannot hold the file is not an error: php says so and puts
// the file in the system temporary directory instead.
$fallback = tempnam($dir . '/does-not-exist', 'pre');
var_dump(is_string($fallback), dirname($fallback) === realpath(sys_get_temp_dir()));
var_dump(filesize($fallback));
unlink($fallback);

// An empty directory was never a request, so there is nothing to report.
$sys = tempnam('', 'pre');
var_dump(dirname($sys) === realpath(sys_get_temp_dir()), filesize($sys));
unlink($sys);

var_dump(rmdir($dir));
