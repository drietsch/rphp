<?php
// pcntl_fork, the wait family and the status macros. The parent always
// waits before it prints, so the output order is fixed.
echo "before fork\n";
$pid = pcntl_fork();
if ($pid === 0) {
    echo "child: ", posix_getppid() > 0 ? 'has a parent' : '?', "\n";
    exit(7);
}
var_dump(is_int($pid) && $pid > 0);
var_dump(pcntl_waitpid($pid, $status, 0, $usage) === $pid);
var_dump($status, pcntl_wifexited($status), pcntl_wexitstatus($status));
var_dump(pcntl_wifsignaled($status), pcntl_wifstopped($status), pcntl_wifcontinued($status));
var_dump(array_keys($usage));

// A child killed by a signal.
$pid = pcntl_fork();
if ($pid === 0) {
    posix_kill(posix_getpid(), SIGKILL);
    exit(0);
}
var_dump(pcntl_wait($status) === $pid);
var_dump(pcntl_wifexited($status), pcntl_wifsignaled($status), pcntl_wtermsig($status));

// A child whose end the parent learns about through SIGCHLD.
pcntl_signal(SIGCHLD, function ($signo, $info) {
    echo "SIGCHLD: ", implode(',', array_keys($info)), " code=", $info['code'], " status=", $info['status'], "\n";
});
$pid = pcntl_fork();
if ($pid === 0) {
    exit(5);
}
pcntl_waitpid($pid, $status);
pcntl_signal_dispatch();
pcntl_signal(SIGCHLD, SIG_DFL);

// waitid with a siginfo out-parameter.
$pid = pcntl_fork();
if ($pid === 0) {
    exit(9);
}
var_dump(pcntl_waitid(P_PID, $pid, $info, WEXITED));
var_dump(array_keys($info), $info['signo'] === SIGCHLD, $info['status'], $info['pid'] === $pid);

// Nothing left to wait for.
var_dump(pcntl_waitpid(-1, $status, WNOHANG), pcntl_get_last_error(), pcntl_errno());
var_dump(pcntl_waitpid(999999, $status, 0, $usage), $status, $usage);
var_dump(pcntl_wait($status, WNOHANG | WUNTRACED));
var_dump(pcntl_waitid(P_ALL, null, $none, WEXITED | WNOHANG), pcntl_get_last_error());
var_dump(pcntl_strerror(pcntl_get_last_error()));

// The status macros on hand-made values.
foreach ([0, 256, 768, 9, 0x137f, 0x117f, 0xffff, -1, PHP_INT_MAX] as $s) {
    echo $s, ': ', json_encode([
        pcntl_wifexited($s), pcntl_wexitstatus($s), pcntl_wifsignaled($s), pcntl_wtermsig($s),
        pcntl_wifstopped($s), pcntl_wstopsig($s), pcntl_wifcontinued($s),
    ]), "\n";
}
echo "parent done\n";
