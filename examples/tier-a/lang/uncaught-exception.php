<?php
// An uncaught exception prints php-cli's fatal block on stdout (message,
// file:line, stack trace, thrown-in) and exits 255; a finally block still
// runs on the way out.
function inner($v) { throw new RuntimeException("not caught " . $v); }
function outer() { try { inner([1, 2]); } finally { echo "finally ran\n"; } }
echo "start\n";
outer();
echo "never\n";
