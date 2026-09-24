<?php
// The coercions of exit()'s `string|int` parameter in weak mode: true is 1,
// a whole float is an int, a fractional one is deprecated and truncated,
// null is deprecated and 0. The last one wins the process status.
$f = 'exit';
echo "calling through a variable\n";
try {
    exit(null === 1 ? 0 : new ArrayObject([]));
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
echo "and now 2.5\n";
$f(2.5);
