<?php
// Tier-A differential: Random\Randomizer over every seedable engine and user
// engines of odd output widths — getInt() across 32- and 64-bit ranges (the
// exact rejection sampling that stitches short draws together), nextInt(),
// getBytes(), getBytesFromString() (masked fast path and the range path for
// alphabets above 256 bytes), shuffleArray(), shuffleBytes(),
// pickArrayKeys(), the MT_RAND_PHP bad scaling, and draws shared with the
// engine object a script holds.

use Random\Engine\Mt19937;
use Random\Engine\PcgOneseq128XslRr64;
use Random\Engine\Xoshiro256StarStar;
use Random\Randomizer;

/** A deterministic engine returning $width bytes per call. */
final class Sha implements \Random\Engine
{
    private int $i = 0;

    public function __construct(private int $width, private int $seed)
    {
    }

    public function generate(): string
    {
        $this->i++;
        return substr(hash('sha256', $this->seed . ':' . $this->i, true), 0, $this->width);
    }
}

$engines = [];
foreach ([0, 1, 42, -7, PHP_INT_MAX] as $s) {
    $engines["mt$s"] = fn() => new Mt19937($s);
    $engines["pcg$s"] = fn() => new PcgOneseq128XslRr64($s);
    $engines["xo$s"] = fn() => new Xoshiro256StarStar($s);
}
$engines['mt-legacy'] = fn() => @new Mt19937(3, MT_RAND_PHP);
$engines['pcg-str'] = fn() => new PcgOneseq128XslRr64('0123456789abcdef');
$engines['xo-str'] = fn() => new Xoshiro256StarStar(str_repeat('ab', 16));
foreach ([1, 2, 3, 5, 8, 9, 13] as $w) {
    $engines["user$w"] = fn() => new Sha($w, $w);
}

$ranges = [[1, 6], [0, 1], [-100, 100], [0, 255], [0, 1000000007], [0, 4294967295], [0, 4294967296],
    [PHP_INT_MIN, PHP_INT_MAX], [0, PHP_INT_MAX], [-5, -5], [10, 2 ** 40], [PHP_INT_MIN, 0]];
$all = implode('', array_map('chr', range(0, 255)));

foreach ($engines as $name => $mk) {
    echo "== $name\n";
    $r = new Randomizer($mk());
    $o = [];
    foreach ($ranges as [$a, $b]) {
        $o[] = $r->getInt($a, $b);
    }
    echo implode(' ', $o), "\n";
    $o = [];
    for ($i = 0; $i < 8; $i++) {
        $o[] = $r->getInt(1, 10);
    }
    echo implode('', $o), ' ', $r->nextInt(), ' ', $r->nextInt(), "\n";
    echo bin2hex($r->getBytes(1)), ' ', bin2hex($r->getBytes(7)), ' ', bin2hex($r->getBytes(8)), ' ', bin2hex($r->getBytes(17)), "\n";
    echo $r->getBytesFromString('abc', 10), ' ', $r->getBytesFromString('0123456789abcdef', 12), ' ', $r->getBytesFromString('x', 3), "\n";
    echo bin2hex($r->getBytesFromString($all, 6)), ' ', md5($r->getBytesFromString(str_repeat('abcdefghijklmnopqrstuvwxyz', 12), 40)), "\n";
    echo implode(',', $r->shuffleArray(range(1, 12))), ' ', $r->shuffleBytes('abcdefghij'), ' ', $r->shuffleBytes('a'), '|', $r->shuffleBytes(''), "\n";
    echo json_encode($r->pickArrayKeys(['a' => 1, 'b' => 2, 'c' => 3, 'd' => 4, 'e' => 5], 2)),
        json_encode($r->pickArrayKeys(range(0, 6), 5)),
        json_encode($r->pickArrayKeys([5 => 'x', 9 => 'y', 'k' => 'z'], 1)),
        json_encode($r->pickArrayKeys(['only' => 1], 1)),
        json_encode($r->pickArrayKeys(range(0, 3), 4)), "\n";
}

// shuffleArray keeps values (and references), drops keys.
$v = 1;
$arr = ['x' => &$v, 'y' => 2, 'z' => [3]];
$out = (new Randomizer(new Mt19937(8)))->shuffleArray($arr);
var_dump($out);
$v = 99;
echo json_encode($out), "\n";

// The Randomizer draws from the object the script holds.
$e = new PcgOneseq128XslRr64(5);
$r = new Randomizer($e);
$twin = clone $e;
$r->getInt(1, 100);
$twin->generate();
echo bin2hex($e->generate()), ' ', bin2hex($twin->generate()), ' ', $r->engine === $e ? 'same' : 'other', "\n";

// Legacy mode keeps RAND_RANGE_BADSCALING; MT19937 mode matches mt_rand().
$legacy = new Randomizer(@new Mt19937(42, MT_RAND_PHP));
echo $legacy->getInt(1, 100), ' ', $legacy->getInt(PHP_INT_MIN, PHP_INT_MAX), ' ', $legacy->getInt(0, 0), "\n";
mt_srand(42);
$std = new Randomizer(new Mt19937(42));
echo $std->getInt(1, 100), ' ', mt_rand(1, 100), ' ', $std->getInt(0, 4294967296), ' ', mt_rand(0, 4294967296), "\n";
mt_srand(3);
$std = new Randomizer(new Mt19937(3));
echo implode(',', $std->shuffleArray(range(1, 8))), ' ';
$a = range(1, 8);
shuffle($a);
echo implode(',', $a), ' ', $std->shuffleBytes('abcdefgh'), ' ', str_shuffle('abcdefgh'), "\n";
