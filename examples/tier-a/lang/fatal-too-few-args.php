<?php
// ArgumentCountError with php's exact text: the caller's file/line in the
// message, the callee in the trace. Exit code 255.
function needs_two($a, $b) {
    return $a + $b;
}
echo "before\n";
echo needs_two(1);
