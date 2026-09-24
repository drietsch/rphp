<?php
// parse_ini_string: plain entries, the value words, numbers, quoting,
// comments, constants and the bit operators.
define('APP_NAME', 'demo');
define('APP_LEVEL', 3);

$ini = <<<'INI'
; a comment line
name = hello world
quoted = "a \"quoted\" value ; not a comment"
single = 'it''s raw'
path = "C:\dir\"
esc = "back\\slash \$dollar \n stays"
empty =
bare
on_ = on
off_ = off
yes_ = yes
no_ = no
true_ = TRUE
false_ = False
none_ = none
null_ = null
one = one
int = 42
neg = -7
float = 1.5
dotted = 1.2.3
exp = 1e5
hex = 0x1A
const = APP_NAME
const_int = APP_LEVEL
const_in_quotes = "APP_NAME"
undefined = NOT_A_CONSTANT
mixed = APP_NAME and APP_LEVEL
ops = E_ALL & ~E_DEPRECATED
or = 1 | 2 | 4
xor = 6 ^ 3
not = !0
paren = (1 | 2) & 3
concat = "a" b "c"
trailing = value ; comment
hash = # not a comment
  indented   =   spaced out
dup = first
dup = second
10 = numeric key
010 = octal-looking key
INI;
var_dump(parse_ini_string($ini));

// A NUL ends the string, as in php's C string.
var_dump(parse_ini_string("a=1\nb=2\0c=3"));
var_dump(parse_ini_string(""));
var_dump(parse_ini_string("; only a comment"));
