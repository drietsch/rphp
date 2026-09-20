<?php
class C { public array $p = []; private array $q = []; public $o;
  function run() { $t = microtime(true); for ($i = 0; $i < 20000; $i++) { $this->p[] = $i; } echo "this->p[]: ", round((microtime(true)-$t)*1000), "ms\n";
    $t = microtime(true); for ($i = 0; $i < 20000; $i++) { $this->q[$i] = $i; } echo "this->q[k]: ", round((microtime(true)-$t)*1000), "ms\n";
    $t = microtime(true); $this->o = new stdClass; $this->o->arr = []; for ($i = 0; $i < 20000; $i++) { $this->o->arr[$i] = $i; } echo "this->o->arr[k]: ", round((microtime(true)-$t)*1000), "ms\n";
  }
}
$t = microtime(true); $a = []; for ($i = 0; $i < 20000; $i++) { $a[] = $i; } echo "a[]: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $m = []; for ($i = 0; $i < 20000; $i++) { $m['k'][] = $i; } echo "m[k][]: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $n = ['a' => ['b' => []]]; for ($i = 0; $i < 20000; $i++) { $n['a']['b'][$i] = $i; } echo "n[a][b][k]: ", round((microtime(true)-$t)*1000), "ms\n";
(new C)->run();
$t = microtime(true); $s = []; for ($i = 0; $i < 20000; $i++) { $s[$i] = $i; $x = $s; } echo "shared copy: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $st = new SplObjectStorage; $arr = ['x' => 1]; for ($i = 0; $i < 20000; $i++) { $arr['x'] = $i; $y = count($arr); } echo "count(): ", round((microtime(true)-$t)*1000), "ms\n";
function f(array $a) { return $a; }
$t = microtime(true); $g = []; for ($i = 0; $i < 20000; $i++) { $g = f($g); $g[] = $i; } echo "pass/return: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $h = []; for ($i = 0; $i < 20000; $i++) { $h[] = $i; $z = $h[0]; } echo "read+append: ", round((microtime(true)-$t)*1000), "ms\n";
$t = microtime(true); $w = []; for ($i = 0; $i < 20000; $i++) { $w[$i] = $i; foreach ($w as $v) { break; } } echo "foreach+append: ", round((microtime(true)-$t)*1000), "ms\n";
