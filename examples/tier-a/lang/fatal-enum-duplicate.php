<?php
// php builds a backed enum's lookup table the first time anything touches a
// case — which is where it notices two cases sharing a value, not at the
// declaration (note the line the error names).
enum Dup: int {
    case A = 1;
    case B = 2;
    case C = 1 << 0;
}
echo "declared\n";
var_dump(Dup::B);
