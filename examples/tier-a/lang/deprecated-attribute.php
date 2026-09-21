<?php
// php 8.4's #[\Deprecated] on functions, methods and class constants: the
// notice on every call or fetch, at the call site, with `since` and the
// message; a class-constant site caches its value but not the notice.
#[\Deprecated] function f() { return 1; }
#[\Deprecated("no more", "2.0")] function g() { return 2; }
#[\Deprecated(since: "1.5")] function h() { return 3; }
class K {
    #[\Deprecated(since: "3.0")] const C = 1;
    #[\Deprecated("use D")] const OLD = 9;
    const D = 2;
    #[\Deprecated("gone")] static function m() { return 'm'; }
    #[\Deprecated(message: "m2", since: "4")] function i() { return 'i'; }
    function fine() { return self::OLD + self::D; }
}
var_dump(f(), g(), h(), K::C, K::OLD, K::D, K::m(), (new K)->i(), (new K)->fine(), (new K)->fine());
for ($i = 0; $i < 2; $i++) { var_dump(K::C + K::D); }
$c = #[\Deprecated("closures too")] function () { return 'c'; }; var_dump($c(), $c());
set_error_handler(function ($no, $msg) { echo "handled: $msg\n"; return true; });
var_dump(f(), K::OLD);
restore_error_handler();
set_error_handler(function ($no, $msg) { throw new LogicException($msg); });
try { g(); } catch (LogicException $e) { echo "thrown: ", $e->getMessage(), "\n"; }
try { K::OLD; } catch (LogicException $e) { echo "thrown: ", $e->getMessage(), "\n"; }
