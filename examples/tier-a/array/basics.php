<?php
// Differential snippet for the array extension additions. Every newly added
// function is exercised and its result printed (print_r/var_dump) so rphp's
// output can be diffed byte-for-byte against stock PHP 8.5. Only plain literals
// and supported control flow are used here.

// array_slice: positive/negative offset and length, key reindexing, preserve.
$letters = ["a", "b", "c", "d", "e"];
print_r(array_slice($letters, 2));
print_r(array_slice($letters, -2, 1));
print_r(array_slice($letters, 1, -1));
print_r(array_slice($letters, 2, 2, true));
$mixed = ["x" => 1, "y" => 2, "z" => 3, 4 => 5, 5 => 6];
print_r(array_slice($mixed, 1, 3));

// array_flip: keys and values swap; numeric-string values fold to int keys.
print_r(array_flip(["a" => 1, "b" => 2, "c" => 1]));
print_r(array_flip(["p", "q", "r"]));

// array_unique: first occurrence wins, keys preserved (SORT_STRING compare).
print_r(array_unique([1, "1", 2, 2, "3"]));
print_r(array_unique(["a", "b", "a", "c"]));

// array_search: loose match returns the key, strict honors type, miss is false.
var_dump(array_search("2", [1, 2, 3]));
var_dump(array_search("2", [1, 2, 3], true));
var_dump(array_search(99, [1, 2, 3]));
var_dump(array_search(3, ["x" => 1, "y" => 3]));

// array_fill: consecutive int keys from the start index (incl. negative).
print_r(array_fill(5, 3, "x"));
print_r(array_fill(-2, 4, 0));

// array_fill_keys: each given value becomes a key mapped to the fill value.
print_r(array_fill_keys(["a", "b", 3], 0));

// array_combine: pair keys with values position by position.
print_r(array_combine(["one", "two", "three"], [1, 2, 3]));

// array_pad: pad right (positive), pad left (negative), and no-op when large.
print_r(array_pad([1, 2, 3], 5, 0));
print_r(array_pad([1, 2, 3], -5, 0));
print_r(array_pad([1, 2, 3], 2, 0));
print_r(array_pad(["a" => 1, 7 => 2], 4, 9));
print_r(array_pad(["a" => 1, 7 => 2], -4, 9));

// array_column: pluck a column, optionally rekey by another column or take rows.
$rows = [
    ["id" => 1, "name" => "alpha"],
    ["id" => 2, "name" => "beta"],
];
print_r(array_column($rows, "name"));
print_r(array_column($rows, "name", "id"));
print_r(array_column($rows, null, "id"));

// array_chunk: split into fixed-size blocks, reindexed or key-preserving.
print_r(array_chunk([1, 2, 3, 4, 5], 2));
print_r(array_chunk(["a" => 1, "b" => 2, "c" => 3], 2, true));

// array_product: numeric product; empty array is 1; strings coerce; floats lift.
echo array_product([2, 3, 4]), "\n";
echo array_product([]), "\n";
echo array_product([2, 1.5]), "\n";
echo array_product(["2", "3"]), "\n";

// array_count_values: tally occurrences (int/string values only).
print_r(array_count_values([1, 1, 2, "a", "a", "a"]));
print_r(array_count_values([1, "1"]));

// array_key_first / array_key_last: ends of the key order, null when empty.
var_dump(array_key_first(["x" => 1, "y" => 2]));
var_dump(array_key_last(["x" => 1, "y" => 2]));
var_dump(array_key_first([]));
var_dump(array_key_last([]));

// array_is_list: consecutive 0-based int keys.
var_dump(array_is_list([1, 2, 3]));
var_dump(array_is_list([1 => 1, 0 => 2]));
var_dump(array_is_list([]));
var_dump(array_is_list(["a" => 1]));

// array_diff: entries of the first array absent from the rest (string compare).
print_r(array_diff([1, 2, 3, 4], [2, 4]));
print_r(array_diff(["a" => 1, "b" => 2, "c" => 3], [2]));
print_r(array_diff([1, "2", 3], ["2"], [3]));

// array_intersect: entries of the first array present in all the others.
print_r(array_intersect([1, 2, 3, 4], [2, 4, 6]));
print_r(array_intersect(["a" => 1, "b" => 2, "c" => 3], [2, 3], [3, 2]));

// array_replace: later arrays overwrite matching keys (no int renumber).
print_r(array_replace([1, 2, 3], [1 => "x", 3 => "y"]));
print_r(array_replace(["a" => 1, "b" => 2], ["b" => 3, "c" => 4]));
