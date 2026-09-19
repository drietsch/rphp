<?php
// The timezone model: the default-timezone slot, the DST resolution rule, and
// the two listings php answers from its own bundled database.
date_default_timezone_set('UTC');

echo date_default_timezone_get(), "\n";
var_dump(date_default_timezone_set('Europe/Berlin'), date_default_timezone_get());
// The spelling is kept verbatim, and the ini entry is left alone.
var_dump(date_default_timezone_set('europe/berlin'), date_default_timezone_get(), ini_get('date.timezone'));
var_dump(@date_default_timezone_set('Nowhere/Nothing'), @date_default_timezone_set('+05:00'), date_default_timezone_get());
var_dump(date_default_timezone_set('CET'), date_default_timezone_get());

// A local time in a spring-forward gap takes the offset from before it; one
// in a fall-back overlap takes the offset in force at the same wall clock
// read as UTC, which is the later one in Berlin and the earlier one in
// New York.
foreach (['Europe/Berlin' => ['2021-03-28 01:30', '2021-03-28 02:00', '2021-03-28 02:30',
                              '2021-03-28 03:00', '2021-10-31 01:30', '2021-10-31 02:00',
                              '2021-10-31 02:30', '2021-10-31 03:00'],
          'America/New_York' => ['2021-03-14 01:30', '2021-03-14 02:30', '2021-03-14 03:30',
                                 '2021-11-07 00:30', '2021-11-07 01:30', '2021-11-07 02:30'],
          'Australia/Sydney' => ['2021-10-03 01:30', '2021-10-03 02:30', '2021-04-04 02:30']] as $zone => $times) {
    date_default_timezone_set($zone);
    foreach ($times as $t) {
        $ts = strtotime($t);
        printf("%-18s %-18s %-12d %s\n", $zone, $t, $ts, date('Y-m-d H:i:s T P I', $ts));
    }
}

// Relative amounts are civil, so a day is a day on the wall clock even when
// it is 23 or 25 real hours.
date_default_timezone_set('Europe/Berlin');
foreach (['2021-03-27 12:00 +1 day', '2021-03-27 12:00 +24 hours', '2021-10-30 12:00 +1 day',
          '2021-03-28 00:30 +2 hours', '@1616803200 +1 day'] as $s) {
    printf("%-28s %s\n", $s, date('Y-m-d H:i:s T', strtotime($s)));
}

// mktime() reads the local wall clock, gmmktime() reads UTC. A time inside a
// fall-back overlap is left out on purpose: php breaks that tie with the
// offset in force *now*, so the answer depends on the season it is run in.
printf("%d %d\n", mktime(2, 30, 0, 3, 28, 2021), gmmktime(2, 30, 0, 3, 28, 2021));
printf("%d %d\n", mktime(12, 0, 0, 7, 1, 2021), gmmktime(12, 0, 0, 7, 1, 2021));

// The listings come from php's own database, not the host's.
$all = timezone_identifiers_list();
$bc = timezone_identifiers_list(4095);
printf("all=%d with_bc=%d first=%s last=%s\n", count($all), count($bc), $all[0], $all[count($all) - 1]);
foreach ([1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 0] as $g) {
    printf("group %-5d %4d\n", $g, count(timezone_identifiers_list($g)));
}
print_r(timezone_identifiers_list(4096, 'DE'));
print_r(timezone_identifiers_list(4096, 'NZ'));
print_r(array_slice(timezone_identifiers_list(4095), 0, 8));

$abbr = timezone_abbreviations_list();
printf("abbreviations=%d\n", count($abbr));
print_r($abbr['cest']);
print_r($abbr['z']);
print_r($abbr['acdt']);
