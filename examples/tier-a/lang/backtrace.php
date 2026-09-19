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
