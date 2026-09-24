<?php
function t(string $f, $s) {
    try { var_dump($f($s)); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
foreach (['a', 'z', 'Az', 'zz', 'Zz', 'a9', '9', 'Zz99', '0z', '00', '09', 'ZZ', 'zZ9', '1e1', 'Aa'] as $s) {
    t('str_increment', $s);
}
foreach (['b', 'Ab', 'Ba', 'a0', 'aA', '1a', '10', '100', 'a00', 'Aa0', 'A0', 'aa', 'aaa', 'Zz98', 'B0'] as $s) {
    t('str_decrement', $s);
}
foreach (['a', 'A', '0', '00', '000', '01', '0a', '0z', '0A'] as $s) {
    t('str_decrement', $s);
}
foreach (['', ' a', 'a-z', "\xe9", '-', 'a b', "a\0"] as $s) {
    t('str_increment', $s);
    t('str_decrement', $s);
}
t('str_increment', 5);
t('str_decrement', 10);
