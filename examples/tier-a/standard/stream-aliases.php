<?php
function t(callable $f) {
    try { var_dump($f()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
file_put_contents('alias.txt', "abc");
$h = fopen('alias.txt', 'r');
t(fn() => strchr('hello world', 'o'));
t(fn() => strchr('hello world', 'o', true));
t(fn() => strchr('hello', 'z'));
t(fn() => socket_get_status($h) == stream_get_meta_data($h));
t(fn() => socket_get_status($h)['mode']);
t(fn() => socket_set_blocking($h, false));
t(fn() => socket_set_blocking($h, true));
t(fn() => set_file_buffer($h, 0));
t(fn() => stream_supports_lock($h));
t(fn() => stream_supports_lock(fopen('php://memory', 'r')));
t(fn() => stream_supports_lock(fopen('php://temp', 'r+')));
t(fn() => stream_supports_lock(STDIN));
t(fn() => stream_supports_lock(opendir('.')));
fclose($h);
t(fn() => stream_supports_lock($h));
t(fn() => socket_get_status($h));
t(fn() => stream_supports_lock(5));
t(fn() => socket_set_blocking(5, true));
unlink('alias.txt');
