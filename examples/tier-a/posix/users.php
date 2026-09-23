<?php
// Users and groups: the entries' keys and order, lookups by name and id
// agreeing, and a miss being `false` without touching the recorded error.
$me = posix_getpwuid(posix_getuid());
var_dump(array_keys($me));
var_dump($me['uid'] === posix_getuid(), is_int($me['gid']));
var_dump($me['name'] === get_current_user());
var_dump(posix_getpwnam($me['name']) === $me);
var_dump(is_string($me['dir']), is_string($me['shell']), is_string($me['gecos']));

$root = posix_getpwnam('root');
var_dump($root['uid'], $root['gid'], $root['name']);
var_dump(posix_getpwuid(0)['name']);

$g = posix_getgrgid(posix_getgid());
var_dump(array_keys($g));
var_dump($g['gid'] === posix_getgid(), is_array($g['members']));
var_dump(posix_getgrnam($g['name']) === $g);
var_dump(posix_getgrgid(0)['gid'], is_string(posix_getgrgid(0)['name']));

// Misses: false, and the last error stays what it was.
posix_kill(999999, 0);
var_dump(posix_get_last_error());
var_dump(posix_getpwnam('no_such_user_rphp'), posix_get_last_error());
var_dump(posix_getpwnam(''), posix_getpwnam("a\0b"), posix_get_last_error());
var_dump(posix_getpwuid(987654), posix_get_last_error());
var_dump(posix_getgrnam('no_such_group_rphp'), posix_getgrnam(''), posix_get_last_error());
var_dump(posix_getgrgid(987654), posix_get_last_error());

// Ids are C-cast: -1 is the all-ones id.
var_dump(posix_getpwuid(-1));
var_dump(posix_getgrgid(-1)['gid']);

var_dump(posix_initgroups('', 0), posix_initgroups("a\0b", 0));
