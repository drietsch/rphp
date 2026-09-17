<?php

// Fixture for `cargo xtask missing`: every construct the scanner classifies.
// The expected extraction is asserted in xtask/src/missing.rs (mod tests).

namespace Fixture;

use ArrayAccess;
use Countable, IteratorAggregate as IA;
use Dom\Document;
use function Dom\import_simplexml as sx;

#[\Attribute(\Attribute::TARGET_CLASS)]
final class Thing extends \ArrayObject implements ArrayAccess, \JsonSerializable
{
    use \Fixture\SomeTrait;

    private ?\Closure $cb = null;

    public function __construct(
        private readonly \DateTimeImmutable|\DateTime $when,
        #[\SensitiveParameter] iterable $items = [],
    ) {
        parent::__construct([]);
    }

    public function jsonSerialize(): array|\stdClass
    {
        return \json_encode($this->items, JSON_THROW_ON_ERROR | JSON_PRETTY_PRINT) ?: new \stdClass();
    }

    public static function make(string $s): static
    {
        $n = strlen($s) + \strlen($s) + mb_strlen($s);
        $x = $this->strlen($s);      // method call, not a function
        $y = Helper::strlen($s);     // static method call; Helper is a class reference
        $z = self::CONSTANT . static::class . parent::class;
        try {
            preg_match('/x/', $s);
        } catch (\ValueError | RuntimeException $e) {
            throw new \LogicException('nope', 0, $e);
        }
        if ($s instanceof \Stringable) {
        }
        $c = PHP_EOL . \DIRECTORY_SEPARATOR . E_ALL;
        $d = is_callable(strlen(...));
        $doc = new Document();
        $e = sx($doc);
        switch ($n) {
            case E_ALL:
                $m = $n > 0 ? ($n) : PHP_INT_MAX;
                break;
        }

        return new static();
    }
}

// A namespaced function shadowing an internal name is a declaration, not a call.
function strlen(): int
{
    return 0;
}

function helper(\SplObjectStorage $s, int|Countable ...$rest): ?\Traversable
{
    return null;
}

function intersect(Countable&\Traversable $x): void
{
}
