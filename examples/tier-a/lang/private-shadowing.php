<?php

// An ancestor's private property and a subclass's of the same name are two
// slots on one object: each class's own code sees its own, and every
// projection shows both under php's mangled keys.
abstract class Base
{
    public function __construct(private string $out) {}

    public function baseOut() { return $this->out; }
    public function baseSet($v) { $this->out = $v; }
    public function baseIsset() { return isset($this->out); }
    public function baseVars() { return get_object_vars($this); }
}
class Child extends Base
{
    public function __construct(private string $out)
    {
        parent::__construct('parent-' . $out);
    }

    public function childOut() { return $this->out; }
    public function childVars() { return get_object_vars($this); }
}
$c = new Child('x');
var_dump($c->baseOut(), $c->childOut(), $c->baseIsset());
$c->baseSet('changed');
var_dump($c->baseOut(), $c->childOut());
var_dump($c);
print_r($c);
echo "\n";
var_export($c);
echo "\n";
var_dump((array) $c, array_keys((array) $c) === ["\0Base\0out", "\0Child\0out"]);
var_dump($c->baseVars(), $c->childVars());
$wire = serialize($c);
var_dump($wire);
$back = unserialize($wire);
var_dump($back->baseOut(), $back->childOut(), $back == $c);
$copy = clone $c;
$copy->baseSet('copied');
var_dump($c->baseOut(), $copy->baseOut(), $copy->childOut());

// Three levels, with a public re-declaration and unset() in the mix.
class A { private $p = 'A'; protected $q = 'A'; public function ap() { return $this->p; } }
class B extends A { private $p = 'B'; public function bp() { return $this->p; } }
class C extends B { private $p = 'C'; public $q = 'C'; public function cp() { return $this->p; } }
class D extends C { public function unsetIt() { unset($this->p); } }
$d = new D();
var_dump($d->ap(), $d->bp(), $d->cp(), count((array) $d));
$d->unsetIt();
var_dump($d->ap(), $d->bp(), count((array) $d));
foreach ($d as $k => $v) {
    echo "iter $k\n";
}
var_dump(json_encode($d));

// Objects compare property by property, in slot order, and an object
// beside a number is that number's kind of `1`.
class Q { public $x = 1; public $y = 'a'; }
$q1 = new Q();
$q2 = new Q();
var_dump($q1 == $q2, $q1 != $q2, $q1 === $q2, $q1 <=> $q2);
$q2->y = 'b';
var_dump($q1 == $q2, $q1 < $q2, $q1 <=> $q2, $q2 <=> $q1);
$q3 = new Q();
$q3->extra = 1;
var_dump($q1 == $q3, $q1 <=> $q3, $q3 <=> $q1);
$n1 = new stdClass();
$n1->o = new Q();
$n2 = new stdClass();
$n2->o = new Q();
var_dump($n1 == $n2, $n1 <=> $n2);
$n2->o->x = 0;
var_dump($n1 == $n2, $n1 <=> $n2);
var_dump(new Q() == new stdClass(), new Q() <=> new stdClass());
var_dump(new Q() == 1, new Q() <=> 5, new Q() == 1.0, new Q() == true, new Q() == null);
var_dump([new Q()] == [new Q()], in_array(new Q(), [new Q()]), array_search(new Q(), [1, new Q()]));
var_dump(new Child('a') == new Child('a'), new Child('a') == new Child('b'));
