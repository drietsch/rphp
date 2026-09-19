<?php
// An anonymous class is a declaration like any other; php names it
// `<prefix>@anonymous\0<file>:<line>$<n>` and shows only the part before the
// NUL wherever it formats a name with `%s`.
$a = new class { public $p = 1; };
var_dump($a);
print_r($a);
echo "\n", get_debug_type($a), "\n";
echo json_encode($a), "\n";

// The prefix is the parent, else the first interface, else `class`.
class P { public function hi(): string { return "P::hi"; } }
interface I { public function tag(): string; }
$b = new class extends P implements I {
    public const WHO = 'b';
    public function tag(): string { return self::WHO; }
};
echo $b->hi(), " ", $b->tag(), "\n";
var_dump($b instanceof P, $b instanceof I, $b instanceof Stringable);
var_dump(get_parent_class($b), class_implements($b));

// Constructor arguments, promotion and a closure over the enclosing scope.
$n = 7;
$c = new class($n) {
    public function __construct(public readonly int $x) {}
    public function twice(): int { return $this->x * 2; }
};
echo $c->x, " ", $c->twice(), "\n";
try {
    $c->x = 1;
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// One class entry per site, however often the site runs: the two objects
// have the same class.
$made = [];
for ($i = 0; $i < 3; $i++) {
    $made[] = new class { public function who(): string { return static::class === get_class($this) ? 'same' : 'differs'; } };
}
var_dump(get_class($made[0]) === get_class($made[2]), $made[1]->who(), count($made));

// Two different sites are two different classes.
$x = new class {};
$y = new class {};
var_dump(get_class($x) === get_class($y));

// Messages name it the short way; `serialize` refuses it outright.
try { $a->nope(); } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { echo "$a"; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { serialize($a); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }

// Anonymous classes nest, and one can be declared inside a method.
$outer = new class {
    public function inner(): object { return new class { public $tag = 'inner'; }; }
};
var_dump($outer->inner()->tag);
