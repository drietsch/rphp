<?php
// `escapeshellarg()` makes one shell word; `escapeshellcmd()` only escapes a
// command line's metacharacters, leaves a *paired* quote alone, and drops a
// 0xFF byte outright.
foreach (['a b', "a'b", '', "x\ny", 'plain'] as $s) {
    var_dump(escapeshellarg($s));
}
foreach ([
    'a&b|c;d',
    'x"y',
    "a'b",
    '"paired"',
    "'q'",
    'a`b',
    "x\nz",
    '*?~<>^()[]{}$',
    '%x%',
    '"a&b"',
    '"a',
    'a"b"c"',
    "a\xffb",
] as $s) {
    var_dump(bin2hex($s), bin2hex(escapeshellcmd($s)));
}
