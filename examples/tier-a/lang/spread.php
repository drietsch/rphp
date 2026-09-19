<?php
// Array unpacking: integer keys are renumbered, string keys preserved (8.1+),
// and a later string key overwrites an earlier one.
$a = [1, 2];
$b = ['k' => 'v', 3];
print_r([...$a, ...$b]);
print_r([0, ...$a, 9]);
print_r(['x' => 1, ...['x' => 2, 'y' => 3]]);
print_r([...[], ...[]]);

// Any Traversable unpacks, including a generator.
function gen() { yield 7; yield 8; }
print_r([...gen()]);

class Agg implements IteratorAggregate {
    public function getIterator(): Iterator { return new ArrayIterator(['a' => 1, 'b' => 2]); }
}
print_r([...new Agg()]);

// A plain object is a TypeError naming its class.
try {
    print_r([...[1], ...new stdClass()]);
} catch (TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
