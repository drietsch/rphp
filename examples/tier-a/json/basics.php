<?php
// Tier-A differential: json_encode / json_decode. Output is diffed byte-for-byte
// against stock PHP 8.5. Flags are passed as plain integers:
//   64 = JSON_UNESCAPED_SLASHES, 128 = JSON_PRETTY_PRINT, 256 = JSON_UNESCAPED_UNICODE.

// --- json_encode: scalars ---
echo json_encode(null) . "\n";
echo json_encode(true) . " " . json_encode(false) . "\n";
echo json_encode(42) . " " . json_encode(-7) . " " . json_encode(0) . "\n";

// --- json_encode: floats (shortest round-trip; integral floats drop the ".0") ---
echo json_encode(1.5) . " " . json_encode(1.0) . " " . json_encode(100.0) . "\n";
echo json_encode(0.1) . " " . json_encode(3.14159) . "\n";
echo json_encode(1234567890.5) . "\n";
echo json_encode(0.0001) . " " . json_encode(0.00001) . "\n";

// --- json_encode: strings & escaping ---
echo json_encode("hello world") . "\n";
echo json_encode("quote\" back\\ slash/here") . "\n";
echo json_encode("tab\tnewline\n") . "\n";
echo json_encode("café 日本 emoji") . "\n";

// --- json_encode: arrays (list) vs objects (assoc / gapped / unordered) ---
echo json_encode([1, 2, 3]) . "\n";
echo json_encode(["a" => 1, "b" => 2, "c" => 3]) . "\n";
echo json_encode(array(1, "two" => 2, 3)) . "\n";
echo json_encode([true, false, null, "s", 1.5]) . "\n";
echo json_encode(["outer" => ["inner" => [1, 2]]]) . "\n";
echo json_encode([]) . "\n";

// --- json_encode: flags ---
echo json_encode("a/b café", 64) . "\n";
echo json_encode("a/b café", 256) . "\n";
echo json_encode("a/b café", 320) . "\n";
echo json_encode([1, 2, 3], 128) . "\n";
echo json_encode(["k" => 1, "nested" => [2, 3]], 128) . "\n";
echo json_encode(["empty" => []], 128) . "\n";

// --- json_decode: scalars ---
var_dump(json_decode("null"));
var_dump(json_decode("true"));
var_dump(json_decode("false"));
var_dump(json_decode("42"));
var_dump(json_decode("-7"));
var_dump(json_decode("3.14"));
var_dump(json_decode("1.5"));
var_dump(json_decode("\"hello\""));

// --- json_decode: escaped strings ---
var_dump(json_decode("\"line1\\nline2\""));
var_dump(json_decode("\"slash\\/done\""));
var_dump(json_decode("\"\\u0041\\u0042\""));
var_dump(json_decode("\"\\u00e9\""));

// --- json_decode: arrays (lists) ---
var_dump(json_decode("[1, 2, 3]"));
var_dump(json_decode("  [ true , null , \"x\" ] "));
var_dump(json_decode("[[1, 2], [3, 4]]"));

// --- json_decode: objects with associative=true (both produce arrays) ---
var_dump(json_decode("{\"a\": 1, \"b\": 2}", true));
var_dump(json_decode("{\"n\": {\"m\": [1, 2]}}", true));

// --- json_decode: object default decode, checked via re-encode round-trip ---
echo json_encode(json_decode("{\"a\": 1, \"b\": [2, 3]}")) . "\n";

// --- json_decode: foreach over a decoded object (matches stdClass iteration) ---
$obj = json_decode("{\"one\": 1, \"two\": 2, \"three\": 3}");
foreach ($obj as $k => $v) {
    echo $k . "=" . $v . "\n";
}

// --- json_decode: errors return null ---
var_dump(json_decode(""));
var_dump(json_decode("not json"));
var_dump(json_decode("[1, 2,]"));
var_dump(json_decode("{\"a\": 1"));
var_dump(json_decode("01"));

// --- round trip ---
echo json_encode(json_decode("[1, \"two\", 3.5, true, null]")) . "\n";
