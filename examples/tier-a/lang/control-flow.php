<?php
// Control flow: for / do-while / while, break N / continue N, switch
// (fallthrough, loose comparison, jump table and compare chain), goto,
// match (strict, no-match error caught by a fatal snippet elsewhere),
// ternary / short ternary / coalesce chains.

// for with comma lists, continue and break levels
$out = '';
for ($i = 0, $j = 10; $i < $j; $i++, $j--) {
    if ($i == 1) continue;
    if ($j == 6) break;
    $out .= "$i-$j,";
}
echo $out, "\n";                                  // 0-10,2-8,3-7,

// nested loops with break 2 / continue 2
$s = '';
for ($i = 0; $i < 4; $i++) {
    for ($j = 0; $j < 4; $j++) {
        if ($j == 1) continue 2;
        if ($i == 3) break 2;
        $s .= "$i$j ";
    }
}
echo $s, "\n";                                    // 00 10 20

// do-while runs at least once; while with a compound condition
$n = 10;
do { echo $n, ' '; $n++; } while ($n < 10);
echo "\n";
$k = 0; $acc = [];
while ($k < 10 && count($acc) < 3) { if ($k % 2) $acc[] = $k; $k++; }
echo implode(',', $acc), "\n";                    // 1,3,5

// switch: fallthrough, default, loose comparison ("1" == 1), continue acts as break
function sw($v) {
    $r = '';
    switch ($v) {
        case 1: $r .= 'one';
        case 2: $r .= 'two'; break;
        case 'a': $r .= 'A'; break;
        default: $r .= 'def';
    }
    return $r;
}
echo sw(1), '|', sw(2), '|', sw('1'), '|', sw('a'), '|', sw(9), "\n";
for ($i = 0; $i < 3; $i++) {
    switch ($i) {
        case 1: continue 2;
        case $i: echo "i$i";      // non-literal case: compare chain
    }
    echo ';';
}
echo "\n";
switch (true) {
    case 5 > 3: echo "gt\n"; break;
    default: echo "no\n";
}

// goto: forward and backward
$g = 0;
loop:
$g++;
if ($g < 3) goto loop;
goto done;
echo "skipped\n";
done:
echo "g=$g\n";

// match: strict comparison, multiple conditions, default, arbitrary conditions
function m($v) {
    return match ($v) {
        1, 2 => 'small',
        '1' => 'string one',
        3 => 'three',
        default => 'other',
    };
}
echo m(1), '|', m('1'), '|', m(3), '|', m(3.0), "\n";
$x = 7;
echo match (true) { $x < 5 => 'lt5', $x < 10 => 'lt10', default => 'big' }, "\n";

// ternaries and coalescing
$a = null; $b = 0; $c = 'c';
echo $a ?? $b ?? $c, '|', $a ?: 'x', '|', $c ?: 'y', '|', $b ? 't' : 'f', '|', ($a ?? 'd') . 'e', "\n";
$arr = ['k' => ['n' => null]];
echo $arr['k']['n'] ?? 'nn', '|', $arr['k']['z']['q'] ?? 'zq', '|', $undef['a'] ?? 'u', "\n";
echo (1 < 2 ? 'yes' : 'no'), ' ', (0 ?: (null ?? 'chain')), "\n";

// early return through nested loops, and a while(true) with break
function find($needle, $hay) {
    foreach ($hay as $i => $row) {
        foreach ($row as $j => $v) {
            if ($v === $needle) return "$i,$j";
        }
    }
    return 'none';
}
echo find(5, [[1, 2], [3, 5]]), ' ', find(9, [[1]]), "\n";
$t = 0;
while (true) { if (++$t >= 4) break; }
echo $t, "\n";
