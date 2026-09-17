<?php
// The call ABI: defaults (constants and thunks evaluated per call), variadics,
// named arguments, argument unpacking (positional and string-keyed), extra
// arguments and func_get_args() / func_num_args() / func_get_arg(), by-ref
// parameters through variables / elements / properties, late-bound function
// lookup (a call before the declaration, a conditional declaration),
// recursion 50000 deep, and evaluation order (the callee resolves before
// its arguments are evaluated).

function defaults($a, $b = 2, $c = PHP_INT_SIZE, $d = [1, 2], $e = null, $f = 'x' . 'y') {
    return "$a|$b|$c|" . json_encode($d) . '|' . json_encode($e) . "|$f";
}
echo defaults(1), "\n";
echo defaults(1, 3, 4, ['z'], false, 'q'), "\n";
echo defaults(b: 9, a: 8), "\n";
echo defaults(1, e: 'named'), "\n";

// each call re-evaluates the default thunk
$evals = 0;
function next_id() { global $evals; return ++$evals; }
function with_thunk($x, $id = null) { return $id ?? next_id(); }
echo with_thunk(0), with_thunk(0), with_thunk(0, 7), "\n";   // 127

// variadics and unpacking
function sum(...$n) { return array_sum($n); }
function tagged($tag, ...$rest) { return $tag . ':' . implode(',', $rest) . ':' . count($rest); }
echo sum(), ' ', sum(1), ' ', sum(1, 2, 3), ' ', sum(...[4, 5, 6]), ' ', sum(1, ...[2, 3], ...[4]), "\n";
echo tagged('t'), ' ', tagged('t', 'a', 'b'), ' ', tagged(...['x', 'y']), "\n";
function kw($a, $b, ...$rest) { return "$a-$b-" . json_encode($rest); }
echo kw(1, 2, 3, 4), ' ', kw(...['a' => 1, 'b' => 2]), ' ', kw(1, 2, x: 3, y: 4), ' ', kw(...[1, 2, 'k' => 'v']), "\n";

// extra arguments and the func_* family
function extras($first) {
    return json_encode([func_num_args(), func_get_args(), func_get_arg(0), $first]);
}
echo extras('a'), ' ', extras('a', 'b', 'c'), "\n";
function modified($p) { $p = 'changed'; return func_get_args(); }
echo json_encode(modified('orig')), "\n";
function no_params() { return func_num_args() . json_encode(func_get_args()); }
echo no_params(), no_params(1, 2), "\n";

// by-reference parameters through every lvalue shape
function set_ref(&$slot, $v) { $slot = $v; }
$var = null; $arr = ['k' => 1, 'n' => ['m' => 1]]; $obj = new stdClass; $obj->p = 1;
set_ref($var, 'v');
set_ref($arr['k'], 'k');
set_ref($arr['new'], 'new');
set_ref($obj->p, 'p');
set_ref($obj->q, 'q');
echo json_encode([$var, $arr, $obj]), "\n";
function by_ref_variadic(&...$refs) { foreach ($refs as &$r) { $r .= '!'; } }
$r1 = 'a'; $r2 = 'b';
by_ref_variadic($r1, $r2);
echo $r1, $r2, "\n";
function by_value($x) { $x[] = 'nope'; return count($x); }
$keep = [1];
echo by_value($keep), count($keep), "\n";
// a by-ref parameter given a temporary is silently by value
$tmp = [3, 1, 2];
sort($tmp);
echo implode($tmp), "\n";

// late binding: called before declared, conditionally declared, by string, by closure
echo later(2), "\n";
function later($x) { return $x * 21; }
if (!function_exists('polyfill')) {
    function polyfill() { return 'poly'; }
}
echo polyfill(), ' ', function_exists('polyfill') ? 'exists' : 'missing', "\n";
$name = 'later';
echo $name(3), ' ', call_user_func($name, 4), ' ', call_user_func_array('later', [5]), "\n";
$byref = 'set_ref';
$target = 'unset';
$byref($target, 'dyn');
echo $target, "\n";

// nested calls in argument position keep their windows straight
function id($x) { return $x; }
function add3($a, $b, $c) { return $a + $b + $c; }
echo add3(id(1), add3(id(2), 3, id(4)), id(add3(5, 6, id(7)))), "\n";   // 28
echo strlen(implode(',', array_map('id', [1, 22, 333]))), "\n";       // 8

// deep recursion does not touch the host stack
function depth($n) { return $n == 0 ? 0 : 1 + depth($n - 1); }
echo depth(50000), "\n";
function mutual_a($n) { return $n == 0 ? 'a' : mutual_b($n - 1); }
function mutual_b($n) { return $n == 0 ? 'b' : mutual_a($n - 1); }
echo mutual_a(10001), "\n";

// the callee is resolved before the arguments are evaluated
function order_log($tag) { echo $tag; return $tag; }
order_log(order_log('1') . order_log('2'));
echo "\n";
echo (function () { return implode(',', func_get_args()); })('a', 'b'), "\n";
