<?php
// array.c: range() (8.3 rules), keys/values/search, map/filter modes, count
// modes, sum/product diagnostics, flip/count_values, pad/chunk/column/
// combine/fill, slice/splice, merge/replace (recursive), find/any/all,
// change_key_case, key_exists.

// --- range ---
$cases = [["a", "e", 2], ["e", "a", 2], ["a", "e", 2.0], ["A", "z", 10], [1, 10, 4], [5, 1, 2], [0, 1, 0.25], ["1", "3"], ["1", "10"], ["a", "c", 1], [1.0, 3.0], [1, 3.0], [1, 1], [1, 1, 5], [5, 5, -1], [3, 1, -1], ["1.5", "3"], [0, 1, 0.3], [1, 3, 1.5], [0.1, 0.3, 0.1], [3, 1, 0.5], [1, 3, 2.0], ["1", "5", "2"], ["a", "z", "3"], [false, true], [true, 3], ["0", "2"], [" 1", "2"], ["1e1", "8"], [-1, -3], ["00", "2"], ["1", "a"], ["a", "1"], ["1.0", "3"], ["-1", "1"], ["1", "1"], [1, 5, 2.5], [1, 5, "4"], [1, 5, "4.0"], [0, 3, 0.7], [2.5, 1], ["2", "1", 0.5], [1, 3, true], [0, 1, 0.1], [1, 4, 0.99999999999]];
foreach ($cases as $c) {
    if (count($c) == 2) {
        echo json_encode(range($c[0], $c[1])), "\n";
    } else {
        echo json_encode(range($c[0], $c[1], $c[2])), "\n";
    }
}
var_dump(range("a", "e", 1.5));       // warning: float step for chars
var_dump(range("", "c"));             // warnings: empty start; converted
var_dump(range("aa", "c"));           // warning: multi-byte start
var_dump(range("a", 5));              // warning: converted to 0
var_dump(range(1, "z"));              // warning
var_dump(range("abc", "10"));         // warnings
var_dump(range("", ""));              // warnings
var_dump(range("a", "1.5"));          // warning
var_dump(range(1.5, "a"));            // warning

// --- keys / values / search ---
var_dump(array_keys(["a" => 1, "b" => "1", "c" => 2, "d" => true], 1), array_keys(["a" => 1, "b" => "1", "c" => 2], 1, true), array_keys([1, 2], "x"), array_keys([null, 0, "", false], null), array_keys([null, 0, "", false], null, true), array_keys([]));
var_dump(array_values(["x" => 1, 5 => 2]), array_search("1", [0, 1, "1"]), array_search("1", [0, 1, "1"], true), array_search("z", ["a"]), in_array("1e1", ["10"]), in_array("1e1", ["10"], true), in_array(0, ["a"]), in_array(null, [0]), in_array("abc", [0]));
var_dump(array_key_exists("a", ["a" => null]), key_exists(1, ["1" => 1]), key_exists("1", [1 => 1]), array_key_exists("", ["" => 1]), array_key_exists(true, [1 => 1]), array_key_exists(1.0, [1 => 1]));
var_dump(array_key_exists(null, ["" => 1]));   // deprecated: null key
var_dump(array_key_exists(1.5, [1 => 1]));     // deprecated: float key
var_dump(array_key_first([]), array_key_first([5 => "a", "b" => 1]), array_key_last(["x" => 1, 7 => 2]), array_is_list([1, 2]), array_is_list([1 => 1]), array_is_list([]));

// --- map / filter ---
var_dump(array_map(null, [1, 2], ["a", "b"]), array_map(null, ["k" => 1]), array_map(fn($a, $b) => $a . $b, [1, 2, 3], ["a", "b"]), array_map("strtoupper", ["k" => "a", 5 => "b"]), array_map(fn($a) => $a, ["k" => 1], ["j" => 2]), array_map(null, ["k" => 1], ["j" => 2]), array_map(fn($x) => $x * 2, []));
var_dump(array_filter([1, 0, 2, null, "", "0", "a", [], [0]]), array_filter(["a" => 1, "b" => 2, "c" => 3], fn($k) => $k != "b", ARRAY_FILTER_USE_KEY), array_filter(["a" => 1, "b" => 2, "c" => 3], fn($v, $k) => $v > 1 && $k != "c", ARRAY_FILTER_USE_BOTH), array_filter([1, 2, 3, 4], fn($v) => $v % 2), array_filter([1], null, 5));
var_dump(array_reduce([1, 2, 3], fn($c, $i) => $c + $i), array_reduce([], fn($c, $i) => $c + $i, "init"), array_reduce([1, 2], fn($c, $i) => $c . $i, ""));

// --- count / sum / product ---
var_dump(count([1, [2, 3], [4, [5]]]), count([1, [2, 3], [4, [5]]], COUNT_RECURSIVE), count([]), count([], 1), sizeof([1, 2]));
var_dump(array_sum([1, "a", [1], "3x", null, true, 2.5, " 4", "5 ", "1e1", "0x10"]));   // warnings
var_dump(array_product([2, "a", [1], "3x", null, true, 2.5]));                          // warnings
var_dump(array_sum([]), array_product([]), array_sum([PHP_INT_MAX, 1]), array_sum(["1", "2"]), array_product(["1.5", 2]), array_sum([1.5, 1]), array_product([2, "3"]));

// --- flip / count_values / fill / combine / pad / chunk / column ---
var_dump(array_flip(["a", "b", "a", 5 => "c"]), array_flip([1.5, true, null, [1], "x"]));    // warnings
var_dump(array_count_values([1.5, true, null, [1], "a", 1, "1"]));                          // warnings
var_dump(array_fill(5, 3, "x"), array_fill(-3, 2, 0), array_fill(0, 0, 1), array_fill_keys(["a", 5, "5", 1.5, true], 0), array_combine([], []), array_combine(["a", 1, "1"], [1, 2, 3]));
var_dump(array_pad([1, 2], 4, 0), array_pad([1, 2], -4, 0), array_pad(["a" => 1], 2, "x"), array_pad([1], 1, 0), array_chunk([1, 2, 3, 4, 5], 2), array_chunk(["a" => 1, "b" => 2, "c" => 3], 2, true), array_chunk([], 3));
$rows = [["id" => 1, "n" => "a", "x" => [1]], ["id" => 2, "n" => "b"], "junk", ["n" => "c", "id" => "k"]];
var_dump(array_column($rows, "n"), array_column($rows, "n", "id"), array_column($rows, null, "id"), array_column($rows, "zzz"), array_column([], "n"));

// --- slice / splice / reverse / merge / replace ---
var_dump(array_slice([1, 2, 3, 4], 1, 2), array_slice([1, 2, 3, 4], -2), array_slice([1, 2, 3, 4], 1, -1, true), array_slice(["a" => 1, 5 => 2, 3], 1), array_slice([1, 2], 5), array_slice([1, 2, 3], 0, null, true));
$sp = [1, 2, 3, 4, 5];
var_dump(array_splice($sp, 1, 2, "x"), $sp);
$sp = ["a" => 1, "b" => 2, 5 => 3];
var_dump(array_splice($sp, -1), $sp);
$sp = [1, 2, 3];
var_dump(array_splice($sp, 1, 0, ["a", "b"]), $sp);
$sp = [1, 2, 3];
var_dump(array_splice($sp, 1, -1), $sp);
var_dump(array_reverse([1, 2, "x" => 3]), array_reverse([1, 2, "x" => 3], true), array_reverse([]));
var_dump(array_merge([1, 2], ["a" => 1], [3, "a" => 2]), array_merge(), array_merge([5 => "x"], [5 => "y"]), array_replace([1, 2, 3], [1 => "b"], [3 => "d"]), array_replace(["a" => 1], ["a" => 2, "b" => 3]));
var_dump(array_merge_recursive(["color" => ["favorite" => "red"], 5], [10, "color" => ["favorite" => "green", "blue"]]), array_merge_recursive(["a" => 1], ["a" => 2]), array_merge_recursive(["a" => [1]], ["a" => 2]), array_merge_recursive(["a" => 1], ["a" => [2]]), array_merge_recursive([], ["a" => [1, 2]]), array_merge_recursive(["a" => ["x" => 1]], ["a" => ["x" => 2]]), array_merge_recursive());
var_dump(array_replace_recursive(["citrus" => ["orange"], "berries" => ["blackberry", "raspberry"]], ["citrus" => "pineapple", "berries" => ["blueberry"]]), array_replace_recursive([1, [2, 3]], [[9], 5]), array_replace_recursive(["a" => ["b" => 1]], ["a" => ["c" => 2]], ["a" => ["b" => 3]]));

// --- find / any / all / change_key_case ---
var_dump(array_find([1, 2, 3, 4], fn($v) => $v > 2), array_find([1, 2], fn($v) => $v > 5), array_find_key(["a" => 1, "b" => 2], fn($v, $k) => $v == 2), array_find_key([], fn($v) => true), array_any([1, 2, 3], fn($v) => $v > 2), array_any([], fn($v) => true), array_all([1, 2, 3], fn($v) => $v > 0), array_all([], fn($v) => false), array_all([1, 2], fn($v, $k) => $k < 1));
var_dump(array_change_key_case(["a" => 1, "B" => 2, 5 => 3, "Ab" => 4]), array_change_key_case(["a" => 1, "B" => 2, "A" => 3], CASE_UPPER), array_change_key_case([1], 5));
var_dump(array_unshift($sp, "u"), $sp, array_push($sp, 1, 2), array_pop($sp), array_shift($sp), $sp);
$e = [];
var_dump(array_pop($e), array_shift($e), $e);
