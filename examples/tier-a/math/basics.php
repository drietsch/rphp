<?php
// Differential snippet for the math extension additions. Every newly added
// function is exercised and its result printed so rphp's output can be diffed
// byte-for-byte against stock PHP 8.5. Floats are printed with `echo`, which
// uses the precision=14 string form both engines share; booleans use var_dump.

// pi: the M_PI constant.
echo pi(), "\n";

// pow: int**int stays int, negative/float exponents and float operands are float.
echo pow(2, 3), "\n";
echo pow(2, 10), "\n";
echo pow(2, -1), "\n";
echo pow(2.0, 3), "\n";
echo pow(2.5, 2), "\n";
echo pow(10, 20), "\n";
echo pow(0, 0), "\n";

// exp / expm1.
echo exp(1), "\n";
echo expm1(1), "\n";
echo expm1(0), "\n";

// log: natural, then explicit bases (10 and 2 use dedicated routines, base 1
// is NAN, an arbitrary base uses the change-of-base division).
echo log(exp(1)), "\n";
echo log(1000, 10), "\n";
echo log(8, 2), "\n";
echo log(27, 3), "\n";
echo log10(1000), "\n";
// Base 1 is NAN; checked via is_nan since echoing NAN would warn.
var_dump(is_nan(log(5, 1)));

// Trigonometry.
echo sin(1), "\n";
echo cos(1), "\n";
echo tan(1), "\n";
echo asin(0.5), "\n";
echo acos(0.5), "\n";
echo atan(1), "\n";
echo atan2(1, 1), "\n";

// Hyperbolic.
echo sinh(1), "\n";
echo cosh(1), "\n";
echo tanh(1), "\n";
echo asinh(1), "\n";
echo acosh(2), "\n";
echo atanh(0.5), "\n";

// Angle conversion.
echo deg2rad(180), "\n";
echo rad2deg(pi()), "\n";

// hypot / fmod / fdiv (fdiv never errors on divide-by-zero).
echo hypot(3, 4), "\n";
echo fmod(10, 3), "\n";
echo fmod(-10, 3), "\n";
echo fdiv(10, 3), "\n";
echo fdiv(1, 0), "\n";
echo fdiv(-1, 0), "\n";
// fdiv(0, 0) is NAN; checked via is_nan below (echoing NAN would warn).

// Float predicates.
var_dump(is_nan(fdiv(0, 0)));
var_dump(is_nan(1.0));
var_dump(is_finite(1.0));
var_dump(is_finite(fdiv(1, 0)));
var_dump(is_infinite(fdiv(1, 0)));
var_dump(is_infinite(1.0));

// Base conversion to string (unsigned 64-bit pattern; negatives wrap).
echo dechex(255), "\n";
echo dechex(0), "\n";
echo dechex(-1), "\n";
echo decbin(5), "\n";
echo decbin(-1), "\n";
echo decoct(8), "\n";
echo decoct(-1), "\n";

// Base conversion from string (Int, promoting to Float past the i64 range).
echo hexdec("ff"), "\n";
echo hexdec("7fffffffffffffff"), "\n";
echo hexdec("ffffffffffffffff"), "\n";
echo bindec("101"), "\n";
echo bindec("1111111111111111111111111111111111111111111111111111111111111111"), "\n";
echo octdec("777"), "\n";
echo octdec("17777777777777777777777"), "\n";
