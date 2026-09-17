<?php
// UnhandledMatchError carries php's rendering of the unmatched value.
function pick($v) {
    return match ($v) { 1 => 'one', 'a' => 'A' };
}
echo pick(1), pick('a'), "\n";
echo pick('zebra');
