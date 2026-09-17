<?php
// set_exception_handler receives the uncaught object; the exit code is 255
// even when the handler returns normally; shutdown functions and
// destructors still run afterwards.
class D { function __destruct() { echo "destructed\n"; } }
$d = new D;
register_shutdown_function(function () { echo "shutdown\n"; });
$prev = set_exception_handler(function (Throwable $t) {
    echo "handler: ", get_class($t), " ", $t->getMessage(), " @", $t->getLine(), "\n";
});
var_dump($prev);
echo "before\n";
throw new DomainException("unhandled");
echo "never\n";
