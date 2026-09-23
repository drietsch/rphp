<?php
// Floats as strings follow the `precision` ini setting: echo, casts,
// interpolation and string functions; -1 is the shortest round-trip form.
$values = [1/3, 1e20/3, 0.1 + 0.2, 123456.789, -0.0, 1e-7, 2.5, 7.0, 1e15, -1.5e-5, 100.0, 1.5e300, 5e-324];
foreach ([-1, 0, 1, 5, 14, 17, 20, 40] as $p) {
    ini_set('precision', $p);
    echo str_pad((string) $p, 3), ': ', implode(' ', $values), "\n";
}
ini_set('precision', 3);
echo "interpolated {$values[0]} ", strlen((string) (1/7)), ' ', str_repeat((string) 0.5, 2), "\n";
var_dump(1/3); // var_dump follows serialize_precision, not precision
echo json_encode(1/3), ' ', var_export(1/3, true), "\n";
ini_restore('precision');
echo 1/3, "\n";
ini_set('precision', 'abc');
echo 1/3, "\n";
