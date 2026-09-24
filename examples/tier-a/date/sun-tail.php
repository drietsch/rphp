<?php
// date_sun_info(), and the deprecated date_sunrise()/date_sunset() with the
// SUNFUNCS_RET_* constants (deprecated in 8.4). php's astro.c doubles are
// printed in full.
var_dump(SUNFUNCS_RET_TIMESTAMP, SUNFUNCS_RET_STRING, SUNFUNCS_RET_DOUBLE);
var_dump(ini_get('date.default_latitude'), ini_get('date.default_longitude'));
var_dump(ini_get('date.sunrise_zenith'), ini_get('date.sunset_zenith'));

$ts = 1700000000;
var_dump(date_sunrise($ts, SUNFUNCS_RET_STRING, 38.4, -9, 90, 1));
var_dump(date_sunset($ts, SUNFUNCS_RET_DOUBLE, 38.4, -9, 90, 1));
var_dump(date_sunrise($ts, SUNFUNCS_RET_TIMESTAMP, 38.4, -9));
var_dump(date_sunrise($ts));
var_dump(date_sunset($ts, 2));
var_dump(date_sunset($ts, 0, 89, 0));
var_dump(date_sunset($ts, 1, -89, 0));
var_dump(date_sunset(1718900000, 1, 22, 88, 90.83, 30));
var_dump(date_sunset(1718900000, 1, 22, 88, 90.83, -30));
var_dump(date_sunset(1718900000, 1, 22, 88, 90.83, -5.9));
try {
    date_sunset($ts, 5, 1, 0);
} catch (\ValueError $e) {
    echo $e->getMessage(), "\n";
}

print_r(date_sun_info($ts, 38.4, -9));
print_r(date_sun_info($ts, 89, 0));
print_r(date_sun_info(1718900000, 89, 0));
print_r(date_sun_info(1718900000, 65, 0));
print_r(date_sun_info(strtotime('2006-12-12'), 31.7667, 35.2333));

// The local day comes from the default timezone.
foreach (['Europe/Berlin', 'Asia/Kolkata', 'America/New_York', 'Pacific/Auckland'] as $tz) {
    date_default_timezone_set($tz);
    echo $tz, "\n";
    print_r(date_sun_info(1718900000, 40.7, -74));
    var_dump(date_sunrise(1718900000, SUNFUNCS_RET_DOUBLE, 52.5, 13.4));
    var_dump(date_sunset(1718900000, SUNFUNCS_RET_STRING, 52.5, 13.4));
}
