<?php
// __debugInfo() returning a non-array is php's fatal error.
class C { function __debugInfo() { return 5; } } echo "before\n"; var_dump(new C); echo "unreached\n";
