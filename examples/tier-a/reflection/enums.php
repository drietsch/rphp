<?php
namespace App\Cards;

// Tier-A differential: ReflectionEnum and the two case reflectors. An enum's
// cases are class constants for every Reflection purpose, so they show up in
// getConstants()/getReflectionConstants() ahead of the declared ones.

enum Suit: string {
    case Hearts = 'H';
    case Spades = 'S';
    const WILD = self::Spades;
    public function colour(): string { return $this === self::Hearts ? 'red' : 'black'; }
}

enum Bare {
    case One;
    case Two;
}

class NotAnEnum {}

$re = new \ReflectionEnum(Suit::class);

echo "-- enum --\n";
echo $re->getName(), ' short=', $re->getShortName(), "\n";
var_dump($re->isEnum(), $re->isBacked(), $re->isFinal(), $re->isInstantiable(), $re->isInternal());
echo 'modifiers=', $re->getModifiers(), ' backing=', (string) $re->getBackingType(),
     ' builtin=', var_export($re->getBackingType()->isBuiltin(), true), "\n";
echo 'interfaces=', implode(',', $re->getInterfaceNames()), "\n";
echo 'methods=', implode(',', array_map(fn($m) => $m->name, $re->getMethods())), "\n";
echo 'props=', implode(',', array_map(fn($p) => $p->name, $re->getProperties())), "\n";
var_dump($re instanceof \ReflectionClass);

echo "-- cases --\n";
var_dump($re->hasCase('Hearts'), $re->hasCase('Clubs'));
foreach ($re->getCases() as $c) {
    echo '  ', get_class($c), ' ', $c->class, '::', $c->name,
         ' backing=', var_export($c->getBackingValue(), true),
         ' value=', $c->getValue()->name, '/', $c->getValue()->value,
         ' enum=', $c->getEnum()->getName(),
         ' enumCase=', var_export($c->isEnumCase(), true),
         ' mod=', $c->getModifiers(), "\n";
}
$h = $re->getCase('Hearts');
var_dump($h->getValue() === Suit::Hearts, $h->getDeclaringClass()->getName());
echo 'colour=', $h->getValue()->colour(), "\n";

echo "-- pure enum --\n";
$rb = new \ReflectionEnum(Bare::class);
var_dump($rb->isBacked(), $rb->getBackingType());
echo 'cases=', implode(',', array_map(fn($c) => get_class($c) . ':' . $c->name, $rb->getCases())), "\n";
var_dump($rb->getCase('One')->getValue() === Bare::One);

echo "-- as class constants --\n";
$rc = new \ReflectionClass(Suit::class);
echo 'constant names=', implode(',', array_keys($rc->getConstants())), "\n";
foreach ($rc->getReflectionConstants() as $k) {
    echo '  ', get_class($k), ' ', $k->class, '::', $k->name,
         ' enumCase=', var_export($k->isEnumCase(), true),
         ' mod=', $k->getModifiers(), "\n";
}
var_dump($rc->hasConstant('WILD'), $rc->hasConstant('Hearts'));
echo 'WILD=', $rc->getConstant('WILD')->name, ' Hearts=', $rc->getConstant('Hearts')->value, "\n";
$wild = new \ReflectionClassConstant(Suit::class, 'WILD');
var_dump($wild->isEnumCase(), $wild->getValue() === Suit::Spades);

echo "-- errors --\n";
foreach ([
    fn() => new \ReflectionEnum(NotAnEnum::class),
    fn() => $re->getCase('Clubs'),
    fn() => new \ReflectionClassConstant(Suit::class, 'Clubs'),
] as $f) {
    try { $f(); echo "no error\n"; }
    catch (\Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
}
