<?php
// The error channel: strerror, the recorded errno (which success leaves
// alone), and the ValueErrors of the id and pid parameters.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
    echo 'errno=', posix_get_last_error(), ' ', posix_errno(), "\n";
}

t(fn() => posix_get_last_error());
foreach ([0, 1, 2, 3, 9, 13, 22, 25, 1000, -5, PHP_INT_MAX] as $n) {
    var_dump(posix_strerror($n));
}

t(fn() => posix_kill(posix_getpid(), 0));
t(fn() => posix_kill(999999, 0));
t(fn() => posix_kill(posix_getpid(), 99));
t(fn() => posix_kill(PHP_INT_MAX, 0));
t(fn() => posix_kill(-3000000000, 0));

// Not root: changing identity fails with EPERM, keeping it succeeds.
t(fn() => posix_setuid(0));
t(fn() => posix_seteuid(0));
t(fn() => posix_setgid(0));
t(fn() => posix_setegid(0));
t(fn() => posix_setuid(posix_getuid()));
t(fn() => posix_seteuid(posix_geteuid()));
t(fn() => posix_setgid(posix_getgid()));
t(fn() => posix_setegid(posix_getegid()));

t(fn() => posix_getpgid(999999));
t(fn() => posix_getpgid(-1));
t(fn() => posix_getsid(999999));
t(fn() => posix_getsid(-1));
t(fn() => posix_getsid(PHP_INT_MAX));
t(fn() => posix_setpgid(999999, 0));
t(fn() => posix_setpgid(-1, 0));
t(fn() => posix_setpgid(0, -1));

try {
    posix_getpid(1);
} catch (\ArgumentCountError $e) {
    echo $e->getMessage(), "\n";
}
try {
    posix_kill('x', 0);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
