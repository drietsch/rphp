<?php
// access, mkfifo, mknod, pathconf / fpathconf, and the descriptor
// functions over ints and streams (stdin is /dev/null under the oracle).
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
    echo 'errno=', posix_get_last_error(), "\n";
}

$dir = sys_get_temp_dir() . '/posix-files-' . getmypid();
mkdir($dir);
chdir($dir);

t(fn() => posix_access('/etc/passwd', POSIX_R_OK));
t(fn() => posix_access('/etc/passwd', POSIX_F_OK));
t(fn() => posix_access('/etc/passwd', POSIX_W_OK | POSIX_R_OK));
t(fn() => posix_access('/no/such/file'));
t(fn() => posix_access(''));
t(fn() => posix_access("a\0b"));

// A fifo, made relative to the working directory.
var_dump(file_exists('pipe'));
t(fn() => posix_mkfifo('pipe', 0600));
var_dump(is_file("$dir/pipe"), (fileperms("pipe") & 0170000) === POSIX_S_IFIFO, file_exists("pipe"));
t(fn() => posix_access('pipe', POSIX_R_OK | POSIX_W_OK));
t(fn() => posix_mkfifo('pipe', 0600));
t(fn() => posix_mkfifo('/no/such/dir/pipe', 0600));
t(fn() => posix_mkfifo("a\0b", 0600));
unlink('pipe');
var_dump(file_exists('pipe'));

t(fn() => posix_mknod('/no/such/dir/node', POSIX_S_IFCHR));
t(fn() => posix_mknod("a\0b", POSIX_S_IFREG));
t(fn() => posix_mknod('/no/such/dir/node', POSIX_S_IFIFO | 0600));

t(fn() => posix_pathconf('/', POSIX_PC_NAME_MAX));
t(fn() => posix_pathconf('/', POSIX_PC_PATH_MAX));
t(fn() => posix_pathconf('', POSIX_PC_NAME_MAX));
t(fn() => posix_pathconf("a\0b", POSIX_PC_NAME_MAX));
t(fn() => posix_pathconf('/no/such/file', POSIX_PC_NAME_MAX));
t(fn() => posix_pathconf('/', 9999));
t(fn() => posix_fpathconf(STDIN, POSIX_PC_NAME_MAX) === posix_fpathconf(0, POSIX_PC_NAME_MAX));
t(fn() => posix_fpathconf(99, POSIX_PC_PIPE_BUF));
t(fn() => posix_fpathconf(-1, POSIX_PC_PIPE_BUF));
t(fn() => posix_fpathconf('x', POSIX_PC_PIPE_BUF));

// Descriptors: stdin is /dev/null, 99 is closed.
t(fn() => posix_isatty(STDIN));
t(fn() => posix_isatty(0));
t(fn() => posix_isatty(99));
t(fn() => posix_isatty(-1));
t(fn() => posix_isatty('x'));
t(fn() => posix_isatty(null));
t(fn() => posix_isatty(fopen('php://memory', 'r')));
t(fn() => posix_isatty(fopen('php://temp', 'r')));
$f = fopen(__FILE__, 'r');
t(fn() => posix_isatty($f));
t(fn() => posix_ttyname(STDIN));
t(fn() => posix_ttyname(99));
t(fn() => posix_ttyname(PHP_INT_MAX));
t(fn() => posix_ttyname($f));
t(fn() => posix_ttyname(fopen('php://memory', 'r')));
t(fn() => posix_ttyname(null));

chdir('/');
rmdir($dir);
