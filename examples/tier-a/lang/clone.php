<?php
// Tier-A: `clone` — the shallow copy, `__clone` on the copy, and the
// `(object)` / `(array)` casts with php's mangled property keys.
// Differentially tested against stock PHP 8.5.

class Node {
    public $v = 1;
    public $next = null;
    public function __clone() {
        if ($this->next !== null) {
            $this->next = clone $this->next;
        }
    }
}

// Without `__clone` the copy shares every object-valued property; `Node`
// deep-copies its `next`, so the two chains are independent.
$a = new Node();
$a->next = new Node();
$a->next->v = 2;
$b = clone $a;
$b->next->v = 99;
echo $a->next->v, ' ', $b->next->v, "\n";      // 2 99

class Shallow {
    public $v = 1;
    public $ref = null;
}
$c = new Shallow();
$c->ref = new Shallow();
$d = clone $c;
$d->ref->v = 7;
echo $c->ref->v, ' ', $d->ref->v, "\n";        // 7 7
var_dump($c->ref === $d->ref);                  // true
var_dump($c === $d);                            // false

// `__clone` runs on the copy, never on the original.
class Counted {
    public $tag = 'orig';
    public function __clone() { $this->tag = 'copy'; }
}
$e = new Counted();
$f = clone $e;
echo $e->tag, ' ', $f->tag, "\n";              // orig copy

// It is inherited like any other method.
class CountedChild extends Counted {}
$g = clone new CountedChild();
echo $g->tag, ' ', get_class($g), "\n";        // copy CountedChild

// Scalars and arrays are copied by value.
class Holder {
    public $n = 1;
    public $list = [1, 2];
}
$h = new Holder();
$i = clone $h;
$i->n = 5;
$i->list[] = 3;
echo $h->n, ' ', count($h->list), ' ', $i->n, ' ', count($i->list), "\n"; // 1 2 5 3

// A private `__clone` cannot be reached from outside.
class Sealed {
    private function __clone() {}
}
try {
    $x = clone new Sealed();
} catch (Error $er) {
    echo $er->getMessage(), "\n";              // Call to private method Sealed::__clone() from global scope
}

// `clone` of a non-object.
try {
    $n = 5;
    $y = clone $n;
} catch (Error $er) {
    echo get_class($er), ': ', $er->getMessage(), "\n";
}

// ---- the casts -------------------------------------------------------------
class Vis {
    private $p = 1;
    protected $q = 2;
    public $r = 3;
}
// `(array)` mangles private (`\0Class\0name`) and protected (`\0*\0name`).
foreach ((array) new Vis() as $k => $v) {
    echo str_replace("\0", '@', $k), ' => ', $v, "\n";
}
echo count((array) new Vis()), "\n";           // 3

// `(object)` of an array is a stdClass, and `(array)` of one round-trips.
$o = (object) ['x' => 1, 'y' => 'two'];
echo get_class($o), ' ', $o->x, ' ', $o->y, "\n";
var_dump((array) $o);

// Scalars, null and lists. (Object ids are deliberately not printed: they
// are an allocation detail, not a language guarantee.)
var_dump((array) 'str');
var_dump((array) null);
echo get_class((object) null), ' ', count((array) (object) null), "\n";
echo ((object) 'hi')->scalar, "\n";
$list = (object) [1, 2];
foreach ((array) $list as $k => $v) {
    echo $k, '=', $v, ' ';
}
echo "\n";

// Casting an object to object is identity.
$v = new Vis();
var_dump((object) $v === $v);                  // true

// A cloned stdClass is a separate object.
$p1 = (object) ['n' => 1];
$p2 = clone $p1;
$p2->n = 2;
echo $p1->n, ' ', $p2->n, "\n";                // 1 2
var_dump($p1 === $p2);                          // false
