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

// --- E3: bare constant names, `const`, define() interplay, magic constants ---
echo PHP_EOL === "\n" ? 'eol' : 'no', ' ', PHP_INT_SIZE, ' ', PHP_INT_MAX, ' ', PHP_VERSION_ID >= 80400 ? 'modern' : 'old', ' ', E_ALL, ' ', M_PI > 3 ? 'pi' : 'nopi', "\n";
const GREETING = 'hi';
const NUMS = [1, 2, 3];
const COMPUTED = GREETING . '!' . PHP_INT_SIZE;
echo GREETING, ' ', count(NUMS), ' ', NUMS[1], ' ', COMPUTED, ' ', \GREETING, "\n";
define('DEFINED_LATER', 42);
echo DEFINED_LATER + 1, ' ', constant('GREETING'), ' ', defined('COMPUTED') ? 'y' : 'n', "\n";
function uses_const($x = PHP_INT_SIZE, $y = GREETING) { return "$x$y"; }
echo uses_const(), ' ', uses_const(1), "\n";
echo __LINE__, ' ', basename(__FILE__), ' ', basename(__DIR__), ' ', __FUNCTION__ === '' ? 'nofn' : __FUNCTION__, ' ', __CLASS__ === '' ? 'nocls' : 'cls', "\n";
function magic() { return __FUNCTION__ . '|' . __METHOD__ . '|' . __LINE__; }
class Magic { function m() { return __CLASS__ . '|' . __FUNCTION__ . '|' . __METHOD__; } }
echo magic(), ' ', (new Magic)->m(), "\n";
$closure = function () { return __FUNCTION__; };
echo strpos($closure(), '{closure:') === 0 ? 'closure-named' : $closure(), "\n";
