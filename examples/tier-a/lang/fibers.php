<?php
// Fibers (php 8.1): start/suspend/resume/throw, the transfer values in
// both directions, status queries, `getReturn()`'s three refusals,
// nesting, suspends inside `foreach`/`try`/`finally` and inside a call's
// argument list, exceptions crossing the fiber boundary both ways, and
// `FiberError`'s reserved constructor.
function t($f) { try { $r = $f(); echo "=> "; var_dump($r); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
var_dump(Fiber::getCurrent());
$fiber = new Fiber(function (string $x): string {
    echo "in fiber: $x\n";
    var_dump(Fiber::getCurrent() instanceof Fiber);
    $y = Fiber::suspend('first');
    echo "resumed with: "; var_dump($y);
    try {
        $z = Fiber::suspend('second');
    } catch (Exception $e) {
        echo "caught in fiber: ", $e->getMessage(), "\n";
        $z = 'after-catch';
    }
    echo "z: $z\n";
    return "done:$x";
});
var_dump($fiber->isStarted(), $fiber->isSuspended(), $fiber->isRunning(), $fiber->isTerminated());
t(fn() => $fiber->getReturn());
t(fn() => $fiber->resume(1));
$v = $fiber->start('arg');
var_dump($v);
var_dump($fiber->isStarted(), $fiber->isSuspended(), $fiber->isRunning(), $fiber->isTerminated());
t(fn() => $fiber->start('again'));
t(fn() => $fiber->getReturn());
$v = $fiber->resume('R1');
var_dump($v);
$v = $fiber->throw(new Exception('boom'));
var_dump($v);
var_dump($fiber->isTerminated(), $fiber->getReturn());
t(fn() => $fiber->resume());
t(fn() => Fiber::suspend());

// exception out of a fiber
$f2 = new Fiber(function () { Fiber::suspend(); throw new RuntimeException('from fiber'); });
$f2->start();
t(fn() => $f2->resume());
var_dump($f2->isTerminated());
t(fn() => $f2->getReturn());

// nested fibers + value passing through calls + finally
function work($n) { $got = Fiber::suspend($n * 10); return $got + 1; }
$outer = new Fiber(function () {
    $inner = new Fiber(function () {
        $a = work(1);
        $b = work(2);
        return [$a, $b];
    });
    $x = $inner->start();          // 10
    $y = Fiber::suspend("outer-sees:$x");
    $x2 = $inner->resume($y);      // work(1) returns $y+1; then work(2) suspends 20
    $r = $inner->resume(100);      // work(2) returns 101 → inner returns
    return [$inner->getReturn(), $x2, $r];
});
var_dump($outer->start());
var_dump($outer->resume(5));
var_dump($outer->getReturn());

// suspend inside foreach/try/finally, with pending call args
$f3 = new Fiber(function () {
    $out = [];
    foreach ([1, 2, 3] as $i) {
        try {
            $out[] = strtoupper(Fiber::suspend($i)) . "-" . str_repeat('x', Fiber::suspend($i * 100));
        } finally {
            $out[] = "f$i";
        }
    }
    return $out;
});
$s = $f3->start();
while (!$f3->isTerminated()) { $s = $f3->resume(is_int($s) && $s < 100 ? "v$s" : 2); }
var_dump($f3->getReturn());

// FiberError construct
t(fn() => new FiberError('x'));
t(fn() => (new Fiber(fn() => 1))->throw(new Exception('e')));
$f4 = new Fiber(fn() => Fiber::suspend(1));
$f4->start();
t(fn() => $f4->start());
$f4->resume();
t(fn() => $f4->resume());
t(fn() => $f4->throw(new Exception('e')));
var_dump($f4->getReturn());
// A generator iterated from the fiber's own frames (the body suspends
// only from bytecode the fiber's loop drives, see COVERAGE.md).
function counter() { for ($i = 1; $i <= 3; $i++) { yield $i; } }
$f6 = new Fiber(function () { $sum = 0; foreach (counter() as $n) { $sum += Fiber::suspend($n); } return $sum; });
$v = $f6->start();
while (!$f6->isTerminated()) { $v = $f6->resume($v * 2); }
var_dump($f6->getReturn());
// traces from inside a fiber
$f7 = new Fiber(function () { throw new LogicException('trace me'); });
try { $f7->start(); } catch (LogicException $e) { echo $e->getTraceAsString(), "\n"; }
$f8 = new Fiber(function () { Fiber::suspend(); debug_print_backtrace(); });
$f8->start();
$f8->resume();
echo "end\n";
