<?php
// `chmod()` and the warning text php gives when the path is not there — the C
// `strerror` string, with no Rust decoration.
$dir = sys_get_temp_dir() . '/rphp-chmod-test';
@mkdir($dir);
$p = $dir . '/f.txt';
file_put_contents($p, 'x');

var_dump(chmod($p, 0o600), substr(sprintf('%o', fileperms($p)), -4));
var_dump(chmod($p, 0o644), substr(sprintf('%o', fileperms($p)), -4));
var_dump(chmod('/nope/nope/nope', 0o644));
var_dump(@chmod('/nope/nope/nope', 0o644), error_get_last()['message']);

// The same rule for the other path functions.
var_dump(@unlink('/nope/nope/nope'), error_get_last()['message']);
var_dump(@rmdir('/nope/nope/nope'), error_get_last()['message']);
var_dump(@rename('/nope/nope/nope', $dir . '/x'), error_get_last()['message']);

var_dump(unlink($p), rmdir($dir));
