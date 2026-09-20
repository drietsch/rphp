<?php
// Parameter defaults through Reflection: `isDefaultValueConstant()` and
// `getDefaultValueConstantName()` see the *expression* — a bare constant
// fetch by its unresolved spelling (`N\PHP_INT_MAX`, `self::X`), a class
// constant with the class resolved — while `getDefaultValue()` evaluates
// it; a native's arginfo (names, types, defaults, return type) comes from
// php's stubs; `__toString()` prints defaults as written and escapes
// strings like `smart_str_append_escaped` (a compound default such as
// `E_ALL | E_STRICT` prints its value here where php prints the
// expression; see COVERAGE.md).
namespace N {
const LOCAL = 3;
class C { const X = 1; const Y = self::X + 1; }
function f($a = 1, $b = PHP_INT_MAX, $c = \PHP_EOL, $d = C::X, $e = \N\C::Y, $f = LOCAL, $i = null, $j = M_PI, $k = C::class, $m = \E_ALL, $n = namespace\LOCAL, $o = ('a' . 'b')) {}
class D extends C { function m($p = self::X, $r = parent::X, $s = D::X) {} }
foreach ((new \ReflectionFunction('N\f'))->getParameters() as $p) {
    echo $p->getName(), ": ", var_export($p->isDefaultValueConstant(), true);
    if ($p->isDefaultValueConstant()) echo " ", $p->getDefaultValueConstantName();
    echo "\n";
}
foreach ((new \ReflectionMethod('N\D', 'm'))->getParameters() as $p) {
    echo $p->getName(), ": ", var_export($p->isDefaultValueConstant(), true);
    if ($p->isDefaultValueConstant()) echo " ", var_export($p->getDefaultValueConstantName(), true);
    echo "\n";
}
try { (new \ReflectionParameter('N\f', 'a'))->getDefaultValueConstantName(); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
function g($a) {}
try { (new \ReflectionParameter('N\g', 'a'))->isDefaultValueConstant(); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { (new \ReflectionParameter('N\g', 'a'))->getDefaultValueConstantName(); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
foreach ((new \ReflectionFunction('json_decode'))->getParameters() as $p) { echo $p->getName(), ": ", var_export($p->isOptional() ? $p->isDefaultValueConstant() : '-', true), "\n"; }
foreach ((new \ReflectionFunction('htmlspecialchars'))->getParameters() as $p) { if ($p->isOptional()) echo $p->getName(), ": ", var_export($p->isDefaultValueConstant(), true), " ", $p->isDefaultValueConstant() ? $p->getDefaultValueConstantName() : '', "\n"; }
echo (new \ReflectionFunction('N\f')), "\n";
}
namespace {
foreach (['count', 'json_decode', 'htmlspecialchars', 'array_keys', 'str_pad', 'preg_split', 'array_filter', 'iterator_to_array', 'session_start', 'array_map', 'sprintf', 'ArrayObject::__construct', 'DateTime::__construct', 'str_contains'] as $f) {
    $r = str_contains($f, '::') ? new ReflectionMethod($f) : new ReflectionFunction($f);
    foreach ($r->getParameters() as $p) {
        echo $f, " \$", $p->getName(), ": optional=", var_export($p->isOptional(), true), " avail=", var_export($p->isDefaultValueAvailable(), true);
        if ($p->isDefaultValueAvailable()) {
            echo " const=", var_export($p->isDefaultValueConstant(), true), " name=", var_export($p->getDefaultValueConstantName(), true), " value=", var_export($p->getDefaultValue(), true);
        }
        echo "\n";
    }
    echo $r, "\n";
}
function ff($a = "it's\\x", $b = ["k" => "v\\", 2 => [1]], $c = 1.0, $d = 1e100, $e = "multi
line\t\x01é", $g = [1, 2, [3]]) {} echo new ReflectionFunction("ff");
}
