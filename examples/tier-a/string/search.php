<?php
// string.c search & replace: str_replace/str_ireplace with &$count and array
// forms, substr_replace array forms, strrchr(before_needle), explode/implode
// edge cases, hex2bin warnings, empty-needle positions, sscanf.

// --- str_replace / str_ireplace ---
var_dump(str_replace("a", "b", "banana", $c), $c);
var_dump(str_replace(["a", "b"], ["1", "2"], "abcab", $c), $c);
var_dump(str_replace(["a", "b"], "x", "abcab", $c), $c);
var_dump(str_replace(["a", "b"], ["1"], "abcab", $c), $c);
var_dump(str_replace("a", "b", ["aa", "ba", 5, null, ["a"]], $c), $c);   // warning: array to string
var_dump(str_replace("", "x", "abc", $c), $c, str_replace(["", "b"], "x", "abc"), str_replace(["a", "1"], ["1", "2"], "a"));
var_dump(str_replace(1, 2, 3141), str_replace("a", "b", 1), str_replace("a", "b", true), str_replace("a", "b", ["k" => "va"]));
var_dump(str_ireplace("A", "x", "aAbB", $c), $c, str_ireplace(["a", "B"], ["1", "2"], "aAbB"), str_ireplace("", "x", "abc"), str_ireplace("É", "e", "café É"));

// --- substr_replace ---
var_dump(substr_replace("Hello", "XX", 1, 2), substr_replace("Hello", "XX", -2, 1), substr_replace("Hello", "XX", 2, -1), substr_replace("abc", "X", 1, 0), substr_replace("abc", "X", 10), substr_replace("abc", "X", -10, 1), substr_replace("abc", ["X"], 1), substr_replace("abc", [], 1));
var_dump(substr_replace(["abc", "def"], "X", 1, 1), substr_replace(["abc", "def"], ["X", "Y"], 1, 1), substr_replace(["abc", "def"], ["X"], [0, 1], [1, 2]), substr_replace(["abc", "def"], "X", [1], 1), substr_replace(["abc", "def"], "X", 1, [1]));
var_dump(substr_replace(["a" => "abc", "b" => "def"], ["X", "Y"], 0, 0), substr_replace(["abc", 5], "X", 0, 1), substr_replace(["abc", "def"], ["k" => "X", "j" => "Y"], ["q" => 1], [1]), substr_replace(["abc", "def", "ghi"], "X", [0, 5, -1], [1]));

// --- strrchr / strstr family ---
var_dump(strrchr("a/b/c", "/", true), strrchr("a/b/c", "/b", true), strrchr("abc", "", true), strrchr("abc", ""), strrchr("a/b/c", "/"));
var_dump(strpos("abc", "", 3), strpos("abc", "", 1), strrpos("abc", "", 3), strrpos("abc", "", -1), stripos("abc", "", 2), strpos("abc", "a", 3), strpos("", "", 0), strpos("", "a"), strpos("abc", "c", -1), stripos("ABC", "b", -2));

// --- explode / implode ---
var_dump(explode(",", "a,b,c", 0), explode(",", "a,b,c", 1), explode(",", "a,b,c", -1), explode(",", "a,b,c", -3), explode(",", "a,b,c", -4), explode(",", "", -1), explode(",", ""), explode(",", "abc", 2), explode(",,", "a,,b,,,c"));
var_dump(implode(",", [1, 2.5, true, false, null, "x"]), implode([1, 2]), implode(", ", []), implode(["a", [1]]), implode(5, [1, 2]));   // warning: array to string
var_dump(implode("-", ["k" => "a", 7 => "b"]), implode(", ", ["x"]), implode(",", [1.0, 0.1 + 0.2, -0.0]));

// --- hex2bin warnings ---
var_dump(hex2bin("abc"));   // warning: odd length
var_dump(hex2bin("zz"));    // warning: not hexadecimal
var_dump(hex2bin(""), bin2hex(hex2bin("00ff7F")));

// --- sscanf ---
var_dump(sscanf("age: 25 name: Bob", "age: %d name: %s"), sscanf("12 apples", "%d %s %s"), sscanf("hello", "%d"), sscanf("", "%d"), sscanf("", ""), sscanf("abc", ""));
$cases = [["  42", "%d"], ["42abc", "%d%s"], ["-42", "%d"], ["+42", "%d"], ["0x1A", "%x"], ["1A", "%x"], ["0x1A", "%i"], ["012", "%i"], ["012", "%d"], ["17", "%o"], ["3.14e2xyz", "%f"], ["3.14", "%e"], [".5", "%f"], ["-.5e-1", "%f"], ["abc", "%c"], ["abc", "%3c"], ["abcdef", "%2s"], ["abc def", "%s"], ["abc", "%[a-b]"], ["abc", "%[^c]"], ["a]c", "%[]a]"], ["a-c", "%[a-]"], ["abc", "%*s%d"], ["12 34", "%*d %d"], ["12", "%d%n"], ["12 34", "%d %d %n"], ["12%34", "%d%%%d"], ["12 34", "%d%d"], ["12,34", "%d,%d"], ["12,34", "%d %d"], ["  12", "%c"], ["12", "%5d"], ["123456", "%2d%2d"], ["99999999999999999999", "%d"], ["99999999999999999999", "%u"], ["-5", "%u"], ["abc", "%s%s"], ["a", "%c%c"], ["2020-01-05", "%4d-%2d-%2d"], ["1e5", "%d"], ["1e5", "%f"], ["inf", "%f"], ["12", "%ld"], ["12", "%hd"], ["12", "%Lf"], ["12", "%X"], ["ab", "%[b-a]"], ["", "%s"], ["abc", "x%s"], ["abc", "a%s"], ["abc", "abc"], ["abc", "abcd"], ["abc", "abc%d"], ["a  b", "a b"], ["ab", "a b"], ["12.5", "%d.%d"], ["0x", "%x"], ["0x", "%i"], ["-0x1a", "%i"], ["0b101", "%i"], ["1.", "%f"], [".", "%f"], ["-", "%d"], ["1e", "%f"], ["1e+", "%f"], ["1.5.5", "%f"], ["12", '%1$d'], ["12 34", '%2$d %1$d'], ["12 x", "%d%c"], ["12 x", "%d %c"], ["hello world", "%[^\n]"], ["%", "%%"], ["%5", "%%%d"], ["00012", "%d"], ["+", "%d"], ["+-1", "%d"], ["ffffffffffffffff", "%x"], ["-ff", "%x"], ["9223372036854775808", "%d"], ["-9223372036854775809", "%d"], ["abc", "%n"], ["  a", "%n%s"], ["12", "%*n"], ["abc", "%s%n"], ["abcABC", "%[a-cA-C]"], ["a-", "%[-a]"], ["-a", "%[-]"], ["]", "%[]]"], ["a^", "%[a^]"], ["1 2 3", "%d %*d %d"], ["  x", "%2c"], ["x y", "%3c"], ["", "%c"], ["", "%[a]"], ["x", "%[a]"], ["x", "%*[a]%s"], ["0777", "%i"], ["08", "%i"], ["0x", "%x%s"], ["0xg", "%i%s"], ["1.5e3", "%e"], ["  -3", "%f"], ["1e-5", "%f"], [" 1", "%1d"], ["12345", "%3d%3d"], ["1", "%d%d%d"], ["\t\n 1", "%d"], ["12", "%s%d"], ["a b", "%s\t%s"], ["a\0b", "%s"], ["a\0b", "%c%c%c"]];
foreach ($cases as $cs) {
    echo json_encode($cs[0]), " ", json_encode($cs[1]), " => ", json_encode(sscanf($cs[0], $cs[1])), "\n";
}
$r = sscanf("x", "%s", $a);
var_dump($r, $a);
$r = sscanf("x", "%s %s", $p, $q);
var_dump($r, $p, $q);
$r = sscanf("", "%s", $z);
var_dump($r, $z);
$r = sscanf("5", "%d %d", $m, $n);
var_dump($r, $m, $n);
$r = sscanf("5", "%d%c", $m2, $n2);
var_dump($r, $m2, $n2);
$r = sscanf("12", "%d%n", $m3, $n3);
var_dump($r, $m3, $n3);
$r = sscanf("12 34", '%2$d %1$d', $m4, $n4);
var_dump($r, $m4, $n4);
