<?php
// Trait composition: php *copies* a trait's members into the using class, so
// the declaring scope of a trait method is the using class (`self::`,
// `parent::`, visibility), each using class gets its own copy of a trait's
// static properties, and the member tables list the class's own members first
// and the trait's after them. Covers copy-in, precedence against the parent
// and against the class body, `insteadof` / `as` conflict resolution,
// per-class statics, trait constants, abstract and static trait members, and
// nested `use`.

// ---- copy-in: the declaring scope becomes the using class -----------------

trait Counter
{
    public static int $n = 0;
    public int $inst = 5;

    const TAG = 'counter';
    const LOUD = self::TAG . '!';

    public static function bump(): int
    {
        static::$n++;
        return static::$n;
    }

    public function who(): string
    {
        return self::class . '/' . static::class . '/' . self::TAG;
    }
}

class Alpha { use Counter; }
class Beta  { use Counter; }

Alpha::bump();
Alpha::bump();
Beta::bump();
echo 'Alpha::$n=', Alpha::$n, ' Beta::$n=', Beta::$n, "\n";   // 2 / 1 — not shared
echo Alpha::TAG, ' ', Alpha::LOUD, "\n";                      // counter counter!
$a = new Alpha();
echo $a->who(), ' inst=', $a->inst, "\n";                     // Alpha/Alpha/counter
var_dump(property_exists('Alpha', 'inst'), method_exists('Alpha', 'bump'));
echo implode(',', get_class_methods('Alpha')), "\n";          // bump,who

// `self::` in a copied constant is re-evaluated in the using class, so a class
// that redeclares the constant the trait built on sees its own value.
trait Tagged
{
    const BASE = 'base';
    const LABEL = self::BASE . '-label';
}
class Plain { use Tagged; }
echo Plain::LABEL, "\n";                                       // base-label

// ---- precedence -----------------------------------------------------------

class Parental
{
    public function m() { return 'parent-m'; }
    public function n() { return 'parent-n'; }
    public $p = 'parent-p';
    public static $s = 'parent-s';
    const K = 'parent-K';
}

trait Overrider
{
    public function m() { return 'trait-m'; }
    public function n() { return 'trait-n'; }
    public $p = 'trait-p';
    public static $s = 'trait-s';
    const K = 'trait-K';
}

class Child extends Parental
{
    use Overrider;

    // The class body wins over the trait; the trait wins over the parent.
    public function n() { return 'child-n'; }
}

$c = new Child();
echo $c->m(), ' ', $c->n(), ' ', $c->p, ' ', Child::$s, ' ', Child::K, "\n";
// trait-m child-n trait-p trait-s trait-K

// The parent keeps its own static cell: the trait's copy replaced it in Child.
echo Parental::$s, '/', Child::$s, "\n";                       // parent-s/trait-s

// A trait method can still reach the parent it was composed into.
trait Chained
{
    public function m() { return parent::m() . '+trait'; }
}
class Linked extends Parental { use Chained; }
echo (new Linked())->m(), "\n";                                // parent-m+trait

// ---- conflict resolution: insteadof and as --------------------------------

trait Hello
{
    public function say() { return 'Hello'; }
    public function shout() { return strtoupper($this->say()); }
}

trait World
{
    public function say() { return 'World'; }
}

class Greeting
{
    use Hello, World {
        Hello::say insteadof World;   // pick a winner for the collision
        World::say as sayWorld;       // keep the loser under another name
        Hello::say as public greet;   // a second name for the winner
        shout as protected;           // change visibility in place
    }

    public function all()
    {
        return $this->say() . ' ' . $this->sayWorld() . ' '
             . $this->shout() . ' ' . $this->greet();
    }
}

$g = new Greeting();
echo $g->all(), "\n";                                          // Hello World HELLO Hello
// Aliases are applied just before the method they rename, and the class's own
// methods come first; `shout` is protected, so it is not listed here.
echo implode(',', get_class_methods('Greeting')), "\n";        // all,greet,say,sayWorld
var_dump(method_exists('Greeting', 'shout'));

// A method the class body declares resolves a collision on its own.
class Decides
{
    use Hello, World;
    public function say() { return 'mine'; }
}
echo (new Decides())->say(), "\n";                             // mine

// Aliasing a static method keeps it static.
trait Maker
{
    public static function make() { return 'made by ' . static::class; }
}
class Factory
{
    use Maker { make as build; }
}
echo Factory::build(), ' / ', Factory::make(), "\n";

// ---- abstract trait members ----------------------------------------------

trait NeedsName
{
    abstract public function name(): string;

    public function hello(): string
    {
        return 'hi ' . $this->name();
    }
}

class Named
{
    use NeedsName;
    public function name(): string { return 'named'; }
}
echo (new Named())->hello(), "\n";                             // hi named

// A body from another trait satisfies an abstract declaration, whichever order
// the two traits appear in.
trait HasName
{
    public function name(): string { return 'from-trait'; }
}
class Composed { use NeedsName, HasName; }
class Composed2 { use HasName, NeedsName; }
echo (new Composed())->hello(), ' ', (new Composed2())->hello(), "\n";

// ---- nested use -----------------------------------------------------------

trait Inner
{
    const IK = 'ik';
    public static $is = 9;
    public $ip = 'ip';
    public function i() { return 'i'; }
}

trait Outer
{
    use Inner;
    public function o() { return 'o'; }
}

class Nested { use Outer; }
$n = new Nested();
echo $n->i(), $n->o(), ' ', Nested::IK, ' ', Nested::$is, ' ', $n->ip, "\n";
echo implode(',', get_class_methods('Nested')), "\n";          // o,i

// The same trait reached twice through the composition is not a conflict.
class Twice { use Inner, Outer; }
echo (new Twice())->i(), (new Twice())->o(), "\n";             // io

// ---- member order ---------------------------------------------------------

class Base2 { public $b1 = 1; public $shared = 'base'; }
trait Props { public $t1 = 't'; public $shared = 'trait'; public $t2 = 't2'; }
class Ordered extends Base2
{
    public $c1 = 'c';
    use Props;
    public $c2 = 'c2';
}
// Parent slots first, then the class body's own, then the trait's; a name the
// parent declared keeps its slot and takes the trait's default.
print_r(get_object_vars(new Ordered()));
