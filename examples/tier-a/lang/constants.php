<?php
// Engine constants through constant()/defined()/define(): the compiler does
// not lower bare constant names yet, so this snippet only uses the builtins.

var_dump(defined('PHP_EOL'), constant('PHP_EOL') === "\n");
var_dump(constant('PHP_VERSION'), constant('PHP_MAJOR_VERSION'), constant('PHP_VERSION_ID'));
var_dump(constant('PHP_INT_MAX'), constant('PHP_INT_SIZE'), constant('PHP_FLOAT_DIG'));
var_dump(constant('E_ALL'), constant('E_WARNING'), constant('E_USER_DEPRECATED'), defined('E_STRICT'));
var_dump(constant('DIRECTORY_SEPARATOR'), constant('PATH_SEPARATOR'), constant('PHP_SAPI'));
var_dump(constant('M_PI'), constant('PHP_ROUND_HALF_UP'), constant('PHP_OUTPUT_HANDLER_STDFLAGS'));
var_dump(defined('NOPE_NOT_DEFINED'));
var_dump(define('ANSWER', 42), constant('ANSWER'), defined('ANSWER'));
var_dump(define('ANSWER', 43));    // warning: already defined
var_dump(constant('ANSWER'));
var_dump(ini_get('display_errors'), ini_get('precision'), ini_get('no.such.directive'));
var_dump(ini_set('precision', '10'), ini_get('precision'));
var_dump(error_reporting(), error_reporting(constant('E_ALL')));
