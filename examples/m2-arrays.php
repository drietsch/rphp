<?php
// Arrays slice demo: literals, keys, indexing, append, foreach, copy-on-write,
// the union operator, and string offsets. Builds on the strings slice.

// --- literals: positional, keyed, and the array(...) form ---
$nums = [10, 20, 30];
$ages = ["alice" => 30, "bob" => 25];
$mixed = array(1, "two" => 2, 3);   // positional keys 0 and 1 around "two"

echo "nums[1] = " . $nums[1] . "\n";        // 20
echo "ages[bob] = " . $ages["bob"] . "\n";  // 25
echo "mixed[1] = " . $mixed[1] . "\n";       // 3  (second positional element)

// --- append + auto-vivification ---
$log = [];
$log[] = "first";
$log[] = "second";
echo $log[0] . ", " . $log[1] . "\n";        // first, second

// --- foreach over values, and over key => value ---
$total = 0;
foreach ($nums as $n) {
    $total = $total + $n;
}
echo "sum = " . $total . "\n";               // 60

foreach ($ages as $name => $age) {
    echo "  $name is $age\n";                // alice is 30 / bob is 25
}

// --- copy-on-write: arrays are values, not references ---
$a = [1, 2];
$b = $a;        // copy
$b[] = 3;       // mutates only $b
echo "a has 2 elems, b has 3: " . $a[1] . " vs " . $b[2] . "\n"; // 2 vs 3

// --- the union operator (+) keeps left-hand keys ---
$defaults = ["color" => "red", "size" => "M"];
$chosen = ["color" => "blue"] + $defaults;
echo "color = " . $chosen["color"] . ", size = " . $chosen["size"] . "\n"; // blue, M

// --- nested arrays (read) and string offsets ---
$grid = [[1, 2], [3, 4]];
echo "grid[1][0] = " . $grid[1][0] . "\n";   // 3
$word = "PHP";
echo "first/last char: " . $word[0] . $word[-1] . "\n"; // PP
