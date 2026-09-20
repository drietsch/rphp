<?php

// `#[...]` on every place php allows one, read back through Reflection.
// Arguments are evaluated when they are asked for, never at declaration.

#[Attribute(Attribute::TARGET_ALL | Attribute::IS_REPEATABLE)]
class Tag
{
    public function __construct(public string $name = '', public int $n = 0) {}
}

#[Attribute(Attribute::TARGET_CLASS)]
class ClassOnly {}

#[Attribute]
class Plain {}

class NotAnAttribute {}

const TAG_FROM_CONST = 'from-const';

#[Tag('class-one')]
#[Tag('class-two', n: 5)]
#[ClassOnly]
class Tagged
{
    #[Tag('const')]
    public const C = 1;

    #[Tag('prop')]
    public int $p = 1;

    #[Tag('static-prop')]
    public static $sp = 1;

    #[Tag('method')]
    public function m(#[Tag('param')] int $a, #[Tag('second')] $b = 2) {}

    public function __construct(#[Tag('promoted')] public int $q = 0) {}
}

#[Tag(TAG_FROM_CONST, n: 1 + 2)]
function tagged_function(#[Tag('fn-param')] $x) {}

interface TaggedInterface {}

#[Tag('enum')]
enum TaggedEnum: string
{
    #[Tag('case')]
    case One = 'one';
}

#[Tag('trait')]
trait TaggedTrait {}

$rc = new ReflectionClass('Tagged');
foreach ($rc->getAttributes() as $a) {
    printf("class %-10s %-28s target=%d repeated=%d\n", $a->getName(),
        json_encode($a->getArguments()), $a->getTarget(), (int) $a->isRepeated());
}
var_dump(count($rc->getAttributes(Tag::class)), count($rc->getAttributes('Nope')),
    count($rc->getAttributes(Tag::class, ReflectionAttribute::IS_INSTANCEOF)));

foreach ([new ReflectionProperty('Tagged', 'p'), new ReflectionProperty('Tagged', 'q')] as $rp) {
    foreach ($rp->getAttributes() as $a) {
        printf("prop  %-10s %s target=%d\n", $a->getName(), json_encode($a->getArguments()), $a->getTarget());
    }
}
foreach ((new ReflectionMethod('Tagged', 'm'))->getAttributes() as $a) {
    printf("method %-9s %s target=%d\n", $a->getName(), json_encode($a->getArguments()), $a->getTarget());
}
foreach ((new ReflectionMethod('Tagged', 'm'))->getParameters() as $rp) {
    foreach ($rp->getAttributes() as $a) {
        printf("param %-10s %s target=%d\n", $a->getName(), json_encode($a->getArguments()), $a->getTarget());
    }
}
$rf = new ReflectionFunction('tagged_function');
foreach ($rf->getAttributes() as $a) {
    printf("fn    %-10s %s target=%d\n", $a->getName(), json_encode($a->getArguments()), $a->getTarget());
}
foreach ($rf->getParameters()[0]->getAttributes() as $a) {
    printf("fnarg %-10s %s\n", $a->getName(), json_encode($a->getArguments()));
}
foreach ([new ReflectionClass('TaggedEnum'), new ReflectionClass('TaggedTrait'),
          new ReflectionClass('TaggedInterface')] as $r) {
    printf("%-16s %d attributes\n", $r->getShortName(), count($r->getAttributes()));
}

// newInstance() validates the class, the target and repeatability, then runs
// the constructor with the arguments as written.
$built = $rc->getAttributes(Tag::class)[1]->newInstance();
var_dump(get_class($built), $built->name, $built->n);
var_dump($rf->getAttributes()[0]->newInstance()->name, $rf->getAttributes()[0]->newInstance()->n);

#[NotAnAttribute]
#[ClassOnly]
class Misused
{
    #[ClassOnly]
    public $bad = 1;
}
foreach ((new ReflectionClass('Misused'))->getAttributes() as $a) {
    try {
        $a->newInstance();
        echo "built ", $a->getName(), "\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
try {
    (new ReflectionProperty('Misused', 'bad'))->getAttributes()[0]->newInstance();
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
