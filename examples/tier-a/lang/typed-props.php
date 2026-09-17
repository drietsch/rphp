<?php
// Tier-A: declared property types and `readonly`. A typed property coerces on
// assignment under the assigning scope's strictness (this file is weak mode),
// starts out uninitialized when it has no default, and a readonly one may be
// written exactly once, from inside the class.
// Differentially tested against stock PHP 8.5.

class Box {
    public int $i = 0;
    public float $f = 0.0;
    public string $s = '';
    public bool $b = false;
    public ?string $maybe = null;
    public array $list = [];
    public int|string $either = 0;
    public $loose = 'anything';
}

$x = new Box();

// Weak mode juggles in php's preference order.
$x->i = "12";
var_dump($x->i);                       // int(12)
$x->i = 1.0;
var_dump($x->i);                       // int(1)
$x->i = true;
var_dump($x->i);                       // int(1)
$x->f = 3;
var_dump($x->f);                       // float(3) — int widens even in strict mode
$x->s = 5;
var_dump($x->s);                       // string(1) "5"
$x->b = "0";
var_dump($x->b);                       // bool(false)
$x->maybe = null;
var_dump($x->maybe);                   // NULL
$x->either = 1.5;                      // Deprecated: implicit float→int
var_dump($x->either);
$x->loose = [];                        // untyped: anything goes
var_dump($x->loose);

// A fractional float into an int property is deprecated, then truncated.
$x->i = 2.5;
var_dump($x->i);                       // int(2)

// Everything else is a TypeError naming the *declaring* class and php's
// spelling of the type.
function fails(callable $f) {
    try {
        $f();
        echo "ok\n";
    } catch (Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}

fails(function () use ($x) { $x->i = "abc"; });
fails(function () use ($x) { $x->i = null; });
fails(function () use ($x) { $x->i = []; });
fails(function () use ($x) { $x->i = new stdClass(); });
fails(function () use ($x) { $x->s = new stdClass(); });
fails(function () use ($x) { $x->list = "nope"; });
fails(function () use ($x) { $x->maybe = []; });

// A typed property with no default starts uninitialized: reading it is an
// Error, writing it is an ordinary first write, and the error names the class
// that declared it.
class Later {
    public int $n;
    public string $t;

    public function fill($v) {
        $this->n = $v;
    }
}

class Heir extends Later {
}

$l = new Heir();
fails(function () use ($l) { echo $l->n; });
var_dump(isset($l->n));                // bool(false)
var_dump(empty($l->n));                // bool(true)
$l->fill("7");
var_dump($l->n);                       // int(7)
var_dump(isset($l->n));                // bool(true)
unset($l->n);
fails(function () use ($l) { echo $l->n; });
$l->t = 'set directly';
var_dump($l->t);

// readonly: one initialization, from inside the declaring class's hierarchy.
class Once {
    public readonly int $id;
    public readonly string $tag;

    public function __construct($id) {
        $this->id = $id;
    }

    public function again($id) {
        $this->id = $id;
    }
}

class Sub extends Once {
    public function tagIt($t) {
        $this->tag = $t;          // readonly implies protected(set): a subclass
    }                             // may still perform the first write
}

$one = new Sub(1);
var_dump($one->id);                    // int(1)
fails(function () use ($one) { $one->again(2); });
fails(function () use ($one) { $one->id = 3; });
fails(function () use ($one) { unset($one->id); });
$one->tagIt('t');
var_dump($one->tag);                   // string(1) "t"
fails(function () use ($one) { $one->tagIt('u'); });

// A readonly property that was never written cannot be initialized from
// outside the class either — the wording differs from the "modify" case.
class Fresh {
    public readonly int $v;
}

$fresh = new Fresh();
fails(function () use ($fresh) { $fresh->v = 1; });
fails(function () use ($fresh) { unset($fresh->v); });
fails(function () use ($fresh) { echo $fresh->v; });

// readonly is checked before the type.
class Typed {
    public readonly int $n;

    public function __construct() {
        $this->n = 1;
    }
}

$t = new Typed();
fails(function () use ($t) { $t->n = "x"; });

// A typed property reached through __set is the accessor's problem, not the
// type system's.
class Guarded {
    private int $hidden = 5;

    public function __get($k) { return "got:$k"; }
    public function __set($k, $v) { echo "set($k)"; }
}

$g = new Guarded();
var_dump($g->hidden);                  // string(10) "got:hidden"
$g->hidden = "not an int";             // set(hidden)
echo "\n";
