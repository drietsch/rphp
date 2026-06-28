<?php
// Differential coverage for the ctype_* predicates. ASCII byte inputs only:
// classification is locale-independent there, and we avoid the integer-argument
// path (PHP emits a deprecation notice for it, which rphp does not reproduce).
// Each function gets a matching input, a non-matching input, and the empty
// string (which every predicate reports as false).

echo "ctype_alnum\n";
var_dump(ctype_alnum("abc123"));
var_dump(ctype_alnum("abc 123"));
var_dump(ctype_alnum(""));

echo "ctype_alpha\n";
var_dump(ctype_alpha("Hello"));
var_dump(ctype_alpha("Hello2"));
var_dump(ctype_alpha(""));

echo "ctype_cntrl\n";
var_dump(ctype_cntrl("\t\n\r"));
var_dump(ctype_cntrl("abc"));
var_dump(ctype_cntrl(""));

echo "ctype_digit\n";
var_dump(ctype_digit("0123456789"));
var_dump(ctype_digit("12.3"));
var_dump(ctype_digit(""));

echo "ctype_graph\n";
var_dump(ctype_graph("abc!#"));
var_dump(ctype_graph("abc def"));
var_dump(ctype_graph(""));

echo "ctype_lower\n";
var_dump(ctype_lower("abcxyz"));
var_dump(ctype_lower("abcXyz"));
var_dump(ctype_lower(""));

echo "ctype_print\n";
var_dump(ctype_print("abc 123!"));
var_dump(ctype_print("abc\tdef"));
var_dump(ctype_print(""));

echo "ctype_punct\n";
var_dump(ctype_punct("!@#$%"));
var_dump(ctype_punct("abc!"));
var_dump(ctype_punct(""));

echo "ctype_space\n";
var_dump(ctype_space(" \t\n\r"));
var_dump(ctype_space("a b"));
var_dump(ctype_space(""));

echo "ctype_upper\n";
var_dump(ctype_upper("ABCXYZ"));
var_dump(ctype_upper("ABCxYZ"));
var_dump(ctype_upper(""));

echo "ctype_xdigit\n";
var_dump(ctype_xdigit("deadBEEF09"));
var_dump(ctype_xdigit("xyz"));
var_dump(ctype_xdigit(""));
