<?php
namespace App\Meta;

// Tier-A differential: the `Attribute` class and `ReflectionAttribute`'s
// shape. The engine does not carry attributes yet — the compiler drops every
// `#[...]` group — so this pins the parts that do not depend on them: the
// class tree, the TARGET_* constants, the private constructor, and the empty
// list every getAttributes() returns for a declaration without attributes.

class Plain {
    const K = 1;
    public int $p = 1;
    public function m(int $a): void {}
}
function plainFn(int $a): void {}

echo "-- Attribute --\n";
$ra = new \ReflectionClass(\Attribute::class);
var_dump($ra->isFinal(), $ra->isInternal(), $ra->getInterfaceNames());
echo 'props=', implode(',', array_map(fn($p) => $p->name, $ra->getProperties())), "\n";
echo '  TARGET_CLASS=', \Attribute::TARGET_CLASS, "\n";
echo '  TARGET_FUNCTION=', \Attribute::TARGET_FUNCTION, "\n";
echo '  TARGET_METHOD=', \Attribute::TARGET_METHOD, "\n";
echo '  TARGET_PROPERTY=', \Attribute::TARGET_PROPERTY, "\n";
echo '  TARGET_CLASS_CONSTANT=', \Attribute::TARGET_CLASS_CONSTANT, "\n";
echo '  TARGET_PARAMETER=', \Attribute::TARGET_PARAMETER, "\n";
echo '  TARGET_CONSTANT=', \Attribute::TARGET_CONSTANT, "\n";
echo '  TARGET_ALL=', \Attribute::TARGET_ALL, "\n";
echo '  IS_REPEATABLE=', \Attribute::IS_REPEATABLE, "\n";
$attr = new \Attribute();
echo 'default flags=', $attr->flags, "\n";
$attr2 = new \Attribute(\Attribute::TARGET_METHOD | \Attribute::IS_REPEATABLE);
echo 'explicit flags=', $attr2->flags, "\n";

echo "-- ReflectionAttribute --\n";
$rr = new \ReflectionClass(\ReflectionAttribute::class);
echo 'name=', $rr->getName(), ' IS_INSTANCEOF=', \ReflectionAttribute::IS_INSTANCEOF, "\n";
var_dump($rr->implementsInterface(\Reflector::class));
echo 'ctor visibility=', implode(',', array_map(
    fn($m) => $m->name . ':' . ($m->isPrivate() ? 'private' : 'public'),
    array_filter($rr->getMethods(), fn($m) => $m->name === '__construct')
)), "\n";
try { new \ReflectionAttribute(); }
catch (\Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }

echo "-- no attributes --\n";
$rc = new \ReflectionClass(Plain::class);
var_dump($rc->getAttributes());
var_dump($rc->getMethod('m')->getAttributes());
var_dump($rc->getMethod('m')->getParameters()[0]->getAttributes());
var_dump($rc->getProperty('p')->getAttributes());
var_dump($rc->getReflectionConstant('K')->getAttributes());
var_dump((new \ReflectionFunction('App\\Meta\\plainFn'))->getAttributes());
echo 'filtered=', count($rc->getAttributes(\Attribute::class)), '/',
     count($rc->getAttributes(\Attribute::class, \ReflectionAttribute::IS_INSTANCEOF)), "\n";

echo "-- doc comments --\n";
// The compiler drops doc comments, so only the "no doc comment" answer is
// pinned here.
var_dump($rc->getMethod('m')->getDocComment(), $rc->getProperty('p')->getDocComment(),
         $rc->getReflectionConstant('K')->getDocComment());

echo "-- Reflector --\n";
foreach (['ReflectionClass', 'ReflectionObject', 'ReflectionMethod', 'ReflectionFunction',
          'ReflectionParameter', 'ReflectionProperty', 'ReflectionClassConstant',
          'ReflectionEnum', 'ReflectionAttribute'] as $c) {
    echo '  ', $c, '=', var_export((new \ReflectionClass($c))->implementsInterface(\Reflector::class), true), "\n";
}
var_dump((new \ReflectionClass(\Reflector::class))->isInterface(),
         (new \ReflectionClass(\ReflectionException::class))->getParentClass()->getName(),
         (new \ReflectionClass(\ReflectionType::class))->isAbstract(),
         (new \ReflectionClass(\ReflectionFunctionAbstract::class))->isAbstract());
