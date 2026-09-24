<?php
// date_parse_from_format(): createFromFormat()'s scanner, reported unresolved.
$cases = [
    ['Y-m-d H:i', '2024-02-30 25:61'],
    ['j.n.Y H:iP', '6.1.2009 13:00+01:00'],
    ['Y-m-d', 'garbage'],
    ['D, d M Y H:i:s T', 'Mon, 15 Aug 2005 15:52:01 CEST'],
    ['U', '1700000000'],
    ['U.u', '1700000000.5'],
    ['U', '-5'],
    ['Y-m-d e', '2020-01-01 Europe/Paris'],
    ['Y-m-d e', '2020-06-01 Mars/Base'],
    ['Y-m-d O', '2020-06-01 +0530'],
    ['Y-m-d+', '2020-01-01 trailing'],
    ['!d', '15'],
    ['Y-m-d|', '2020-01-02'],
    ['Y', '20'], ['Y', '12345'], ['Y', 'x2020'], ['Y-m-d', '2020'], ['Y-m-d', '2020-'],
    ['i', '5'], ['s', '7'], ['H', '7'], ['d', 'x5'], ['m', 'ab'],
    ['Y-m-d H:i', '2020-01-02'], ['Y*d', '2020abc-05'], ['H\h', '10h'], ['H\h', '10x'],
    ['D Y-m-d', 'Mon 2024-01-03'], ['l', 'Friday'], ['D', 'Mo'],
    ['Y-m-d', ''], ['', ''], ['', 'x'], ['Y\\', '2020'],
    ['Y#m#d', '2020/01.02'], ['Y#m', '2020x01'], ['Y?m', '2020x01'],
    ['G:i A', '3:05 PM'], ['h:i a', '12:05 am'], ['h:i A', '11:05 P.M.'], ['h', '13'], ['A', 'pm'],
    ['z Y', '59 2020'], ['Y z', '2020 59'], ['M', 'xii'], ['M', 'Janu'], ['F Y', 'june 2020'],
    ['Y  m', '2020 06'], ['Y m', '2020   06'], ['Y-m-d', '2020x01x02'],
    ['Y-m-d G:i:s.v', '2020-01-02 3:04:05.12'], ['YmdHis', '20200102030405'], ['Ymd', '2020012'],
    ['jS F Y', '1st june 2020'], ['Y-m-dTH:i', '2020-01-02T10:00'], ['?Y', 'x2020'], ['*Y', 'abc 2020'],
    ['N', '1'], ['X-m-d', '-0044-03-15'], ['y', '69'], ['y', '70'],
];
foreach ($cases as [$f, $s]) {
    echo str_pad("$f | $s", 36), json_encode(date_parse_from_format($f, $s)), "\n";
}

// createFromFormat() shares the scanner and its diagnostics.
var_dump(DateTime::createFromFormat('Y-m-d', 'garbage'), DateTime::getLastErrors());
var_dump(DateTime::createFromFormat('!Y-m-d', '2021-02-30')->format('Y-m-d'), DateTime::getLastErrors());
var_dump(DateTime::createFromFormat('!Y-m-d', '2021-02-03')->format(DATE_ATOM), DateTime::getLastErrors());
var_dump(DateTime::createFromFormat('!Y-m-d H:i T', '2020-06-01 10:00 CEST'));
var_dump(date_create_from_format('!D Y-m-d', 'Mon 2024-01-03')->format('Y-m-d l'));

try {
    date_parse_from_format([], '');
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
