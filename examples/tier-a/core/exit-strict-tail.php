<?php
declare(strict_types=1);

// Under strict_types the construct's argument is checked like any call's.
foreach ([true, 1.0, null] as $v) {
    try {
        exit($v);
    } catch (\TypeError $e) {
        echo $e->getMessage(), "\n";
    }
}
die(7);
