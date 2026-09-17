<?php
// Tier-A differential: the interfaces php's object model assumes exist
// (zend_interfaces.c + spl_iterators.c) — that they are declared as
// interfaces, that their parents are right, and that a user class can
// implement them and be seen through them.

$ifaces = [
    'Traversable', 'Iterator', 'IteratorAggregate', 'ArrayAccess', 'Countable',
    'Stringable', 'JsonSerializable', 'Serializable', 'UnitEnum', 'BackedEnum',
    'SeekableIterator',
];
foreach ($ifaces as $i) {
    // An interface is not a class: interface_exists yes, class_exists no.
    echo $i, ' ', (int) interface_exists($i), (int) class_exists($i), "\n";
}

// The parent relationships the engine relies on.
var_dump(is_subclass_of('Iterator', 'Traversable'));
var_dump(is_subclass_of('IteratorAggregate', 'Traversable'));
var_dump(is_subclass_of('SeekableIterator', 'Iterator'));
var_dump(is_subclass_of('SeekableIterator', 'Traversable'));
var_dump(is_subclass_of('BackedEnum', 'UnitEnum'));
var_dump(is_subclass_of('Traversable', 'Iterator'));

// A user class linking against them.
class Bag implements Iterator, Countable, ArrayAccess, JsonSerializable
{
    private array $items = ['a' => 1, 'b' => 2, 'c' => 3];
    private int $i = 0;

    public function current(): mixed { return array_values($this->items)[$this->i]; }
    public function key(): mixed { return array_keys($this->items)[$this->i]; }
    public function next(): void { $this->i++; }
    public function rewind(): void { $this->i = 0; }
    public function valid(): bool { return $this->i < count($this->items); }

    public function count(): int { return count($this->items); }

    public function offsetExists(mixed $o): bool { return isset($this->items[$o]); }
    public function offsetGet(mixed $o): mixed { return $this->items[$o]; }
    public function offsetSet(mixed $o, mixed $v): void
    {
        if ($o === null) { $this->items[] = $v; } else { $this->items[$o] = $v; }
    }
    public function offsetUnset(mixed $o): void { unset($this->items[$o]); }

    public function jsonSerialize(): mixed { return $this->items; }
}

$bag = new Bag;
var_dump($bag instanceof Iterator, $bag instanceof Traversable);
var_dump($bag instanceof Countable, $bag instanceof ArrayAccess);
var_dump($bag instanceof IteratorAggregate);

echo count($bag), "\n";
echo $bag['b'], "\n";
var_dump(isset($bag['a']), isset($bag['zz']));
$bag['d'] = 4;
$bag[] = 5;
unset($bag['a']);
echo count($bag), "\n";
echo json_encode($bag), "\n";

foreach ($bag as $k => $v) {
    echo $k, '=', $v, ' ';
}
echo "\n";

// A class with __toString is a Stringable in php 8.
class Label implements Stringable
{
    public function __toString(): string { return 'label'; }
}
$l = new Label;
var_dump($l instanceof Stringable);
echo $l, "\n";

// The interface set a class reports (sorted: the flattening order is an
// implementation detail, the membership is not).
$impl = array_values(class_implements('Bag'));
sort($impl);
print_r($impl);
