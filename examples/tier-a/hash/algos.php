<?php

// Every algorithm this engine implements, against php's own answer for the
// same input — one-shot, incremental, forked mid-stream, and keyed.
// `hash_algos()` itself is not printed: php lists sixty and the ones still
// missing here (tiger, gost, snefru, haval, murmur3) would be the only
// difference.
$algos = ['md2', 'md4', 'md5', 'sha1', 'sha224', 'sha256', 'sha384',
    'sha512/224', 'sha512/256', 'sha512', 'sha3-224', 'sha3-256', 'sha3-384',
    'sha3-512', 'ripemd128', 'ripemd160', 'ripemd256', 'ripemd320',
    'whirlpool', 'adler32', 'crc32', 'crc32b', 'crc32c', 'fnv132', 'fnv1a32',
    'fnv164', 'fnv1a64', 'joaat', 'xxh32', 'xxh64', 'xxh3', 'xxh128'];

$text = 'The quick brown fox jumps over the lazy dog';
foreach ($algos as $a) {
    printf("%-11s %d %s %s %s\n", $a, (int) in_array($a, hash_algos(), true),
        hash($a, $text), hash($a, ''), bin2hex(hash($a, 'x', true)));
}

// A context forked with hash_copy() keeps the state up to the fork.
foreach ($algos as $a) {
    $c = hash_init($a);
    hash_update($c, 'The quick brown ');
    $fork = hash_copy($c);
    hash_update($c, 'fox jumps over the lazy dog');
    printf("%-11s %s %s\n", $a, hash_final($c), hash_final($fork));
}

// The keyed forms, over exactly the algorithms php will key.
foreach ($algos as $a) {
    if (!in_array($a, hash_hmac_algos(), true)) {
        continue;
    }
    printf("%-11s %s %s\n", $a, hash_hmac($a, 'message', 'key'),
        hash_pbkdf2($a, 'pw', 'salt', 10, 16));
}
echo hash_hkdf('sha3-256', 'key', 16, 'info', 'salt') === ''
    ? "hkdf empty\n" : bin2hex(hash_hkdf('sha3-256', 'key', 16, 'info', 'salt')) . "\n";

// A name neither engine knows is a ValueError. (The algorithms php has and
// this does not — tiger, gost, snefru, haval, murmur3 — are cataloged in
// COVERAGE.md; asking for one here would be the only divergence in this
// file.)
foreach (['nope', 'sha3-255', ''] as $a) {
    try {
        hash($a, 'x');
        echo "no throw: $a\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
