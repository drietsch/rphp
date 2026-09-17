<?php
// Tier-A differential: base conversion — base_convert across bases with
// whitespace / prefix stripping, the invalid-character deprecation, the
// float path past 64 bits (php's fmod digit peeling), the ValueErrors for
// bad bases, and the dec*/​*dec pairs.

var_dump(base_convert("ff", 16, 2), base_convert("zz", 36, 10), base_convert("1010", 2, 16), base_convert("0x1A", 16, 10), base_convert("", 10, 2));
var_dump(base_convert("9223372036854775808", 10, 16), base_convert("ffffffffffffffffff", 16, 10), base_convert("hello", 36, 2), base_convert("12", 10, 10), base_convert(255, 10, 16));
var_dump(base_convert(" ff ", 16, 10), base_convert("0xff", 16, 2), base_convert("0b11", 2, 10), base_convert("0o7", 8, 10), base_convert("0B11", 2, 10));
var_dump(base_convert("ffffffffffffffff", 16, 10), base_convert("18446744073709551616", 10, 16), base_convert("1000000000000000000000000000000", 10, 36), base_convert("zzzzzzzzzzzzzzzzzzzz", 36, 2));
var_dump(base_convert("-ff", 16, 10));
var_dump(base_convert("1g", 16, 10));
var_dump(base_convert("1e3", 10, 10));
var_dump(base_convert(1.5, 10, 2));
var_dump(hexdec("ff"), hexdec("0x1A"), hexdec(" 1a "), hexdec("0X1a"), hexdec("7fffffffffffffff"), hexdec("ffffffffffffffff"));
var_dump(bindec("101"), bindec("0b11"), bindec("1111111111111111111111111111111111111111111111111111111111111111"));
var_dump(octdec("777"), octdec("0o17"), octdec("017"), octdec("17777777777777777777777"));
var_dump(hexdec("x"));
var_dump(hexdec("1g"));
var_dump(bindec("12"));
var_dump(octdec("8"));
var_dump(dechex(255), dechex(0), dechex(-1), decbin(5), decbin(-1), decoct(8), decoct(-1), dechex(constant('PHP_INT_MAX')));
var_dump(base_convert("1", 10, 37));
