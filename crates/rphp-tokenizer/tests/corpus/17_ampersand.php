<?php
$a = &$b; $a = & $b; $a = &
$b; $a = &/* c */$b; $a = & /* c */ $b; $a = &// c
$b; $a = &# c
$b; $a = &#[x]
$b; $a & $b; $a && $b; $a &= $b; $a &...$b; $a & ...$b; $a &.. $b; $a &. $b; &$; &$1; & $ x; &&$a; & &$a; &&&$a; & && $a;
function f(&$x, & $y, &...$z, & ...$w, array &$v, ?int & $u, Foo|Bar &$t, Foo&Bar $s, Foo & Bar $r, (Foo&Bar)|null $q) {}
$f = fn&($x) => $x; $f = fn &($x) => $x; function &g() {} function & h() {}
foreach ($a as &$v) {} foreach ($a as $k => &$v) {} [&$a, & $b] = $c; $x = [&$a]; f(&$a); f(& $a);
$a & /* multi
line */ $b; $a &

$b; $a & #
$b; $a &/**/$b; $a &/**//**/$b; $a &/**/ /**/ $b; $a & /* $ */ $b; $a & // $
$b; $a &# $
$b; $a &
# comment
# another
$b; $a & /* a */ // b
$c; $a & //
...$b; $a &

...$b; $a &.$b; $a &..$b; $a &....$b;
$a &
