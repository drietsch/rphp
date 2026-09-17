<?php
// __destruct runs when the last handle goes away (at the next op boundary),
// on unset, on reassignment, when a function's locals die, and at script end
// in php's order: globals in reverse order (those held only by their
// variable), then the rest in creation order.

class D {
    public $n;
    public function __construct($n) { $this->n = $n; echo "ctor{$this->n} "; }
    public function __destruct() { echo "dtor{$this->n} "; }
}
function scope() { $l = new D("local"); echo "in scope "; }
scope();
echo "\n";
$a = new D("a");
$a = null;
echo "| ";
$b = new D("b");
unset($b);
echo "| ";
$c = new D("c");
$c = new D("c2");
echo "| ";
$keep = new D("k1");
$alias = $keep;
$keep = null;
echo "still alive ";
$alias = null;
echo "\n";
function make() { return new D("temp"); }
make();
echo "| discarded\n";
$x = new D("x");
$y = new D("y");
$z = new D("z");
$w = $z;
register_shutdown_function(function () { echo "shutdown "; });
echo "end\n";
