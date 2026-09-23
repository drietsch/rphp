<?php
// The order php lists interfaces in: user classes append each declared
// interface's own list back to front after all the declared ones; internal
// classes interleave it; an internal parent's list is inherited reversed.
interface A {} interface B extends A {} interface C {} interface D extends B, C {}
class P implements C {} class Q extends P implements B {} class R extends Q implements D, Countable { function count(): int { return 0; } }
echo implode(',', class_implements('Q')), "\n", implode(',', class_implements('R')), "\n";
echo implode(',', class_implements(new ArrayIterator([]))), "\n", implode(',', class_implements('ArrayObject')), "\n";
echo implode(',', (new ReflectionClass('R'))->getInterfaceNames()), "\n";
echo implode(',', class_implements('D')), "\n";
var_dump(new R instanceof A);
interface A0 {} interface A1 extends A0 {} interface A2 {} interface B2 extends A1, A2 {}
interface E extends SeekableIterator {} interface F extends B2, C {}
abstract class X implements B, C {} abstract class Y implements SeekableIterator, Countable {} abstract class Z implements B2 {} abstract class W implements C, B2, A {}
abstract class X2 extends X implements B2 {}
class S { function __toString(): string { return ""; } } interface I {} class T implements I { function __toString(): string { return ""; } }
foreach (["B2","E","F","X","Y","Z","W","X2","S","T"] as $c) echo $c, ": ", implode(",", class_implements($c)), "\n";
foreach (["SeekableIterator","RecursiveIterator","OuterIterator","Iterator","IteratorAggregate","ArrayIterator","RecursiveArrayIterator","IteratorIterator","FilterIterator","RecursiveIteratorIterator","SplObjectStorage","SplDoublyLinkedList","SplQueue","SplFixedArray","Generator","DateTimeImmutable","DatePeriod","Exception","ErrorException","WeakMap","SplFileObject","DirectoryIterator","RecursiveDirectoryIterator","BackedEnum","Stringable"] as $c) echo $c, ": ", implode(",", class_implements($c)), "\n";
class M extends Exception {} class N extends RuntimeException {} class U extends ArrayIterator implements Countable {} class U2 extends ArrayIterator {} class S2 extends Exception { function __toString(): string { return ""; } }
foreach (["M","N","U","U2","S2"] as $c) echo $c, ": ", implode(",", class_implements($c)), "\n";
