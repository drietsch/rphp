<?php
class B { public $buf = ''; private string $p = 'p'; protected ?string $n = null; public int $i = 1; public readonly string $ro;
  function __construct() { $this->ro = 'r'; }
  function app($s) { $this->p .= $s; $this->n .= $s; return [$this->p, $this->n]; } }
$b = new B; $b->buf .= 'x'; $b->buf .= 5; $b->buf .= 1.5; var_dump($b->buf, $b->app('q'), $b->app('r'));
try { $b->p .= 'z'; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { $b->ro .= 'z'; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$b->i .= '2'; var_dump($b->i);
$c = new B; $r = &$c->buf; $c->buf .= 'a'; $c->buf .= 'b'; var_dump($r, $c->buf);
$d = clone $c; $d->buf .= 'c'; var_dump($c->buf, $d->buf);
class M { private $data = []; function __get($n) { return $this->data[$n] ?? ''; } function __set($n, $v) { echo "set $n\n"; $this->data[$n] = $v; } }
$m = new M; $m->x .= 'a'; $m->x .= 'b'; var_dump($m->x);
$u = new B; unset($u->buf); $u->buf .= 'k'; var_dump($u->buf);
