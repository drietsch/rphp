<?php
// A `*` precision of -1 (shortest round-trip) only exists for the %g family.
echo sprintf("%.*d", -1, 7);
