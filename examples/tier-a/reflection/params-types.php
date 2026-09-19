<?php
namespace App\Sig;

// Tier-A differential: ReflectionParameter and the ReflectionType family.
// Internal functions are left out: the engine carries no arginfo for a
// native beyond its arity, so their parameters have no declared types.

interface Ia {}
interface Ib {}

class Holder {
    const LIMIT = 5;
    public int $typed = 1;
    public ?Ia $maybe = null;
    public $untyped = 'u';
    public string|int $either = 1;
    public const int SIZED = 7;

    public function __construct(
        public readonly string $promoted = 'p',
        protected int $plain = 0,
    ) {}

    public function sig(
        int $a,
        ?string $b,
        string|int $c,
        Ia&Ib $d,
        mixed $e,
        array|null $f,
        Ia|Ib|null $g = null,
        ?self $h = null,
        $untyped = PHP_INT_MAX,
        int $k = self::LIMIT,
        array $l = [1, 2],
        int ...$rest
    ) {}

    public function ret(): static {}
    public function retSelf(): self {}
    public function retVoid(): void {}
    public function retNever(): never { throw new \Exception('x'); }
    public function retDnf(): (Ia&Ib)|null { return null; }
    public function retNone() {}
    public function iter(iterable $i, callable $c, false $no = false): true { return true; }
}

function show(?\ReflectionType $t): string {
    if ($t === null) {
        return 'null';
    }
    $out = get_class($t) . '{' . (string) $t . ' allowsNull=' . var_export($t->allowsNull(), true);
    if ($t instanceof \ReflectionNamedType) {
        $out .= ' name=' . $t->getName() . ' builtin=' . var_export($t->isBuiltin(), true);
    }
    if ($t instanceof \ReflectionUnionType || $t instanceof \ReflectionIntersectionType) {
        $parts = [];
        foreach ($t->getTypes() as $s) {
            $parts[] = show($s);
        }
        $out .= ' types=[' . implode(' ', $parts) . ']';
    }
    return $out . '}';
}

$m = new \ReflectionMethod(Holder::class, 'sig');
echo "-- parameters --\n";
echo 'count=', $m->getNumberOfParameters(), '/', $m->getNumberOfRequiredParameters(),
     ' variadic=', var_export($m->isVariadic(), true), "\n";
foreach ($m->getParameters() as $p) {
    printf(
        "  #%d $%s opt=%s def=%s null=%s var=%s ref=%s val=%s prom=%s has=%s\n",
        $p->getPosition(), $p->getName(),
        var_export($p->isOptional(), true),
        var_export($p->isDefaultValueAvailable(), true),
        var_export($p->allowsNull(), true),
        var_export($p->isVariadic(), true),
        var_export($p->isPassedByReference(), true),
        var_export($p->canBePassedByValue(), true),
        var_export($p->isPromoted(), true),
        var_export($p->hasType(), true)
    );
    echo '      type=', show($p->getType()), "\n";
}

echo "-- defaults --\n";
foreach ($m->getParameters() as $p) {
    if ($p->isDefaultValueAvailable()) {
        echo '  $', $p->getName(), ' = ', var_export($p->getDefaultValue(), true), "\n";
    }
}

echo "-- declaring --\n";
$p0 = $m->getParameters()[0];
echo $p0->getDeclaringClass()->getName(), '::', $p0->getDeclaringFunction()->getName(), "\n";
$fp = (new \ReflectionFunction('App\\Sig\\show'))->getParameters()[0];
var_dump($fp->getDeclaringClass());
echo $fp->getDeclaringFunction()->getName(), ' ', show($fp->getType()), "\n";

echo "-- promoted --\n";
$ctor = (new \ReflectionClass(Holder::class))->getConstructor();
foreach ($ctor->getParameters() as $p) {
    echo '  $', $p->getName(), ' promoted=', var_export($p->isPromoted(), true),
         ' default=', var_export($p->getDefaultValue(), true), "\n";
}
var_dump((new \ReflectionProperty(Holder::class, 'promoted'))->isPromoted(),
         (new \ReflectionProperty(Holder::class, 'typed'))->isPromoted());

echo "-- direct construction --\n";
$byIndex = new \ReflectionParameter([Holder::class, 'sig'], 1);
$byName = new \ReflectionParameter([Holder::class, 'sig'], 'c');
echo $byIndex->getName(), '@', $byIndex->getPosition(), ' ', $byName->getName(), '@', $byName->getPosition(), "\n";
$byFn = new \ReflectionParameter('App\\Sig\\show', 0);
echo $byFn->getName(), '@', $byFn->getPosition(), "\n";

echo "-- return types --\n";
foreach (['ret', 'retSelf', 'retVoid', 'retNever', 'retDnf', 'retNone', 'iter'] as $n) {
    $r = new \ReflectionMethod(Holder::class, $n);
    echo '  ', $n, ' has=', var_export($r->hasReturnType(), true), ' ', show($r->getReturnType()), "\n";
}

echo "-- property and constant types --\n";
foreach (['typed', 'maybe', 'untyped', 'either', 'promoted'] as $n) {
    $p = new \ReflectionProperty(Holder::class, $n);
    echo '  $', $n, ' has=', var_export($p->hasType(), true), ' ', show($p->getType()), "\n";
}
$k = new \ReflectionClassConstant(Holder::class, 'SIZED');
echo '  SIZED has=', var_export($k->hasType(), true), ' ', show($k->getType()), "\n";
$k2 = new \ReflectionClassConstant(Holder::class, 'LIMIT');
echo '  LIMIT has=', var_export($k2->hasType(), true), ' ', show($k2->getType()), "\n";

echo "-- ReflectionType shape --\n";
$t = (new \ReflectionMethod(Holder::class, 'ret'))->getReturnType();
var_dump($t instanceof \ReflectionType, $t instanceof \Stringable);

// `__toString()`, which code in the wild parses for `= NULL`.
function tostring_shapes(int $a, ?string $b = null, int|float $d = 5,
    bool $t = true, array $arr = [1, 'k' => 2], string ...$rest) {}
foreach ((new \ReflectionFunction(__NAMESPACE__ . '\\tostring_shapes'))->getParameters() as $p) {
    printf("%-2s %-12s allowsNull=%d %s\n", $p->getName(),
        $p->hasType() ? (string) $p->getType() : '-', (int) $p->allowsNull(),
        (string) $p);
}
function by_ref_param(&$x, $plain) {}
foreach ((new \ReflectionFunction(__NAMESPACE__ . '\\by_ref_param'))->getParameters() as $p) {
    echo (string) $p, "\n";
}
