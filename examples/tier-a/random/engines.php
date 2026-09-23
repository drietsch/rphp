<?php
// Tier-A differential: the three seedable Random\Engine classes — raw
// generate() output for integer and string seeds (Mt19937 in both modes,
// PcgOneseq128XslRr64, Xoshiro256StarStar), PCG's jump() against stepping,
// Xoshiro's jump()/jumpLong(), clone forking the state, the state arrays of
// __serialize()/__debugInfo(), and the engines' independence from mt_rand().

use Random\Engine\Mt19937;
use Random\Engine\PcgOneseq128XslRr64;
use Random\Engine\Xoshiro256StarStar;

function run(\Random\Engine $e, int $n = 6): string
{
    $out = [];
    for ($i = 0; $i < $n; $i++) {
        $out[] = bin2hex($e->generate());
    }
    return implode(' ', $out);
}

foreach ([0, 1, 2, 42, 1234567, -1, -42, PHP_INT_MAX, PHP_INT_MIN, 4294967296] as $seed) {
    echo "seed $seed\n";
    echo '  mt   ', run(new Mt19937($seed)), "\n";
    echo '  pcg  ', run(new PcgOneseq128XslRr64($seed)), "\n";
    echo '  xo   ', run(new Xoshiro256StarStar($seed)), "\n";
}
echo 'mt legacy ', run(@new Mt19937(42, MT_RAND_PHP)), "\n";
echo 'mt mode 0 ', run(new Mt19937(42, MT_RAND_MT19937)), "\n";
echo 'pcg str   ', run(new PcgOneseq128XslRr64('0123456789abcdef')), "\n";
echo 'pcg nul   ', run(new PcgOneseq128XslRr64(str_repeat("\0", 16))), "\n";
echo 'xo str    ', run(new Xoshiro256StarStar(str_repeat("\x01\x02\x03\x04", 8))), "\n";
echo 'xo one    ', run(new Xoshiro256StarStar("\x01" . str_repeat("\0", 31))), "\n";

// Mt19937 across the 624-word reload, and its 4-byte output width.
$mt = new Mt19937(7);
for ($i = 0; $i < 1300; $i++) {
    $mt->generate();
}
echo run($mt, 3), ' ', strlen($mt->generate()), "\n";

// PCG: jump($n) is $n steps.
$a = new PcgOneseq128XslRr64(99);
$b = clone $a;
$a->jump(12345);
for ($i = 0; $i < 12345; $i++) {
    $b->generate();
}
echo run($a, 2), ' | ', run($b, 2), "\n";
$a->jump(0);
echo run($a, 1), "\n";
$a->jump(PHP_INT_MAX);
echo run($a, 2), "\n";

// Xoshiro: jump() is 2^128 steps, jumpLong() 2^192.
$x = new Xoshiro256StarStar(5);
$x->jump();
echo run($x, 2), "\n";
$x->jumpLong();
echo run($x, 2), "\n";

// clone forks the state; the copies then run in step.
foreach ([new Mt19937(3), new PcgOneseq128XslRr64(3), new Xoshiro256StarStar(3)] as $e) {
    $e->generate();
    $c = clone $e;
    echo get_class($e), ': ', run($e, 2), ' = ', run($c, 2), "\n";
}

// The state arrays.
$p = new PcgOneseq128XslRr64(1);
print_r($p->__debugInfo());
print_r($p->__serialize());
$p->generate();
print_r($p);
echo "\n";
print_r((new Xoshiro256StarStar(1))->__serialize());
$m = new Mt19937(42);
$m->generate();
$m->generate();
$s = $m->__serialize();
echo count($s), ' ', count($s[0]), ' ', count($s[1]), ' ', $s[1][0], ' ', $s[1][623], ' ', $s[1][624], ' ', $s[1][625], "\n";
$d = $m->__debugInfo();
echo implode(',', array_keys($d)), ' ', count($d['__states']), ' ', md5(implode('', array_slice($d['__states'], 0, 624))), "\n";
$legacy = @new Mt19937(1, MT_RAND_PHP);
echo $legacy->__serialize()[1][625], "\n";

// The engine's state is its own: mt_rand() and a Mt19937 object with the
// same seed produce the same words (>> 1 for mt_rand) without sharing.
mt_srand(42);
$e = new Mt19937(42);
$w = unpack('V', $e->generate())[1];
echo $w >> 1, ' ', mt_rand(), "\n";
$w = unpack('V', $e->generate())[1];
mt_srand(1);
echo $w >> 1, ' ', mt_rand(), "\n";
