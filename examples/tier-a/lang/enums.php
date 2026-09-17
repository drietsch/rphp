<?php
// Tier-A: enum declarations — pure and backed, cases, constants, methods,
// static methods, interfaces, and the `UnitEnum`/`BackedEnum` API every enum
// gets for free. Differentially tested against stock PHP 8.5.

interface HasLabel {
    public function label(): string;
}

enum Direction {
    case Up;
    case Down;
    case Left;
    case Right;

    public function opposite(): Direction {
        return match ($this) {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        };
    }
}

enum Suit: string implements HasLabel {
    case Hearts = 'H';
    case Spades = 'S';
    case Clubs = 'C';
    case Diamonds = 'D';

    const DEFAULT = self::Hearts;
    const COUNT = 4;

    public function label(): string {
        return $this->name . '(' . $this->value . ')';
    }

    public function isRed(): bool {
        return $this === self::Hearts || $this === self::Diamonds;
    }

    public static function fromChar(string $c): Suit {
        return self::from($c);
    }
}

enum Level: int {
    case Low = 1;
    case Mid = 2;
    case High = 3;

    public static function highest(): Level {
        return self::High;
    }
}

// A pure enum's cases are singletons with a `name` and no `value`.
echo Direction::Up->name, ' ', Direction::Up->opposite()->name, "\n";
var_dump(Direction::Up === Direction::Up, Direction::Up === Direction::Down);
echo count(Direction::cases()), "\n";
foreach (Direction::cases() as $d) {
    echo $d->name, ' ';
}
echo "\n";

// A backed enum adds `value`, `from()` and `tryFrom()`.
echo Suit::Hearts->name, ' ', Suit::Hearts->value, ' ', Suit::Spades->label(), "\n";
var_dump(Suit::from('C'), Suit::tryFrom('S'), Suit::tryFrom('Z'));
var_dump(Suit::fromChar('D') === Suit::Diamonds);
var_dump(Suit::Hearts->isRed(), Suit::Spades->isRed());

// Enum constants, including one that names a case.
var_dump(Suit::DEFAULT === Suit::Hearts);
echo Suit::COUNT, ' ', count(Suit::cases()), "\n";

// Enums implement the engine interfaces and any they declare.
var_dump(
    Suit::Hearts instanceof HasLabel,
    Suit::Hearts instanceof UnitEnum,
    Suit::Hearts instanceof BackedEnum,
    Direction::Up instanceof UnitEnum,
    Direction::Up instanceof BackedEnum
);

// `from()` on a value no case carries is a ValueError.
try {
    Level::from(9);
} catch (ValueError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
var_dump(Level::tryFrom(9));
echo Level::highest()->name, ' ', Level::highest()->value, "\n";

// Cases work as array keys' values, in match, and compare by identity.
$names = array_map(static fn($l) => $l->name, Level::cases());
echo implode('|', $names), "\n";
echo match (Level::Mid) { Level::Low => 'low', Level::Mid => 'mid', Level::High => 'high' }, "\n";

// An enum is final and cannot be instantiated.
try {
    new Suit();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
