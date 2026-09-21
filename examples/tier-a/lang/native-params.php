<?php
// php's parameter parsing for builtins (zend_parse_parameters): a scalar
// parameter takes what weak mode converts (int ← whole float, numeric
// string, bool; float ← int, numeric string; string ← any scalar; bool ←
// any scalar), null for a non-nullable scalar is the 8.1 deprecation and
// the scalar's zero, a fractional float or float-string for an int
// parameter loses precision with a deprecation, and anything else is the
// TypeError naming the declared type.
function t($f) { try { $r = $f(); var_dump($r); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
t(fn() => strlen(null)); t(fn() => strlen([])); t(fn() => strlen(12)); t(fn() => strlen(1.5)); t(fn() => strlen(true)); t(fn() => strlen(false));
t(fn() => str_repeat("a", "3")); t(fn() => str_repeat("a", "3.5")); t(fn() => str_repeat("a", 3.5)); t(fn() => str_repeat("a", 3.0)); t(fn() => str_repeat("a", "3x")); t(fn() => str_repeat("a", "x")); t(fn() => str_repeat("a", null)); t(fn() => str_repeat("a", true)); t(fn() => str_repeat("a", [])); t(fn() => str_repeat("a", 1e30));
t(fn() => abs("1.5")); t(fn() => abs("2")); t(fn() => abs(" 3 ")); t(fn() => abs("abc")); t(fn() => abs(null)); t(fn() => abs(true)); t(fn() => abs([]));
t(fn() => count(null)); t(fn() => count("x")); t(fn() => in_array(1, null)); t(fn() => in_array(1, "x")); t(fn() => array_keys("x"));
t(fn() => round("3.7")); t(fn() => round(true)); t(fn() => round("x")); t(fn() => str_pad("a", 5, 7)); t(fn() => str_pad("a", "5")); t(fn() => str_pad("a", 5, "-", "1"));
t(fn() => array_key_exists(1.5, [1 => 1])); t(fn() => array_key_exists(null, ["" => 1])); t(fn() => array_key_exists(true, [1 => 1]));
t(fn() => implode(1, [1, 2])); t(fn() => implode(",", "x")); t(fn() => implode(",", null));
t(fn() => substr("hello", "1", null)); t(fn() => substr("hello", 1.0, "2")); t(fn() => intdiv("7", 2.0)); t(fn() => intdiv(7.5, 2)); t(fn() => intdiv(9007199254740993.0, 1));
t(fn() => strtoupper(new stdClass)); t(fn() => strtoupper(new class { function __toString() { return "ts"; } }));
t(fn() => sqrt("16")); t(fn() => sqrt(null)); t(fn() => max("a", null)); t(fn() => str_contains("abc", 1)); t(fn() => str_contains(123, "2")); t(fn() => explode(1, "a1b"));
t(fn() => array_fill(0, "2", 1)); t(fn() => array_fill("0", 2, 1)); t(fn() => array_slice([1, 2, 3], "1")); t(fn() => array_slice([1, 2, 3], 1, null, "yes"));
t(fn() => range(1, 3, "1")); t(fn() => number_format("1234.5", "1")); t(fn() => number_format(1234.5, 1, null, null));
t(fn() => strpos("hello", "l", "1")); t(fn() => strpos("hello", "l", 1.0)); t(fn() => strpos("hello", "l", 1.5));
t(fn() => json_encode([1], "0")); t(fn() => json_decode("[1]", 1)); t(fn() => htmlspecialchars(5)); t(fn() => htmlspecialchars("a", "2"));
t(fn() => md5(1.5)); t(fn() => base64_encode(true)); t(fn() => dechex("255")); t(fn() => dechex("ff")); t(fn() => chr("65")); t(fn() => ord(65));
t(fn() => array_map(null, [1])); t(fn() => array_map('strtoupper', ["a"])); t(fn() => usort($x, "strcmp"));
function strict_probe() { include __DIR__ . '/native-params-strict.inc'; }
strict_probe();
// Native methods go through the same parsing.
t(fn() => (new ArrayObject([1, 2]))->offsetGet("1")); t(fn() => (new ArrayIterator([1]))->seek("0")); t(fn() => (new ArrayIterator([1]))->seek(null)); t(fn() => (new ArrayIterator([1]))->seek("x"));
t(fn() => (new SplFixedArray("2"))->getSize()); t(fn() => (new SplFixedArray(2.5))->getSize()); t(fn() => (new SplFixedArray([]))->getSize());
t(fn() => (new DateTime("2020-01-02"))->format(5)); t(fn() => (new DateTime("2020-01-02"))->format(null)); t(fn() => (new DateTime("2020-01-02"))->setDate("2021", 1.0, true)->format("Y-m-d"));
t(fn() => str_repeat("-", (new SplQueue)->count())); t(fn() => (new SplPriorityQueue)->setExtractFlags("3")); t(fn() => (new SplDoublyLinkedList)->add("0", "v"));
