<?php
// The Throwable hierarchy and try/catch: class tree, accessors, previous
// chains, multi-catch, catch by interface, an undeclared catch type never
// matching, user subclasses of native exceptions, rethrow.

function thrower($msg) { throw new RuntimeException($msg, 42); }

try {
    thrower("boom");
} catch (LogicException $e) {
    echo "not here\n";
} catch (RuntimeException $e) {
    echo get_class($e), ": ", $e->getMessage(), " code=", $e->getCode(), " line=", $e->getLine(), "\n";
    echo $e->getTraceAsString(), "\n";
    echo count($e->getTrace()), " ", $e->getTrace()[0]["function"], " ", $e->getTrace()[0]["args"][0], "\n";
}

// previous chains and __toString (relative paths keep the output portable).
$inner = new InvalidArgumentException("inner", 1);
$outer = new Exception("outer", 2, $inner);
echo get_class($outer->getPrevious()), " ", $outer->getPrevious()->getMessage(), "\n";
var_dump($inner->getPrevious());
echo str_replace(__FILE__, "FILE", (string) $outer), "\n";

// catch by interface / base class / multi-catch; an unknown class never matches.
try { throw new DivisionByZeroError("dbz"); } catch (NoSuchClass $e) { echo "no\n"; } catch (ArithmeticError $e) { echo "arith ", get_class($e), "\n"; }
try { throw new OutOfBoundsException("oob"); } catch (LogicException | RuntimeException $e) { echo "multi ", get_class($e), "\n"; }
try { throw new TypeError("te"); } catch (Throwable $t) { echo "throwable ", get_class($t), " ", $t instanceof Error ? "Error" : "Exception", "\n"; }
try { throw new Exception("plain"); } catch (Exception) { echo "caught without a variable\n"; }

// user subclasses inherit the native layout and methods.
class MyException extends RuntimeException {
    public $extra;
    public function __construct($message, $extra) {
        parent::__construct($message, 7);
        $this->extra = $extra;
    }
    public function describe() { return $this->getMessage() . "/" . $this->extra . "/" . $this->getCode(); }
}
try { throw new MyException("mine", "data"); } catch (Exception $e) {
    echo $e->describe(), " ", $e instanceof Throwable ? "T" : "-", " ", $e instanceof MyException ? "M" : "-", "\n";
    echo get_parent_class($e), " ", implode(",", class_parents($e)), "\n";
    echo implode(",", class_implements($e)), "\n";
}

// rethrow from a catch, nested try, exception across a native callback.
function rethrow() {
    try { thrower("first"); } catch (Exception $e) { throw new LogicException("second", 0, $e); }
}
try { rethrow(); } catch (LogicException $e) { echo $e->getMessage(), " <- ", $e->getPrevious()->getMessage(), "\n"; }
try {
    array_map(function ($x) { if ($x > 1) { throw new UnexpectedValueException("cb $x"); } return $x; }, [1, 2, 3]);
} catch (UnexpectedValueException $e) {
    echo "across native: ", $e->getMessage(), "\n";
}

// throwing non-objects / non-Throwables.
try { throw "str"; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { throw new stdClass; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { new Throwable(); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }

// ErrorException carries a severity and explicit file/line.
$ee = new ErrorException("warned", 0, E_WARNING, "some.php", 12);
echo get_class($ee), " ", $ee->getSeverity(), " ", $ee->getFile(), ":", $ee->getLine(), "\n";
echo "end\n";
