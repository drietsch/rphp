<?php
// Tier-A differential: serialize()/unserialize() of the engines and the
// Randomizer — the exact serialized form, sequences that continue across a
// round trip, upper-case hex state accepted, and every rejection: wrong
// element counts, bad hex, an out-of-range Mt19937 index or mode, an
// all-zero Xoshiro state, extra members, a non-engine, an already
// constructed Randomizer (GH-19765), and Secure (not serializable).

use Random\Engine\Mt19937;
use Random\Engine\PcgOneseq128XslRr64;
use Random\Engine\Secure;
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

echo serialize(new PcgOneseq128XslRr64(1)), "\n";
echo serialize(new Xoshiro256StarStar(1)), "\n";
echo serialize(new Randomizer(new PcgOneseq128XslRr64(1))), "\n";
echo strlen(serialize(new Mt19937(1))), ' ', md5(serialize(new Mt19937(1))), "\n";

foreach ([new Mt19937(9), new PcgOneseq128XslRr64(9), new Xoshiro256StarStar(9), @new Mt19937(9, MT_RAND_PHP)] as $e) {
    for ($i = 0; $i < 700; $i++) {
        $e->generate();
    }
    $s = serialize($e);
    $u = unserialize($s);
    echo get_class($u), ' ', md5($s), ' ', bin2hex($e->generate()), ' ', bin2hex($u->generate()), "\n";
    $r = new Randomizer($e);
    $r2 = unserialize(serialize($r));
    echo '  ', $r->getInt(1, 1000), ' ', $r2->getInt(1, 1000), ' ', $r2->engine === $e ? 'same' : 'copy', ' ', get_class($r2->engine), "\n";
}

// A user engine inside a Randomizer serializes with its properties.
final class Seq implements \Random\Engine
{
    public int $i = 0;

    public function generate(): string
    {
        return chr(++$this->i);
    }
}
$r = new Randomizer(new Seq());
$r->getInt(0, 100);
$s = serialize($r);
echo $s, "\n";
echo unserialize($s)->getInt(0, 100), ' ', $r->getInt(0, 100), "\n";

$pcg = 'O:33:"Random\Engine\PcgOneseq128XslRr64":2:{i:0;a:0:{}i:1;a:2:{i:0;s:16:"%s";i:1;s:16:"%s";}}';
t(fn() => bin2hex(unserialize(sprintf($pcg, '9CD108B9CEABD26B', 'DF3B50D88069A5EF'))->generate()));
t(fn() => unserialize(sprintf($pcg, '9cd108b9ceabd26x', 'df3b50d88069a5ef')));
t(fn() => unserialize('O:33:"Random\Engine\PcgOneseq128XslRr64":2:{i:0;a:0:{}i:1;a:2:{i:0;s:15:"9cd108b9ceabd26";i:1;s:16:"df3b50d88069a5ef";}}'));
t(fn() => unserialize('O:33:"Random\Engine\PcgOneseq128XslRr64":2:{i:0;a:1:{s:3:"foo";i:1;}i:1;a:2:{i:0;s:16:"9cd108b9ceabd26b";i:1;s:16:"df3b50d88069a5ef";}}'));
t(fn() => unserialize('O:33:"Random\Engine\PcgOneseq128XslRr64":1:{i:0;a:0:{}}'));
t(fn() => unserialize('O:33:"Random\Engine\PcgOneseq128XslRr64":2:{i:0;a:0:{}i:1;a:1:{i:0;s:16:"9cd108b9ceabd26b";}}'));
t(fn() => unserialize('O:32:"Random\Engine\Xoshiro256StarStar":2:{i:0;a:0:{}i:1;a:4:{i:0;s:16:"0000000000000000";i:1;s:16:"0000000000000000";i:2;s:16:"0000000000000000";i:3;s:16:"0000000000000000";}}'));
t(fn() => bin2hex(unserialize('O:32:"Random\Engine\Xoshiro256StarStar":2:{i:0;a:0:{}i:1;a:4:{i:0;s:16:"0100000000000000";i:1;s:16:"0000000000000000";i:2;s:16:"0000000000000000";i:3;s:16:"0000000000000000";}}')->generate()));
t(fn() => unserialize('O:32:"Random\Engine\Xoshiro256StarStar":2:{i:0;a:0:{}i:1;a:4:{i:0;i:1;i:1;s:16:"0000000000000000";i:2;s:16:"0000000000000000";i:3;s:16:"0000000000000000";}}'));

$mt = (new Mt19937(4))->__serialize();
$tweak = function (int $i, $v) use ($mt) {
    $d = $mt;
    $d[1][$i] = $v;
    $e = new Mt19937(1);
    $e->__unserialize($d);
    return bin2hex($e->generate());
};
t(fn() => $tweak(624, 0));
t(fn() => $tweak(624, 623));
t(fn() => $tweak(624, 624));
t(fn() => $tweak(624, 625));
t(fn() => $tweak(624, -1));
t(fn() => $tweak(624, '1'));
t(fn() => $tweak(625, 1));
t(fn() => $tweak(625, 2));
t(fn() => $tweak(0, 'zzzzzzzz'));
t(fn() => $tweak(0, 'ABCDEF01'));
t(fn() => $tweak(3, 1));
t(fn() => (new Mt19937(1))->__unserialize([]));
t(fn() => (new Mt19937(1))->__unserialize([[], []]));
t(fn() => (new Mt19937(1))->__unserialize([1, $mt[1]]));
t(fn() => (new Mt19937(1))->__unserialize('x'));

t(fn() => unserialize('O:17:"Random\Randomizer":0:{}'));
t(fn() => unserialize('O:17:"Random\Randomizer":1:{i:0;a:1:{s:6:"engine";O:8:"stdClass":0:{}}}'));
t(fn() => unserialize('O:17:"Random\Randomizer":1:{i:0;a:0:{}}'));
t(fn() => unserialize('O:17:"Random\Randomizer":1:{i:0;s:1:"x";}'));
t(fn() => (new Randomizer(new Mt19937(1)))->__unserialize([['engine' => new PcgOneseq128XslRr64(1)]]));
$r = new Randomizer(new Mt19937(1));
try {
    $r->__unserialize([['engine' => new PcgOneseq128XslRr64(1)]]);
} catch (\Exception $e) {
    echo $e->getMessage(), "\n";
}
echo get_class($r->engine), "\n";

t(fn() => serialize(new Secure()));
t(fn() => serialize(new Randomizer()));
t(fn() => serialize(new Randomizer(new Secure())));
t(fn() => unserialize('O:20:"Random\Engine\Secure":0:{}'));
t(fn() => method_exists(Secure::class, '__serialize'));
t(fn() => (new Randomizer(new Xoshiro256StarStar(2)))->__serialize()[0]['engine']->__serialize()[1]);
