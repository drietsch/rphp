<?php
// INI_SCANNER_RAW and INI_SCANNER_TYPED next to the normal mode.
define('C', 'constant');

$ini = <<<'INI'
[sec]
bool_on = on
bool_off = off
nil = null
int = 5
neg = -5
float = 1.5
negfloat = -1.5
big = 99999999999999999999
words = 5 apples
paren = (5)
op = 1 | 2
quoted_num = "5"
single = '5'
const = C
raw = a ; b
quoted = "a ; b"
tail = "q" ; comment
odd = "a" ; c "d"
spaced =    x y
empty =
INI;
foreach ([INI_SCANNER_NORMAL, INI_SCANNER_RAW, INI_SCANNER_TYPED] as $mode) {
    echo "--- mode $mode\n";
    var_dump(parse_ini_string($ini, true, $mode));
}

// Raw section names keep their spaces and quotes.
var_dump(parse_ini_string("[ a \"b\" ]\nk = v", true, INI_SCANNER_RAW));
var_dump(parse_ini_string("k = yes\nl = none", false, INI_SCANNER_TYPED));
