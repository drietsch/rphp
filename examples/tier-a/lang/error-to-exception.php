<?php
// A set_error_handler callback may throw: warnings become ErrorException
// at the emitting op (Symfony's ErrorHandler), catchable like any other.
set_error_handler(function ($no, $str, $file, $line) {
    throw new ErrorException($str, 0, $no, $file, $line);
});
try {
    $a = [];
    echo $a["missing"];
    echo "not reached\n";
} catch (ErrorException $e) {
    echo get_class($e), ": ", $e->getMessage(), " severity=", $e->getSeverity(), " line=", $e->getLine(), "\n";
}
try {
    echo "x" + 1;
} catch (TypeError $e) {
    echo "TypeError: ", $e->getMessage(), "\n";
}
function trig() { trigger_error("user warning", E_USER_WARNING); return "unreached"; }
try { trig(); } catch (ErrorException $e) { echo "user: ", $e->getMessage(), " ", $e->getSeverity(), "\n"; }
restore_error_handler();
echo @$undefined_after_restore, "restored\n";
echo "end\n";
