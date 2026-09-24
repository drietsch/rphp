<?php
// php 8.4: exit() and die() are functions as well as constructs.
var_dump(function_exists('exit'), function_exists('die'), is_callable('exit'), is_callable('die'));
var_dump(in_array('exit', get_extension_funcs('Core')), in_array('die', get_extension_funcs('Core')));

// The parameter is `string|int $status = 0`.
foreach ([[], new stdClass, fopen('php://memory', 'r')] as $bad) {
    try {
        exit($bad);
    } catch (\TypeError $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
try {
    array_map('exit', [[1]]);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
try {
    call_user_func('die', []);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
try {
    exit(1, 2);
} catch (\ArgumentCountError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    exit(code: 1);
} catch (\Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    $f = 'die';
    $f(status: [1]);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}

$c = exit(...);
var_dump($c instanceof Closure);
echo (new ReflectionFunction('exit')), (new ReflectionFunction('die'));

register_shutdown_function(function () {
    echo "shutdown runs\n";
});
try {
    // A string is printed and the status is 0; `finally` does not run.
    exit(status: "bye\n");
} finally {
    echo "not reached\n";
}
