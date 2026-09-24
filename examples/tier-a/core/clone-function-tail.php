<?php
// php 8.5: clone() is a function, with a $withProperties array ("clone with").
var_dump(function_exists('clone'), is_callable('clone'));

$o = new stdClass;
$o->a = 1;
var_dump(clone($o, ['a' => 2, 'b' => 3]));
var_dump(clone(object: $o, withProperties: ['c' => 1]));

final class Point
{
    public function __construct(public readonly int $x, public readonly int $y = 0) {}

    public function withX(int $x): static
    {
        // Inside the class a readonly property may be re-initialized once.
        return clone($this, ['x' => $x]);
    }

    public function __clone()
    {
        echo "__clone\n";
    }
}
$p = new Point(1, 2);
var_dump($p->withX(5));
try {
    clone($p, ['x' => 9]);
} catch (\Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

class Secret
{
    private $hidden = 1;
    protected $prot = 2;
}
try {
    clone(new Secret, ['hidden' => 2]);
} catch (\Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

$f = 'clone';
$copy = $f($o);
var_dump($copy == $o, $copy === $o);
var_dump(count(array_map('clone', [$o, $o])));
$c = clone(...);
var_dump($c($o) == $o);

foreach ([1, 'str', null] as $bad) {
    try {
        $f($bad);
    } catch (\TypeError $e) {
        echo $e->getMessage(), "\n";
    }
}
try {
    $f($o, 5);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
echo new ReflectionFunction('clone');
