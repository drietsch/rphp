<?php
// finally: normal / throw / return / break / continue paths, return in
// finally overriding a pending exception or return, nested finally bodies,
// an exception thrown inside finally chaining the pending one as previous.

function normal() { try { echo "try "; } finally { echo "finally "; } echo "after\n"; }
normal();

function ret_in_try() { try { return "value"; } finally { echo "cleanup "; } }
echo ret_in_try(), "\n";

function ret_override() { try { return "try"; } finally { return "finally"; } }
echo ret_override(), "\n";

function throw_override() { try { throw new Exception("lost"); } finally { return "finally wins"; } }
echo throw_override(), "\n";

function pending_throw() { try { throw new Exception("kept"); } finally { echo "runs "; } }
try { pending_throw(); } catch (Exception $e) { echo "caught ", $e->getMessage(), "\n"; }

function nested() {
    try {
        try { return "inner"; } finally { echo "f1 "; }
    } finally {
        echo "f2 ";
    }
}
echo nested(), "\n";

function nested_throw() {
    try {
        try { throw new Exception("deep"); } finally { echo "f1 "; }
    } catch (Exception $e) {
        echo "mid ", $e->getMessage(), " ";
        return "from catch";
    } finally {
        echo "f2 ";
    }
}
echo nested_throw(), "\n";

for ($i = 0; $i < 4; $i++) {
    try {
        if ($i == 1) continue;
        if ($i == 3) break;
        echo "body$i ";
    } finally {
        echo "fin$i ";
    }
}
echo "\n";

$n = 0;
while (true) {
    try {
        try {
            $n++;
            if ($n < 3) continue;
            break;
        } finally {
            echo "inner$n ";
        }
    } finally {
        echo "outer$n ";
    }
}
echo "\n";

try {
    try { throw new Exception("a"); } finally { throw new Exception("b"); }
} catch (Exception $e) {
    echo $e->getMessage(), " previous=", $e->getPrevious()->getMessage(), "\n";
}

try {
    try { throw new Exception("e1"); }
    catch (Exception $e) { echo "caught ", $e->getMessage(), " "; throw new Exception("e2"); }
    finally { echo "finally "; }
} catch (Exception $e) {
    echo "outer ", $e->getMessage(), "\n";
}

function deep_return() {
    foreach ([1, 2] as $x) {
        try {
            try { if ($x == 2) return "x=$x"; } finally { echo "in$x "; }
        } finally {
            echo "out$x ";
        }
    }
    return "none";
}
echo deep_return(), "\n";

function catch_then_finally_value() {
    try { throw new LogicException("l"); } catch (LogicException $e) { return "c"; } finally { echo "f "; }
}
echo catch_then_finally_value(), "\n";
echo "end\n";
