<?php
// Blocking a signal holds it back until it is unblocked; the old mask
// comes back as a list.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

pcntl_signal(SIGUSR1, function ($signo) { echo "got $signo\n"; });
var_dump(pcntl_sigprocmask(SIG_BLOCK, [SIGUSR1], $old), $old);
posix_kill(posix_getpid(), SIGUSR1);
pcntl_signal_dispatch();
echo "blocked: nothing yet\n";
var_dump(pcntl_sigprocmask(SIG_UNBLOCK, [SIGUSR1], $old), $old);
pcntl_signal_dispatch();

var_dump(pcntl_sigprocmask(SIG_SETMASK, [SIGUSR2, SIGHUP]));
var_dump(pcntl_sigprocmask(SIG_BLOCK, [SIGINT], $old), $old);
var_dump(pcntl_sigprocmask(SIG_SETMASK, [SIGINT], $old), $old);
var_dump(pcntl_sigprocmask(SIG_UNBLOCK, [SIGINT], $old), $old);
var_dump(pcntl_sigprocmask(SIG_BLOCK, [SIGKILL, SIGSTOP], $old), $old);
var_dump(pcntl_sigprocmask(SIG_BLOCK, ['2', true, 3.0], $old), $old);
pcntl_sigprocmask(SIG_SETMASK, [SIGUSR1]);
pcntl_sigprocmask(SIG_UNBLOCK, [SIGUSR1], $old);
var_dump($old);

t(fn() => pcntl_sigprocmask(99, [SIGUSR1]));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, []));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, [0]));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, [32]));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, [null]));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, ['x']));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, [[1]]));
t(fn() => pcntl_sigprocmask(SIG_BLOCK, [1.5]));
