<?php
// Sections, `key[]` / `key[offset]` arrays and numeric keys.
define('K', 'from_const');

$ini = <<<'INI'
top = 1
list[] = a
list[] = b
map[x] = 1
map[ y ] = 2
map[5] = 3
map[] = 4
map[K] = 5
map["q"] = 6
scalar = s
scalar[] = replaced

[first]
a = 1
[ spaced ]
b = 2
["quoted"]
c = 3
[7]
d = 4
[K]
e = 5
[first]
again = the earlier [first] is replaced
nested[] = x
nested[k] = y
INI;
var_dump(parse_ini_string($ini));
var_dump(parse_ini_string($ini, true));

// Numeric-looking keys: php's symtable rules, and the array form's own.
var_dump(parse_ini_string("1 = a\n01 = b\n-1 = c\n1.5 = d\n5[] = e\n05[] = f\n-3[] = g\n+4[] = h"));
var_dump(parse_ini_string("[s]\n[t]", true));
var_dump(parse_ini_string("[s]x\ny=1", true));
