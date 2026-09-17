<?php
// Tier-A: inheritance, visibility & instanceof — single inheritance with virtual
// dispatch, `parent::`/`self::` scoped calls, protected/private access from the
// right context, and `instanceof` over the class chain. Differentially tested
// against stock PHP 8.5. (Visibility *violations* are fatal errors, so this
// snippet only performs legal accesses.)

class Shape {
    protected $name;
    private $id;

    public function __construct($name, $id) {
        $this->name = $name;
        $this->id = $id;
    }

    public function area() {
        return 0;
    }

    // Virtual dispatch: area() resolves on the runtime class.
    public function describe() {
        return $this->name . "#" . $this->id . " area=" . $this->area();
    }

    // A private member is reachable only from this class.
    public function tag() {
        return self::prefix() . $this->id;
    }

    private function prefix() {
        return "id:";
    }
}

class Circle extends Shape {
    protected $r;

    public function __construct($id, $r) {
        parent::__construct("circle", $id);   // scoped call up the chain
        $this->r = $r;
    }

    public function area() {                    // override
        return 3 * $this->r * $this->r;
    }
}

class Rectangle extends Shape {
    public $w;
    public $h;

    public function __construct($id, $w, $h) {
        parent::__construct("rect", $id);
        $this->w = $w;
        $this->h = $h;
    }

    public function area() {
        return $this->w * $this->h;
    }
}

$c = new Circle(1, 10);
$r = new Rectangle(2, 4, 5);

echo $c->describe(), "\n";        // circle#1 area=300  (virtual area())
echo $r->describe(), "\n";        // rect#2 area=20
echo $c->tag(), "\n";             // id:1  (self:: + private method)
echo $r->area(), "\n";            // 20

// A subclass reads an inherited protected property in its own method.
class Named extends Shape {
    public function label() {
        return $this->name;          // protected, inherited — legal here
    }
}
$n = new Named("widget", 9);
echo $n->label(), "\n";           // widget

// instanceof walks the inheritance chain.
if ($c instanceof Circle) { echo "circle\n"; }
if ($c instanceof Shape) { echo "is-shape\n"; }       // true via parent
if ($r instanceof Circle) { echo "no\n"; } else { echo "rect-not-circle\n"; }

// Polymorphism through a shared base type.
function report($shape) {
    if ($shape instanceof Shape) {
        return $shape->describe();
    }
    return "not a shape";
}
echo report($c), "\n";            // circle#1 area=300
echo report($r), "\n";            // rect#2 area=20
echo report(42), "\n";            // not a shape

// json_encode emits public properties only (protected/private are omitted).
echo json_encode($r), "\n";       // {"w":4,"h":5}
echo json_encode($c), "\n";       // {}  — Circle has no public properties
