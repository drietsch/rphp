<?php
// Tier-A: every callable spelling resolves through one path, so a direct
// `$f()`, `call_user_func`, `array_map` and `usort` always agree; plus
// first-class callable syntax and closure rebinding. Differentially tested
// against stock PHP 8.5.

function twice($x) { return $x * 2; }

class Math {
    public $bias = 10;
    public function add($n) { return $n + $this->bias; }
    public static function square($n) { return $n * $n; }
    public function __invoke($n) { return "inv($n)"; }
    private function secret($n) { return "secret($n)"; }
    public function secretFirstClass() { return $this->secret(...); }
}

class Sub extends Math {
    public function add($n) { return "sub:" . parent::add($n); }
}

$m = new Math();
$s = new Sub();

// ---- the spellings, each invoked four ways -------------------------------
$callables = [
    'twice',              // a user function by name
    'Math::square',       // a static method by string
];
foreach ($callables as $c) {
    echo $c, ' ', call_user_func($c, 5), ' ', implode(',', array_map($c, [2, 3])), "\n";
}

echo call_user_func([$m, 'add'], 1), "\n";          // 11
echo call_user_func(['Math', 'square'], 4), "\n";   // 16
echo call_user_func([$m, 'square'], 5), "\n";       // 25 (static through an instance)
echo call_user_func($m, 6), "\n";                   // inv(6)
echo call_user_func_array([$m, 'add'], [2]), "\n";  // 12

// A closure, called directly and through a builtin.
$triple = function ($x) { return $x * 3; };
echo $triple(4), "\n";                              // 12
echo implode(',', array_map($triple, [1, 2])), "\n"; // 3,6
echo call_user_func($triple, 5), "\n";              // 15

// A subclass override, and `parent::` from inside it.
echo $s->add(1), "\n";                              // sub:11
echo implode(',', array_map([$s, 'add'], [1, 2])), "\n"; // sub:11,sub:12

// ---- higher-order builtins ------------------------------------------------
$nums = [5, 3, 9, 1];
usort($nums, function ($a, $b) { return $a <=> $b; });
echo implode(',', $nums), "\n";                     // 1,3,5,9

$words = ['pear', 'fig', 'banana'];
usort($words, 'strcmp');
echo implode(',', $words), "\n";                    // banana,fig,pear

echo implode(',', array_map([$m, 'add'], [1, 2, 3])), "\n";   // 11,12,13
echo implode(',', array_map(['Math', 'square'], [2, 3])), "\n"; // 4,9
echo implode(',', array_map('Math::square', [4])), "\n";       // 16

// ---- first-class callable syntax ------------------------------------------
$len = strlen(...);
echo $len('abcd'), "\n";                            // 4
echo implode(',', array_map($len, ['a', 'bb'])), "\n"; // 1,2

$t = twice(...);
echo $t(7), "\n";                                   // 14

$add = $m->add(...);
echo $add(5), "\n";                                 // 15
echo implode(',', array_map($add, [0, 1])), "\n";   // 10,11

$sq = Math::square(...);
echo $sq(6), "\n";                                  // 36

$again = $t(...);                                   // over an existing closure
echo $again(8), "\n";                               // 16

// A first-class callable taken inside the class keeps reaching the private
// method after it escapes.
$priv = $m->secretFirstClass();
echo $priv(2), "\n";                                // secret(2)

// ---- closure rebinding ----------------------------------------------------
class Vault {
    private $code = 42;
    public $label = 'v';
}
$peek = function () { return $this->code; };
$bound = $peek->bindTo(new Vault(), 'Vault');
echo $bound(), "\n";                                // 42

$label = (function () { return $this->label; })->bindTo(new Vault(), Vault::class);
echo $label(), "\n";                                // v

$sum = function ($extra) { return $this->code + $extra; };
echo $sum->call(new Vault(), 8), "\n";              // 50

// Rebinding never mutates the original closure.
$v = new Vault();
$one = (function () { return $this->label; })->bindTo($v, 'Vault');
echo $one(), "\n";                                  // v

// `Closure::bind` is the static spelling of the same thing.
$viaBind = Closure::bind($peek, new Vault(), 'Vault');
echo $viaBind(), "\n";                              // 42

// `Closure::fromCallable` accepts every spelling the engine resolves.
$up = Closure::fromCallable('strtoupper');
echo $up('hi'), "\n";                               // HI
$fc1 = Closure::fromCallable([$m, 'add']);
echo $fc1(5), "\n";                                 // 15
$fc2 = Closure::fromCallable('Math::square');
echo $fc2(3), "\n";                                 // 9
$fc3 = Closure::fromCallable($triple);
echo $fc3(3), "\n";                                 // 9
echo implode(',', array_map(Closure::fromCallable('twice'), [1, 2])), "\n"; // 2,4

try {
    Closure::fromCallable('definitely_not_here');
} catch (TypeError $e) {
    echo $e->getMessage(), "\n";
}

// ---- errors ---------------------------------------------------------------
try {
    $gone = 'no_such_function_here';
    $gone();
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
try {
    $bad = [$m, 'nothingHere'];
    $bad(1);
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
try {
    $nc = new Vault();
    $nc(1);
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
try {
    $missing = $add;
    $missing();                                     // too few arguments
} catch (ArgumentCountError $e) {
    echo get_class($e), "\n";
}
