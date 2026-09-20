<?php
// An uninitialized typed property has no value, and every viewer says so
// its own way: var_dump prints `uninitialized(T)`, the rest skip the slot.
class A {
    public string $b;
    public $c;
    protected ?int $d;
    private array $e;
    public readonly int $f;
    public int|string $g;
    public ?A $h;
}
$a = new A;
var_dump($a);
print_r($a); echo "\n";
var_export($a); echo "\n";
var_dump((array) $a, get_object_vars($a), json_encode($a), serialize($a), count((array) $a));
foreach ($a as $k => $v) { echo "$k\n"; }
var_dump(isset($a->b), property_exists($a, 'b'));
try { echo $a->b; } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
$a->b = 'now';
unset($a->c);
var_dump($a);
unset($a->b);
var_dump($a, isset($a->b));
