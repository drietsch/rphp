<?php
$s = 'abc'; $t = $s; $s[1] = 'X'; var_dump($s, $t); $s[5] = 'Y'; var_dump($s); $s[-1] = 'Z'; var_dump($s);
$r = &$s; $s[0] = 'Q'; var_dump($r); $u = $s; $s[0] = 'W'; var_dump($u, $s);
$a = ['k' => 'hello']; $b = $a; $a['k'][0] = 'J'; var_dump($a, $b);
try { $s[-10] = 'x'; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$s[1] = 'longer'; var_dump($s);
class C { public $p = 'prop'; } $c = new C; $c->p[0] = 'P'; var_dump($c->p);
