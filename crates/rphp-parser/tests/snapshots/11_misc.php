<?php
exit(1); exit; die; die("x"); print 1; $r = print "y";
include "a"; include_once "b"; require "c"; require_once "d"; eval("1");
isset($a, $b[0], $c->d, C::$e); empty($x);
unset($a, $b[0]); global $g, $$h; static $s = 1, $t;
$m = match(true) { 1, 2 => "a", default => "b" };
$n = match($x) { };
throw new E; $t = $a ?? throw new E;
$arr = [1, 2 => 3, ...$c, "k" => &$v]; $leg = array(1, 2);
$e = [1, 2][0] . "abc"[1] . array(1)[0];
(void) f();
