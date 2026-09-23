<?php
// pcntl_signal + posix_kill + pcntl_signal_dispatch: the handler gets
// ($signo, $siginfo), deliveries queue until dispatched, oldest first.
$log = [];
$handler = function (int $signo, $info) use (&$log) {
    $log[] = $signo;
    echo "handler($signo): ", implode(',', array_keys($info)), "\n";
    var_dump($info['signo'] === $signo, $info['errno'], $info['code']);
    if (isset($info['pid'])) {
        var_dump($info['pid'] === posix_getpid(), $info['uid'] === posix_getuid());
    }
};
var_dump(pcntl_signal(SIGUSR1, $handler));
var_dump(pcntl_signal(SIGUSR2, $handler, false));
var_dump(pcntl_signal(SIGHUP, $handler));
var_dump(pcntl_signal(SIGTERM, $handler));

posix_kill(posix_getpid(), SIGUSR1);
posix_kill(posix_getpid(), SIGHUP);
posix_kill(posix_getpid(), SIGUSR2);
echo "queued, not yet run: ", count($log), "\n";
var_dump(pcntl_signal_dispatch());
echo "ran: ", implode(' ', $log), "\n";
var_dump(pcntl_signal_dispatch());

// The stored handler comes back as it was given.
var_dump(pcntl_signal_get_handler(SIGUSR1) === $handler);
var_dump(pcntl_signal_get_handler(SIGINT), pcntl_signal_get_handler(SIGWINCH));

// SIG_IGN: the signal is discarded; SIG_DFL is stored as the int.
var_dump(pcntl_signal(SIGUSR1, SIG_IGN), pcntl_signal_get_handler(SIGUSR1));
posix_kill(posix_getpid(), SIGUSR1);
pcntl_signal_dispatch();
echo "after ignored: ", count($log), "\n";
var_dump(pcntl_signal(SIGWINCH, SIG_DFL), pcntl_signal_get_handler(SIGWINCH));

// A method callable and a function name are handlers too.
class Registry
{
    public array $seen = [];
    public function handle(int $signal): void
    {
        $this->seen[] = $signal;
    }
}
$r = new Registry();
pcntl_signal(SIGTERM, [$r, 'handle']);
function on_alarm($signo, $info) { echo "on_alarm ", $signo, ' ', count($info), "\n"; }
pcntl_signal(SIGALRM, 'on_alarm');
posix_kill(posix_getpid(), SIGTERM);
posix_kill(posix_getpid(), SIGALRM);
pcntl_signal_dispatch();
var_dump($r->seen);
var_dump(pcntl_signal_get_handler(SIGTERM) === [$r, 'handle']);

// A handler that throws: the exception surfaces from the dispatch.
pcntl_signal(SIGUSR2, function () { throw new RuntimeException('from the handler'); });
posix_kill(posix_getpid(), SIGUSR2);
try {
    pcntl_signal_dispatch();
} catch (RuntimeException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
