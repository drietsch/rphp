<?php
// The argument checks, the recorded errno and the error texts.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
    echo 'last_error=', pcntl_get_last_error(), "\n";
}

t(fn() => pcntl_get_last_error());
t(fn() => pcntl_signal(0, SIG_DFL));
t(fn() => pcntl_signal(-1, SIG_DFL));
t(fn() => pcntl_signal(32, SIG_DFL));
t(fn() => pcntl_signal(SIGUSR1, 5));
t(fn() => pcntl_signal(SIGUSR1, 'no_such_function'));
t(fn() => pcntl_signal(SIGUSR1, [1]));
t(fn() => pcntl_signal(SIGUSR1, 1.5));
t(fn() => pcntl_signal(SIGUSR1, new stdClass()));
t(fn() => pcntl_signal_get_handler(0));
t(fn() => pcntl_signal_get_handler(32));
t(fn() => pcntl_signal_get_handler(SIGKILL));

foreach ([0, 1, 2, 4, 10, 22, 9999, -1, PHP_INT_MAX] as $n) {
    var_dump(pcntl_strerror($n));
}

t(fn() => pcntl_getpriority() === pcntl_getpriority(posix_getpid()));
t(fn() => pcntl_getpriority(999999));
t(fn() => pcntl_getpriority(-1));
t(fn() => pcntl_getpriority(null, 99));
t(fn() => pcntl_getpriority(null, PRIO_DARWIN_BG));
t(fn() => is_int(pcntl_getpriority(null, PRIO_PGRP)));
t(fn() => pcntl_setpriority(pcntl_getpriority()));
t(fn() => pcntl_setpriority(-5));
t(fn() => pcntl_setpriority(0, 999999));
t(fn() => pcntl_setpriority(0, null, 99));

t(fn() => pcntl_alarm(0));
t(fn() => pcntl_alarm(100));
t(fn() => pcntl_alarm(0));
t(fn() => pcntl_alarm(PHP_INT_MAX));
t(fn() => pcntl_alarm(0));

t(fn() => pcntl_exec('/no/such/program'));
t(fn() => pcntl_exec("a\0b"));
t(fn() => pcntl_exec('/bin/echo', ["a\0b"]));
t(fn() => pcntl_exec('/bin/echo', [], ["A" => "x\0y"]));
t(fn() => pcntl_exec('/bin/echo', [], ["a\0b" => '1']));
t(fn() => pcntl_errno());

try {
    pcntl_fork(1);
} catch (\ArgumentCountError $e) {
    echo $e->getMessage(), "\n";
}
