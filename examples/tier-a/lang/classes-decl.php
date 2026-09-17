<?php
// Tier-A: what a class-like *declaration* can say — interfaces, class
// constants (including lazily evaluated ones), static properties and methods,
// constructor property promotion, readonly, abstract and final. Differentially
// tested against stock PHP 8.5.

interface HasArea {
    const UNIT = 'cm';
    public function area(): float;
}

interface Named {
    public function name(): string;
}

// An interface may extend several others; its constants reach the implementor.
interface Shape extends HasArea, Named {
    const SIDES = 0;
}

abstract class Base {
    const SIDES = 0;
    // A class constant whose initializer is not a literal is evaluated lazily,
    // in this class's scope, on first use.
    const DOUBLE_SIDES = self::SIDES * 2;
    const LABELS = [HasArea::UNIT, 'shape'];

    // Static properties live on the class; a subclass that does not redeclare
    // shares the very same storage.
    public static int $made = 0;
    protected static $registry = [];

    abstract public function area(): float;

    public function name(): string {
        return static::class;
    }

    public static function born(string $what): void {
        static::$made++;
        self::$registry[] = $what;
    }

    public static function registry(): array {
        return self::$registry;
    }

    final public function describe(): string {
        return $this->name() . ' ' . $this->area() . static::UNIT;
    }
}

final class Square extends Base implements Shape {
    const SIDES = 4;

    public function __construct(private float $side) {
        self::born('square');
    }

    public function area(): float {
        return $this->side * $this->side;
    }
}

class Circle extends Base implements Shape {
    const SIDES = 1;
    // Redeclaring a static property gives the subclass its own storage.
    public static int $made = 100;

    public function __construct(public readonly float $r, public readonly string $tag = 'c') {
        self::born('circle');
    }

    public function area(): float {
        return 3.0 * $this->r * $this->r;
    }
}

$s = new Square(3);
$c = new Circle(2, 'disc');

echo $s->describe(), "\n";                 // Square 9cm
echo $c->describe(), "\n";                 // Circle 12cm
echo Square::SIDES, ' ', Base::DOUBLE_SIDES, ' ', HasArea::UNIT, ' ', Shape::UNIT, "\n";
echo implode(',', Base::LABELS), "\n";
echo Base::$made, ' ', Square::$made, ' ', Circle::$made, "\n";
echo implode(',', Base::registry()), "\n";

// Promoted parameters really are declared properties, in the constructor's
// position among the members.
echo $c->r, ' ', $c->tag, "\n";
var_dump($s instanceof Shape, $s instanceof HasArea, $s instanceof Base, $c instanceof Named);

// A readonly property may be written once, from the declaring scope.
try {
    $c->r = 5.0;
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// Static properties are per declaring class, and writable by name.
Circle::$made = 7;
echo Base::$made, ' ', Circle::$made, "\n";

// Late static binding through a static method.
class Counter {
    public static $n = 0;
    public static function bump(): int {
        return ++static::$n;
    }
    public static function who(): string {
        return static::class;
    }
}
class SubCounter extends Counter {
    public static $n = 50;
}
echo Counter::bump(), Counter::bump(), ' ', SubCounter::bump(), "\n";
echo Counter::who(), ' ', SubCounter::who(), "\n";

// A private constant is only reachable from inside; `final const` cannot be
// redeclared by a child.
class Secrets {
    private const KEY = 'k';
    final public const VERSION = 2;
    public static function key(): string {
        return self::KEY;
    }
}
echo Secrets::key(), Secrets::VERSION, "\n";
try {
    echo Secrets::KEY;
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
