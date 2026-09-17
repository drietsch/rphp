<?php
// formatted_print.c: the printf family — every conversion, flag, width and
// precision form, positional and `*` arguments, special floats, notices.

// --- conversions ---
var_dump(sprintf("%d|%u|%x|%X|%o|%b|%c|%s|%%", -42, -1, 255, 255, 8, 5, 65, "str"));
var_dump(sprintf("%f|%F|%e|%E|%g|%G|%h|%H", 3.14159, 3.14159, 12345.678, 12345.678, 0.00001234, 123456789.0, 1234.5678, 0.00001234));
var_dump(sprintf("%s|%s|%s|%s|%s", 1.0, true, false, null, 1e15));
var_dump(sprintf("%d|%d|%d|%d|%d", "12abc", 3.99, "1e3", "abc", null));
var_dump(sprintf("%u|%x|%b|%o|%c", -5, -1, -1, -1, 256 + 65));

// --- padding, alignment, sign ---
var_dump(sprintf("%5d|%-5d|%05d|%+d|%+d|%+5d|%+5d|%05d|%-05d|", 42, 42, 42, 42, -42, 5, -5, -5, -42));
var_dump(sprintf("%10s|%-10s|%'*10s|%.3s|%5.2s|%05s|%-05s|", "hi", "hi", "hi", "hello", "abcdef", "ab", "ab"));
var_dump(sprintf("%'x8d|%-'x8d|%'#5d|%'#+5d|%+'#5d|%-+5d|%3d|%.3d|%5.3d|", 42, 42, -5, 5, 5, 5, 12345, 5, 5));
var_dump(sprintf("%8.2f|%08.2f|%-08.2f|%'05.1f|%010.2e|%+010.2f|%-05x|%-05u|%-08e|%-05b|%-05o|%-06g|%5c|%-'x6c|", 3.14159, -3.14, -3.14, 3.14159, -3.5, 3.5, 255, 5, 1.5, 5, 8, 1.5, 65, 65));
var_dump(sprintf("% d|% 5d|%5s|%-5s|", 42, 42, "abcdefg", "ab"));
// php quirks: a `%` conversion is a literal that still consumes a slot; a
// precision empties %o/%x/%b; one `l` modifier is ignored.
var_dump(sprintf("% %%d", 1234, -5678), sprintf("%10.4o|%-10.4o|%04o|%04.4o|%.2x|%5.1b|", 0123456, 01234567, -01234567, 01234567, 255, 5), sprintf("%ld|%lu|%lx", 7, 8, 255));

// --- precision and rounding ---
var_dump(sprintf("%.0f|%.0f|%.0f|%.0f|%.1f|%.2f|%.2f|%.1f|%.2f", 2.5, 3.5, 0.5, -0.5, 0.05, 1.005, 1.045, -0.04, -0.001));
var_dump(sprintf("%f|%.15f|%.10f|%.3f|%.1f|%.2f", 1e20, 0.1, 1e-7, 1234567.8915, 1e300, 123456789012345678.0));
var_dump(sprintf("%e|%.14e|%.0e|%.2e|%.3e|%e|%.1e|%E", 0, 1.0, 1234.5, 12345.678, 999999.9, 1e300, 9.96, -1.5));
var_dump(sprintf("%g|%g|%g|%g|%g|%g|%g|%.2g|%.10g|%.0g|%.6g|%.6g", 0, 100000.0, 1000000.0, 0.0001, 0.00001, 123456.0, 1234567.0, 0.0001234, 123456789012.0, 1234.5, 123456.5, 1234565.0));
var_dump(sprintf("%.4g|%.4g|%.3g|%.3g|%.4g|%.4g|%.4g|%.4g|%.4g|%.1g|%.1g|%.1g|%.2g|%.3g|%.3g", 71905, 71915, 99950, 99850, 719050, 7190.5, 0.71905, 0.0000071905, 719005, 0.15, 0.25, 15, 2.5, 1000, 999.5));
var_dump(sprintf("%.3h|%.0h|%.60g", 1.0 / 3, 1234.5, 0.1));           // notice: precision truncated to 53
var_dump(sprintf("%.60f", 1.1));                                       // notice
var_dump(sprintf("%.60e", 1.1));                                       // notice

// --- special floats ---
$nan = constant('NAN');
$inf = constant('INF');
var_dump(sprintf("%f|%F|%e|%g|%d|%s|%5.1f|%05.1f|%+f|%+f|%+.1f", $nan, $inf, -$inf, $nan, -0.0, -0.0, $inf, $inf, $inf, $nan, -$inf));

// --- positional and star arguments ---
var_dump(sprintf("%2\$s %1\$s", "a", "b"), sprintf("%1\$s %s %s", "a", "b", "c"), sprintf("%s %1\$s", "a"), sprintf("%1\$'x5d", 5), sprintf("%1\$-5d|", 5));
var_dump(sprintf("%*d|%-*d|%.*f", 5, 42, 4, 7, 2, 3.14159), sprintf("%2\$*1\$d|", 4, 1), sprintf("%*1\$d", 4, 1), sprintf("%.*g|%.*g|%.*G|%.*g|%.*g|%.*g|%.*g|%.*h", -1, 0.1, -1, 1 / 3, -1, 1e20, -1, 123456789012345678.0, -1, 1e-7, -1, 0.00001, -1, 1e16, -1, 1e17));

// --- vsprintf / vprintf / printf ---
var_dump(vsprintf("%s-%d", ["a", 5]), vsprintf("%2\$s-%1\$s", ["x", "y"]), vsprintf("%d %d", [1, "x" => 2]));
$n = printf("n=%d %s\n", 42, "x");
echo "printf returned ", $n, "\n";
$n = vprintf("%05.1f|%s\n", [3.14159, "y"]);
echo "vprintf returned ", $n, "\n";
