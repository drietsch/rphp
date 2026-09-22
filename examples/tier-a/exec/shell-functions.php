<?php
// exec / shell_exec / system / passthru: what comes back and what is echoed.
var_dump(exec('printf "a\nb  \nc\n"', $lines, $code), $lines, $code);
var_dump(exec('exit 4', $more, $code2), $more, $code2);
var_dump(shell_exec('printf "x\ny\n"'), shell_exec('true'));
var_dump(system('printf "one\ntwo\n"', $c), $c);
var_dump(passthru('printf "raw\n"', $c2), $c2);
$lines = ['keep'];
exec('echo appended', $lines);
var_dump($lines);
