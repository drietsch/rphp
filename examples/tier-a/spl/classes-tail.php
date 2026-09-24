<?php
// spl_classes(): every class and interface ext/spl declares, name => name.
$c = spl_classes();
var_dump(count($c));
print_r($c);
foreach ($c as $k => $v) {
    if ($k !== $v) {
        echo "odd: $k\n";
    }
}
