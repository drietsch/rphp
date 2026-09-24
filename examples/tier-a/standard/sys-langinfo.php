<?php
// Tier-A differential: nl_langinfo() and its constants in the C locale,
// CODESET following LC_CTYPE, and the invalid-item warning.

foreach (["CODESET", "D_T_FMT", "D_FMT", "T_FMT", "T_FMT_AMPM", "AM_STR", "PM_STR",
          "ERA", "ERA_D_FMT", "ERA_D_T_FMT", "ERA_T_FMT", "ALT_DIGITS", "RADIXCHAR",
          "THOUSEP", "YESEXPR", "NOEXPR", "YESSTR", "NOSTR", "CRNCYSTR"] as $c) {
    echo $c, "=", constant($c), " ";
    var_dump(nl_langinfo(constant($c)));
}
for ($i = 1; $i <= 7; $i++) {
    echo nl_langinfo(constant("DAY_$i")), "/", nl_langinfo(constant("ABDAY_$i")), " ";
}
echo "\n";
for ($i = 1; $i <= 12; $i++) {
    echo nl_langinfo(constant("MON_$i")), "/", nl_langinfo(constant("ABMON_$i")), " ";
}
echo "\n";

// Every item 0..56 answers; the rest warn.
$ok = 0;
for ($i = 0; $i <= 56; $i++) {
    $ok += is_string(nl_langinfo($i));
}
var_dump($ok);
var_dump(nl_langinfo(57), nl_langinfo(-1), nl_langinfo(PHP_INT_MAX));

// CODESET follows LC_CTYPE.
var_dump(setlocale(LC_CTYPE, "0"), nl_langinfo(CODESET));
setlocale(LC_ALL, "C");
var_dump(nl_langinfo(CODESET));
setlocale(LC_CTYPE, "C.UTF-8");
var_dump(nl_langinfo(CODESET));

try {
    nl_langinfo("x");
} catch (\TypeError $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
