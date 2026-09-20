<?php
// Exceptions and traces through `yield from`, and the frames php shows for
// a generator driven by foreach (none for the loop itself) versus by its
// methods (`Generator->next()` beneath `[internal function]`).
class R { public function resolve($a, $b) { yield 1; throw new RuntimeException("x"); } }
class C { public function get() { foreach ((new R)->resolve(1, 2) as $v) { echo $v, "\n"; } } }
function f() { $c = new C; $c->get(); }
try { f(); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; foreach ($e->getTrace() as $t) { echo json_encode(array_intersect_key($t, ['function'=>1,'class'=>1,'type'=>1,'line'=>1])), "\n"; } }

$g = (function() { yield 1; throw new LogicException("y"); })();
try { foreach ($g as $v); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
$g = (function() { yield 1; throw new LogicException("y2"); })();
try { $g->current(); $g->next(); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }

function inner() { yield 1; throw new DomainException("z"); }
function gen() { yield from inner(); }
try { foreach (gen() as $v); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }

// A `try` around the `yield from` catches what the delegate throws.
function gen2() { try { yield from inner(); } catch (Exception $e) { echo "caught ", $e->getMessage(), "\n"; echo $e->getTraceAsString(), "\n"; } yield 99; }
foreach (gen2() as $v) echo $v, "\n";

// The delegate's return value is the value of the expression.
function inner2() { yield 1; yield 2; return 7; }
function gen3() { $r = yield from inner2(); echo "r=$r\n"; yield 3; }
foreach (gen3() as $k => $v) echo "$k=>$v\n";

// Throwing at the delegate's first touch.
function inner3() { throw new UnexpectedValueException("first"); yield 1; }
function gen4() { try { yield from inner3(); } catch (UnexpectedValueException $e) { echo "caught first\n"; } yield 4; }
foreach (gen4() as $v) echo $v, "\n";

// Three levels, and `send()` through the delegates.
function deep3() { $x = yield 'a'; echo "deep3 got $x\n"; $y = yield 'b'; echo "deep3 got $y\n"; return 'D3'; }
function deep2() { $r = yield from deep3(); echo "deep2 saw $r\n"; return 'D2'; }
function deep1() { $r = yield from deep2(); echo "deep1 saw $r\n"; }
$g = deep1();
echo $g->current(), "\n";
echo $g->send('one'), "\n";
var_dump($g->send('two'));
var_dump($g->valid());

// `throw()` into a delegate, caught there.
function catching() { try { yield 1; } catch (Exception $e) { echo "delegate caught ", $e->getMessage(), "\n"; } yield 2; }
function outer5() { yield from catching(); yield 3; }
$g = outer5();
echo $g->current(), "\n";
echo $g->throw(new Exception("thrown in")), "\n";
$g->next(); echo $g->current(), "\n";

// finally in the outer runs when the delegate's exception escapes.
function outer6() { try { yield from inner(); } finally { echo "outer finally\n"; } }
try { foreach (outer6() as $v) echo $v, "\n"; } catch (DomainException $e) { echo "escaped: ", $e->getMessage(), "\n"; }

// A generator whose `yield from` delegate throws synchronously (a plain
// method), driven by foreach: no Generator frame, the body's frame at the
// loop; and the first step (the implicit rewind) is no frame either.
class Inner { function resolve($r, $m) { throw new RuntimeException("nf"); } }
class Inner2 { function resolve($r, $m) { return []; } }
class Tr { function __construct(private $inner) {} function resolve($r, $m): iterable { yield from $this->inner->resolve($r, $m); echo "after\n"; } }
class AR { function getArguments($rs) { foreach ($rs as $resolver) { foreach ($resolver->resolve(1, 2) as $a) {} } } }
try { (new AR)->getArguments([new Tr(new Inner2), new Tr(new Inner)]); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
function g1() { yield from thrower(); }
function thrower() { throw new LogicException("t"); yield 1; }
try { foreach (g1() as $v) {} } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
$g = g1(); try { $g->current(); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
$g = (new Tr(new Inner))->resolve(1, 2); try { $g->rewind(); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
