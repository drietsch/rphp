<?php
// timezone_location_get() (DateTimeZone::getLocation()'s alias) and
// timezone_version_get() — the tzdata version is a platform value, so only
// its shape is printed.
print_r(timezone_location_get(new DateTimeZone('Europe/Vienna')));
print_r(timezone_location_get(new DateTimeZone('America/New_York')));
var_dump(timezone_location_get(new DateTimeZone('UTC')));
var_dump(timezone_location_get(new DateTimeZone('+02:00')));
var_dump(timezone_location_get(new DateTimeZone('CEST')));
var_dump(timezone_location_get(timezone_open('Asia/Tokyo')) == (new DateTimeZone('Asia/Tokyo'))->getLocation());
try {
    timezone_location_get('Europe/Vienna');
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}

$v = timezone_version_get();
var_dump(is_string($v), (bool) preg_match('/^(\d{4}\.\d+|0\.system)$/', $v));
