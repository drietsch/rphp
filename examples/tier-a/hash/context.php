<?php
// The incremental API: a `HashContext` shows one property, keeps the running
// digest out of sight, and is spent once finalized.
$c = hash_init('sha256');
var_dump($c, get_class($c), $c instanceof HashContext);
hash_update($c, 'ab');
hash_update($c, 'cd');
$forked = hash_copy($c);
hash_update($forked, 'ef');
var_dump(
    hash_final($c) === hash('sha256', 'abcd'),
    hash_final($forked) === hash('sha256', 'abcdef'),
);

// `clone` forks it the same way.
$a = hash_init('sha1');
hash_update($a, 'a');
$b = clone $a;
hash_update($b, 'b');
var_dump(hash_final($a) === hash('sha1', 'a'), hash_final($b) === hash('sha1', 'ab'));

// A keyed context is the same thing as `hash_hmac()`.
$k = hash_init('sha256', HASH_HMAC, 'key');
hash_update($k, 'message');
var_dump(hash_final($k) === hash_hmac('sha256', 'message', 'key'));
var_dump(hash_hmac('sha256', 'message', 'key'));
var_dump(hash_hmac('md5', 'message', 'key'));
var_dump(hash_hmac('sha1', 'x', str_repeat('k', 200)));
var_dump(hash_hmac('sha512', '', ''));

// Constant-time comparison, key derivation.
var_dump(hash_equals('abc', 'abc'), hash_equals('abc', 'abd'), hash_equals('abc', 'ab'));
var_dump(hash_pbkdf2('sha256', 'password', 'salt', 1000, 20));
var_dump(hash_pbkdf2('sha256', 'password', 'salt', 1, 0));
var_dump(hash_pbkdf2('sha1', 'p', 's', 2, 9));
var_dump(bin2hex(hash_hkdf('sha256', 'key')));
var_dump(bin2hex(hash_hkdf('sha256', 'key', 10, 'info', 'salt')));

// The algorithms this build carries, against a fixed input. (php's
// `hash_algos()` lists sixty; the rest are cataloged, so the list itself is
// not compared here.)
foreach ([
    'md5', 'sha1', 'sha224', 'sha256', 'sha384', 'sha512', 'sha512/224',
    'sha512/256', 'crc32', 'crc32b', 'adler32', 'fnv132', 'fnv1a32', 'fnv164',
    'fnv1a64', 'joaat',
] as $algo) {
    printf("%-11s %s\n", $algo, hash($algo, 'The quick brown fox'));
}
var_dump(in_array('sha256', hash_hmac_algos(), true), in_array('crc32b', hash_hmac_algos(), true));

// The error paths.
foreach ([
    fn() => hash('nope', 'x'),
    fn() => hash_init('nope'),
    fn() => hash_init('crc32b', HASH_HMAC, 'k'),
    fn() => hash_init('sha256', HASH_HMAC, ''),
    fn() => hash_hmac('crc32b', 'd', 'k'),
    fn() => hash_update('notacontext', 'x'),
    fn() => new HashContext(),
    fn() => hash_pbkdf2('sha256', 'p', 's', 0),
    fn() => hash_hkdf('sha256', ''),
] as $case) {
    try {
        $case();
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
$spent = hash_init('md5');
hash_final($spent);
try { hash_update($spent, 'more'); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
