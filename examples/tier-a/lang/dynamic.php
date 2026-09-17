<?php
// Late binding at run time: `new $cls` / `new (expr)`, `$obj::class`,
// `[$obj, 'm']` / `['C', 'm']` / `'C::m'` callables through every calling
// path, `$x instanceof $y` with strings and objects, dynamic property and
// method names, conditional function and class declarations, and the
// class-introspection builtins over the class table.

class Base {
    public $name = 'base';
    protected $hidden = 'h';
    public function who() { return static::class . ':' . $this->name; }
    public function twice($x) { return $x * 2; }
    protected function secret() { return 'secret'; }
    public function reveal() { return $this->secret() . '/' . self::class; }
}
class Child extends Base {
    public $name = 'child';
    public function who() { return 'child-' . parent::who(); }
}

// dynamic instantiation
$cls = 'Child';
$c1 = new $cls;
$c2 = new (strtolower('CHILD') === 'child' ? 'Child' : 'Base');
$c3 = new $c1;                                 // an object works as a class reference
echo $c1->who(), ' ', $c2->who(), ' ', $c3->who(), ' ', (new Base)->who(), "\n";
echo $c1::class, ' ', Child::class, ' ', Base::class, ' ', get_class($c1), "\n";

// callables in every shape, through direct calls and the higher-order natives
$pair = [$c1, 'twice'];
$str = 'Base::twice';
echo $pair(2), ' ', call_user_func($pair, 3), ' ', call_user_func_array($pair, [4]), ' ', implode(',', array_map($pair, [5, 6])), "\n";
$m = 'twice'; $p = 'name';
echo $c1->$m(7), ' ', $c1->{'tw' . 'ice'}(8), ' ', $c1->$p, ' ', $c1->{'name'}, "\n";
var_dump(is_callable($pair), is_callable([$c1, 'nope']), is_callable($str), is_callable('Child::who'), is_callable('strlen'), is_callable('nope'));
echo call_user_func([new Child, 'reveal']), "\n";

// instanceof with dynamic right-hand sides
$base = 'Base'; $childObj = new Child; $nope = 'Nope';
var_dump($childObj instanceof $base, $childObj instanceof $childObj, $childObj instanceof $nope, $childObj instanceof Child, 'str' instanceof Base, (new Base) instanceof Child);
var_dump($childObj instanceof stdClass, (fn() => 1) instanceof Closure);

// conditional declarations happen when the statement runs
var_dump(function_exists('maybe'), class_exists('Maybe'));
if (true) {
    function maybe() { return 'maybe!'; }
    class Maybe extends Base { public $name = 'maybe'; }
}
var_dump(function_exists('maybe'), class_exists('Maybe'), class_exists('maybe'));
echo maybe(), ' ', (new Maybe)->who(), "\n";
if (false) {
    function never() {}
}
var_dump(function_exists('never'));

// introspection
var_dump(get_parent_class($childObj), get_parent_class('Base'), method_exists($childObj, 'WHO'), method_exists('Base', 'secret'), method_exists('Base', 'nope'));
var_dump(property_exists('Base', 'hidden'), property_exists($childObj, 'nope'), is_a($childObj, 'Base'), is_a('Child', 'Base'), is_a('Child', 'Base', true), is_subclass_of($childObj, 'Base'), is_subclass_of('Base', 'Base'));
var_dump(get_object_vars($childObj), get_class_methods('Child'));
$std = new stdClass; $std->a = 1; $std->{'b c'} = 2;
var_dump(get_object_vars($std), (array) $std, get_class($std), spl_object_id($std) > 0);

// dynamic function names resolve natives and user functions alike
$fn = 'strtoupper';
$user = 'maybe';
echo $fn('x'), $user(), ' ', array_sum(array_map('intval', ['1', '2'])), "\n";
