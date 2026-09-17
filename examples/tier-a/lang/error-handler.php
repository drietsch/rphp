<?php
// The diagnostics channel: a VM warning (undefined array key) reaches a
// set_error_handler closure with (errno, errstr, errfile, errline); a false
// return falls through to the standard display; restore_error_handler pops;
// error_get_last records the last standard-path error; trigger_error levels.

$a = [];
echo $a["missing"];                // standard display
$h = set_error_handler(function ($no, $str, $file, $line) {
    echo "handled[$no] $str on line $line\n";
    return true;
});
var_dump($h);                      // no previous handler
echo $a[7];                        // handled
$n = null;
echo $n[0];                        // handled
set_error_handler(function ($no, $str) {
    echo "pass-through: $str\n";
    return false;                  // fall through to the standard display
});
echo $a["x"];
restore_error_handler();
restore_error_handler();
echo $a["y"];                      // standard display again
$last = error_get_last();
echo $last["message"], " @", $last["line"], " type=", $last["type"], "\n";
error_clear_last();
var_dump(error_get_last());
trigger_error("custom notice");
trigger_error("custom warning", constant('E_USER_WARNING'));
trigger_error("custom deprecation", constant('E_USER_DEPRECATED'));
register_shutdown_function(function () { echo "shutdown\n"; });
echo "end\n";
