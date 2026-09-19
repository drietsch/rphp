<?php
// A class the program does not declare comes back as php's
// `__PHP_Incomplete_Class`, whose first property is the name that could not
// be resolved — and which grows the rest without the dynamic-property
// deprecation.
$o = unserialize('O:5:"Gone1":2:{s:1:"a";i:1;s:1:"b";s:2:"xy";}');
var_dump(get_class($o), $o instanceof __PHP_Incomplete_Class);
print_r($o);
echo "\n";
var_dump(get_object_vars($o));

// `allowed_classes => false` makes every class unresolvable, declared or not.
class Known { public $a = 1; }
$blocked = unserialize(serialize(new Known()), ['allowed_classes' => false]);
print_r($blocked);
echo "\n";

// A declared class still round-trips.
$kept = unserialize(serialize(new Known()));
var_dump(get_class($kept), $kept->a);

// stdClass and an `#[AllowDynamicProperties]`-style class take new
// properties in silence; an ordinary one deprecates.
$std = unserialize('O:8:"stdClass":1:{s:1:"z";i:9;}');
var_dump($std->z);
