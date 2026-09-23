<?php
// The process's identity: ids, groups, session, uname, times — printed as
// shapes and relations, since the values differ from run to run.
var_dump(extension_loaded('posix'));
var_dump(posix_getpid() === getmypid(), posix_getpid() > 0);
var_dump(is_int(posix_getppid()), posix_getppid() > 0, posix_getppid() !== posix_getpid());
var_dump(posix_getuid() === getmyuid(), posix_geteuid() === posix_getuid());
var_dump(posix_getgid() === getmygid(), posix_getegid() === posix_getgid());
var_dump(posix_getpgrp() === posix_getpgid(0), posix_getpgid(posix_getpid()) === posix_getpgrp());
var_dump(is_int(posix_getsid(0)), posix_getsid(0) === posix_getsid(posix_getpid()));

$groups = posix_getgroups();
var_dump(is_array($groups), $groups === array_values($groups));
var_dump(count(array_filter($groups, 'is_int')) === count($groups));

$login = posix_getlogin();
var_dump(is_string($login) || $login === false);

$u = posix_uname();
var_dump(array_keys($u));
var_dump($u['sysname'] === PHP_OS, $u['machine'] === php_uname('m'), $u['release'] === php_uname('r'));
var_dump($u['nodename'] === php_uname('n'));

$t = posix_times();
var_dump(array_keys($t));
foreach ($t as $k => $v) {
    var_dump($k, is_int($v) && $v >= 0);
}

var_dump(posix_ctermid());
var_dump(posix_getcwd() === getcwd());
chdir('/');
var_dump(posix_getcwd());
