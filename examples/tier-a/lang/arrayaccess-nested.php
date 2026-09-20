<?php
// Nested writes through `ArrayAccess` (`$o[$k][] = 1`, `$o[$k]++`,
// `$o[$k] .= "x"`, `$o[$k] ??= 5`): php reaches real storage on
// `ArrayObject`/`ArrayIterator`/`WeakMap`, modifies a temporary (with its
// "Indirect modification" notice, and no `offsetSet`) for any other
// class, and follows a by-reference `offsetGet()`; a compound assignment
// is `offsetGet` + `offsetSet` for every class.
class U implements ArrayAccess { public $d = []; function offsetExists($o): bool { echo "exists($o) "; return isset($this->d[$o]); } function offsetGet($o): mixed { echo "get($o) "; return $this->d[$o] ?? null; } function offsetSet($o, $v): void { echo "set($o) "; if ($o === null) $this->d[] = $v; else $this->d[$o] = $v; } function offsetUnset($o): void { echo "unset($o) "; unset($this->d[$o]); } }
class R implements ArrayAccess { public $d = []; function offsetExists($o): bool { return isset($this->d[$o]); } function &offsetGet($o): mixed { echo "get&($o) "; if (!isset($this->d[$o])) $this->d[$o] = null; return $this->d[$o]; } function offsetSet($o, $v): void { echo "set($o) "; if ($o === null) $this->d[] = $v; else $this->d[$o] = $v; } function offsetUnset($o): void { unset($this->d[$o]); } }
$k = new stdClass;
$w = new WeakMap; $w[$k] = []; $w[$k][] = 1; $w[$k]['x'] = 2; var_dump($w[$k]);
$w2 = new WeakMap; try { $w2[$k][] = 1; } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } var_dump(count($w2));
$ao = new ArrayObject(['a' => []]); $ao['a'][] = 1; $ao['b']['c'] = 2; var_dump($ao->getArrayCopy());
$ai = new ArrayIterator(['a' => []]); $ai['a'][] = 1; var_dump($ai->getArrayCopy());
$s = new SplObjectStorage; $s[$k] = []; $s[$k][] = 1; var_dump($s[$k]);
$f = new SplFixedArray(2); $f[0] = []; $f[0][] = 1; var_dump($f[0]);
$u = new U; $u['a'] = []; $u['a'][] = 1; echo "\n"; var_dump($u->d); $u['b']['c'] = 2; echo "\n"; var_dump($u->d);
$r = new R; $r['a'][] = 1; $r['b']['c'] = 2; echo "\n"; var_dump($r->d);
$u2 = new U; $u2['q'] .= 'x'; echo "\n"; var_dump($u2->d); $u2['n']++; echo "\n"; var_dump($u2->d); $u2['m'] ??= 5; echo "\n"; var_dump($u2->d);
$ao2 = new ArrayObject(); $ao2['q'] .= 'x'; $ao2['n']++; $ao2['m'] ??= 5; var_dump($ao2->getArrayCopy());
