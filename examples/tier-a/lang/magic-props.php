<?php
// Tier-A: php's magic property accessors — __get / __set / __isset / __unset,
// when they are consulted, and the per-(object, property, accessor) recursion
// guard that makes the `private $data` + __get pattern terminate.
// Differentially tested against stock PHP 8.5.

class Bag {
    private $d = [];

    public function __get($k) {
        echo "get($k)";
        return isset($this->d[$k]) ? $this->d[$k] : null;
    }

    public function __set($k, $v) {
        echo "set($k)";
        $this->d[$k] = $v;
    }

    public function __isset($k) {
        echo "isset($k)";
        return isset($this->d[$k]);
    }

    public function __unset($k) {
        echo "unset($k)";
        unset($this->d[$k]);
    }
}

// An undeclared name goes to the accessors; the guard lets the bodies touch
// $this->d (a *different* property) without recursing.
$b = new Bag();
$b->a = 1;
echo $b->a, "\n";                      // set(a)get(a)1
var_dump(isset($b->a));                // isset(a)bool(true)
unset($b->a);
var_dump(isset($b->a));                // unset(a)isset(a)bool(false)
var_dump(empty($b->a));                // isset(a)bool(true)
$b->a = 0;
var_dump(empty($b->a));                // set(a)isset(a)get(a)bool(true)
var_dump($b->a);                       // get(a)int(0)
echo "\n";

// A declared, visible property never reaches the accessors — not even when it
// holds null.
class Plain {
    public $v = 'V';
    public $n = null;

    public function __get($k) { echo "[G:$k]"; return 'g'; }
    public function __set($k, $x) { echo "[S:$k]"; }
    public function __isset($k) { echo "[I:$k]"; return true; }
    public function __unset($k) { echo "[U:$k]"; }
}

$p = new Plain();
echo $p->v, "\n";                      // V
$p->v = 'W';
echo $p->v, "\n";                      // W
var_dump(isset($p->n));                // bool(false)  — no __isset
var_dump(isset($p->v));                // bool(true)

// unset() of a visible declared property is direct; afterwards the slot is
// empty, so every further access *is* magic.
unset($p->v);
var_dump(isset($p->v));                // [I:v]bool(true)
echo $p->v, "\n";                      // [G:v]g
$p->v = 'X';                           // [S:v]
echo "\n";
var_dump($p->v);                       // [G:v]string(1) "g"

// A property the calling scope cannot see is the accessors' business too.
class Hidden {
    private $secret = 's';
    public $open = 'o';

    public function __get($k) { echo "[G:$k]"; return "via-get"; }
    public function __set($k, $v) { echo "[S:$k=$v]"; }
    public function __isset($k) { echo "[I:$k]"; return $k === 'secret'; }
    public function __unset($k) { echo "[U:$k]"; }

    public function inside() {
        // Inside the class the property is plainly visible.
        return $this->secret;
    }
}

$h = new Hidden();
echo $h->inside(), "\n";               // s
echo $h->secret, "\n";                 // [G:secret]via-get
$h->secret = 'z';
echo "\n";
var_dump(isset($h->secret));           // [I:secret]bool(true)
var_dump(isset($h->missing));          // [I:missing]bool(false)
unset($h->secret);
echo "\n";
echo $h->inside(), "\n";               // s — __unset never touched the slot

// The four guards are independent: inside __get the *write* of the same
// property still reaches __set, and inside that __set the read of the same
// property falls through to the plain (undefined) behaviour.
class Cross {
    public function __get($k) {
        echo "[G:$k]";
        $this->$k = 'w';
        return "g$k";
    }

    public function __set($k, $v) {
        echo "[S:$k=$v]";
        $x = @$this->$k;
        echo '(read:', var_export($x, true), ')';
    }
}

$c = new Cross();
echo $c->foo, "\n";                    // [G:foo][S:foo=w](read:NULL)gfoo

// An inherited *private* property is not merely invisible, it is nameless:
// php reports it undefined and routes the access to the accessors.
class Base {
    private $bp = 'bp';
    public function fromBase($o) { return $o->bp; }
}

class Kid extends Base {
    private $kp = 'kp';
    public function fromKid($o) { return $o->kp; }
}

$k = new Kid();
echo $k->fromBase($k), "\n";           // bp — Base can see its own private
echo $k->fromKid($k), "\n";            // kp
var_dump($k->bp);                      // Warning: Undefined property: Kid::$bp
try {
    echo $k->kp, "\n";
} catch (Error $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}

// Without accessors: the undefined-property warning, the "Cannot access"
// Error, and the php 8.2 dynamic-property deprecation.
class Strict {
    public $a = 1;
    private $b = 2;
    protected $c = 3;
}

$s = new Strict();
var_dump($s->nope);
try { echo $s->b; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { echo $s->c; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { $s->b = 1; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { unset($s->b); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$s->fresh = 9;                         // Deprecated: dynamic property
echo $s->fresh, "\n";                  // 9
var_dump(isset($s->b), empty($s->b));  // no accessors: false, true

$o = new stdClass();                   // stdClass is exempt
$o->any = 'ok';
echo $o->any, "\n";

// Reading a property off a non-object warns; writing one is an Error.
$nothing = null;
var_dump($nothing->x);
try { $nothing->x = 1; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$str = "s";
try { $str->x = 1; } catch (Error $e) { echo $e->getMessage(), "\n"; }
