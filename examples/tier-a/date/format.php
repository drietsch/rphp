<?php
// date()'s format characters, and the calendar functions that read the same
// broken-down fields. Fixed timestamps only: the differential suite compares
// byte for byte.
date_default_timezone_set('UTC');

$stamps = [1234567890, 0, -1, 951782400, 1609459200, 253402300799, -62135596800];
$chars = str_split('dDjlNSwzWFmMntLoXxYyaABgGhHisuveIOPpTZcrU');

foreach ($stamps as $ts) {
    $row = [];
    foreach ($chars as $c) {
        $row[] = $c . '=' . date($c, $ts);
    }
    echo $ts, ': ', implode(' ', $row), "\n";
}

// A backslash escapes the next character; an unknown one passes through.
echo date('\\Y Y \\\\ Q q', 1234567890), "\n";
echo date(DATE_ATOM, 1234567890), "\n";
echo date(DATE_ISO8601_EXPANDED, 1234567890), "\n";
echo date(DATE_RFC2822, 1234567890), "\n";
echo date(DATE_COOKIE, 1234567890), "\n";
echo date(DATE_RFC850, 1234567890), "\n";

// gmdate() is UTC by name but GMT by abbreviation.
date_default_timezone_set('Europe/Berlin');
echo gmdate('c e T p P I Z', 1234567890), "\n";
echo date('c e T p P I Z', 1234567890), "\n";
echo date('c e T p P I Z', 1246402800), "\n";

// idate() takes exactly one character and only the numeric ones.
date_default_timezone_set('UTC');
foreach (['Y', 'y', 'm', 'd', 'H', 'i', 's', 'z', 'W', 'N', 'U', 'B', 'L', 't'] as $c) {
    echo $c, '=', var_export(idate($c, 1234567890), true), ' ';
}
echo "\n";
var_dump(@idate('D', 1234567890), @idate('Ym', 1234567890), @idate('', 1234567890));

// mktime() normalizes out-of-range fields and widens two-digit years.
foreach ([[0, 0, 0, 1, 1, 2000], [0, 0, 0, 13, 1, 2000], [0, 0, 0, 1, 32, 2000],
          [25, 70, 70, 1, 1, 2000], [0, 0, 0, 1, 1, 70], [0, 0, 0, 1, 1, 100],
          [0, 0, 0, 1, 1, 101], [0, 0, 0, 1, 1, 69], [0, 0, 0, 2, 30, 2021]] as $a) {
    printf("mktime(%s) = %d %s\n", implode(',', $a), mktime(...$a), date('Y-m-d H:i:s', mktime(...$a)));
}
date_default_timezone_set('Europe/Berlin');
echo mktime(0, 0, 0, 1, 1, 2000), ' ', gmmktime(0, 0, 0, 1, 1, 2000), "\n";

// checkdate() only knows years 1 to 32767.
date_default_timezone_set('UTC');
foreach ([[2, 29, 2021], [2, 29, 2020], [13, 1, 2000], [0, 1, 2000], [1, 0, 2000],
          [1, 1, 0], [1, 1, 32767], [1, 1, 32768], [12, 31, 1999]] as $a) {
    printf("checkdate(%s) = %s\n", implode(',', $a), var_export(checkdate(...$a), true));
}

// getdate() and localtime() report the same instant two ways.
date_default_timezone_set('Europe/Berlin');
print_r(getdate(1234567890));
print_r(localtime(1234567890));
print_r(localtime(1234567890, true));
