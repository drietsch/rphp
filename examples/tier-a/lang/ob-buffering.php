<?php
// Output buffering: nested ob_start levels, ob_get_clean/ob_get_contents/
// ob_end_flush/ob_get_flush, ob_get_level/ob_get_length, and the edge cases
// (no active buffer) — all on the engine's output stack.

var_dump(ob_get_level());
var_dump(ob_get_clean());          // false: nothing to clean
ob_start();
echo "captured";
var_dump(ob_get_length());
$s = ob_get_clean();
echo "[" . $s . "]\n";

ob_start();
echo "outer-";
ob_start();
echo "inner";
echo ob_get_level();               // 2, into the inner buffer
ob_end_flush();                    // inner -> outer
echo "-tail";
$all = ob_get_contents();
ob_end_clean();
echo $all . "\n";

ob_start();
echo "flushed";
$got = ob_get_flush();             // prints and returns
echo "|" . $got . "\n";

$st = ob_get_status();
var_dump(count($st));
ob_start();
$st = ob_get_status();
echo $st["name"], " level=", $st["level"], " used=", $st["buffer_used"], "\n";
ob_end_clean();
var_dump(ob_end_clean());          // notice + false
