<?php
// The mhash family (functions deprecated in 8.1, MHASH_* constants in 8.5).
var_dump(MHASH_CRC32, MHASH_MD5, MHASH_SHA1, MHASH_SHA256, MHASH_XXH128);
var_dump(mhash_count());
foreach ([0, 1, 2, 5, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 28, 29, 30, 31, 32, 33, 34, 38, 39, 40, 41] as $id) {
    echo $id, ' ', mhash_get_hash_name($id), ' ', mhash_get_block_size($id), ' ', bin2hex(mhash($id, 'abc')), "\n";
}
foreach ([4, 6, 26, 42, -1] as $id) {
    var_dump(mhash_get_hash_name($id), mhash_get_block_size($id), mhash($id, 'x'));
}
var_dump(bin2hex(mhash(MHASH_MD5, 'abc', 'key')));
var_dump(bin2hex(mhash(MHASH_SHA256, 'abc', str_repeat('k', 100))));
var_dump(bin2hex(mhash(MHASH_MD5, 'abc', null)));
var_dump(bin2hex(mhash(MHASH_MD5, 'abc', '')));
try {
    mhash(MHASH_CRC32, 'abc', 'key');
} catch (\ValueError $e) {
    echo $e->getMessage(), "\n";
}

var_dump(bin2hex(mhash_keygen_s2k(MHASH_SHA1, 'pw', 'saltsalt', 20)));
var_dump(bin2hex(mhash_keygen_s2k(MHASH_MD5, 'pw', 'salt', 40)));
var_dump(bin2hex(mhash_keygen_s2k(MHASH_SHA256, 'password', 'a-much-longer-salt', 7)));
var_dump(bin2hex(mhash_keygen_s2k(MHASH_CRC32, 'a', 'b', 10)));
var_dump(mhash_keygen_s2k(4, 'a', 'b', 4));
try {
    mhash_keygen_s2k(MHASH_MD5, 'a', 'b', 0);
} catch (\ValueError $e) {
    echo $e->getMessage(), "\n";
}
