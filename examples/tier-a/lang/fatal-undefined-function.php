<?php
// Exercises the stderr/exit-code channel of the differential harness: calling
// an undefined function through a callable string is an uncaught Error, so
// both engines must exit with 255. Nothing is echoed before the fault on
// purpose — see divergences.toml for what is (temporarily) normalized away.
$f = 'nope';
$f();
