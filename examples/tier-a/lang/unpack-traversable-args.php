<?php
// Argument unpacking of a Traversable: a generator, an Iterator, an
// IteratorAggregate, and string keys becoming named arguments (8.1).
function gen() { yield 1; yield 2; yield 3; }
function f(...$args) { return implode(',', $args); }
echo f(...gen()), "\n";
echo f(...new ArrayIterator([4, 5])), "\n";
class Agg implements IteratorAggregate { public function getIterator(): Iterator { return new ArrayIterator([6, 7]); } }
echo f(...new Agg()), "\n";
function g($a, $b) { return "$a-$b"; }
function named() { yield 'b' => 'B'; yield 'a' => 'A'; }
echo g(...named()), "\n";
try { f(...new stdClass()); } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
