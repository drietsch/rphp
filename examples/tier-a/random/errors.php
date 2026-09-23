<?php
// Tier-A differential: the argument checks of the engines and the
// Randomizer — seed type and length errors, Xoshiro's all-NUL seed, the
// Mt19937 mode check and MT_RAND_PHP's two deprecations, PCG's negative
// jump, float/numeric-string/null coercions, every ValueError of the
// Randomizer's methods, and an uncaught one at the end (exit 255).

use Random\Engine\Mt19937;
use Random\Engine\PcgOneseq128XslRr64;
use Random\Engine\Xoshiro256StarStar;
use Random\Randomizer;

function t(callable $f): void
{
    try {
        $v = $f();
        echo is_object($v) ? get_class($v) : json_encode($v), "\n";
    } catch (\Throwable $t) {
        echo get_class($t), ': ', $t->getMessage(), "\n";
    }
}

t(fn() => new PcgOneseq128XslRr64('abc'));
t(fn() => new PcgOneseq128XslRr64(str_repeat('a', 17)));
t(fn() => new PcgOneseq128XslRr64('123'));
t(fn() => new PcgOneseq128XslRr64([]));
t(fn() => bin2hex((new PcgOneseq128XslRr64(1.5))->generate()));
t(fn() => bin2hex((new PcgOneseq128XslRr64(true))->generate()));
t(fn() => bin2hex((new PcgOneseq128XslRr64(null))->generate()) !== '');
t(fn() => new Xoshiro256StarStar('abc'));
t(fn() => new Xoshiro256StarStar(str_repeat("\0", 32)));
t(fn() => new Xoshiro256StarStar(new \stdClass()));
t(fn() => bin2hex((new Xoshiro256StarStar(2.0))->generate()));
t(fn() => new Mt19937(1, 5));
t(fn() => new Mt19937(1, -1));
t(fn() => bin2hex((new Mt19937(1, MT_RAND_PHP))->generate()));
t(fn() => new Mt19937('x'));
t(fn() => bin2hex((new Mt19937('7'))->generate()));
t(fn() => bin2hex((new Mt19937(7.0))->generate()));
t(fn() => bin2hex((new Mt19937(7.5))->generate()));
t(fn() => new Mt19937([]));
t(fn() => new Mt19937(1, 'x'));
t(fn() => (new PcgOneseq128XslRr64(1))->jump(-1));
t(fn() => (new PcgOneseq128XslRr64(1))->jump('x'));
t(fn() => new Randomizer(new \stdClass()));
t(fn() => new Randomizer(1));

$r = new Randomizer(new Mt19937(1));
t(fn() => $r->getInt(5, 1));
t(fn() => $r->getInt('a', 1));
t(fn() => $r->getInt('3', '4'));
t(fn() => $r->getInt(1.0, 2.5));
t(fn() => $r->getInt(null, 3));
t(fn() => $r->getBytes(0));
t(fn() => $r->getBytes(-5));
t(fn() => $r->getBytesFromString('', 0));
t(fn() => $r->getBytesFromString('', 1));
t(fn() => $r->getBytesFromString('a', 0));
t(fn() => $r->getBytesFromString(12, 3));
t(fn() => $r->getBytesFromString([], 3));
t(fn() => $r->pickArrayKeys([], 1));
t(fn() => $r->pickArrayKeys([1, 2], 3));
t(fn() => $r->pickArrayKeys([1, 2], 0));
t(fn() => $r->pickArrayKeys([1, 2], -1));
t(fn() => $r->pickArrayKeys('x', 1));
t(fn() => $r->shuffleArray('x'));
t(fn() => $r->shuffleBytes([]));
t(fn() => $r->shuffleBytes(123));
t(fn() => $r->getInt(1));
t(fn() => $r->getInt(max: 2));
t(fn() => $r->getInt(1, 2, 3));

$r->getInt(10, 1);
