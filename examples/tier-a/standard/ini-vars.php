<?php
// ${name} expansion from the environment, with the 8.3 `:-` fallback.
putenv('RPHP_INI_VAR=from env');
putenv('RPHP_INI_NUM=7');

$ini = <<<'INI'
plain = ${RPHP_INI_VAR}
missing = ${RPHP_INI_NOPE}
around = x${RPHP_INI_VAR}y
quoted = "in quotes: ${RPHP_INI_VAR}"
fallback = ${RPHP_INI_NOPE:-default value}
unused_fallback = ${RPHP_INI_VAR:-default}
const_fallback = ${RPHP_INI_NOPE:-PHP_INT_SIZE}
empty_fallback = ${RPHP_INI_NOPE:-}
word_fallback = ${RPHP_INI_NOPE:-true} ${RPHP_INI_NOPE:-null} ${RPHP_INI_NOPE:-FALSE}
nested = ${RPHP_INI_NOPE:-${RPHP_INI_VAR}}
spaced = ${ RPHP_INI_VAR }
num = ${RPHP_INI_NUM}
arr[${RPHP_INI_NUM}] = offset from env
[sec_${RPHP_INI_NUM}]
k = v
INI;
var_dump(parse_ini_string($ini, true));
var_dump(parse_ini_string("n = \${RPHP_INI_NUM}\nm = 5\${RPHP_INI_NUM}", false, INI_SCANNER_TYPED));
var_dump(parse_ini_string("r = \${RPHP_INI_VAR}", false, INI_SCANNER_RAW));

// A one-letter name before `:-` is php's scanner quirk: a syntax error.
var_dump(parse_ini_string("q = \${a:-b}"));
