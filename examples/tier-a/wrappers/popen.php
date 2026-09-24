<?php
// popen/pclose through the shell.
$p = popen('echo hello; echo world', 'r');
var_dump(get_resource_type($p));
var_dump(fgets($p));
var_dump(stream_get_contents($p));
var_dump(feof($p));
var_dump(stream_get_meta_data($p));
var_dump(pclose($p));
var_dump(pclose(popen('exit 3', 'r')));
$p = popen('cat > popen_out.txt', 'w');
var_dump(fwrite($p, "written\n"));
var_dump(pclose($p));
var_dump(file_get_contents('popen_out.txt'));
unlink('popen_out.txt');
$p = popen('cat', 'wb'); var_dump(stream_get_meta_data($p)['mode']); pclose($p);
$p = popen('true', 'rb'); var_dump(stream_get_meta_data($p)['mode']); pclose($p);
foreach (['x', 'r+', 'rw', 'w+', 'rt'] as $mode) {
    try { popen('true', $mode); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
}
try { popen("a\0b", 'r'); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
$p = popen('true', 'r');
var_dump(fclose($p));
try { pclose($p); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
try { pclose('nope'); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
$p = popen('nonexistent_cmd_xyz 2>/dev/null', 'r');
var_dump(fread($p, 100));
var_dump(pclose($p));
