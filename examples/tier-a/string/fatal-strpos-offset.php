<?php
// A start offset outside the haystack is a ValueError (PHP 8).
var_dump(strpos("abc", "a", -3));
var_dump(strpos("abc", "a", 5));
