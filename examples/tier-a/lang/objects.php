<?php
// Tier-A: objects & classes vertical slice — class declarations, properties with
// defaults, constructors, methods, `$this`, method chaining, reference
// semantics, and object encoding. Differentially tested against stock PHP 8.5.

class Point {
    public $x = 0;
    public $y = 0;

    public function __construct($x, $y) {
        $this->x = $x;
        $this->y = $y;
    }

    // Fluent mutator: returns $this so calls chain.
    public function move($dx, $dy) {
        $this->x = $this->x + $dx;
        $this->y = $this->y + $dy;
        return $this;
    }

    public function lenSq() {
        return $this->x * $this->x + $this->y * $this->y;
    }

    public function label() {
        return "(" . $this->x . ", " . $this->y . ")";
    }
}

$p = new Point(3, 4);
echo $p->label(), "\n";           // (3, 4)
echo $p->lenSq(), "\n";           // 25

// Method chaining mutates the same instance.
$p->move(1, 1)->move(-2, 0);
echo $p->label(), "\n";           // (2, 5)

// Property defaults apply when the constructor leaves them untouched.
class Bag {
    public $items = 0;
    public $name = "bag";
    public $open = true;
    public $ratio = 0.5;
    public $empty = null;
}
$b = new Bag();
echo $b->items, " ", $b->name, " ", $b->ratio, "\n";   // 0 bag 0.5
if ($b->open) { echo "open\n"; } else { echo "shut\n"; }            // open
if ($b->empty === null) { echo "null\n"; } else { echo "set\n"; }   // null

// Reference semantics: assignment aliases the same object, `===` is identity.
$q = $p;
$q->move(10, 10);
echo $p->label(), "\n";                                // (12, 15) — seen through $p
if ($p === $q) { echo "same\n"; } else { echo "diff\n"; }   // same
$r = new Point(12, 15);
if ($p === $r) { echo "same\n"; } else { echo "diff\n"; }   // diff — distinct instances

// Objects passed to / returned from functions keep their identity.
function shift($pt) {
    return $pt->move(100, 0);
}
echo shift($p)->label(), "\n";                         // (112, 15)
echo $p->x, "\n";                                      // 112

// An object encodes to a JSON object over its public properties.
echo json_encode(new Point(1, 2)), "\n";               // {"x":1,"y":2}
echo json_encode($b), "\n";

// A class can be used before its declaration appears (declarations are hoisted).
$g = new Greeter("hi");
echo $g->greet("world"), "\n";                         // hi, world

class Greeter {
    public $prefix = "";
    public function __construct($prefix) {
        $this->prefix = $prefix;
    }
    public function greet($who) {
        return $this->prefix . ", " . $who;
    }
}
