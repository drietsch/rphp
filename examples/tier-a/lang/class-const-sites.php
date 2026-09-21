<?php
// Class constants through one site: self/parent/static resolution, private
// and protected visibility, enum cases as constants, a deprecated constant
// noticed on every fetch, a lazily evaluated initializer, `::class`.
class A { const X = 1; const Y = self::X + 1; protected const P = 'p'; private const Q = 'q'; #[\Deprecated("use Y")] const OLD = 9;
  static function all() { return [self::X, self::Y, self::P, self::Q, static::X]; } }
class B extends A { const X = 10; static function mine() { return [self::X, parent::X, static::X, self::P]; } }
enum E: string { case One = 'one'; const ALIAS = self::One; }
for ($i = 0; $i < 3; $i++) { var_dump(A::all(), B::mine(), B::all()); }
var_dump(A::X, B::X, E::One, E::ALIAS, E::One === E::ALIAS, A::class, B::Y);
try { var_dump(A::P); } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { var_dump(B::Q); } catch (Error $e) { echo $e->getMessage(), "\n"; }
var_dump(A::OLD, A::OLD);
$c = 'A'; var_dump($c::X, constant('A::Y'));
class Lazy { const L = Other::V * 2; } class Other { const V = 21; } var_dump(Lazy::L, Lazy::L);
