<?php
// A literal passed to a by-reference parameter is an Error at call time.
function take(&$x) { $x = 1; }
take(5);
