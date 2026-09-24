<?php
// Tier-A differential: getrusage() (shape only — the numbers are the
// process's own), proc_nice(), time_sleep_until(), ftok(), lchown()/lchgrp().

foreach ([0, 1, 5, -1] as $mode) {
    $u = getrusage($mode);
    echo $mode, ": ", implode(",", array_keys($u)), "\n";
    var_dump(count(array_filter($u, 'is_int')) === count($u));
}
var_dump(getrusage()["ru_utime.tv_usec"] < 1000000);

// proc_nice(): 0 is always allowed; raising the priority needs root.
var_dump(proc_nice(0));
var_dump(proc_nice(-5));
var_dump(proc_nice(PHP_INT_MAX)); // taken as a C int: -1

// time_sleep_until()
var_dump(time_sleep_until(1.0));
var_dump(time_sleep_until(microtime(true) + 0.02));

// ftok()
file_put_contents("ftok.txt", "x");
$k = ftok("ftok.txt", "A");
$s = stat("ftok.txt");
var_dump($k === ((0x41 << 24) | (($s["dev"] & 0xff) << 16) | ($s["ino"] & 0xffff)));
var_dump(ftok("ftok.txt", "a") - $k === (0x61 - 0x41) << 24);
var_dump(ftok("missing.txt", "a"));
foreach ([["", "a"], ["ftok.txt", ""], ["ftok.txt", "ab"]] as [$f, $p]) {
    try {
        ftok($f, $p);
    } catch (\ValueError $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}

// lchown()/lchgrp(): the link itself, chown's rules for names and ids.
symlink("ftok.txt", "ftok.lnk");
var_dump(lchown("ftok.lnk", getmyuid()));
var_dump(lchown("missing.lnk", getmyuid()), lchgrp("missing.lnk", 0));
var_dump(lchown("ftok.lnk", "no-such-user-xyz"), lchgrp("ftok.lnk", "no-such-group-xyz"));
var_dump(lchown("ftok.lnk", 1.5));
try {
    lchown("ftok.lnk", []);
} catch (\TypeError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
unlink("ftok.lnk");
unlink("ftok.txt");
