<?php
// Tier-A differential: urlencode / rawurlencode (RFC 1738 `+` and `%7E` vs
// RFC 3986 `%20` and `~`) and their decoders, including invalid or truncated
// percent escapes that are passed through untouched.

$samples = [
    "a b+c&d=e/f?g~h.i-j_k!*'()",
    "\x00\xff\"#%<>[]\\^`{|}",
    "café 日本",
    "already%20encoded+plus",
    "",
    "-._~",
    "A1 z9",
];
foreach ($samples as $s) {
    echo urlencode($s), "\n";
    echo rawurlencode($s), "\n";
}

$encoded = [
    "a+b%20c%2Fd%zz%2",
    "%",
    "%2",
    "%2G",
    "%%41",
    "%41%42%43+%2b",
    "%e2%82%ac",
    "plus+sign%2Bencoded",
    "%00null",
];
foreach ($encoded as $s) {
    var_dump(urldecode($s));
    var_dump(rawurldecode($s));
}

// Round trips.
$raw = "x=1&y=a b/c?d";
var_dump(urldecode(urlencode($raw)) === $raw, rawurldecode(rawurlencode($raw)) === $raw);
