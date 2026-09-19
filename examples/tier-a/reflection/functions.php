<?php
namespace App\Fn;

// Tier-A differential: ReflectionFunction / ReflectionMethod — the callable
// side of Reflection. Closure names are left out: php 8.4 renamed a closure
// to `{closure:file:line}` and the name would carry a path.

function plain(int $a, string $b = 'b', int ...$rest): string { return $a . $b . count($rest); }
function gen(): \Generator { yield 1; }
function byRef(array &$out, int $n = 0): void { $out[] = $n; }

class Svc {
    public const TAG = 'svc';
    public function __construct(private int $base = 10) {}
    public function add(int $n): int { return $this->base + $n; }
    private function hidden(string $s): string { return "<$s>"; }
    protected function guarded(): string { return 'g'; }
    public static function tag(string $suffix = '!'): string { return self::TAG . $suffix; }
    final public function sealed(): void {}
    public function __destruct() {}
}
abstract class Abs { abstract public function todo(): void; }

$rf = new \ReflectionFunction('App\\Fn\\plain');

echo "-- function --\n";
echo $rf->getName(), '|', $rf->getShortName(), '|', $rf->getNamespaceName(), '|';
var_dump($rf->inNamespace());
var_dump($rf->isInternal(), $rf->isUserDefined(), $rf->isClosure(), $rf->isGenerator(),
         $rf->isVariadic(), $rf->isStatic());
echo 'params=', $rf->getNumberOfParameters(), '/', $rf->getNumberOfRequiredParameters(), "\n";
var_dump($rf->hasReturnType(), (string) $rf->getReturnType());
var_dump($rf->getDocComment());
var_dump($rf->getFileName() === __FILE__);
echo 'invoke=', $rf->invoke(1, 'z', 7, 8), " invokeArgs=", $rf->invokeArgs([2, 'y']), "\n";
$cl = $rf->getClosure();
echo 'closure=', $cl(3, 'x'), "\n";

echo "-- generator / by-reference --\n";
var_dump((new \ReflectionFunction('App\\Fn\\gen'))->isGenerator());
$rb = new \ReflectionFunction('App\\Fn\\byRef');
var_dump($rb->getParameters()[0]->isPassedByReference(), $rb->getParameters()[1]->isPassedByReference());

echo "-- internal function --\n";
$ri = new \ReflectionFunction('strtoupper');
var_dump($ri->isInternal(), $ri->isUserDefined(), $ri->getFileName(), $ri->getDocComment());
echo 'invoke=', $ri->invoke('abc'), " closure=", ($ri->getClosure())('def'), "\n";

echo "-- methods --\n";
$rc = new \ReflectionClass(Svc::class);
foreach (['__construct', 'add', 'hidden', 'guarded', 'tag', 'sealed', '__destruct'] as $n) {
    $m = $rc->getMethod($n);
    printf(
        "  %s::%s mod=%d pub=%s prot=%s priv=%s stat=%s abs=%s fin=%s ctor=%s dtor=%s\n",
        $m->class, $m->name, $m->getModifiers(),
        var_export($m->isPublic(), true), var_export($m->isProtected(), true),
        var_export($m->isPrivate(), true), var_export($m->isStatic(), true),
        var_export($m->isAbstract(), true), var_export($m->isFinal(), true),
        var_export($m->isConstructor(), true), var_export($m->isDestructor(), true)
    );
}
var_dump((new \ReflectionMethod(Abs::class, 'todo'))->isAbstract());

$svc = new Svc(100);
$add = new \ReflectionMethod(Svc::class, 'add');
echo 'invoke=', $add->invoke($svc, 5), ' invokeArgs=', $add->invokeArgs($svc, [6]), "\n";
$hidden = new \ReflectionMethod(Svc::class, 'hidden');
echo 'private invoke=', $hidden->invoke($svc, 'p'), "\n";
$tag = new \ReflectionMethod(Svc::class, 'tag');
echo 'static invoke=', $tag->invoke(null, '?'), ' via object=', $tag->invoke($svc), "\n";
$bound = $add->getClosure($svc);
echo 'method closure=', $bound(7), "\n";
$statc = $tag->getClosure();
echo 'static closure=', $statc('#'), "\n";
echo 'declaring=', $add->getDeclaringClass()->getName(), "\n";
var_dump($add->getFileName() === __FILE__, $add->getDocComment());
echo 'fromName=', \ReflectionMethod::createFromMethodName('App\\Fn\\Svc::add')->class, "\n";
var_dump($add instanceof \ReflectionFunctionAbstract, $add instanceof \Reflector);

echo "-- closures --\n";
$fn = function (int $x, int $y = 2): int { return $x + $y; };
$rcl = new \ReflectionFunction($fn);
var_dump($rcl->isClosure(), $rcl->isStatic(), $rcl->isInternal());
echo 'params=', $rcl->getNumberOfParameters(), '/', $rcl->getNumberOfRequiredParameters(),
     ' ret=', (string) $rcl->getReturnType(), ' invoke=', $rcl->invoke(1), "\n";
$st = static function (): int { return 0; };
var_dump((new \ReflectionFunction($st))->isStatic());
var_dump((new \ReflectionFunction($fn))->getClosureThis());

echo "-- errors --\n";
foreach ([
    fn() => new \ReflectionFunction('App\\Fn\\missing'),
    fn() => new \ReflectionMethod(Svc::class, 'missing'),
    fn() => (new \ReflectionMethod(Svc::class, 'add'))->invoke(null),
    fn() => (new \ReflectionMethod(Svc::class, 'add'))->invoke(new \stdClass()),
    fn() => (new \ReflectionMethod(Abs::class, 'todo'))->invoke(null),
    fn() => (new \ReflectionMethod(Svc::class, 'add'))->getClosure(),
] as $f) {
    try { $f(); echo "no error\n"; }
    catch (\Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
}
