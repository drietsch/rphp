<?php
// `debug_backtrace()` walks the same frames an exception's trace does, from
// the caller of the call outwards — the `debug_backtrace()` frame itself is
// not part of its own answer.
class Holder
{
    public function method(int $x): array { return self::stat($x); }
    public static function stat(int $x): array { return plain($x); }
}

function plain(int $x): array
{
    $bt = debug_backtrace();
    $out = [];
    foreach ($bt as $i => $f) {
        $out[$i] = [
            'keys' => array_keys($f),
            'function' => $f['function'],
            'class' => $f['class'] ?? null,
            'type' => $f['type'] ?? null,
            'args' => $f['args'] ?? null,
            'object' => isset($f['object']) ? get_class($f['object']) : null,
        ];
    }
    return $out;
}

print_r((new Holder())->method(7));

// The two flags, and the limit.
function flags(): void
{
    print_r(array_keys(debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS)[0]));
    print_r(array_keys(debug_backtrace(0)[0]));
    var_dump(count(debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS, 1)));
    var_dump(count(debug_backtrace()) > 0);
}
flags();

// At top level there is nothing above the call.
var_dump(debug_backtrace());

// `debug_print_backtrace()` prints the same walk and, unlike a trace string,
// no closing `{main}` line.
function printed(): void { debug_print_backtrace(); echo "[end]\n"; }
printed();
$closure = function (): void { debug_print_backtrace(); echo "[end]\n"; };
$closure();
debug_print_backtrace();
echo "[top]\n";

// Trace arguments: strings are cut to 15 bytes, then escaped C-style.
function traceArg($s) { throw new Exception("x"); }
foreach (["a\\b\nc\x01'q\"z", "0123456789abcdef\\xyz", "012345678901234\\", "ü\t\r\0é\x7f\x1b\f\v"] as $s) {
    try { traceArg($s); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
}

// A closure's frame names the class it is scoped to; a call in a fluent
// chain is filed under the line of the method name.
class Scoped {
    static function s() { $c = function () { throw new Exception("x"); }; $c(); }
    function i() { $c = function () { throw new Exception("y"); }; $c(); }
    static function st() { $c = static function () { throw new Exception("z"); }; $c(); }
    function chain() { return $this; }
    function boom() { throw new Exception("chain"); }
}
foreach (['s', 'st'] as $m) { try { Scoped::$m(); } catch (Exception $e) { $t = $e->getTrace()[0]; var_dump($t['class'] ?? null, $t['type'] ?? null, $t['function']); } }
try { (new Scoped)->i(); } catch (Exception $e) { $t = $e->getTrace()[0]; var_dump($t['class'] ?? null, $t['type'] ?? null); }
try {
    (new Scoped)
        ->chain()
        ->boom();
} catch (Exception $e) { var_dump($e->getLine(), $e->getTrace()[0]['line']); }
try {
    $x = (new Scoped)
        ->chain()
        ->boom(
            1,
        );
} catch (Exception $e) { var_dump($e->getTrace()[0]['line']); }
try {
    Scoped
        ::s();
} catch (Exception $e) { var_dump($e->getTrace()[1]['line']); }

// A trait's method is filed under the class using it.
trait Tr { function tm() { throw new Exception("t"); } }
class UsesTr { use Tr; }
try { (new UsesTr)->tm(); } catch (Exception $e) { var_dump($e->getTrace()[0]['class'], $e->getTraceAsString()); }
