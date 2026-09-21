<?php
// yield from chains: the root's resume runs the innermost body (php's
// leaf), keys and sent values pass through, a leaf's return completes
// the yield from above it, exceptions surface at each yield from on the
// way up (finally blocks run), traces show the chain, and a return out
// of a try/finally finishes the generator.
function leaf() { try { $x = yield 'k1' => 'v1'; echo "leaf got ", var_export($x, true), "\n"; yield 'k2' => 'v2'; return 'LEAF'; } finally { echo "leaf finally\n"; } }
function mid() { $r = yield from leaf(); echo "mid saw $r\n"; yield 'm' => 'mv'; return 'MID'; }
function top() { $r = yield from mid(); echo "top saw $r\n"; yield 't' => 'tv'; }
foreach (top() as $k => $v) echo "$k=$v\n";
$g = top(); var_dump($g->key(), $g->current()); var_dump($g->send('S')); var_dump($g->key()); $g->next(); var_dump($g->current()); $g->next(); var_dump($g->current(), $g->valid()); $g->next(); var_dump($g->valid());
var_dump(iterator_to_array(top(), false));
function thrower() { yield 1; throw new RuntimeException("deep"); }
function m2() { try { yield from thrower(); } finally { echo "m2 finally\n"; } yield 'unreached'; }
function t2() { try { yield from m2(); } catch (RuntimeException $e) { echo "t2 caught ", $e->getMessage(), "\n"; print_r(array_map(fn($f) => $f['function'], $e->getTrace())); } yield 'after'; }
foreach (t2() as $v) echo "v=$v\n";
function arrgen() { $r = yield from [10, 20]; var_dump($r); $r2 = yield from new ArrayIterator(['a' => 1]); var_dump($r2); yield 'end'; }
foreach (arrgen() as $k => $v) echo "$k:$v\n";
function tg() { yield from thrower(); }
$x = tg(); var_dump($x->current()); try { $x->next(); } catch (RuntimeException $e) { echo "outer caught ", $e->getMessage(), "\n"; } var_dump($x->valid());
try { foreach (tg() as $v) { echo "got $v\n"; } } catch (RuntimeException $e) { echo "loop caught\n"; }
function catcher() { try { yield 1; yield 2; } catch (LogicException $e) { echo "inner caught ", $e->getMessage(), "\n"; yield 'recovered'; } }
function o3() { yield from catcher(); yield 'o3 end'; }
$g3 = o3(); var_dump($g3->current()); var_dump($g3->throw(new LogicException("thrown"))); $g3->next(); var_dump($g3->current());
function self_delegate() { $g = self_delegate_inner(); yield from $g; yield from $g; }
function self_delegate_inner() { yield 1; yield 2; }
foreach (self_delegate() as $v) echo $v; echo "\n";
