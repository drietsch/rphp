<?php
// Tier-A differential: var_export over every value kind — scalars (incl.
// PHP_INT_MIN's expression form and float `.0` / exponent layout under
// serialize_precision=-1), quoted strings with NUL splicing, nested arrays,
// objects as `\Class::__set_state(array(...))`, and the `$return` flag.

var_export(null); echo "\n";
var_export(true); echo "\n";
var_export(false); echo "\n";
var_export(42); echo "\n";
var_export(-42); echo "\n";
var_export(constant('PHP_INT_MIN')); echo "\n";
var_export(constant('PHP_INT_MAX')); echo "\n";

// Floats: integral values keep `.0`; the exponent form switches at 1e17 / 1e-5.
var_export(1.0); echo "\n";
var_export(0.1); echo "\n";
var_export(-0.0); echo "\n";
var_export(2.5); echo "\n";
var_export(100.0); echo "\n";
var_export(1e15); echo "\n";
var_export(1e17); echo "\n";
var_export(1e25); echo "\n";
var_export(0.0001); echo "\n";
var_export(1e-5); echo "\n";
var_export(0.1 + 0.2); echo "\n";
var_export(123456789012345678.0); echo "\n";
var_export(fdiv(1, 0)); echo "\n";
var_export(fdiv(-1, 0)); echo "\n";
var_export(fdiv(0, 0)); echo "\n";

// Strings: quotes and backslashes escaped, NUL bytes spliced as "\0".
var_export("abc"); echo "\n";
var_export("it's"); echo "\n";
var_export("back\\slash"); echo "\n";
var_export("nul\0byte"); echo "\n";
var_export("\0"); echo "\n";
var_export("a\0"); echo "\n";
var_export("multi\nline"); echo "\n";
var_export(""); echo "\n";

// Arrays.
var_export([]); echo "\n";
var_export([1, 2, 3]); echo "\n";
var_export(["a" => 1, "b" => [1, 2], 5 => "x", "q'q" => "v'v"]); echo "\n";
var_export([[[]]]); echo "\n";
var_export([-1 => 'neg', "k" => null, "f" => false]); echo "\n";
var_export(["a\0b" => 1]); echo "\n";
var_export([1.0, 2.5, [true]]); echo "\n";

// Objects: declared properties in declaration order regardless of visibility.
class Foo {
    public $p = 1;
    protected $q = [1];
    private $r = null;
}
$f = new Foo;
var_export($f); echo "\n";
var_export([$f, "x" => [$f]]); echo "\n";

class Bar extends Foo {
    public $z = 2.0;
}
$b = new Bar;
var_export($b); echo "\n";

// $return = true hands the text back instead of printing it.
$s = var_export([1, "two" => 2.0], true);
echo strlen($s), "\n";
echo $s, "\n";
var_dump(var_export("x", true));
var_dump(var_export("y"));
echo "\n";
