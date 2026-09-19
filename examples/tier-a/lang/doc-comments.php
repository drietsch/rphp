<?php

// The `/** … */` a declaration carries is what Reflection answers with —
// container compilers and annotation readers live on it.

/**
 * A class docblock.
 *
 * @psalm-type ConfigType = array{x: int}
 */
class Documented
{
    /** A constant docblock. */
    public const C = 1;

    /** A property docblock. */
    public int $p = 1;

    /** Two properties, one docblock. */
    public $a = 1, $b = 2;

    /** A static docblock. */
    public static $s = 1;

    /**
     * A method docblock.
     */
    public function m(): void {}

    // A plain comment is not a docblock.
    public function plain(): void {}

    /** A promoted-constructor docblock. */
    public function __construct(public int $promoted = 0) {}
}

/** A function docblock. */
function documented_fn() {}

# A hash comment is not one either.
function undocumented_fn() {}

$r = new ReflectionClass('Documented');
var_dump($r->getDocComment());
var_dump((new ReflectionProperty('Documented', 'p'))->getDocComment());
var_dump((new ReflectionProperty('Documented', 'a'))->getDocComment());
var_dump((new ReflectionProperty('Documented', 'b'))->getDocComment());
var_dump((new ReflectionProperty('Documented', 'promoted'))->getDocComment());
var_dump((new ReflectionMethod('Documented', 'm'))->getDocComment());
var_dump((new ReflectionMethod('Documented', 'plain'))->getDocComment());
var_dump((new ReflectionFunction('documented_fn'))->getDocComment());
var_dump((new ReflectionFunction('undocumented_fn'))->getDocComment());
var_dump((new ReflectionClass('stdClass'))->getDocComment());
var_dump((new ReflectionFunction('strlen'))->getDocComment());

$closure = /** A closure docblock. */ function () {};
var_dump((new ReflectionFunction($closure))->getDocComment());

interface Documenteds
{
    /** An interface method docblock. */
    public function im(): void;
}
var_dump((new ReflectionClass('Documenteds'))->getDocComment(),
    (new ReflectionMethod('Documenteds', 'im'))->getDocComment());

/** A trait docblock. */
trait DocumentedTrait
{
    /** A trait method docblock. */
    public function tm(): void {}

    /** A trait property docblock. */
    public $tp = 1;
}
class UsesTrait
{
    use DocumentedTrait;
}
var_dump((new ReflectionClass('DocumentedTrait'))->getDocComment(),
    (new ReflectionMethod('UsesTrait', 'tm'))->getDocComment(),
    (new ReflectionProperty('UsesTrait', 'tp'))->getDocComment());

/** An enum docblock. */
enum DocumentedEnum: string
{
    /** A case docblock. */
    case One = 'one';
}
var_dump((new ReflectionClass('DocumentedEnum'))->getDocComment());
