<?php
// Hard links, symbolic links and ownership. `symlink()` stores its target
// byte for byte — a relative target is read against the link's own directory,
// never against the cwd — and `linkinfo()` answers `-1` rather than `false`
// when the path is not there.
$dir = sys_get_temp_dir() . '/rphp-fs-links';
@mkdir($dir);
$p = $dir . '/f.txt';
foreach (['sl', 'rel', 'hl', 'broken'] as $e) {
    @unlink($dir . '/' . $e);
}
file_put_contents($p, "data\n");

var_dump(symlink($p, $dir . '/sl'));
var_dump(readlink($dir . '/sl') === $p);
var_dump(is_link($dir . '/sl'), is_file($dir . '/sl'));

// A relative target is stored as written.
chdir($dir);
var_dump(symlink('f.txt', 'rel'), readlink('rel'), file_get_contents('rel'));

// A symlink may point at nothing; a hard link may not.
var_dump(symlink($dir . '/nope', $dir . '/broken'), readlink($dir . '/broken') === $dir . '/nope');
var_dump(file_exists($dir . '/broken'), is_link($dir . '/broken'));

var_dump(link($p, $dir . '/hl'));
var_dump(stat($p)['nlink'], is_link($dir . '/hl'));
var_dump(file_get_contents($dir . '/hl'));

// `linkinfo()` is the device of the unfollowed stat, which is only ever used
// as "is this path there at all".
var_dump(linkinfo($p) === stat($p)['dev']);
var_dump(linkinfo($dir . '/sl') === lstat($dir . '/sl')['dev']);
var_dump(@linkinfo($dir . '/nope'), error_get_last()['message']);

// The failures, all reported through the warning channel with php's C
// `strerror` text.
var_dump(@symlink($p, $dir . '/sl'), error_get_last()['message']);
var_dump(@link($p, $dir . '/hl'), error_get_last()['message']);
var_dump(@link($dir . '/nope', $dir . '/hl2'), error_get_last()['message']);
var_dump(@readlink($p), error_get_last()['message']);
var_dump(@readlink($dir . '/nope'), error_get_last()['message']);

// Ownership. Setting a file to the owner and group it already has is the one
// change an unprivileged process is always allowed to make.
var_dump(fileowner($p) === getmyuid());
var_dump(is_int(filegroup($p)));
var_dump(chown($p, fileowner($p)), chgrp($p, filegroup($p)));
var_dump(@chown($dir . '/nope', 0), error_get_last()['message']);
var_dump(@chgrp($dir . '/nope', 0), error_get_last()['message']);
// A *string* is a name, never an id: `"0"` is looked up, not used as uid 0.
var_dump(@chown($p, 'no-such-user-xyz'), error_get_last()['message']);
var_dump(@chgrp($p, 'no-such-group-xyz'), error_get_last()['message']);
var_dump(@chown($p, '0'), error_get_last()['message']);

chdir(sys_get_temp_dir());
foreach (['sl', 'rel', 'hl', 'broken', 'f.txt'] as $e) {
    unlink($dir . '/' . $e);
}
var_dump(rmdir($dir));
