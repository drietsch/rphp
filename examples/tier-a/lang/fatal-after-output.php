<?php
// Output echoed before an uncaught error must reach stdout, then the
// php-cli fatal block follows on stdout; exit code 255.
echo "before\n";
$f = 'nope';
$f();
echo "never\n";
