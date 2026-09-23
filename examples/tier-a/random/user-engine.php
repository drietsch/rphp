<?php
// Tier-A differential: user classes implementing Random\Engine behind a
// Randomizer — outputs of 1 to 12 bytes stitched into 32/64-bit words (and
// cut at 8), a counter engine whose every draw is visible, an engine
// returning by reference, one throwing, the empty string
// (BrokenRandomEngineError) and a constant engine that exhausts the 50
// rejection attempts in each method that samples.

use Random\Randomizer;

final class Counter implements \Random\Engine
{
    public int $n = 0;

    public function __construct(private int $width = 4)
    {
    }

    public function generate(): string
    {
        $this->n++;
        return substr(pack('P', $this->n * 0x0101010101), 0, $this->width) . str_repeat("\xAA", max(0, $this->width - 8));
    }
}

foreach ([1, 2, 3, 4, 5, 7, 8, 9, 12] as $w) {
    $c = new Counter($w);
    $r = new Randomizer($c);
    $o = [$r->getInt(0, 1000), $r->getInt(0, 4294967295), $r->getInt(-1, 4294967296), $r->nextInt(), bin2hex($r->getBytes(11))];
    $o[] = json_encode($r->nextFloat());
    $o[] = json_encode($r->getFloat(0, 1));
    $o[] = $r->getBytesFromString('abcdefgh', 5);
    $o[] = implode('', $r->shuffleArray(range(0, 9)));
    $o[] = json_encode($r->pickArrayKeys(range(0, 9), 3));
    echo "width $w: ", implode(' ', $o), " draws={$c->n}\n";
}

final class ByRef implements \Random\Engine
{
    public string $s = "\x01\x02\x03\x04\x05\x06\x07\x08";

    public function &generate(): string
    {
        return $this->s;
    }
}
$r = new Randomizer(new ByRef());
echo $r->getInt(0, 1000), ' ', $r->nextInt(), ' ', bin2hex($r->getBytes(3)), "\n";

final class Numeric implements \Random\Engine
{
    public function generate(): string
    {
        return 12345;
    }
}
$r = new Randomizer(new Numeric());
echo $r->nextInt(), ' ', bin2hex($r->engine->generate()), "\n";

function t(callable $f): void
{
    try {
        $v = $f();
        echo json_encode($v), "\n";
    } catch (\Throwable $t) {
        echo get_class($t), ': ', $t->getMessage(), "\n";
    }
}

final class Thrower implements \Random\Engine
{
    public function generate(): string
    {
        throw new \RuntimeException('engine failed');
    }
}
$r = new Randomizer(new Thrower());
t(fn() => $r->getInt(1, 5));
t(fn() => $r->nextInt());
t(fn() => $r->getBytes(4));
t(fn() => $r->shuffleBytes('abc'));

final class Nothing implements \Random\Engine
{
    public function generate(): string
    {
        return '';
    }
}
$r = new Randomizer(new Nothing());
t(fn() => $r->nextInt());
t(fn() => $r->nextFloat());
t(fn() => $r->getInt(1, 10));
t(fn() => $r->getBytes(1));
t(fn() => $r->getBytesFromString('ab', 1));
t(fn() => $r->shuffleArray([1, 2]));
t(fn() => $r->shuffleBytes('x'));
t(fn() => $r->pickArrayKeys([1], 1));

final class Ones implements \Random\Engine
{
    public int $calls = 0;

    public function generate(): string
    {
        $this->calls++;
        return "\xff\xff\xff\xff\xff\xff\xff\xff";
    }
}
$e = new Ones();
$r = new Randomizer($e);
t(fn() => $r->getInt(1, 10));
t(fn() => $r->getInt(0, 2 ** 40));
t(fn() => $r->getInt(0, 15));
t(fn() => $r->getBytesFromString('abc', 3));
t(fn() => $r->pickArrayKeys([1, 2, 3, 4], 2));
t(fn() => $r->shuffleArray([1, 2, 3]));
t(fn() => $r->nextInt());
t(fn() => $r->nextFloat());
t(fn() => $r->getFloat(0, 1));
t(fn() => $r->getFloat(0, 1, \Random\IntervalBoundary::ClosedClosed));
echo 'calls ', $e->calls, "\n";

// Zero bytes: every range is zero, pickArrayKeys fails after 50 repeats.
final class Zeros implements \Random\Engine
{
    public function generate(): string
    {
        return "\0";
    }
}
$r = new Randomizer(new Zeros());
t(fn() => [$r->getInt(5, 10), $r->nextInt(), $r->getBytesFromString('xyz', 4), $r->shuffleBytes('abcd')]);
t(fn() => $r->pickArrayKeys([1, 2, 3, 4], 2));
