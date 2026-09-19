<?php
// The Core introspection functions: what a class uses, what a resource is,
// and what the program has defined.
trait Alpha {}
trait Beta {}
class Base { use Alpha; }
class Kid extends Base { use Beta; }

var_dump(class_uses(new Kid), class_uses('Base'), class_uses(new Kid, false));
var_dump(class_uses('NoSuchClass'));

$h = fopen('php://memory', 'r+');
var_dump(get_resource_type($h));
fclose($h);

// The flat form of `get_defined_constants()` holds the engine's own.
$c = get_defined_constants();
var_dump(is_array($c), $c['PHP_EOL'], $c['PHP_INT_SIZE'], array_key_exists('M_PI', $c));
define('MY_OWN_CONSTANT', 'mine');
$cat = get_defined_constants(true);
var_dump($cat['user'], array_key_exists('PHP_EOL', $cat['Core']));

// `get_defined_functions()` splits internal from user, both lower-cased and
// the user half in declaration order.
$f = get_defined_functions();
var_dump(array_keys($f), $f['user'], in_array('strlen', $f['internal'], true));
function Zebra() {}
function apple() {}
var_dump(get_defined_functions()['user']);

// `is_a()` and `is_subclass_of()` over names as well as objects.
interface Marker {}
class Marked extends Kid implements Marker {}
var_dump(
    is_a('Marked', 'Marked', true),
    is_a('Marked', 'Base', true),
    is_a('Marked', 'Marker', true),
    is_a(new Marked(), 'Base'),
    is_a('Marked', 'Base'),
    is_subclass_of('Marked', 'Base'),
    is_subclass_of('Marked', 'Marked'),
);

// The 8.5 handler getters.
var_dump(get_error_handler(), get_exception_handler());
$h1 = function (): void {};
set_error_handler($h1);
set_exception_handler($h1);
var_dump(get_error_handler() === $h1, get_exception_handler() === $h1);
restore_error_handler();
var_dump(get_error_handler());
