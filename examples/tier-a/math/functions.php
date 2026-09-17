<?php
// Tier-A differential: the math.c completion — log1p, fpow, pow's zero-base
// deprecation, the invalid-character deprecation of the base converters,
// abs/floor/ceil/sqrt on ints, strings and signed zeros, and the values
// printed with var_dump's shortest-round-trip float layout.

var_dump(log1p(1), log1p(0), log1p(-1), is_nan(log1p(-2)));
var_dump(fpow(2, 3), fpow(2, -1), fpow(2, 0.5), fpow(0, 0), is_nan(fpow(-8, 1 / 3)), fpow(0, -1));
var_dump(pow(2, 3), pow(2, -1), pow(2.0, 3), pow("2", "3"), pow(2, 63), pow(-2, 63), pow(2, 64), pow(0, 0));
var_dump(pow(0, -1));
var_dump(pow(0.0, -1.5));
var_dump(abs("-5"), abs(-0.0), abs(-5.5), abs(constant('PHP_INT_MIN')), abs(-7));
var_dump(floor("5.7"), ceil(5), ceil(-0.5), floor(-0.0), floor(-5.5), ceil("4.1"));
var_dump(sqrt(4), sqrt("9"), is_nan(sqrt(-1)), intdiv(-7, 2), is_nan(fmod(5.5, 0)));
var_dump(exp(1), log(exp(2)), log(1000, 10), log(8, 2), log10(100), log(0), is_nan(log(-1)));
var_dump(hypot(3, 4), deg2rad(90), rad2deg(1), atan2(1, 1), sin(0), cos(0));
var_dump(is_finite(1e308 * 10), is_infinite(-1e308 * 10), is_nan(fdiv(0, 0)));
var_dump(max(1, 2.5, "3"), min([4, "2", 3.5]), max("apple", "banana"), min(-1, -1.5));
var_dump(pi());
var_dump(constant('M_PI'), constant('M_E'), constant('M_SQRT2'), constant('PHP_ROUND_HALF_UP'), constant('PHP_ROUND_HALF_ODD'));
