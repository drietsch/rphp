<?php
// Tier-A: the *runtime* side of enums — case objects are singletons, the
// implicit `name`/`value` properties, `cases()`/`from()`/`tryFrom()` and the
// errors php raises for a bad backing value or a misused case. The
// declaration forms live in `enums.php`. Differentially tested against stock
// PHP 8.5. (Deliberately no `var_dump()` of a case: that rendering is the
// formatter's business, and it is covered in `enums.php`.)

interface Describable
{
    public function describe(): string;
}

enum Status: string implements Describable
{
    case Draft = 'draft';
    case Live = 'live';
    case Gone = 'gone';

    const DEFAULT = self::Draft;

    public function describe(): string
    {
        return static::class . '::' . $this->name . '=' . $this->value;
    }

    public static function fallback(): self
    {
        return self::tryFrom('nope') ?? self::DEFAULT;
    }
}

enum Priority: int
{
    case Low = 1;
    case High = 10;
}

enum Colour
{
    case Red;
    case Blue;
}

function attempt(callable $f): void
{
    try {
        $v = $f();
        echo $v === null ? 'NULL' : $v, "\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

// ---- cases are singletons ---------------------------------------------------

var_dump(Status::Draft === Status::Draft);
var_dump(Status::Draft === Status::from('draft'));
var_dump(Status::Draft === Status::tryFrom('draft'));
var_dump(Status::Draft === Status::cases()[0]);
var_dump(Status::Draft == Status::Live, Status::Draft === Status::Live);
var_dump(Status::DEFAULT === Status::Draft);
var_dump(Colour::Red === Colour::cases()[0]);
// Identity holds through a by-value round trip.
$copy = Status::Live;
var_dump($copy === Status::Live);
$array = [Status::Live];
var_dump($array[0] === Status::Live);

// ---- name and value ---------------------------------------------------------

echo Status::Draft->name, ' ', Status::Draft->value, "\n";
echo Priority::High->name, ' ', Priority::High->value, "\n";
echo Colour::Blue->name, "\n";
echo Status::Live->describe(), "\n";
var_dump(Status::Live instanceof Describable);
var_dump(Status::Live instanceof Status);
var_dump(Status::Live instanceof UnitEnum, Status::Live instanceof BackedEnum);
var_dump(Colour::Red instanceof UnitEnum, Colour::Red instanceof BackedEnum);

// A case's `name` and `value` are readonly.
attempt(static function () {
    $c = Status::Draft;
    $c->name = 'other';
    return 'assigned';
});

// ---- cases() ----------------------------------------------------------------

echo count(Status::cases()), count(Priority::cases()), count(Colour::cases()), "\n";
foreach (Status::cases() as $i => $case) {
    echo $i, ':', $case->name, '=', $case->value, ' ';
}
echo "\n";
foreach (Colour::cases() as $case) {
    echo $case->name, ' ';
}
echo "\n";
echo implode(',', array_map(static fn(Priority $p) => $p->name, Priority::cases())), "\n";
echo implode(',', array_map(static fn(Status $s) => $s->value, Status::cases())), "\n";

// ---- from() and tryFrom() ---------------------------------------------------

echo Status::from('live')->name, ' ', Priority::from(10)->name, "\n";
echo Status::fallback()->name, "\n";
attempt(static fn() => Status::tryFrom('missing'));
attempt(static fn() => Status::from('missing')->name);
attempt(static fn() => Priority::from(99)->name);
attempt(static fn() => Priority::tryFrom(99));
// Weak mode coerces a numeric string to the int backing, and an int to the
// string backing.
echo Priority::from('10')->name, "\n";
attempt(static fn() => Priority::from('x')->name);
attempt(static fn() => Status::from(7)->name);
attempt(static fn() => Priority::from([])->name);
attempt(static fn() => Colour::cases()[0]->name);

// ---- what an enum is not ----------------------------------------------------

attempt(static fn() => new Status());
attempt(static function () {
    $c = Status::Draft;
    return (clone $c)->name;
});
attempt(static fn() => Status::Missing);
attempt(static fn() => Status::Draft->missing ?? 'absent');

// ---- cases in match and as array keys ---------------------------------------

$label = match (Status::Gone) {
    Status::Draft => 'not published',
    Status::Live => 'published',
    Status::Gone => 'removed',
};
echo $label, "\n";

$byValue = [];
foreach (Status::cases() as $case) {
    $byValue[$case->value] = $case->name;
}
echo implode('|', $byValue), "\n";
