<?php

// The one-line and block descriptions every reflector prints, which a
// container's resource signature hashes. (A constant default prints its
// *value* here where php prints its name — see reflection/func.rs — so none
// is used below.)
#[Attribute]
class Tag
{
    public function __construct(public string $n = '', public array $list = []) {}
}
interface I
{
    public function im(int $a): void;
}
abstract class P implements I
{
    /** doc */
    #[Tag('m', list: [1, 'k' => 'v'])]
    final public static function f(int $x = 1, ?string $y = null, self ...$rest): ?string { return null; }
    abstract protected function g();
    public function im(int $a): void {}
    private function &h(array $a = [], $u = 'text'): array|bool { return $a; }
    public function __construct(private int $promoted = 3) {}
    public function __destruct() {}
}
class C extends P
{
    protected function g() {}
}
function plain($a, $b = 2) {}
function &byref(): int { static $x = 1; return $x; }
$cl = function (int $z) use (&$cl): void {};

foreach (['f', 'g', 'im', 'h', '__construct', '__destruct'] as $m) {
    echo (string) new ReflectionMethod('C', $m), "----\n";
}
echo (string) new ReflectionMethod('I', 'im'), "----\n";
echo (string) new ReflectionFunction('plain'), "----\n";
echo (string) new ReflectionFunction('byref'), "----\n";
echo (string) new ReflectionFunction($cl), "----\n";
foreach ((new ReflectionMethod('C', 'f'))->getAttributes() as $a) {
    echo (string) $a, "----\n";
}

#[Tag]
class Q
{
    public int $p = 1;
    public static $s = 2;
    public readonly int $ro;
    protected ?array $a = null;
    private $u;
    public private(set) int $ps = 1;
    const C = 'x';
    final protected const D = 2;
    public const array E = [1];
}
$q = new Q();
foreach (['p', 's', 'ro', 'a', 'u', 'ps'] as $n) {
    echo (string) new ReflectionProperty('Q', $n), "|\n";
}
echo (string) new ReflectionProperty('P', 'promoted'), "|\n";
echo (string) (new ReflectionClass('Q'))->getAttributes()[0], "|\n";
foreach (['C', 'D', 'E'] as $k) {
    echo (string) new ReflectionClassConstant('Q', $k), "|\n";
}
enum Suit: string
{
    case Hearts = 'h';
}
echo (string) new ReflectionEnumBackedCase('Suit', 'Hearts'), "|\n";
echo (string) (new ReflectionMethod('C', 'f'))->getReturnType(), "\n";
var_dump((new ReflectionFunction('byref'))->returnsReference(),
    (new ReflectionMethod('C', 'h'))->returnsReference(),
    (new ReflectionFunction('plain'))->returnsReference());
