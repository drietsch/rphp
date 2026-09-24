<?php
class A {
    const NAME = 'A';
    public static function test() {
        echo static::NAME, " ", get_called_class(), " ", implode(",", func_get_args()), "\n";
        return 42;
    }
    public static function named(int $x = 0, int $y = 0) { echo static::class, " x=$x y=$y\n"; }
}
class B extends A {
    const NAME = 'B';
    public static function test() {
        var_dump(forward_static_call(['A', 'test'], 'more', 'args'));
        forward_static_call('test', 'other', 'args');
        forward_static_call_array(['A', 'test'], ['x', 'y']);
        forward_static_call_array('A::named', ['y' => 2]);
        forward_static_call_array('A::named', [1, 'y' => 3]);
        forward_static_call('A::named', y: 5);
        try { forward_static_call_array('A::test', ['k' => 1]); } catch (\Error $e) { echo $e->getMessage(), "\n"; }
        try { forward_static_call_array('A::named', ['y' => 1, 2]); } catch (\Error $e) { echo $e->getMessage(), "\n"; }
        forward_static_call(fn() => print(static::class . "\n"));
        forward_static_call([new C, 'inst']);
        forward_static_call(['C', 'test']);
        call_user_func(['A', 'test'], 'via call_user_func');
    }
}
class C {
    public function inst() { echo "inst ", static::class, "\n"; }
    public static function test() { echo "C ", static::class, "\n"; }
}
class D extends B {
    const NAME = 'D';
    public static function run() {
        forward_static_call(['A', 'test'], 'from D');
        forward_static_call(['B', 'test']);
    }
}
function test() { echo "fn ", implode(",", func_get_args()), "\n"; }

B::test();
D::run();
try { forward_static_call('test', 1); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { forward_static_call('nope'); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
function g() { forward_static_call('test', 2); }
try { g(); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
class E {
    static function t() {
        foreach (['nope', 'Nope::x', 'A::nope', ['A', 'nope'], ['Nope', 'x'], [1, 2], [1], 5] as $cb) {
            try { forward_static_call($cb); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        }
        try { forward_static_call_array('test', 5); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        var_dump(forward_static_call_array('strtoupper', ['abc']));
        var_dump(forward_static_call('max', 3, 9, 4));
    }
}
E::t();
try { forward_static_call(); } catch (\ArgumentCountError $e) { echo $e->getMessage(), "\n"; }
