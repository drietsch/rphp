<?php
// self/parent/static inside a closure declared outside any class resolve
// at run time against the scope Closure::bind() gives it; unbound, php
// errors at the use, not at compile time.
class A { const X = 5; public static $p = 'sp'; static function m() { return 'A::m'; } function inst() { return 'inst'; } }
class B extends A { const X = 6; }
$f = function () { return self::class; }; try { var_dump($f()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$s = function () { return static::class; }; try { var_dump($s()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$g = function () { return parent::X; }; try { var_dump($g()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$h = fn() => new self; try { var_dump($h()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$i = function () { return self::$p; }; try { var_dump($i()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$j = fn() => self::m(); try { var_dump($j()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$k = fn() => self::X; try { var_dump($k()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$l = fn($x) => $x instanceof self; try { var_dump($l(new A)); } catch (Error $e) { echo $e->getMessage(), "\n"; }
var_dump(Closure::bind($f, null, A::class)(), Closure::bind($s, null, B::class)(), Closure::bind($g, null, B::class)(), Closure::bind($i, null, A::class)(), Closure::bind($j, null, A::class)(), Closure::bind($k, null, B::class)(), Closure::bind($l, null, A::class)(new B));
var_dump(get_class(Closure::bind($h, null, A::class)()));
$m = function () { return [self::X, static::X, $this->inst()]; }; var_dump(Closure::bind($m, new B, A::class)());
$n = static fn() => static::X; var_dump(Closure::bind($n, null, B::class)());
