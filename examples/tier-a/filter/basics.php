<?php
// The three-way outcome (value / false / null) is what callers depend on.
foreach ([
    ['42', FILTER_VALIDATE_INT], [' 42 ', FILTER_VALIDATE_INT], ['4.5', FILTER_VALIDATE_INT],
    ['abc', FILTER_VALIDATE_INT], ['', FILTER_VALIDATE_INT], ['-7', FILTER_VALIDATE_INT],
    ['1.5', FILTER_VALIDATE_FLOAT], ['abc', FILTER_VALIDATE_FLOAT],
    ['yes', FILTER_VALIDATE_BOOL], ['off', FILTER_VALIDATE_BOOL], ['maybe', FILTER_VALIDATE_BOOL],
    ['TRUE', FILTER_VALIDATE_BOOL], ['0', FILTER_VALIDATE_BOOL],
    ['a@b.com', FILTER_VALIDATE_EMAIL], ['nope', FILTER_VALIDATE_EMAIL], ['a..b@c.com', FILTER_VALIDATE_EMAIL],
    ['http://x.com/p', FILTER_VALIDATE_URL], ['notaurl', FILTER_VALIDATE_URL],
    ['1.2.3.4', FILTER_VALIDATE_IP], ['999.1.1.1', FILTER_VALIDATE_IP], ['::1', FILTER_VALIDATE_IP],
    ['aa:bb:cc:dd:ee:ff', FILTER_VALIDATE_MAC], ['zz:bb:cc:dd:ee:ff', FILTER_VALIDATE_MAC],
] as [$v, $f]) {
    printf("%-20s %-3d => %s\n", var_export($v, true), $f, var_export(filter_var($v, $f), true));
}

// Ranges, defaults and NULL_ON_FAILURE.
var_dump(filter_var('5', FILTER_VALIDATE_INT, ['options' => ['min_range' => 1, 'max_range' => 4]]));
var_dump(filter_var('3', FILTER_VALIDATE_INT, ['options' => ['min_range' => 1, 'max_range' => 4]]));
var_dump(filter_var('maybe', FILTER_VALIDATE_BOOL, FILTER_NULL_ON_FAILURE));
var_dump(filter_var('x', FILTER_VALIDATE_INT, ['options' => ['default' => 7]]));
var_dump(filter_var('0x1A', FILTER_VALIDATE_INT, FILTER_FLAG_ALLOW_HEX));

// Sanitizers.
var_dump(filter_var('a1b-2c+3', FILTER_SANITIZE_NUMBER_INT));
var_dump(filter_var("o'brien", FILTER_SANITIZE_ADD_SLASHES));
var_dump(filter_id('int'), filter_id('nope'));

// FILTER_CALLBACK maps the value (every leaf of an array) through the
// `options` callable; the array flags decide what an array does.
var_dump(filter_var("ab", FILTER_CALLBACK, ["options" => "strtoupper"]), filter_var(["a", "b"], FILTER_CALLBACK, ["options" => fn($v) => $v . "!"]), filter_var(5, FILTER_CALLBACK, ["options" => fn($v) => $v]));
try { filter_var("x", FILTER_CALLBACK, ["options" => "nope"]); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(filter_var(["1", "x"], FILTER_VALIDATE_INT), filter_var(["1", "x"], FILTER_VALIDATE_INT, FILTER_REQUIRE_ARRAY), filter_var("1", FILTER_VALIDATE_INT, FILTER_FORCE_ARRAY), filter_var(["a" => ["1"]], FILTER_VALIDATE_INT, FILTER_REQUIRE_ARRAY), filter_var("1", FILTER_VALIDATE_INT, FILTER_REQUIRE_ARRAY));
var_dump(FILTER_CALLBACK, FILTER_THROW_ON_FAILURE, FILTER_FLAG_EMAIL_UNICODE, @FILTER_SANITIZE_STRIPPED);
echo FILTER_SANITIZE_STRING, "\n";
