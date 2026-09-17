<?php
// Tier-A: traits — member copy-in, per-using-class static properties, trait
// constants, abstract trait methods, conflict resolution with `insteadof` and
// aliasing/visibility changes with `as`. Differentially tested against PHP 8.5.

trait Counting {
    // Each using class gets its *own* copy of a trait's static property.
    public static int $count = 0;
    public int $ticks = 0;
    const STEP = 1;

    public static function bump(): int {
        return static::$count += static::STEP;
    }

    public function tick(): int {
        $this->ticks += self::STEP;
        return $this->ticks;
    }
}

trait Greeting {
    public function hello(): string {
        return 'hello from ' . $this->who();
    }

    // A trait may require the using class to supply a method.
    abstract public function who(): string;
}

class Widget {
    use Counting;
    use Greeting;

    public function who(): string {
        return 'Widget';
    }
}

class Gadget {
    use Counting, Greeting;

    public function who(): string {
        return 'Gadget';
    }
}

Widget::bump();
Widget::bump();
Gadget::bump();
echo Widget::$count, ' ', Gadget::$count, "\n";   // 2 1 — separate storage
echo Widget::STEP, ' ', Gadget::STEP, "\n";       // the trait's constant

$w = new Widget();
echo $w->tick(), $w->tick(), ' ', $w->hello(), "\n";
echo (new Gadget())->hello(), "\n";

// Conflict resolution: two traits define the same method.
trait English {
    public function say(): string { return 'Hello'; }
    public function shout(): string { return strtoupper($this->say()); }
}
trait French {
    public function say(): string { return 'Bonjour'; }
}

class Polyglot {
    use English, French {
        English::say insteadof French;
        French::say as sayFrench;
        shout as protected loudly;
    }

    public function both(): string {
        return $this->say() . '/' . $this->sayFrench() . '/' . $this->loudly();
    }
}

$p = new Polyglot();
echo $p->say(), ' ', $p->sayFrench(), "\n";
echo $p->both(), "\n";
try {
    $p->loudly();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// A trait member is copied in, so the using class is the declaring scope: a
// private member of the trait is private to each using class.
trait Secretive {
    private string $secret = 's';
    public function reveal(): string { return $this->secret; }
}
class Vault { use Secretive; }
echo (new Vault())->reveal(), "\n";

// A trait can use another trait; the members flatten transitively.
trait Inner { public function deep(): string { return 'deep'; } }
trait Outer { use Inner; public function shallow(): string { return 'shallow/' . $this->deep(); } }
class Nested { use Outer; }
echo (new Nested())->shallow(), "\n";

// The using class's own member always wins over the trait's.
trait Defaults { public function label(): string { return 'trait'; } }
class Overrider { use Defaults; public function label(): string { return 'class'; } }
echo (new Overrider())->label(), "\n";

// A trait is not instantiable and not a type.
try {
    new Counting();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
var_dump(class_exists('Widget'), trait_exists('Counting'), $w instanceof Widget);
