<?php
// Every constant php's pcntl declares on darwin, with its value.
foreach ([
    'WNOHANG', 'WUNTRACED', 'WCONTINUED', 'WEXITED', 'WSTOPPED', 'WNOWAIT',
    'P_ALL', 'P_PID', 'P_PGID', 'SIG_IGN', 'SIG_DFL', 'SIG_ERR',
    'SIGHUP', 'SIGINT', 'SIGQUIT', 'SIGILL', 'SIGTRAP', 'SIGABRT', 'SIGIOT', 'SIGBUS',
    'SIGFPE', 'SIGKILL', 'SIGUSR1', 'SIGSEGV', 'SIGUSR2', 'SIGPIPE', 'SIGALRM', 'SIGTERM',
    'SIGCHLD', 'SIGCONT', 'SIGSTOP', 'SIGTSTP', 'SIGTTIN', 'SIGTTOU', 'SIGURG', 'SIGXCPU',
    'SIGXFSZ', 'SIGVTALRM', 'SIGPROF', 'SIGWINCH', 'SIGIO', 'SIGINFO', 'SIGSYS', 'SIGBABY',
    'PRIO_PGRP', 'PRIO_USER', 'PRIO_PROCESS', 'PRIO_DARWIN_BG', 'PRIO_DARWIN_THREAD',
    'SIG_BLOCK', 'SIG_UNBLOCK', 'SIG_SETMASK',
    'PCNTL_EINTR', 'PCNTL_ECHILD', 'PCNTL_EINVAL', 'PCNTL_EAGAIN', 'PCNTL_ESRCH',
    'PCNTL_EACCES', 'PCNTL_EPERM', 'PCNTL_ENOMEM', 'PCNTL_E2BIG', 'PCNTL_EFAULT', 'PCNTL_EIO',
    'PCNTL_EISDIR', 'PCNTL_ELOOP', 'PCNTL_EMFILE', 'PCNTL_ENAMETOOLONG', 'PCNTL_ENFILE',
    'PCNTL_ENOENT', 'PCNTL_ENOEXEC', 'PCNTL_ENOTDIR', 'PCNTL_ETXTBSY', 'PCNTL_ENOSPC',
    'PCNTL_EUSERS',
] as $name) {
    echo $name, ' = ', var_export(constant($name), true), "\n";
}
var_dump(extension_loaded('pcntl'));
foreach (['pcntl_signal', 'pcntl_async_signals', 'pcntl_fork', 'pcntl_waitid', 'pcntl_unshare', 'pcntl_sigwaitinfo', 'pcntl_sigtimedwait', 'pcntl_rfork'] as $f) {
    echo $f, ' ', var_export(function_exists($f), true), "\n";
}
