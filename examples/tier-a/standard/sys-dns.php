<?php
// Tier-A differential: the DNS functions' argument checks and the paths
// that answer without asking a name server (offline-safe: no case here
// sends a query).

foreach (["A", "NS", "CNAME", "SOA", "PTR", "HINFO", "CAA", "MX", "TXT", "SRV", "NAPTR", "AAAA", "A6", "ANY", "ALL"] as $t) {
    echo "DNS_$t=", constant("DNS_$t"), "\n";
}

$calls = [
    fn() => checkdnsrr("", "MX"),
    fn() => checkdnsrr("example.com", "BOGUS"),
    fn() => checkdnsrr("example.com", ""),
    fn() => dns_check_record("", "A"),
    fn() => dns_check_record("example.com", "bogus"),
    fn() => dns_get_record("example.com", 12345),
    fn() => dns_get_record("example.com", DNS_ALL | DNS_ANY),
    fn() => dns_get_record("example.com", -1),
    fn() => dns_get_record("example.com", 0, $a, $b, true),
    fn() => dns_get_record("example.com", 65536, $a, $b, true),
    fn() => dns_get_record("example.com", -5, $a, $b, true),
    // No type bit set: no query at all.
    fn() => dns_get_record("example.com", 0),
    // Names that cannot be encoded are "not found" before any query.
    fn() => dns_get_record("a..b", DNS_A),
    fn() => dns_get_record(str_repeat("a", 64) . ".example", DNS_MX),
    fn() => checkdnsrr("a..b"),
    fn() => checkdnsrr(str_repeat("x.", 130) . "com", "a"),
    fn() => dns_check_record("a..b", "txt"),
];
foreach ($calls as $i => $f) {
    try {
        echo "$i ";
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}

// The by-reference outputs are reset to arrays.
$ns = "old";
var_dump(dns_get_record("example.com", 0, $ns), $ns);
$hosts = "old";
$weights = "old";
var_dump(getmxrr("a..b", $hosts, $weights), $hosts, $weights);
$hosts = 1;
var_dump(dns_get_mx("a..b", $hosts), $hosts);

try {
    getmxrr("a", $h, $w, 1);
} catch (\Throwable $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
try {
    dns_get_record([]);
} catch (\Throwable $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
