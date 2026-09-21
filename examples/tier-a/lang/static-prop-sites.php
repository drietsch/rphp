<?php
// Static properties through one site: visibility, typed coercion, a bound
// reference, inherited vs redeclared storage, late static binding, an
// uninitialized typed property read, passed by value and by reference.
class A { public static $pub = 1; protected static $prot = 'p'; private static $priv = [1]; public static int $typed = 0; public static ?string $ns = null;
  static function bump() { self::$prot .= 'x'; self::$priv[] = 2; static::$pub++; self::$typed += 1; return [self::$prot, self::$priv, self::$typed]; }
  static function readPriv() { return self::$priv; } }
class B extends A { public static $pub = 100; static function bumpB() { self::$pub++; parent::$pub++; return [self::$pub, parent::$pub, static::$pub]; } }
for ($i = 0; $i < 3; $i++) { var_dump(A::bump()); }
var_dump(A::$pub, B::$pub, B::bumpB(), B::bumpB(), A::readPriv());
try { A::$prot = 1; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { A::$typed = "x"; } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
A::$typed = "5"; var_dump(A::$typed); A::$typed = 2.0; var_dump(A::$typed);
A::$ns = 'a'; A::$ns = null; var_dump(A::$ns);
$r = &A::$pub; $r = 50; var_dump(A::$pub); A::$pub = 7; var_dump($r);
class C { public static array $items = []; static function add($x) { self::$items[] = $x; return count(self::$items); } }
for ($i = 0; $i < 5; $i++) { C::add($i); } var_dump(C::$items);
class D { public static $late; } class E extends D {} E::$late = 'e'; var_dump(D::$late, E::$late);
try { var_dump(A::$nope); } catch (Error $e) { echo $e->getMessage(), "\n"; }
class U { public static int $u; } try { var_dump(U::$u); } catch (Error $e) { echo $e->getMessage(), "\n"; } U::$u = 3; var_dump(U::$u);
class U2 { public static int $u; public static $a = [1]; }
try { var_dump(U2::$u); } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { sort(U2::$u); } catch (Error $e) { echo $e->getMessage(), "\n"; }
sort(U2::$a); array_push(U2::$a, 2); var_dump(U2::$a, count(U2::$a), max(U2::$a));
function byref(&$x) { $x[] = 'r'; } byref(U2::$a); var_dump(U2::$a);
