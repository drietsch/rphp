<?php
#[\AllowDynamicProperties, Custom(\Attribute::TARGET_CLASS)]
abstract class C extends P implements I, J
{
    use T1, T2 { T1::m insteadof T2; T2::m as protected n; m as private; T1::x as final; }
    final public const int X = 1, Y = 2;
    protected static ?int $s = null;
    public $a, $b = 1;
    var $legacy;
    public int $p { get { return 1; } final set(int $v) => $v; }
    public string $q { &get => $this->q; set { $this->q = $value; } }
    public function __construct(public readonly string $n, protected(set) int $m, private(set) int $h { get => 1; }) {}
    abstract protected function f(): static;
    public static function g(): self {}
    function &h() {}
}
abstract class D { abstract function f(); }
interface I extends J, K { const X = 1; public function f(); public string $p { get; set; } }
trait T { abstract public function g(); public static $x; }
enum E { case A; case B; const X = self::A; }
enum F: string implements I { case X = "x"; #[A] case Y = "y"; public function f() {} }
readonly class R { public function __construct(public int $x, private(set) string $y) {} }
$o = new class { }; $p = new class(1) extends P implements I { public $x; }; $q = new #[A] readonly class {};
