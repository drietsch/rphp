<?php
// pcntl_async_signals(true): handlers run without pcntl_signal_dispatch —
// right after the call that raised the signal, inside loops, and for a
// timer (pcntl_alarm) that fires while the script is busy. This is the
// path Symfony Console's SignalRegistry relies on for SIGINT / SIGTERM.
var_dump(pcntl_async_signals());
var_dump(pcntl_async_signals(true));
var_dump(pcntl_async_signals(null), pcntl_async_signals());

$got = [];
foreach ([SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGALRM] as $s) {
    pcntl_signal($s, function (int $signo, array $info) use (&$got) {
        $got[] = $signo;
        echo "handled $signo\n";
    });
}

posix_kill(posix_getpid(), SIGTERM);
echo "after SIGTERM: ", implode(',', $got), "\n";

// Inside a loop: the handler runs between iterations.
for ($i = 0; $i < 3; $i++) {
    if ($i === 1) {
        posix_kill(posix_getpid(), SIGUSR1);
    }
    echo "iteration $i, seen ", count($got), "\n";
}

// A handler chained to the previous one, as SignalRegistry does.
$previous = pcntl_signal_get_handler(SIGINT);
pcntl_signal(SIGINT, function (int $signo, $info) use ($previous) {
    echo "outer $signo\n";
    if (is_callable($previous)) {
        $previous($signo, $info);
    }
});
posix_kill(posix_getpid(), SIGINT);

// pcntl_alarm: the SIGALRM arrives while the loop spins.
var_dump(pcntl_alarm(1));
$start = microtime(true);
while (!in_array(SIGALRM, $got, true) && microtime(true) - $start < 5) {
    usleep(1000);
}
var_dump(in_array(SIGALRM, $got, true));
var_dump(pcntl_alarm(0));

// Switched off: deliveries wait for an explicit dispatch again.
var_dump(pcntl_async_signals(false));
posix_kill(posix_getpid(), SIGUSR2);
echo "pending, seen ", count($got), "\n";
pcntl_signal_dispatch();
echo "dispatched, seen ", count($got), "\n";
echo implode(',', $got), "\n";

// exit() from a handler ends the script, as a console command does.
pcntl_async_signals(true);
pcntl_signal(SIGTERM, function () {
    echo "terminating\n";
    exit(3);
});
register_shutdown_function(function () { echo "shutdown ran\n"; });
posix_kill(posix_getpid(), SIGTERM);
echo "not reached\n";
