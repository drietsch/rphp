<?php
// Builtins on the engine's frameless call path behave exactly as on the
// full one: a diagnostic, an exception, a __toString() or a callback
// re-runs the call with a frame, so traces, handlers and side effects
// come out as php's.
function f() { return strlen("abc") + abs(-2) + count([1, 2]) + intdiv(7, 2) + max(1, 5) + min(4, 2); }
var_dump(f());
class S { public $n = 0; function __toString() { $this->n++; echo "[ts]"; return "abcd"; } }
$s = new S; var_dump(strlen($s), $s->n, str_repeat($s, 2), $s->n, in_array("abcd", [$s]), $s->n);
try { var_dump(intdiv(1, 0)); } catch (DivisionByZeroError $e) { echo get_class($e), ": ", $e->getMessage(), "\n", $e->getTraceAsString(), "\n"; }
try { var_dump(str_repeat("x", -1)); } catch (ValueError $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(array_combine([1, 2], [1])); } catch (ValueError $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
set_error_handler(function ($no, $str, $file, $line) { echo "handler($no): $str @$line\n"; echo implode(" < ", array_map(fn($fr) => $fr['function'], debug_backtrace())), "\n"; return true; });
function g($v) { return "" . $v; }
var_dump(g([1]));
var_dump(max("abc", 1), min([1, "x"]), 5 % 3, "5" + "5", 10 / 4);
var_dump(array_sum([1, "2", "x"]));
var_dump(str_replace("a", "b", "banana"), substr("hello", 1, 3), strpos("hello", "l"), ucfirst("élan"), sprintf("%05.1f|%s", 3.14159, "x"));
var_dump(5 <=> 3, implode(",", [1, 2, null, false, true]));
restore_error_handler();
$w = new WeakMap; $o = new stdClass; $w[$o] = 1;
var_dump(count($w), spl_object_id($o) === spl_object_id($o), is_countable($w), is_iterable($w));
function h(...$xs) { return max(...$xs); }
var_dump(h(3, 9, 4), max(...[1, 2]), array_key_exists("a", ["a" => null]), array_search(2, [1, 2, 3], true));
try { var_dump(max([])); } catch (ValueError $e) { echo get_class($e), ": ", $e->getMessage(), "\n", $e->getTraceAsString(), "\n"; }
$r = []; $r[] = strlen(...); var_dump($r[0]("four"), array_map('strlen', ["a", "bb"]), call_user_func('abs', -1));
