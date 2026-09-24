<?php
// Tier-A differential: net_get_interfaces() — the shape of every entry
// (addresses are the machine's own), and the loopback interface in full.

$ifs = net_get_interfaces();
var_dump(is_array($ifs), count($ifs) > 0);
foreach ($ifs as $name => $if) {
    $shape = [];
    foreach ($if["unicast"] as $u) {
        $shape[] = implode(",", array_keys($u));
    }
    echo is_string($name) ? "name" : "?", " up=", gettype($if["up"]), " keys=", implode(",", array_keys($if)),
        " [", implode(" | ", array_unique($shape)), "]\n";
}
$lo = $ifs["lo0"] ?? $ifs["lo"] ?? null;
if ($lo !== null) {
    var_dump($lo["up"]);
    foreach ($lo["unicast"] as $u) {
        if (isset($u["address"])) {
            echo $u["family"], " ", $u["address"], " ", $u["netmask"] ?? "-", "\n";
        }
    }
}
try {
    net_get_interfaces(1);
} catch (\ArgumentCountError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
