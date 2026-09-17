<?php
// Tier-A: class-member expressions — static properties in every position,
// class constants, `::class`, static method calls and late static binding.
// Differentially tested against stock PHP 8.5.

class Counter {
    public const START = 10;
    public const LABEL = 'counter';

    public static $n = 0;
    public static $log = [];

    public static function bump($by = 1) {
        // `self::` is the lexical class, `static::` the called one.
        self::$n += $by;
        static::$log[] = static::class;
        return static::$n;
    }

    public static function name() {
        return static::class . '/' . self::class;
    }

    public static function label() {
        return static::LABEL;
    }
}

class SubCounter extends Counter {
    public const LABEL = 'sub';

    // Own storage: a redeclared static property does not share the parent's.
    public static $log = [];

    public static function tag() {
        return parent::LABEL . '>' . static::LABEL . ' (' . parent::class . ')';
    }
}

// ---- class constants --------------------------------------------------------

echo Counter::START, "\n";                  // 10
echo Counter::LABEL, ' ', SubCounter::LABEL, "\n";   // counter sub
// An inherited constant is visible through the subclass.
echo SubCounter::START, "\n";               // 10

$cls = 'SubCounter';
echo $cls::LABEL, "\n";                     // sub
$obj = new SubCounter();
echo $obj::LABEL, "\n";                     // sub
$which = 'LABEL';
echo Counter::{$which}, "\n";               // counter

// ---- ::class ----------------------------------------------------------------

echo Counter::class, ' ', SubCounter::class, "\n";   // Counter SubCounter
echo $obj::class, "\n";                              // SubCounter
echo Counter::name(), "\n";                          // Counter/Counter
echo SubCounter::name(), "\n";                       // SubCounter/Counter
echo SubCounter::tag(), "\n";                        // counter>sub (Counter)

// ---- static properties: read, write, compound, increment --------------------

Counter::$n = 1;
echo Counter::$n, "\n";                     // 1
Counter::$n += 4;
echo Counter::$n, "\n";                     // 5
Counter::$n++;
echo Counter::$n, "\n";                     // 6
echo ++Counter::$n, ' ', Counter::$n--, ' ', Counter::$n, "\n";   // 7 7 6
Counter::$n .= '!';
echo Counter::$n, "\n";                     // 6!

// A subclass that does not redeclare shares the parent's storage.
Counter::$n = 100;
echo SubCounter::$n, "\n";                  // 100
SubCounter::$n = 200;
echo Counter::$n, "\n";                     // 200

// Through a class-name string and through an object.
echo $cls::$n, ' ', $obj::$n, "\n";         // 200 200
$prop = 'n';
echo Counter::$$prop, "\n";                 // 200

// ---- static properties: isset / empty / ?? / ??= / by reference -------------

var_dump(isset(Counter::$n), isset(Counter::$missing));
var_dump(empty(Counter::$n), empty(Counter::$missing));
Counter::$n = 0;
var_dump(empty(Counter::$n));
var_dump(Counter::$missing ?? 'fallback');

class Nullable { public static $v = null; }
Nullable::$v ??= 'set';
Nullable::$v ??= 'again';
echo Nullable::$v, "\n";                    // set

Counter::$n = 5;
$alias = &Counter::$n;
$alias = 42;
echo Counter::$n, "\n";                     // 42
Counter::$n = 43;
echo $alias, "\n";                          // 43

// An array-valued static property is mutated in place.
Counter::$log = [];
SubCounter::$log = [];
Counter::$log[] = 'a';
Counter::$log['k'] = 'b';
echo implode(',', Counter::$log), ' ', count(SubCounter::$log), "\n";   // a,b 0

// A by-reference parameter binds the property's own cell.
function push_twice(array &$dst, $v) {
    $dst[] = $v;
    $dst[] = $v;
}
push_twice(Counter::$log, 'c');
echo implode(',', Counter::$log), "\n";     // a,b,c,c

// ---- static calls and late static binding -----------------------------------

Counter::$n = 0;
echo Counter::bump(), ' ', Counter::bump(2), "\n";        // 1 3
echo SubCounter::bump(10), "\n";                          // 13
echo implode(',', Counter::$log), "\n";                   // a,b,c,c,Counter,Counter
echo implode(',', SubCounter::$log), "\n";                // SubCounter

$method = 'bump';
echo Counter::$method(4), "\n";                           // 17
echo $cls::bump(1), "\n";                                 // 18
echo $obj::bump(1), "\n";                                 // 19
echo Counter::{'bump'}(1), "\n";                          // 20
echo $cls::$method(1), "\n";                              // 21

echo Counter::label(), ' ', SubCounter::label(), "\n";    // counter sub

// ---- new self / new static / new parent / new $cls --------------------------

class Maker {
    public static function selfOne() { return new self(); }
    public static function staticOne() { return new static(); }
}
class SubMaker extends Maker {
    public static function parentOne() { return new parent(); }
}

echo get_class(Maker::selfOne()), "\n";         // Maker
echo get_class(SubMaker::selfOne()), "\n";      // Maker
echo get_class(SubMaker::staticOne()), "\n";    // SubMaker
echo get_class(SubMaker::parentOne()), "\n";    // Maker
$mk = 'SubMaker';
echo get_class(new $mk()), "\n";                // SubMaker
echo get_class(new ($mk)()), "\n";              // SubMaker
$proto = new SubMaker();
echo get_class(new $proto()), "\n";             // SubMaker

// ---- instanceof -------------------------------------------------------------

var_dump($proto instanceof Maker, $proto instanceof SubMaker);
var_dump($proto instanceof $mk, $proto instanceof $proto);
var_dump($proto instanceof Counter);
var_dump('SubMaker' instanceof Maker);

// ---- first-class callable syntax (php 8.1) ----------------------------------

$len = strlen(...);
echo $len('abcd'), "\n";                    // 4

$bumpS = Counter::bump(...);
Counter::$n = 0;
echo $bumpS(7), "\n";                       // 7

$bumpC = $cls::bump(...);
echo $bumpC(1), "\n";                       // 8

$name = $obj::name(...);
echo $name(), "\n";                         // SubCounter/Counter

class Greeter {
    public $greeting = 'hi';
    public function greet($who) { return $this->greeting . ' ' . $who; }
}
$g = new Greeter();
$greet = $g->greet(...);
echo $greet('there'), "\n";                 // hi there
$mname = 'greet';
$greet2 = $g->$mname(...);
echo $greet2('again'), "\n";                // hi again

$dyn = 'strtoupper';
$up = $dyn(...);
echo $up('shout'), "\n";                    // SHOUT

var_dump($len instanceof Closure, is_callable($greet));

// ---- errors are catchable ---------------------------------------------------

try {
    echo Counter::$nope;
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    echo Counter::NOPE;
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    Counter::nope();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// An element of an array-valued static property can be unset (unsetting the
// property itself is an Error in php, so it is not exercised here).
Counter::$log = ['a' => 1, 'b' => 2];
unset(Counter::$log['a']);
echo count(Counter::$log), ' ', implode(',', Counter::$log), "\n";   // 1 2

echo "done\n";
