<?php
// One call site, many receivers: subclasses overriding, a private method
// shadowed in a subclass called from the parent's scope, __call fallback,
// static methods through instances, a closure receiver, an object swapped
// for one of another class, and a method that appears via a trait.
class A { function who() { return 'A'; } private function p() { return 'A::p'; } function callP() { return $this->p(); } static function s() { return static::class; } }
class B extends A { function who() { return 'B'; } private function p() { return 'B::p'; } }
class C extends B { function who() { return 'C'; } }
class M { function __call($n, $a) { return "magic $n"; } }
trait T { function tm() { return 'T::tm'; } }
class D { use T; function who() { return 'D'; } }
function site($o) { return $o->who(); }
function siteP(A $o) { return $o->callP(); }
function siteS($o) { return $o->s(); }
function siteAny($o) { return $o->tm(); }
foreach ([new A, new B, new C, new A, new C, new M, new D] as $o) {
    try { var_dump(site($o)); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
foreach ([new A, new B, new C, new B] as $o) { var_dump(siteP($o)); }
foreach ([new A, new B, new C] as $o) { var_dump(siteS($o)); }
foreach ([new D, new M, new D] as $o) { try { var_dump(siteAny($o)); } catch (Error $e) { echo $e->getMessage(), "\n"; } }
$f = function () { return 'closure'; };
foreach ([$f, new M] as $o) { try { var_dump($o->__invoke()); } catch (Error $e) { echo $e->getMessage(), "\n"; } }
class V { private function hidden() { return 'h'; } function viaSelf() { return $this->hidden(); } }
$v = new V; var_dump($v->viaSelf());
try { $v->hidden(); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$g = fn() => $this->hidden();
var_dump(Closure::bind($g, $v, V::class)());
