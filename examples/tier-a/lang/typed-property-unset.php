<?php
// A typed property that was unset() is not "uninitialized": php hands its
// reads, writes, isset() and unset() to the magic methods (with the read's
// result checked against the declared type), where a never-initialized one
// bypasses them and errors on read. Symfony's Constraint::$groups lazy
// initialization relies on this.
class C {
    public ?array $g = null;
    public array $never;
    public $untyped = 1;
    function __construct() { unset($this->g); unset($this->untyped); }
    function __get($n) { echo "__get($n)\n"; if ($n === 'g') { $this->g = ['lazy']; return $this->g; } return "magic $n"; }
    function __set($n, $v) { echo "__set($n)\n"; if ($n === 'g') { $this->g = (array) $v; } }
    function __isset($n) { echo "__isset($n)\n"; return $n === 'g'; }
    function __unset($n) { echo "__unset($n)\n"; }
}
$c = new C;
var_dump(isset($c->never), isset($c->g), isset($c->untyped));
try { var_dump($c->never); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump($c->g);
var_dump($c->g);
var_dump($c->untyped);
try { $c->g = 'x'; } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump($c->g);
unset($c->g); var_dump(isset($c->g)); $c->g = ['direct?']; var_dump($c->g);
unset($c->untyped); unset($c->untyped);
$d = clone $c; unset($d->g); var_dump($d->g);
var_dump((array) $c);
var_dump($c);
class E { public int $x; }
$e = new E; var_dump(isset($e->x)); unset($e->x); var_dump(isset($e->x)); try { $e->x; } catch (Error $ex) { echo $ex->getMessage(), "\n"; } $e->x = 3; var_dump($e->x);
unset($c->never); var_dump(isset($c->never));
try { var_dump($c->never); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$c->never = [1];
try { var_dump($c->never); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
