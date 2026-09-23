<?php
// Writes to a real file land on disk at once, as php's unbuffered plain-file
// writes do: another handle or file_get_contents() sees them before fclose().
$p = sys_get_temp_dir() . '/wt-' . getmypid() . '.txt';
file_put_contents($p, "OLD CONTENT");
$h = fopen($p, 'w'); clearstatcache(); var_dump(file_get_contents($p), filesize($p));
fwrite($h, "abc"); var_dump(file_get_contents($p));
$h2 = fopen($p, 'a'); fwrite($h2, "def"); var_dump(file_get_contents($p));
fwrite($h, "XY"); var_dump(file_get_contents($p));
fseek($h, 10); fwrite($h, "Z"); var_dump(bin2hex(file_get_contents($p)));
ftruncate($h, 4); var_dump(file_get_contents($p));
fclose($h); fclose($h2); var_dump(file_get_contents($p));
$r = fopen($p, 'r+'); fwrite($r, "Q"); var_dump(file_get_contents($p), fread($r, 10)); fclose($r);
$w = fopen($p, 'w+'); fwrite($w, "hello"); rewind($w); var_dump(fread($w, 10)); 
$x = fopen($p . 'x', 'x'); fwrite($x, 'x!'); var_dump(file_get_contents($p.'x')); unlink($p.'x');
$c = fopen($p, 'c+'); var_dump(fread($c, 3)); fwrite($c, "!!"); var_dump(file_get_contents($p));
$t = tmpfile(); fwrite($t, "tmp"); var_dump(file_get_contents(stream_get_meta_data($t)['uri']));
unlink($p);
$u = stream_get_meta_data($t)['uri']; var_dump(stream_get_meta_data($t)['mode']); fclose($t); var_dump(file_exists($u));
$t3 = tmpfile(); $u3 = stream_get_meta_data($t3)["uri"]; var_dump(file_exists($u3));
