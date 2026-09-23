<?php
// getrlimit / setrlimit (keys, "unlimited", the validation) and sysconf.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
    echo 'errno=', posix_get_last_error(), "\n";
}

$all = posix_getrlimit();
var_dump(array_keys($all));
foreach ($all as $k => $v) {
    var_dump($k, is_int($v) || $v === 'unlimited');
}
$nofile = posix_getrlimit(POSIX_RLIMIT_NOFILE);
var_dump(count($nofile), $nofile === [$all['soft openfiles'], $all['hard openfiles']]);
var_dump(posix_getrlimit(null) === $all);
t(fn() => posix_getrlimit(99));
t(fn() => posix_getrlimit(-1));

// Lowering the soft core limit is always allowed.
$core = posix_getrlimit(POSIX_RLIMIT_CORE);
$hard = $core[1] === 'unlimited' ? -1 : $core[1];
t(fn() => posix_setrlimit(POSIX_RLIMIT_CORE, 0, $hard));
var_dump(posix_getrlimit(POSIX_RLIMIT_CORE)[0]);
t(fn() => posix_setrlimit(POSIX_RLIMIT_CORE, 10, 5));
t(fn() => posix_setrlimit(POSIX_RLIMIT_CORE, -2, 5));
t(fn() => posix_setrlimit(POSIX_RLIMIT_CORE, 0, -2));
t(fn() => posix_setrlimit(99, 1, 1));

var_dump(posix_sysconf(POSIX_SC_PAGESIZE) > 0);
var_dump(posix_sysconf(POSIX_SC_NPROCESSORS_ONLN) >= 1);
var_dump(posix_sysconf(POSIX_SC_NPROCESSORS_CONF) >= posix_sysconf(POSIX_SC_NPROCESSORS_ONLN));
var_dump(posix_sysconf(POSIX_SC_CLK_TCK));
var_dump(posix_sysconf(POSIX_SC_ARG_MAX) > 4096, posix_sysconf(POSIX_SC_OPEN_MAX) > 0);
t(fn() => posix_sysconf(9999));
t(fn() => posix_sysconf(-1));
