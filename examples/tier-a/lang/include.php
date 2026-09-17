<?php
// include / require (_once): path resolution relative to the including
// file's directory, return values, scope sharing (variables flow both ways,
// functions and classes are declared globally), `_once` short-circuiting,
// includes from inside a function seeing that function's scope, and the
// warnings + `false` of a missing include.

$set_by_includer = 'outer';
$r = require __DIR__ . '/include-target.php';
var_dump($r, $set_by_target, from_target(), (new FromTarget)->origin);
$include_count = 5;
$r2 = include 'include-target.php';                  // resolved against the including file's dir
var_dump($r2['count']);
var_dump(include_once __DIR__ . '/include-target.php');
var_dump(require_once 'include-target.php');

function scoped() {
    $set_by_includer = 'inner';
    $got = include __DIR__ . '/include-target.php';
    return [$set_by_target, $got['count'], isset($include_count)];
}
var_dump(scoped());
var_dump($set_by_target);                            // the global one is untouched by the call

$missing = @include 'no-such-file.php';
var_dump($missing);
$missing2 = include 'no-such-file.php';
var_dump($missing2);
echo "end\n";
