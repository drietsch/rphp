<?php
namespace App\Model;

// Tier-A differential: ReflectionClass / ReflectionObject — the class
// metadata Symfony's container reads. Dumped first so the object handle is
// #1 on both sides.
var_dump(new \ReflectionClass(\ReflectionClass::class));

interface Named { const KIND = 'named'; public function name(): string; }
interface Tagged extends Named { public function tag(): string; }
trait Loggable { public function log(): string { return 'log'; } }

abstract class Base implements Tagged {
    use Loggable;
    public int $id = 0;
    protected string $label = 'base';
    private array $secret = [1];
    public static int $made = 0;
    const BASE_CONST = 'B';
    public function name(): string { return 'base'; }
    abstract public function tag(): string;
    final protected function seal(): void {}
}

final class Leaf extends Base {
    public string $extra = 'x';
    private const HIDDEN = 'h';
    public const VISIBLE = 'v';
    // The static property is declared after the promoted one on purpose: the
    // engine keeps instance and static properties in separate tables, so
    // Reflection can only report them grouped, not interleaved.
    public function __construct(public readonly string $key = 'k') {}
    public static string $tally = 't';
    public function tag(): string { return 'leaf'; }
    private function inner(): int { return 1; }
    public static function make(): string { return 'made'; }
}

readonly class Frozen { public function __construct(public int $n = 1) {} }

$r = new \ReflectionClass(Leaf::class);

echo "-- names --\n";
echo $r->getName(), "|", $r->getShortName(), "|", $r->getNamespaceName(), "|";
var_dump($r->inNamespace());

echo "-- predicates --\n";
foreach (['isInterface', 'isTrait', 'isEnum', 'isAbstract', 'isFinal', 'isReadOnly',
          'isAnonymous', 'isInstantiable', 'isCloneable', 'isInternal', 'isUserDefined',
          'isIterable'] as $p) {
    echo $p, '=', var_export($r->$p(), true), ' ';
}
echo "\n";
echo 'Base isAbstract=', var_export((new \ReflectionClass(Base::class))->isAbstract(), true), "\n";
echo 'Named isInterface=', var_export((new \ReflectionClass(Named::class))->isInterface(), true), "\n";
echo 'Loggable isTrait=', var_export((new \ReflectionClass(Loggable::class))->isTrait(), true), "\n";

echo "-- modifiers --\n";
echo 'Leaf=', $r->getModifiers(), ' Base=', (new \ReflectionClass(Base::class))->getModifiers(),
     ' Named=', (new \ReflectionClass(Named::class))->getModifiers(),
     ' Frozen=', (new \ReflectionClass(Frozen::class))->getModifiers(), "\n";
echo 'IS_FINAL=', \ReflectionClass::IS_FINAL, ' IS_EXPLICIT_ABSTRACT=', \ReflectionClass::IS_EXPLICIT_ABSTRACT,
     ' IS_IMPLICIT_ABSTRACT=', \ReflectionClass::IS_IMPLICIT_ABSTRACT,
     ' IS_READONLY=', \ReflectionClass::IS_READONLY, "\n";

echo "-- inheritance --\n";
echo 'parent=', $r->getParentClass()->getName(), "\n";
var_dump((new \ReflectionClass(Base::class))->getParentClass());
echo 'interfaces=', implode(',', $r->getInterfaceNames()), "\n";
echo 'getInterfaces keys=', implode(',', array_keys($r->getInterfaces())), "\n";
var_dump($r->implementsInterface(Named::class), $r->isSubclassOf(Base::class), $r->isSubclassOf(Leaf::class));
echo 'traits Base=', implode(',', (new \ReflectionClass(Base::class))->getTraitNames()),
     ' Leaf=', implode(',', $r->getTraitNames()), "\n";

echo "-- methods --\n";
foreach ($r->getMethods() as $m) {
    echo '  ', $m->class, '::', $m->name, ' mod=', $m->getModifiers(), "\n";
}
var_dump($r->hasMethod('inner'), $r->hasMethod('nope'));
echo 'getMethod=', $r->getMethod('log')->class, '::', $r->getMethod('log')->name, "\n";
echo 'ctor=', $r->getConstructor()->name, "\n";
var_dump((new \ReflectionClass(Base::class))->getConstructor());
echo 'static filter=', implode(',', array_map(fn($m) => $m->name, $r->getMethods(\ReflectionMethod::IS_STATIC))), "\n";

echo "-- properties --\n";
foreach ($r->getProperties() as $p) {
    echo '  ', $p->class, '::$', $p->name, ' mod=', $p->getModifiers(), "\n";
}
var_dump($r->hasProperty('label'), $r->hasProperty('secret'), $r->hasProperty('nope'));
echo 'public filter=', implode(',', array_map(fn($p) => $p->name, $r->getProperties(\ReflectionProperty::IS_PUBLIC))), "\n";

echo "-- constants --\n";
var_export($r->getConstants());
echo "\n";
echo 'rconsts=', implode(',', array_map(fn($c) => $c->class . '::' . $c->name, $r->getReflectionConstants())), "\n";
var_dump($r->hasConstant('VISIBLE'), $r->hasConstant('NOPE'), $r->getConstant('VISIBLE'));
var_dump($r->getReflectionConstant('NOPE'));

echo "-- statics --\n";
var_export($r->getStaticProperties());
echo "\n";
var_dump($r->getStaticPropertyValue('tally'), $r->getStaticPropertyValue('nope', 'fallback'));
$r->setStaticPropertyValue('tally', 'T2');
var_dump(Leaf::$tally, $r->getStaticPropertyValue('made'));

echo "-- instances --\n";
$a = $r->newInstance('one');
$b = $r->newInstanceArgs(['two']);
$c = $r->newInstanceWithoutConstructor();
echo get_class($a), '/', $a->key, ' ', get_class($b), '/', $b->key, ' ', get_class($c), "\n";
var_dump($r->isInstance($a), $r->isInstance(new Frozen()));
var_dump($r->getFileName() === __FILE__);

echo "-- ReflectionObject --\n";
$o = new \stdClass();
$o->alpha = 1;
$o->beta = 2;
$ro = new \ReflectionObject($o);
echo $ro->getName(), ' props=', implode(',', array_map(fn($p) => $p->class . '::$' . $p->name, $ro->getProperties())), "\n";
var_dump($ro->hasProperty('alpha'), $ro->getProperty('beta')->getValue($o));
var_dump($ro instanceof \ReflectionClass, $ro instanceof \Reflector);
// A ReflectionClass built from an instance still reports the class only.
echo 'class view props=', count((new \ReflectionClass($o))->getProperties()), "\n";

echo "-- errors --\n";
foreach ([
    fn() => new \ReflectionClass('App\\Model\\NoSuchClass'),
    fn() => $r->getMethod('nope'),
    fn() => $r->getProperty('nope'),
    fn() => $r->getStaticPropertyValue('nope'),
    fn() => (new \ReflectionClass(Base::class))->newInstance(),
    fn() => (new \ReflectionClass(Named::class))->newInstanceWithoutConstructor(),
    fn() => $r->implementsInterface(Base::class),
    fn() => $r->isSubclassOf('App\\Model\\Missing'),
] as $f) {
    try { $f(); echo "no error\n"; }
    catch (\Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
}
