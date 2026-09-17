<?php
// Tier-A: the *runtime* side of class-level storage — lazy initializers,
// shared cells over the inheritance chain, visibility, typed static
// properties and late static binding. The expression forms themselves live in
// `static-access.php`. Differentially tested against stock PHP 8.5.

class Config
{
    public const BASE = 10;
    // A constant whose initializer is an expression: evaluated on first use,
    // in the declaring class's scope, not at declaration time.
    public const DOUBLE = self::BASE * 2;
    public const TABLE = [self::BASE => 'base', 'k' => self::DOUBLE];

    protected const SECRET = 'shh';
    private const HIDDEN = 'nope';

    // Static property initializers are lazy too, and may name constants.
    public static $limit = self::DOUBLE;
    public static array $seen = [self::BASE];

    public static function secret(): string
    {
        return self::SECRET . '/' . static::SECRET;
    }

    public static function hidden(): string
    {
        return self::HIDDEN;
    }
}

class SubConfig extends Config
{
    protected const SECRET = 'sub';
}

echo Config::BASE, ' ', Config::DOUBLE, "\n";
echo Config::TABLE[10], ' ', Config::TABLE['k'], "\n";
echo Config::$limit, ' ', implode(',', Config::$seen), "\n";
echo Config::secret(), ' ', SubConfig::secret(), "\n";
echo Config::hidden(), "\n";
// An inherited constant is reachable through the subclass.
echo SubConfig::BASE, ' ', SubConfig::DOUBLE, "\n";

// ---- visibility -------------------------------------------------------------

function attempt(callable $f): void
{
    try {
        $v = $f();
        echo is_array($v) ? 'array' : var_export($v, true), "\n";
    } catch (Error $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

attempt(static fn() => Config::SECRET);
attempt(static fn() => Config::HIDDEN);
// A private constant is not inherited: through the subclass it is undefined.
attempt(static fn() => SubConfig::HIDDEN);
attempt(static fn() => Config::MISSING);

class Vault
{
    public static $open = 'open';
    protected static $guarded = 'guarded';
    private static $sealed = 'sealed';

    public static function peek(): string
    {
        return self::$open . ',' . self::$guarded . ',' . self::$sealed;
    }
}

class SubVault extends Vault
{
    public static function peekInherited(): string
    {
        // A protected static is reachable from the subclass; the private one
        // is not, and it is the *referenced* class the error names.
        return static::$guarded;
    }
}

echo Vault::peek(), "\n";
echo SubVault::peekInherited(), "\n";
attempt(static fn() => Vault::$guarded);
attempt(static fn() => Vault::$sealed);
attempt(static fn() => SubVault::$sealed);
attempt(static fn() => Vault::$absent);
var_dump(isset(Vault::$sealed), isset(Vault::$absent), isset(Vault::$open));

// ---- one cell, shared down the chain ----------------------------------------

class Base
{
    public static $shared = 'base';
    public static $own = 'base';
}

class Mid extends Base
{
    // Redeclaring gives this class (and its children) their own storage.
    public static $own = 'mid';
}

class Leaf extends Mid
{
}

Leaf::$shared = 'written through Leaf';
echo Base::$shared, "\n";
Mid::$own = 'written through Mid';
echo Base::$own, ' | ', Mid::$own, ' | ', Leaf::$own, "\n";

// A reference binds the cell itself, so both spellings see the write.
$r = &Base::$shared;
$r = 'through the reference';
echo Leaf::$shared, "\n";
Leaf::$shared = 'back through the class';
echo $r, "\n";

// ---- typed static properties ------------------------------------------------

class Typed
{
    public static int $count = 0;
    public static ?string $label = null;
    public static int $late;
}

Typed::$count = '7';            // coerced in weak mode
var_dump(Typed::$count);
Typed::$count += 1.0;
var_dump(Typed::$count);
Typed::$label = 'set';
var_dump(Typed::$label);
attempt(static fn() => Typed::$count = 'seven');
attempt(static fn() => Typed::$late);
var_dump(isset(Typed::$late));
Typed::$late = 3;
var_dump(Typed::$late);

// ---- late static binding ----------------------------------------------------

class Model
{
    public const KIND = 'model';
    public static $registry = 'model';

    public static function kind(): string
    {
        return static::KIND;
    }

    public static function registry(): string
    {
        return static::$registry;
    }

    public static function make(): static
    {
        return new static();
    }

    public static function describe(): string
    {
        // A forwarding call keeps the called class; a named one does not.
        return static::kind() . '|' . self::kind() . '|' . Model::kind();
    }

    public function viaInstance(): string
    {
        return static::class . '|' . self::class . '|' . static::kind();
    }
}

class Post extends Model
{
    public const KIND = 'post';
    public static $registry = 'post';

    public static function describeParent(): string
    {
        return parent::kind() . '|' . parent::describe();
    }
}

echo Model::kind(), ' ', Post::kind(), "\n";
echo Model::registry(), ' ', Post::registry(), "\n";
echo get_class(Model::make()), ' ', get_class(Post::make()), "\n";
echo Model::describe(), "\n";
echo Post::describe(), "\n";
echo Post::describeParent(), "\n";
echo (new Post())->viaInstance(), "\n";

// ---- a self-referencing constant is an error, not a hang --------------------

class Loop
{
    public const A = self::A;
}

try {
    echo Loop::A;
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
