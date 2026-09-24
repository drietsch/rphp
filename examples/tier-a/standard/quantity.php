<?php
$inputs = [
    '1K', '1k', '2M', '1G', '1g', '0x10', '0X1F', '010', '0o10', '0O7', '0b11', '0B1', ' 12 ', "\t7\n",
    'abc', '12x', '', '  ', '-1k', '+5M', '1 g', ' 3 kb', '1kk', '1e2', '08', '0b2', '0z', '0x', '0x0x10',
    '0x 1', '0x-1', '0k', '0', '0 ', '-0x10', '-1', '-9223372036854775808', '9223372036854775807',
    '9223372036854775807k', '9999999999999999999', '99999999999999999999', "\x01\\\n\t\x1b\xff x",
    '-', '+', '128M', '-128m', '2G', '8589934592G',
];
foreach ($inputs as $s) {
    echo json_encode($s), ": ";
    var_dump(ini_parse_quantity($s));
}
try { ini_parse_quantity([]); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump(ini_parse_quantity(64));
