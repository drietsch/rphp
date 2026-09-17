<?php
// An uncaught exception with a previous chain renders every link with
// `Next`, innermost first, and a user __toString is used when defined.
class Wrapped extends Exception {}
function fail() { throw new LogicException("root cause"); }
try { fail(); } catch (LogicException $e) { throw new Wrapped("wrapper", 0, $e); }
