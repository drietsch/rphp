<?php
// An unknown conversion is a ValueError (a missing argument would have been
// skipped unvalidated, so both arguments are supplied here).
echo sprintf("%s %v", "a", "b");
