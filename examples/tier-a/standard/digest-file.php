<?php
function t(callable $f) {
    try { var_dump($f()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
file_put_contents('digest.txt', "hello");
file_put_contents('empty.txt', "");
t(fn() => md5_file('digest.txt'));
t(fn() => bin2hex(md5_file('digest.txt', true)));
t(fn() => sha1_file('digest.txt'));
t(fn() => bin2hex(sha1_file('digest.txt', true)));
t(fn() => md5_file('empty.txt'));
t(fn() => sha1_file('empty.txt'));
t(fn() => md5_file('nope.txt'));
t(fn() => sha1_file('nope.txt'));
t(fn() => md5_file(''));
t(fn() => sha1_file(''));
t(fn() => md5_file('.'));
t(fn() => md5_file("digest\0.txt"));
t(fn() => sha1_file("digest\0.txt"));
t(fn() => md5_file('php://memory'));
t(fn() => md5_file('file://' . realpath('digest.txt')) === md5('hello'));
t(fn() => md5_file([]));
unlink('digest.txt');
unlink('empty.txt');
