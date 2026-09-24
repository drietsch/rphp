<?php
// Tier-A differential: the services and protocols databases
// (getservbyname/getservbyport/getprotobyname/getprotobynumber).

// Services: name or alias, protocol exact, case-sensitive.
var_dump(getservbyname("http", "tcp"), getservbyname("www", "tcp"), getservbyname("http", "udp"));
var_dump(getservbyname("ssh", "tcp"), getservbyname("ftp", "tcp"), getservbyname("smtp", "tcp"));
var_dump(getservbyname("HTTP", "tcp"));
var_dump(getservbyname("nope-service", "tcp"), getservbyname("", "tcp"));

// Ports: the official name, the port modulo 2^16.
var_dump(getservbyport(80, "tcp"), getservbyport(80, "udp"), getservbyport(22, "tcp"));
var_dump(getservbyport(443, "tcp"), getservbyport(65616, "tcp"));
var_dump(getservbyport(0, "tcp"), getservbyport(-1, "tcp"), getservbyport(80, "nope"));

// Protocols: name or alias, case-sensitive.
var_dump(getprotobyname("tcp"), getprotobyname("TCP"), getprotobyname("udp"), getprotobyname("icmp"));
var_dump(getprotobyname("ip"), getprotobyname("IPv6"), getprotobyname("nope"), getprotobyname(""));
var_dump(getprotobynumber(0), getprotobynumber(6), getprotobynumber(17), getprotobynumber(41));
var_dump(getprotobynumber(-1), getprotobynumber(262), getprotobynumber(1000));

// Type errors from the parameter parser.
try {
    getservbyport([], "tcp");
} catch (\TypeError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
try {
    getprotobynumber("x");
} catch (\TypeError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
