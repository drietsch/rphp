<?php
$l = sys_getloadavg();
var_dump(is_array($l), count($l), array_keys($l));
foreach ($l as $v) var_dump(is_float($v) && $v >= 0);
try { sys_getloadavg(1); } catch (\ArgumentCountError $e) { echo $e->getMessage(), "\n"; }

var_dump(cli_get_process_title());
var_dump(cli_set_process_title('my worker'));
var_dump(cli_get_process_title());
var_dump(cli_set_process_title("a\0b\0c"));
var_dump(cli_get_process_title());
var_dump(cli_set_process_title(''));
var_dump(cli_get_process_title());
var_dump(cli_set_process_title(42));
var_dump(cli_get_process_title());
try { cli_set_process_title([]); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
