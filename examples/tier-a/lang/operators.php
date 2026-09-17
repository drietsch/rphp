<?php
// Operators: compound assignments, ++/-- (incl. string increment and the
// 8.3+ diagnostics), casts, bitwise / shift operators on ints and strings,
// xor, print, unary plus/minus, string offsets, numeric-string arithmetic.

// compound assignments on variables, elements and properties
$a = 5; $a += 2; $a -= 1; $a *= 3; $a /= 2; $a **= 2; $a %= 7; $a .= 'x';
var_dump($a);
$b = 6; $b &= 3; $b |= 8; $b ^= 1; $b <<= 2; $b >>= 1;
var_dump($b);
$c = null; $c ??= 4; $c ??= 5;
var_dump($c);
$arr = ['n' => 1, 's' => 'a'];
$arr['n'] += 10; $arr['s'] .= 'b'; $arr['m'] ??= 'new'; $arr['m'] ??= 'nope';
$o = new stdClass; $o->n = 1; $o->n *= 5; $o->s = 'p'; $o->s .= 'q'; $o->z ??= 'zz';
var_dump($arr, $o);

// increment / decrement: pre and post, on ints, floats, null, strings
$i = 5;
echo $i++, ' ', $i, ' ', ++$i, ' ', $i--, ' ', --$i, "\n";   // 5 6 7 7 5
$f = 1.5; $f++; $f--; $f--;
var_dump($f);
$n = null; $n++;
var_dump($n);
$s = 'a'; $s++; $t = 'Az'; $t++; $u = 'zz'; $u++; $v = 'a9'; $v++; $w = '5'; $w++; $x = '5.5'; $x--;
var_dump($s, $t, $u, $v, $w, $x);
$big = PHP_INT_MAX; $big++;
var_dump($big);
$e = ['k' => 1]; $e['k']++; ++$e['k']; $e['k']--;
$o->n++; --$o->n; $o->n++;
var_dump($e['k'], $o->n);

// casts
var_dump((int) '12abc', (int) 1.9, (int) '1e3', (float) '1.5x', (float) 'abc', (bool) '0', (bool) '0.0', (bool) [], (bool) [0]);
var_dump((string) 1.0, (string) true, (string) null, (string) 0.1, (array) 's', (array) null, (array) 1.5);
var_dump((object) ['a' => 1, 'b' => [2]], (object) 5, (array) (object) ['x' => 'y']);

// bitwise and shifts
var_dump(6 & 3, 6 | 3, 6 ^ 3, ~5, 1 << 3, -16 >> 2, 1 << 63, 1 << 64, 10 >> 65, -10 >> 65);
var_dump('ab' & 'c', 'ab' | 'c', 'ab' ^ 'c', ~'ab', '12' & 6, 7.9 & 3);

// logical xor / not / short-circuit with side effects
$side = 0;
function bump() { global $side; $side++; return true; }
var_dump(true xor false, true xor true, !0, !'a', false && bump(), true || bump(), false or bump(), $side);

// unary plus and minus, numeric strings, precedence
var_dump(+'3', -'3.5', +'0x1A', -0, 2 ** -1, 2 ** 3 ** 2, -2 ** 2, 7 % -3, -7 % 3, 10 / 4, '10' + '5.5', '3' . '4' * 2);
var_dump('12abc' + 1);
var_dump(1 + 1.0, PHP_INT_MAX + 1, 0.1 + 0.2 == 0.3, 1 <=> 2, 'b' <=> 'a', [1, 2] <=> [1, 3], null <=> false);

// print is an expression that returns 1
$r = print "printed\n";
var_dump($r);
echo print('x'), "\n";

// string offsets: read (negative too), write pads with spaces
$str = 'hello';
echo $str[0], $str[-1], $str[1], "\n";
$str[0] = 'J'; $str[7] = '!';
var_dump($str, strlen($str));
$str[1] = 'ABC';
var_dump($str);

// @ silences the undefined-key warning, error_reporting() reflects it inside
$q = [];
$v1 = @$q['nope'];
var_dump($v1, @error_reporting() === error_reporting());
echo "done\n";
