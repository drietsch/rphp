<?php
// The tier-a oracle runs with `zend.assertions=-1`, where php does not
// compile an `assert()` at all — so its argument is never evaluated and the
// call is simply `true`.
var_dump(ini_get('zend.assertions'));
$called = false;
$probe = function () use (&$called) { $called = true; return false; };
var_dump(assert($probe()), $called);
var_dump(assert(false), assert(1 === 2), assert(false, 'ignored'));
$counter = 0;
$bump = function () use (&$counter) { $counter++; return true; };
assert($bump());
assert($bump());
var_dump($counter);
