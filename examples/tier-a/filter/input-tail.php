<?php
// filter_input()/filter_input_array() read the SAPI's original input arrays.
// Under the CLI, GET/POST/COOKIE were never initialized; SERVER holds only
// the five variables the CLI registers; ENV is the environment at startup.
var_dump(filter_input(INPUT_GET, 'x'));
var_dump(filter_input(INPUT_GET, 'x', FILTER_VALIDATE_INT));
var_dump(filter_input(INPUT_GET, 'x', FILTER_VALIDATE_INT, FILTER_NULL_ON_FAILURE));
var_dump(filter_input(INPUT_POST, 'x', FILTER_DEFAULT, ['options' => ['default' => 5]]));
var_dump(filter_input(INPUT_COOKIE, 'x', FILTER_VALIDATE_BOOL, ['flags' => FILTER_NULL_ON_FAILURE]));
var_dump(filter_input_array(INPUT_GET));
var_dump(filter_input_array(INPUT_POST, ['a' => FILTER_VALIDATE_INT]));
var_dump(filter_input_array(INPUT_COOKIE, ['a' => FILTER_VALIDATE_INT], false));
var_dump(filter_has_var(INPUT_GET, 'x'));

// $_GET is not what filter_input() reads.
$_GET['x'] = '5';
var_dump(filter_input(INPUT_GET, 'x'), filter_has_var(INPUT_GET, 'x'));

$server = filter_input_array(INPUT_SERVER);
var_dump(array_keys($server));
var_dump(basename($server['PHP_SELF']), $server['DOCUMENT_ROOT']);
var_dump(filter_input(INPUT_SERVER, 'argv'), filter_input(INPUT_SERVER, 'REQUEST_TIME'));
var_dump(filter_has_var(INPUT_SERVER, 'PHP_SELF'), filter_has_var(INPUT_SERVER, 'argc'));
var_dump(basename(filter_input(INPUT_SERVER, 'SCRIPT_NAME', FILTER_CALLBACK, ['options' => 'strtoupper'])));
var_dump(filter_input(INPUT_SERVER, 'PHP_SELF', FILTER_VALIDATE_INT));
$f = filter_input_array(INPUT_SERVER, ['DOCUMENT_ROOT' => FILTER_DEFAULT, 'NOPE' => FILTER_VALIDATE_INT]);
var_dump($f);

$env = filter_input_array(INPUT_ENV);
var_dump(is_array($env), isset($env['PATH']), filter_has_var(INPUT_ENV, 'PATH'));
var_dump(filter_input(INPUT_ENV, 'PATH') === getenv('PATH'));
putenv('PATH=/changed');
var_dump(filter_input(INPUT_ENV, 'PATH') === '/changed');

foreach (['filter_input', 'filter_input_array', 'filter_has_var'] as $fn) {
    try {
        $fn === 'filter_input_array' ? $fn(3) : $fn(3, 'x');
    } catch (\ValueError $e) {
        echo $e->getMessage(), "\n";
    }
}
var_dump(INPUT_POST, INPUT_GET, INPUT_COOKIE, INPUT_ENV, INPUT_SERVER);
