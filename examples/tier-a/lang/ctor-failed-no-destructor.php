<?php
// php never destructs an object whose construction did not complete: an exception while the arguments of `new` are evaluated, or thrown by the constructor itself (doctrine/dbal's Logging\Connection relies on it).
class Lg { public $x = 'ok'; }
class L {
    public function __construct(public $c, private readonly Lg $l) { echo "ctor\n"; }
    public function __destruct() { echo "destruct ", $this->l->x, "\n"; }
}
function fail() { throw new Exception("boom"); }
// 1. the exception is raised while the arguments are evaluated: no ctor, no dtor
try { new L(fail(), new Lg()); } catch (Exception $e) { echo "caught 1\n"; }
// 2. the constructor itself throws: no dtor either
class M { public function __construct() { throw new Exception("in ctor"); } public function __destruct() { echo "M destruct\n"; } }
try { new M(); } catch (Exception $e) { echo "caught 2\n"; }
// 3. a completed construction is destructed
$ok = new L(1, new Lg());
unset($ok);
echo "end\n";
