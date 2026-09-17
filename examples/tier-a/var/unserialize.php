<?php
// Tier-A differential: unserialize() — every scalar form (incl. `d:INF;`,
// `d:-0;`, the `S:` escaped-string format with its deprecation), arrays with
// numeric-string keys, objects of declared classes with mangled protected /
// private names and dynamic properties (8.2 deprecation), `r:`/`R:`
// back-references, the `allowed_classes` / `max_depth` options, and php's
// exact warning texts and error offsets for malformed input.

var_dump(unserialize('N;'));
var_dump(unserialize('b:1;'), unserialize('b:0;'));
var_dump(unserialize('i:42;'), unserialize('i:-7;'), unserialize('i:+5;'), unserialize('i:05;'));
var_dump(unserialize('d:0.1;'), unserialize('d:1;'), unserialize('d:INF;'), unserialize('d:-INF;'), unserialize('d:1.0E+25;'), unserialize('d:-0;'));
var_dump(unserialize('d:1e3;'), unserialize('d:.5;'), unserialize('d:5.;'), unserialize('d:-.5e+1;'));
var_dump(is_nan(unserialize('d:NAN;')));
var_dump(unserialize('s:3:"abc";'), unserialize('s:0:"";'), unserialize('s:3:"a' . "\0" . 'b";'));
var_dump(unserialize('a:0:{}'));
var_dump(unserialize('a:2:{i:0;i:1;s:1:"k";a:1:{i:0;b:1;}}'));
var_dump(unserialize('a:1:{s:1:"5";i:1;}'));
var_dump(unserialize('a:2:{i:0;i:1;i:0;i:2;}'));
var_dump(unserialize('a:2:{i:0;i:1;i:1;R:2;}'));
var_dump(unserialize('a:2:{i:0;a:1:{i:0;i:5;}i:1;R:3;}'));
var_dump(unserialize('S:3:"a\62c";'));
var_dump(unserialize('i:99999999999999999999;'));

class Foo {
    public $p = 1;
    protected $q = 2;
    private $r = 3;
}
var_dump(unserialize('O:3:"Foo":1:{s:1:"p";i:9;}'));
var_dump(unserialize('O:3:"Foo":3:{s:1:"p";i:9;s:4:"' . "\0*\0" . 'q";i:8;s:6:"' . "\0Foo\0" . 'r";i:7;}'));
var_dump(unserialize('O:3:"Foo":1:{s:3:"new";i:9;}'));
var_dump(unserialize('O:3:"Foo":1:{i:0;i:1;}'));
var_dump(unserialize('O:3:"Foo":0:{}', ['allowed_classes' => ['foo']]));
var_dump(unserialize('O:3:"Foo":0:{}', ['allowed_classes' => true]));
var_dump(unserialize('a:2:{i:0;O:3:"Foo":0:{}i:1;r:2;}'));
var_dump(unserialize('O:3:"Foo":2:{s:1:"p";i:1;s:1:"x";R:2;}'));

echo "--- extra data ---\n";
var_dump(unserialize('i:42;junk'));
var_dump(unserialize('a:1:{i:0;i:1;}}'));
var_dump(unserialize('O:3:"Foo":0:{}extra'));

echo "--- errors ---\n";
var_dump(unserialize(''));
var_dump(unserialize(false));
var_dump(unserialize('x'));
var_dump(unserialize('}'));
var_dump(unserialize('N'));
var_dump(unserialize('i:42'));
var_dump(unserialize('i:;'));
var_dump(unserialize('b:2;'));
var_dump(unserialize('b:;'));
var_dump(unserialize('d:abc;'));
var_dump(unserialize('d:1e;'));
var_dump(unserialize('s:5:"abc";'));
var_dump(unserialize('s:3:"abc"'));
var_dump(unserialize('s:3:"abc"}'));
var_dump(unserialize('s:-1:"";'));
var_dump(unserialize('s:1'));
var_dump(unserialize('s:1:"'));
var_dump(unserialize('a:2:{i:0;i:1;}'));
var_dump(unserialize('a:1:{i:0;i:1;'));
var_dump(unserialize('a:1:{i:0;'));
var_dump(unserialize('a:1:{'));
var_dump(unserialize('a:-1:{}'));
var_dump(unserialize('a:1:{d:1.5;i:1;}'));
var_dump(unserialize('a:1:{b:1;i:1;}'));
var_dump(unserialize('a:1:{N;i:1;}'));
var_dump(unserialize('a:1:{i:0;R:0;}'));
var_dump(unserialize('a:1:{i:0;R:9;}'));
var_dump(unserialize('a:2:{i:0;i:5;i:1;r:1;}'));
var_dump(unserialize('R:1;'));
var_dump(unserialize('r:1;'));
var_dump(unserialize('O:3:"Foo":0:'));
var_dump(unserialize('O:3:"Foo":0:{'));
var_dump(unserialize('O:3:"Foo":-1:{}'));
var_dump(unserialize('O:0:"":0:{}'));
var_dump(unserialize('O:3:"Foo"'));
var_dump(unserialize('O:3:"Foo":'));
var_dump(unserialize('O:3:"Foo":0'));
var_dump(unserialize('a:1:{i:0;a:1:{i:0;a:1:{i:0;i:1;}}}', ['max_depth' => 2]));
var_dump(unserialize('a:1:{i:0;a:1:{i:0;a:1:{i:0;i:1;}}}', ['max_depth' => 3]));
var_dump(unserialize('a:1:{i:0;O:3:"Foo":0:{}}', ['max_depth' => 1]));
