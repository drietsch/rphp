<?php
// hash_update_file() and hash_update_stream() feed a running HashContext.
file_put_contents('hash_tail.txt', "hello world\n");

$c = hash_init('md5');
var_dump(hash_update_file($c, 'hash_tail.txt'));
var_dump(hash_final($c) === md5("hello world\n"));

$c = hash_init('sha256');
hash_update($c, 'prefix:');
hash_update_file($c, 'hash_tail.txt');
var_dump(hash_final($c) === hash('sha256', "prefix:hello world\n"));

$c = hash_init('md5');
var_dump(hash_update_file($c, 'missing_tail.txt'));

$c = hash_init('md5', HASH_HMAC, 'key');
hash_update_file($c, 'hash_tail.txt');
var_dump(hash_final($c) === hash_hmac('md5', "hello world\n", 'key'));

$c = hash_init('sha1');
$f = fopen('hash_tail.txt', 'r');
var_dump(hash_update_stream($c, $f, 5));
var_dump(ftell($f));
var_dump(hash_update_stream($c, $f));
var_dump(hash_update_stream($c, $f));
var_dump(hash_final($c) === sha1("hello world\n"));
fclose($f);

$c = hash_init('crc32b');
$m = fopen('php://memory', 'w+');
fwrite($m, 'abcdef');
rewind($m);
var_dump(hash_update_stream($c, $m, 0));
var_dump(hash_update_stream($c, $m, -5));
var_dump(hash_final($c) === hash('crc32b', 'abcdef'));

try {
    hash_update_stream($c, $m);
} catch (\Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    hash_update_file($c, 'hash_tail.txt');
} catch (\Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    hash_update_stream(hash_init('md5'), 1);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
try {
    hash_update_file('md5', 'hash_tail.txt');
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
unlink('hash_tail.txt');
