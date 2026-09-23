<?php
// Tier-A differential: Randomizer::nextFloat() and getFloat() — php's
// γ-section for all four IntervalBoundary cases, on intervals near zero,
// with opposite signs, where |min| > |max|, across binade boundaries, at the
// extremes of the double range and one ulp wide; plus the argument errors.

use Random\Engine\Mt19937;
use Random\Engine\PcgOneseq128XslRr64;
use Random\Engine\Xoshiro256StarStar;
use Random\IntervalBoundary;
use Random\Randomizer;

// Floats print through json_encode (serialize_precision -1: every bit).
$intervals = [[0, 1], [-1, 1], [-10, 1], [1, 2], [-3.5, -1.25], [0.1, 0.3], [-1e300, 1e300],
    [1e10, 1e10 + 1], [-2, 1e-5], [0.5, 4], [-PHP_FLOAT_MAX, PHP_FLOAT_MAX], [0, PHP_FLOAT_MIN],
    [1.0, 1.0000000000000004], [-1e-300, 0], [100, 1e6]];

foreach ([new Mt19937(1), new PcgOneseq128XslRr64(2), new Xoshiro256StarStar(3)] as $e) {
    $r = new Randomizer($e);
    echo get_class($e), "\n";
    echo '  next ', json_encode([$r->nextFloat(), $r->nextFloat(), $r->nextFloat()]), "\n";
    foreach ($intervals as [$a, $b]) {
        $f = [];
        foreach (IntervalBoundary::cases() as $bd) {
            $f[] = $r->getFloat($a, $b, $bd);
        }
        $f[] = $r->getFloat($a, $b);
        echo '  ', json_encode([$a, $b]), ' ', json_encode($f), "\n";
    }
    echo '  point ', json_encode([$r->getFloat(5, 5, IntervalBoundary::ClosedClosed), $r->getFloat(-0.0, 0.0, IntervalBoundary::ClosedClosed)]), "\n";
    echo '  ulp ', json_encode([$r->getFloat(1.0, 1.0000000000000002, IntervalBoundary::OpenClosed), $r->getFloat(1.0, 1.0000000000000002, IntervalBoundary::ClosedOpen)]), "\n";
}

// Every value of a two-step interval, in order.
$r = new Randomizer(new Xoshiro256StarStar(11));
$seen = [];
for ($i = 0; $i < 200; $i++) {
    $seen[json_encode($r->getFloat(1.0, 1.0000000000000004, IntervalBoundary::ClosedClosed))] = true;
}
ksort($seen);
echo implode(' ', array_keys($seen)), "\n";

// A named boundary argument.
echo json_encode((new Randomizer(new Mt19937(5)))->getFloat(0, 1, boundary: IntervalBoundary::OpenOpen)), "\n";

function t(callable $f): void
{
    try {
        var_dump($f());
    } catch (\Throwable $t) {
        echo get_class($t), ': ', $t->getMessage(), "\n";
    }
}
$r = new Randomizer(new Mt19937(1));
t(fn() => $r->getFloat(1, 1));
t(fn() => $r->getFloat(2, 1, IntervalBoundary::ClosedClosed));
t(fn() => $r->getFloat(2, 1, IntervalBoundary::OpenClosed));
t(fn() => $r->getFloat(1, 1, IntervalBoundary::OpenOpen));
t(fn() => $r->getFloat(INF, 1));
t(fn() => $r->getFloat(-INF, 1));
t(fn() => $r->getFloat(1, NAN));
t(fn() => $r->getFloat(1.0, 1.0000000000000002, IntervalBoundary::OpenOpen));
t(fn() => $r->getFloat(1, 2, 3));
t(fn() => $r->getFloat('a', 2));
t(fn() => $r->getFloat('1.5', '2.5'));
t(fn() => \Random\IntervalBoundary::cases());
t(fn() => \Random\IntervalBoundary::OpenClosed->name);
