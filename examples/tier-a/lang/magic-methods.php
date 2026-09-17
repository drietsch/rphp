<?php
// Tier-A: the call-side magic methods — `__call`, `__callStatic`, `__invoke` —
// and the errors php raises when a class has none of them. Differentially
// tested against stock PHP 8.5.

class Proxy {
    public function __call($name, $args) {
        return "call:$name(" . implode(',', $args) . ")";
    }
    public static function __callStatic($name, $args) {
        return "static:$name(" . implode(',', $args) . ")";
    }
    public function __invoke($x) {
        return "invoke:$x";
    }
    private function hidden() {
        return "real-hidden";
    }
    public function reachHidden() {
        return $this->hidden();
    }
    public function visible($n) {
        return "real-visible:$n";
    }
}

$p = new Proxy();

// A missing method goes to `__call`, with the name spelled as written.
echo $p->missing(1, 2), "\n";       // call:missing(1,2)
echo $p->MiXeD(), "\n";             // call:MiXeD()
echo $p->missing(), "\n";           // call:missing()

// A method that exists but is not visible from here goes to `__call` too;
// from inside the class the real one runs.
echo $p->hidden(), "\n";            // call:hidden()
echo $p->reachHidden(), "\n";       // real-hidden
echo $p->visible(7), "\n";          // real-visible:7

// `__callStatic` for the static spelling.
echo Proxy::gone('x'), "\n";        // static:gone(x)
echo Proxy::GONE(), "\n";           // static:GONE()

// `__invoke` makes the object callable — directly and through the shared
// callable path every higher-order builtin uses.
echo $p(9), "\n";                   // invoke:9
echo implode(',', array_map($p, [1, 2])), "\n";   // invoke:1,invoke:2

// The magic methods are reached through the callable path as well.
echo call_user_func([$p, 'zap'], 1), "\n";        // call:zap(1)
echo call_user_func('Proxy::zip'), "\n";          // static:zip()
echo call_user_func(['Proxy', 'zop'], 2), "\n";   // static:zop(2)
echo call_user_func_array([$p, 'zup'], [3, 4]), "\n"; // call:zup(3,4)

// Inherited magic: the subclass gets both.
class SubProxy extends Proxy {}
$s = new SubProxy();
echo $s->deep(5), "\n";             // call:deep(5)
echo SubProxy::deeper(), "\n";      // static:deeper()

// A class with no magic method raises php's errors.
class Plain {
    private function priv() { return 'p'; }
    protected function prot() { return 'q'; }
    public function pub() { return 'pub'; }
}
$q = new Plain();
try {
    $q->nope();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    $q->priv();
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
try {
    $q->prot();
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
try {
    Plain::pub();
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
try {
    Plain::nothing();
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}

// A private method of an ancestor is not visible to the child's scope, so it
// falls through to `__call`; the ancestor's own scope still reaches it.
class Base {
    private function secret() { return 'Base::secret'; }
    public function fromBase() { return $this->secret(); }
    public function __call($n, $a) { return "base-call:$n"; }
}
class Derived extends Base {
    private function secret() { return 'Derived::secret'; }
    public function fromDerived() { return $this->secret(); }
}
$d = new Derived();
echo $d->fromBase(), "\n";          // Base::secret
echo $d->fromDerived(), "\n";       // Derived::secret
echo $d->secret(), "\n";            // base-call:secret

// `__call` receives the arguments by value.
class ByValue {
    public function __call($n, $a) { $a[0] = 'changed'; return $a[0]; }
}
$b = new ByValue();
$v = 'original';
echo $b->touch($v), "\n";           // changed
echo $v, "\n";                      // original
