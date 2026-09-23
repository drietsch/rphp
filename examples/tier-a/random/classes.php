<?php
// Tier-A differential: the shape of ext/random's classes — the interfaces
// and exception hierarchy, final classes and their methods, the readonly
// $engine (a second __construct, writes, unset), dynamic properties
// refused, clone (engines fork, Secure and Randomizer refuse), the default
// Secure engine (shapes only: its output is the OS CSPRNG), and the
// IntervalBoundary enum.

use Random\Engine\Mt19937;
use Random\Engine\PcgOneseq128XslRr64;
use Random\Engine\Secure;
use Random\Engine\Xoshiro256StarStar;
use Random\Randomizer;

function t(callable $f): void
{
    try {
        $v = $f();
        echo is_object($v) ? get_class($v) : json_encode($v), "\n";
    } catch (\Throwable $t) {
        echo get_class($t), ': ', $t->getMessage(), "\n";
    }
}

foreach ([\Random\Engine::class, \Random\CryptoSafeEngine::class, Mt19937::class, PcgOneseq128XslRr64::class,
    Xoshiro256StarStar::class, Secure::class, Randomizer::class, \Random\RandomError::class,
    \Random\BrokenRandomEngineError::class, \Random\RandomException::class] as $c) {
    $rc = new \ReflectionClass($c);
    $methods = [];
    foreach ($rc->getMethods() as $m) {
        if ($m->class === $c) {
            $methods[] = $m->name;
        }
    }
    echo $c, ' final=', (int) $rc->isFinal(), ' interface=', (int) $rc->isInterface(),
        ' parent=', $rc->getParentClass() ? $rc->getParentClass()->name : '-',
        ' methods=', implode(',', $methods), "\n";
}
var_dump(new Secure() instanceof \Random\CryptoSafeEngine, new Secure() instanceof \Random\Engine,
    new Mt19937() instanceof \Random\CryptoSafeEngine, new \Random\BrokenRandomEngineError() instanceof \Error,
    new \Random\RandomException() instanceof \Exception, is_subclass_of(Xoshiro256StarStar::class, \Random\Engine::class));

// The readonly engine.
$r = new Randomizer(new Mt19937(1));
t(fn() => get_class($r->engine));
t(fn() => isset($r->engine));
t(function () use ($r) { $r->engine = new Secure(); });
t(function () use ($r) { $r->__construct(); });
t(function () use ($r) { unset($r->engine); });
t(fn() => (new \ReflectionProperty(Randomizer::class, 'engine'))->isReadOnly());
t(fn() => (string) (new \ReflectionProperty(Randomizer::class, 'engine'))->getType());
print_r($r);
echo "\n";
print_r((array) $r);
print_r(get_object_vars($r));
echo json_encode($r), "\n";

// No dynamic properties anywhere.
t(function () use ($r) { $r->foo = 1; });
t(function () { $e = new Mt19937(1); $e->foo = 1; });
t(function () { $e = new PcgOneseq128XslRr64(1); $e->foo = 1; });
t(function () { $e = new Secure(); $e->foo = 1; });
t(function () { $e = new Mt19937(1); unset($e->foo); return 'ok'; });
t(fn() => @$r->nope);

// Clone.
t(fn() => clone $r);
t(fn() => clone new Secure());
t(fn() => bin2hex((clone new Xoshiro256StarStar(1))->generate()));

// The default engine.
$s = new Randomizer();
t(fn() => get_class($s->engine));
t(fn() => strlen($s->getBytes(33)));
t(fn() => $s->getInt(5, 5));
t(fn() => is_int($s->nextInt()) && $s->nextInt() >= 0);
t(fn() => ($f = $s->nextFloat()) >= 0 && $f < 1);
t(fn() => strlen((new Secure())->generate()));
t(fn() => count($s->shuffleArray(range(1, 50))));
t(fn() => strlen($s->shuffleBytes('abcdef')));
t(fn() => count($s->pickArrayKeys(range(1, 50), 7)));
t(fn() => strspn($s->getBytesFromString('ab', 64), 'ab'));
t(fn() => get_class((new Randomizer(null))->engine));
print_r(new Secure());
echo "\n";

// The enum.
var_dump(\Random\IntervalBoundary::cases());
t(fn() => \Random\IntervalBoundary::ClosedOpen->name);
t(fn() => \Random\IntervalBoundary::ClosedOpen instanceof \UnitEnum);
t(fn() => \Random\IntervalBoundary::ClosedOpen === \Random\IntervalBoundary::ClosedOpen);
