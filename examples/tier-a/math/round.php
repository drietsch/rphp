<?php
// Tier-A differential: round() — php's pre-rounding (the decision is taken by
// comparing the original value against the candidate edge in its own scale,
// so 0.285 → 0.29 and 2.675 → 2.68), negative precision, the four
// PHP_ROUND_HALF_* modes plus the 8.4 RoundingMode values as ints (5–8),
// the int fast path and the overflow / huge-precision guards.

var_dump(round(2.5), round(-2.5), round(1.955, 2), round(5.045, 2), round(5.055, 2), round(0.285, 2), round(2.675, 2), round(1.005, 2));
var_dump(round(1234.5678, -2), round(1234.5678, -4), round(1.45, 1), round(1.55, 1), round(-1.45, 1), round(0.5), round(-0.5), round(-0.4));
var_dump(round(2.5, 0, 1), round(2.5, 0, 2), round(2.5, 0, 3), round(3.5, 0, 3), round(2.5, 0, 4), round(3.5, 0, 4), round(-2.5, 0, 2));
var_dump(round(-2.5, 0, 3), round(-3.5, 0, 3), round(-2.5, 0, 4), round(-0.5, 0, 2), round(-0.5, 0, 3), round(0.5, 0, 4));
var_dump(round(0.15, 1, 3), round(0.25, 1, 3), round(0.35, 1, 3), round(1.45, 1, 2), round(-1.45, 1, 2), round(1.455, 2, 2));
var_dump(round(5.5, 0, 5), round(-5.5, 0, 5), round(5.5, 0, 6), round(-5.5, 0, 6), round(5.5, 0, 7), round(-5.5, 0, 7), round(5.5, 0, 8), round(-5.5, 0, 8));
var_dump(round(5.4, 0, 6), round(5.6, 0, 5), round(1.234, 2, 8), round(1.231, 2, 8), round(-1.231, 2, 7), round(1.23, 1, 7), round(0.5, 0, 7), round(-0.5, 0, 8), round(1.1, 0, 5));
var_dump(round(15, -1, 5), round(15, -1, 6), round(15, -1, 7), round(15, -1, 8), round(15, -1, 2), round(15, -1, 4), round(25, -1, 4));
var_dump(round(3, -1), round(15, -1), round(25, -1), round(-15, -1), round(5, -1, 3), round(1234, -2), round(12, -2), round(50, -2), round(-5, -1));
var_dump(round(5), round(5, 2), round(-5), round("3.7"), round("1e3"), round(true), round(constant('PHP_INT_MAX'), -1), round(constant('PHP_INT_MIN'), -1));
var_dump(round(9223372036854775807, -18), round(9223372036854775807, -19), round(1, -20), round(1, -19));
var_dump(round(1e20), round(1.0e15 + 0.5), round(constant('PHP_INT_MAX')), round(1.0E-10, 5), round(123456.789, 3), round(0.1 + 0.2, 15));
var_dump(round(1.23456789012345678, 17), round(1.5, 400), round(1.5, -400), round(3.14159, 3), round(0.49999999999999994), round(4503599627370497.0));
var_dump(round(1.4999999999999998), round(1234567.891, -3), round(-1234567.891, -3), round(9.5, -1), round(1000000000000000.5), round(2.5e-7, 7));
var_dump(round(1.1, 30), round(1e300, -300), round(1e-300, 300), round(1.0, 20), round(12345678901234567890.0, -5));
var_dump(round(1e300, 10), round(1e300, 22), round(1e300, 23), round(1.7976931348623157e308, 1), round(-1e300, 10), round(1e-320, 3));
var_dump(round(1e22, -22), round(5e21, -22), round(1.5e22, -22), round(4.9999999999999995e-7, 6), round(0.0, 2), round(-0.0));
var_dump(round(fdiv(0, 0)), round(fdiv(1, 0)), round(fdiv(-1, 0)));
var_dump(round(1.5, 0, 9));
