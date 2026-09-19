<?php
// strtotime()'s scanner: absolute formats, the relative grammar, and the
// places where the order the rules fire in is observable. The base timestamp
// is fixed (Friday 2009-02-13 23:31:30 UTC) so the output is reproducible.
date_default_timezone_set('UTC');
const BASE = 1234567890;

function show(array $inputs): void {
    foreach ($inputs as $s) {
        $t = @strtotime($s, BASE);
        printf("%-38s %-13s %s\n", $s, $t === false ? 'false' : $t, $t === false ? '' : date('D Y-m-d H:i:s', $t));
    }
}

echo "-- absolute --\n";
show([
    '2008-07-01', '2008-7-1', '08-07-01', '2008/07/01', '7/1/2008', '7/1', '1.7.2008',
    '01.07.08', '13.02.2009', '20080701', '2008182', '2008.182', '2008W27', '2008-W27-3',
    'July 1 2008', '1 July 2008', 'July 2008', '2008 July', 'Jul 1st, 2008', '1st July',
    'Jul-01-2008', '2008-Jul-01', '13.VI.2021', '13.vi.2021',
    '2008-07-01T12:30:45', '2008-07-01T12:30:45Z', '2008-07-01T12:30:45+02:00',
    '2008-07-01T12:30:45.123456+02:00', '20080701T123045', '2000:12:21 16:01:07',
    'Thu, 21 Dec 2000 16:01:07 +0200', 'Friday, 13-Feb-2009 23:31:30 UTC',
    '12:30', '12:30:45', '12:30:45.5', '1230', '123045', '12pm', '12am', '1:30 pm',
    '1.30pm', '11:59:59 PM', '@1234567890', '@-100', '@1234567890.5', '@0 +1 hour',
    '2009', '1999', '0060', '2400', '25:00', '', ' ', 'garbage', 'xyz',
]);

echo "-- relative --\n";
show([
    'now', 'today', 'midnight', 'noon', 'tomorrow', 'yesterday', 'tomorrow noon',
    '3pm tomorrow', 'tomorrow 3pm', 'back of 7', 'front of 7', 'back of 7pm',
    '+1 week', '+1 week 2 days 4 hours 2 seconds', '3 days ago', '1 fortnight',
    '+1 month', '-1 month', '2021-01-31 +1 month', '2021-03-31 -1 month',
    '+1 msec ago', '-1 msec ago',
    'next Thursday', 'last Monday', 'this monday', 'monday', 'next friday',
    '1 friday', '2 sundays', '3 fridays', 'saturday ago', 'monday ago',
    'this week', 'next week', 'last week', 'sunday this week', 'monday this week',
    'weekday', 'weekdays', 'next weekday', '+1 weekday', '+5 weekdays', '-1 weekday',
    'monday next month', 'next month monday', 'monday next year',
    'first day of', 'last day of', 'first day of next month', 'last day of next month',
    'first day of tomorrow', 'last day of february 2021',
    'first monday of january 2021', 'second monday of january 2021',
    'fifth monday of january 2021', 'last monday of january 2021',
    'this monday of january 2021', 'last sunday of january 2021',
    'first monday of next month', 'last monday of next month',
    'first monday of next year', 'first monday of january 2021 +1 day',
    'first week', 'first weeks', '1 sunday of january 2021', 'of january 2021',
]);

echo "-- base and zone --\n";
foreach (['2021-01-15 10:20:30', '2021-06-10 10:20:30', '2020-03-05 10:20:30'] as $b) {
    $base = strtotime($b);
    foreach (['first monday of next month', 'first monday of next year',
              'last monday of next year', 'last day of next month', 'weekday'] as $s) {
        printf("%s %-30s %s\n", $b, $s, date('D Y-m-d H:i:s', strtotime($s, $base)));
    }
}

echo "-- zones in the string --\n";
show([
    '2009-02-13 23:31:30 UTC', '2009-02-13 23:31:30 GMT', '2009-02-13 23:31:30 Z',
    '2009-02-13 23:31:30 EST', '2009-02-13 23:31:30 (CET)', '2009-02-13 23:31:30 CEST',
    '2009-02-13 23:31:30 America/New_York', '2009-02-13 23:31:30 +0200',
    '2009-02-13 23:31:30 -02:30', '2009-02-13 23:31:30 GMT+2', '2009-02-13 23:31:30 gmt+2',
    '2009-02-13 23:31:30 UTC CET', '2009-02-13T23:31:30+25:00', 'X', 'A', 'N',
]);
